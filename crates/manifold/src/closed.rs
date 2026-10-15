//! Recently Closed: close, reopen and eviction over the document's `closed` field; and Duplicate's snapshot (moves
//! to `duplicate.rs` in Task 4).

use super::*;
use crate::store::model::{Kind, Stamp};

/// A snapshot of a node and, if it's a group, every live member at any depth, as
/// `subtree_entry` builds it. `doc.nodes[0]` is the first node copied.
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

/// How far a duplicate lands from its original, in the level's world units (`App::duplicate_nodes`).
const DUPLICATE_OFFSET: f32 = 40.0;

/// How many roots the history keeps before `evict_closed` deletes the oldest, like a browser's own bounded
/// "Recently closed" list.
pub(crate) const MAX_CLOSED: usize = 20;

/// Whether a node of stored type `ty` (`Mirror::ty`) with `fields` (its snapshot's) is worth keeping when it is
/// closed. `false` for a completed download (it has no closed state -- starting it over isn't "reopening"), a
/// blank note (nothing to restore; the `Blank` tag) and no type at all; everything else -- tabs, groups, images,
/// PDFs, JSON files, jars and a type this build doesn't know (a shell, spec decision 4) -- is. The capability rows
/// in `caps/rows.rs` decide, by the row the type reads as (`card_kind::canonical_type`).
pub(crate) fn worth_reopening(ty: &str, fields: &session_sync::Fields) -> bool {
    use crate::caps::{Tag, TagSet};
    let ty = crate::card_kind::canonical_type(ty);
    let text = fields.get(&field::TEXT);
    let blank = !matches!(text, Some(store::model::FieldValue::Str(text)) if !text.trim().is_empty());
    let tags = if ty == store::model::TYPE_NOTE && blank { TagSet::of(&[Tag::Blank]) } else { TagSet::EMPTY };
    crate::caps::registry().caps(ty, tags).restorable
}

