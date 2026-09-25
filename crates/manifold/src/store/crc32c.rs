//! CRC-32C (Castagnoli), table-driven, with an SSE4.2 path where the CPU
//! has it. Every journal frame carries one (spec "File and frames").

const POLY: u32 = 0x82F6_3B78; // reflected Castagnoli polynomial

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

pub fn crc32c(data: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sse4.2") {
        // SAFETY: the CPU was just checked for SSE4.2.
        return unsafe { sse42(data) };
    }
    table(data)
}

fn table(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
fn sse42(data: &[u8]) -> u32 {
    use std::arch::x86_64::{_mm_crc32_u64, _mm_crc32_u8};
    let (words, rest) = data.as_chunks::<8>();
    let mut c = u64::from(!0u32);
    for word in words {
        c = _mm_crc32_u64(c, u64::from_le_bytes(*word));
    }
    let mut c = c as u32;
    for &b in rest {
        c = _mm_crc32_u8(c, b);
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::XorShift;

    #[test]
    fn standard_check_vectors() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
        // RFC 3720 appendix B.4.
        assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
        assert_eq!(crc32c(&[0xFFu8; 32]), 0x62A8_AB43);
        let ascending: Vec<u8> = (0..32).collect();
        assert_eq!(crc32c(&ascending), 0x46DD_794E);
    }

    #[test]
    fn sse42_path_equals_table_path() {
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("sse4.2") {
            let mut rng = XorShift(99);
            for len in 0..300 {
                let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                // SAFETY: checked for SSE4.2 above.
                assert_eq!(unsafe { sse42(&data) }, table(&data), "len {len}");
            }
        }
    }
}
