//! Protocol state machine for one pool connection. It takes Stratum messages in,
//! hands work to a [`ChipBackend`], and returns the `mining.submit` requests to
//! send. It does no I/O, so the pool driver in `pool.rs` and the tests can both
//! use it.

use serde_json::Value;
use thiserror::Error;

use crate::chip::{ChipBackend, ChipError};
use crate::hash::meets_target;
use crate::header::BlockHeader;
use crate::stratum::{Notify, StratumError, Subscription, submit_request};
use crate::target::{target_from_bits, target_from_pool_difficulty};

/// Request id used for `mining.subscribe`.
pub const SUBSCRIBE_ID: u64 = 1;
/// Request id used for `mining.authorize`.
pub const AUTHORIZE_ID: u64 = 2;
/// First id used for `mining.submit`. Responses at or above this are shares.
pub const FIRST_SUBMIT_ID: u64 = 100;

#[derive(Debug, Error)]
pub enum MinerError {
    #[error(transparent)]
    Stratum(#[from] StratumError),
    #[error(transparent)]
    Chip(#[from] ChipError),
    #[error("pool rejected the worker authorization")]
    AuthorizeRejected,
    #[error("pool message is missing `{0}`")]
    Protocol(&'static str),
}

/// The work currently loaded into the backend.
#[derive(Debug, Clone)]
struct Work {
    job_id: String,
    header: BlockHeader,
    target: [u8; 32],
    extranonce2: Vec<u8>,
}

pub struct Miner<B: ChipBackend> {
    backend: B,
    user: String,
    subscription: Option<Subscription>,
    pool_target: Option<[u8; 32]>,
    job: Option<Notify>,
    work: Option<Work>,
    extranonce2_counter: u64,
    next_submit_id: u64,
    pub accepted: u64,
    pub rejected: u64,
}

impl<B: ChipBackend> Miner<B> {
    pub fn new(backend: B, user: impl Into<String>) -> Self {
        Self {
            backend,
            user: user.into(),
            subscription: None,
            pool_target: None,
            job: None,
            work: None,
            extranonce2_counter: 0,
            next_submit_id: FIRST_SUBMIT_ID,
            accepted: 0,
            rejected: 0,
        }
    }

    /// Handles one message from the pool and returns the requests to send back.
    pub fn on_message(&mut self, message: &Value) -> Result<Vec<Value>, MinerError> {
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            match method {
                "mining.notify" => {
                    let notify = Notify::from_params(&message["params"])?;
                    self.new_job(notify)?;
                }
                "mining.set_difficulty" => {
                    let difficulty = message["params"]
                        .get(0)
                        .and_then(Value::as_f64)
                        .ok_or(MinerError::Protocol("difficulty"))?;
                    self.set_difficulty(difficulty)?;
                }
                _ => {}
            }
            return Ok(Vec::new());
        }

        match message.get("id").and_then(Value::as_u64) {
            Some(SUBSCRIBE_ID) => {
                self.subscription = Some(Subscription::from_result(&message["result"])?);
                self.load_work()?;
            }
            Some(AUTHORIZE_ID) => {
                if message["result"].as_bool() != Some(true) {
                    return Err(MinerError::AuthorizeRejected);
                }
            }
            Some(id) if id >= FIRST_SUBMIT_ID => {
                if message["result"].as_bool() == Some(true) {
                    self.accepted += 1;
                } else {
                    self.rejected += 1;
                }
            }
            _ => {}
        }
        Ok(Vec::new())
    }

    /// Asks the backend for nonces and returns a submit request for each one
    /// that meets the share target. Every nonce is checked in software first.
    pub fn poll(&mut self) -> Result<Vec<Value>, MinerError> {
        let nonces = self.backend.poll_nonces()?;
        let Some(work) = self.work.clone() else {
            return Ok(Vec::new());
        };

        let mut submits = Vec::new();
        for nonce in nonces {
            let mut header = work.header;
            header.nonce = nonce;
            if !meets_target(&header.pow_digest(), &work.target) {
                continue;
            }
            let id = self.next_submit_id;
            self.next_submit_id += 1;
            submits.push(submit_request(
                id,
                &self.user,
                &work.job_id,
                &work.extranonce2,
                header.time,
                nonce,
            ));
        }
        Ok(submits)
    }

    /// The share target currently in force, if the pool has sent a difficulty.
    pub fn share_target(&self) -> Option<[u8; 32]> {
        self.work.as_ref().map(|work| work.target)
    }

    fn set_difficulty(&mut self, difficulty: f64) -> Result<(), MinerError> {
        self.pool_target = Some(
            target_from_pool_difficulty(difficulty).ok_or(MinerError::Protocol("difficulty"))?,
        );
        self.load_work()
    }

    fn new_job(&mut self, notify: Notify) -> Result<(), MinerError> {
        // A new job has a new coinbase, so extranonce2 starts again.
        self.extranonce2_counter = 0;
        self.job = Some(notify);
        self.load_work()
    }

    /// Loads the current job into the backend. Nothing is loaded until both a
    /// job and a pool difficulty are known, because shares before that could be
    /// rejected.
    fn load_work(&mut self) -> Result<(), MinerError> {
        let (Some(notify), Some(subscription), Some(pool_target)) =
            (&self.job, &self.subscription, self.pool_target)
        else {
            self.work = None;
            return Ok(());
        };

        let extranonce2 =
            extranonce2_bytes(self.extranonce2_counter, subscription.extranonce2_size);
        let header = notify.header(&subscription.extranonce1, &extranonce2)?;
        let block_target =
            target_from_bits(notify.nbits).ok_or(StratumError::Malformed("nbits"))?;
        let target = pool_target.min(block_target);

        self.backend.load_work(header, target)?;
        self.work = Some(Work {
            job_id: notify.job_id.clone(),
            header,
            target,
            extranonce2,
        });
        Ok(())
    }
}

/// Big-endian encoding of the extranonce2 counter, trimmed or padded to the
/// width the pool asked for.
fn extranonce2_bytes(counter: u64, size: usize) -> Vec<u8> {
    let all = counter.to_be_bytes();
    if size >= all.len() {
        let mut padded = vec![0u8; size - all.len()];
        padded.extend_from_slice(&all);
        padded
    } else {
        all[all.len() - size..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::SoftwareBackend;
    use crate::hash::double_sha256;
    use crate::hex;
    use serde_json::json;

    /// Easy block target, so the software backend finds shares in a few hashes.
    fn job_params() -> Value {
        json!([
            "job1",
            "4d16b6f85af6e2198f44ae2a6de67f78487ae5611b77c6c0440b921e00000000",
            "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff",
            "ffffffff0100f2052a010000001976a914000000000000000000000000000000000000000088ac00000000",
            [],
            "20000000",
            "2007ffff",
            "504e86b9",
            true
        ])
    }

    fn subscribe_response() -> Value {
        json!({
            "id": SUBSCRIBE_ID,
            "result": [[["mining.set_difficulty", "s"]], "08000002", 4],
            "error": null
        })
    }

    fn notify_message() -> Value {
        json!({ "id": null, "method": "mining.notify", "params": job_params() })
    }

    fn difficulty_message(difficulty: f64) -> Value {
        json!({ "id": null, "method": "mining.set_difficulty", "params": [difficulty] })
    }

    fn miner() -> Miner<SoftwareBackend> {
        Miner::new(SoftwareBackend::new(0, 16), "addr.worker")
    }

    #[test]
    fn finds_shares_that_verify_against_the_target() {
        let mut miner = miner();
        miner.on_message(&subscribe_response()).unwrap();
        // Difficulty 2^-27 gives a target about 1 in 32 hashes, so shares
        // appear within a few dozen polls.
        miner
            .on_message(&difficulty_message(1.0 / 134_217_728.0))
            .unwrap();
        miner.on_message(&notify_message()).unwrap();

        let mut submits = Vec::new();
        for _ in 0..10_000 {
            submits = miner.poll().unwrap();
            if !submits.is_empty() {
                break;
            }
        }
        assert!(!submits.is_empty(), "expected shares at the easy target");

        let target = miner.share_target().unwrap();
        for submit in &submits {
            let params = &submit["params"];
            assert_eq!(params[0], "addr.worker");
            assert_eq!(params[1], "job1");
            let nonce = u32::from_str_radix(params[4].as_str().unwrap(), 16).unwrap();
            let extranonce2 = hex::decode(params[2].as_str().unwrap()).unwrap();

            let notify = Notify::from_params(&job_params()).unwrap();
            let subscription = Subscription::from_result(&subscribe_response()["result"]).unwrap();
            let mut header = notify
                .header(&subscription.extranonce1, &extranonce2)
                .unwrap();
            header.nonce = nonce;
            let digest = double_sha256(&header.to_bytes());
            assert!(meets_target(&digest, &target));
        }
    }

    #[test]
    fn counts_pool_responses_to_submits() {
        let mut miner = miner();
        miner
            .on_message(&json!({ "id": FIRST_SUBMIT_ID, "result": true, "error": null }))
            .unwrap();
        miner
            .on_message(&json!({ "id": FIRST_SUBMIT_ID + 1, "result": false, "error": [23, "stale", null] }))
            .unwrap();
        assert_eq!((miner.accepted, miner.rejected), (1, 1));
    }

    #[test]
    fn stops_when_authorization_is_rejected() {
        let mut miner = miner();
        let result =
            miner.on_message(&json!({ "id": AUTHORIZE_ID, "result": false, "error": null }));
        assert!(matches!(result, Err(MinerError::AuthorizeRejected)));
    }

    #[test]
    fn waits_for_a_difficulty_before_loading_work() {
        let mut miner = miner();
        miner.on_message(&subscribe_response()).unwrap();
        miner.on_message(&notify_message()).unwrap();
        assert!(miner.share_target().is_none());
        assert!(miner.poll().unwrap().is_empty());
    }

    #[test]
    fn extranonce2_is_fixed_width_big_endian() {
        assert_eq!(extranonce2_bytes(0x0102, 4), vec![0, 0, 1, 2]);
        assert_eq!(extranonce2_bytes(0x0102, 1), vec![2]);
        assert_eq!(extranonce2_bytes(1, 0), Vec::<u8>::new());
    }
}
