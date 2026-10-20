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
        // hide below would then lose it): flush the pending creates first. A save that fails leaves them pending
        // (checked below, before anything is hidden).
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
        let (kept, gone): (Vec<String>, Vec<String>) =
            subtree.into_iter().partition(|key| key == id || self.restorable(key));
        // A kept node the flush could not create has no document entry: its `closed` write would be dropped as not
        // editable and the hide would lose the node for good. Leave everything shown instead.
        let uncreated = |key: &String| key.parse().is_ok_and(|n| self.writes.pending_creates.contains(&n));
        if kept.iter().any(uncreated) {
            self.queue_toast("Couldn't save the session, so nothing was closed".to_string());
            return;
        }
        // Where each card was drawn becomes its document pos (2b-4 Ruling 10: a reopen lands where it was shown).
        for key in &kept {
            let carries_pos = Kind::from_type(self.mirror.ty(key)).fields().contains(&field::POS);
            if carries_pos && self.layout.contains_key(key) {
                self.edit_pos(key, Some(self.layout_pos(key)));
            }
        }
        for key in &gone {
            self.remove_node(key);
        }
        self.writes.closed_flags.push((root, self.replica));
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
        self.writes.closed_flags.push((id, 0));
        self.writes.closed_flags.extend(nested.into_iter().map(|(_, d, closer)| (d, closer)));
        self.save_session();
        self.evict_closed();
        let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
        if !doc.nodes.get(&id).is_some_and(|n| n.is_live() && !n.closed()) {
            return;
        }
        // Where the document shows it, not where the graph does: a tab still spawning or waking has no `nodes` entry.
        let placement = doc.placement(id);
        if matches!(placement, Placement::Shown(_)) {
            self.regroup_held(&id.to_string());
        }
        match placement {
            Placement::Hidden => {
                self.queue_toast("Reopened inside a closed group: it shows when that group is reopened".to_string());
            }
            Placement::Shown(group) if group.map(|g| g.to_string()) != self.current_group => {
                let level = match group {
                    None => "the top level".to_string(),
                    Some(g) => {
                        let shown =
                            crate::names::display_name(self.mirror.name(&g.to_string()), crate::names::UNTITLED);
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
        // The document lags this replica's own queued close and reopen writes (`closed_flags`): counting it now could
        // evict the very node being reopened. The save that commits them evicts afterwards (`apply_visibility`'s
        // `apply_remote`, or the caller's own call).
        if !self.writes.closed_flags.is_empty() {
            return;
        }
        let Some(doc) = self.journal.as_ref().map(|j| j.doc()) else { return };
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
        let evicted: Vec<String> = doc
            .with_descendants(victims.into_iter().collect())
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
        assert_eq!(stored(&app, &g, field::CLOSED), FieldValue::Replica(app.replica));
        assert_eq!(stored(&app, &inner, field::CLOSED), FieldValue::Replica(0), "only the root is flagged");
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
        crate::session_sync::tests::remote_set(&mut other, &theirs, field::CLOSED, FieldValue::Replica(99));
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
            crate::session_sync::tests::remote_set(&mut other, n, field::CLOSED, FieldValue::Replica(99));
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
        crate::session_sync::tests::remote_set(&mut b, &notes[0], field::CLOSED, FieldValue::Replica(0)); // ... B's stale reopen.
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
        crate::session_sync::tests::remote_set(&mut other, &g, field::CLOSED, FieldValue::Replica(99));
        let op = store::ops::OpBody::Move { node: default, group: Some(g.parse().unwrap()) };
        other.begin().unwrap().commit(vec![op]).unwrap();
        assert!(app.save_session());
        assert_eq!(app.default_jar(), Some(default), "the same Default, shown again");
        let doc = app.journal.as_ref().unwrap().doc();
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
        let (closed, kept) = (note(&mut first, "closed"), note(&mut first, "kept"));
        assert!(first.save_session());
        first.close_node(&closed);
        assert_eq!(stored(&first, &closed, field::CLOSED), FieldValue::Replica(first.replica));
        let replica = first.replica;
        drop(first);

        // Another installation (a fresh replica id) opens the file: the close is not its own.
        let mut stranger = app_on(&path, 2);
        assert!(stranger.restore_session());
        stranger.reopen_closed_shortcut();
        assert!(!stranger.nodes.contains_key(&closed), "not this replica's close");
        drop(stranger);

        // The same installation, a new launch (new actor, same replica id).
        let mut second = app_on(&path, 3);
        second.replica = replica;
        assert!(second.restore_session());
        assert!(second.nodes.contains_key(&kept) && !second.nodes.contains_key(&closed));
        second.reopen_closed_shortcut();
        assert!(second.nodes.contains_key(&closed), "found after the restart");
        assert_eq!(stored(&second, &closed, field::CLOSED), FieldValue::Replica(0));
    }

    /// Defect 1: reopening a group re-stamps the roots nested under it as the replicas that closed them, not as the
    /// reopener.
    #[test]
    fn a_reopen_keeps_each_nested_roots_original_closer() {
        let path = store::testutil::temp_dir("closed-restamp-closer").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let (a, b) = (note(&mut app, "a"), note(&mut app, "b"));
        app.edit_group(&a, Some(g.clone()));
        app.edit_group(&b, Some(g.clone()));
        assert!(app.save_session());
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, &a, field::CLOSED, FieldValue::Replica(99));
        assert!(app.save_session());
        app.close_node(&b);
        app.close_node(&g);
        app.reopen_node(g.parse().unwrap());
        assert_eq!(stored(&app, &a, field::CLOSED), FieldValue::Replica(99), "the other replica's close stays its");
        assert_eq!(stored(&app, &b, field::CLOSED), FieldValue::Replica(app.replica));
        assert_eq!(stored(&app, &g, field::CLOSED), FieldValue::Replica(0));
    }

    /// Defect 3: the save that commits a reopen may first apply remote ops; the eviction they trigger must not count
    /// the node being reopened as still closed.
    #[test]
    fn a_reopen_is_not_evicted_by_a_remote_close_arriving_in_the_same_save() {
        let path = store::testutil::temp_dir("closed-reopen-remote-evict").join("session.mnf");
        let mut app = app_on(&path, 1);
        let notes: Vec<String> = (0..=MAX_CLOSED).map(|i| note(&mut app, &format!("n{i}"))).collect();
        assert!(app.save_session());
        for n in &notes[..MAX_CLOSED] {
            app.close_node(n);
        }
        assert_eq!(closed_ids(&app).len(), MAX_CLOSED);
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, &notes[MAX_CLOSED], field::CLOSED, FieldValue::Replica(99));
        // The oldest close is reopened; the save applies the remote close first (21 closed roots in the document).
        app.reopen_node(notes[0].parse().unwrap());
        let doc = app.journal.as_ref().unwrap().doc();
        assert!(!doc.nodes[&notes[0].parse().unwrap()].deleted, "the reopened node was not evicted");
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
        let keep = note(&mut app, "other");
        assert!(app.save_session());
        let fresh = note(&mut app, "fresh");
        let fresh_id: NodeId = fresh.parse().unwrap();
        assert!(app.writes.pending_creates.contains(&fresh_id), "created this tick, not saved");
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
        app.close_node(&fresh);
        set_readonly(false);
        assert!(app.nodes.contains_key(&fresh), "still shown");
        assert!(app.writes.closed_flags.is_empty(), "no close queued");
        assert!(app.writes.pending_creates.contains(&fresh_id), "still to be created");
        assert!(app.save_session());
        let doc = app.journal.as_ref().unwrap().doc();
        assert!(doc.nodes[&fresh_id].is_live() && !doc.nodes[&fresh_id].closed());
        assert!(app.nodes.contains_key(&keep));
    }

    /// A group with one kept note inside, saved.
    fn group_with_note(app: &mut App) -> (String, String) {
        let g = add_group(app, "G");
        let inner = note(app, "kept");
        app.edit_group(&inner, Some(g.clone()));
        assert!(app.save_session());
        (g, inner)
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
        app.apply_node_action(&ctx, &g, CardControl::Close.action());
        assert!(!app.nodes.contains_key(&g) && !app.nodes.contains_key(&inner), "hidden with its members");
        assert_eq!(closed_ids(&app), std::slice::from_ref(&g), "a Recently Closed root, not a delete");
        app.reopen_node(g.parse().unwrap());
        assert!(app.nodes.contains_key(&g) && app.nodes.contains_key(&inner));
        assert_eq!(app.mirror.group(&inner), Some(g.as_str()), "the members are still in it");

        app.apply_node_action(&ctx, &g, NodeAction::Dissolve);
        assert!(!app.nodes.contains_key(&g) && app.nodes.contains_key(&inner), "dissolved: the member stays");
        assert_eq!(app.mirror.group(&inner), None);
        assert_eq!(
            closed_ids(&app),
            std::slice::from_ref(&g),
            "the emptied group waits in Recently Closed for an undo"
        );
    }

    /// "Dissolve Group" closes the emptied group holding its members' ids: Undo (or the Recently Closed row) reopens
    /// the same group, with its name and color, around the members still at its level.
    #[test]
    fn dissolve_group_is_undone_by_reopening_the_same_group() {
        use crate::node_menu::NodeAction;
        let ctx = egui::Context::default();
        let color = session_sync::color_value(Some(egui::Color32::from_rgb(1, 2, 3)));
        for from_toast in [true, false] {
            let path = store::testutil::temp_dir("dissolve-undo").join("session.mnf");
            let mut app = app_on(&path, 1);
            let (g, inner) = group_with_note(&mut app);
            let moved_on = note(&mut app, "moved on");
            app.edit_group(&moved_on, Some(g.clone()));
            app.edit_name(&g, "Research");
            app.edit_field(&g, field::COLOR, color.clone());
            assert!(app.save_session());
            app.apply_node_action(&ctx, &g, NodeAction::Dissolve);
            let (message, ids) = app.toasts.undo.take().expect("the dissolve offers an undo");
            assert!(message.starts_with("Dissolved"), "{message}");
            // A member the user put elsewhere meanwhile stays there.
            let elsewhere = add_group(&mut app, "Elsewhere");
            app.edit_group(&moved_on, Some(elsewhere.clone()));
            if from_toast {
                app.undo_ungroup(ids);
            } else {
                app.reopen_node(g.parse().unwrap());
            }
            assert_eq!(app.mirror.group(&inner), Some(g.as_str()), "back in the same group (toast: {from_toast})");
            assert_eq!(app.mirror.group(&moved_on), Some(elsewhere.as_str()));
            assert_eq!(app.mirror.name(&g), "Research");
            assert_eq!(app.mirror.get(&g, field::COLOR), color);
            assert!(closed_ids(&app).is_empty());
            assert_eq!(stored(&app, &g, field::HELD), FieldValue::Str(String::new()), "forgotten once restored");
        }
    }

    /// A merge closes the source group holding its members: Undo reopens it around them, and a member the user moved
    /// on stays where it went. The target keeps the members that stay.
    #[test]
    fn a_merge_is_undone_by_reopening_the_source_group() {
        let path = store::testutil::temp_dir("merge-undo").join("session.mnf");
        let mut app = app_on(&path, 1);
        let (source, kept) = group_with_note(&mut app);
        let (_, left) = group_with_note(&mut app);
        app.edit_group(&left, Some(source.clone()));
        let target = add_group(&mut app, "T");
        assert!(app.save_session());
        app.merge_groups(&source, &target);
        assert_eq!(app.mirror.group(&kept), Some(target.as_str()));
        assert!(closed_ids(&app).contains(&source));
        let (message, ids) = app.toasts.undo.take().expect("the merge offers an undo");
        assert!(message.starts_with("Merged"), "{message}");
        app.edit_group(&left, None);
        app.undo_ungroup(ids);
        assert_eq!(app.mirror.group(&kept), Some(source.as_str()), "back in the source group");
        assert_eq!(app.mirror.group(&left), None, "a member moved on stays");
        assert!(app.nodes.contains_key(&source) && closed_ids(&app).is_empty());
    }

    /// Ungroup All offers one Undo for every group it dissolved.
    #[test]
    fn ungroup_all_is_undone_in_one_step() {
        let path = store::testutil::temp_dir("ungroup-undo").join("session.mnf");
        let mut app = app_on(&path, 1);
        let (g1, n1) = group_with_note(&mut app);
        let (g2, n2) = group_with_note(&mut app);
        app.ungroup(&[g1.clone(), g2.clone()]);
        assert!(app.mirror.group(&n1).is_none() && app.mirror.group(&n2).is_none());
        let (message, ids) = app.toasts.undo.take().expect("an undo");
        assert_eq!(message, "Ungrouped 2 groups");
        app.undo_ungroup(ids);
        assert_eq!(app.mirror.group(&n1), Some(g1.as_str()));
        assert_eq!(app.mirror.group(&n2), Some(g2.as_str()));
    }

    /// Past the cap the emptied groups go before any real close, so one Ungroup All never pushes out closed tabs.
    #[test]
    fn eviction_takes_emptied_groups_before_real_closes() {
        let path = store::testutil::temp_dir("evict-emptied").join("session.mnf");
        let mut app = app_on(&path, 1);
        let first = note(&mut app, "first closed");
        app.close_node(&first);
        let groups: Vec<String> = (0..MAX_CLOSED).map(|_| group_with_note(&mut app).0).collect();
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
            app.interaction.selected.extend([inner.clone(), g.clone()]);
            app.apply_graph_action(&ctx, crate::graph_ui::GraphAction::CloseSelected);
            assert!(app.interaction.selected.is_empty());
            assert_eq!(closed_ids(&app), std::slice::from_ref(&g), "the group is the only closed root");
            assert_eq!(
                stored(&app, &inner, field::CLOSED),
                FieldValue::Replica(0),
                "the member has no close of its own"
            );
            app.reopen_node(g.parse().unwrap());
            assert!(app.nodes.contains_key(&inner));
            assert_eq!(app.mirror.group(&inner), Some(g.as_str()));
        }
    }

    /// A merge drops the source group from the selection with the rest of its UI state (`forget_id`).
    #[test]
    fn a_merged_away_group_leaves_the_selection() {
        let mut app = App::default();
        let (source, target) = (add_group(&mut app, "S"), add_group(&mut app, "T"));
        app.interaction.selected.extend([source.clone(), target.clone()]);
        app.merge_groups(&source, &target);
        assert!(!app.nodes.contains_key(&source));
        assert_eq!(app.interaction.selected, HashSet::from([target]));
    }

    /// A Join (or a menu's Move into) onto a group another replica closed after the menu opened is a no-op: the node
    /// stays where it is rather than vanishing into the hidden group. A merge onto it is refused the same way.
    #[test]
    fn a_join_onto_a_group_closed_remotely_does_nothing() {
        use crate::card::DropEffect;
        let path = store::testutil::temp_dir("closed-join-hidden").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let (n, other_group) = (note(&mut app, "n"), add_group(&mut app, "H"));
        assert!(app.save_session());
        let mut other = store::journal::Journal::open(&path, store::model::ActorId(2)).unwrap();
        crate::session_sync::tests::remote_set(&mut other, &g, field::CLOSED, FieldValue::Replica(99));
        assert!(app.save_session());
        assert!(!app.nodes.contains_key(&g), "closed remotely, so not shown");
        app.apply_drop(&n, Some((DropEffect::Join, Some(g.clone()))));
        app.apply_node_action(&egui::Context::default(), &n, crate::node_menu::NodeAction::MoveToGroup(g.clone()));
        assert_eq!(app.mirror.group(&n), None, "still at the top level");
        assert!(app.nodes.contains_key(&n));
        app.apply_drop(&other_group, Some((DropEffect::Merge, Some(g.clone()))));
        assert!(app.nodes.contains_key(&other_group), "the merge into a hidden group is refused");
    }
}
