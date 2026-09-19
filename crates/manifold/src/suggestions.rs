use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::settings::SearchEngine;

/// One row in the address bar's autocomplete dropdown.
#[derive(Debug, Clone)]
pub enum Suggestion {
    /// Jump to (focus) an already-open tab instead of navigating anywhere.
    OpenTab {
        tab_id: String,
        url: String,
        title: String,
    },
    /// Run `query` as a search — only ever the address bar's own typed
    /// text as a whole, not a per-word thing, matching how every real
    /// browser's omnibox treats non-URL input.
    Search { query: String },
}

impl Suggestion {
    pub fn label(&self) -> String {
        match self {
            Suggestion::OpenTab { title, url, .. } => {
                if title.is_empty() {
                    url.clone()
                } else {
                    format!("{title} — {url}")
                }
            }
            Suggestion::Search { query } => format!("Search for \u{201c}{query}\u{201d}"),
        }
    }
}

/// Whether `input` should be treated as a URL to navigate to directly
/// (`https://...`, `example.com`, `localhost:8080`) versus a search query
/// to hand off to a search engine — the same fork every real browser's
/// address bar makes. Deliberately simple (no full URL-syntax validation):
/// a scheme prefix, or a single word containing a dot and no whitespace
/// (a bare domain), counts as a URL; anything with a space, or plain words
/// with no dot, is a search query.
pub fn looks_like_url(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains(char::is_whitespace) {
        return false;
    }
    if trimmed.contains("://") {
        return true;
    }
    // A bare domain/host (`example.com`, `localhost:8080`) — requires a
    // dot (or is literally "localhost") since a single word with neither
    // is far more likely a search term ("rust", "weather") than a host.
    let host_part = trimmed.split(['/', '?', '#']).next().unwrap_or(trimmed);
    let host_only = host_part.split(':').next().unwrap_or(host_part);
    host_only == "localhost" || host_only.contains('.')
}

/// The URL to actually navigate to for the current address-bar text: as
/// typed if it already looks like a URL (adding `https://` when no scheme
/// is present, since engines generally require one), otherwise a search via
/// `search_engine` (the user's configured default — see `settings.rs`).
pub fn resolve_navigation_url(input: &str, search_engine: SearchEngine) -> String {
    let trimmed = input.trim();
    if looks_like_url(trimmed) {
        if trimmed.contains("://") {
            trimmed.to_string()
        } else {
            format!("https://{trimmed}")
        }
    } else {
        search_engine.search_url(trimmed)
    }
}

