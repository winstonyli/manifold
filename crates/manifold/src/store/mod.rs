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
pub mod tree;
pub mod varint;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};

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
    /// Each call returns a distinct directory, even for the same `name`, so
    /// parallel tests that share a name cannot delete each other's files.
    /// The first call of a run also sweeps other runs' leftovers.
    pub fn temp_dir(name: &str) -> PathBuf {
        static SWEEP: std::sync::Once = std::sync::Once::new();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        SWEEP.call_once(|| {
            sweep_old_dirs(&std::env::temp_dir(), std::time::Duration::from_secs(60 * 60));
        });
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("manifold-store-{}-{n}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Removes `root`'s `manifold-store-<pid>-*` directories of other
    /// processes last modified more than `older_than` ago. Returns how many.
    /// An hour is far longer than a test run, so a concurrent run's folders stay.
    pub fn sweep_old_dirs(root: &Path, older_than: std::time::Duration) -> usize {
        let ours = format!("manifold-store-{}-", std::process::id());
        let Ok(entries) = std::fs::read_dir(root) else { return 0 };
        entries
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.starts_with("manifold-store-") && !name.starts_with(&ours)
            })
            .filter(|e| {
                e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().unwrap_or_default() >= older_than)
            })
            .filter(|e| std::fs::remove_dir_all(e.path()).is_ok())
            .count()
    }

    /// What a document shows: its shown nodes, with their fields and the
    /// group each is shown in.
    pub fn shown(doc: &Doc) -> Vec<(NodeId, BTreeMap<u32, Reg>, Option<NodeId>)> {
        doc.shown().map(|(id, node)| (id, node.fields.clone(), doc.effective_group(id))).collect()
    }

    /// Writes an empty journal at `path` whose header says format version 1
    /// (a numeric kind, no `type`), with a valid CRC. Returns its bytes.
    pub fn version_one_file(path: &Path) -> Vec<u8> {
        drop(super::journal::Journal::open(path, super::model::ActorId(1)).unwrap());
        let mut bytes = std::fs::read(path).unwrap();
        bytes[4] = 1;
        let crc = super::crc32c::crc32c(&bytes[..21]);
        bytes[21..25].copy_from_slice(&crc.to_le_bytes());
        std::fs::write(path, &bytes).unwrap();
        bytes
    }

    mod tests {
        use super::*;

        #[test]
        fn the_sweep_removes_only_other_runs_store_dirs() {
            let root = temp_dir("sweep_root");
            let other = root.join("manifold-store-1-old");
            let ours = root.join(format!("manifold-store-{}-mine", std::process::id()));
            let unrelated = root.join("something-else");
            for d in [&other, &ours, &unrelated] {
                std::fs::create_dir_all(d).unwrap();
            }
            assert_eq!(sweep_old_dirs(&root, std::time::Duration::ZERO), 1);
            assert!(!other.exists() && ours.exists() && unrelated.exists());
        }

        #[test]
        fn temp_dir_is_distinct_for_the_same_name() {
            let a = temp_dir("same");
            let b = temp_dir("same");
            assert_ne!(a, b);
            std::fs::write(a.join("file.txt"), b"a").unwrap();
            assert!(!b.join("file.txt").exists());
        }
    }
}
