//! Duplicate: a copy of shown nodes beside them, with fresh ids (spec 2026-10-14-closed-field-design.md decision 13).
//! A pure export of the subtree (`subtree_json`) pasted back through `store::json::import` (`App::paste_copy`),
//! independent of Recently Closed.

use super::*;
use crate::closed::members_under;
use crate::store::model::NodeId;

/// How far a duplicate lands from its original, in the level's world units (`App::duplicate_nodes`).
const DUPLICATE_OFFSET: f32 = 40.0;

/// `id`'s live tab zoom, when it differs from the default -- what `subtree_json` records in `doc.local["zoom"]`. Zoom
/// isn't a store field (it's personal state), so it isn't in the node's snapshot.
pub(crate) fn tab_zoom(app: &App, id: NodeId) -> Option<f64> {
    let zoom = match app.nodes.get(&id).map(|n| &n.runtime) {
        Some(Runtime::Tab(tab)) => tab.zoom_factor,
        Some(Runtime::Unloaded(tab)) => tab.zoom_factor,
        _ => return None,
    };
    (zoom != app.settings.zoom_factor).then_some(zoom)
}

/// `id`'s tab title, when it has one -- what `subtree_json` records in `doc.local["title"]`. Like zoom, a title is
/// personal state.
pub(crate) fn tab_title(app: &App, id: NodeId) -> Option<String> {
    let title = match app.nodes.get(&id).map(|n| &n.runtime) {
        Some(Runtime::Tab(tab)) => &tab.title,
        Some(Runtime::Unloaded(tab)) => &tab.title,
        _ => return None,
    };
    (!title.is_empty()).then(|| title.clone())
}

/// `fields` (`id`'s snapshot's, `App::snapshot`) as the user saw the node: a tab's `url` replaced by its live one, a note's
/// `text` by what its card shows (`App::note_text`: a box being typed in holds text no save has committed), and `pos`
/// by where its card was drawn (`App::layout`), and a file card's `path` by the one resolved on this machine (the
/// payload's). A copy is the page that was shown, not the shared url the mirror holds (2b-3 Ruling 1), where it was
/// shown, not at its document pos (2b-4 Ruling 10), from the file this machine read (2b-5 Ruling 5).
fn live_fields(app: &App, id: NodeId, fields: &session_sync::Fields) -> session_sync::Fields {
    let mut fields = fields.clone();
    // Never recorded: a Duplicate of the Default is an ordinary jar (only one jar is the Default).
    fields.remove(&field::IS_DEFAULT);
    if let Some(url) = app.nodes.get(&id).and_then(|n| n.runtime.saved_tab()) {
        fields.insert(field::URL, store::model::FieldValue::Str(url.to_owned()));
    }
    if fields.contains_key(&field::TEXT) {
        fields.insert(field::TEXT, store::model::FieldValue::Str(app.note_text(id).to_owned()));
    }
    if let Some(path) = app.nodes.get(&id).and_then(|n| n.runtime.file_path()) {
        if fields.contains_key(&field::PATH) {
            fields.insert(field::PATH, store::model::FieldValue::Str(path.to_owned()));
        }
    }
    if app.layout.contains_key(&id) && fields.contains_key(&field::POS) {
        fields.insert(field::POS, session_sync::pos_value(app.layout_pos(id)));
    }
    fields
}

