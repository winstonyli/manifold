use std::path::PathBuf;

use crate::process_manager::EngineKind;

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_search_engine: SearchEngine::DuckDuckGo,
            default_starting_engine: EngineKind::Servo,
            zoom_factor: 1.0,
            home_page: "https://example.com".to_string(),
        }
    }
}

/// `EngineKind` has no reason to derive serde elsewhere (it's an internal
/// process-management type, not otherwise persisted) — a small manual
/// string mapping here keeps that decoupled rather than adding a derive to
/// `process_manager` just for this one call site.
mod engine_kind_serde {
    use crate::process_manager::EngineKind;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(kind: &EngineKind, s: S) -> Result<S::Ok, S::Error> {
        let name = match kind {
            EngineKind::Servo => "servo",
            EngineKind::Cef => "cef",
            EngineKind::Webkit => "webkit",
            EngineKind::Webview2 => "webview2",
        };
        name.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<EngineKind, D::Error> {
        let name = String::deserialize(d)?;
        Ok(match name.as_str() {
            "cef" => EngineKind::Cef,
            "webkit" => EngineKind::Webkit,
            "webview2" => EngineKind::Webview2,
            _ => EngineKind::Servo,
        })
    }
}

fn settings_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("multiengine-browser").join("settings.json"))
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
        if let Some(dir) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(dir) {
                eprintln!("settings: failed to create {}: {err}", dir.display());
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&path, json) {
                    eprintln!("settings: failed to write {}: {err}", path.display());
                }
            }
            Err(err) => eprintln!("settings: failed to serialize: {err}"),
        }
    }
}
