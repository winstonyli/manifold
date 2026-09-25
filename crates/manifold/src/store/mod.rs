//! The session store: a replicated operation log in a custom binary
//! format. Pure (no egui, no `App`); `session_sync` adapts it to the live
//! graph. See docs/superpowers/specs/2026-09-25-session-op-log-design.md.

// Nothing outside `store` uses it until the app integration lands.
#![allow(dead_code)]

pub mod crc32c;
pub mod tlv;
pub mod varint;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    /// Seeded xorshift64*, so randomized tests repeat without a dependency.
    pub struct XorShift(pub u64);

    impl XorShift {
        pub fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        pub fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A fresh, empty directory for one test, under the system temp dir.
    pub fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manifold-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