/// The export of the shown subtrees under `roots` (each node and, for a group, everything under it), the shape
/// `App::duplicate_nodes` copies: each root first, written without a group, and the members after, in draw order.
/// `offset` moves each root's pos, so a copy lands beside what it copies. A link comes along when both its ends do.
/// `None` when nothing is exported. `doc.nodes[0]` is the first node copied. `doc.local` carries a tab's personal
/// state, which isn't in the store: its zoom when it differs from the default settings zoom, and its title when it
/// has one, as `{"zoom": {"<label>": 1.25}, "title": {"<label>": "…"}}`.
pub(crate) fn subtree_json(app: &App, roots: &[NodeId], offset: egui::Vec2) -> Option<store::json::JsonDoc> {
    let (mut nodes, mut zoom, mut title) = (Vec::new(), serde_json::Map::new(), serde_json::Map::new());
    let mut record = |id: NodeId| {
        if let Some(z) = tab_zoom(app, id) {
            zoom.insert(id.to_string(), serde_json::json!(z));
        }
        if let Some(t) = tab_title(app, id) {
            title.insert(id.to_string(), serde_json::json!(t));
        }
    };
    for &id in roots {
        let Some(root) = app.snapshot(id) else { continue };
        let mut root_fields = live_fields(app, id, &root.fields);
        if let Some(store::model::FieldValue::Pos(x, y)) = root_fields.get_mut(&field::POS) {
            (*x, *y) = (*x + offset.x, *y + offset.y);
        }
        nodes.push(store::json::node_json(&id.to_string(), &root.ty, &root_fields, &root.trees, None));
        record(id);
        // Only a group has members; for anything else this finds none. Membership and containment are the mirror's.
        let members = members_under(id, &app.node_order, |id| app.doc.mirror.group(*id), |id| app.is_container(*id));
        for member_id in members {
            let Some(member) = app.snapshot(member_id) else { continue };
            // The Default never sits in a group (spec decision 10); a copy of it would be a second Default.
            if app.is_default_jar(member_id) {
                continue;
            }
            let fields = live_fields(app, member_id, &member.fields);
            nodes.push(store::json::node_json(
                &member_id.to_string(),
                &member.ty,
                &fields,
                &member.trees,
                member.group,
            ));
            record(member_id);
        }
    }
    if nodes.is_empty() {
        return None;
    }
    // A link comes along only between cards that do (both ends exported above); import remaps its ends to the new ids.
    let kept: std::collections::BTreeSet<store::model::NodeId> =
        nodes.iter().filter_map(|n| n.id.parse().ok()).collect();
    let mut links: Vec<_> = app.doc.mirror.link_ids().collect();
    links.sort();
    for link in links {
        let (from, to) = app.doc.mirror.link_ends(link);
        if from.is_some_and(|n| kept.contains(&n)) && to.is_some_and(|n| kept.contains(&n)) {
            if let Some(snap) = app.snapshot(link) {
                nodes.push(store::json::node_json(&link.to_string(), &snap.ty, &snap.fields, &snap.trees, None));
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
    Some(store::json::JsonDoc { nodes, local })
}

impl App {
    /// Duplicate: a copy of each of `ids` beside it. A tab is a fresh page at its saved url and engine (`duplicate_tab`),
    /// sharing no session state. Any other node, with everything under it for a group, is copied through an export
    /// and paste (`subtree_json`, `paste_copy`): fresh ids, content and name kept, links between the copied cards
    /// remapped, tabs inside a group arriving asleep. A node whose group is also in `ids` is copied with that group,
    /// not again.
    pub(crate) fn duplicate_nodes(&mut self, ids: &[NodeId]) {
        let covered = |app: &App, id: NodeId| {
            let mut group = app.doc.mirror.group(id);
            for _ in 0..app.nodes.len() {
                let Some(g) = group else { return false };
                if ids.contains(&g) {
                    return true;
                }
                group = app.doc.mirror.group(g);
            }
            false
        };
        let ids: Vec<NodeId> =
            ids.iter().copied().filter(|id| self.nodes.contains_key(id) && !covered(self, *id)).collect();
        let (tabs, roots): (Vec<NodeId>, Vec<NodeId>) =
            ids.into_iter().partition(|id| self.nodes[id].runtime.saved_tab().is_some());
        let entry = subtree_json(self, &roots, egui::Vec2::splat(DUPLICATE_OFFSET));
        for tab in tabs {
            self.duplicate_tab(tab);
        }
        if let Some(doc) = entry {
            self.paste_copy(doc);
        }
    }

    /// Duplicate on `id` as the menu and Ctrl+D mean it: the whole selection when `id` is in it, else `id` alone.
    pub(crate) fn duplicate_selection(&mut self, id: NodeId) {
        let ids: Vec<NodeId> = if self.interaction.selected.contains(&CardId::from(id)) {
            let mut all: Vec<NodeId> = self.interaction.selected.iter().filter_map(|c| c.node()).collect();
            all.sort();
            all
        } else {
            vec![id]
        };
        self.duplicate_nodes(&ids);
    }

    /// Pastes a Duplicate's export where the original was: fresh ids via `store::json::import`, links inside
    /// remapped, the root at `insertion_group`, tabs asleep with the zoom and title the export recorded. Ends with
    /// `save_session`.
    pub(crate) fn paste_copy(&mut self, doc: store::json::JsonDoc) {
        self.leave_json_level();
        let next_id = || self.alloc_node_id();
        let imported = match store::json::import(&doc, next_id, store::json::Trust::Own) {
            Ok(imported) => imported,
            Err(err) => {
                self.queue_toast(format!("Couldn't duplicate: {err}"));
                return;
            }
        };
        if imported.is_empty() {
            return;
        }
        // Each tab's zoom and title, recorded in `doc.local` under its old label, carried over to its fresh id
        // (`imported[i]` is `doc.nodes[i]`, per `import`'s contract) and set on its sleeping card once it's in place.
        let local = |key: &str, label: &str| doc.local.as_ref()?.get(key)?.get(label).cloned();
        let personal: Vec<(NodeId, Option<f64>, Option<String>)> = doc
            .nodes
            .iter()
            .zip(&imported)
            .map(|(n, i)| {
                let zoom = local("zoom", &n.id).and_then(|v| v.as_f64());
                let title = local("title", &n.id).and_then(|v| v.as_str().map(str::to_string));
                (i.id, zoom, title)
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
    use crate::session_sync::tests::{add, add_group, app_on};
    use crate::store::model::{FieldValue, Kind, NodeId};

    /// Duplicate copies a renamed note beside the original with its name and text, and a group with its member (not
    /// again when the member is selected too); the originals stay.
    #[test]
    fn duplicating_a_note_and_a_group_copies_them_beside_the_originals() {
        let path = store::testutil::temp_dir("duplicate-nodes").join("session.mnf");
        let mut app = app_on(&path, 1);
        let text = |t: &str| Runtime::Note(crate::node::NotePayload { text: t.into(), editing: false });
        let note = add(&mut app, Kind::Note, text("n"));
        app.edit_pos(note, Some(egui::vec2(10.0, 20.0)));
        app.snap_layout(note, egui::vec2(10.0, 20.0));
        app.edit_name(note, "Mine");
        let group = add_group(&mut app, "G");
        let inner = add(&mut app, Kind::Note, text("inner"));
        app.edit_group(inner, Some(group));
        assert!(app.save_session());
        let before = app.nodes.len();
        app.duplicate_nodes(&[note, group, inner]);
        assert_eq!(app.nodes.len(), before + 3, "note, group and its one member, the member not twice");
        let copies: Vec<NodeId> = app.nodes.keys().copied().filter(|id| ![note, group, inner].contains(id)).collect();
        assert!(copies.iter().any(|&id| app.doc.mirror.name(id) == "Mine" && app.doc.mirror.text(id) == "n"));
        let copy = *copies.iter().find(|&&id| app.doc.mirror.name(id) == "Mine").unwrap();
        assert_eq!(app.doc.mirror.pos(copy), Some(egui::vec2(10.0 + DUPLICATE_OFFSET, 20.0 + DUPLICATE_OFFSET)));
        assert!(app.nodes.contains_key(&note) && app.nodes.contains_key(&inner));
    }

    #[test]
    fn duplicating_a_group_copies_an_unknown_member_as_a_shell_a_jar_and_inside_links_only() {
        use crate::node::NotePayload;
        use crate::session_sync::tests::{add_jar, restored_unknown, FUTURE};
        use crate::store::model::GENERIC;
        let (mut app, unknown, _path) = restored_unknown();
        let note_rt = |t: &str| Runtime::Note(NotePayload { text: t.into(), editing: false });
        let g = add_group(&mut app, "G");
        let note = add(&mut app, Kind::Note, note_rt("n"));
        let jar = add_jar(&mut app, "Work");
        let outside = add(&mut app, Kind::Note, note_rt("o"));
        for member in [note, unknown, jar] {
            app.edit_group(member, Some(g));
        }
        let _inside = app.create_link(note, unknown);
        let _across = app.create_link(note, outside);
        assert!(app.save_session());
        let before: std::collections::BTreeSet<NodeId> = app.nodes.keys().copied().collect();
        app.duplicate_nodes(std::slice::from_ref(&g));
        let fresh: Vec<NodeId> = app.nodes.keys().copied().filter(|k| !before.contains(k)).collect();
        assert_eq!(fresh.len(), 4, "a group and its three members, all with fresh ids");
        let of_type = |ty: &str| fresh.iter().copied().find(|&k| app.doc.mirror.ty(k) == ty);
        let new_group = of_type(Kind::Group.type_str().unwrap()).expect("a new group");
        let new_note = of_type(Kind::Note.type_str().unwrap()).expect("a new note");
        let new_jar = of_type(Kind::Jar.type_str().unwrap()).expect("a new jar");
        let new_unknown = of_type(FUTURE).expect("a new unknown card");
        for member in [new_note, new_jar, new_unknown] {
            assert_eq!(app.doc.mirror.group(member), Some(new_group), "members land in the new group");
        }
        // The unknown copy is a shell of the stored type: only the type, the group and the generic fields.
        let doc = app.doc.journal.as_ref().unwrap().doc();
        let shell = &doc.nodes[&new_unknown];
        assert_eq!(shell.ty(), FUTURE);
        assert!(shell
            .fields
            .keys()
            .all(|k| k.field == field::TYPE || k.field == field::GROUP || GENERIC.contains(&k.field)));
        assert_ne!(doc.nodes[&new_jar].get(field::IS_DEFAULT), FieldValue::Bool(true));
        // Links: the one inside the group is copied between the copies; the one leaving it is not.
        assert_eq!(app.links_naming(new_note).len(), 1, "only the inside link");
        assert_eq!(app.links_naming(new_unknown).len(), 1);
        assert_eq!(app.links_naming(outside).len(), 1, "the outside card gained no link");
        let link = app.links_naming(new_note).remove(0);
        let (from, to) = app.doc.mirror.link_ends(link);
        let ends = [from, to];
        assert!(ends.contains(&Some(new_note)) && ends.contains(&Some(new_unknown)));
    }
}
