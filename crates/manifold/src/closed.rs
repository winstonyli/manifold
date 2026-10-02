//! Recently Closed: what closing a node records and how reopening rebuilds it.

use super::*;

/// closed node and, if it's a group, every live member at any depth, as
/// `closed_entry` built it. `doc.nodes[0]` is the node that was closed.
/// `App::reopen_closed` imports the whole snapshot with fresh ids. Not
/// cookies/localStorage/scroll position, the same "graph shape only"
/// limit session restore and an engine swap have. `doc.local` carries a
/// tab's personal state, which isn't in the store: its zoom when it
/// differs from the default settings zoom, and its title when it has one,
/// as `{"zoom": {"<label>": 1.25}, "title": {"<label>": "…"}}`. Both are
/// set on that tab's sleeping card when it's reopened, and the title
/// names it in the Recently Closed list.
pub(crate) struct ClosedEntry {
    pub(crate) doc: store::json::JsonDoc,
}

impl ClosedEntry {
    /// `doc.local[key]`'s value for the node recorded as `label`.
    pub(crate) fn local(&self, key: &str, label: &str) -> Option<&serde_json::Value> {
        self.doc.local.as_ref()?.get(key)?.get(label)
    }
}

/// How many entries `App::closed` keeps before dropping the oldest, like a
/// browser's own bounded "Recently closed" list.
pub(crate) const MAX_CLOSED: usize = 20;

/// Whether `kind`/`fields` (as `App::shared_nodes` sees a live node) is
/// worth keeping in a `ClosedEntry`. `false` for a completed download (it
/// has no closed state -- starting it over isn't "reopening") and a blank
/// note (nothing to restore); everything else -- tabs, groups, images,
/// PDFs, JSON files and leaves -- is.
pub(crate) fn worth_reopening(kind: store::model::Kind, fields: &session_sync::Fields) -> bool {
    match kind {
        store::model::Kind::Download => false,
        store::model::Kind::Note => match fields.get(&field::TEXT) {
            Some(store::model::FieldValue::Str(text)) => !text.trim().is_empty(),
            _ => false,
        },
        store::model::Kind::Tab
        | store::model::Kind::Group
        | store::model::Kind::Image
        | store::model::Kind::Pdf
        | store::model::Kind::Json
        | store::model::Kind::JsonLeaf
        | store::model::Kind::Unknown(_) => true,
    }
}

/// `id`'s live tab zoom, when it differs from the default -- what
/// `closed_entry` records in `doc.local["zoom"]`. Zoom isn't a store
/// field (it's personal state), so it doesn't come from `shared_nodes`.
pub(crate) fn tab_zoom(app: &App, id: &str) -> Option<f64> {
    let zoom = match app.nodes.get(id).map(|n| &n.payload) {
        Some(NodePayload::Tab(tab)) => tab.zoom_factor,
        Some(NodePayload::Unloaded(tab)) => tab.zoom_factor,
        _ => return None,
    };
    (zoom != app.settings.zoom_factor).then_some(zoom)
}

/// `id`'s tab title, when it has one -- what `closed_entry` records in
/// `doc.local["title"]`. Like zoom, a title is personal state.
pub(crate) fn tab_title(app: &App, id: &str) -> Option<String> {
    let title = match app.nodes.get(id).map(|n| &n.payload) {
        Some(NodePayload::Tab(tab)) => &tab.title,
        Some(NodePayload::Unloaded(tab)) => &tab.title,
        _ => return None,
    };
    (!title.is_empty()).then(|| title.clone())
}

