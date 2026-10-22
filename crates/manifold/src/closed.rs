//! Recently Closed: close, reopen and eviction over the document's `closed` field.

use super::*;
use crate::store::model::{FieldValue, Kind, NodeId, Placement, Stamp};

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

/// Every node under `root` at any depth (`order` is the membership's source), each group visited once: a group
/// cycle (never built, but the walk must end) can't hang it.
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
    crate::walk::descendants(root, |g| members.get(g).cloned().unwrap_or_default(), is_group)
}

impl App {
    /// Whether closing `id` keeps it (spec decision 4): a node the document can hold whose kind and value are worth
    /// reopening (`worth_reopening`). Without a journal nothing is kept: the close is a plain destroy.
    fn restorable(&self, id: NodeId) -> bool {
        self.doc.journal.is_some() && self.snapshot(id).is_some_and(|s| worth_reopening(&s.ty, &s.fields))
    }

    /// Closes one node (spec decision 4), the single user-facing close: a card's hover ✕, its context-menu
    /// Delete/Close/"Remove from List", the Downloads popup's Remove, Ctrl+W, Close Selected and the rest. Typed text
    /// and the drawn position are written first; a node not worth reopening is destroyed as before; otherwise the
    /// members not worth reopening go (a blank note, a download), `closed` is queued on the root, and the subtree is
    /// hidden in place with its engines unloaded. Saves, then evicts past `MAX_CLOSED`.
    pub(crate) fn close_node(&mut self, id: NodeId) {
        if self.is_default_jar(id) {
            self.queue_toast("The Default jar can't be closed".to_string());
            return;
        }
        self.commit_note_buffers();
        // A node created this tick has no snapshot yet and its `closed` flag would be dropped as not editable (the
        // hide below would then lose it): flush the pending creates first. A save that fails leaves them pending
        // (checked below, before anything is hidden).
        if self.doc.writes.has_creates() {
            self.save_session();
        }
        if !self.restorable(id) {
            self.destroy_node(id);
            // A closed jar's tabs now resolve to the Default jar.
            self.sweep_jar_switches();
            self.save_session();
            return;
        }
        // The subtree as this replica holds it: the mirror's membership over `node_order`, so a tab mid-respawn and a
        // member not saved yet are found too.
        let members = members_under(id, &self.node_order, |m| self.doc.mirror.group(*m), |m| self.is_container(*m));
        let subtree: Vec<NodeId> = std::iter::once(id).chain(members).collect();
        let (kept, gone): (Vec<NodeId>, Vec<NodeId>) =
            subtree.into_iter().partition(|&key| key == id || self.restorable(key));
        // A kept node the flush could not create has no document entry: its `closed` write would be dropped as not
        // editable and the hide would lose the node for good. Leave everything shown instead.
        let uncreated = |key: &NodeId| self.doc.writes.life(*key) == Some(crate::session_sync::Life::Create);
        if kept.iter().any(uncreated) {
            self.queue_toast("Couldn't save the session, so nothing was closed".to_string());
            return;
        }
        // Where each card was drawn becomes its document pos (2b-4 Ruling 10: a reopen lands where it was shown).
        for &key in &kept {
            let carries_pos = Kind::from_type(self.doc.mirror.ty(key)).fields().contains(&field::POS);
            if carries_pos && self.layout.contains_key(&key) {
                self.edit_pos(key, Some(self.layout_pos(key)));
            }
        }
        for &key in &gone {
            self.remove_node(key);
        }
        self.doc.writes.queue_closed(id, self.doc.replica);
        let tabs: Vec<NodeId> = kept
            .iter()
            .copied()
            .filter(|k| self.nodes.get(k).is_some_and(|n| n.runtime.saved_tab().is_some()))
            .collect();
        self.begin_tabs_shutdown(&tabs);
        self.leave_hidden(id);
        for &key in &kept {
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
    pub(crate) fn reopen_node(&mut self, id: NodeId) {
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
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
        // Each keeps the replica that closed it: a re-stamp moves it in the history, not to the reopener.
        let mut nested: Vec<(Stamp, store::model::NodeId, u64)> = doc
            .with_descendants(std::collections::BTreeSet::from([id]))
            .into_iter()
            .filter(|&d| d != id && directly_under(d))
            .filter_map(|d| {
                let n = doc.nodes.get(&d).filter(|n| n.is_live() && n.closed())?;
                Some((n.fields.get(&field::CLOSED)?.stamp, d, n.closer()))
            })
            .collect();
        nested.sort();
        self.doc.writes.queue_closed(id, 0);
        for (_, d, closer) in nested {
            self.doc.writes.queue_closed(d, closer);
        }
        self.save_session();
        self.evict_closed();
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
        if !doc.nodes.get(&id).is_some_and(|n| n.is_live() && !n.closed()) {
            return;
        }
        // Where the document shows it, not where the graph does: a tab still spawning or waking has no `nodes` entry.
        let placement = doc.placement(id);
        if matches!(placement, Placement::Shown(_)) {
            self.regroup_held(id);
        }
        match placement {
            Placement::Hidden => {
                self.queue_toast("Reopened inside a closed group: it shows when that group is reopened".to_string());
            }
            Placement::Shown(group) if group != self.current_group => {
                let level = match group {
                    None => "the top level".to_string(),
                    Some(g) => {
                        let shown = crate::names::display_name(self.doc.mirror.name(g), crate::names::UNTITLED);
                        let shown = crate::names::to_plain(&shown);
                        format!("“{shown}”")
                    }
                };
                self.queue_toast(format!("Reopened in {level}"));
            }
            Placement::Shown(_) => {}
        }
    }

    /// Eviction (spec decision 6): every root past `MAX_CLOSED` has its whole subtree Deleted, nested closed nodes
    /// included, so nothing under it resurfaces. Idempotent across replicas; the only place `closed` becomes
    /// `deleted`. After a local close, after remote ops (`apply_remote`) and at load.
    pub(crate) fn evict_closed(&mut self) {
        // The document lags this replica's own queued close and reopen writes (`PendingWrites::closed`): counting it now could
        // evict the very node being reopened. The save that commits them evicts afterwards (`apply_visibility`'s
        // `apply_remote`, or the caller's own call).
        if self.doc.writes.has_closed() {
            return;
        }
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return };
        let roots = doc.closed_roots();
        if roots.len() <= MAX_CLOSED {
            return;
        }
        // Past the cap the oldest go, but an emptied group (`App::ungroup`) before any real close: one Ungroup All of
        // many groups must not push out the tabs a user closed.
        let emptied = |id: &NodeId| {
            doc.nodes.get(id).is_some_and(|n| matches!(n.get(field::HELD), FieldValue::Str(held) if !held.is_empty()))
        };
        let excess = roots.len() - MAX_CLOSED;
        let oldest_first = || roots.iter().rev().map(|&(id, _)| id);
        let mut victims: Vec<NodeId> = oldest_first().filter(emptied).take(excess).collect();
        victims.extend(oldest_first().filter(|id| !emptied(id)).take(excess - victims.len()));
        let evicted: Vec<NodeId> = doc
            .with_descendants(victims.into_iter().collect())
            .into_iter()
            .filter(|id| doc.nodes.get(id).is_some_and(|n| !n.deleted))
            .collect();
        for &id in &evicted {
            self.queue_delete(id);
        }
        // Files are released only after the Delete committed (decision 11: `release_backing` reads `backing_keep`).
        if self.save_session() {
            for &id in &evicted {
                self.forget_deleted(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::session_sync::tests::{add, add_group, add_note, app_on, closed_ids, group_with_note, stored};

    use crate::store::model::{field, FieldValue, Kind, NodeId};

    #[test]
    fn close_flags_the_root_hides_the_subtree_and_reopen_brings_the_same_ids_back() {
        let path = store::testutil::temp_dir("closed-same-ids").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let inner = add_note(&mut app, "kept");
        app.edit_group(inner, Some(g));
        let outside = add_note(&mut app, "outside");
        let _link = app.create_link(inner, outside);
        assert!(app.save_session());
        let link: Vec<NodeId> = app.doc.mirror.link_ids().collect();
        app.close_node(g);
        assert_eq!(stored(&app, g, field::CLOSED), FieldValue::Replica(app.doc.replica));
        assert_eq!(stored(&app, inner, field::CLOSED), FieldValue::Replica(0), "only the root is flagged");
        assert!(!app.nodes.contains_key(&g) && !app.nodes.contains_key(&inner));
        assert_eq!(closed_ids(&app), std::slice::from_ref(&g));
        app.reopen_node(g);
        assert!(app.nodes.contains_key(&g) && app.nodes.contains_key(&inner), "same ids");
        assert_eq!(app.doc.mirror.group(inner), Some(g));
        assert_eq!(app.doc.mirror.link_ids().collect::<Vec<_>>(), link, "the link to the outside node survived");
        assert!(closed_ids(&app).is_empty());
    }

    #[test]
    fn a_close_flushes_typed_text_and_the_drawn_position() {
        let path = store::testutil::temp_dir("closed-flush").join("session.mnf");
        let mut app = app_on(&path, 1);
        let n = add_note(&mut app, "v1");
        assert!(app.save_session());
        app.nodes.get_mut(&n).unwrap().as_note_mut().unwrap().text = "typed".into();
        app.snap_layout(n, egui::vec2(120.0, 80.0));
        app.close_node(n);
        assert_eq!(stored(&app, n, field::TEXT), FieldValue::Str("typed".into()));
        assert_eq!(stored(&app, n, field::POS), FieldValue::Pos(120.0, 80.0));
        app.reopen_node(n);
        assert_eq!(app.note_text(n), "typed");
        assert_eq!(app.layout_pos(n), egui::vec2(120.0, 80.0));
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
        assert_eq!(app.doc.mirror.url(tab), "https://shared/", "the shared url is unchanged");
        app.close_node(tab);
        assert_eq!(app.doc.hidden_local[&tab].live_url.as_ref().map(|l| l.url.as_str()), Some("https://live/"));
        app.reopen_node(tab);
        let reopened = app.nodes[&tab].as_unloaded().unwrap();
        assert_eq!((reopened.url.as_str(), reopened.title.as_str()), ("https://live/", "T"));
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
        let at = crate::fetched::Placement { group: Some(group), pos: egui::vec2(40.0, 40.0) };
        let inside = app.create_jar("In", at);
        let outside = add_jar(&mut app, "Out");
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let other = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        for t in [tab, other] {
            app.edit_group(t, Some(group));
        }
        app.set_jar_edge(tab, Some(inside));
        app.set_jar_edge(other, Some(outside));
        assert!(app.save_session());
        app.close_node(group);
        app.reopen_node(group);
        assert!(app.jar_defs().contains_key(&inside), "the reopened jar has the same id");
        assert_eq!(app.doc.mirror.jar(tab), Some(inside), "follows the reopened jar");
        assert_eq!(app.doc.mirror.jar(other), Some(outside), "an outside jar keeps its id");
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
        app.close_node(jar);
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
        let blank = add_note(&mut app, "  ");
        app.edit_group(blank, Some(g));
        let lone = add_note(&mut app, "");
        assert!(app.save_session());
        app.close_node(g);
        let doc = app.doc.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&blank].deleted, "dropped as the snapshot dropped it");
        app.close_node(lone);
        assert!(app.doc.journal.as_ref().unwrap().doc().nodes[&lone].deleted);
        assert_eq!(closed_ids(&app), [g]);
    }

    #[test]
    fn an_in_progress_download_in_a_closing_group_is_removed() {
        let path = store::testutil::temp_dir("closed-download").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let tab = add(&mut app, Kind::Tab, crate::session_sync::tests::sleeping_tab("https://x.test/", "T"));
        app.edit_group(tab, Some(g));
        assert!(app.save_session());
        // An in-progress download takes its tab's group and has no document node (`restorable` is false).
        app.insert_download_node(tab, "f.bin".into(), Some(3), Some("https://x.test/f.bin".into()));
        let download = app.nodes.iter().find(|(_, n)| n.as_download().is_some()).map(|(id, _)| *id).unwrap();
        assert_eq!(app.doc.mirror.group(download), Some(g));
        app.close_node(g);
        assert!(!app.nodes.contains_key(&download), "the download went with the closing group");
        app.reopen_node(g);
        assert!(!app.nodes.contains_key(&download), "reopening the group doesn't bring it back");
        assert!(app.nodes.contains_key(&tab), "the tab came back");
    }

    #[test]
    fn closing_past_twenty_evicts_the_oldest_whole_subtree() {
        let path = store::testutil::temp_dir("closed-evict").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "Oldest");
        let nested = add_note(&mut app, "nested");
        app.edit_group(nested, Some(g));
        let rest: Vec<NodeId> = (0..MAX_CLOSED).map(|i| add_note(&mut app, &format!("n{i}"))).collect();
        assert!(app.save_session());
        app.close_node(nested); // a closed node nested in the oldest root: evicted with it
        app.close_node(g);
        for n in &rest {
            app.close_node(*n);
        }
        let doc = app.doc.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&g].deleted && doc.nodes[&nested].deleted);
        assert_eq!(closed_ids(&app).len(), MAX_CLOSED);
    }

