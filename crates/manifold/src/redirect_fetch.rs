//! Fetches (or, for a `file://` url, reads from disk) the bytes behind a
//! tab whose page is content that becomes a node, or behind a node's
//! source url (Refresh, a missing file). manifold fetches on its own,
//! engine-agnostic, like `favicon_fetch`: a thread per running job (at most four), an `mpsc`
//! channel, and a `tick()` each frame.
//!
//! The kind comes from `content::kind_of`, and the bytes must pass
//! `content::passes` before anything is written: a server's claimed type
//! isn't trusted. A file kind is saved to the profile's content dir as
//! `<node id>.<ext>`; a note's text comes back directly. `App::
//! insert_content` then makes the node, the same insert a picked file
//! goes through.

use crate::content::{self, ContentKind};
use crate::WebBody;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// What a finished fetch is for.
pub enum FetchTarget {
    /// Convert this tab, if it still shows `url`.
    Tab(String),
    /// Reload this node's file (Refresh, or a missing file).
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

/// Fetches running at once; the rest wait in `throttle`. A pasted or restored session can name dozens
/// of large files, and each running fetch holds its whole body (up to 100 MB) in memory.
const MAX_REDIRECT_FETCHES: usize = 4;

pub struct RedirectFetcher {
    dir: Option<PathBuf>,
    tx: mpsc::Sender<FetchResult>,
    rx: mpsc::Receiver<FetchResult>,
    /// Nodes with a fetch running or waiting: a second Refresh (or a Retry racing a
    /// restore-time refetch) is dropped instead of writing the same file.
    in_flight: HashSet<String>,
    /// Jobs beyond `MAX_REDIRECT_FETCHES` wait here.
    throttle: crate::net::Throttle<FetchJob>,
    /// `fetch`; tests swap in a stub.
    fetch: fn(&Path, &str, &str, ContentKind) -> Option<WebBody>,
}

impl RedirectFetcher {
    /// `dir` is `profile::content_dir()`; `None` (tests) fails every fetch.
    pub fn new(dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        let throttle = crate::net::Throttle::new(MAX_REDIRECT_FETCHES);
        Self { dir, tx, rx, in_flight: HashSet::new(), throttle, fetch }
    }

    /// Fetches `job.url` on a background thread, or queues it behind `MAX_REDIRECT_FETCHES` running
    /// ones. `false` (and nothing done) when `job.node_id` is already being fetched or waiting.
    pub fn request(&mut self, job: FetchJob) -> bool {
        if !self.in_flight.insert(job.node_id.clone()) {
            return false;
        }
        if let Some(job) = self.throttle.admit(job) {
            self.start(job);
        }
        true
    }

    fn start(&self, job: FetchJob) {
        let (tx, dir, fetch) = (self.tx.clone(), self.dir.clone(), self.fetch);
        std::thread::spawn(move || {
            // A panic still reports (as a failure), so `tick` frees the node and the slot;
            // the panic hook has already printed it.
            let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                dir.as_deref().and_then(|d| fetch(d, &job.node_id, &job.url, job.kind))
            }))
            .unwrap_or(None);
            let _ = tx.send(FetchResult { job, body });
        });
    }

    /// Call once per frame. `body: None` means the fetch failed (a network
    /// error, or bytes that don't actually pass `content::passes` despite
    /// the engine's own `document.contentType` check) -- the tab is just
    /// left as an ordinary tab in that case, still showing whichever
    /// native viewer page the engine itself rendered for it.
    pub fn tick(&mut self) -> Vec<FetchResult> {
        let results: Vec<FetchResult> = self.rx.try_iter().collect();
        for r in &results {
            self.in_flight.remove(&r.job.node_id);
            if let Some(next) = self.throttle.finish() {
                self.start(next);
            }
        }
        results
    }
}

