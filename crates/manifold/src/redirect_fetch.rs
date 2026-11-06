//! Fetches (or, for a `file://` url, reads from disk) the bytes behind a
//! tab whose page is content that becomes a node, or behind a node's
//! source url (Refresh, a missing file). manifold fetches on its own,
//! engine-agnostic, like `favicon_fetch`: jobs run on a `worker::Pool` (at most four
//! at once), and a `tick()` each frame collects what finished.
//!
//! The kind comes from `content::kind_of`, and the bytes must pass
//! `content::passes` before anything is written: a server's claimed type
//! isn't trusted. A file kind is saved to the profile's content dir as
//! `<node id>.<ext>`; a note's text comes back directly. `App::
//! insert_content` then makes the node, the same insert a picked file
//! goes through.

use crate::content::{self, ContentKind};
use crate::store::model::NodeId;
use crate::worker::Pool;
use crate::WebBody;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What a finished fetch is for.
pub enum FetchTarget {
    /// Convert this tab, if it still shows `url`.
    Tab(NodeId),
    /// Reload this node's file (Refresh, or a missing file).
    Node(NodeId),
}

pub struct FetchJob {
    pub target: FetchTarget,
    /// The node the content becomes: names the saved file.
    pub node_id: NodeId,
    pub url: String,
    pub kind: ContentKind,
}

/// Fetches running at once; the rest wait in the pool. A pasted or restored session can name dozens
/// of large files, and each running fetch holds its whole body (up to 100 MB) in memory.
const MAX_REDIRECT_FETCHES: usize = 4;

pub struct RedirectFetcher {
    /// Keyed by `job.node_id`, so a second Refresh (or a Retry racing a restore-time refetch) is dropped
    /// instead of writing the same file; a result of `None` is a failed fetch.
    pool: Pool<NodeId, FetchJob, Option<WebBody>>,
}

impl RedirectFetcher {
    /// `dir` is `profile::content_dir()`; `None` (tests) fails every fetch.
    /// `fetch` is `redirect_fetch::fetch` in `Workers::new`; tests swap in a stub.
    pub fn new(dir: Option<PathBuf>, fetch: fn(&Path, NodeId, &str, ContentKind) -> Option<WebBody>) -> Self {
        let pool = Pool::new(MAX_REDIRECT_FETCHES, move |job: &FetchJob| {
            dir.as_deref().and_then(|d| fetch(d, job.node_id, &job.url, job.kind))
        });
        Self { pool }
    }

    /// Fetches `job.url` on a background thread, or queues it behind `MAX_REDIRECT_FETCHES` running
    /// ones. `false` (and nothing done) when `job.node_id` is already being fetched or waiting.
    pub fn request(&self, job: FetchJob) -> bool {
        self.pool.request(job.node_id, job)
    }

    /// Call once per frame. A `None` body means the fetch failed (a network
    /// error, bytes that don't actually pass `content::passes` despite
    /// the engine's own `document.contentType` check, or a panic) -- the tab is just
    /// left as an ordinary tab in that case, still showing whichever
    /// native viewer page the engine itself rendered for it.
    pub fn tick(&self) -> Vec<(FetchJob, Option<WebBody>)> {
        self.pool.tick().into_iter().map(|(_, job, body)| (job, body.flatten())).collect()
    }
}