/// Every node under `root` at any depth, in `order`, each group visited
/// once: a group cycle (never built, but the loop must end) can't hang it.
pub(crate) fn members_under(
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
    // Never recorded: a Duplicate or reopen of the Default is an ordinary jar (only one jar is the Default).
    fields.remove(&field::IS_DEFAULT);
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

/// The snapshot of the live subtrees under `roots` (each node and, for a group, everything under it), the shape
/// `App::duplicate_nodes` copies: each root first, written without a group, and the
/// members after, in draw order. `keep` (a node's type and snapshot fields) drops the nodes it refuses, a root
/// among them; `offset` moves each root's pos, so a copy lands beside what it copies. A link comes along when both
/// its ends do. `None` when nothing is kept.
pub(crate) fn subtree_entry(
    app: &App,
    roots: &[&str],
    offset: egui::Vec2,
    keep: impl Fn(&str, &session_sync::Fields) -> bool,
) -> Option<ClosedEntry> {
    let order: Vec<store::model::NodeId> = app.node_order.iter().filter_map(|s| s.parse().ok()).collect();
    let (mut nodes, mut zoom, mut title) = (Vec::new(), serde_json::Map::new(), serde_json::Map::new());
    let mut record = |label: &str| {
        if let Some(z) = tab_zoom(app, label) {
            zoom.insert(label.to_string(), serde_json::json!(z));
        }
        if let Some(t) = tab_title(app, label) {
            title.insert(label.to_string(), serde_json::json!(t));
        }
    };
    for &id in roots {
        let Ok(root_id) = id.parse::<store::model::NodeId>() else { continue };
        let Some(root) = app.snapshot(root_id) else { continue };
        let mut root_fields = live_fields(app, id, &root.fields);
        if !keep(app.mirror.ty(id), &root_fields) {
            continue;
        }
        if let Some(store::model::FieldValue::Pos(x, y)) = root_fields.get_mut(&field::POS) {
            (*x, *y) = (*x + offset.x, *y + offset.y);
        }
        nodes.push(store::json::node_json(id, &root.ty, &root_fields, None));
        record(id);
        // Only a group has members; for anything else this finds none. Membership and containment are the mirror's.
        let members = members_under(
            root_id,
            &order,
            |id| app.mirror.group(&id.to_string()).and_then(|g| g.parse().ok()),
            |id| app.is_container(&id.to_string()),
        );
        for member_id in members {
            let Some(member) = app.snapshot(member_id) else { continue };
            let label = member_id.to_string();
            // The Default is not closed with its group (it moves out), so a reopen must not bring a copy.
            if app.is_default_jar(&label) {
                continue;
            }
            let fields = live_fields(app, &label, &member.fields);
            if keep(app.mirror.ty(&label), &fields) {
                nodes.push(store::json::node_json(&label, &member.ty, &fields, member.group));
                record(&label);
            }
        }
    }
    if nodes.is_empty() {
        return None;
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
                nodes.push(store::json::node_json(&label, &snap.ty, &snap.fields, None));
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

impl App {
    /// Whether closing `id` keeps it (spec decision 4): a node the document can hold whose kind and value are worth
    /// reopening (`worth_reopening`). Without a journal nothing is kept: the close is a plain destroy.
    fn restorable(&self, id: &str) -> bool {
        let Some(node) = id.parse::<store::model::NodeId>().ok() else { return false };
        self.journal.is_some() && self.snapshot(node).is_some_and(|s| worth_reopening(&s.ty, &s.fields))
    }

    /// Closes one node (spec decision 4), the single user-facing close: a card's hover ✕, its context-menu
    /// Delete/Close/"Remove from List", the Downloads popup's Remove, Ctrl+W, Close Selected and the rest. Typed text
    /// and the drawn position are written first; a node not worth reopening is destroyed as before; otherwise the
    /// members not worth reopening go (a blank note, a download), `closed` is queued on the root, and the subtree is
    /// hidden in place with its engines unloaded. Saves, then evicts past `MAX_CLOSED`.
    pub(crate) fn close_node(&mut self, id: &str) {
        if self.is_default_jar(id) {
            self.queue_toast("The Default jar can't be closed".to_string());
            return;
        }
        self.commit_note_buffers();
        // A node created this tick has no snapshot yet and its `closed` flag would be dropped as not editable (the
        // hide below would then lose it): flush the pending creates first.
        if !self.writes.pending_creates.is_empty() {
            self.save_session();
        }
        if !self.restorable(id) {
            self.destroy_node(id);
            // A closed jar's tabs now resolve to the Default jar.
            self.sweep_jar_switches();
            self.save_session();
            return;
        }
        let root: store::model::NodeId = id.parse().expect("restorable ids parse");
        // The subtree as this replica holds it: the mirror's membership over `node_order`, so a tab mid-respawn and a
        // member not saved yet are found too.
        let order: Vec<store::model::NodeId> = self.node_order.iter().filter_map(|s| s.parse().ok()).collect();
        let members = members_under(
            root,
            &order,
            |m| self.mirror.group(&m.to_string()).and_then(|g| g.parse().ok()),
            |m| self.is_container(&m.to_string()),
        );
        let subtree: Vec<String> = std::iter::once(root).chain(members).map(|n| n.to_string()).collect();
        // Where each card was drawn becomes its document pos (2b-4 Ruling 10: a reopen lands where it was shown).
        for key in &subtree {
            let carries_pos = Kind::from_type(self.mirror.ty(key)).fields().contains(&field::POS);
            if carries_pos && self.layout.contains_key(key) {
                self.edit_pos(key, Some(self.layout_pos(key)));
            }
        }
        let (kept, gone): (Vec<String>, Vec<String>) =
            subtree.into_iter().partition(|key| key == id || self.restorable(key));
        for key in &gone {
            self.remove_node(key);
        }
        self.writes.closed_flags.push((root, true));
        let tabs: Vec<String> = kept
            .iter()
            .filter(|k| self.nodes.get(*k).is_some_and(|n| n.runtime.saved_tab().is_some()))
            .cloned()
            .collect();
        self.begin_tabs_shutdown(&tabs);
        self.leave_hidden(root);
        for key in &kept {
            self.hide_node(key);
        }
        self.sweep_jar_switches();
        self.save_session();
        self.evict_closed();
    }

    /// Reopen (spec decision 7): clears `closed` on `id` and re-stamps each closed root directly under it, oldest
    /// first, so they enter the history newest in their old order. The save's visibility pass shows what is no
    /// longer hidden (`apply_visibility`), and `apply_remote` sweeps the jar switches. A node that stays hidden
    /// (its group is closed) or lands at another level says so (plan D8).
    pub(crate) fn reopen_node(&mut self, id: store::model::NodeId) {
        let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
        // A closed node `d` under `id` with no other closed node between them.
        let directly_under = |d: store::model::NodeId| {
            let mut cur = doc.nodes.get(&d).and_then(|n| n.group);
            for _ in 0..=doc.nodes.len() {
                let Some(c) = cur else { return false };
                if c == id {
                    return true;
                }
                let Some(n) = doc.nodes.get(&c) else { return false };
                if !n.deleted && n.closed() {
                    return false;
                }
                cur = n.group;
            }
            false
        };
        let mut nested: Vec<(Stamp, store::model::NodeId)> = doc
            .with_descendants(std::collections::BTreeSet::from([id]))
            .into_iter()
            .filter(|&d| d != id && directly_under(d))
            .filter_map(|d| {
                let n = doc.nodes.get(&d).filter(|n| n.is_live() && n.closed())?;
                Some((n.fields.get(&field::CLOSED)?.stamp, d))
            })
            .collect();
        nested.sort();
        self.writes.closed_flags.push((id, false));
        self.writes.closed_flags.extend(nested.into_iter().map(|(_, d)| (d, true)));
        self.save_session();
        let key = id.to_string();
        let reopened =
            self.journal.as_ref().is_some_and(|j| j.doc().nodes.get(&id).is_some_and(|n| n.is_live() && !n.closed()));
        if !reopened {
            return;
        }
        if !self.nodes.contains_key(&key) && !self.spawns.spawning.contains_key(&key) {
            self.queue_toast("Reopened inside a closed group: it shows when that group is reopened".to_string());
        } else if self.mirror.group(&key) != self.current_group.as_deref() {
            let level = match self.mirror.group(&key) {
                None => "the top level".to_string(),
                Some(g) => {
                    let shown = crate::names::display_name(self.mirror.name(g), crate::names::UNTITLED);
                    let shown = crate::names::to_plain(&shown);
                    format!("“{shown}”")
                }
            };
            self.queue_toast(format!("Reopened in {level}"));
        }
    }

    /// Eviction (spec decision 6): every root past `MAX_CLOSED` has its whole subtree Deleted, nested closed nodes
    /// included, so nothing under it resurfaces. Idempotent across replicas; the only place `closed` becomes
    /// `deleted`. After a local close, after remote ops (`apply_remote`) and at load.
    pub(crate) fn evict_closed(&mut self) {
        let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
        let roots = doc.closed_roots();
        if roots.len() <= MAX_CLOSED {
            return;
        }
        let evicted: Vec<String> = doc
            .with_descendants(roots[MAX_CLOSED..].iter().map(|&(id, _)| id).collect())
            .into_iter()
            .filter(|id| doc.nodes.get(id).is_some_and(|n| !n.deleted))
            .map(|id| id.to_string())
            .collect();
        for id in &evicted {
            self.queue_delete(id);
        }
        // Files are released only after the Delete committed (decision 11: `release_backing` reads `backing_keep`).
        if self.save_session() {
            for id in &evicted {
                self.forget_deleted(id);
            }
        }
    }

    /// Duplicate: a copy of each of `ids` beside it. A tab is a fresh page at its saved url and engine (`duplicate_tab`),
    /// sharing no session state. Any other node, with everything under it for a group, is copied through the same
    /// snapshot Recently Closed uses (`subtree_entry`, `reopen_closed`): fresh ids, content and name kept, links
    /// between the copied cards remapped, tabs inside a group arriving asleep. A node whose group is also in `ids`
    /// is copied with that group, not again.
    pub(crate) fn duplicate_nodes(&mut self, ids: &[String]) {
        let covered = |app: &App, id: &str| {
            let mut group = app.mirror.group(id);
            for _ in 0..app.nodes.len() {
                let Some(g) = group else { return false };
                if ids.iter().any(|i| i == g) {
                    return true;
                }
                group = app.mirror.group(g);
            }
            false
        };
        let ids: Vec<&String> = ids.iter().filter(|id| self.nodes.contains_key(*id) && !covered(self, id)).collect();
        let (tabs, roots): (Vec<&String>, Vec<&String>) =
            ids.into_iter().partition(|id| self.nodes[*id].runtime.saved_tab().is_some());
        let roots: Vec<&str> = roots.into_iter().map(String::as_str).collect();
        let entry = subtree_entry(self, &roots, egui::Vec2::splat(DUPLICATE_OFFSET), |_, _| true);
        for tab in tabs.into_iter().cloned().collect::<Vec<_>>() {
            self.duplicate_tab(&tab);
        }
        if let Some(entry) = entry {
            self.reopen_closed(entry);
        }
    }

    /// Duplicate on `id` as the menu and Ctrl+D mean it: the whole selection when `id` is in it, else `id` alone.
    pub(crate) fn duplicate_selection(&mut self, id: &str) {
        let ids: Vec<String> = if self.interaction.selected.contains(id) {
            let mut all: Vec<String> = self.interaction.selected.iter().cloned().collect();
            all.sort();
            all
        } else {
            vec![id.to_owned()]
        };
        self.duplicate_nodes(&ids);
    }

    /// After an import with fresh ids: each jar whose old id is no longer a live node gets a copy of its cookie file
    /// under its new id, so a reopened jar comes back signed in (spec "One reopen rule"). One whose old id is still
    /// live (a Duplicate) gets nothing: the copy starts signed out. A copy, not a move: a named session or another
    /// instance's Recently Closed list may still name the old id, and the startup prune removes the old file once
    /// nothing holds it. `imported[i]` is `doc.nodes[i]` (`store::json::import`).
    pub(crate) fn rebind_jar_files(&self, doc: &store::json::JsonDoc, imported: &[store::json::Imported]) {
        let Some(dir) = self.jars_dir() else { return };
        for (old, new) in doc.nodes.iter().zip(imported) {
            if new.kind() != Kind::Jar || self.nodes.contains_key(&old.id) {
                continue;
            }
            let (from, to) = (dir.join(format!("{}.json", old.id)), crate::jar::Jar::path(&dir, new.id));
            if from.exists() && !to.exists() {
                if let Err(e) = std::fs::copy(&from, &to) {
                    eprintln!("jar: could not carry jar {} over to {}: {e}", old.id, new.id);
                }
            }
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
        self.rebind_jar_files(&entry.doc, &imported);
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
    use crate::session_sync::tests::{add, add_group, app_on, stored};
    use crate::store::model::{field, FieldValue, Kind, NodeId};

    fn note(app: &mut App, text: &str) -> String {
        add(app, Kind::Note, Runtime::Note(NotePayload { text: text.into(), editing: false }))
    }

    fn closed_ids(app: &App) -> Vec<String> {
        app.journal.as_ref().unwrap().doc().closed_roots().into_iter().map(|(n, _)| n.to_string()).collect()
    }

    #[test]
    fn close_flags_the_root_hides_the_subtree_and_reopen_brings_the_same_ids_back() {
        let path = store::testutil::temp_dir("closed-same-ids").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let inner = note(&mut app, "kept");
        app.edit_group(&inner, Some(g.clone()));
        let outside = note(&mut app, "outside");
        let _link = app.create_link(&inner, &outside);
        assert!(app.save_session());
        let link: Vec<NodeId> = app.mirror.link_ids().collect();
        app.close_node(&g);
        assert_eq!(stored(&app, &g, field::CLOSED), FieldValue::Bool(true));
        assert_eq!(stored(&app, &inner, field::CLOSED), FieldValue::Bool(false), "only the root is flagged");
        assert!(!app.nodes.contains_key(&g) && !app.nodes.contains_key(&inner));
        assert_eq!(closed_ids(&app), std::slice::from_ref(&g));
        app.reopen_node(g.parse().unwrap());
        assert!(app.nodes.contains_key(&g) && app.nodes.contains_key(&inner), "same ids");
        assert_eq!(app.mirror.group(&inner), Some(g.as_str()));
        assert_eq!(app.mirror.link_ids().collect::<Vec<_>>(), link, "the link to the outside node survived");
        assert!(closed_ids(&app).is_empty());
    }

    #[test]
    fn a_close_flushes_typed_text_and_the_drawn_position() {
        let path = store::testutil::temp_dir("closed-flush").join("session.mnf");
        let mut app = app_on(&path, 1);
        let n = note(&mut app, "v1");
        assert!(app.save_session());
        app.nodes.get_mut(&n).unwrap().as_note_mut().unwrap().text = "typed".into();
        app.snap_layout(&n, egui::vec2(120.0, 80.0));
        app.close_node(&n);
        assert_eq!(stored(&app, &n, field::TEXT), FieldValue::Str("typed".into()));
        assert_eq!(stored(&app, &n, field::POS), FieldValue::Pos(120.0, 80.0));
        app.reopen_node(n.parse().unwrap());
        assert_eq!(app.note_text(&n), "typed");
        assert_eq!(app.layout_pos(&n), egui::vec2(120.0, 80.0));
    }

    /// A tab whose page moved on without sharing it (a redirect, a script, a restore at its `LiveUrl`) reopens at the
    /// page that was closed, not at the shared url: its personal state is kept in `hidden_local`.
    #[test]
    fn a_closed_tab_reopens_at_its_live_url() {
        use crate::session_sync::tests::sleeping_tab;
        let path = store::testutil::temp_dir("closed-live-url").join("session.mnf");
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://shared/", "T"));
        assert!(app.save_session());
        if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
            t.url = "https://live/".into();
        }
        assert_eq!(app.mirror.url(&tab), "https://shared/", "the shared url is unchanged");
        app.close_node(&tab);
        assert_eq!(app.hidden_local[&tab].live_url.as_ref().map(|l| l.url.as_str()), Some("https://live/"));
        app.reopen_node(tab.parse().unwrap());
        let reopened = app.nodes[&tab].as_unloaded().unwrap();
        assert_eq!((reopened.url.as_str(), reopened.title.as_str()), ("https://live/", "T"));
    }

    /// Duplicate copies a renamed note beside the original with its name and text, and a group with its member (not
    /// again when the member is selected too); the originals stay.
    #[test]
    fn duplicating_a_note_and_a_group_copies_them_beside_the_originals() {
        let path = store::testutil::temp_dir("duplicate-nodes").join("session.mnf");
        let mut app = app_on(&path, 1);
        let text = |t: &str| Runtime::Note(crate::node::NotePayload { text: t.into(), editing: false });
        let note = add(&mut app, Kind::Note, text("n"));
        app.edit_pos(&note, Some(egui::vec2(10.0, 20.0)));
        app.snap_layout(&note, egui::vec2(10.0, 20.0));
        app.edit_name(&note, "Mine");
        let group = add_group(&mut app, "G");
        let inner = add(&mut app, Kind::Note, text("inner"));
        app.edit_group(&inner, Some(group.clone()));
        assert!(app.save_session());
        let before = app.nodes.len();
        app.duplicate_nodes(&[note.clone(), group.clone(), inner.clone()]);
        assert_eq!(app.nodes.len(), before + 3, "note, group and its one member, the member not twice");
        let copies: Vec<&String> = app.nodes.keys().filter(|id| ![&note, &group, &inner].contains(id)).collect();
        assert!(copies.iter().any(|id| app.mirror.name(id) == "Mine" && app.mirror.text(id) == "n"));
        let copy = copies.iter().find(|id| app.mirror.name(id) == "Mine").unwrap();
        assert_eq!(app.mirror.pos(copy), Some(egui::vec2(10.0 + DUPLICATE_OFFSET, 20.0 + DUPLICATE_OFFSET)));
        assert!(app.nodes.contains_key(&note) && app.nodes.contains_key(&inner));
    }

    /// A type this build doesn't know comes back as a shell of its type (spec decision 4), whatever its case; no type
    /// at all (`Mirror::ty` of a node the mirror lacks) does not; a blank note is not, whatever the case of its type.
    #[test]
    fn an_unknown_type_is_worth_reopening_as_a_shell() {
        assert!(worth_reopening("application/x-future", &Default::default()));
        assert!(worth_reopening("application/vnd.manifold.json-leaf", &Default::default()), "a retired type");
        assert!(!worth_reopening("", &Default::default()));
        assert!(!worth_reopening("Text/Markdown", &Default::default()), "a blank note, its type in another case");
        assert!(!worth_reopening(store::model::TYPE_DOWNLOAD, &Default::default()));
    }

    /// Decided by the node's type string (`Mirror::ty`) and, for a note, whether its text is blank.
    #[test]
    fn worth_reopening_is_what_it_was_for_every_kind() {
        let text = |s: &str| session_sync::Fields::from([(field::TEXT, FieldValue::Str(s.into()))]);
        let none = session_sync::Fields::new();
        for (kind, fields, want) in [
            (Kind::Tab, &none, true),
            (Kind::Group, &none, true),
            (Kind::Image, &none, true),
            (Kind::Pdf, &none, true),
            (Kind::Json, &none, true),
            (Kind::Download, &none, false),
            (Kind::Jar, &none, true),
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
        use store::model::ActorId;
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

    #[test]
    fn closing_a_group_keeps_its_jar_and_a_tab_follows_it_back_on_reopen() {
        use crate::session_sync::tests::{add_jar, sleeping_tab};
        let path = store::testutil::temp_dir("closed-group-jar").join("session.mnf");
        let mut app = app_on(&path, 1);
        app.ensure_default_jar();
        let group = add_group(&mut app, "G");
        let at = crate::fetched::Placement { group: Some(group.clone()), pos: egui::vec2(40.0, 40.0) };
        let inside = app.create_jar("In", at);
        let outside = add_jar(&mut app, "Out");
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let other = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        for t in [&tab, &other] {
            app.edit_group(t, Some(group.clone()));
        }
        app.set_jar_edge(&tab, Some(inside));
        app.set_jar_edge(&other, Some(outside));
        assert!(app.save_session());
        app.close_node(&group);
        app.reopen_node(group.parse().unwrap());
        assert!(app.jar_defs().contains_key(&inside), "the reopened jar has the same id");
        assert_eq!(app.mirror.jar(&tab), Some(inside), "follows the reopened jar");
        assert_eq!(app.mirror.jar(&other), Some(outside), "an outside jar keeps its id");
    }

    #[test]
    fn closing_and_reopening_a_jar_keeps_its_id_and_its_cookie_file() {
        let path = store::testutil::temp_dir("closed-jar-cookies").join("session.mnf");
        let jars = store::testutil::temp_dir("rebind-jars");
        let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 1) };
        let jar = crate::session_sync::tests::add_jar(&mut app, "Work");
        assert!(app.save_session());
        let file = jars.join(format!("{jar}.json"));
        std::fs::write(&file, b"cookies").unwrap();
        app.close_node(&jar.to_string());
        assert!(!app.jar_defs().contains_key(&jar), "hidden while closed");
        assert!(file.exists(), "the cookie file was never copied or touched");
        app.reopen_node(jar);
        assert!(app.jar_defs().contains_key(&jar), "the same id back");
        assert_eq!(std::fs::read(&file).unwrap(), b"cookies");
    }

    #[test]
    fn a_blank_note_in_a_closing_group_is_deleted_and_a_blank_root_is_not_recorded() {
        let path = store::testutil::temp_dir("closed-blank").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let blank = note(&mut app, "  ");
        app.edit_group(&blank, Some(g.clone()));
        let lone = note(&mut app, "");
        assert!(app.save_session());
        app.close_node(&g);
        let doc = app.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&blank.parse().unwrap()].deleted, "dropped as the snapshot dropped it");
        app.close_node(&lone);
        assert!(app.journal.as_ref().unwrap().doc().nodes[&lone.parse().unwrap()].deleted);
        assert_eq!(closed_ids(&app), [g]);
    }

    #[test]
    fn an_in_progress_download_in_a_closing_group_is_removed() {
        let path = store::testutil::temp_dir("closed-download").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let tab = add(&mut app, Kind::Tab, crate::session_sync::tests::sleeping_tab("https://x.test/", "T"));
        app.edit_group(&tab, Some(g.clone()));
        assert!(app.save_session());
        // An in-progress download takes its tab's group and has no document node (`restorable` is false).
        app.insert_download_node(tab.clone(), "f.bin".into(), Some(3), Some("https://x.test/f.bin".into()));
        let download = app.nodes.iter().find(|(_, n)| n.as_download().is_some()).map(|(id, _)| id.clone()).unwrap();
        assert_eq!(app.mirror.group(&download), Some(g.as_str()));
        app.close_node(&g);
        assert!(!app.nodes.contains_key(&download), "the download went with the closing group");
        app.reopen_node(g.parse().unwrap());
        assert!(!app.nodes.contains_key(&download), "reopening the group doesn't bring it back");
        assert!(app.nodes.contains_key(&tab), "the tab came back");
    }

    #[test]
    fn closing_past_twenty_evicts_the_oldest_whole_subtree() {
        let path = store::testutil::temp_dir("closed-evict").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "Oldest");
        let nested = note(&mut app, "nested");
        app.edit_group(&nested, Some(g.clone()));
        let rest: Vec<String> = (0..MAX_CLOSED).map(|i| note(&mut app, &format!("n{i}"))).collect();
        assert!(app.save_session());
        app.close_node(&nested); // a closed node nested in the oldest root: evicted with it
        app.close_node(&g);
        for n in &rest {
            app.close_node(n);
        }
        let doc = app.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&g.parse().unwrap()].deleted && doc.nodes[&nested.parse().unwrap()].deleted);
        assert_eq!(closed_ids(&app).len(), MAX_CLOSED);
    }

    #[test]
    fn reopening_a_group_re_stamps_its_nested_roots_newest_in_old_order() {
        let path = store::testutil::temp_dir("closed-restamp").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let (a, b) = (note(&mut app, "a"), note(&mut app, "b"));
        app.edit_group(&a, Some(g.clone()));
        app.edit_group(&b, Some(g.clone()));
        let other = note(&mut app, "other");
        assert!(app.save_session());
        app.close_node(&a);
        app.close_node(&b);
        app.close_node(&g);
        app.close_node(&other);
        app.reopen_node(g.parse().unwrap());
        assert_eq!(closed_ids(&app), [b, a, other], "the nested ones are newest, b after a as before");
    }

    #[test]
    fn ctrl_shift_t_skips_another_replicas_close() {
        let path = store::testutil::temp_dir("closed-mine").join("session.mnf");
        let mut app = app_on(&path, 1);
        let (mine, theirs) = (note(&mut app, "mine"), note(&mut app, "theirs"));
        assert!(app.save_session());
        app.close_node(&mine);
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, &theirs, field::CLOSED, FieldValue::Bool(true));
        assert!(app.save_session());
        assert_eq!(closed_ids(&app)[0], theirs, "theirs is newest");
        app.reopen_closed_shortcut();
        assert!(app.nodes.contains_key(&mine) && !app.nodes.contains_key(&theirs));
    }

    #[test]
    fn a_replica_that_only_receives_closes_evicts_and_agrees() {
        let path = store::testutil::temp_dir("closed-remote-evict").join("session.mnf");
        let mut a = app_on(&path, 1);
        let notes: Vec<String> = (0..=MAX_CLOSED).map(|i| note(&mut a, &format!("n{i}"))).collect();
        assert!(a.save_session());
        // Another replica closes all MAX_CLOSED + 1 notes; A closes none, it only receives them.
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        for n in &notes {
            crate::session_sync::tests::remote_set(&mut other, n, field::CLOSED, FieldValue::Bool(true));
        }
        assert!(a.save_session());
        let doc = a.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&notes[0].parse().unwrap()].deleted, "A evicted the oldest on receipt");
        assert_eq!(closed_ids(&a).len(), MAX_CLOSED);
        // A replica restored from the same file agrees on the same set and order.
        let mut b = app_on(&path, 3);
        b.restore_session();
        assert_eq!(closed_ids(&a), closed_ids(&b), "one order on every replica");
    }

    #[test]
    fn a_reopen_concurrent_with_its_eviction_loses() {
        let path = store::testutil::temp_dir("closed-reopen-vs-evict").join("session.mnf");
        let mut a = app_on(&path, 1);
        let notes: Vec<String> = (0..=MAX_CLOSED).map(|i| note(&mut a, &format!("n{i}"))).collect();
        assert!(a.save_session());
        for n in &notes[..MAX_CLOSED] {
            a.close_node(n);
        }
        // B opens now, so it still sees notes[0] closed and has not seen A's 21st close.
        let mut b = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        a.close_node(&notes[MAX_CLOSED]); // A's 21st close evicts notes[0] ...
        crate::session_sync::tests::remote_set(&mut b, &notes[0], field::CLOSED, FieldValue::Bool(false)); // ... B's stale reopen.
        assert!(a.save_session());
        assert!(a.journal.as_ref().unwrap().doc().nodes[&notes[0].parse().unwrap()].deleted, "Delete wins");
        assert!(!a.nodes.contains_key(&notes[0]));
    }

    #[test]
    fn a_tab_on_a_closed_jar_uses_the_default_and_returns() {
        let path = store::testutil::temp_dir("closed-jar-fallback").join("session.mnf");
        let mut app = app_on(&path, 1);
        app.ensure_default_jar();
        let default = app.default_jar().unwrap();
        let work = app.create_jar("Work", crate::fetched::Placement { group: None, pos: egui::Vec2::ZERO });
        let tab = add(&mut app, Kind::Tab, crate::session_sync::tests::sleeping_tab("https://a.com/", "A"));
        app.set_jar_edge(&tab, Some(work));
        assert!(app.save_session());
        assert_eq!(app.effective_jar(&tab), Some((work, false)));
        app.close_node(&work.to_string());
        assert_eq!(app.effective_jar(&tab), Some((default, true)), "the closed jar dangles to the Default");
        app.reopen_node(work);
        assert_eq!(app.effective_jar(&tab), Some((work, false)), "reopened: Work again; the stored edge never changed");
    }

    #[test]
    fn a_default_arriving_inside_a_closed_group_is_moved_out_not_shadowed() {
        let path = store::testutil::temp_dir("closed-default").join("session.mnf");
        let mut app = app_on(&path, 1);
        app.ensure_default_jar();
        let g = add_group(&mut app, "G");
        assert!(app.save_session());
        let default = app.default_jar().unwrap();
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, &g, field::CLOSED, FieldValue::Bool(true));
        let op = store::ops::OpBody::Move { node: default, group: Some(g.parse().unwrap()) };
        other.begin().unwrap().commit(vec![op]).unwrap();
        assert!(app.save_session());
        assert_eq!(app.default_jar(), Some(default), "the same Default, shown again");
        let doc = app.journal.as_ref().unwrap().doc();
        assert_eq!(doc.nodes[&default].group, None);
        let defaults = doc.live().filter(|(_, n)| n.get(field::IS_DEFAULT) == FieldValue::Bool(true)).count();
        assert_eq!(defaults, 1, "no second Default was made");
    }
}
