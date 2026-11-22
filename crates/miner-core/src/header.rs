//! The 80-byte block header that is hashed for proof of work.

use crate::hash::double_sha256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    pub version: u32,
    /// Previous block hash in internal byte order.
    pub prev_hash: [u8; 32],
    /// Merkle root in internal byte order.
    pub merkle_root: [u8; 32],
    pub time: u32,
    pub bits: u32,
    pub nonce: u32,
}

impl BlockHeader {
    pub fn to_bytes(&self) -> [u8; 80] {
        let mut out = [0u8; 80];
        out[0..4].copy_from_slice(&self.version.to_le_bytes());
        out[4..36].copy_from_slice(&self.prev_hash);
        out[36..68].copy_from_slice(&self.merkle_root);
        out[68..72].copy_from_slice(&self.time.to_le_bytes());
        out[72..76].copy_from_slice(&self.bits.to_le_bytes());
        out[76..80].copy_from_slice(&self.nonce.to_le_bytes());
        out
    }

    /// Double SHA-256 of the serialized header, in internal byte order.
    pub fn pow_digest(&self) -> [u8; 32] {
        double_sha256(&self.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_fields_in_wire_order() {
        let header = BlockHeader {
            version: 0x2000_0000,
            prev_hash: [0x11; 32],
            merkle_root: [0x22; 32],
            time: 0x6000_0001,
            bits: 0x1703_0000,
            nonce: 0xdead_beef,
        };
        let bytes = header.to_bytes();
        assert_eq!(&bytes[0..4], &[0x00, 0x00, 0x00, 0x20]);
        assert_eq!(&bytes[4..36], &[0x11; 32]);
        assert_eq!(&bytes[36..68], &[0x22; 32]);
        assert_eq!(&bytes[68..72], &[0x01, 0x00, 0x00, 0x60]);
        assert_eq!(&bytes[76..80], &[0xef, 0xbe, 0xad, 0xde]);
    }
}