/// Fetches (or, for `file://`, reads) `url`, checks it is `kind`, and
/// saves it as `<node_id>.<ext>` in `dir`. A note's text comes back
/// instead of being saved. Blocking: `request` runs it on a thread. A
/// `file:` URL on another host is refused (`is_unc_file_url`). A web
/// fetch's failure is logged with the URL's origin only: a path or query
/// can carry a token.
pub fn fetch(dir: &Path, node_id: &str, url: &str, kind: ContentKind) -> Option<WebBody> {
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
                    let origin = crate::favicon_fetch::url_origin(url).unwrap_or_default();
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
pub fn save(dir: &Path, node_id: &str, kind: ContentKind, bytes: Vec<u8>) -> Option<WebBody> {
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
    use crate::content::{path_file_url, ContentKind};
    use crate::WebBody;

    static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static RELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// A stub fetch that counts how many run at once and holds until released.
    fn gated_fetch(_: &Path, _: &str, _: &str, _: ContentKind) -> Option<WebBody> {
        use std::sync::atomic::Ordering::SeqCst;
        let now = RUNNING.fetch_add(1, SeqCst) + 1;
        PEAK.fetch_max(now, SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !RELEASE.load(SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        RUNNING.fetch_sub(1, SeqCst);
        None
    }

    #[test]
    fn at_most_four_redirect_fetches_run_at_once() {
        use std::sync::atomic::Ordering::SeqCst;
        RUNNING.store(0, SeqCst);
        PEAK.store(0, SeqCst);
        RELEASE.store(false, SeqCst);
        let mut fetcher = RedirectFetcher::new(Some(crate::store::testutil::temp_dir("redirect_cap")));
        fetcher.fetch = gated_fetch;
        for n in 0..10 {
            let job = FetchJob {
                target: FetchTarget::Node(format!("7-{n}")),
                node_id: format!("7-{n}"),
                url: "https://example.invalid/x.pdf".into(),
                kind: ContentKind::Pdf,
            };
            assert!(fetcher.request(job));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while RUNNING.load(SeqCst) < MAX_REDIRECT_FETCHES && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        // An absence cannot be waited for: give any extra thread 100 ms to show itself. Before the
        // cap all ten start within a millisecond or two.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(PEAK.load(SeqCst), MAX_REDIRECT_FETCHES, "no fifth fetch started while four were held");
        RELEASE.store(true, SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut done = 0;
        while done < 10 && std::time::Instant::now() < deadline {
            done += fetcher.tick().len();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(done, 10, "every queued job eventually ran and reported");
        assert_eq!(PEAK.load(SeqCst), MAX_REDIRECT_FETCHES);
    }

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
        let tmp = std::fs::read_dir(&out).unwrap().flatten().any(|e| e.path().extension().is_some_and(|x| x == "tmp"));
        assert!(!tmp, "saved through a renamed temp file");
    }

    #[test]
    fn a_file_url_past_its_kinds_limit_is_refused() {
        let dir = crate::store::testutil::temp_dir("redirect_file_limit");
        let src = dir.join("long.txt");
        std::fs::write(&src, vec![b'a'; content::NOTE_MAX_BYTES + 1]).unwrap();
        assert!(fetch(&dir, "9-2", &path_file_url(&src), ContentKind::Note).is_none());
        std::fs::write(&src, vec![b'a'; content::NOTE_MAX_BYTES]).unwrap();
        assert!(matches!(fetch(&dir, "9-2", &path_file_url(&src), ContentKind::Note), Some(WebBody::Text(_))));
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
        let mut fetcher = RedirectFetcher::new(None);
        let job = || FetchJob {
            target: FetchTarget::Node("7-1".into()),
            node_id: "7-1".into(),
            url: "https://example.invalid/x.pdf".into(),
            kind: ContentKind::Pdf,
        };
        assert!(fetcher.request(job()));
        assert!(!fetcher.request(job()), "already in flight");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut results = Vec::new();
        while results.is_empty() && std::time::Instant::now() < deadline {
            results.extend(fetcher.tick());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(results.len(), 1);
        assert!(fetcher.request(job()), "free again once its result is ticked");
    }

    /// A fetch that panics still reports (as a failure) and frees its node:
    /// otherwise Refresh and Retry would be dead for it until a restart.
    #[test]
    fn a_panicking_fetch_still_frees_its_node() {
        let mut fetcher = RedirectFetcher::new(Some(crate::store::testutil::temp_dir("redirect_panic")));
        fetcher.fetch = |_, _, _, _| panic!("stub fetch panics (expected in this test)");
        let job = || FetchJob {
            target: FetchTarget::Node("7-2".into()),
            node_id: "7-2".into(),
            url: "https://example.invalid/x.pdf".into(),
            kind: ContentKind::Pdf,
        };
        assert!(fetcher.request(job()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut results = Vec::new();
        while results.is_empty() && std::time::Instant::now() < deadline {
            results.extend(fetcher.tick());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(results.as_slice(), [FetchResult { body: None, .. }]), "reported as a failure");
        assert!(fetcher.request(job()), "the node is free again");
    }
}