/// The `ClosedEntry` closing `id` should record: `id` and, for a group,
/// every node under it at any depth (in `app.node_order`, i.e. draw
/// order), minus what `worth_reopening` skips. `None` when `id` itself
/// isn't worth reopening, which includes a lone "Expand as Graph" leaf:
/// it's one of many, and each would push a real entry out of `MAX_CLOSED`.
pub(crate) fn closed_entry(app: &App, id: &str) -> Option<ClosedEntry> {
    let shared = app.shared_nodes();
    let root_id: store::model::NodeId = id.parse().ok()?;
    let root = shared.get(&root_id)?;
    if root.kind == store::model::Kind::JsonLeaf || !worth_reopening(root.kind, &root.fields) {
        return None;
    }
    let mut nodes = vec![store::json::node_json(id, root.kind, &root.fields, None)];
    let (mut zoom, mut title) = (serde_json::Map::new(), serde_json::Map::new());
    let mut record = |label: &str| {
        if let Some(z) = tab_zoom(app, label) {
            zoom.insert(label.to_string(), serde_json::json!(z));
        }
        if let Some(t) = tab_title(app, label) {
            title.insert(label.to_string(), serde_json::json!(t));
        }
    };
    record(id);
    // Only a group has members; for anything else this finds none.
    let mut levels = vec![root_id];
    while let Some(level) = levels.pop() {
        for member_id in &app.node_order {
            let Ok(member_id) = member_id.parse::<store::model::NodeId>() else { continue };
            let Some(member) = shared.get(&member_id) else { continue };
            if member.group != Some(level) {
                continue;
            }
            if member.kind == store::model::Kind::Group {
                levels.push(member_id);
            }
            if worth_reopening(member.kind, &member.fields) {
                let label = member_id.to_string();
                nodes.push(store::json::node_json(&label, member.kind, &member.fields, member.group));
                record(&label);
            }
        }
    }
    let mut local = serde_json::Map::new();
    for (key, map) in [("zoom", zoom), ("title", title)] {
        if !map.is_empty() {
            local.insert(key.to_string(), serde_json::Value::Object(map));
        }
    }
    let local = (!local.is_empty()).then_some(serde_json::Value::Object(local));
    Some(ClosedEntry { doc: store::json::JsonDoc { nodes, local } })
}

/// Marks each content-dir file `entry`'s nodes own as modified now, so a
/// recent close keeps its file through `content::PRUNE_GRACE` even if
/// another instance's Recently Closed list overwrites this one's.
pub(crate) fn touch_owned_content(dir: &std::path::Path, entry: &ClosedEntry) {
    for node in &entry.doc.nodes {
        let Some(path) = node.fields.get("path").and_then(|p| p.as_str()).map(std::path::Path::new) else { continue };
        if content::owned_content_file(dir, &node.id, path) {
            let file = std::fs::OpenOptions::new().write(true).open(path);
            let _ = file.and_then(|f| f.set_modified(std::time::SystemTime::now()));
        }
    }
}

impl App {
    /// Records what closing `id` loses (see `closed_entry`), dropping the
    /// oldest entry past `MAX_CLOSED`. Called before anything is removed.
    pub(crate) fn record_closed(&mut self, id: &str) {
        if let Some(entry) = closed_entry(self, id) {
            if let Some(dir) = profile::content_dir() {
                touch_owned_content(&dir, &entry);
            }
            self.closed.push(entry);
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
            self.closed_changed_at = Some(std::time::Instant::now());
        }
    }

