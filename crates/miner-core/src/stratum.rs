//! Stratum v1 messages: parsing `mining.notify` and `subscribe` results, building
//! a block header from a job, and encoding `subscribe`, `authorize` and `submit`.
//!
//! Conventions follow the usual Stratum v1 implementations (cgminer, bfgminer).
//! The previous-block hash is sent with each 32-bit word byte-reversed, and
//! `version`, `nbits`, `ntime` and `nonce` are big-endian hex integers. Check
//! these against a live pool before trusting shares from this code.

use serde_json::{Value, json};
use thiserror::Error;

use crate::hash::double_sha256;
use crate::header::BlockHeader;
use crate::hex;
use crate::target::target_from_bits;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StratumError {
    #[error("malformed field `{0}`")]
    Malformed(&'static str),
}

/// A `mining.notify` job, with hex fields kept as sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notify {
    pub job_id: String,
    pub prev_hash: String,
    pub coinb1: String,
    pub coinb2: String,
    pub merkle_branch: Vec<String>,
    pub version: u32,
    pub nbits: u32,
    pub ntime: u32,
    pub clean_jobs: bool,
}

/// The `mining.subscribe` result: extranonce1 and the extranonce2 length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub extranonce1: Vec<u8>,
    pub extranonce2_size: usize,
}

impl Subscription {
    /// Parses the `result` array of a `mining.subscribe` response.
    pub fn from_result(result: &Value) -> Result<Self, StratumError> {
        let items = result.as_array().ok_or(StratumError::Malformed("result"))?;
        let extranonce1 = items
            .get(1)
            .and_then(Value::as_str)
            .and_then(hex::decode)
            .ok_or(StratumError::Malformed("extranonce1"))?;
        let extranonce2_size = items
            .get(2)
            .and_then(Value::as_u64)
            .ok_or(StratumError::Malformed("extranonce2_size"))?;
        Ok(Self {
            extranonce1,
            extranonce2_size: usize::try_from(extranonce2_size)
                .map_err(|_| StratumError::Malformed("extranonce2_size"))?,
        })
    }
}

impl Notify {
    /// Parses the `params` array of a `mining.notify` message.
    pub fn from_params(params: &Value) -> Result<Self, StratumError> {
        let items = params.as_array().ok_or(StratumError::Malformed("params"))?;
        let text = |index: usize, name: &'static str| {
            items
                .get(index)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(StratumError::Malformed(name))
        };
        let hex_u32 = |index: usize, name: &'static str| {
            items
                .get(index)
                .and_then(Value::as_str)
                .and_then(|value| u32::from_str_radix(value, 16).ok())
                .ok_or(StratumError::Malformed(name))
        };

        let merkle_branch = items
            .get(4)
            .and_then(Value::as_array)
            .ok_or(StratumError::Malformed("merkle_branch"))?
            .iter()
            .map(|entry| entry.as_str().map(str::to_owned))
            .collect::<Option<Vec<String>>>()
            .ok_or(StratumError::Malformed("merkle_branch"))?;

        Ok(Self {
            job_id: text(0, "job_id")?,
            prev_hash: text(1, "prev_hash")?,
            coinb1: text(2, "coinb1")?,
            coinb2: text(3, "coinb2")?,
            merkle_branch,
            version: hex_u32(5, "version")?,
            nbits: hex_u32(6, "nbits")?,
            ntime: hex_u32(7, "ntime")?,
            clean_jobs: items
                .get(8)
                .and_then(Value::as_bool)
                .ok_or(StratumError::Malformed("clean_jobs"))?,
        })
    }

    /// Builds the header for one extranonce2 choice, with nonce 0. The caller
    /// sets the nonce per hash attempt.
    pub fn header(
        &self,
        extranonce1: &[u8],
        extranonce2: &[u8],
    ) -> Result<BlockHeader, StratumError> {
        let mut coinbase = hex::decode(&self.coinb1).ok_or(StratumError::Malformed("coinb1"))?;
        coinbase.extend_from_slice(extranonce1);
        coinbase.extend_from_slice(extranonce2);
        coinbase.extend(hex::decode(&self.coinb2).ok_or(StratumError::Malformed("coinb2"))?);

        let mut root = double_sha256(&coinbase);
        for branch in &self.merkle_branch {
            let node = hex::decode(branch).ok_or(StratumError::Malformed("merkle_branch"))?;
            let mut pair = root.to_vec();
            pair.extend_from_slice(&node);
            root = double_sha256(&pair);
        }

        Ok(BlockHeader {
            version: self.version,
            prev_hash: prev_hash_from_stratum(&self.prev_hash)?,
            merkle_root: root,
            time: self.ntime,
            bits: self.nbits,
            nonce: 0,
        })
    }

    /// The share target implied by `nbits`, or an error for an invalid encoding.
    pub fn target(&self) -> Result<[u8; 32], StratumError> {
        target_from_bits(self.nbits).ok_or(StratumError::Malformed("nbits"))
    }
}

