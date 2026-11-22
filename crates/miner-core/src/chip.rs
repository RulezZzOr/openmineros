//! Boundary between the mining core and a hashing backend.
//!
//! A backend receives one header template with a target and reports nonces
//! that meet it. The core verifies every reported nonce again in software before
//! it becomes a share, so a faulty backend cannot submit a bad share.

use thiserror::Error;

use crate::hash::{double_sha256, meets_target};
use crate::header::BlockHeader;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChipError {
    #[error("backend `{0}` is not implemented in this build")]
    Unimplemented(&'static str),
}

pub trait ChipBackend {
    /// Replaces the current work. `header.nonce` is ignored; the backend
    /// chooses the nonces it searches.
    fn load_work(&mut self, header: BlockHeader, target: [u8; 32]) -> Result<(), ChipError>;

    /// Returns nonces found since the last call that meet the target.
    fn poll_nonces(&mut self) -> Result<Vec<u32>, ChipError>;
}

/// Software reference backend. It scans a bounded nonce window on each poll,
/// so it is only useful for tests and for checking the verification path.
#[derive(Debug, Default)]
pub struct SoftwareBackend {
    work: Option<(BlockHeader, [u8; 32])>,
    next_nonce: u32,
    batch: u32,
}

impl SoftwareBackend {
    /// Starts scanning at `start_nonce`, checking `batch` nonces per poll.
    pub fn new(start_nonce: u32, batch: u32) -> Self {
        Self {
            work: None,
            next_nonce: start_nonce,
            batch,
        }
    }
}

impl ChipBackend for SoftwareBackend {
    fn load_work(&mut self, header: BlockHeader, target: [u8; 32]) -> Result<(), ChipError> {
        self.work = Some((header, target));
        Ok(())
    }

    fn poll_nonces(&mut self) -> Result<Vec<u32>, ChipError> {
        let Some((mut header, target)) = self.work else {
            return Ok(Vec::new());
        };
        let mut found = Vec::new();
        for _ in 0..self.batch {
            header.nonce = self.next_nonce;
            if meets_target(&double_sha256(&header.to_bytes()), &target) {
                found.push(header.nonce);
            }
            self.next_nonce = self.next_nonce.wrapping_add(1);
        }
        Ok(found)
    }
}

/// Placeholder for the Antminer BM1398 hashboard path. The chip protocol and
/// the register map are not verified yet, so this backend refuses all work.
#[derive(Debug, Default)]
pub struct Bm1398Backend;

impl ChipBackend for Bm1398Backend {
    fn load_work(&mut self, _header: BlockHeader, _target: [u8; 32]) -> Result<(), ChipError> {
        Err(ChipError::Unimplemented("bm1398"))
    }

    fn poll_nonces(&mut self) -> Result<Vec<u32>, ChipError> {
        Err(ChipError::Unimplemented("bm1398"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::target_from_bits;

    fn genesis_header(nonce: u32) -> BlockHeader {
        let mut merkle_root =
            hex_to_array("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b");
        merkle_root.reverse();
        BlockHeader {
            version: 1,
            prev_hash: [0; 32],
            merkle_root,
            time: 1_231_006_505,
            bits: 0x1d00_ffff,
            nonce,
        }
    }

    fn hex_to_array(text: &str) -> [u8; 32] {
        crate::hex::decode(text).unwrap().try_into().unwrap()
    }

    #[test]
    fn software_backend_finds_the_genesis_nonce() {
        let target = target_from_bits(0x1d00_ffff).unwrap();
        let genesis_nonce = 2_083_236_893u32;
        let mut backend = SoftwareBackend::new(genesis_nonce - 3, 8);
        backend.load_work(genesis_header(0), target).unwrap();
        assert_eq!(backend.poll_nonces().unwrap(), vec![genesis_nonce]);
    }

    #[test]
    fn software_backend_is_idle_without_work() {
        let mut backend = SoftwareBackend::new(0, 16);
        assert!(backend.poll_nonces().unwrap().is_empty());
    }

    #[test]
    fn bm1398_backend_refuses_work() {
        let mut backend = Bm1398Backend;
        assert_eq!(
            backend.load_work(genesis_header(0), [0; 32]),
            Err(ChipError::Unimplemented("bm1398"))
        );
        assert_eq!(
            backend.poll_nonces(),
            Err(ChipError::Unimplemented("bm1398"))
        );
    }
}