    #[test]
    fn reopening_a_group_re_stamps_its_nested_roots_newest_in_old_order() {
        let path = store::testutil::temp_dir("closed-restamp").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let (a, b) = (add_note(&mut app, "a"), add_note(&mut app, "b"));
        app.edit_group(a, Some(g));
        app.edit_group(b, Some(g));
        let other = add_note(&mut app, "other");
        assert!(app.save_session());
        app.close_node(a);
        app.close_node(b);
        app.close_node(g);
        app.close_node(other);
        app.reopen_node(g);
        assert_eq!(closed_ids(&app), [b, a, other], "the nested ones are newest, b after a as before");
    }

    #[test]
    fn ctrl_shift_t_skips_another_replicas_close() {
        let path = store::testutil::temp_dir("closed-mine").join("session.mnf");
        let mut app = app_on(&path, 1);
        let (mine, theirs) = (add_note(&mut app, "mine"), add_note(&mut app, "theirs"));
        assert!(app.save_session());
        app.close_node(mine);
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, theirs, field::CLOSED, FieldValue::Replica(99));
        assert!(app.save_session());
        assert_eq!(closed_ids(&app)[0], theirs, "theirs is newest");
        app.reopen_closed_shortcut();
        assert!(app.nodes.contains_key(&mine) && !app.nodes.contains_key(&theirs));
    }

    #[test]
    fn a_replica_that_only_receives_closes_evicts_and_agrees() {
        let path = store::testutil::temp_dir("closed-remote-evict").join("session.mnf");
        let mut a = app_on(&path, 1);
        let notes: Vec<NodeId> = (0..=MAX_CLOSED).map(|i| add_note(&mut a, &format!("n{i}"))).collect();
        assert!(a.save_session());
        // Another replica closes all MAX_CLOSED + 1 notes; A closes none, it only receives them.
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        for n in &notes {
            crate::session_sync::tests::remote_set(&mut other, *n, field::CLOSED, FieldValue::Replica(99));
        }
        assert!(a.save_session());
        let doc = a.doc.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&notes[0]].deleted, "A evicted the oldest on receipt");
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
        let notes: Vec<NodeId> = (0..=MAX_CLOSED).map(|i| add_note(&mut a, &format!("n{i}"))).collect();
        assert!(a.save_session());
        for n in &notes[..MAX_CLOSED] {
            a.close_node(*n);
        }
        // B opens now, so it still sees notes[0] closed and has not seen A's 21st close.
        let mut b = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        a.close_node(notes[MAX_CLOSED]); // A's 21st close evicts notes[0] ...
        crate::session_sync::tests::remote_set(&mut b, notes[0], field::CLOSED, FieldValue::Replica(0)); // ... B's stale reopen.
        assert!(a.save_session());
        assert!(a.doc.journal.as_ref().unwrap().doc().nodes[&notes[0]].deleted, "Delete wins");
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
        app.set_jar_edge(tab, Some(work));
        assert!(app.save_session());
        assert_eq!(app.effective_jar(tab), Some((work, false)));
        app.close_node(work);
        assert_eq!(app.effective_jar(tab), Some((default, true)), "the closed jar dangles to the Default");
        app.reopen_node(work);
        assert_eq!(app.effective_jar(tab), Some((work, false)), "reopened: Work again; the stored edge never changed");
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
        crate::session_sync::tests::remote_set(&mut other, g, field::CLOSED, FieldValue::Replica(99));
        let op = store::ops::OpBody::Move { node: default, group: Some(g) };
        other.begin().unwrap().commit(vec![op]).unwrap();
        assert!(app.save_session());
        assert_eq!(app.default_jar(), Some(default), "the same Default, shown again");
        let doc = app.doc.journal.as_ref().unwrap().doc();
        assert_eq!(doc.nodes[&default].group, None);
        let defaults = doc.live().filter(|(_, n)| n.get(field::IS_DEFAULT) == FieldValue::Bool(true)).count();
        assert_eq!(defaults, 1, "no second Default was made");
    }

    /// Defect 1: the replica id outlives a launch (the actor does not), so `Ctrl+Shift+T` finds, after a restart,
    /// what this installation closed -- and only that.
    #[test]
    fn a_close_survives_an_app_restart_with_the_same_replica_id_and_the_shortcut_finds_it() {
        let path = store::testutil::temp_dir("closed-restart").join("session.mnf");
        let mut first = app_on(&path, 1);
        let (closed, kept) = (add_note(&mut first, "closed"), add_note(&mut first, "kept"));
        assert!(first.save_session());
        first.close_node(closed);
        assert_eq!(stored(&first, closed, field::CLOSED), FieldValue::Replica(first.doc.replica));
        let replica = first.doc.replica;
        drop(first);

        // Another installation (a fresh replica id) opens the file: the close is not its own.
        let mut stranger = app_on(&path, 2);
        assert!(stranger.restore_session());
        stranger.reopen_closed_shortcut();
        assert!(!stranger.nodes.contains_key(&closed), "not this replica's close");
        drop(stranger);

        // The same installation, a new launch (new actor, same replica id).
        let mut second = app_on(&path, 3);
        second.doc.replica = replica;
        assert!(second.restore_session());
        assert!(second.nodes.contains_key(&kept) && !second.nodes.contains_key(&closed));
        second.reopen_closed_shortcut();
        assert!(second.nodes.contains_key(&closed), "found after the restart");
        assert_eq!(stored(&second, closed, field::CLOSED), FieldValue::Replica(0));
    }

    /// Defect 1: reopening a group re-stamps the roots nested under it as the replicas that closed them, not as the
    /// reopener.
    #[test]
    fn a_reopen_keeps_each_nested_roots_original_closer() {
        let path = store::testutil::temp_dir("closed-restamp-closer").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let (a, b) = (add_note(&mut app, "a"), add_note(&mut app, "b"));
        app.edit_group(a, Some(g));
        app.edit_group(b, Some(g));
        assert!(app.save_session());
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, a, field::CLOSED, FieldValue::Replica(99));
        assert!(app.save_session());
        app.close_node(b);
        app.close_node(g);
        app.reopen_node(g);
        assert_eq!(stored(&app, a, field::CLOSED), FieldValue::Replica(99), "the other replica's close stays its");
        assert_eq!(stored(&app, b, field::CLOSED), FieldValue::Replica(app.doc.replica));
        assert_eq!(stored(&app, g, field::CLOSED), FieldValue::Replica(0));
    }

    /// Defect 3: the save that commits a reopen may first apply remote ops; the eviction they trigger must not count
    /// the node being reopened as still closed.
    #[test]
    fn a_reopen_is_not_evicted_by_a_remote_close_arriving_in_the_same_save() {
        let path = store::testutil::temp_dir("closed-reopen-remote-evict").join("session.mnf");
        let mut app = app_on(&path, 1);
        let notes: Vec<NodeId> = (0..=MAX_CLOSED).map(|i| add_note(&mut app, &format!("n{i}"))).collect();
        assert!(app.save_session());
        for n in &notes[..MAX_CLOSED] {
            app.close_node(*n);
        }
        assert_eq!(closed_ids(&app).len(), MAX_CLOSED);
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, notes[MAX_CLOSED], field::CLOSED, FieldValue::Replica(99));
        // The oldest close is reopened; the save applies the remote close first (21 closed roots in the document).
        app.reopen_node(notes[0]);
        let doc = app.doc.journal.as_ref().unwrap().doc();
        assert!(!doc.nodes[&notes[0]].deleted, "the reopened node was not evicted");
        assert!(app.nodes.contains_key(&notes[0]));
        assert_eq!(closed_ids(&app).len(), MAX_CLOSED, "19 of ours and the remote one");
        assert!(closed_ids(&app).contains(&notes[MAX_CLOSED]));
    }

    /// Defect 4: a close whose flush of the pending creates fails leaves the node shown and unclosed, and a later
    /// save creates it.
    #[test]
    fn a_close_aborts_when_the_flush_of_pending_creates_fails() {
        let path = store::testutil::temp_dir("closed-flush-fails").join("session.mnf");
        let mut app = app_on(&path, 1);
        let keep = add_note(&mut app, "other");
        assert!(app.save_session());
        let fresh = add_note(&mut app, "fresh");
        assert_eq!(app.doc.writes.life(fresh), Some(crate::session_sync::Life::Create), "created this tick, not saved");
        let set_readonly = |readonly: bool| {
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "the Windows read-only attribute makes the append fail; a temp file this test owns"
            )]
            perms.set_readonly(readonly);
            std::fs::set_permissions(&path, perms).unwrap();
        };
        set_readonly(true);
        app.close_node(fresh);
        set_readonly(false);
        assert!(app.nodes.contains_key(&fresh), "still shown");
        assert!(!app.doc.writes.has_closed(), "no close queued");
        assert_eq!(app.doc.writes.life(fresh), Some(crate::session_sync::Life::Create), "still to be created");
        assert!(app.save_session());
        let doc = app.doc.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&fresh].is_live() && !doc.nodes[&fresh].closed());
        assert!(app.nodes.contains_key(&keep));
    }

    /// The group card's close (the ✕, Del and a middle-click all resolve to `CardControl::Close.action()`) closes the
    /// group with its members, restorable like any card; only the explicit "Dissolve Group" row dissolves it.
    #[test]
    fn a_groups_close_control_closes_it_and_dissolve_stays_explicit() {
        use crate::card::CardControl;
        use crate::node_menu::NodeAction;
        let ctx = egui::Context::default();
        let path = store::testutil::temp_dir("closed-group-x").join("session.mnf");
        let mut app = app_on(&path, 1);
        let (g, inner) = group_with_note(&mut app);
        app.apply_node_action(&ctx, g, CardControl::Close.action());
        assert!(!app.nodes.contains_key(&g) && !app.nodes.contains_key(&inner), "hidden with its members");
        assert_eq!(closed_ids(&app), std::slice::from_ref(&g), "a Recently Closed root, not a delete");
        app.reopen_node(g);
        assert!(app.nodes.contains_key(&g) && app.nodes.contains_key(&inner));
        assert_eq!(app.doc.mirror.group(inner), Some(g), "the members are still in it");

        app.apply_node_action(&ctx, g, NodeAction::Dissolve);
        assert!(!app.nodes.contains_key(&g) && app.nodes.contains_key(&inner), "dissolved: the member stays");
        assert_eq!(app.doc.mirror.group(inner), None);
        assert_eq!(
            closed_ids(&app),
            std::slice::from_ref(&g),
            "the emptied group waits in Recently Closed for an undo"
        );
    }

    /// Past the cap the emptied groups go before any real close, so one Ungroup All never pushes out closed tabs.
    #[test]
    fn eviction_takes_emptied_groups_before_real_closes() {
        let path = store::testutil::temp_dir("evict-emptied").join("session.mnf");
        let mut app = app_on(&path, 1);
        let first = add_note(&mut app, "first closed");
        app.close_node(first);
        let groups: Vec<NodeId> = (0..MAX_CLOSED).map(|_| group_with_note(&mut app).0).collect();
        app.ungroup(&groups);
        let closed = closed_ids(&app);
        assert_eq!(closed.len(), MAX_CLOSED, "capped");
        assert!(closed.contains(&first), "the real close, the oldest entry, survived the emptied groups");
    }

    /// Close Selected with a group and a member inside it both selected: the group closes first (outermost first), the
    /// member is already hidden at its turn and is skipped, so only the group is a closed root and a reopen of it
    /// brings the member back.
    #[test]
    fn close_selected_with_a_group_and_its_member_closes_only_the_group() {
        let ctx = egui::Context::default();
        for _ in 0..8 {
            let path = store::testutil::temp_dir("closed-selected-nested").join("session.mnf");
            let mut app = app_on(&path, 1);
            let (g, inner) = group_with_note(&mut app);
            app.interaction.selected.extend([crate::card_id::CardId::from(inner), crate::card_id::CardId::from(g)]);
            app.apply_graph_action(&ctx, crate::graph_ui::GraphAction::CloseSelected);
            assert!(app.interaction.selected.is_empty());
            assert_eq!(closed_ids(&app), std::slice::from_ref(&g), "the group is the only closed root");
            assert_eq!(
                stored(&app, inner, field::CLOSED),
                FieldValue::Replica(0),
                "the member has no close of its own"
            );
            app.reopen_node(g);
            assert!(app.nodes.contains_key(&inner));
            assert_eq!(app.doc.mirror.group(inner), Some(g));
        }
    }
}
