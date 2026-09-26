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
    /// as-is on import (which allocates fresh ids via `App::alloc_node_id`,
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
    /// The nested group (a `GroupExportGroup::id`) this tab sits in;
    /// `None` for the exported group itself.
    #[serde(default)]
    pub group: Option<String>,
}

/// A group nested, at any depth, inside the exported one. Its id, like a
/// tab's, only links members within this export.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupExportGroup {
    pub id: String,
    pub name: String,
    pub color: Option<(u8, u8, u8)>,
    pub pos_x: f32,
    pub pos_y: f32,
    /// Same as `GroupExportTab::group`.
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupExport {
    pub name: String,
    /// `egui::Color32` isn't `Serialize` -- stored as a plain RGB triple
    /// and converted at the call site.
    pub color: Option<(u8, u8, u8)>,
    pub tabs: Vec<GroupExportTab>,
    /// Empty in files from before nested groups were exported.
    #[serde(default)]
    pub groups: Vec<GroupExportGroup>,
}

pub fn load_from(path: &Path) -> Result<GroupExport, String> {
    crate::json_file::load(path, "group")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_without_nested_groups_loads() {
        let json = r#"{"name": "Reading", "color": null, "tabs": [
            {"id": "tab-1", "parent": null, "url": "https://example.com/", "engine": "webview2", "pos_x": 0.0, "pos_y": 0.0}]}"#;
        let export: GroupExport = serde_json::from_str(json).unwrap();
        assert!(export.groups.is_empty());
        assert_eq!(export.tabs[0].group, None);
    }
}
