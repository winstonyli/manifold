/// Shows a native "Save As" dialog defaulting to `default_name`, filtered
/// to `.json`; if a path was picked, returns it along with the exact
/// "Saved <path>" toast text this app shows for every export (Session,
/// Bookmarks, one Group) on success. Doesn't write anything itself, and
/// deliberately isn't a method on `App` -- the caller still does its own
/// serialization (`self.export_session_to(&path)`, etc.)
/// and sets `self.toasts.save_toast`, which a `&mut self` helper here couldn't
/// do without a borrow conflict against a closure also wanting to touch
/// `self`. Previously this dialog-plus-toast tail was hand-copied three
/// times, differing only in the default filename and the filter's label.
pub(crate) fn save_dialog_with_toast(
    default_name: impl Into<String>,
    filter_label: &str,
) -> Option<(std::path::PathBuf, (String, std::time::Instant))> {
    let path =
        rfd::FileDialog::new().set_file_name(default_name.into()).add_filter(filter_label, &["json"]).save_file()?;
    let toast = (format!("Saved {}", path.display()), std::time::Instant::now());
    Some((path, toast))
}

/// Writes a JSON snapshot (spec "JSON snapshots") to `path`, pretty-
/// printed, logging (not toasting) a failure -- shared by every Export
/// Group site (headless `EXPORTGROUP`, the node menu's own "Export
/// Group..." and the popup's), matching the old per-group export file's
/// own save, which likewise only logged. `what` names the file's kind for
/// the log line, matching `json_file::load`'s own `what`.
pub(crate) fn write_json_export(json: &crate::store::json::JsonDoc, path: &std::path::Path, what: &str) {
    let written = serde_json::to_string_pretty(json).map_err(|e| e.to_string()).and_then(|text| {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        crate::store::atomic::replace(path, text.as_bytes()).map_err(|e| e.to_string())
    });
    if let Err(err) = written {
        eprintln!("{what}: couldn't export to {}: {err}", path.display());
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Export Group writes through `crate::store::atomic::replace`: a blocked target stays unwritten.
    #[test]
    fn a_group_export_is_written_through_the_atomic_replace() {
        let path = crate::store::testutil::temp_dir("group-export-atomic").join("g.group.json");
        let json = crate::store::json::JsonDoc::default();
        crate::store::atomic::block_for_test(&path, true);
        write_json_export(&json, &path, "group");
        assert!(!path.exists(), "the write went around crate::store::atomic::replace");
        crate::store::atomic::block_for_test(&path, false);
        write_json_export(&json, &path, "group");
        assert_eq!(crate::json_file::load::<crate::store::json::JsonDoc>(&path, "group"), Ok(json));
    }
}
