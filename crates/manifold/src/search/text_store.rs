//! The text of each page, kept as an append-only log of JSON lines (`text.log`): one record per line, a tombstone
//! (`"gone":true`) for a removed card. Loading replays the log, so a torn last line (a crash mid-write) costs only
//! itself. `compact_if_wasteful` rewrites it when dead lines outnumber live records.

use crate::store::model::NodeId;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE: &str = "text.log";

pub(crate) struct TextRecord {
    pub id: NodeId,
    pub url: String,
    pub title: String,
    pub text: String,
    /// Unix seconds when the text was read.
    pub at: u64,
}

impl TextRecord {
    fn line(&self) -> String {
        json!({"id": self.id.to_string(), "url": self.url, "title": self.title, "text": self.text, "at": self.at})
            .to_string()
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(TextRecord {
            id: v.get("id")?.as_str()?.parse().ok()?,
            url: v.get("url")?.as_str()?.to_owned(),
            title: v.get("title")?.as_str()?.to_owned(),
            text: v.get("text")?.as_str()?.to_owned(),
            at: v.get("at")?.as_u64()?,
        })
    }
}

pub(crate) struct TextStore {
    /// `None` keeps the records in memory only (tests, and a profile that cannot be found).
    path: Option<PathBuf>,
    live: HashMap<NodeId, TextRecord>,
    file_lines: usize,
}

impl TextStore {
    /// Loads `dir/text.log` (missing: empty). `None` keeps everything in memory.
    pub(crate) fn open(dir: Option<&Path>) -> Self {
        let path = dir.map(|d| d.join(FILE));
        let mut store = TextStore { path, live: HashMap::new(), file_lines: 0 };
        let Some(path) = store.path.clone() else { return store };
        let Ok(bytes) = std::fs::read(&path) else { return store };
        for line in String::from_utf8_lossy(&bytes).lines() {
            store.file_lines += 1;
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if v.get("gone").and_then(Value::as_bool) == Some(true) {
                if let Some(id) = v.get("id").and_then(Value::as_str).and_then(|s| s.parse().ok()) {
                    store.live.remove(&id);
                }
            } else if let Some(rec) = TextRecord::parse(&v) {
                store.live.insert(rec.id, rec);
            }
        }
        // A torn last line has no newline; end it so the next append starts a line of its own.
        if bytes.last().is_some_and(|&b| b != b'\n') {
            store.append("");
        }
        store
    }

    pub(crate) fn put(&mut self, rec: TextRecord) {
        self.append(&rec.line());
        self.live.insert(rec.id, rec);
    }

    pub(crate) fn remove(&mut self, id: NodeId) {
        if self.live.remove(&id).is_some() {
            self.append(&json!({"id": id.to_string(), "gone": true}).to_string());
        }
    }

    pub(crate) fn get(&self, id: NodeId) -> Option<&TextRecord> {
        self.live.get(&id)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.live.keys().copied()
    }

