use std::path::PathBuf;

use serde::Deserialize as _;

use crate::json_file::ProfileFile;
use crate::process_manager::{view_kind_serde, ViewKind};

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

/// Which look the app has: the system's (egui follows winit's theme events), or one fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
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
/// `#[serde(default)]`: a file missing some fields keeps the ones it has
/// and takes `Default` for the rest (field-level defaults, like
/// `has_shown_first_launch_demo`'s, still win).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Light, dark, or the system's (`theme::apply_theme_choice`).
    #[serde(deserialize_with = "theme_choice")]
    pub theme: ThemeChoice,
    #[serde(deserialize_with = "search_engine")]
    pub default_search_engine: SearchEngine,
    #[serde(serialize_with = "view_kind_serde::serialize", deserialize_with = "starting_view_kind")]
    pub default_starting_view_kind: ViewKind,
    /// A linear multiplier (1.0 = 100%), matching WebKit's own
    /// `WebView::set_zoom_level`'s semantics directly — CEF's zoom API is
    /// log-scale internally (Chromium's own `zoom_level`, not a linear
    /// factor) and gets converted at the point of use (`main.rs`'s
    /// `send_zoom_to`), not stored converted here.
    #[serde(deserialize_with = "zoom")]
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
    /// restores those numbers as if they were logical points, which on a
    /// monitor scaled past 100% makes the window come back *larger* than it was
    /// (e.g. 2744 saved physical pixels reopen as 2744 points = 3430
    /// physical at 125%). That doesn't self-correct: the next save records
    /// the new, inflated size in points, so the window simply stays at it
    /// from then on, and the user has to resize it once by hand. It does
    /// stop growing, which is the runaway this fix was actually for. Not
    /// applied or updated in headless mode, which
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
    /// the session store: it needs to survive exactly the case the session
    /// doesn't (a user with zero saved tabs), and `Settings` is the one
    /// file already guaranteed to persist across an empty session the same
    /// way `home_page`/`default_starting_view_kind` do.
    #[serde(default = "default_already_shown")]
    pub has_shown_first_launch_demo: bool,
    /// Minutes a background tab may sit out of sight before it goes back to
    /// sleep (`App::request_sleep`); `None` (the default) never does. Applies to
    /// tabs already open, unlike the new-tab defaults above. See
    /// `idle_sleep`.
    #[serde(default)]
    pub idle_sleep_minutes: Option<u32>,
    /// A CEF runtime directory the user supplied instead of downloading one (`view_install::
    /// validate_custom`); used for launches only while it validates.
    #[serde(default)]
    pub cef_directory: Option<String>,
    /// Where downloads are saved: an absolute folder, made at the first download if missing. `None` (or text that
    /// is not a full path) means the user's Downloads folder (`profile::downloads_dir`).
    #[serde(default)]
    pub download_directory: Option<String>,
}

fn default_already_shown() -> bool {
    true
}

/// A typed or pasted folder as a setting holds it: trimmed, and one pair of surrounding quotes dropped (Explorer's
/// "Copy as path" adds them), then trimmed again.
pub(crate) fn typed_folder(text: &str) -> &str {
    let text = text.trim();
    text.strip_prefix('"').and_then(|t| t.strip_suffix('"')).map_or(text, str::trim)
}

/// The folder a folder setting names, in Windows' own form (`std::path::absolute`: `/` read as `\`, `.` and `..`
/// resolved, no disk read), so every path built on it reads one separator; the setting keeps the text as typed.
/// `None` when the text is not a full path, which a folder setting does not use.
pub(crate) fn full_folder(text: &str) -> Option<PathBuf> {
    let path = std::path::Path::new(text);
    path.is_absolute().then(|| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_search_engine: SearchEngine::DuckDuckGo,
            theme: ThemeChoice::System,
            default_starting_view_kind: crate::process_manager::platform_default_view_kind(manifold_hosts::Os::here()),
            zoom_factor: 1.0,
            home_page: "manifold://start".to_string(),
            window_width: None,
            window_height: None,
            window_x: None,
            window_y: None,
            has_shown_first_launch_demo: false,
            idle_sleep_minutes: None,
            cef_directory: None,
            download_directory: None,
        }
    }
}

/// The page zoom the UI steps through (`zoom_in_shortcut`, `zoom_out_shortcut`, the settings page's buttons); a
/// stored value outside it loads clamped into it.
pub(crate) const ZOOM_RANGE: std::ops::RangeInclusive<f64> = 0.5..=2.0;

/// A value this build can't read (a newer build's name, a hand edit) loads as that setting's default, logged, rather
/// than failing the whole file, which `load_or_move_aside` would set aside with every other setting (H4).
fn lenient<'de, D: serde::Deserializer<'de>, T>(
    d: D,
    what: &str,
    read: impl FnOnce(&serde_json::Value) -> Option<T>,
    default: impl FnOnce() -> T,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(read(&value).unwrap_or_else(|| {
        eprintln!("settings: unreadable {what} {value}; using the default");
        default()
    }))
}

