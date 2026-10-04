//! The session store: a replicated operation log in a custom binary
//! format. Pure (no egui, no `App`); `session_sync` adapts it to the live
//! graph. See docs/superpowers/specs/2026-09-25-session-op-log-design.md.

pub mod atomic;
pub mod crc32c;
pub mod journal;
pub mod json;
pub mod local;
pub mod model;
pub mod ops;
pub mod tlv;
pub mod varint;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    use std::collections::BTreeMap;

    use super::model::{Doc, NodeId, Reg};

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

    /// What a document shows: its live nodes, with their fields and the
    /// group each is shown in.
    pub fn shown(doc: &Doc) -> Vec<(NodeId, BTreeMap<u32, Reg>, Option<NodeId>)> {
        doc.live().map(|(id, node)| (id, node.fields.clone(), doc.effective_group(id))).collect()
    }
}
