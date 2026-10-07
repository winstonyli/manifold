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

/// Whether a node of type `ty` (`Mirror::ty`) with `fields` (its snapshot's) is
/// worth keeping in a `ClosedEntry`. `false` for a completed download (it
/// has no closed state -- starting it over isn't "reopening"), a jar, a
/// blank note (nothing to restore; the `Blank` tag) and a type this build
/// doesn't know; everything else -- tabs, groups, images, PDFs, JSON files
/// and leaves -- is. The capability rows in `caps/rows.rs` decide.
pub(crate) fn worth_reopening(ty: &str, fields: &session_sync::Fields) -> bool {
    use crate::caps::{Tag, TagSet};
    let text = fields.get(&field::TEXT);
    let blank = !matches!(text, Some(store::model::FieldValue::Str(text)) if !text.trim().is_empty());
    let tags = if ty == store::model::TYPE_NOTE && blank { TagSet::of(&[Tag::Blank]) } else { TagSet::EMPTY };
    crate::caps::registry().caps(ty, tags).restorable
}

/// Every node under `root` at any depth, in `order`, each group visited
/// once: a group cycle (never built, but the loop must end) can't hang it.
fn members_under(
    root: store::model::NodeId,
    order: &[store::model::NodeId],
    group_of: impl Fn(&store::model::NodeId) -> Option<store::model::NodeId>,
    is_group: impl Fn(&store::model::NodeId) -> bool,
) -> Vec<store::model::NodeId> {
    let mut members: HashMap<store::model::NodeId, Vec<store::model::NodeId>> = HashMap::new();
    for id in order {
        if let Some(group) = group_of(id) {
            members.entry(group).or_default().push(*id);
        }
    }
    let (mut out, mut seen, mut levels) = (Vec::new(), HashSet::from([root]), vec![root]);
    while let Some(level) = levels.pop() {
        for id in members.get(&level).into_iter().flatten() {
            if is_group(id) && seen.insert(*id) {
                levels.push(*id);
            }
            if *id != root {
                out.push(*id);
            }
        }
    }
    out
}

/// `id`'s live tab zoom, when it differs from the default -- what
/// `closed_entry` records in `doc.local["zoom"]`. Zoom isn't a store
/// field (it's personal state), so it isn't in the node's snapshot.
pub(crate) fn tab_zoom(app: &App, id: &str) -> Option<f64> {
    let zoom = match app.nodes.get(id).map(|n| &n.runtime) {
        Some(Runtime::Tab(tab)) => tab.zoom_factor,
        Some(Runtime::Unloaded(tab)) => tab.zoom_factor,
        _ => return None,
    };
    (zoom != app.settings.zoom_factor).then_some(zoom)
}

/// `id`'s tab title, when it has one -- what `closed_entry` records in
/// `doc.local["title"]`. Like zoom, a title is personal state.
pub(crate) fn tab_title(app: &App, id: &str) -> Option<String> {
    let title = match app.nodes.get(id).map(|n| &n.runtime) {
        Some(Runtime::Tab(tab)) => &tab.title,
        Some(Runtime::Unloaded(tab)) => &tab.title,
        _ => return None,
    };
    (!title.is_empty()).then(|| title.clone())
}

/// `fields` (`id`'s snapshot's, `App::snapshot`) as the user saw the node: a tab's `url` replaced by its live one, a note's
/// `text` by what its card shows (`App::note_text`: a box being typed in holds text no save has committed), and `pos`
/// by where its card was drawn (`App::layout`), and a file card's `path` by the one resolved on this machine (the
/// payload's). Recently Closed reopens the page that was closed, not the shared url the mirror holds (2b-3 Ruling 1),
/// where it was shown, not at its document pos (2b-4 Ruling 10), from the file this machine read (2b-5 Ruling 5).
fn live_fields(app: &App, id: &str, fields: &session_sync::Fields) -> session_sync::Fields {
    let mut fields = fields.clone();
    if let Some(url) = app.nodes.get(id).and_then(|n| n.runtime.saved_tab()) {
        fields.insert(field::URL, store::model::FieldValue::Str(url.to_owned()));
    }
    if fields.contains_key(&field::TEXT) {
        fields.insert(field::TEXT, store::model::FieldValue::Str(app.note_text(id).to_owned()));
    }
    if let Some(path) = app.nodes.get(id).and_then(|n| n.runtime.file_path()) {
        if fields.contains_key(&field::PATH) {
            fields.insert(field::PATH, store::model::FieldValue::Str(path.to_owned()));
        }
    }
    if app.layout.contains_key(id) && fields.contains_key(&field::POS) {
        fields.insert(field::POS, session_sync::pos_value(app.layout_pos(id)));
    }
    fields
}

