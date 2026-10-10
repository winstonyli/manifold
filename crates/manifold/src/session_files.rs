//! Settings and named-session persistence, and the first-launch demo, of `App`.

use super::*;

impl App {
    /// Writes `settings.json`, unless another process (another
    /// `manifold.exe` on this profile) changed it since this one last read
    /// or wrote it: then the write goes to `settings::conflict_path()` and a
    /// toast says so, rather than clobbering theirs. The first save of a run
    /// (`settings_fingerprint` still `None`) always proceeds. Replaces every
    /// direct `self.settings.save()` call site. It writes the window
    /// geometry too, so a debounced save still waiting is done by it.
    pub(crate) fn save_settings(&mut self) {
        self.settings_save_due = None;
        let on_disk = settings::fingerprint();
        if self.settings_fingerprint.is_some() && on_disk != self.settings_fingerprint {
            if let Some(path) = settings::conflict_path() {
                self.settings.save_to(&path);
                eprintln!(
                    "settings: settings.json changed on disk since last load/save -- wrote {} instead",
                    path.display()
                );
                self.queue_toast(format!(
                    "Settings changed by another process -- your changes saved to {}",
                    path.display()
                ));
            }
            return;
        }
        self.settings.save();
        self.settings_fingerprint = settings::fingerprint();
    }

    /// Settings changed (window size or position): written once things
    /// are quiet for `SETTINGS_SAVE_DELAY`, or at exit.
    pub(crate) fn note_settings_changed(&mut self) {
        self.settings_save_due = Some(std::time::Instant::now() + SETTINGS_SAVE_DELAY);
    }

    /// Writes settings if a change is due by `now`. Returns whether it wrote.
    pub(crate) fn flush_settings_if_due(&mut self, now: std::time::Instant) -> bool {
        match self.settings_save_due {
            Some(due) if due <= now => {
                self.settings_save_due = None;
                self.save_settings();
                true
            }
            _ => false,
        }
    }

    /// At exit: writes a debounced settings change still waiting.
    pub(crate) fn flush_pending_settings(&mut self) {
        if self.settings_save_due.is_some() {
            self.save_settings();
        }
    }

    /// `session::named_session_path`, toasting why there is none: "Not a session name" for a name that can't be a
    /// file (`session_name_is_safe`; a device name such as `CON` survives `sanitize_session_name`), "No session
    /// file" without a profile.
    fn named_session_path_or_toast(&mut self, name: &str) -> Option<std::path::PathBuf> {
        let path = session::named_session_path(name);
        if path.is_none() {
            self.queue_toast(if session::session_name_is_safe(name) {
                "No session file".into()
            } else {
                let shown = crate::names::to_plain(&crate::names::display_name(name, crate::names::UNTITLED));
                format!("Not a session name: {shown}")
            });
        }
        path
    }

    /// "Save Session As..." (settings popup) / headless `SAVESESSIONAS` --
    /// a full compacted copy of the running store (spec "Named sessions"),
    /// with this profile's own personal state (`local_state_for_copy`), saved to `sessions/<name>
    /// .mnf`. Saves the running session first so the copy isn't missing
    /// recent edits; a failure there or in the copy itself is toasted, not
    /// fatal (the running session is untouched either way). `raw` is sanitised here (`sanitize_session_name`), for
    /// Settings and the headless command alike; nothing left of it saves nothing.
    pub(crate) fn save_named_session(&mut self, raw: &str) {
        let name = sanitize_session_name(raw);
        if name.is_empty() {
            return;
        }
        // The Settings page lists saves and sizes: read them again next frame, whatever happens below.
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
        let Some(path) = self.named_session_path_or_toast(&name) else { return };
        if !self.save_session() {
            self.queue_toast("The saved copy may miss recent edits: the session couldn't be saved first".into());
        }
        let local = self.local_state_for_copy();
        let Some(journal) = self.journal.as_ref() else { return };
        if let Err(err) = journal.save_copy(&path, local.as_deref()) {
            let shown = crate::names::to_plain(&crate::names::display_name(&name, crate::names::UNTITLED));
            self.queue_toast(format!("Couldn't save as {shown}: {err}"));
        }
    }

    /// The personal state Save As gives its copy: this profile's, or none while startup's is still unread
    /// (`App::local_persist`.`local_unread`). The live view then lacks what the file holds (its Recently Closed, say); a copy
    /// with none opens with defaults rather than a partial state passed off as this profile's.
    pub(crate) fn local_state_for_copy(&self) -> Option<Vec<u8>> {
        self.local_persist.local_unread.is_none().then(|| self.local_state().to_bytes())
    }

