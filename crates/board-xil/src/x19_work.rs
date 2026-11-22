//! Work frames for the X19 work-tx FIFO and nonce words from work-rx.
//!
//! Frame layout (hypothesis under test, after the Braiins s9io core):
//! `[ext_work_id, nbits, ntime, merkle_tail, midstate 0 (8 words), …]`.
//! The order of the eight words inside a midstate is the open question, so
//! it is a parameter ([`MidstateOrder`]) until a hardware test settles it.

use serde::Serialize;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (i, c) in block.chunks_exact(4).enumerate() {
        w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}

/// SHA-256 state after the first 64 header bytes.
pub fn midstate(header: &[u8; 80]) -> [u32; 8] {
    let mut state = H0;
    compress(&mut state, header[..64].try_into().unwrap());
    state
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state = H0;
    let mut msg = data.to_vec();
    let bits = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks_exact(64) {
        compress(&mut state, block.try_into().unwrap());
    }
    let mut out = [0u8; 32];
    for (o, s) in out.chunks_exact_mut(4).zip(state) {
        o.copy_from_slice(&s.to_be_bytes());
    }
    out
}

/// Number of leading zero bits of the block hash (as Bitcoin displays it).
pub fn hash_zero_bits(header: &[u8; 80], nonce: u32) -> u32 {
    let mut h = *header;
    h[76..80].copy_from_slice(&nonce.to_le_bytes());
    let digest = sha256(&sha256(&h));
    let mut zeros = 0;
    for byte in digest.iter().rev() {
        if *byte == 0 {
            zeros += 8;
        } else {
            zeros += byte.leading_zeros();
            break;
        }
    }
    zeros
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MidstateOrder {
    /// H0..H7 as numeric words.
    Forward,
    /// H7..H0 as numeric words.
    Reversed,
    /// H0..H7, each word byte-swapped.
    ForwardSwapped,
    /// H7..H0, each word byte-swapped.
    ReversedSwapped,
}

impl MidstateOrder {
    pub const ALL: [Self; 4] = [
        Self::Forward,
        Self::Reversed,
        Self::ForwardSwapped,
        Self::ReversedSwapped,
    ];

    fn words(self, m: &[u32; 8]) -> [u32; 8] {
        let mut w = *m;
        if matches!(self, Self::Reversed | Self::ReversedSwapped) {
            w.reverse();
        }
        if matches!(self, Self::ForwardSwapped | Self::ReversedSwapped) {
            for x in &mut w {
                *x = x.swap_bytes();
            }
        }
        w
    }
}

fn le_word(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Builds one work-tx frame for `header`, repeating its midstate `slots` times.
pub fn frame(ext_id: u32, header: &[u8; 80], slots: usize, order: MidstateOrder) -> Vec<u32> {
    let mut out = vec![
        ext_id,
        le_word(&header[72..76]),
        le_word(&header[68..72]),
        le_word(&header[64..68]),
    ];
    let m = order.words(&midstate(header));
    for _ in 0..slots {
        out.extend_from_slice(&m);
    }
    out
}

/// Builds one frame from several headers that differ only in `version`
/// (rolled versions share the second SHA-256 block). One midstate per header.
pub fn frame_versions(ext_id: u32, headers: &[[u8; 80]], order: MidstateOrder) -> Vec<u32> {
    let h = &headers[0];
    let mut out = vec![
        ext_id,
        le_word(&h[72..76]),
        le_word(&h[68..72]),
        le_word(&h[64..68]),
    ];
    for hdr in headers {
        out.extend_from_slice(&order.words(&midstate(hdr)));
    }
    out
}

/// Version for midstate slot `slot`: the masked bits of `base` plus `slot`.
pub fn rolled_version(base: u32, mask: u32, slot: u32) -> u32 {
    if mask == 0 {
        return base;
    }
    let shift = mask.trailing_zeros();
    let field = ((base & mask) >> shift).wrapping_add(slot);
    (base & !mask) | ((field << shift) & mask)
}

/// One work-rx answer (two FIFO words).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NonceWord {
    pub nonce: u32,
    pub solution: u8,
    pub work_id: u16,
    pub crc: u8,
}

pub fn decode_nonce(w0: u32, w1: u32) -> NonceWord {
    NonceWord {
        nonce: w0,
        solution: (w1 & 0xFF) as u8,
        work_id: ((w1 >> 8) & 0xFFFF) as u16,
        crc: (w1 >> 24) as u8,
    }
}

/// Deterministic test header number `n`: fixed prev-hash and merkle root,
/// ntime advanced by `n` so every frame is distinct work.
pub fn test_header(n: u32) -> [u8; 80] {
    let mut h = [0u8; 80];
    h[0..4].copy_from_slice(&0x2000_0000u32.to_le_bytes());
    for (i, b) in h[4..68].iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(37).wrapping_add(11);
    }
    h[68..72].copy_from_slice(&(0x6740_0000u32 + n).to_le_bytes());
    h[72..76].copy_from_slice(&0x1703_a30cu32.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genesis() -> [u8; 80] {
        let hex = "0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c";
        let mut h = [0u8; 80];
        for (i, b) in h.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap();
        }
        h
    }

    #[test]
    fn genesis_block_has_its_known_difficulty() {
        let g = genesis();
        let nonce = le_word(&g[76..80]);
        // 000000000019d668… has 43 leading zero bits.
        assert_eq!(hash_zero_bits(&g, nonce), 43);
        assert!(hash_zero_bits(&g, nonce ^ 1) < 20);
    }

    #[test]
    fn sha256_matches_the_empty_string_vector() {
        assert_eq!(sha256(b"")[..4], [0xe3, 0xb0, 0xc4, 0x42],);
    }

    #[test]
    fn frame_is_header_words_plus_midstates() {
        let h = test_header(0);
        let f = frame(8, &h, 8, MidstateOrder::Forward);
        assert_eq!(f.len(), 4 + 64);
        assert_eq!(f[0], 8);
        assert_eq!(f[1], 0x1703_a30c);
        assert_eq!(f[2], 0x6740_0000);
        assert_eq!(&f[4..12], &midstate(&h));
        let r = frame(8, &h, 1, MidstateOrder::ReversedSwapped);
        assert_eq!(r[4], midstate(&h)[7].swap_bytes());
    }

    #[test]
    fn rolled_versions_stay_inside_the_mask() {
        let m = 0x1fff_e000;
        assert_eq!(rolled_version(0x2000_0000, m, 0), 0x2000_0000);
        assert_eq!(rolled_version(0x2000_0000, m, 3), 0x2000_6000);
        assert_eq!(rolled_version(0x3fff_e000, m, 1) & !m, 0x2000_0000);
        assert_eq!(rolled_version(0x2000_0000, 0, 5), 0x2000_0000);
    }

    #[test]
    fn nonce_word_fields() {
        let n = decode_nonce(0xdead_beef, 0x9a00_3802);
        assert_eq!(
            (n.nonce, n.solution, n.work_id, n.crc),
            (0xdead_beef, 2, 0x38, 0x9a)
        );
    }
}
