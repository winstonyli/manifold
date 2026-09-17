//! Export/import a single group's own subtree to an arbitrary file — same
//! "share/back up outside the app" motivation `session.rs`'s Export/Import
//! Session has, just scoped to one group's members instead of the whole
//! graph. Not persisted anywhere on its own (no auto-save file, unlike
//! `session.rs`/`bookmarks.rs`) -- a plain one-shot save/load through a real
//! file dialog (or, headlessly, an explicit path).

use std::path::Path;

use crate::process_manager::{engine_kind_serde, EngineKind};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupExportTab {
    /// The exporting session's own tab id -- reused only to resolve
    /// `parent` links between tabs within this same export, never reused
    /// as-is on import (which allocates fresh ids via `App::alloc_tab_id`,
    /// since importing into an already-running session can't assume the
    /// old ids are free).
    pub id: String,
    /// `Some` only when the parent is also a member of this same group
    /// (and thus also present in `tabs`) -- a member whose real parent
    /// was outside the group becomes a root within the imported copy,
    /// since that outside tab isn't part of the export at all.
    pub parent: Option<String>,
    pub url: String,
    #[serde(with = "engine_kind_serde")]
    pub engine: EngineKind,
    pub pos_x: f32,
    pub pos_y: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupExport {
    pub name: String,
    /// `egui::Color32` isn't `Serialize` -- stored as a plain RGB triple
    /// and converted at the call site.
    pub color: Option<(u8, u8, u8)>,
    pub tabs: Vec<GroupExportTab>,
}

pub fn load_from(path: &Path) -> Option<GroupExport> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn save_to(export: &GroupExport, path: &Path) {
    if let Some(dir) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(dir) {
            eprintln!("group_export: failed to create {}: {err}", dir.display());
            return;
        }
    }
    match serde_json::to_string_pretty(export) {
        Ok(json) => {
            if let Err(err) = std::fs::write(path, json) {
                eprintln!("group_export: failed to write {}: {err}", path.display());
            }
        }
        Err(err) => eprintln!("group_export: failed to serialize: {err}"),
    }
}
