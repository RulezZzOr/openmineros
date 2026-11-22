//! Tokio driver for one Stratum v1 pool connection. It sends the handshake,
//! feeds every pool line to [`Miner`], polls the backend, and writes the
//! submits back. Messages are newline-delimited JSON.

use std::time::Duration;

use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::chip::ChipBackend;
use crate::miner::{AUTHORIZE_ID, Miner, MinerError, SUBSCRIBE_ID};
use crate::stratum::{authorize_request, subscribe_request};

/// How often the backend is polled when the pool is quiet.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Error)]
pub enum PoolError {
    #[error("I/O error on the pool connection: {0}")]
    Io(#[from] std::io::Error),
    #[error("pool sent invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("pool closed the connection")]
    Closed,
    #[error(transparent)]
    Miner(#[from] MinerError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunSummary {
    pub accepted: u64,
    pub rejected: u64,
}

/// Connects to `addr`, mines until `max_accepted` shares are accepted, and
/// returns the final counts. Use `u64::MAX` to run until the connection drops.
pub async fn run<B: ChipBackend>(
    addr: &str,
    user: &str,
    password: &str,
    backend: B,
    max_accepted: u64,
) -> Result<RunSummary, PoolError> {
    let stream = TcpStream::connect(addr).await?;
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let mut miner = Miner::new(backend, user);

    send(&mut write_half, &subscribe_request(SUBSCRIBE_ID)).await?;
    send(
        &mut write_half,
        &authorize_request(AUTHORIZE_ID, user, password),
    )
    .await?;

    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    loop {
        if miner.accepted >= max_accepted {
            return Ok(RunSummary {
                accepted: miner.accepted,
                rejected: miner.rejected,
            });
        }

        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else {
                    return Err(PoolError::Closed);
                };
                let message: Value = serde_json::from_str(&line)?;
                for submit in miner.on_message(&message)? {
                    send(&mut write_half, &submit).await?;
                }
            }
            _ = ticker.tick() => {
                for submit in miner.poll()? {
                    send(&mut write_half, &submit).await?;
                }
            }
        }
    }
}

async fn send<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) -> Result<(), PoolError> {
    let mut line = message.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::SoftwareBackend;
    use serde_json::json;
    use tokio::net::TcpListener;

    /// A pool that accepts every share. It sends difficulty 2^-27 and an easy job,
    /// then answers `mining.submit` with `result: true`.
    async fn mock_pool(listener: TcpListener) {
        let (socket, _) = listener.accept().await.unwrap();
        let (read_half, mut write_half) = socket.into_split();
        let mut lines = BufReader::new(read_half).lines();

        let subscribe: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        send(
            &mut write_half,
            &json!({
                "id": subscribe["id"],
                "result": [[["mining.set_difficulty", "s"]], "08000002", 4],
                "error": null
            }),
        )
        .await
        .unwrap();

        let authorize: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        send(
            &mut write_half,
            &json!({ "id": authorize["id"], "result": true, "error": null }),
        )
        .await
        .unwrap();

        send(
            &mut write_half,
            &json!({ "id": null, "method": "mining.set_difficulty", "params": [1.0 / 134_217_728.0] }),
        )
        .await
        .unwrap();
        send(&mut write_half, &json!({
            "id": null,
            "method": "mining.notify",
            "params": [
                "job1",
                "4d16b6f85af6e2198f44ae2a6de67f78487ae5611b77c6c0440b921e00000000",
                "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff",
                "ffffffff0100f2052a010000001976a914000000000000000000000000000000000000000088ac00000000",
                [],
                "20000000",
                "2007ffff",
                "504e86b9",
                true
            ]
        }))
        .await
        .unwrap();

        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            if request["method"] == "mining.submit" {
                send(
                    &mut write_half,
                    &json!({ "id": request["id"], "result": true, "error": null }),
                )
                .await
                .unwrap();
            }
        }
    }

    #[tokio::test]
    async fn mines_and_gets_shares_accepted_by_a_pool() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let pool = tokio::spawn(mock_pool(listener));

        let summary = tokio::time::timeout(
            Duration::from_secs(30),
            run(&addr, "addr.worker", "x", SoftwareBackend::new(0, 16), 1),
        )
        .await
        .expect("driver timed out")
        .expect("driver failed");

        assert!(summary.accepted >= 1);
        assert_eq!(summary.rejected, 0);
        pool.abort();
    }

    #[tokio::test]
    async fn reports_a_closed_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let closer = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            drop(socket);
        });

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            run(&addr, "u", "p", SoftwareBackend::new(0, 1), u64::MAX),
        )
        .await
        .expect("driver timed out");
        // The pool may close before our first write lands, which shows up as a
        // broken pipe rather than a clean end-of-stream. Both mean the same thing here.
        assert!(matches!(
            result,
            Err(PoolError::Closed) | Err(PoolError::Io(_))
        ));
        closer.await.unwrap();
    }
}