/// Fetches (or, for `file://`, reads) `url`, checks it is `kind`, and
/// saves it as `<node_id>.<ext>` in `dir`. A note's text comes back
/// instead of being saved. Blocking: `request` runs it on a thread. A
/// `file:` URL on another host is refused (`is_unc_file_url`). A web
/// fetch's failure is logged with the URL's origin only: a path or query
/// can carry a token.
pub fn fetch(dir: &Path, node_id: NodeId, url: &str, kind: ContentKind) -> Option<WebBody> {
    let limit = max_bytes(kind);
    let bytes = match content::file_url_path(url) {
        Some(_) if is_unc_file_url(url) => {
            eprintln!("fetch: refused a file URL on another host (node {node_id})");
            return None;
        }
        Some(path) => {
            let mut bytes = Vec::new();
            std::fs::File::open(path).ok()?.take(limit + 1).read_to_end(&mut bytes).ok()?;
            bytes
        }
        None => {
            let budget = crate::net::Budget {
                connect: Duration::from_secs(10),
                response: Duration::from_secs(20),
                body: crate::net::body_budget(limit, SLOW_BYTES_PER_SEC, Duration::from_secs(60)),
            };
            match crate::net::get_limited(url, limit, budget) {
                Ok(bytes) => bytes,
                Err(e) => {
                    let origin = crate::url_parts::UrlParts::parse(url).origin().unwrap_or_default();
                    eprintln!("fetch: {origin} (node {node_id}): {e}");
                    return None;
                }
            }
        }
    };
    if bytes.len() as u64 > limit {
        return None;
    }
    save(dir, node_id, kind, bytes)
}

/// Most a web fetch's body may take: `max_bytes` at 128 KB/s (about
/// 1 Mbit/s), at least a minute. 100 MB gets about 13 minutes.
const SLOW_BYTES_PER_SEC: u64 = 128 * 1024;

/// A `file:` URL naming another machine (`file://host/share/...`, or a
/// path that decodes to `\\host\...`): reading it would make Windows
/// connect to that host and offer the user's credentials. Such URLs come
/// from synced or imported sessions, not from the user's own picks.
pub fn is_unc_file_url(url: &str) -> bool {
    content::file_url_path(url).is_some_and(|p| {
        let s = p.to_string_lossy();
        // Windows: `\\host\share` (UNC) and `\x` (root-relative, what
        // `file:////host/...` decodes to); a drive path (`C:\`) passes.
        // Elsewhere: `//host`; an absolute path (`/home/...`) passes.
        if cfg!(windows) {
            s.starts_with('\\') || s.starts_with('/')
        } else {
            s.starts_with("//")
        }
    })
}

/// The most a fetch of `kind` reads: a note's limit, or 100 MB.
fn max_bytes(kind: ContentKind) -> u64 {
    match kind {
        ContentKind::Note => content::NOTE_MAX_BYTES as u64,
        _ => 100_000_000,
    }
}

