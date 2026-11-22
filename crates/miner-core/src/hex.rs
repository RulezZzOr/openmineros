//! Minimal hex codec for Stratum fields and test fixtures.

/// Decodes an even-length hex string, accepting either case.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| Some(nibble(pair[0])? << 4 | nibble(pair[1])?))
        .collect()
}

/// Encodes bytes as lowercase hex.
pub fn encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_bad_input() {
        assert_eq!(decode("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(decode("ABcd"), Some(vec![0xab, 0xcd]));
        assert_eq!(encode(&[0x00, 0xff, 0x10]), "00ff10");
        assert_eq!(decode("abc"), None);
        assert_eq!(decode("zz"), None);
    }
}
