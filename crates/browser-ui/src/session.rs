//! Session persistence: the tab graph's shape (which tabs, their url,
//! engine, position, and who opened whom), which tab is focused, and which
//! view is active are saved to disk on every change that matters and
//! reloaded at startup — `App::restore_session` replays it instead of
//! always spawning a single fresh root tab, so quitting and relaunching
//! picks up roughly where it left off. Like `settings.rs`'s own file, a
//! missing or corrupt one just falls back to the old single-root-tab
//! behavior rather than failing to start.
//!
//! Only the graph's *shape* is portable across a restart — page state
//! (cookies, localStorage, IndexedDB, scroll position) is not, the same
//! "known v1 limitation" the README already documents for an engine swap;
//! a restored tab is a fresh navigation to its last known url, not a
//! resumed one.

use std::path::PathBuf;

use crate::process_manager::{engine_kind_serde, EngineKind};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionTab {
    pub id: String,
    pub parent: Option<String>,
    pub url: String,
    #[serde(with = "engine_kind_serde")]
    pub engine: EngineKind,
    /// Graph-view layout position — restoring it keeps a familiar-looking
    /// layout across a restart instead of every tab re-jittering from the
    /// same spawn point and re-settling from scratch.
    pub pos_x: f32,
    pub pos_y: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub focused: Option<String>,
    pub view_mode_tab: bool,
    pub next_tab_id: u64,
}

fn session_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("multiengine-browser").join("session.json"))
}

impl Session {
    /// Missing file, unreadable/corrupt JSON, or no `%APPDATA%` at all all
    /// just return `None` — the caller falls back to spawning a single
    /// fresh root tab, same posture as `Settings::load`'s own fallback.
    pub fn load() -> Option<Self> {
        let path = session_path()?;
        let contents = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&contents).ok()
    }

    pub fn save(&self) {
        let Some(path) = session_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(dir) {
                eprintln!("session: failed to create {}: {err}", dir.display());
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&path, json) {
                    eprintln!("session: failed to write {}: {err}", path.display());
                }
            }
            Err(err) => eprintln!("session: failed to serialize: {err}"),
        }
    }
}
