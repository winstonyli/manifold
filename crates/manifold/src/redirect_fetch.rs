//! Fetches (or, for a `file://` tab, reads from disk) the bytes behind a
//! tab whose page turned out to be raw, non-HTML content worth turning
//! into a real graph node instead of leaving it sitting there as an
//! ordinary tab -- same "manifold does its own independent HTTP fetch,
//! engine-agnostic" posture `favicon_fetch` already established, and the
//! same "spawn a thread, `mpsc` channel, main loop `tick()`s it" shape
//! every other one-shot background fetch in this project uses
//! (`favicon_fetch`, `pdf_fetch`).
//!
//! The kind comes from `content::kind_of` (the same one-table dispatch a
//! local file or `ADDFILE` goes through); the downloaded (or read) bytes
//! are then checked with `content::passes` before anything is written or
//! converted -- a server's (or a stale local file's) claimed type isn't
//! trusted blindly. A file kind is saved to the profile's content dir as
//! `<node id>.<ext>`; a note's text comes back directly instead, since a
//! note isn't a file on disk.
//!
//! Deliberately reuses each kind's *existing*, synchronous, file-path-based
//! insert function (`App::insert_image_node_with_id`, etc.) rather than
//! teaching that kind's payload a second, network-sourced way to exist:
//! once saved, a converted node is indistinguishable from one the user
//! picked by hand -- same GPU upload path (where relevant), same session
//! persistence, same context menu.

use crate::content::{self, ContentKind};
use crate::WebBody;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// What a finished fetch is for.
pub enum FetchTarget {
    /// Convert this tab, if it still shows `url`.
    Tab(String),
    /// Reload this node's file (Refresh, or a missing file; Task 5/6).
    Node(String),
}

pub struct FetchJob {
    pub target: FetchTarget,
    /// The node the content becomes: names the saved file.
    pub node_id: String,
    pub url: String,
    pub kind: ContentKind,
}

pub struct FetchResult {
    pub job: FetchJob,
    /// `None`: the fetch failed or the bytes failed their check.
    pub body: Option<WebBody>,
}

pub struct RedirectFetcher {
    dir: Option<PathBuf>,
    tx: mpsc::Sender<FetchResult>,
    rx: mpsc::Receiver<FetchResult>,
}

impl RedirectFetcher {
    /// `dir` is `profile::content_dir()`; `None` (tests) fails every fetch.
    pub fn new(dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { dir, tx, rx }
    }

    /// Fetches `job.url` on a background thread.
    pub fn request(&self, job: FetchJob) {
        let tx = self.tx.clone();
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            let body = dir.as_deref().and_then(|d| fetch(d, &job.node_id, &job.url, job.kind));
            let _ = tx.send(FetchResult { job, body });
        });
    }

    /// Call once per frame. `body: None` means the fetch failed (a network
    /// error, or bytes that don't actually pass `content::passes` despite
    /// the engine's own `document.contentType` check) -- the tab is just
    /// left as an ordinary tab in that case, still showing whichever
    /// native viewer page the engine itself rendered for it.
    pub fn tick(&self) -> Vec<FetchResult> {
        self.rx.try_iter().collect()
    }
}

/// Fetches (or, for `file://`, reads) `url`, checks it is `kind`, and
/// saves it as `<node_id>.<ext>` in `dir`. A note's text comes back
/// instead of being saved. Blocking: `request` runs it on a thread.
pub fn fetch(dir: &Path, node_id: &str, url: &str, kind: ContentKind) -> Option<WebBody> {
    let bytes = match content::file_url_path(url) {
        Some(path) => std::fs::read(path).ok()?,
        None => {
            let response = ureq::get(url).config().timeout_global(Some(Duration::from_secs(15))).build().call().ok()?;
            response.into_body().with_config().limit(100_000_000).read_to_vec().ok()?
        }
    };
    save(dir, node_id, kind, bytes)
}

/// Checks `bytes` are `kind` and saves them (or returns a note's text).
pub fn save(dir: &Path, node_id: &str, kind: ContentKind, bytes: Vec<u8>) -> Option<WebBody> {
    if !content::passes(kind, &bytes) {
        return None;
    }
    if kind == ContentKind::Note {
        return String::from_utf8(bytes).ok().map(WebBody::Text);
    }
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{node_id}.{}", content::extension(kind, &bytes)));
    std::fs::write(&path, &bytes).ok()?;
    Some(WebBody::File { kind, path: path.to_string_lossy().into_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{path_file_url, ContentKind};
    use crate::WebBody;

    #[test]
    fn saves_checked_content_by_node_id() {
        let dir = crate::store::testutil::temp_dir("redirect_save");
        match save(&dir, "7-1", ContentKind::Json, br#"{"a": 1}"#.to_vec()) {
            Some(WebBody::File { kind: ContentKind::Json, path }) => {
                assert_eq!(std::path::Path::new(&path), dir.join("7-1.json"));
                assert_eq!(std::fs::read(&path).unwrap(), br#"{"a": 1}"#);
            }
            _ => panic!("expected a saved JSON file"),
        }
        assert!(save(&dir, "7-2", ContentKind::Json, b"{broken".to_vec()).is_none());
        assert!(!dir.join("7-2.json").exists());
        assert!(matches!(save(&dir, "7-3", ContentKind::Note, b"hi".to_vec()), Some(WebBody::Text(t)) if t == "hi"));
        assert!(!dir.join("7-3.txt").exists(), "a note's text isn't saved");
    }

    #[test]
    fn file_urls_are_read_from_disk() {
        let dir = crate::store::testutil::temp_dir("redirect_file_url");
        let src = dir.join("in put.pdf");
        std::fs::write(&src, b"%PDF-1.4\n").unwrap();
        let out = dir.join("out");
        let body = fetch(&out, "9-1", &path_file_url(&src), ContentKind::Pdf);
        assert!(matches!(body, Some(WebBody::File { kind: ContentKind::Pdf, .. })));
        assert!(out.join("9-1.pdf").exists(), "the content dir is created on demand");
    }
}