    /// Rebuilds a closed entry where it was: fresh ids via `store::json::
    /// import`, the old positions/colors/zooms, links inside the entry
    /// remapped (import resolves labels within the file), and a tab's
    /// `parent` outside the entry kept only if that node still exists.
    /// The root's group becomes `self.current_group` (it's written as
    /// `None` in the doc -- see `closed_entry`); every other node keeps
    /// the group `closed_entry` recorded if it's still part of this entry,
    /// and otherwise joins the root (`materialize_imported`). Nothing
    /// gains focus, and tabs come back asleep, starting when opened (spec
    /// "Lazy tabs"). An image or JSON file that's gone since is toasted and
    /// skipped; a PDF reopens as its error card. Ends with `save_session`.
    pub(crate) fn reopen_closed(&mut self, entry: ClosedEntry) {
        self.leave_json_level();
        let imported = match store::json::import(&entry.doc, || self.alloc_node_id().parse().expect("a store id")) {
            Ok(imported) => imported,
            Err(err) => {
                self.queue_toast(format!("Couldn't reopen: {err}"));
                return;
            }
        };
        if imported.is_empty() {
            return;
        }
        // Each tab's zoom and title, recorded in `doc.local` under its old
        // label, carried over to its fresh id (`imported[i]` is
        // `entry.doc.nodes[i]`, per `import`'s contract) and set on its
        // sleeping card once it's in place.
        let personal: Vec<(String, Option<f64>, Option<String>)> = entry
            .doc
            .nodes
            .iter()
            .zip(&imported)
            .map(|(n, i)| {
                let zoom = entry.local("zoom", &n.id).and_then(|v| v.as_f64());
                let title = entry.local("title", &n.id).and_then(|v| v.as_str()).map(str::to_string);
                (i.id.to_string(), zoom, title)
            })
            .collect();

        let root = session_sync::ImportRoot { group: self.insertion_group(), pos: None };
        self.materialize_imported(imported, Some(root), true);
        for (id, zoom, title) in personal {
            if let Some(NodePayload::Unloaded(tab)) = self.nodes.get_mut(&id).map(|n| &mut n.payload) {
                if let Some(zoom) = zoom {
                    tab.zoom_factor = zoom;
                }
                if let Some(title) = title {
                    tab.title = title;
                }
            }
        }
        self.save_session();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closing a group records it and every live member at any depth as
    /// one store-shaped entry, root first: `entry.doc.nodes[0]` is the
    /// group itself, and `worth_reopening` drops a blank note and a
    /// completed download along the way (a download has nothing sensible
    /// to reopen, the same as before this test's rewrite for Task 8).
    /// These are today's selection rules -- which ids, in what order, and
    /// which roots give `None` -- kept exactly as the pre-store-snapshot
    /// version of this test pinned them.
    #[test]
    fn closed_entry_snapshots_the_whole_subtree() {
        use crate::session_sync::tests::{add, app_on};
        let path = store::testutil::temp_dir("closed-entry").join("session.mnf");
        let mut app = app_on(&path, 1);
        let put_in = |app: &mut App, id: &str, group: &str| app.nodes.get_mut(id).unwrap().group = Some(group.into());

        let outer = add(&mut app, NodePayload::Group(GroupPayload { name: "Outer".into(), from_json: false }));
        let note0 = add(&mut app, NodePayload::Note(NotePayload { text: "kept".into(), editing: false, url: None }));
        put_in(&mut app, &note0, &outer);
        let note1 = add(&mut app, NodePayload::Note(NotePayload { text: " ".into(), editing: false, url: None }));
        put_in(&mut app, &note1, &outer);
        let inner = add(&mut app, NodePayload::Group(GroupPayload { name: "Inner".into(), from_json: false }));
        put_in(&mut app, &inner, &outer);
        let img0 = add(
            &mut app,
            NodePayload::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None, url: None }),
        );
        put_in(&mut app, &img0, &inner);
        let json0 = add(
            &mut app,
            NodePayload::Json(JsonPayload {
                path: "d.json".into(),
                value: 1.into(),
                key: Some("k".into()),
                error: None,
                url: None,
                view: json_view::JsonView::default(),
            }),
        );
        put_in(&mut app, &json0, &inner);
        let dl0 = add(
            &mut app,
            NodePayload::Download(DownloadPayload {
                path: "f.bin".into(),
                url: None,
                bytes_received: 1,
                total_bytes: Some(1),
                completed: true,
                failed: false,
            }),
        );
        put_in(&mut app, &dl0, &inner);
        let note2 = add(&mut app, NodePayload::Note(NotePayload { text: "outside".into(), editing: false, url: None }));

        let entry = closed_entry(&app, &outer).unwrap();
        let ids: Vec<&str> = entry.doc.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            [outer.as_str(), note0.as_str(), inner.as_str(), img0.as_str(), json0.as_str()],
            "a blank note and a completed download are both skipped"
        );
        assert_eq!(entry.doc.nodes[0].kind, "group");
        assert!(entry.doc.nodes[0].fields.get("group").is_none(), "the root's group isn't recorded");
        assert_eq!(entry.doc.nodes[3].fields.get("group").and_then(|v| v.as_str()), Some(inner.as_str()));

        let leaf_outside = add(
            &mut app,
            NodePayload::Json(JsonPayload {
                path: "d.json".into(),
                value: 1.into(),
                key: Some("k".into()),
                error: None,
                url: None,
                view: json_view::JsonView::default(),
            }),
        );
        assert!(closed_entry(&app, &leaf_outside).is_none(), "a JSON leaf outside a group gives None");
        assert!(closed_entry(&app, &note1).is_none(), "a blank note gives None");
        assert_eq!(closed_entry(&app, &note2).unwrap().doc.nodes.len(), 1);
    }
}
