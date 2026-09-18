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
//! its own network fetch (`browser-ui` has no async runtime) -- keyed by
//! origin rather than per-request generation, since a favicon fetch has no
//! notion of "stale" the way a debounced search query does: whichever
//! result for a given origin arrives is just as valid whenever it lands.

use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Duration;

/// A successfully decoded favicon: raw RGBA8 pixels plus dimensions, the
/// same shape `GpuState::upload_favicon` already expects.
pub struct FaviconImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Fetches and decodes favicons on background threads, deduplicating
/// concurrent requests for the same origin (two tabs navigating to the same
/// site around the same time shouldn't fire two redundant HTTP requests).
/// Does *not* itself cache successful results across origins already seen —
/// that's `App::favicon_cache`'s job, since this type only needs to know
/// about requests currently in flight, not history.
pub struct FaviconFetcher {
    tx: mpsc::Sender<(String, Option<FaviconImage>)>,
    rx: mpsc::Receiver<(String, Option<FaviconImage>)>,
    in_flight: HashSet<String>,
}

impl FaviconFetcher {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, in_flight: HashSet::new() }
    }

    /// Requests `origin`'s favicon if it isn't already being fetched. A
    /// no-op if a request for this exact origin is already in flight --
    /// the caller is expected to have already checked its own cache for a
    /// previously-*completed* fetch (this only dedupes concurrent ones).
    pub fn request(&mut self, origin: String) {
        if !self.in_flight.insert(origin.clone()) {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = fetch_favicon(&origin);
            let _ = tx.send((origin, result));
        });
    }

    /// Call once per frame. Returns every fetch that completed since the
    /// last call -- `None` for the image means the fetch failed (no
    /// favicon at that origin, a network error, or undecodable bytes);
    /// the caller should still remember that as a negative result so it
    /// doesn't keep retrying the same origin every navigation.
    pub fn tick(&mut self) -> Vec<(String, Option<FaviconImage>)> {
        let mut results = Vec::new();
        while let Ok((origin, result)) = self.rx.try_recv() {
            self.in_flight.remove(&origin);
            results.push((origin, result));
        }
        results
    }
}

/// The `scheme://host[:port]` prefix of `url`, or `None` if `url` has no
/// `://` at all (a `data:`/`about:` URL, or already-malformed input) --
/// deliberately simple hand-rolled parsing, same "no full URL-syntax
/// validation" posture `suggestions::looks_like_url` already documents,
/// rather than pulling in a real URL-parsing crate for just this.
pub fn url_origin(url: &str) -> Option<String> {
    let scheme_end = url.find("://")?;
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return None;
    }
    Some(format!("{}://{authority}", &url[..scheme_end]))
}

/// `GET {origin}/favicon.ico`, decoded to RGBA8 -- the same well-established
/// browser convention every engine's own native favicon API ultimately
/// falls back to when a page doesn't advertise one explicitly via a
/// `<link rel="icon">` (this project doesn't parse page HTML at all, so
/// that richer path isn't available here; the plain `/favicon.ico` guess
/// covers the overwhelming majority of real sites without needing it).
/// Same size/timeout caps as `engine-host-cef`'s own now-dead `fetch_favicon`
/// this was ported from -- a favicon is always small, so a generous cap
/// here is purely a safety net against a misbehaving server, not a real
/// constraint.
fn fetch_favicon(origin: &str) -> Option<FaviconImage> {
    let url = format!("{origin}/favicon.ico");
    let response = ureq::get(&url)
        .config()
        .timeout_global(Some(Duration::from_secs(3)))
        .build()
        .call()
        .ok()?;
    let bytes = response.into_body().with_config().limit(2_000_000).read_to_vec().ok()?;
    let image = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(FaviconImage { rgba: image.into_raw(), width, height })
}
