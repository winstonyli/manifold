//! What the app says about a failed download (`manifold_view_protocol::DownloadFailure`): the card's face, the
//! Downloads popup row, the toast and the tooltip, worded here and nowhere else. The face draws unclipped at a fixed
//! 13 points on download cards down to about 116 points wide (`card::DOWNLOAD_NODE_SIZE` is 160 by 110, and
//! `card::tab_has_header` draws it from 80 points tall), so each short reason must fit that width in the app's font
//! (`tests::every_face_fits_the_smallest_card_and_only_other_is_plain` measures it). The engine's or OS's own words
//! (`detail`) go in the tooltip only: the face, the popup and the toast take the kind alone.

use std::path::{Path, PathBuf};

use manifold_view_protocol::{DownloadFailure, FailureKind};

use crate::store::model::NodeId;
use crate::App;

/// The few words for `kind` on the card, the popup row and the toast; `None` for `Other`, which says no more than
/// that it failed.
fn reason(kind: FailureKind) -> Option<&'static str> {
    match kind {
        FailureKind::FolderUnreachable => Some("Can't reach folder"),
        FailureKind::FolderUnusable => Some("Can't save here"),
        FailureKind::DiskFull => Some("Disk full"),
        FailureKind::Blocked => Some("Blocked"),
        FailureKind::Network => Some("Network error"),
        FailureKind::Server => Some("Server error"),
        FailureKind::Other => None,
    }
}

/// The tooltip's first line: what happened, as a sentence.
fn long(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::FolderUnreachable => "This engine can't reach the download folder",
        FailureKind::FolderUnusable => "The download folder can't be made or written to, or can't hold this file",
        FailureKind::DiskFull => "The disk the download folder is on is full",
        FailureKind::Blocked => "Security software or a policy blocked it",
        FailureKind::Network => "The connection failed or timed out",
        FailureKind::Server => "The server refused it or sent an error",
        FailureKind::Other => "The download stopped before it finished",
    }
}

/// A failed download card's face.
pub(crate) fn card_face(kind: FailureKind) -> &'static str {
    reason(kind).unwrap_or("Download failed")
}

/// The Downloads popup's status row for a failed download.
pub(crate) fn popup_row(kind: FailureKind) -> String {
    match reason(kind) {
        Some(reason) => format!("Failed: {reason}"),
        None => "Failed".to_owned(),
    }
}

/// The toast for a download that failed at `path`.
pub(crate) fn toast(path: &str, kind: FailureKind) -> String {
    match reason(kind) {
        Some(reason) => format!("Download failed: {path} ({reason})"),
        None => format!("Download failed: {path}"),
    }
}

/// The hover text: what happened, the folder the download was meant for, and the engine's or OS's own words, a line
/// each; a blank detail or no folder adds no line.
fn tooltip(failure: &DownloadFailure, folder: Option<&Path>) -> String {
    let folder = folder.map(|f| format!("Folder: {}", f.display()));
    let detail = Some(failure.detail.trim()).filter(|d| !d.is_empty()).map(str::to_owned);
    std::iter::once(long(failure.kind).to_owned()).chain(folder).chain(detail).collect::<Vec<_>>().join("\n")
}

/// The folder a download that failed at `path` was meant for: the path's own folder when it is a full path, else
/// `app_folder`, the app's download folder (a refusal for want of a folder reports the bare file name).
fn folder_of(path: &str, app_folder: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    let path = Path::new(path);
    match path.parent() {
        Some(parent) if path.is_absolute() => Some(parent.to_path_buf()),
        _ => app_folder(),
    }
}

impl App {
    /// Download card `id`'s hover text when it failed (`tooltip`), else `None`. A screen reader hears it after the
    /// card's name (`card::card_label`).
    pub(crate) fn download_tooltip(&self, id: NodeId) -> Option<String> {
        let download = self.nodes.get(&id)?.as_download()?;
        let failure = download.failure()?;
        let folder = folder_of(&download.path, || self.download_folder());
        Some(tooltip(failure, folder.as_deref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{field, FieldValue};
    use manifold_view_protocol::ViewToHost;

    /// The face draws unclipped at a fixed 13 points (`download_body.rs`, `push_centered_text`) on every download card
    /// tall enough for a header (`card::tab_has_header`): `DOWNLOAD_NODE_SIZE` is 160 by 110 and the header needs 80
    /// points of height, so down to about 116 points of width. Laid out in the app's own font, each short reason fits
    /// inside the card's margins there (a character count would not bound its width); each kind reads differently,
    /// and only `Other` falls back to "Download failed".
    #[test]
    fn every_face_fits_the_smallest_card_and_only_other_is_plain() {
        let faces: std::collections::HashSet<&str> = FailureKind::ALL.iter().map(|&k| card_face(k)).collect();
        assert_eq!(faces.len(), FailureKind::ALL.len(), "two kinds read the same");
        for kind in FailureKind::ALL {
            assert_eq!(card_face(kind) == "Download failed", kind == FailureKind::Other, "{kind:?}");
        }
        // The narrowest card that draws a face is between 116 and 117 points wide on screen.
        let card = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::card::DOWNLOAD_NODE_SIZE);
        let drawn_at = |width: f32| crate::card::tab_has_header(card, card.width() / width);
        assert!(drawn_at(117.0) && !drawn_at(116.0), "the narrowest card with a face moved: update `room`");
        let room = 116.0 - 2.0 * crate::card::GRAPH_THUMB_MARGIN;
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut widths: Vec<(FailureKind, f32)> = Vec::new();
        // `set_fonts` takes effect at the start of the next pass, so this one lays out in the app's font.
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::FontId::proportional(13.0);
            widths = FailureKind::ALL
                .iter()
                .map(|&kind| {
                    let face = card_face(kind).to_owned();
                    (kind, ui.ctx().fonts_mut(|f| f.layout_no_wrap(face, font.clone(), egui::Color32::WHITE)).size().x)
                })
                .collect();
        });
        // The font atlas upload is never applied here; dropping it unapplied is a debug assertion.
        output.textures_delta.clear();
        assert_eq!(widths.len(), FailureKind::ALL.len());
        for (kind, width) in widths {
            assert!(width <= room, "{kind:?}: {:?} is {width} points wide; the card has {room}", card_face(kind));
        }
    }

