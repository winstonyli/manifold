//! Bijective prefix varint (spec "Varint: bijective prefix, big-endian"):
//! the first byte's leading 1-bits count the extra bytes, payload bits are
//! big-endian, and each tier starts where the one below it ends, so every
//! `u64` has exactly one encoding and byte order equals numeric order.

/// The first value of each tier: tier `k` (`k` extra bytes) holds
/// `OFFSETS[k]..OFFSETS[k + 1]`, and tier 8 runs to `u64::MAX`.
const OFFSETS: [u64; 9] = {
    let mut offsets = [0u64; 9];
    let mut k = 0;
    while k < 8 {
        offsets[k + 1] = offsets[k] + (1u64 << (7 + 7 * k));
        k += 1;
    }
    offsets
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarintError {
    /// The buffer ends inside the varint.
    Truncated,
    /// A 9-byte varint whose value would pass `u64::MAX`.
    Overflow,
}

/// How many bytes `write` uses for `value`: 1 to 9.
pub fn encoded_len(value: u64) -> usize {
    (0..8).find(|&k| value < OFFSETS[k + 1]).map_or(9, |k| k + 1)
}

pub fn write(out: &mut Vec<u8>, value: u64) {
    let extra = encoded_len(value) - 1;
    let payload = (value - OFFSETS[extra]).to_be_bytes();
    if extra == 8 {
        out.push(0xFF);
        out.extend_from_slice(&payload);
        return;
    }
    // `extra` leading 1-bits, a 0, then the payload's top bits.
    let prefix = !(0xFFu8 >> extra);
    let tail = &payload[7 - extra..];
    out.push(prefix | tail[0]);
    out.extend_from_slice(&tail[1..]);
}

/// Decodes the varint at the start of `buf`: its value and its length.
pub fn read(buf: &[u8]) -> Result<(u64, usize), VarintError> {
    let first = *buf.first().ok_or(VarintError::Truncated)?;
    let extra = first.leading_ones() as usize;
    let len = extra + 1;
    let bytes = buf.get(..len).ok_or(VarintError::Truncated)?;
    if extra == 8 {
        let payload = u64::from_be_bytes(bytes[1..].try_into().expect("8 bytes"));
        let value = payload.checked_add(OFFSETS[8]).ok_or(VarintError::Overflow)?;
        return Ok((value, len));
    }
    let mut payload = u64::from(first & (0x7F >> extra));
    for &b in &bytes[1..] {
        payload = (payload << 8) | u64::from(b);
    }
    Ok((payload + OFFSETS[extra], len))
}

/// Maps signed to unsigned so small magnitudes stay short: 0, -1, 1, -2 ...
/// become 0, 1, 2, 3 ...
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

pub fn unzigzag(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::XorShift;

    fn enc(v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        write(&mut out, v);
        out
    }

    fn round_trip(v: u64) {
        let bytes = enc(v);
        assert_eq!(bytes.len(), encoded_len(v), "{v}");
        assert_eq!(read(&bytes), Ok((v, bytes.len())), "{v}");
    }

    #[test]
    fn tiers_start_where_the_spec_says() {
        assert_eq!(OFFSETS[1], 128);
        assert_eq!(OFFSETS[2], 16_512);
        assert_eq!(OFFSETS[3], 2_113_664);
        for (k, &offset) in OFFSETS.iter().enumerate() {
            round_trip(offset);
            assert_eq!(encoded_len(offset), k + 1);
            if k > 0 {
                round_trip(offset - 1);
                assert_eq!(encoded_len(offset - 1), k);
            }
        }
        round_trip(u64::MAX);
        assert_eq!(enc(u64::MAX).len(), 9);
    }

    #[test]
    fn one_and_two_byte_tiers_round_trip_exhaustively() {
        for v in 0..OFFSETS[2] {
            round_trip(v);
        }
    }

    #[test]
    fn every_one_and_two_byte_string_decodes_canonically() {
        for b0 in 0u8..=0xBF {
            let strings: Vec<Vec<u8>> =
                if b0 < 0x80 { vec![vec![b0]] } else { (0..=255u8).map(|b1| vec![b0, b1]).collect() };
            for bytes in strings {
                let (v, len) = read(&bytes).unwrap();
                assert_eq!(len, bytes.len());
                assert_eq!(enc(v), bytes);
            }
        }
    }

    #[test]
    fn random_byte_strings_of_every_tier_decode_canonically() {
        let mut rng = XorShift(7);
        for _ in 0..20_000 {
            let extra = rng.below(9) as usize;
            let mut bytes: Vec<u8> = (0..=extra).map(|_| rng.next() as u8).collect();
            bytes[0] = if extra == 8 { 0xFF } else { !(0xFFu8 >> extra) | (bytes[0] & (0x7F >> extra)) };
            match read(&bytes) {
                Ok((v, len)) => {
                    assert_eq!(len, bytes.len());
                    assert_eq!(enc(v), bytes);
                }
                Err(err) => assert!(extra == 8 && err == VarintError::Overflow),
            }
        }
    }

    #[test]
    fn random_values_round_trip_and_sort_like_numbers() {
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        for _ in 0..20_000 {
            // Shifting spreads the values over every tier.
            let a = rng.next() >> rng.below(64);
            let b = rng.next() >> rng.below(64);
            round_trip(a);
            assert_eq!(enc(a).cmp(&enc(b)), a.cmp(&b), "{a} vs {b}");
        }
    }

    #[test]
    fn nine_byte_overflow_and_truncation_are_errors() {
        let mut bytes = [0xFFu8; 9];
        assert_eq!(read(&bytes), Err(VarintError::Overflow));
        bytes[1..].copy_from_slice(&(u64::MAX - OFFSETS[8]).to_be_bytes());
        assert_eq!(read(&bytes), Ok((u64::MAX, 9)));
        assert_eq!(read(&[]), Err(VarintError::Truncated));
        assert_eq!(read(&enc(OFFSETS[3])[..2]), Err(VarintError::Truncated));
    }

    #[test]
    fn zigzag_round_trips() {
        for v in [0, 1, -1, 2, -2, i64::MAX, i64::MIN] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
        assert_eq!((zigzag(0), zigzag(-1), zigzag(1)), (0, 1, 2));
    }
}