/// Checks `bytes` are `kind` and saves them (or returns a note's text).
/// Written to a temp file first, fsynced and renamed over (`store::atomic`),
/// so a reader never sees half a file. The temp is unique per call, so two
/// fetches for one node never share it.
pub fn save(dir: &Path, node_id: NodeId, kind: ContentKind, bytes: Vec<u8>) -> Option<WebBody> {
    if !content::passes(kind, &bytes) {
        return None;
    }
    if kind == ContentKind::Note {
        return String::from_utf8(bytes).ok().map(WebBody::Text);
    }
    std::fs::create_dir_all(dir).ok()?;
    let name = format!("{node_id}.{}", content::extension(kind, &bytes));
    let path = dir.join(&name);
    crate::store::atomic::replace(&path, &bytes).ok()?;
    Some(WebBody::File { kind, path: path.to_string_lossy().into_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::testutil::node;
    use crate::content::{path_file_url, ContentKind};
    use crate::WebBody;

    #[test]
    fn saves_checked_content_by_node_id() {
        let dir = crate::store::testutil::temp_dir("redirect_save");
        match save(&dir, node(1), ContentKind::Json, br#"{"a": 1}"#.to_vec()) {
            Some(WebBody::File { kind: ContentKind::Json, path }) => {
                assert_eq!(std::path::Path::new(&path), dir.join(format!("{}.json", node(1))));
                assert_eq!(std::fs::read(&path).unwrap(), br#"{"a": 1}"#);
            }
            _ => panic!("expected a saved JSON file"),
        }
        assert!(save(&dir, node(2), ContentKind::Json, b"{broken".to_vec()).is_none());
        assert!(!dir.join(format!("{}.json", node(2))).exists());
        assert!(matches!(save(&dir, node(3), ContentKind::Note, b"hi".to_vec()), Some(WebBody::Text(t)) if t == "hi"));
        assert!(!dir.join(format!("{}.txt", node(3))).exists(), "a note's text isn't saved");
    }

    #[test]
    fn file_urls_are_read_from_disk() {
        let dir = crate::store::testutil::temp_dir("redirect_file_url");
        let src = dir.join("in put.pdf");
        std::fs::write(&src, b"%PDF-1.4\n").unwrap();
        let out = dir.join("out");
        let body = fetch(&out, node(4), &path_file_url(&src), ContentKind::Pdf);
        assert!(matches!(body, Some(WebBody::File { kind: ContentKind::Pdf, .. })));
        assert!(out.join(format!("{}.pdf", node(4))).exists(), "the content dir is created on demand");
        let tmp = std::fs::read_dir(&out).unwrap().flatten().any(|e| e.path().extension().is_some_and(|x| x == "tmp"));
        assert!(!tmp, "saved through a renamed temp file");
    }

    #[test]
    fn a_file_url_past_its_kinds_limit_is_refused() {
        let dir = crate::store::testutil::temp_dir("redirect_file_limit");
        let src = dir.join("long.txt");
        std::fs::write(&src, vec![b'a'; content::NOTE_MAX_BYTES + 1]).unwrap();
        assert!(fetch(&dir, node(5), &path_file_url(&src), ContentKind::Note).is_none());
        std::fs::write(&src, vec![b'a'; content::NOTE_MAX_BYTES]).unwrap();
        assert!(matches!(fetch(&dir, node(5), &path_file_url(&src), ContentKind::Note), Some(WebBody::Text(_))));
    }

    #[cfg(windows)] // the host forms decode to UNC paths on Windows only
    #[test]
    fn unc_file_urls_are_recognized() {
        assert!(is_unc_file_url("file://server/share/x.pdf"));
        assert!(is_unc_file_url("file:///%5C%5Cserver/share/x.pdf"));
        assert!(is_unc_file_url("file:////server/share/x.pdf"));
        assert!(!is_unc_file_url("file:///C:/x.pdf"));
        assert!(!is_unc_file_url("file://localhost/C:/x.pdf"));
        assert!(!is_unc_file_url("https://server/x.pdf"));
    }

    #[test]
    fn a_second_request_for_a_node_in_flight_is_dropped() {
        // `None`: every fetch fails at once, without the network.
        let fetcher = RedirectFetcher::new(None, fetch);
        let job = || FetchJob {
            target: FetchTarget::Node(node(1)),
            node_id: node(1),
            url: "https://example.invalid/x.pdf".into(),
            kind: ContentKind::Pdf,
        };
        assert!(fetcher.request(job()));
        assert!(!fetcher.request(job()), "already in flight");
        fetcher.pool.wait_for_threads(0);
        assert_eq!(fetcher.tick().len(), 1);
        assert!(fetcher.request(job()), "free again once its result is ticked");
    }

    /// A fetch that panics still reports (as a failure, with its job) and frees its node:
    /// otherwise Refresh and Retry would be dead for it until a restart.
    #[test]
    fn a_panicking_fetch_still_frees_its_node() {
        let dir = Some(crate::store::testutil::temp_dir("redirect_panic"));
        let fetcher = RedirectFetcher::new(dir, |_, _, _, _| panic!("stub fetch panics (expected in this test)"));
        let job = || FetchJob {
            target: FetchTarget::Node(node(2)),
            node_id: node(2),
            url: "https://example.invalid/x.pdf".into(),
            kind: ContentKind::Pdf,
        };
        assert!(fetcher.request(job()));
        fetcher.pool.wait_for_threads(0);
        let results = fetcher.tick();
        assert!(
            matches!(results.as_slice(), [(job, None)] if job.node_id == node(2)),
            "reported as a failure, carrying its job"
        );
        assert!(fetcher.request(job()), "the node is free again");
    }
}
