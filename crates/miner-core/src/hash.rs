//! Double SHA-256 and the comparison Bitcoin-style miners use for shares.

use sha2::{Digest, Sha256};

/// SHA-256 applied twice. The result is in the internal (little-endian) byte
/// order that block headers and merkle trees use.
pub fn double_sha256(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    Sha256::digest(first).into()
}

/// Reverses a 32-byte digest so it reads as one big-endian number, the way
/// hashes are displayed and compared against a target.
pub fn as_big_endian(digest: &[u8; 32]) -> [u8; 32] {
    let mut out = *digest;
    out.reverse();
    out
}

/// True when the hash, read as a big-endian number, is at or below `target`.
pub fn meets_target(digest: &[u8; 32], target: &[u8; 32]) -> bool {
    as_big_endian(digest) <= *target
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;

    fn be(text: &str) -> [u8; 32] {
        hex::decode(text).unwrap().try_into().unwrap()
    }

    #[test]
    fn genesis_block_hash_matches_published_value() {
        // Header of Bitcoin's genesis block (version 1, time 1231006505,
        // bits 0x1d00ffff, nonce 2083236893).
        let mut header = [0u8; 80];
        header[0..4].copy_from_slice(&1u32.to_le_bytes());
        let merkle = be("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b");
        let mut merkle_internal = merkle;
        merkle_internal.reverse();
        header[36..68].copy_from_slice(&merkle_internal);
        header[68..72].copy_from_slice(&1231006505u32.to_le_bytes());
        header[72..76].copy_from_slice(&0x1d00ffffu32.to_le_bytes());
        header[76..80].copy_from_slice(&2083236893u32.to_le_bytes());

        let digest = double_sha256(&header);
        assert_eq!(
            as_big_endian(&digest),
            be("000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f")
        );
    }

    #[test]
    fn meets_target_compares_big_endian_values() {
        // Target is the big-endian number 1.
        let mut target = [0u8; 32];
        target[31] = 0x01;

        // Internal byte 0 becomes big-endian byte 31, so this digest is 1.
        let mut equal = [0u8; 32];
        equal[0] = 0x01;
        assert!(meets_target(&equal, &target));

        // Internal byte 31 becomes big-endian byte 0, so this digest is 2^248.
        let mut above = [0u8; 32];
        above[31] = 0x01;
        assert!(!meets_target(&above, &target));
    }
}