    #[test]
    fn the_popup_and_the_toast_name_the_short_reason() {
        assert_eq!(popup_row(FailureKind::DiskFull), "Failed: Disk full");
        assert_eq!(popup_row(FailureKind::Other), "Failed");
        assert_eq!(toast(r"D:\dl\a.zip", FailureKind::Network), r"Download failed: D:\dl\a.zip (Network error)");
        assert_eq!(toast("a.zip", FailureKind::Other), "Download failed: a.zip");
    }

    /// The tooltip says what happened, where, and the engine's or OS's words, a line each; a blank detail or no folder
    /// adds no line.
    #[test]
    fn the_tooltip_says_why_where_and_the_engines_words() {
        let folder = std::path::Path::new("dl");
        let unusable = DownloadFailure::new(FailureKind::FolderUnusable, " Access is denied. (os error 5)\n");
        assert_eq!(
            tooltip(&unusable, Some(folder)),
            "The download folder can't be made or written to, or can't hold this file\nFolder: dl\n\
             Access is denied. (os error 5)"
        );
        let network = DownloadFailure::new(FailureKind::Network, " ");
        assert_eq!(tooltip(&network, None), "The connection failed or timed out");
    }

    /// Review Focus 1, 3 and 5. A refusal for want of a folder (a bare file name) names the folder the app means now;
    /// a failure at a full path names that path's folder, even after the setting moves (P12); a renamed card still
    /// says why, after the name a screen reader hears first.
    #[test]
    fn a_failed_cards_tooltip_names_its_folder_and_survives_a_rename() {
        let mut app = App::default();
        let tab = app.alloc_node_id();
        app.insert_note_at(tab, None);
        let root = crate::store::testutil::temp_dir("download-tooltip");
        let (a, b) = (root.join("a"), root.join("b"));
        app.set_download_directory(a.to_str().unwrap());
        let full = a.join("full.zip").to_string_lossy().into_owned();
        for path in ["bare.zip", full.as_str(), "running.zip"] {
            app.insert_download_node(tab, path.to_owned(), None, None);
        }
        let unreachable = DownloadFailure::new(FailureKind::FolderUnreachable, "");
        app.handle_view_message(tab, ViewToHost::DownloadFailed { path: "bare.zip".into(), failure: unreachable });
        let unusable = DownloadFailure::new(FailureKind::FolderUnusable, "Access is denied. (os error 5)");
        app.handle_view_message(tab, ViewToHost::DownloadFailed { path: full.clone(), failure: unusable });
        let id_of = |app: &App, path: &str| {
            let found =
                app.node_order.iter().copied().find(|id| app.nodes[id].as_download().is_some_and(|d| d.path == path));
            found.unwrap()
        };
        let (bare, at_full, running) = (id_of(&app, "bare.zip"), id_of(&app, &full), id_of(&app, "running.zip"));
        let names_folder = |tip: Option<String>, dir: &std::path::Path| {
            tip.is_some_and(|t| t.lines().any(|l| l == format!("Folder: {}", dir.display())))
        };
        assert!(names_folder(app.download_tooltip(bare), &a), "a bare name: the app's folder");
        assert!(names_folder(app.download_tooltip(at_full), &a));
        app.set_download_directory(b.to_str().unwrap());
        assert!(names_folder(app.download_tooltip(bare), &b), "a bare name follows the setting");
        assert!(names_folder(app.download_tooltip(at_full), &a), "a full path keeps its own folder");
        assert_eq!(app.download_tooltip(running), None, "in flight: no tooltip");
        assert_eq!(app.download_tooltip(tab), None, "not a download");
        app.doc.mirror.set(at_full, field::NAME, FieldValue::Str("Mine".into()));
        let label =
            crate::card::card_label(app.accessible_name(at_full).unwrap(), app.download_tooltip(at_full).as_deref());
        assert!(label.starts_with("Download: Mine. The download folder can't be made or written to"), "{label}");
        assert!(label.ends_with("Access is denied. (os error 5)"), "{label}");
    }
}