/// A tiny percent-encoder covering what a search query actually needs
/// (space and the usual URL-structural characters) — not a full RFC 3986
/// implementation, since a search query is free text, not a URL component
/// with sub-delimiters that matter. `pub(crate)`: also used by
/// `settings::SearchEngine::search_url` for the same reason.
pub(crate) fn urlencoding_light(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Debounced, generation-tagged fetcher for DuckDuckGo's autocomplete
/// endpoint. Runs each HTTP request on its own throwaway thread (`manifold`
/// has no async runtime — this is the same "just spawn a thread" pattern
/// `ipc_protocol`'s transport reader threads already use) and drops any
/// response whose generation doesn't match the most recent query, so a
/// slow/late response for text the user has since changed or cleared can
/// never clobber a newer (or absent) result.
pub struct SuggestionFetcher {
    tx: mpsc::Sender<(u64, Vec<String>)>,
    rx: mpsc::Receiver<(u64, Vec<String>)>,
    generation: u64,
    pending_query: Option<String>,
    pending_since: Option<Instant>,
    last_fetched: Option<String>,
}

const DEBOUNCE: Duration = Duration::from_millis(200);

/// Spawns the one-time TLS/DNS warm-up this process's very first real HTTPS
/// request needs. Found live: the *first* HTTPS request this process ever
/// makes (rustls/ureq's one-time TLS setup — root store loading, cipher
/// negotiation tables, etc. — plus a cold DNS+TLS handshake) is slow enough
/// that doing it for the first time exactly when the user starts typing a
/// search query reliably tripped Servo's engine-host into a crash/respawn
/// loop (confirmed reproducible: never happened on the *second* search of a
/// session, only ever the first). Absorbing that one-time cost here, at
/// startup on a throwaway background thread, keeps it off the critical path
/// by the time it's actually needed. The result is discarded — this is pure
/// warm-up, not a real query.
///
/// Called once, centrally, from `App::default()` — not from inside
/// `SuggestionFetcher::new()` the way it used to be, once a second network
/// consumer (favicon fetching) existed too: each doing its own redundant
/// warm-up would be wasteful, and worse, whichever one wasn't constructed
/// first could still race a real request against the *other's* not-yet-
/// finished warm-up. One shared call, before either fetcher is built,
/// avoids both problems.
pub fn warm_up_https_client() {
    std::thread::spawn(|| {
        let _ = fetch_suggestions("a");
    });
}

impl SuggestionFetcher {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            generation: 0,
            pending_query: None,
            pending_since: None,
            last_fetched: None,
        }
    }

    /// Call whenever the address bar's text actually changes (not every
    /// frame) — starts (or restarts) the debounce window and invalidates
    /// any in-flight/stale request via a fresh generation.
    pub fn note_text_changed(&mut self, text: &str) {
        self.generation += 1;
        if looks_like_url(text) || text.trim().is_empty() {
            // Already a URL (or empty) — no search suggestions wanted;
            // clear so a stale debounce timer doesn't fire one later.
            self.pending_query = None;
            self.pending_since = None;
        } else {
            self.pending_query = Some(text.trim().to_string());
            self.pending_since = Some(Instant::now());
        }
    }

    /// Call once per frame. Fires the debounced HTTP fetch if it's due,
    /// and returns freshly-arrived, still-relevant suggestions if any
    /// landed this tick (an empty `Vec` clears previous ones — e.g. the
    /// query stopped looking like a search or the request failed).
    pub fn tick(&mut self) -> Option<Vec<String>> {
        if let (Some(query), Some(since)) = (&self.pending_query, self.pending_since) {
            if since.elapsed() >= DEBOUNCE && self.last_fetched.as_deref() != Some(query.as_str()) {
                self.last_fetched = Some(query.clone());
                spawn_fetch(query.clone(), self.generation, self.tx.clone());
            }
        }

        let mut latest = None;
        while let Ok((gen, results)) = self.rx.try_recv() {
            if gen == self.generation {
                latest = Some(results);
            }
        }
        latest
    }
}

fn spawn_fetch(query: String, generation: u64, tx: mpsc::Sender<(u64, Vec<String>)>) {
    std::thread::spawn(move || {
        let results = fetch_suggestions(&query).unwrap_or_default();
        let _ = tx.send((generation, results));
    });
}

/// DuckDuckGo's `type=list` autocomplete endpoint — same one commonly used
/// by third-party tools for this exact purpose, no API key needed. Returns
/// a 2-element array, `["<the query echoed back>", ["suggestion one",
/// "suggestion two", ...]]` (confirmed directly against the live endpoint,
/// not just assumed from docs — the `type=list` variant's second element
/// is plain strings, not `{"phrase": "..."}` objects like some of
/// DuckDuckGo's *other* autocomplete response shapes use). A network
/// error, non-200, or unparseable body just yields no suggestions (see
/// `SuggestionFetcher`'s doc comment — this is cosmetic, never
/// load-bearing).
fn fetch_suggestions(query: &str) -> Option<Vec<String>> {
    let url = format!(
        "https://duckduckgo.com/ac/?q={}&type=list",
        urlencoding_light(query)
    );
    let response = ureq::get(&url)
        .config()
        .timeout_global(Some(Duration::from_secs(3)))
        .build()
        .call()
        .ok()?;
    let body = response.into_body().with_config().limit(200_000).read_to_vec().ok()?;
    let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
    let phrases = value.as_array()?.get(1)?.as_array()?;
    Some(
        phrases
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
    )
}