/// The `ClosedEntry` closing `id` should record: `id` and, for a group,
/// every node under it at any depth (in `app.node_order`, i.e. draw
/// order), minus what `worth_reopening` skips. `None` when `id` itself
/// isn't worth reopening, which includes a lone "Expand as Graph" leaf:
/// it's one of many, and each would push a real entry out of `MAX_CLOSED`.
pub(crate) fn closed_entry(app: &App, id: &str) -> Option<ClosedEntry> {
    let root_id: store::model::NodeId = id.parse().ok()?;
    let root = app.snapshot(root_id)?;
    let root_fields = live_fields(app, id, &root.fields);
    if root.kind == store::model::Kind::JsonLeaf || !worth_reopening(app.mirror.ty(id), &root_fields) {
        return None;
    }
    let mut nodes = vec![store::json::node_json(id, root.kind, &root_fields, None)];
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
    // Only a group has members; for anything else this finds none. Membership and containment are the mirror's.
    let order: Vec<store::model::NodeId> = app.node_order.iter().filter_map(|s| s.parse().ok()).collect();
    let members = members_under(
        root_id,
        &order,
        |id| app.mirror.group(&id.to_string()).and_then(|g| g.parse().ok()),
        |id| crate::caps::edges().target_accepts(crate::caps::CONTAINS, app.mirror.ty(&id.to_string())),
    );
    for member_id in members {
        let Some(member) = app.snapshot(member_id) else { continue };
        let label = member_id.to_string();
        let fields = live_fields(app, &label, &member.fields);
        if worth_reopening(app.mirror.ty(&label), &fields) {
            nodes.push(store::json::node_json(&label, member.kind, &fields, member.group));
            record(&label);
        }
    }
    // A link comes back only between cards that do (both ends recorded above); import remaps its ends to the new ids.
    let kept: std::collections::BTreeSet<store::model::NodeId> =
        nodes.iter().filter_map(|n| n.id.parse().ok()).collect();
    let mut links: Vec<_> = app.mirror.link_ids().collect();
    links.sort();
    for link in links {
        let label = link.to_string();
        let (from, to) = app.mirror.link_ends(&label);
        if from.is_some_and(|n| kept.contains(&n)) && to.is_some_and(|n| kept.contains(&n)) {
            if let Some(snap) = app.snapshot(link) {
                nodes.push(store::json::node_json(&label, snap.kind, &snap.fields, None));
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
    /// remapped (import resolves labels within the file), and a node's
    /// `parent` ("opened from", any kind that has one) outside the entry
    /// kept only if that node still exists.
    /// The root's group becomes `self.current_group` (it's written as
    /// `None` in the doc -- see `closed_entry`); every other node keeps
    /// the group `closed_entry` recorded if it's still part of this entry,
    /// and otherwise joins the root (`materialize_imported`). Nothing
    /// gains focus, and tabs come back asleep, starting when opened (spec
    /// "Lazy tabs"). An image or JSON file that's gone since is toasted and
    /// skipped; a PDF reopens as its error card. Ends with `save_session`.
    pub(crate) fn reopen_closed(&mut self, entry: ClosedEntry) {
        self.leave_json_level();
        let next_id = || self.alloc_node_id().parse().expect("a store id");
        let imported = match store::json::import(&entry.doc, next_id, store::json::Trust::Own) {
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
            if let Some(Runtime::Unloaded(tab)) = self.nodes.get_mut(&id).map(|n| &mut n.runtime) {
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
    use crate::store::model::Kind;

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
        let put_in = |app: &mut App, id: &str, group: &str| app.edit_group(id, Some(group.into()));

        let outer = crate::session_sync::tests::add_group(&mut app, "Outer");
        let note0 = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "kept".into(), editing: false }));
        put_in(&mut app, &note0, &outer);
        let note1 = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: " ".into(), editing: false }));
        put_in(&mut app, &note1, &outer);
        let inner = crate::session_sync::tests::add_group(&mut app, "Inner");
        put_in(&mut app, &inner, &outer);
        let img0 = add(
            &mut app,
            Kind::Image,
            Runtime::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None }),
        );
        put_in(&mut app, &img0, &inner);
        let json0 = add(
            &mut app,
            Kind::JsonLeaf,
            Runtime::Json(JsonPayload {
                path: "d.json".into(),
                value: std::sync::Arc::new(1.into()),
                leaf: true,
                error: None,
            }),
        );
        put_in(&mut app, &json0, &inner);
        let dl0 = add(
            &mut app,
            Kind::Download,
            Runtime::Download(DownloadPayload {
                path: "f.bin".into(),
                bytes_received: 1,
                total_bytes: Some(1),
                completed: true,
                failed: false,
            }),
        );
        put_in(&mut app, &dl0, &inner);
        let note2 = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "outside".into(), editing: false }));

        let entry = closed_entry(&app, &outer).unwrap();
        let ids: Vec<&str> = entry.doc.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            [outer.as_str(), note0.as_str(), inner.as_str(), img0.as_str(), json0.as_str()],
            "a blank note and a completed download are both skipped"
        );
        assert_eq!(entry.doc.nodes[0].kind(), Some(Kind::Group));
        assert!(entry.doc.nodes[0].fields.get("group").is_none(), "the root's group isn't recorded");
        assert_eq!(entry.doc.nodes[3].fields.get("group").and_then(|v| v.as_str()), Some(inner.as_str()));

        let leaf_outside = add(
            &mut app,
            Kind::JsonLeaf,
            Runtime::Json(JsonPayload {
                path: "d.json".into(),
                value: std::sync::Arc::new(1.into()),
                leaf: true,
                error: None,
            }),
        );
        assert!(closed_entry(&app, &leaf_outside).is_none(), "a JSON leaf outside a group gives None");
        assert!(closed_entry(&app, &note1).is_none(), "a blank note gives None");
        assert_eq!(closed_entry(&app, &note2).unwrap().doc.nodes.len(), 1);
    }

    /// A tab whose page moved on without sharing it (a redirect, a script, a restore at its `LiveUrl`) reopens from
    /// Recently Closed at the page that was closed, not at the shared url.
    #[test]
    fn a_closed_tab_reopens_at_its_live_url() {
        use crate::session_sync::tests::{add, app_on, sleeping_tab};
        let path = store::testutil::temp_dir("closed-live-url").join("session.mnf");
        let mut app = app_on(&path, 1);
        let group = crate::session_sync::tests::add_group(&mut app, "G");
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://shared/", "T"));
        let member = add(&mut app, Kind::Tab, sleeping_tab("https://m-shared/", "M"));
        app.edit_group(&member, Some(group.clone()));
        assert!(app.save_session());
        for (id, live) in [(&tab, "https://live/"), (&member, "https://m-live/")] {
            if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(id).unwrap().runtime {
                t.url = live.into();
            }
        }
        assert_eq!(app.mirror.url(&tab), "https://shared/", "the shared url is unchanged");
        let url_of = |entry: &ClosedEntry, i: usize| entry.doc.nodes[i].fields.get("url").cloned();
        let entry = closed_entry(&app, &tab).unwrap();
        assert_eq!(url_of(&entry, 0), Some(serde_json::json!("https://live/")));
        let entry = closed_entry(&app, &group).unwrap();
        assert_eq!(url_of(&entry, 1), Some(serde_json::json!("https://m-live/")), "a group member's too");

        app.close_node(&tab);
        let entry = app.closed.pop().unwrap();
        app.reopen_closed(entry);
        let reopened = app.nodes.values().filter_map(|n| n.as_unloaded()).find(|t| t.title == "T").unwrap();
        assert_eq!(reopened.url, "https://live/");
    }

    /// Recently Closed records where the card was shown (its layout position), not its document pos, and reopens it
    /// there (2b-4 Ruling 10).
    #[test]
    fn a_closed_node_reopens_where_it_was_shown() {
        use crate::session_sync::tests::app_on;
        let path = store::testutil::temp_dir("closed-layout-pos").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = Runtime::Note(crate::node::NotePayload { text: "n".into(), editing: false });
        let at = crate::fetched::Placement { group: None, pos: egui::vec2(10.0, 20.0) };
        let note = app.place_node(None, Kind::Note, note, at);
        assert!(app.save_session());
        app.snap_layout(&note, egui::vec2(70.0, 80.0));
        let entry = closed_entry(&app, &note).unwrap();
        assert_eq!(entry.doc.nodes[0].fields.get("pos"), Some(&serde_json::json!([70.0, 80.0])));

        app.close_node(&note);
        let entry = app.closed.pop().unwrap();
        app.reopen_closed(entry);
        let reopened = app.nodes.keys().find(|id| **id != note).unwrap().clone();
        assert_eq!(app.layout_pos(&reopened), egui::vec2(70.0, 80.0));
        assert_eq!(app.mirror.pos(&reopened), Some(egui::vec2(70.0, 80.0)));
    }

    /// A note closed while it is typed in records what the box held, which no save has committed yet.
    #[test]
    fn a_closed_note_records_what_was_typed() {
        let mut app = App::default();
        let note = Runtime::Note(crate::node::NotePayload { text: "n".into(), editing: false });
        let at = crate::fetched::Placement { group: None, pos: egui::vec2(10.0, 20.0) };
        let note = app.place_node(None, Kind::Note, note, at);
        let n = app.nodes.get_mut(&note).unwrap().as_note_mut().unwrap();
        (n.editing, n.text) = (true, "typed".into());
        let entry = closed_entry(&app, &note).unwrap();
        assert_eq!(entry.doc.nodes[0].fields.get("text"), Some(&serde_json::json!("typed")));
    }

    /// A type this build doesn't know, and no type at all (`Mirror::ty` of a node the mirror lacks), are not.
    #[test]
    fn an_unknown_type_is_not_worth_reopening() {
        assert!(!worth_reopening("application/x-future", &Default::default()));
        assert!(!worth_reopening("", &Default::default()));
    }

    /// Decided by the node's type string (`Mirror::ty`) and, for a note, whether its text is blank.
    #[test]
    fn worth_reopening_is_what_it_was_for_every_kind() {
        use store::model::{FieldValue, Kind};
        let text = |s: &str| session_sync::Fields::from([(field::TEXT, FieldValue::Str(s.into()))]);
        let none = session_sync::Fields::new();
        for (kind, fields, want) in [
            (Kind::Tab, &none, true),
            (Kind::Group, &none, true),
            (Kind::Image, &none, true),
            (Kind::Pdf, &none, true),
            (Kind::Json, &none, true),
            (Kind::JsonLeaf, &none, true),
            (Kind::Download, &none, false),
            (Kind::Jar, &none, false),
            (Kind::Link, &none, false),
            (Kind::Note, &text("hi"), true),
            (Kind::Note, &text("  \n"), false),
            (Kind::Note, &none, false),
        ] {
            assert_eq!(worth_reopening(kind.type_str().unwrap(), fields), want, "{kind:?}");
        }
        assert!(worth_reopening("image/gif", &none), "any image, not only the stored `image/png`");
    }

    #[test]
    fn a_group_cycle_ends_the_walk() {
        use store::model::{ActorId, NodeId};
        let id = |n| NodeId { actor: ActorId(1), counter: n };
        let (a, b, c) = (id(1), id(2), id(3));
        // a contains b, b contains a (a cycle), and c is in b.
        let group_of = |n: &NodeId| match n.counter {
            1 => Some(b),
            2 => Some(a),
            3 => Some(b),
            _ => None,
        };
        let out = members_under(a, &[a, b, c], group_of, |n| n.counter < 3);
        assert_eq!(out, [b, c], "b and c, once each");
    }
}