fn search_engine<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SearchEngine, D::Error> {
    lenient(
        d,
        "default_search_engine",
        |v| SearchEngine::deserialize(v).ok(),
        || Settings::default().default_search_engine,
    )
}

fn theme_choice<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ThemeChoice, D::Error> {
    lenient(d, "theme", |v| ThemeChoice::deserialize(v).ok(), || Settings::default().theme)
}

fn starting_view_kind<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ViewKind, D::Error> {
    lenient(
        d,
        "default_starting_view_kind",
        |v| v.as_str().and_then(ViewKind::from_name),
        || Settings::default().default_starting_view_kind,
    )
}

fn zoom<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    lenient(d, "zoom_factor", |v| v.as_f64().map(|z| z.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end())), || 1.0)
}

const SETTINGS: ProfileFile = ProfileFile { file: "settings.json", what: "settings" };

/// Cheap, process-local "did this file's bytes change" check -- not a
/// cryptographic or cross-run-stable hash, just `DefaultHasher` (SipHash)
/// over the raw file contents, good enough to notice that some *other*
/// process touched `settings.json` since this one last read or wrote it.
fn content_fingerprint(contents: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    contents.hash(&mut hasher);
    hasher.finish()
}

/// A fingerprint of `settings.json` exactly as it sits on disk right now --
/// `None` if it doesn't exist or can't be read. `App::save_settings`
/// compares it against what it saw at last load/save, so a second
/// `manifold.exe` on this profile that changed the file meanwhile is
/// noticed instead of silently clobbered.
pub fn fingerprint() -> Option<u64> {
    let contents = std::fs::read_to_string(SETTINGS.path()?).ok()?;
    Some(content_fingerprint(&contents))
}

/// Where a save that lost a fingerprint race lands instead of overwriting
/// the real `settings.json`. Unix-seconds-suffixed, and numbered within a
/// second, so a second conflict later in the same run can't overwrite the
/// first one's unreviewed data.
pub fn conflict_path() -> Option<PathBuf> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Some(conflict_path_in(&crate::profile::dir()?, secs))
}

/// `settings.conflict-<secs>.json`, or `-<n>` after the seconds when that
/// name is taken, so a second conflict in the same second keeps the first.
/// The name is claimed by creating the file (`create_new`), so two processes
/// can't be handed one name; the save that follows replaces the empty file. If
/// the directory can't be written, the first name comes back and the save fails
/// visibly as it would have.
fn conflict_path_in(dir: &std::path::Path, secs: u64) -> PathBuf {
    for n in 0u32.. {
        let path = match n {
            0 => dir.join(format!("settings.conflict-{secs}.json")),
            n => dir.join(format!("settings.conflict-{secs}-{n}.json")),
        };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            _ => return path,
        }
    }
    unreachable!("an unbounded range finds a free name")
}

impl Settings {
    /// No `%APPDATA%` or no `settings.json`: defaults. An unreadable one
    /// also gives defaults, but is moved aside first and reported as the
    /// `Err` half (see `json_file::load_or_move_aside`) -- a broken settings
    /// file should never stop the browser from starting.
    pub fn load() -> (Self, Option<String>) {
        SETTINGS.load()
    }

    pub fn save(&self) {
        SETTINGS.save(self);
    }

