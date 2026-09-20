use std::path::PathBuf;

use crate::process_manager::{engine_kind_serde, EngineKind};

/// Which search engine `resolve_navigation_url`/the autocomplete dropdown's
/// "Search for X" rows actually navigate to. Deliberately separate from
/// suggestion *fetching* — `SuggestionFetcher` (see `suggestions.rs`) always
/// queries DuckDuckGo's own `/ac/` endpoint regardless of this setting,
/// since Google's and Bing's autocomplete APIs are different-shaped
/// integrations we haven't built; the suggested *words* just get handed to
/// whichever engine is chosen here when a row is actually accepted. Not
/// pretending otherwise — the settings UI says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SearchEngine {
    DuckDuckGo,
    Google,
    Bing,
}

impl SearchEngine {
    pub const ALL: [SearchEngine; 3] = [SearchEngine::DuckDuckGo, SearchEngine::Google, SearchEngine::Bing];

    pub fn label(self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Google => "Google",
            SearchEngine::Bing => "Bing",
        }
    }

    /// Builds the actual search-results URL for `query` — used both for a
    /// plain Enter-navigate of non-URL text and for the dropdown's own
    /// "Search for X" rows, so what's offered always matches where it goes.
    pub fn search_url(self, query: &str) -> String {
        let encoded = crate::suggestions::urlencoding_light(query);
        match self {
            SearchEngine::DuckDuckGo => format!("https://duckduckgo.com/?q={encoded}"),
            SearchEngine::Google => format!("https://www.google.com/search?q={encoded}"),
            SearchEngine::Bing => format!("https://www.bing.com/search?q={encoded}"),
        }
    }
}

/// User-configurable, persisted-to-disk settings — everything here is a
/// *default* applied to newly spawned tabs (starting engine, home page,
/// zoom) or to future navigation (search engine); none of it retroactively
/// changes an already-open tab. Loaded once at startup (`Settings::load`),
/// saved on every change (`save`, fire-and-forget — a failed write is
/// logged, not fatal, since losing a settings change is annoying but never
/// breaks the browser itself).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub default_search_engine: SearchEngine,
    #[serde(with = "engine_kind_serde")]
    pub default_starting_engine: EngineKind,
    /// A linear multiplier (1.0 = 100%), matching WebKit's own
    /// `WebView::set_zoom_level`'s semantics directly — CEF's zoom API is
    /// log-scale internally (Chromium's own `zoom_level`, not a linear
    /// factor) and gets converted at the point of use (`main.rs`'s
    /// `send_zoom_to`), not stored converted here.
    pub zoom_factor: f64,
    pub home_page: String,
    /// The OS window's own size last time it changed -- in DPI-independent
    /// *logical points*, not physical pixels. Confirmed live why this
    /// matters: saving physical pixels and restoring via `PhysicalSize`
    /// caused the window to grow by exactly the monitor's scale factor on
    /// every single launch on a multi-monitor setup with mixed DPI --
    /// Windows' own Per-Monitor-V2 behavior rescales a window's *physical*
    /// size to preserve its *logical* size when it lands on a
    /// differently-scaled monitor than where that physical size was
    /// captured, and since the (now-rescaled, bigger) size got saved right
    /// back, it compounded by another factor on every restart (confirmed
    /// live: 2744 -> 3430 physical width across two consecutive launches,
    /// exactly `* 1.25`). Logical points are stable across monitors of any
    /// DPI, which is the whole reason winit's `LogicalSize` type exists.
    /// `None` means no saved geometry yet (a fresh install, or a
    /// `settings.json` from before this existed), falling back to winit's
    /// own default. `#[serde(default)]` so an older file missing these
    /// fields still loads instead of failing entirely (same fallback
    /// posture `Settings::load` already has for the whole file); an older
    /// file whose values were saved as *physical* pixels before this fix
    /// will restore once at the wrong (smaller, if the window was ever on a
    /// >100%-scaled monitor) logical size, then self-correct on the very
    /// next resize/save. Not applied or updated in headless mode, which
    /// already forces its own deliberate off-screen position (see
    /// `resumed`'s own doc comment) -- saving that would corrupt a real
    /// saved geometry the next normal launch would apply.
    #[serde(default)]
    pub window_width: Option<u32>,
    #[serde(default)]
    pub window_height: Option<u32>,
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
    /// Whether the one-time first-launch demo graph (see `App::
    /// spawn_first_launch_demo`) has already been shown. Defaults to `true`
    /// when *missing from an existing settings.json* (`default_already_shown`
    /// below) -- deliberately the opposite of this field's own `Default for
    /// Settings` value (`false`, for a genuinely fresh install with no
    /// settings.json at all) -- so upgrading from a build that predates
    /// this field never replays the demo for an existing user, only a
    /// brand-new one. Deliberately a `Settings` field rather than living in
    /// `session.json`: it needs to survive exactly the case `session.json`
    /// doesn't (a user with zero saved tabs), and `Settings` is the one
    /// file already guaranteed to persist across an empty session the same
    /// way `home_page`/`default_starting_engine` do.
    #[serde(default = "default_already_shown")]
    pub has_shown_first_launch_demo: bool,
}

fn default_already_shown() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_search_engine: SearchEngine::DuckDuckGo,
            default_starting_engine: EngineKind::Webview2,
            zoom_factor: 1.0,
            home_page: "https://example.com".to_string(),
            window_width: None,
            window_height: None,
            window_x: None,
            window_y: None,
            has_shown_first_launch_demo: false,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("manifold").join("settings.json"))
}

/// Cheap, process-local "did this file's bytes change" check -- see
/// `session::content_fingerprint`'s own doc comment (this is the same
/// idea, kept as its own small copy per that module's own doc comment
/// rather than shared, since `session.rs`/`settings.rs` are otherwise
/// already fully independent of each other).
fn content_fingerprint(contents: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

/// A fingerprint of `settings.json` exactly as it sits on disk right now --
/// see `session::fingerprint`'s own doc comment for the full reasoning;
/// this is the identical mechanism applied to the other single-shared-file
/// auto-save path.
pub fn fingerprint() -> Option<u64> {
    let contents = std::fs::read_to_string(settings_path()?).ok()?;
    Some(content_fingerprint(&contents))
}

/// Where a save that lost a fingerprint race lands instead of overwriting
/// the real `settings.json` -- see `session::conflict_path`'s own doc
/// comment for why this is timestamped rather than a fixed name.
pub fn conflict_path() -> Option<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("manifold").join(format!("settings.conflict-{secs}.json")))
}

impl Settings {
    /// Missing file, unreadable/corrupt JSON, or no `%APPDATA%` at all
    /// (unusual on Windows, but not worth panicking over) all just fall
    /// back to defaults — a broken settings file should never stop the
    /// browser from starting.
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else {
            return;
        };
        self.save_to(&path);
    }

    /// Same write as `save`, just to an arbitrary path -- used by `App::
    /// save_settings` to redirect a fingerprint-conflicted save to `settings::
    /// conflict_path()` instead of overwriting the real `settings.json`.
    pub fn save_to(&self, path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(dir) {
                eprintln!("settings: failed to create {}: {err}", dir.display());
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(err) = std::fs::write(path, json) {
                    eprintln!("settings: failed to write {}: {err}", path.display());
                }
            }
            Err(err) => eprintln!("settings: failed to serialize: {err}"),
        }
    }
}
