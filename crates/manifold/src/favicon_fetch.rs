//! Favicon fetching, done once here rather than aggregated from each
//! engine's own native favicon API. That per-engine approach (still visible
//! in git history) was genuinely inconsistent: Servo/WebKit/WebView2 each
//! had their own working-but-different mechanism, and CEF's never actually
//! worked at all (`on_favicon_urlchange` fires but its `icon_urls` list is
//! always empty under CEF's windowless/Alloy runtime -- a confirmed
//! platform limitation, not fixable from that view-host's side). Fetching
//! `{origin}/favicon.ico` directly, independent of whichever engine is
//! rendering a given tab, gets identical behavior across all four --
//! including Servo, which never had *any* favicon support before this.
//!
//! The queueing, the cap of four and the panic guard are `worker::Pool`'s
//! (`manifold` has no async runtime) -- keyed by origin rather than
//! per-request generation, since a favicon fetch has no notion of "stale"
//! the way a debounced search query does: whichever result for a given
//! origin arrives is just as valid whenever it lands.
//!
//! Each fetched favicon is also kept on disk (`profile::favicon_cache_dir`),
//! one PNG per origin, so a launch shows the favicons of sleeping tabs
//! without fetching them again. A copy younger than `CACHE_FRESH` is used
//! as is; an older one is refetched, and still used if that fails (offline,
//! say). Files untouched for `CACHE_PRUNE` are deleted at launch. A failed
//! fetch isn't recorded, so an origin without a favicon is tried once per
//! launch, as before.

use crate::worker::Pool;
use std::path::{Path, PathBuf};
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

/// Favicon fetches running at once; the rest wait in the pool. A restored
/// session can name dozens of origins at the same moment.
const MAX_FAVICON_FETCHES: usize = 4;

/// Fetches and decodes favicons on a `worker::Pool`, deduplicating
/// concurrent requests for the same origin (two tabs navigating to the same
/// site around the same time shouldn't fire two redundant HTTP requests).
/// Which origins already resolved this launch is `Workers::favicon_resolved`'s
/// job; this type knows only the requests in flight, and the disk cache.
pub struct FaviconFetcher {
    /// Keyed and jobbed by origin; a result of `None` is no favicon.
    pool: Pool<String, String, Option<FaviconImage>>,
}

impl FaviconFetcher {
    /// `cache_dir` is where favicons persist between launches (`None`: in
    /// memory only). Pruning it runs on its own thread.
    /// `fetch` is the network fetch: `Workers::new` passes `fetch_favicon`, tests a stub so no
    /// request leaves the machine.
    pub fn new(cache_dir: Option<PathBuf>, fetch: fn(&str) -> Option<FaviconImage>) -> Self {
        if let Some(dir) = cache_dir.clone() {
            std::thread::spawn(move || prune_cache(&dir, CACHE_PRUNE));
        }
        let pool = Pool::new(MAX_FAVICON_FETCHES, move |origin: &String| match &cache_dir {
            Some(dir) => cached_or_fetched(dir, origin, fetch),
            None => fetch(origin),
        });
        Self { pool }
    }

    /// Requests `origin`'s favicon if it isn't already being fetched: it
    /// starts at once while fewer than `MAX_FAVICON_FETCHES` run, and waits
    /// in the pool otherwise. A no-op if this origin is already running
    /// or waiting -- the caller is expected to have already checked its own
    /// cache for a previously-*completed* fetch (this only dedupes
    /// concurrent ones).
    pub fn request(&self, origin: String) {
        self.pool.request(origin.clone(), origin);
    }

    /// Call once per frame. Returns every fetch that completed since the
    /// last call -- `None` for the image means the fetch failed (no
    /// favicon at that origin, a network error, undecodable bytes, or a
    /// panic); the caller should still remember that as a negative result so
    /// it doesn't keep retrying the same origin every navigation.
    pub fn tick(&self) -> Vec<(String, Option<FaviconImage>)> {
        self.pool.tick().into_iter().map(|(origin, _, image)| (origin, image.flatten())).collect()
    }
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
pub(crate) fn fetch_favicon(origin: &str) -> Option<FaviconImage> {
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
    let image = image::open(path).ok()?.into_rgba8();
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

    /// The pool reports a panic as `None`; the fetcher reads it as "no favicon" for that origin.
    #[test]
    fn a_panicking_fetch_is_no_favicon() {
        let fetcher = FaviconFetcher::new(None, |_| panic!("stub fetch panics (expected in this test)"));
        fetcher.request("https://p.example".into());
        fetcher.pool.wait_for_threads(0);
        let done = fetcher.tick();
        assert!(matches!(done.as_slice(), [(origin, None)] if origin == "https://p.example"));
        assert_eq!(fetcher.pool.pending(), 0, "the origin is free again");
    }

    /// Queued origins start as threads free up, with no `tick` to start them.
    #[test]
    fn favicons_past_the_cap_finish_without_a_tick() {
        let fetcher = FaviconFetcher::new(None, |_| None);
        for n in 0..MAX_FAVICON_FETCHES + 3 {
            fetcher.request(format!("https://{n}.test"));
        }
        fetcher.pool.wait_for_threads(0);
        assert_eq!(fetcher.tick().len(), MAX_FAVICON_FETCHES + 3);
    }
}
