//! Favicon fetching, done once here rather than aggregated from each
//! engine's own native favicon API. That per-engine approach (still visible
//! in git history) was genuinely inconsistent: Servo/WebKit/WebView2 each
//! had their own working-but-different mechanism, and CEF's never actually
//! worked at all (`on_favicon_urlchange` fires but its `icon_urls` list is
//! always empty under CEF's windowless/Alloy runtime -- a confirmed
//! platform limitation, not fixable from that engine-host's side). Fetching
//! `{origin}/favicon.ico` directly, independent of whichever engine is
//! rendering a given tab, gets identical behavior across all four --
//! including Servo, which never had *any* favicon support before this.
//!
//! Same "just spawn a thread" pattern `SuggestionFetcher` already uses for
//! its own network fetch (`manifold` has no async runtime) -- keyed by
//! origin rather than per-request generation, since a favicon fetch has no
//! notion of "stale" the way a debounced search query does: whichever
//! result for a given origin arrives is just as valid whenever it lands.
//!
//! Each fetched favicon is also kept on disk (`profile::favicon_cache_dir`),
//! one PNG per origin, so a launch shows the favicons of sleeping tabs
//! without fetching them again. A copy younger than `CACHE_FRESH` is used
//! as is; an older one is refetched, and still used if that fails (offline,
//! say). Files untouched for `CACHE_PRUNE` are deleted at launch. A failed
//! fetch isn't recorded, so an origin without a favicon is tried once per
//! launch, as before.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

/// How long a cached favicon is used without asking its origin again.
const CACHE_FRESH: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long a cached favicon nobody asked for survives.
const CACHE_PRUNE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// A successfully decoded favicon: raw RGBA8 pixels plus dimensions, the
/// same shape `GpuState::upload_favicon` already expects.
pub struct FaviconImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Favicon fetches running at once; the rest wait in `queue`. A restored
/// session can name dozens of origins at the same moment.
const MAX_FAVICON_FETCHES: usize = 4;

/// Fetches and decodes favicons on background threads, deduplicating
/// concurrent requests for the same origin (two tabs navigating to the same
/// site around the same time shouldn't fire two redundant HTTP requests).
/// Which origins already resolved this launch is `App::favicon_resolved`'s
/// job; this type knows only the requests in flight, and the disk cache.
pub struct FaviconFetcher {
    tx: mpsc::Sender<(String, Option<FaviconImage>)>,
    rx: mpsc::Receiver<(String, Option<FaviconImage>)>,
    in_flight: HashSet<String>,
    /// Origins waiting for one of the `MAX_FAVICON_FETCHES` slots.
    queue: VecDeque<String>,
    cache_dir: Option<PathBuf>,
    /// `fetch_favicon`; tests swap in a stub so no request leaves the machine.
    fetch: fn(&str) -> Option<FaviconImage>,
}

impl FaviconFetcher {
    /// `cache_dir` is where favicons persist between launches (`None`: in
    /// memory only). Pruning it runs on its own thread.
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        if let Some(dir) = cache_dir.clone() {
            std::thread::spawn(move || prune_cache(&dir, CACHE_PRUNE));
        }
        Self { tx, rx, in_flight: HashSet::new(), queue: VecDeque::new(), cache_dir, fetch: fetch_favicon }
    }

    /// Requests `origin`'s favicon if it isn't already being fetched: it
    /// starts at once while fewer than `MAX_FAVICON_FETCHES` run, and waits
    /// in the queue otherwise. A no-op if this origin is already running or
    /// queued -- the caller is expected to have already checked its own
    /// cache for a previously-*completed* fetch (this only dedupes
    /// concurrent ones).
    pub fn request(&mut self, origin: String) {
        if self.in_flight.contains(&origin) || self.queue.contains(&origin) {
            return;
        }
        if self.in_flight.len() < MAX_FAVICON_FETCHES {
            self.start(origin);
        } else {
            self.queue.push_back(origin);
        }
    }

    fn start(&mut self, origin: String) {
        self.in_flight.insert(origin.clone());
        let (tx, cache_dir, fetch) = (self.tx.clone(), self.cache_dir.clone(), self.fetch);
        std::thread::spawn(move || {
            let result = match &cache_dir {
                Some(dir) => cached_or_fetched(dir, &origin, fetch),
                None => fetch(&origin),
            };
            let _ = tx.send((origin, result));
        });
    }

    /// Call once per frame. Returns every fetch that completed since the
    /// last call -- `None` for the image means the fetch failed (no
    /// favicon at that origin, a network error, or undecodable bytes);
    /// the caller should still remember that as a negative result so it
    /// doesn't keep retrying the same origin every navigation. Queued
    /// origins start here as slots free up.
    pub fn tick(&mut self) -> Vec<(String, Option<FaviconImage>)> {
        let mut results = Vec::new();
        while let Ok((origin, result)) = self.rx.try_recv() {
            self.in_flight.remove(&origin);
            results.push((origin, result));
        }
        while self.in_flight.len() < MAX_FAVICON_FETCHES {
            let Some(origin) = self.queue.pop_front() else { break };
            self.start(origin);
        }
        results
    }
}