    /// Same write as `save`, just to an arbitrary path -- used by `App::
    /// save_settings` to redirect a fingerprint-conflicted save to `settings::
    /// conflict_path()` instead of overwriting the real `settings.json`.
    pub fn save_to(&self, path: &std::path::Path) {
        crate::json_file::save(self, path, SETTINGS.what);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_conflict_in_one_second_gets_its_own_name() {
        let dir = crate::store::testutil::temp_dir("settings_conflict");
        let first = conflict_path_in(&dir, 7);
        assert_eq!(first, dir.join("settings.conflict-7.json"));
        // The name is claimed on return: no write between the calls, and the next caller still gets another.
        assert!(first.exists());
        let second = conflict_path_in(&dir, 7);
        assert_eq!(second, dir.join("settings.conflict-7-1.json"));
        assert_eq!(conflict_path_in(&dir, 7), dir.join("settings.conflict-7-2.json"));
    }

    #[test]
    fn cef_directory_defaults_when_missing_and_round_trips() {
        let old: Settings = serde_json::from_str(r#"{"home_page":"https://a.test"}"#).unwrap();
        assert_eq!(old.cef_directory, None);
        let settings = Settings { cef_directory: Some("D:/cef".into()), ..Settings::default() };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back.cef_directory.as_deref(), Some("D:/cef"));
    }

    #[test]
    fn download_directory_defaults_when_missing_and_round_trips() {
        let old: Settings = serde_json::from_str(r#"{"home_page":"https://a.test"}"#).unwrap();
        assert_eq!(old.download_directory, None);
        let settings = Settings { download_directory: Some("D:\\Zoë Q\\dl".into()), ..Settings::default() };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back.download_directory.as_deref(), Some("D:\\Zoë Q\\dl"));
    }

    /// Review Focus 1: Explorer's "Copy as path" quotes a folder; the setting holds it without them.
    #[test]
    fn a_pasted_folder_loses_its_quotes_and_padding() {
        assert_eq!(typed_folder(" \"C:\\Users\\Zoë Q\\Downloads\" "), "C:\\Users\\Zoë Q\\Downloads");
        assert_eq!(typed_folder("\" C:\\a \""), "C:\\a");
        assert_eq!(typed_folder("C:\\a"), "C:\\a");
        assert_eq!(typed_folder("  "), "");
        assert_eq!(typed_folder("\"\""), "");
        assert_eq!(typed_folder("\""), "\"", "a lone quote is not a pair");
    }

    /// Review Focus 1: a full path typed with `/` (from a shell or a config file) is the folder in Windows' own form,
    /// so every download path built on it, and Explorer's `/select` on Reveal, read one separator. Compared as text:
    /// `Path` equality ignores the separator.
    #[cfg(windows)]
    #[test]
    fn a_full_folder_is_read_in_windows_form() {
        let text = |t: &str| full_folder(t).map(|d| d.to_string_lossy().into_owned());
        assert_eq!(text("D:/dl").as_deref(), Some(r"D:\dl"));
        let file = full_folder("D:/dl/").map(|d| d.join("a.zip").to_string_lossy().into_owned());
        assert_eq!(file.as_deref(), Some(r"D:\dl\a.zip"), "no doubled separator");
        assert_eq!(text(r"D:\dl\..\x").as_deref(), Some(r"D:\x"));
        assert_eq!(text("//nas/share/dl").as_deref(), Some(r"\\nas\share\dl"));
        assert_eq!(text(r"C:\Users\Zoë Q\Downloads").as_deref(), Some(r"C:\Users\Zoë Q\Downloads"));
        assert_eq!(full_folder("Downloads2"), None);
        assert_eq!(full_folder("D:dl"), None, "drive-relative");
        assert_eq!(full_folder(""), None);
    }

    /// A file without the field takes the platform default; a saved engine is kept even where it can't run.
    #[test]
    fn the_starting_view_defaults_per_platform_and_a_saved_one_is_kept() {
        let old: Settings = serde_json::from_str(r#"{"home_page":"https://a.test"}"#).unwrap();
        assert_eq!(
            old.default_starting_view_kind,
            crate::process_manager::platform_default_view_kind(manifold_hosts::Os::here())
        );
        let saved: Settings = serde_json::from_str(r#"{"default_starting_view_kind":"webview2"}"#).unwrap();
        assert_eq!(saved.default_starting_view_kind, ViewKind::Webview2);
    }

    /// H4: a value this build can't read resets that setting alone, not the whole file.
    #[test]
    fn an_unknown_value_resets_only_its_own_setting() {
        let s: Settings = serde_json::from_str(
            r#"{"home_page":"https://a.test","default_search_engine":"Kagi",
                "default_starting_view_kind":"chrome","zoom_factor":0,"cef_directory":"D:/cef"}"#,
        )
        .unwrap();
        assert_eq!(s.home_page, "https://a.test");
        assert_eq!(s.cef_directory.as_deref(), Some("D:/cef"));
        assert_eq!(s.default_search_engine, SearchEngine::DuckDuckGo);
        assert_eq!(s.default_starting_view_kind, Settings::default().default_starting_view_kind);
        assert_eq!(s.zoom_factor, 0.5, "clamped into the range the UI steps through");
    }

    #[test]
    fn theme_defaults_to_system_and_round_trips() {
        let old: Settings = serde_json::from_str(r#"{"home_page":"https://a.test"}"#).unwrap();
        assert_eq!(old.theme, ThemeChoice::System);
        let dark = Settings { theme: ThemeChoice::Dark, ..Settings::default() };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&dark).unwrap()).unwrap();
        assert_eq!(back.theme, ThemeChoice::Dark);
        let bad: Settings = serde_json::from_str(r#"{"theme":"sepia"}"#).unwrap();
        assert_eq!(bad.theme, ThemeChoice::System);
    }

    /// H4: zoom loads clamped, a wrongly typed zoom is the default, and the defaults round-trip.
    #[test]
    fn zoom_is_clamped_and_a_wrong_type_is_its_default() {
        let zoom = |text: &str| serde_json::from_str::<Settings>(text).unwrap().zoom_factor;
        assert_eq!(zoom(r#"{"zoom_factor":9}"#), 2.0);
        assert_eq!(zoom(r#"{"zoom_factor":"1.2"}"#), 1.0);
        assert_eq!(zoom(r#"{"zoom_factor":1.3}"#), 1.3);
        let defaults = Settings::default();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&defaults).unwrap()).unwrap();
        assert_eq!(back.default_search_engine, defaults.default_search_engine);
        assert_eq!(back.default_starting_view_kind, defaults.default_starting_view_kind);
        assert_eq!(back.zoom_factor, defaults.zoom_factor);
    }
}