    /// Forgets every record and deletes the file.
    pub(crate) fn clear(&mut self) {
        self.live.clear();
        self.file_lines = 0;
        if let Some(path) = &self.path {
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("search: could not delete {}: {e}", path.display());
                }
            }
        }
    }

    /// Rewrites the file as the live records when it holds more than twice as many lines (plus 100) as records.
    pub(crate) fn compact_if_wasteful(&mut self) {
        let Some(path) = &self.path else { return };
        if self.file_lines <= 2 * self.live.len() + 100 {
            return;
        }
        let mut body = String::new();
        for rec in self.live.values() {
            body.push_str(&rec.line());
            body.push('\n');
        }
        match crate::store::atomic::replace_private(path, body.as_bytes()) {
            Ok(()) => self.file_lines = self.live.len(),
            Err(e) => eprintln!("search: could not compact {}: {e}", path.display()),
        }
    }

    fn append(&mut self, line: &str) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let written =
            std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| writeln!(f, "{line}"));
        match written {
            Ok(()) => self.file_lines += 1,
            Err(e) => eprintln!("search: could not write {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::testutil::node;
    use crate::store;

    fn rec(n: u64, text: &str) -> TextRecord {
        TextRecord { id: node(n), url: format!("https://a.test/{n}"), title: "t".into(), text: text.into(), at: 7 }
    }

    #[test]
    fn put_then_reopen_returns_the_record() {
        let dir = store::testutil::temp_dir("text-put");
        let mut s = TextStore::open(Some(&dir));
        s.put(rec(1, "hello"));
        drop(s);
        let s = TextStore::open(Some(&dir));
        let r = s.get(node(1)).unwrap();
        assert_eq!((r.text.as_str(), r.url.as_str(), r.at), ("hello", "https://a.test/1", 7));
    }

    #[test]
    fn remove_is_remembered_after_reopen() {
        let dir = store::testutil::temp_dir("text-remove");
        let mut s = TextStore::open(Some(&dir));
        s.put(rec(1, "hello"));
        s.remove(node(1));
        assert!(s.get(node(1)).is_none());
        drop(s);
        assert!(TextStore::open(Some(&dir)).get(node(1)).is_none());
    }

    #[test]
    fn a_later_put_wins() {
        let dir = store::testutil::temp_dir("text-later");
        let mut s = TextStore::open(Some(&dir));
        s.put(rec(1, "first"));
        s.put(rec(1, "second"));
        drop(s);
        assert_eq!(TextStore::open(Some(&dir)).get(node(1)).unwrap().text, "second");
    }

    #[test]
    fn a_torn_last_line_is_ignored_and_does_not_poison_the_next_write() {
        use std::io::Write;
        let dir = store::testutil::temp_dir("text-torn");
        let mut s = TextStore::open(Some(&dir));
        s.put(rec(1, "kept"));
        drop(s);
        let mut f = std::fs::OpenOptions::new().append(true).open(dir.join("text.log")).unwrap();
        f.write_all(b"{\"id\":").unwrap();
        drop(f);
        let mut s = TextStore::open(Some(&dir));
        assert_eq!(s.get(node(1)).unwrap().text, "kept");
        s.put(rec(2, "after"));
        drop(s);
        let s = TextStore::open(Some(&dir));
        assert_eq!(s.get(node(1)).unwrap().text, "kept");
        assert_eq!(s.get(node(2)).unwrap().text, "after");
    }

    #[test]
    fn compaction_keeps_live_records_and_shrinks_the_file() {
        let dir = store::testutil::temp_dir("text-compact");
        let mut s = TextStore::open(Some(&dir));
        for i in 0..300 {
            s.put(rec(1, &format!("v{i}")));
        }
        s.compact_if_wasteful();
        let lines = std::fs::read_to_string(dir.join("text.log")).unwrap().lines().count();
        assert_eq!(lines, 1);
        drop(s);
        assert_eq!(TextStore::open(Some(&dir)).get(node(1)).unwrap().text, "v299");
    }

    #[test]
    fn a_tidy_log_is_not_rewritten() {
        let dir = store::testutil::temp_dir("text-tidy");
        let mut s = TextStore::open(Some(&dir));
        s.put(rec(1, "a"));
        s.put(rec(1, "b"));
        s.compact_if_wasteful();
        assert_eq!(std::fs::read_to_string(dir.join("text.log")).unwrap().lines().count(), 2);
    }

    #[test]
    fn clear_deletes_the_file_and_the_records() {
        let dir = store::testutil::temp_dir("text-clear");
        let mut s = TextStore::open(Some(&dir));
        s.put(rec(1, "a"));
        s.clear();
        assert!(s.get(node(1)).is_none() && !dir.join("text.log").exists());
        assert!(TextStore::open(Some(&dir)).get(node(1)).is_none());
    }

    #[test]
    fn memory_only_store_never_writes() {
        let mut s = TextStore::open(None);
        s.put(rec(1, "a"));
        assert_eq!(s.get(node(1)).unwrap().text, "a");
        assert_eq!(s.ids().count(), 1);
        s.remove(node(1));
        s.compact_if_wasteful();
        assert!(s.get(node(1)).is_none());
    }
}