    /// "Open Session" (settings popup) / headless `OPENSESSION` -- reads a
    /// named save's store file and personal state, then replaces the
    /// running session with it via `replace_running_session` (same path
    /// Import Session uses). A read error, including one from a file
    /// written by a newer manifold (`JournalError::ReadOnly` -- `Journal::
    /// read` still reads it, just says so in its `Display`), is toasted
    /// and leaves the running session untouched.
    pub(crate) fn load_named_session(&mut self, name: &str) {
        let shown = crate::names::to_plain(&crate::names::display_name(name, crate::names::UNTITLED));
        let Some(path) = self.named_session_path_or_toast(name) else { return };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.queue_toast(format!("Couldn't open {shown}: {err}"));
                return;
            }
        };
        let doc = match store::journal::Journal::read(&bytes) {
            Ok(doc) => doc,
            Err(err) => {
                self.queue_toast(format!("Couldn't open {shown}: {err}"));
                return;
            }
        };
        let local = store::local::read(&path, doc.id)
            .ok()
            .flatten()
            .and_then(|bytes| local_state::LocalState::from_bytes(&bytes));
        let mut json = store::json::doc_to_json(&doc);
        json.local = local.map(|l| serde_json::to_value(l).expect("personal state serializes"));
        self.replace_running_session(&json, store::json::Trust::Own);
    }

    /// Whether the first-launch demo can render here: its `manifold://` pages need WebView2.
    pub(crate) fn demo_available(&self) -> bool {
        !matches!(self.engine_state(EngineKind::Webview2), engine_install::EngineState::Unavailable(_))
    }

    /// The one-time first-launch experience for a brand-new install -- see
    /// `Settings::has_shown_first_launch_demo`'s own doc comment for
    /// exactly when this fires instead of `spawn_root_tab`. A small,
    /// hand-arranged graph -- fixed positions, not `graph_layout::
    /// spawn_jitter`, unlike every other multi-node spawn in this file --
    /// since this is the one place a deliberately curated first impression
    /// matters more than avoiding a few lines of manual layout. Fully
    /// network-independent: every tab points at a `manifold://` page (see
    /// that scheme's own design doc), so it renders identically offline.
    /// `SpawnPurpose::Restore` for every tab, with `self.focused` set
    /// before any of them are issued, mirrors exactly how `apply_session`
    /// starts a real saved session -- landing in Graph view with nothing
    /// stealing focus from the intended main tab, not `Root`'s own "jump
    /// straight to Tab view" behavior, which would hide the very layout
    /// this exists to show off.
    ///
    /// Always WebView2, whatever the default engine: it is the only engine that serves `manifold://` (see
    /// `docs/superpowers/specs/2026-09-19-manifold-scheme-design.md`). Where WebView2 is unavailable the caller opens
    /// a normal root tab instead (`demo_available`).
    pub(crate) fn spawn_first_launch_demo(&mut self) {
        let kind = EngineKind::Webview2;
        let zoom = self.settings.zoom_factor;

        let welcome_id = self.alloc_node_id();
        self.focused = Some(welcome_id.clone());
        self.view_mode = ViewMode::Graph;
        let welcome = "manifold://welcome".to_string();
        self.spawn_tab_async(TabSpawn::new(welcome_id, welcome, kind, egui::Vec2::ZERO, SpawnPurpose::Restore, zoom));

        let note = Runtime::Note(NotePayload {
            text: "This is a Note -- drag it anywhere, or right-click to delete it \
                           (undo via \"Recently Closed\" if you change your mind)."
                .to_string(),
            editing: false,
        });
        self.place_node(None, store::model::Kind::Note, note, Placement { group: None, pos: egui::vec2(-300.0, 0.0) });

        let group_pos = egui::vec2(300.0, 0.0);
        let group_id = self.place_group(None, "Try grouping", Placement { group: None, pos: group_pos });
        // A small fixed offset, not `graph_layout::spawn_jitter` -- its
        // radius is calibrated to spread a new tab away from a busy live
        // layout (see its own doc comment), which is far too large for
        // "nestle just barely off-center inside a two-member group" and
        // would occasionally land a member tab embarrassingly far from the
        // group it belongs to. These positions are only ever visible if
        // this group is later entered/ungrouped, but they should still
        // look deliberate then, not scattered.
        for (url, offset) in
            [("manifold://welcome", egui::vec2(-40.0, 40.0)), ("manifold://tips", egui::vec2(40.0, 40.0))]
        {
            let id = self.alloc_node_id();
            let pos = group_pos + offset;
            let spawn = TabSpawn::new(id, url.to_string(), kind, pos, SpawnPurpose::Restore, zoom);
            self.spawn_tab_async(TabSpawn { group: Some(group_id.clone()), ..spawn });
        }

        // Deliberately left as a plain card, not entered -- the welcome
        // page's own "Try it" section already tells the user to
        // double-click this exact kind of node themselves, so entering its
        // level here would rob them of the discovery the copy is pointing
        // at. Synthetic, not file-backed (`path` is a display-only label, not a
        // real path -- see `JsonPayload`'s own doc comment on `leaf` for the
        // same "inherited path, no real ownership" reasoning leaves use,
        // though this node is a genuine standalone JSON node, not an
        // exploded leaf).
        let json = Runtime::Json(JsonPayload {
            path: "(demo).json".to_string(),
            value: std::sync::Arc::new(serde_json::json!({
                "project": "Manifold",
                "tags": ["browser", "graph-ui"],
                "version": {"major": 0, "minor": 1}
            })),
            leaf: false,
            error: None,
        });
        self.place_node(None, store::model::Kind::Json, json, Placement { group: None, pos: egui::vec2(0.0, -260.0) });
    }
}
