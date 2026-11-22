//! Expansion of the compact `bits` encoding used in block headers.

/// Expands `bits` into a 256-bit big-endian target, the value a hash must not
/// exceed. Returns `None` for the negative and overflowing encodings that
/// Bitcoin rejects.
pub fn target_from_bits(bits: u32) -> Option<[u8; 32]> {
    let exponent = (bits >> 24) as i64;
    let mantissa = bits & 0x007f_ffff;
    if bits & 0x0080_0000 != 0 {
        return None;
    }

    let mut target = [0u8; 32];
    // The mantissa's three bytes have big-endian weights 256^2, 256^1, 256^0,
    // scaled by 256^(exponent - 3). A byte whose weight falls below zero is
    // dropped, matching Bitcoin's truncation for small exponents.
    for (index, byte) in mantissa.to_be_bytes()[1..].iter().enumerate() {
        let weight = (2 - index as i64) + exponent - 3;
        if weight < 0 {
            continue;
        }
        if weight > 31 {
            if *byte != 0 {
                return None;
            }
            continue;
        }
        target[31 - weight as usize] = *byte;
    }
    Some(target)
}

/// Share target for an integer pool difficulty: the difficulty-1 target divided
/// by `difficulty`. Returns `None` for zero. Pools may send fractional values;
/// callers round them up so the resulting target is never looser than the pool's.
pub fn target_from_difficulty(difficulty: u64) -> Option<[u8; 32]> {
    if difficulty == 0 {
        return None;
    }
    let diff1 = target_from_bits(0x1d00_ffff)?;
    let divisor = u128::from(difficulty);
    let mut out = [0u8; 32];
    let mut remainder: u128 = 0;
    for (index, byte) in diff1.iter().enumerate() {
        // remainder < divisor, so the running value is below 256 * divisor and
        // the quotient always fits in one byte.
        let current = (remainder << 8) | u128::from(*byte);
        out[index] = (current / divisor) as u8;
        remainder = current % divisor;
    }
    Some(out)
}

/// Share target for a pool difficulty, which may be fractional.
///
/// For difficulty `d >= 1` the target is `diff1 / ceil(d)`, which is never
/// looser than the pool's. For `d < 1` it is `diff1 * floor(1/d)`, which is
/// never looser either, since `floor` only lowers the multiplier. Returns `None`
/// for zero, negative, or non-finite values.
pub fn target_from_pool_difficulty(difficulty: f64) -> Option<[u8; 32]> {
    if !difficulty.is_finite() || difficulty <= 0.0 {
        return None;
    }
    if difficulty >= 1.0 {
        // Float-to-integer casts saturate, so huge difficulties map to u64::MAX.
        return target_from_difficulty(difficulty.ceil() as u64);
    }
    let diff1 = target_from_bits(0x1d00_ffff)?;
    let multiplier = ((1.0 / difficulty).floor() as u64).max(1);
    Some(scale_target(&diff1, multiplier))
}

/// Multiplies a 256-bit big-endian value by `factor`, saturating to the maximum
/// target if the product does not fit.
fn scale_target(target: &[u8; 32], factor: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut carry: u128 = 0;
    for index in (0..32).rev() {
        let product = u128::from(target[index]) * u128::from(factor) + carry;
        out[index] = product as u8;
        carry = product >> 8;
    }
    if carry != 0 { [0xff; 32] } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_difficulty_one_matches_the_integer_path() {
        assert_eq!(target_from_pool_difficulty(1.0), target_from_difficulty(1));
    }

    #[test]
    fn fractional_difficulty_loosens_the_target() {
        // 2^-27 scales diff1 by 2^27, so the top byte becomes 0x07 (about 1 in 32).
        let target = target_from_pool_difficulty(1.0 / 134_217_728.0).unwrap();
        assert_eq!(target[0], 0x07);
        assert!(target > target_from_bits(0x1d00_ffff).unwrap());
    }

    #[test]
    fn rejects_non_positive_and_non_finite_difficulty() {
        assert_eq!(target_from_pool_difficulty(0.0), None);
        assert_eq!(target_from_pool_difficulty(-1.0), None);
        assert_eq!(target_from_pool_difficulty(f64::NAN), None);
        assert_eq!(target_from_pool_difficulty(f64::INFINITY), None);
    }

    #[test]
    fn difficulty_one_gives_the_difficulty_one_target() {
        assert_eq!(target_from_difficulty(1), target_from_bits(0x1d00_ffff));
    }

    #[test]
    fn difficulty_256_shifts_the_target_by_one_byte() {
        let diff1 = target_from_bits(0x1d00_ffff).unwrap();
        let shifted = target_from_difficulty(256).unwrap();
        assert_eq!(shifted[0], 0);
        assert_eq!(&shifted[1..], &diff1[..31]);
    }

    #[test]
    fn zero_difficulty_has_no_target() {
        assert_eq!(target_from_difficulty(0), None);
    }

    #[test]
    fn expands_the_genesis_difficulty_one_target() {
        let target = target_from_bits(0x1d00_ffff).unwrap();
        let mut expected = [0u8; 32];
        expected[4] = 0xff;
        expected[5] = 0xff;
        assert_eq!(target, expected);
        assert_eq!(&target[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn rejects_negative_and_overflowing_encodings() {
        assert_eq!(target_from_bits(0x0380_0001), None);
        // Exponent 34 pushes the mantissa's top byte beyond 256 bits.
        assert_eq!(target_from_bits(0x2201_0000), None);
    }
}