/// The `scheme://host[:port]` prefix of `url`, lowercased and without any
/// userinfo, or `None` if `url` has no `://` at all (a `data:`/`about:`
/// URL, or already-malformed input) or no host --
/// deliberately simple hand-rolled parsing, same "no full URL-syntax
/// validation" posture `suggestions::looks_like_url` already documents,
/// rather than pulling in a real URL-parsing crate for just this.
pub fn url_origin(url: &str) -> Option<String> {
    let scheme_end = url.find("://")?;
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    // Userinfo (`user:pass@`) is a credential: never part of a cache name or key.
    let host = rest[..authority_end].rsplit_once('@').map_or(&rest[..authority_end], |(_, host)| host);
    if host.is_empty() {
        return None;
    }
    Some(format!("{}://{}", url[..scheme_end].to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// `GET {origin}/favicon.ico`, decoded to RGBA8 -- the same well-established
/// browser convention every engine's own native favicon API ultimately
/// falls back to when a page doesn't advertise one explicitly via a
/// `<link rel="icon">` (this project doesn't parse page HTML at all, so
/// that richer path isn't available here; the plain `/favicon.ico` guess
/// covers the overwhelming majority of real sites without needing it).
/// The size and timeout caps are safety nets against a misbehaving server;
/// a favicon is small. A missing favicon is the common case, so a failure
/// isn't logged.
fn fetch_favicon(origin: &str) -> Option<FaviconImage> {
    let url = format!("{origin}/favicon.ico");
    let budget = crate::net::Budget {
        connect: Duration::from_secs(3),
        response: Duration::from_secs(3),
        body: Duration::from_secs(5),
    };
    let bytes = crate::net::get_limited(&url, 2_000_000, budget).ok()?;
    let image = crate::rgba::decode_limited(&bytes, 512).ok()?;
    let image = crate::rgba::fit_image(image, crate::rgba::FAVICON_SIDE);
    let (width, height) = image.dimensions();
    Some(FaviconImage { rgba: image.into_raw(), width, height })
}

/// `origin`'s favicon from `dir` while it's fresh; otherwise `fetch`ed and
/// saved there, falling back to the stale copy if the fetch fails.
fn cached_or_fetched(
    dir: &Path,
    origin: &str,
    fetch: impl FnOnce(&str) -> Option<FaviconImage>,
) -> Option<FaviconImage> {
    let path = dir.join(cache_file_name(origin));
    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if age.is_some_and(|age| age < CACHE_FRESH) {
        if let Some(image) = read_png(&path) {
            return Some(image);
        }
    }
    match fetch(origin) {
        Some(image) => {
            let saved = std::fs::create_dir_all(dir).map_err(|e| e.to_string()).and_then(|()| {
                image::save_buffer(&path, &image.rgba, image.width, image.height, image::ColorType::Rgba8)
                    .map_err(|e| e.to_string())
            });
            if let Err(err) = saved {
                eprintln!("favicon: couldn't cache {origin}: {err}");
            }
            Some(image)
        }
        None => age.and_then(|_| read_png(&path)),
    }
}

fn read_png(path: &Path) -> Option<FaviconImage> {
    // A cache file written before favicons were fitted may be full size.
    let image = crate::rgba::fit_image(image::open(path).ok()?.into_rgba8(), crate::rgba::FAVICON_SIDE);
    let (width, height) = image.dimensions();
    Some(FaviconImage { rgba: image.into_raw(), width, height })
}

/// `https://example.org:8080` -> `https_3a_2f_2fexample.org_3a8080.png`: every
/// character a file name can't safely hold (and `_` itself) becomes `_` and its
/// two hex digits, so two origins never share a file.
fn cache_file_name(origin: &str) -> String {
    let mut name = String::new();
    for b in origin.bytes() {
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' {
            name.push(b as char);
        } else {
            name.push_str(&format!("_{b:02x}"));
        }
    }
    format!("{name}.png")
}

/// Deletes the files in `dir` not written for `max_age`.
fn prune_cache(dir: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::now());
        if modified.elapsed().is_ok_and(|age| age >= max_age) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;

    fn image(shade: u8) -> FaviconImage {
        FaviconImage { rgba: vec![shade; 2 * 2 * 4], width: 2, height: 2 }
    }

    fn age(dir: &Path, origin: &str, by: Duration) {
        let file = std::fs::File::options().write(true).open(dir.join(cache_file_name(origin))).unwrap();
        file.set_modified(SystemTime::now() - by).unwrap();
    }

    /// A fetched favicon is saved and then served from disk while fresh; a
    /// stale one is refetched, and still served when that fetch fails.
    #[test]
    fn the_cache_serves_fresh_copies_and_falls_back_to_stale_ones() {
        let dir = temp_dir("favicon-cache");
        let origin = "https://example.org:8080";
        let got = |fetch: Option<u8>| cached_or_fetched(&dir, origin, |_| fetch.map(image)).map(|i| i.rgba[0]);
        assert_eq!(got(None), None, "nothing cached, nothing fetched");
        assert_eq!(got(Some(1)), Some(1), "fetched");
        assert_eq!(got(Some(2)), Some(1), "fresh: served from disk, not refetched");
        age(&dir, origin, CACHE_FRESH);
        assert_eq!(got(None), Some(1), "stale, fetch failed: the stale copy");
        assert_eq!(got(Some(3)), Some(3), "stale: refetched and saved");
        assert_eq!(got(None), Some(3));
    }

    #[test]
    fn pruning_deletes_only_old_files() {
        let dir = temp_dir("favicon-prune");
        for (origin, shade) in [("https://old", 1), ("https://new", 2)] {
            cached_or_fetched(&dir, origin, |_| Some(image(shade)));
        }
        age(&dir, "https://old", CACHE_PRUNE);
        prune_cache(&dir, CACHE_PRUNE);
        assert!(!dir.join(cache_file_name("https://old")).exists());
        assert!(dir.join(cache_file_name("https://new")).exists());
    }

    #[test]
    fn a_cache_file_name_is_the_origin_made_safe() {
        assert_eq!(cache_file_name("https://example.org:8080"), "https_3a_2f_2fexample.org_3a8080.png");
        assert_ne!(cache_file_name("https://a:1"), cache_file_name("https://a_1"));
    }

    #[test]
    fn an_origin_drops_userinfo_and_case() {
        assert_eq!(url_origin("https://user:pass@Example.COM/x?y"), Some("https://example.com".into()));
        assert_eq!(url_origin("HTTP://a@b@Host:8080#f"), Some("http://host:8080".into()));
        assert_eq!(url_origin("https://user@/x"), None, "no host left");
    }

    #[test]
    fn at_most_four_favicon_fetches_run_at_once() {
        let mut fetcher = FaviconFetcher::new(None);
        fetcher.fetch = |_| None;
        for n in 0..6 {
            fetcher.request(format!("https://o{n}.example"));
        }
        fetcher.request("https://o5.example".into());
        assert_eq!(
            (fetcher.in_flight.len(), fetcher.queue.len()),
            (MAX_FAVICON_FETCHES, 2),
            "a queued origin isn't queued twice"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut done = Vec::new();
        while done.len() < 6 && std::time::Instant::now() < deadline {
            done.extend(fetcher.tick().into_iter().map(|(origin, _)| origin));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        done.sort();
        assert_eq!(done, (0..6).map(|n| format!("https://o{n}.example")).collect::<Vec<_>>());
    }
}