/// Converts the Stratum previous-hash field to internal byte order by
/// reversing each 4-byte word.
pub fn prev_hash_from_stratum(text: &str) -> Result<[u8; 32], StratumError> {
    let bytes = hex::decode(text).ok_or(StratumError::Malformed("prev_hash"))?;
    let mut out = [0u8; 32];
    if bytes.len() != 32 {
        return Err(StratumError::Malformed("prev_hash"));
    }
    for (chunk_in, chunk_out) in bytes.chunks(4).zip(out.chunks_mut(4)) {
        for (dst, src) in chunk_out.iter_mut().zip(chunk_in.iter().rev()) {
            *dst = *src;
        }
    }
    Ok(out)
}

/// Builds a `mining.subscribe` request.
pub fn subscribe_request(id: u64) -> Value {
    json!({ "id": id, "method": "mining.subscribe", "params": ["openmineros/0.1.0"] })
}

/// Builds a `mining.authorize` request.
pub fn authorize_request(id: u64, user: &str, password: &str) -> Value {
    json!({ "id": id, "method": "mining.authorize", "params": [user, password] })
}

/// Builds a `mining.submit` request for a share found with `nonce`.
pub fn submit_request(
    id: u64,
    user: &str,
    job_id: &str,
    extranonce2: &[u8],
    ntime: u32,
    nonce: u32,
) -> Value {
    json!({
        "id": id,
        "method": "mining.submit",
        "params": [
            user,
            job_id,
            hex::encode(extranonce2),
            format!("{ntime:08x}"),
            format!("{nonce:08x}"),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_notify_params() -> Value {
        json!([
            "job1",
            "4d16b6f85af6e2198f44ae2a6de67f78487ae5611b77c6c0440b921e00000000",
            "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff",
            "ffffffff0100f2052a010000001976a914000000000000000000000000000000000000000088ac00000000",
            [],
            "20000000",
            "1d00ffff",
            "504e86b9",
            true
        ])
    }

    #[test]
    fn parses_a_notify_message() {
        let notify = Notify::from_params(&sample_notify_params()).unwrap();
        assert_eq!(notify.job_id, "job1");
        assert_eq!(notify.version, 0x2000_0000);
        assert_eq!(notify.nbits, 0x1d00_ffff);
        assert_eq!(notify.ntime, 0x504e_86b9);
        assert!(notify.clean_jobs);
        assert!(notify.merkle_branch.is_empty());
    }

    #[test]
    fn rejects_a_truncated_notify() {
        let mut params = sample_notify_params();
        params.as_array_mut().unwrap().truncate(5);
        assert!(matches!(
            Notify::from_params(&params),
            Err(StratumError::Malformed(_))
        ));
    }

    #[test]
    fn prev_hash_reverses_each_word() {
        let text = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let out = prev_hash_from_stratum(text).unwrap();
        assert_eq!(&out[0..4], &[0x04, 0x03, 0x02, 0x01]);
        assert_eq!(&out[28..32], &[0x20, 0x1f, 0x1e, 0x1d]);
    }

    #[test]
    fn merkle_root_without_branches_is_the_coinbase_hash() {
        let notify = Notify::from_params(&sample_notify_params()).unwrap();
        let extranonce1 = [0xaa, 0xbb, 0xcc, 0xdd];
        let extranonce2 = [0x00, 0x00, 0x00, 0x01];
        let header = notify.header(&extranonce1, &extranonce2).unwrap();

        let mut coinbase = hex::decode(&notify.coinb1).unwrap();
        coinbase.extend_from_slice(&extranonce1);
        coinbase.extend_from_slice(&extranonce2);
        coinbase.extend(hex::decode(&notify.coinb2).unwrap());
        assert_eq!(header.merkle_root, double_sha256(&coinbase));
        assert_eq!(header.nonce, 0);
        assert_eq!(header.bits, 0x1d00_ffff);
    }

    #[test]
    fn parses_subscribe_result() {
        let result = json!([[["mining.set_difficulty", "sub1"]], "08000002", 4]);
        let subscription = Subscription::from_result(&result).unwrap();
        assert_eq!(subscription.extranonce1, vec![0x08, 0x00, 0x00, 0x02]);
        assert_eq!(subscription.extranonce2_size, 4);
    }

    #[test]
    fn submit_request_uses_fixed_width_hex() {
        let request = submit_request(7, "addr.worker", "job1", &[0x00, 0x01], 0x504e_86b9, 0x0a);
        assert_eq!(request["method"], "mining.submit");
        assert_eq!(request["params"][2], "0001");
        assert_eq!(request["params"][3], "504e86b9");
        assert_eq!(request["params"][4], "0000000a");
    }
}
