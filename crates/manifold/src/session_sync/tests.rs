use super::*;
use crate::redirect_fetch::{FetchResult, FetchTarget};
use crate::store::journal::Journal;
use crate::store::model::ActorId;
use crate::store::testutil::{temp_dir, XorShift};
use crate::{NodeAction, SpawnPurpose};

const A: ActorId = ActorId(0xA);

#[test]
fn jar_fields_belong_to_tabs_groups_and_jars() {
    use field::*;
    assert_eq!(Kind::Tab.fields(), &[POS, COLOR, PARENT, URL, ENGINE, PINNED, JAR, JAR_SET]);
    assert_eq!(Kind::Group.fields(), &[POS, COLOR, NAME, FROM_JSON, DEFAULT_JAR]);
    assert_eq!(Kind::Jar.fields(), &[COLOR, NAME, EPHEMERAL]);
    assert_eq!(Kind::Link.fields(), &[NAME, FROM, TO]);
    assert!(Kind::Unknown.fields().is_empty());
}

/// Waits up to ~1 s for `app.redirect_fetcher` to report something.
/// Under test (no content dir) every fetch fails at once.
fn drain_fetches(app: &mut App) -> Vec<FetchResult> {
    for _ in 0..200 {
        let results = app.redirect_fetcher.tick();
        if !results.is_empty() {
            return results;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Vec::new()
}

#[test]
fn alternating_store_notices_are_each_shown_once_until_a_commit() {
    let dir = crate::store::testutil::temp_dir("notices");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    for _ in 0..3 {
        app.notice("read-only".into());
        app.notice("damaged".into());
    }
    assert_eq!(app.save_toast_queue.len(), 2);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    app.notice("read-only".into());
    assert_eq!(app.save_toast_queue.len(), 3, "a commit lets a notice show again");
}

#[test]
fn recovery_notices_say_what_was_lost_and_where_the_original_is() {
    let r =
        Recovery { damaged: vec![(10, 20)], saved: Some("C:/x/session.mnf.damaged-1".into()), ..Recovery::default() };
    assert_eq!(
        recovery_notice(&r),
        "The session file was damaged; 1 damaged part was skipped. The original is saved at C:/x/session.mnf.damaged-1"
    );
    let r = Recovery { from_prev: true, saved: None, ..Recovery::default() };
    assert!(recovery_notice(&r).contains("rebuilt from its previous copy"));
}

fn id(counter: u64) -> NodeId {
    NodeId { actor: A, counter }
}

fn shared(kind: Kind, group: Option<NodeId>, set: &[(u32, FieldValue)]) -> Snapshot {
    let mut fields: Fields =
        kind.fields().iter().map(|&no| (no, FieldValue::absent(field_def(no).unwrap().ty))).collect();
    for (no, value) in set {
        fields.insert(*no, value.clone());
    }
    Snapshot { kind, group, fields }
}

fn note(text: &str, group: Option<NodeId>) -> Snapshot {
    shared(Kind::Note, group, &[(field::TEXT, FieldValue::Str(text.into()))])
}

fn group(name: &str) -> Snapshot {
    shared(Kind::Group, None, &[(field::NAME, FieldValue::Str(name.into()))])
}

/// What `doc` shows, shaped like `snapshots`.
fn shown(doc: &Doc) -> BTreeMap<NodeId, Snapshot> {
    doc.live()
        .map(|(id, node)| (id, Snapshot { kind: node.kind(), group: doc.effective_group(id), fields: fields_of(node) }))
        .collect()
}

/// What the store holds for every live node and jar (`App::snapshot`), by store id: an in-progress download, a
/// hand-built id and a tab whose spawn is in flight are left out. `pub(crate)`: other modules' tests compare it too.
pub(crate) fn snapshots(app: &App) -> BTreeMap<NodeId, Snapshot> {
    let nodes = app.nodes.keys().filter_map(|key| key.parse::<NodeId>().ok());
    let jars = app.mirror.jar_defs().keys().copied();
    nodes.chain(jars).filter_map(|id| Some((id, app.snapshot(id)?))).collect()
}

/// Actor 1's op count: what a save wrote.
fn ops_of(app: &App) -> u64 {
    app.journal.as_ref().unwrap().doc().version.get(&ActorId(1)).copied().unwrap_or(0)
}

/// Whether the document shows `id` live.
fn live_in_doc(app: &App, id: &str) -> bool {
    app.journal.as_ref().unwrap().doc().nodes.get(&id.parse().unwrap()).is_some_and(Node::is_live)
}

/// The commit builder (2b-5 Task 3): a save creates every queued node the document lacks, each group before the
/// nodes inside it, so a member's `Create` names a group that exists. The member's id sorts before its group's.
#[test]
fn new_nodes_are_created_groups_first() {
    let mut app = app_on(&temp_dir("create-groups-first").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "inside".into(), editing: false }));
    let group = add_group(&mut app, "G");
    assert!(note < group, "the member's id sorts first");
    app.edit_group(&note, Some(group.clone()));
    assert!(app.save_session());
    assert!(app.pending_creates.is_empty(), "created");
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(note.parse().unwrap()), Some(group.parse().unwrap()));
    assert_eq!(doc.nodes[&group.parse().unwrap()].get(field::NAME), FieldValue::Str("G".into()));
    assert_eq!(shown(doc), snapshots(&app));
}

#[test]
fn an_unchanged_graph_emits_nothing() {
    let mut app = app_on(&temp_dir("unchanged-nothing").join("session.mnf"), 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "a".into(), editing: false }));
    assert!(app.save_session());
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "a second save writes nothing");
}

/// A node removed here is deleted at the next save (`queue_delete`); a tab mid-respawn is not removed, so it is kept
/// (Ruling 4: `spawning` is no longer a keep list, respawns just never queue).
#[test]
fn a_removed_node_is_deleted_and_a_respawning_one_kept() {
    let mut app = app_on(&temp_dir("delete-unless-respawning").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "a".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.remove_node(&note);
    start_respawn(&mut app, &tab);
    assert!(app.save_session());
    assert!(!live_in_doc(&app, &note), "deleted");
    assert!(live_in_doc(&app, &tab), "kept while respawning");
    assert!(app.pending_deletes.is_empty());
}

/// A queued create for an id the document holds as a tombstone is dropped (ids are never reused): here a tab
/// another replica deleted while it was waking, which is still shown until the wake lands.
#[test]
fn a_tombstone_is_never_recreated() {
    let path = temp_dir("tombstone-no-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.focus_tab(&tab);
    assert!(app.spawning.contains_key(&tab) && app.nodes.contains_key(&tab), "waking: shown and in flight");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: tab.parse().unwrap() }]).unwrap();
    app.pending_creates.insert(tab.parse().unwrap());
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "no Create");
    assert!(!live_in_doc(&app, &tab));
    fail_spawns(&mut app);
    assert!(!app.nodes.contains_key(&tab), "the parked remote delete lands");
}

/// Another replica deleted a group: its members are shown at the top here, but that is the document's own
/// effective group, so the next save writes no `Move`.
#[test]
fn re_homing_out_of_a_deleted_group_is_not_a_move() {
    let path = temp_dir("rehome-no-move").join("session.mnf");
    let mut app = app_on(&path, 1);
    let group = add_group(&mut app, "G");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_group(&tab, Some(group.clone()));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: group.parse().unwrap() }]).unwrap();
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&group) && app.nodes.contains_key(&tab));
    assert_eq!(app.mirror.group(&tab), None, "shown at the top");
    assert_eq!(ops_of(&app), ops, "nothing written");
}

/// A new node's `parent` naming an id the document has never seen is written as none (Ruling 12: one resolver for
/// every Id a `Create` carries).
#[test]
fn a_parent_naming_a_missing_node_is_written_as_none() {
    let mut app = app_on(&temp_dir("parent-missing").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_parent(&tab, Some(id(9).to_string()));
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::PARENT), FieldValue::Id(None));
}

/// A created node's parent or group that the same save deletes is written as none, not as the dead id (Ruling 5;
/// final review minor 3): a new tab opened from a tab into a group, both closed before its spawn landed.
#[test]
fn a_create_naming_a_node_deleted_by_the_same_save_writes_none() {
    let mut app = app_on(&temp_dir("create-names-deleted").join("session.mnf"), 1);
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://o/", ""));
    let group = add_group(&mut app, "G");
    assert!(app.save_session());
    let tab = app.spawn_tab_at("https://n/", egui::Vec2::ZERO, Some(group.clone()), Some(opener.clone()));
    assert!(app.spawning.contains_key(&tab));
    app.remove_tab(&opener);
    app.dissolve_group(&group);
    assert!(app.save_session());
    assert!(!live_in_doc(&app, &opener) && !live_in_doc(&app, &group));
    assert!(live_in_doc(&app, &tab), "created from its spawn snapshot");
    assert_eq!(stored(&app, &tab, field::PARENT), FieldValue::Id(None));
    assert_eq!(app.journal.as_ref().unwrap().doc().nodes[&tab.parse().unwrap()].group, None);
}

/// Random local edits through the app's writers (place, group, rename, type, close), each followed by one save:
/// the document then shows exactly the live graph, and a second save writes nothing.
#[test]
fn random_edits_converge_in_one_save() {
    let mut rng = XorShift(0x5E55);
    let mut app = app_on(&temp_dir("random-converge").join("session.mnf"), 1);
    // The model: each live node's kind and group.
    let mut model: BTreeMap<String, (Kind, Option<String>)> = BTreeMap::new();
    let groups = |model: &BTreeMap<String, (Kind, Option<String>)>| -> Vec<String> {
        model.iter().filter(|(_, (k, _))| *k == Kind::Group).map(|(id, _)| id.clone()).collect()
    };
    for _ in 0..120 {
        match rng.below(5) {
            0 | 1 => {
                let groups = groups(&model);
                let group = (!groups.is_empty() && rng.below(2) == 0)
                    .then(|| groups[rng.below(groups.len() as u64) as usize].clone());
                let (id, kind) = if rng.below(3) == 0 {
                    (add_group(&mut app, "g"), Kind::Group)
                } else {
                    (
                        add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false })),
                        Kind::Note,
                    )
                };
                app.edit_group(&id, group.clone());
                model.insert(id, (kind, group));
            }
            2 if !model.is_empty() => {
                let key = model.keys().nth(rng.below(model.len() as u64) as usize).unwrap().clone();
                let value = rng.below(1 << 24).to_string();
                if model[&key].0 == Kind::Group {
                    app.edit_name(&key, &value);
                } else {
                    app.edit_text(&key, &value);
                }
            }
            3 if !model.is_empty() => {
                let key = model.keys().nth(rng.below(model.len() as u64) as usize).unwrap().clone();
                // Destroying a group destroys what it holds, as the app does.
                let doomed: Vec<String> =
                    model.keys().filter(|n| **n == key || is_inside(&model, n, &key)).cloned().collect();
                app.destroy_node(&key);
                for n in doomed {
                    model.remove(&n);
                }
            }
            4 if !model.is_empty() => {
                let key = model.keys().nth(rng.below(model.len() as u64) as usize).unwrap().clone();
                // A move into `key` itself or one of its descendants would create a cycle.
                let candidates: Vec<Option<String>> = std::iter::once(None)
                    .chain(groups(&model).into_iter().map(Some))
                    .filter(|g| g.as_ref().is_none_or(|g| *g != key && !is_inside(&model, g, &key)))
                    .collect();
                let choice = candidates[rng.below(candidates.len() as u64) as usize].clone();
                app.edit_group(&key, choice.clone());
                model.get_mut(&key).unwrap().1 = choice;
            }
            _ => {}
        }
        assert!(app.save_session());
        let doc = app.journal.as_ref().unwrap().doc();
        assert_eq!(shown(doc), snapshots(&app), "the document shows the live graph");
        let live: BTreeMap<String, (Kind, Option<String>)> = doc
            .live()
            .map(|(id, n)| (id.to_string(), (n.kind(), doc.effective_group(id).map(|g| g.to_string()))))
            .collect();
        assert_eq!(live, model, "and the model");
        let ops = ops_of(&app);
        assert!(app.save_session());
        assert_eq!(ops_of(&app), ops, "a second save writes nothing");
    }
}

fn is_inside(model: &BTreeMap<String, (Kind, Option<String>)>, node: &str, ancestor: &str) -> bool {
    let mut at = model.get(node).and_then(|(_, g)| g.as_deref());
    while let Some(g) = at {
        if g == ancestor {
            return true;
        }
        at = model.get(g).and_then(|(_, g)| g.as_deref());
    }
    false
}

/// A Root/New spawn that fails keeps its place as a failed card, in the document whether or not a save had created
/// it while in flight; Split, whose pane is not a card, is deleted and only toasts.
#[test]
fn a_failed_new_spawn_leaves_a_failed_card() {
    for saved_first in [true, false] {
        let mut app = app_on(&temp_dir("failed-new-spawn").join("session.mnf"), 1);
        let id = app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
        if saved_first {
            assert!(app.save_session());
            assert!(live_in_doc(&app, &id), "created while in flight");
        }
        fail_spawns(&mut app);
        let tab = app.nodes[&id].as_unloaded().expect("a failed card");
        assert!(matches!(tab.state, UnloadedState::Failed(_)), "saved_first={saved_first}");
        assert_eq!(tab.url, "https://a/");
        assert_eq!(app.card_kind(&id).map(|kind| kind.ty), Some(crate::store::model::TYPE_TAB));
        assert!(live_in_doc(&app, &id), "persisted, saved_first={saved_first}");
        assert!(app.pending_creates.is_empty() && app.pending_deletes.is_empty());
    }
}

/// A failed Root also leaves the app in Graph view (success does), so the card is seen.
#[test]
fn a_failed_root_spawn_shows_its_card_in_graph_view() {
    let mut app = app_on(&temp_dir("failed-root-spawn").join("session.mnf"), 1);
    assert_eq!(app.view_mode, ViewMode::Item, "the app starts in Item view");
    app.spawn_root_tab();
    fail_spawns(&mut app);
    assert_eq!(app.nodes.len(), 1);
    assert_eq!(app.view_mode, ViewMode::Graph);
}

/// A failed Split makes no card: its node is deleted, and the failure only toasts.
#[test]
fn a_failed_split_spawn_toasts_and_makes_no_card() {
    let mut app = app_on(&temp_dir("failed-split-spawn").join("session.mnf"), 1);
    let id = app.spawn_new_tab_with_kind(None, None, "https://s/", crate::EngineKind::Servo, SpawnPurpose::Split, None);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(!app.nodes.contains_key(&id), "no card for a split pane");
    assert!(!live_in_doc(&app, &id), "deleted by the failure's save");
    assert!(app.save_toast_queue.iter().any(|toast| toast.contains("Couldn't start split view")));
}

/// Open Session: the replaced session's nodes of every kind are deleted from the document; its jars stay (they
/// are not part of the graph), and the new nodes are created.
#[test]
fn teardown_deletes_every_old_document_node_but_keeps_jars() {
    let path = temp_dir("teardown-deletes").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "old".into(), editing: false }));
    let group = add_group(&mut app, "G");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://old/", ""));
    app.edit_group(&tab, Some(group.clone()));
    let jar = app.create_jar("Work");
    assert!(app.save_session());
    let json =
        JsonDoc { nodes: vec![json_node("t", Kind::Tab, serde_json::json!({ "url": "https://new/" }))], local: None };
    app.replace_running_session(&json, crate::store::json::Trust::Foreign);
    for old in [&note, &group, &tab] {
        assert!(!live_in_doc(&app, old), "{old} deleted");
    }
    assert!(live_in_doc(&app, &jar.to_string()), "the jar stays");
    let doc = app.journal.as_ref().unwrap().doc();
    let urls: Vec<FieldValue> =
        doc.live().filter(|(_, n)| n.kind() == Kind::Tab).map(|(_, n)| n.get(field::URL)).collect();
    assert_eq!(urls, [FieldValue::Str("https://new/".into())]);
    assert!(app.pending_deletes.is_empty());
}

/// Another replica's delete removes the node here and queues nothing: the document already lacks it.
#[test]
fn a_remote_removal_queues_no_delete() {
    let mut app = app_on(&temp_dir("remote-removal-queues-nothing").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    let group = add_group(&mut app, "G");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_group(&tab, Some(group.clone()));
    let jar = app.create_jar("Work");
    assert!(app.save_session());
    for id in [note.parse().unwrap(), tab.parse().unwrap(), group.parse().unwrap(), jar] {
        app.apply_remote(vec![(id, None)]);
        assert!(!app.nodes.contains_key(&id.to_string()) && !app.mirror.jar_defs().contains_key(&id), "{id} removed");
        assert!(app.pending_deletes.is_empty(), "{id}: nothing queued");
    }
}

/// A crash respawn, an engine swap and a content reload take the node out of `nodes` and put it back: none
/// queues a delete, so a save in between keeps the document node.
#[test]
fn a_crash_respawn_a_swap_and_a_reload_delete_nothing() {
    let dir = temp_dir("respawn-swap-reload-keep");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    let crashed = add(&mut app, Kind::Tab, sleeping_tab("https://c/", ""));
    let (swapped, _peer) = live_tab(&mut app, EngineKind::Servo, "https://s/");
    let json = app.alloc_node_id();
    let body = crate::redirect_fetch::save(&dir, &json, crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    let source = crate::ContentSource::Web { url: "https://x.test/j".into(), body };
    app.insert_content(source, json.clone(), egui::Vec2::ZERO, None).unwrap();
    assert!(app.save_session());
    start_respawn(&mut app, &crashed);
    app.finish_swap(&swapped, EngineKind::Webview2, "https://s/".into());
    assert!(app.spawning.contains_key(&swapped) && !app.nodes.contains_key(&swapped), "swapping");
    let moved = dir.join("moved.json");
    std::fs::write(&moved, b"[2]").unwrap();
    let path = moved.to_string_lossy().into_owned();
    app.reload_fetched_node(&json, crate::WebBody::File { kind: crate::content::ContentKind::Json, path });
    assert!(app.pending_deletes.is_empty(), "nothing queued");
    assert!(app.save_session());
    for id in [&crashed, &swapped, &json] {
        assert!(live_in_doc(&app, id), "{id} kept");
    }
}

/// A download is created once it completes: an in-progress one can't resume, so it is not written, but its queued
/// create waits for the save after completion.
#[test]
fn a_download_is_created_once_it_completes() {
    let mut app = app_on(&temp_dir("download-create-on-complete").join("session.mnf"), 1);
    let payload = DownloadPayload {
        path: "f.bin".into(),
        bytes_received: 1,
        total_bytes: Some(2),
        completed: false,
        failed: false,
    };
    let dl = add(&mut app, Kind::Download, Runtime::Download(payload));
    assert!(app.save_session());
    assert!(!app.journal.as_ref().unwrap().doc().nodes.contains_key(&dl.parse().unwrap()), "not yet");
    assert!(app.pending_creates.contains(&dl.parse().unwrap()), "still queued");
    app.nodes.get_mut(&dl).unwrap().runtime = Runtime::Download(DownloadPayload {
        path: "f.bin".into(),
        bytes_received: 2,
        total_bytes: Some(2),
        completed: true,
        failed: false,
    });
    assert!(app.save_session());
    assert!(live_in_doc(&app, &dl), "created");
}

/// A jar from an opened or imported file (a fresh id) is created like a local one.
#[test]
fn an_imported_jar_is_created() {
    let mut app = app_on(&temp_dir("imported-jar-created").join("session.mnf"), 1);
    app.replace_running_session(&file_with_a_work_jar(), crate::store::json::Trust::Foreign);
    let jar = *app.mirror.jar_defs().keys().next().expect("the jar imported");
    assert!(live_in_doc(&app, &jar.to_string()));
}

/// Ruling 14: in a test build every save checks that the live graph and the document agree, so a node that leaves
/// `nodes` behind the queues' back (a missed delete funnel) is caught at once.
#[test]
#[should_panic(expected = "delete funnel")]
fn a_node_removed_behind_the_queues_back_trips_the_invariant() {
    let mut app = app_on(&temp_dir("invariant-missed-delete").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    app.nodes.remove(&note);
    app.save_session();
}

/// The same for a node that enters `nodes` without `place_node` (a missed create funnel).
#[test]
#[should_panic(expected = "create funnel")]
fn a_node_inserted_behind_the_queues_back_trips_the_invariant() {
    let mut app = app_on(&temp_dir("invariant-missed-create").join("session.mnf"), 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    let id = app.alloc_node_id();
    let payload = Runtime::Note(NotePayload { text: "stray".into(), editing: false });
    // Behind `place_node` on purpose, so the mirror is given the type by hand: a stray node, not an untyped one.
    app.mirror.set(&id, field::TYPE, FieldValue::Str(Kind::Note.type_str().unwrap().to_owned()));
    app.nodes.insert(id, crate::GraphNode { runtime: payload });
    app.save_session();
}

/// A node whose runtime does not run its stored type (a create that passed a mismatched kind) trips it too.
#[test]
#[should_panic(expected = "mismatched kind")]
fn a_node_whose_runtime_does_not_match_its_type_trips_the_invariant() {
    let mut app = app_on(&temp_dir("invariant-mismatched-kind").join("session.mnf"), 1);
    add(&mut app, Kind::Image, Runtime::Pdf(PdfPayload { path: "d.pdf".into(), outcome: None }));
    app.save_session();
}

/// Materializes `nodes` into a windowless app, as `apply_remote` would
/// show another replica's new nodes.
fn app_with(nodes: &BTreeMap<NodeId, Snapshot>) -> App {
    let mut app = App::default();
    let changes = nodes.iter().map(|(&id, s)| (id, Some((s.kind, s.fields.clone(), s.group)))).collect();
    app.apply_remote(changes);
    app
}

#[test]
fn a_tabs_jar_and_a_groups_default_project_to_fields_and_back() {
    let jar = NodeId { actor: ActorId(7), counter: 1 };
    let nodes = BTreeMap::from([
        (
            id(1),
            shared(
                Kind::Tab,
                None,
                &[
                    (field::URL, FieldValue::Str("https://a.com/".into())),
                    (field::ENGINE, FieldValue::Engine(engine_code(EngineKind::Servo))),
                    (field::JAR, FieldValue::Id(Some(jar))),
                    (field::JAR_SET, FieldValue::Bool(true)),
                ],
            ),
        ),
        (id(2), shared(Kind::Group, None, &[(field::DEFAULT_JAR, FieldValue::Id(Some(jar)))])),
    ]);
    let app = app_with(&nodes);
    assert!(app.nodes[&id(1).to_string()].runtime.as_unloaded().is_some());
    assert_eq!(app.mirror.jar_choice(&id(1).to_string()), crate::jar::JarChoice { jar: Some(jar), set: true });
    assert_eq!(app.mirror.default_jar(&id(2).to_string()), Some(jar));
    assert_eq!(snapshots(&app), nodes, "the live payloads project back to the same fields");
}

#[test]
fn a_remote_jar_edit_updates_a_sleeping_tab_and_a_group() {
    let jar = NodeId { actor: ActorId(2), counter: 50 };
    let tab = shared(Kind::Tab, None, &[(field::URL, FieldValue::Str("https://a.com/".into()))]);
    let group = shared(Kind::Group, None, &[]);
    let mut app = app_with(&BTreeMap::from([(id(1), tab.clone()), (id(2), group.clone())]));
    let mut tab_fields = tab.fields.clone();
    tab_fields.insert(field::JAR, FieldValue::Id(Some(jar)));
    let mut group_fields = group.fields.clone();
    group_fields.insert(field::DEFAULT_JAR, FieldValue::Id(Some(jar)));
    app.apply_remote(vec![
        (id(1), Some((Kind::Tab, tab_fields, None))),
        (id(2), Some((Kind::Group, group_fields, None))),
    ]);
    assert_eq!(app.mirror.jar_choice(&id(1).to_string()).jar, Some(jar));
    assert_eq!(app.mirror.default_jar(&id(2).to_string()), Some(jar));
    assert_eq!(app.inherited_jar(Some(&id(2).to_string())), Some(jar), "new tabs in the group start in it");
}

#[test]
fn a_restored_note_group_and_leaf_project_back_to_their_fields() {
    let red = color_value(Some(egui::Color32::from_rgb(200, 10, 10)));
    let nodes = BTreeMap::from([
        (
            id(0),
            shared(
                Kind::Note,
                Some(id(1)),
                &[
                    (field::TEXT, FieldValue::Str("hello".into())),
                    (field::POS, FieldValue::Pos(3.0, -4.0)),
                    (field::COLOR, red),
                ],
            ),
        ),
        (
            id(1),
            shared(
                Kind::Group,
                None,
                &[(field::NAME, FieldValue::Str("G".into())), (field::FROM_JSON, FieldValue::Bool(true))],
            ),
        ),
        (
            id(2),
            shared(
                Kind::JsonLeaf,
                Some(id(1)),
                &[
                    (field::PATH, FieldValue::Str("C:/data.json".into())),
                    (field::KEY, FieldValue::Str("answer".into())),
                    (field::VALUE, FieldValue::Str("42".into())),
                ],
            ),
        ),
    ]);
    assert_eq!(snapshots(&app_with(&nodes)), nodes);
}

/// An App holding one Json node, id `id(1)`, loaded from a temp file.
fn app_with_json_file(name: &str) -> (App, String) {
    let dir = crate::store::testutil::temp_dir(name);
    let path = dir.join("v.json");
    std::fs::write(&path, r#"{"users": [{"name": "a"}]}"#).unwrap();
    let mut app = App::default();
    let node = id(1).to_string();
    app.insert_json_node_with_id(node.clone(), path.to_string_lossy().into_owned(), egui::Vec2::ZERO, None).unwrap();
    (app, node)
}

#[test]
fn a_json_nodes_view_round_trips_through_the_store() {
    let (mut app, node) = app_with_json_file("json_view_field");
    app.edit_view(&node, r#"{"/users":{"collapsed":true}}"#);

    let shared = snapshots(&app);
    let fields = &shared[&id(1)].fields;
    assert_eq!(fields.get(&field::VIEW), Some(&FieldValue::Str(r#"{"/users":{"collapsed":true}}"#.into())));

    // Restoring those fields gives the same view back.
    let mut restored = App::default();
    restored.materialize(node.clone(), Kind::Json, fields, None, None);
    assert_eq!(restored.mirror.view(&node).unwrap().get("/users").unwrap().collapsed, Some(true));
}

#[test]
fn a_remote_view_change_updates_the_node_in_place() {
    let (mut app, node) = app_with_json_file("json_view_remote");
    let (value, path) = app.nodes[&node].as_json().map(|j| (j.value.clone(), j.path.clone())).unwrap();
    let mut fields = snapshots(&app)[&id(1)].fields.clone();
    fields.insert(field::VIEW, FieldValue::Str(r#"{"":{"collapsed":true}}"#.into()));
    app.materialize(node.clone(), Kind::Json, &fields, None, None);
    assert_eq!(app.mirror.view(&node).unwrap().get("").unwrap().collapsed, Some(true));
    let json = app.nodes[&node].as_json().unwrap();
    assert_eq!((&json.value, &json.path), (&value, &path), "only the view changed");
}

#[test]
fn a_missing_content_file_with_a_known_url_becomes_a_placeholder_that_refetches() {
    let nodes = BTreeMap::from([(
        id(0),
        shared(
            Kind::Json,
            None,
            &[
                (field::PATH, FieldValue::Str(r"Z:\gone.json".into())),
                (field::URL, FieldValue::Str("https://x.test/a.json".into())),
            ],
        ),
    )]);
    let mut app = app_with(&nodes);
    let node_id = id(0).to_string();
    match &app.nodes[&node_id].runtime {
        Runtime::Json(p) => {
            assert!(p.error.is_none(), "a known-URL miss never reaches insert_json_node_with_id's own toast");
            assert!(!p.leaf);
        }
        _ => panic!("expected a Json placeholder"),
    }
    assert_eq!(app.source_url(&node_id), Some("https://x.test/a.json"));
    assert!(app.save_toast_queue.is_empty() && app.save_toast.is_none(), "no 'Couldn't open' toast");
    let results = drain_fetches(&mut app);
    assert_eq!(results.len(), 1, "materialize's own refresh_node queued exactly one fetch");
    assert!(matches!(&results[0].job.target, FetchTarget::Node(n) if *n == node_id));
    assert!(results[0].body.is_none(), "no content dir under test, so the fetch itself fails");
}

/// Its refetch failing (no content dir under test) turns each into the
/// missing-file card, with no toast: the card and its Retry are the
/// feedback.
#[test]
fn a_failed_refetch_of_a_missing_file_shows_the_missing_file_card() {
    let url = |u: &str| (field::URL, FieldValue::Str(u.into()));
    let gone = |p: &str| (field::PATH, FieldValue::Str(p.into()));
    let nodes = BTreeMap::from([
        (id(0), shared(Kind::Json, None, &[gone(r"Z:\gone.json"), url("https://x.test/a.json")])),
        (id(1), shared(Kind::Image, None, &[gone(r"Z:\gone.png"), url("https://x.test/a.png")])),
        (id(2), shared(Kind::Pdf, None, &[gone(r"Z:\gone.pdf"), url("https://x.test/a.pdf")])),
    ]);
    let mut app = app_with(&nodes);
    let mut results = Vec::new();
    while results.len() < 3 {
        let more = drain_fetches(&mut app);
        assert!(!more.is_empty(), "every missing file was refetched");
        results.extend(more);
    }
    for FetchResult { job, body } in results {
        assert!(body.is_none());
        let FetchTarget::Node(node) = &job.target else { panic!("a node refetch") };
        app.refresh_fetch_failed(node);
    }
    let error = |n: u64| match &app.nodes[&id(n).to_string()].runtime {
        Runtime::Json(p) => p.error.clone(),
        Runtime::Image(p) => p.error.clone(),
        Runtime::Pdf(p) => p.outcome.as_ref().and_then(|o| o.as_ref().err()).cloned(),
        _ => None,
    };
    assert_eq!(error(0).as_deref(), Some("Couldn't fetch https://x.test/a.json"));
    assert_eq!(error(1).as_deref(), Some("Couldn't fetch https://x.test/a.png"));
    assert_eq!(error(2).as_deref(), Some("Couldn't fetch https://x.test/a.pdf"));
    assert!(app.save_toast_queue.is_empty() && app.save_toast.is_none(), "no toast");
}

#[test]
fn retry_missing_file_refetches_a_node_with_a_url_instead_of_retrying_its_local_path() {
    let nodes = BTreeMap::from([(
        id(0),
        shared(
            Kind::Json,
            None,
            &[
                (field::PATH, FieldValue::Str(r"Z:\gone.json".into())),
                (field::URL, FieldValue::Str("https://x.test/a.json".into())),
            ],
        ),
    )]);
    let mut app = app_with(&nodes);
    let node_id = id(0).to_string();
    // `materialize` already queued one refetch on its own; drain and
    // discard it so the assertion below reflects only the click.
    drain_fetches(&mut app);
    app.retry_missing_file(&node_id);
    let results = drain_fetches(&mut app);
    assert_eq!(results.len(), 1, "the URL branch queued exactly one fetch");
    assert!(matches!(&results[0].job.target, FetchTarget::Node(n) if *n == node_id));
}

#[test]
fn a_remotely_removed_group_leaves_no_member_naming_it() {
    let nodes = BTreeMap::from([
        (id(0), group("Outer")),
        (id(1), shared(Kind::Group, Some(id(0)), &[(field::NAME, FieldValue::Str("Inner".into()))])),
        (id(2), note("stored member", Some(id(1)))),
    ]);
    let mut app = app_with(&nodes);
    // A note only this replica has, not yet saved, inside the group.
    let local = id(3).to_string();
    let payload = Runtime::Note(NotePayload { text: "local".into(), editing: false });
    app.insert_placeholder_node(local.clone(), egui::Vec2::ZERO, Kind::Note, payload);
    app.edit_group(&local, Some(id(1).to_string()));
    app.current_group = Some(id(1).to_string());
    // What `remote_changes` reports when another replica deletes Inner:
    // the group gone, its stored member now shown in Outer.
    let member = nodes[&id(2)].fields.clone();
    app.apply_remote(vec![(id(1), None), (id(2), Some((Kind::Note, member, Some(id(0)))))]);
    let inner = id(1).to_string();
    assert!(!app.nodes.contains_key(&inner) && !app.node_order.contains(&inner));
    let outer = Some(id(0).to_string());
    assert_eq!(app.mirror.group(&id(2).to_string()), outer.as_deref());
    assert_eq!(app.mirror.group(&local), outer.as_deref(), "the local-only member moves to the effective group");
    assert_eq!(app.current_group, outer, "the view leaves the removed level");
    assert!(app.nodes.keys().all(|n| app.mirror.group(n) != Some(inner.as_str())));
}

/// A windowless app saving to `path` as `actor`. `pub(crate)`: also
/// used by `main.rs`'s own windowless tests (e.g. `closed_entry`'s).
pub(crate) fn app_on(path: &std::path::Path, actor: u64) -> App {
    App { journal: Some(Journal::open(path, ActorId(actor)).unwrap()), ..App::default() }
}

/// A node of `kind` at the top level with `runtime` as its live state; its type is seeded from `kind`.
pub(crate) fn add(app: &mut App, kind: Kind, runtime: Runtime) -> String {
    let id = app.alloc_node_id();
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, kind, runtime);
    id
}

/// A group named `name` at the top level (`App::place_group`).
pub(crate) fn add_group(app: &mut App, name: &str) -> String {
    let id = app.alloc_node_id();
    app.place_group(Some(id), name, crate::fetched::Placement { group: None, pos: egui::Vec2::ZERO })
}

/// Another replica sets one field of `node`.
pub(crate) fn remote_set(other: &mut Journal, node: &str, no: u32, value: FieldValue) {
    let node = node.parse().unwrap();
    other.begin().unwrap().commit(vec![OpBody::Set { node, field: no, enc: encode_field(no, &value) }]).unwrap();
}

/// `node`'s field `no` as the document stores it.
pub(crate) fn stored(app: &App, node: &str, no: u32) -> FieldValue {
    app.journal.as_ref().unwrap().doc().nodes[&node.parse().unwrap()].get(no)
}

#[test]
fn a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another() {
    let path = temp_dir("sync-fields").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();

    // Unsaved here: typing in the note, and a navigation (the live url).
    if let Runtime::Note(n) = &mut app.nodes.get_mut(&note).unwrap().runtime {
        n.text = "typing".into();
    }
    if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
        t.url = "https://u1/".into();
    }
    let red = color_value(Some(egui::Color32::RED));
    remote_set(&mut other, &note, field::COLOR, red.clone());
    remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(true));

    // The remote round applies, then the next round commits.
    assert!(app.save_session());
    let live = &app.nodes[&note];
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED));
    assert!(matches!(&live.runtime, Runtime::Note(n) if n.text == "typing"));
    assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("typing".into()));
    assert!(app.mirror.pinned(&tab));
    assert!(matches!(&app.nodes[&tab].runtime, Runtime::Unloaded(t) if t.url == "https://u1/"));
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://u0/".into()));

    // A later remote edit to the same field does apply.
    remote_set(&mut other, &note, field::TEXT, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert!(matches!(&app.nodes[&note].runtime, Runtime::Note(n) if n.text == "theirs"));
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED));
}

/// The note's text box has focus and holds `text` (2b-4 Ruling 6: the payload is the edit buffer).
fn type_into(app: &mut App, note: &str, text: &str) {
    let n = app.nodes.get_mut(note).unwrap().as_note_mut().unwrap();
    n.editing = true;
    n.text = text.into();
}

fn buffer(app: &App, note: &str) -> String {
    app.nodes[note].as_note().unwrap().text.clone()
}

/// Ruling 6: typing writes the mirror only when a save or the blur commits it, never per keystroke.
#[test]
fn typing_commits_on_blur_and_at_save() {
    let path = temp_dir("text-commit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());
    assert_eq!(app.mirror.text(&note), "v1");
    type_into(&mut app, &note, "v2");
    assert_eq!(app.mirror.text(&note), "v1", "no mirror write per keystroke");
    assert_eq!(app.note_text(&note), "v2", "the card shows the buffer while it is typed in");
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("v2".into()), "a save while editing commits it");
    assert_eq!(app.mirror.text(&note), "v2");
    assert!(app.nodes[&note].as_note().unwrap().editing, "a save doesn't end the edit");
    type_into(&mut app, &note, "v3");
    app.blur_note(&note);
    assert!(!app.nodes[&note].as_note().unwrap().editing);
    assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("v3".into()), "the blur commits it");
    assert_eq!((app.mirror.text(&note), app.note_text(&note)), ("v3", "v3"));
}

/// Ruling 7: a remote text change that arrives while the note is typed in leaves the box alone and is held. At the
/// blur, what the user typed wins and the held value is dropped; with nothing typed, the held value applies.
#[test]
fn a_remote_text_change_is_held_while_the_note_is_typed_in() {
    for typed in [true, false] {
        let path = temp_dir("text-held").join("session.mnf");
        let mut app = app_on(&path, 1);
        let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        type_into(&mut app, &note, "v1");
        remote_set(&mut other, &note, field::TEXT, FieldValue::Str("theirs".into()));
        remote_set(&mut other, &note, field::COLOR, red());
        assert!(app.save_session());
        assert_eq!(buffer(&app, &note), "v1", "the box is not overwritten");
        assert_eq!(app.mirror.text(&note), "v1", "held");
        assert_eq!(app.held_text.get(&note).map(String::as_str), Some("theirs"));
        assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED), "their other field arrives");
        if typed {
            type_into(&mut app, &note, "mine");
        }
        app.blur_note(&note);
        let want = if typed { "mine" } else { "theirs" };
        assert_eq!((app.mirror.text(&note), buffer(&app, &note).as_str()), (want, want), "typed: {typed}");
        assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str(want.into()), "typed: {typed}");
        assert!(app.held_text.is_empty(), "typed: {typed}");
    }
}

/// Ruling 7, a commit between: text a save already committed is older than a remote change held after it, so with
/// nothing typed since, the held value applies at the blur. A remote change that arrives while a typed edit is
/// pending loses to it, as any pending edit wins.
#[test]
fn a_held_text_change_follows_the_order_of_the_commits() {
    let path = temp_dir("text-held-order").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    // Typed, then their change, then a save: the save queues ours first (`commit_note_buffers`), so theirs is held
    // behind that pending edit, never remembered in `held_text`, and ours commits over it.
    type_into(&mut app, &note, "mine");
    remote_set(&mut other, &note, field::TEXT, FieldValue::Str("before".into()));
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("mine".into()));
    assert!(app.held_text.is_empty(), "held behind the pending edit, not remembered");
    // Their change after that commit, nothing typed since: theirs applies at the blur.
    remote_set(&mut other, &note, field::TEXT, FieldValue::Str("after".into()));
    assert!(app.save_session());
    assert_eq!(buffer(&app, &note), "mine");
    app.blur_note(&note);
    assert_eq!((app.mirror.text(&note), buffer(&app, &note).as_str()), ("after", "after"));
    assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("after".into()));
}

/// Ruling 7: a remote text held while the note is typed in is older than a local commit that follows it, so that
/// commit drops it (`save_session`), and the blur keeps what was typed.
#[test]
fn a_held_remote_text_is_dropped_by_a_later_local_commit() {
    let path = temp_dir("text-held-dropped").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    type_into(&mut app, &note, "v1");
    remote_set(&mut other, &note, field::TEXT, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert_eq!(app.held_text.get(&note).map(String::as_str), Some("theirs"), "held, nothing typed yet");
    type_into(&mut app, &note, "mine");
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("mine".into()));
    assert!(app.held_text.is_empty(), "the commit came after theirs");
    app.blur_note(&note);
    assert_eq!((app.mirror.text(&note), buffer(&app, &note).as_str()), ("mine", "mine"));
    assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("mine".into()));
}

/// As before 2b-4: a note nobody is typing in shows another replica's text at once.
#[test]
fn a_remote_text_change_updates_an_idle_note() {
    let path = temp_dir("text-idle").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &note, field::TEXT, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert_eq!((app.mirror.text(&note), app.note_text(&note)), ("theirs", "theirs"));
    assert_eq!(buffer(&app, &note), "theirs");
    assert!(app.held_text.is_empty());
}

/// Ruling 6: a note can reach 1 MB, so the per-frame reader borrows: the mirror's own string when idle, the box's
/// buffer while typed in; neither is a copy.
#[test]
fn a_large_note_edit_does_not_clone_per_frame() {
    let mut app = App::default();
    let text = "x".repeat(1 << 20);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text, editing: false }));
    let idle: &str = app.note_text(&note);
    assert_eq!(idle.as_ptr(), app.mirror.text(&note).as_ptr(), "idle: the mirror's string");
    assert_eq!(app.mirror.text(&note).as_ptr(), app.mirror.text(&note).as_ptr(), "`Mirror::text` borrows");
    app.nodes.get_mut(&note).unwrap().as_note_mut().unwrap().editing = true;
    let editing: &str = app.note_text(&note);
    assert_eq!(editing.as_ptr(), app.nodes[&note].as_note().unwrap().text.as_ptr(), "editing: the buffer");
}

/// Ruling 9: the rename commit is the group name's writer (`edit_name`), and another replica's rename reaches the
/// mirror.
#[test]
fn a_rename_saves_and_a_remote_rename_reaches_the_mirror() {
    let path = temp_dir("name-rename").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::NAME), FieldValue::Str("G".into()), "a new group's Create carries its name");
    app.apply_node_action(&egui::Context::default(), &g, NodeAction::CommitRename("Errands".into()));
    assert_eq!(app.mirror.name(&g), "Errands", "the mirror shows it at once");
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::NAME), FieldValue::Str("Errands".into()));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &g, field::NAME, FieldValue::Str("Theirs".into()));
    assert!(app.save_session());
    assert_eq!(app.mirror.name(&g), "Theirs");
    assert_eq!(stored(&app, &g, field::NAME), FieldValue::Str("Theirs".into()), "not diffed back");
}

/// Ruling 9 with 2b-3 Ruling 3: a rename waiting for a save wins over another replica's.
#[test]
fn a_pending_local_rename_wins_over_a_remote_one() {
    let path = temp_dir("name-pending").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_name(&g, "mine");
    remote_set(&mut other, &g, field::NAME, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert_eq!(app.mirror.name(&g), "mine");
    assert_eq!(stored(&app, &g, field::NAME), FieldValue::Str("mine".into()));
}

/// A Json node `v.json` (`{"users": [...]}`) with a store id, in an app saving to `path`.
fn json_on(path: &std::path::Path, name: &str) -> (App, String) {
    let file = temp_dir(name).join("v.json");
    std::fs::write(&file, r#"{"users": [{"name": "a"}]}"#).unwrap();
    let mut app = app_on(path, 1);
    let node = app.alloc_node_id();
    app.insert_json_node_with_id(node.clone(), file.to_string_lossy().into_owned(), egui::Vec2::ZERO, None).unwrap();
    (app, node)
}

/// Ruling 8: a view edit (`edit_view`) saves, and the decoded cache follows every change of the mirror's view,
/// another replica's too.
#[test]
fn a_json_view_edit_saves_and_a_remote_one_refreshes_the_decoded_cache() {
    let path = temp_dir("view-edit").join("session.mnf");
    let (mut app, node) = json_on(&path, "view-edit-file");
    assert!(app.save_session());
    let users = r#"{"/users":{"collapsed":true}}"#;
    app.edit_view(&node, users);
    assert_eq!(app.mirror.view(&node).and_then(|v| v.get("/users")).and_then(|o| o.collapsed), Some(true));
    assert!(app.save_session());
    assert_eq!(stored(&app, &node, field::VIEW), FieldValue::Str(users.into()));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &node, field::VIEW, FieldValue::Str(r#"{"":{"collapsed":true}}"#.into()));
    assert!(app.save_session());
    let view = app.mirror.view(&node).expect("a view");
    assert_eq!(view.get("").and_then(|o| o.collapsed), Some(true), "the cache shows theirs");
    assert!(view.get("/users").is_none(), "and only theirs");
}

#[test]
fn url_changes_after_a_host_started_load_stay_local_until_it_settles() {
    let mut app = App::default();
    assert!(app.shares_url_change("7", None), "an ordinary tab shares");
    app.note_host_load("7");
    assert!(!app.shares_url_change("7", None), "a redirect right after the host's navigation");
    assert!(!app.shares_url_change("7", None), "still within the grace");
    app.host_loads.insert("7".into(), std::time::Instant::now() - HOST_LOAD_GRACE);
    assert!(app.shares_url_change("7", None), "settled: the next navigation is shared");
    assert!(app.host_loads.is_empty());
}

/// An engine that says how a navigation started overrides the grace.
#[test]
fn the_engines_cause_decides_whether_a_url_change_is_shared() {
    let mut app = App::default();
    assert!(!app.shares_url_change("7", Some(false)), "a script's navigation stays local");
    app.note_host_load("7");
    assert!(app.shares_url_change("7", Some(true)), "a click right after the host's load is shared");
    assert!(app.host_loads.is_empty(), "and ends the host's load");
}

/// Right after a typed navigation, a report still at the url it left
/// is from before the engine started it.
#[test]
fn a_report_still_at_the_url_a_typed_navigation_left_is_stale() {
    let mut app = App::default();
    let report = |app: &mut App, url: &str| app.reported_url("7", url.into(), true, "https://typed/".into());
    assert_eq!(report(&mut app, "https://old/"), "https://old/", "no typed navigation: as reported");
    app.navigated_from.insert("7".into(), ("https://old/".into(), std::time::Instant::now(), false));
    assert_eq!(report(&mut app, "https://old/"), "https://typed/");
    assert_eq!(report(&mut app, "https://typed/"), "https://typed/");
    assert_eq!(report(&mut app, "https://old/"), "https://old/", "after the engine moved on: real (going back)");
    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(2);
    app.navigated_from.insert("7".into(), ("https://old/".into(), long_ago, false));
    assert_eq!(report(&mut app, "https://old/"), "https://old/", "the navigation never started");
}

/// A typed navigation that never leaves the page (an attachment, a 204): CEF
/// reports it started and finished, both still at the old url, and nothing
/// after. The finish is the page's real url, not a stale report.
#[test]
fn a_typed_navigation_that_ends_at_the_url_it_left_keeps_that_url() {
    let mut app = App::default();
    let report =
        |app: &mut App, url: &str, loading: bool| app.reported_url("7", url.into(), loading, "https://typed/".into());
    app.navigated_from.insert("7".into(), ("https://old/".into(), std::time::Instant::now(), false));
    assert_eq!(report(&mut app, "https://old/", false), "https://typed/", "a finish from before it started");
    assert_eq!(report(&mut app, "https://old/", true), "https://typed/", "started, address not changed yet");
    assert_eq!(report(&mut app, "https://old/", false), "https://old/", "finished without leaving");
    assert!(app.navigated_from.is_empty());
}

// ---- 2b-4: the document pos in the mirror, the layout a side-table ----------

/// A note placed at `pos` (top level), as `place_node` places every new node.
fn a_note_at(app: &mut App, pos: egui::Vec2) -> String {
    let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
    app.place_node(None, Kind::Note, note, crate::fetched::Placement { group: None, pos })
}

/// What a drag does to the layout every frame (`card.rs`), then its release.
fn drag(app: &mut App, ids: &[&str], delta: egui::Vec2) {
    app.dragging = Some(ids[0].to_owned());
    app.drag_moved = ids.iter().map(|&id| id.to_owned()).collect();
    for &id in ids {
        app.layout.entry(id.to_owned()).or_default().pos += delta;
    }
}

/// Ruling 2: a drag release writes the document pos; physics (a settle) moves only the layout.
#[test]
fn a_drag_release_saves_the_dropped_position() {
    let path = temp_dir("pos-drag").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 10.0));
    let other = a_note_at(&mut app, egui::vec2(52.0, 10.0));
    assert!(app.save_session());
    drag(&mut app, &[&note], egui::vec2(40.0, 0.0));
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(10.0, 10.0), "a drag in progress is not saved");
    app.release_drag();
    assert_eq!(app.mirror.pos(&note), Some(egui::vec2(50.0, 10.0)), "shown at once");
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(50.0, 10.0));

    app.settle_current_level();
    assert_ne!(app.layout_pos(&note), egui::vec2(50.0, 10.0), "the overlapping cards were pushed apart");
    assert_ne!(app.layout_pos(&other), egui::vec2(52.0, 10.0));
    assert!(app.save_session());
    assert!(app.pending.is_empty());
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(50.0, 10.0), "physics never writes the document");
    assert_eq!(stored(&app, &other, field::POS), FieldValue::Pos(52.0, 10.0));
    assert_eq!(app.mirror.pos(&note), Some(egui::vec2(50.0, 10.0)));
}

/// Ruling 3: a new node's `Create` carries where it was placed, not where a settle moved it before the first save.
#[test]
fn a_node_is_created_at_its_placement_not_its_settled_position() {
    let path = temp_dir("pos-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
    let other = a_note_at(&mut app, egui::vec2(11.0, 20.0));
    assert_eq!(app.layout_pos(&note), egui::vec2(10.0, 20.0), "the layout starts at the placement too");
    app.settle_current_level();
    assert_ne!(app.layout_pos(&note), egui::vec2(10.0, 20.0));
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(10.0, 20.0));
    assert_eq!(stored(&app, &other, field::POS), FieldValue::Pos(11.0, 20.0));
}

/// Ruling 4: an unsaved local pos beats another replica's; their other fields still arrive, and the layout is not
/// snapped to theirs.
#[test]
fn a_local_pos_edit_wins_over_a_remote_one() {
    let path = temp_dir("pos-local-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    drag(&mut app, &[&note], egui::vec2(80.0, 0.0));
    app.release_drag();
    remote_set(&mut other, &note, field::POS, FieldValue::Pos(5.0, 6.0));
    remote_set(&mut other, &note, field::COLOR, red());
    // The remote round applies, then the next round commits.
    assert!(app.save_session());
    assert_eq!(app.mirror.pos(&note), Some(egui::vec2(90.0, 20.0)));
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(90.0, 20.0));
    assert_eq!(app.layout_pos(&note), egui::vec2(90.0, 20.0), "not snapped to theirs");
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED), "their other field arrives");
}

/// Ruling 4: another replica's pos reaches the mirror and snaps the layout, at rest; nothing is written back.
#[test]
fn a_remote_pos_snaps_the_layout_and_zeroes_its_velocity() {
    let path = temp_dir("pos-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
    assert!(app.save_session());
    app.layout.insert(note.clone(), crate::Layout { pos: egui::vec2(12.0, 22.0), vel: egui::vec2(3.0, 4.0) });
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &note, field::POS, FieldValue::Pos(5.0, 6.0));
    let pos_stamp = |doc: &Doc| doc.nodes[&note.parse().unwrap()].fields[&field::POS].stamp;
    let theirs = pos_stamp(other.doc());
    assert!(app.save_session());
    assert_eq!(app.mirror.pos(&note), Some(egui::vec2(5.0, 6.0)));
    assert_eq!(app.layout[&note], crate::Layout { pos: egui::vec2(5.0, 6.0), vel: egui::Vec2::ZERO });
    assert!(app.save_session());
    assert_eq!(pos_stamp(app.journal.as_ref().unwrap().doc()), theirs, "nothing written back");
}

/// Ruling 4: a card the user is dragging is not snapped by a remote pos; its release then wins.
#[test]
fn a_remote_pos_does_not_snap_a_node_being_dragged() {
    let path = temp_dir("pos-remote-drag").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (a_note_at(&mut app, egui::vec2(10.0, 20.0)), a_note_at(&mut app, egui::vec2(300.0, 20.0)));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    // `a` is dragged, `b` moves with it as part of the selection.
    drag(&mut app, &[&a, &b], egui::vec2(0.0, 50.0));
    remote_set(&mut other, &a, field::POS, FieldValue::Pos(5.0, 6.0));
    remote_set(&mut other, &b, field::POS, FieldValue::Pos(7.0, 8.0));
    assert!(app.save_session());
    assert_eq!(app.mirror.pos(&a), Some(egui::vec2(5.0, 6.0)), "the document pos is theirs until the release");
    assert_eq!(app.layout_pos(&a), egui::vec2(10.0, 70.0), "the dragged card stays under the pointer");
    assert_eq!(app.layout_pos(&b), egui::vec2(300.0, 70.0), "and so does the rest of the selection");
    app.release_drag();
    assert!(app.save_session());
    assert_eq!(stored(&app, &a, field::POS), FieldValue::Pos(10.0, 70.0), "the release wins");
    assert_eq!(stored(&app, &b, field::POS), FieldValue::Pos(300.0, 70.0));
}

/// A drag whose grabbed card stops being drawn ends with no drop: a remote pos then snaps its cards, and a later
/// release (the card drawn again) saves nothing over it.
#[test]
fn a_stale_drag_does_not_overwrite_a_remote_pos() {
    let path = temp_dir("pos-stale-drag").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let (a, b) = (a_note_at(&mut app, egui::vec2(10.0, 20.0)), a_note_at(&mut app, egui::vec2(300.0, 20.0)));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();

    // A level switch with the button held.
    drag(&mut app, &[&a, &b], egui::vec2(0.0, 50.0));
    app.switch_group_level(Some(g.clone()));
    assert!(app.dragging.is_none() && app.drag_moved.is_empty());
    remote_set(&mut other, &a, field::POS, FieldValue::Pos(5.0, 6.0));
    assert!(app.save_session());
    assert_eq!(app.layout_pos(&a), egui::vec2(5.0, 6.0), "no longer dragged, so the remote pos snaps it");
    app.switch_group_level(None);
    app.release_drag();
    assert!(app.save_session());
    assert_eq!(stored(&app, &a, field::POS), FieldValue::Pos(5.0, 6.0));
    assert_eq!(stored(&app, &b, field::POS), FieldValue::Pos(300.0, 20.0), "the ended drag saved nothing");

    // Another replica moves the grabbed card into another group mid-drag, and gives it a new pos.
    drag(&mut app, &[&a], egui::vec2(0.0, 50.0));
    let node = a.parse().unwrap();
    let pos = FieldValue::Pos(7.0, 8.0);
    let set = OpBody::Set { node, field: field::POS, enc: encode_field(field::POS, &pos) };
    other.begin().unwrap().commit(vec![OpBody::Move { node, group: Some(g.parse().unwrap()) }, set]).unwrap();
    assert!(app.save_session());
    assert_eq!(app.layout_pos(&a), egui::vec2(7.0, 8.0), "off the level, the drag no longer holds the remote pos back");
    app.end_stale_drag(true);
    assert!(app.dragging.is_none() && app.drag_moved.is_empty(), "the frame's check ends the drag");
    app.release_drag();
    assert!(app.save_session());
    assert_eq!(stored(&app, &a, field::POS), FieldValue::Pos(7.0, 8.0));

    // Leaving Graph view ends a drag too.
    drag(&mut app, &[&b], egui::vec2(0.0, 50.0));
    app.end_stale_drag(false);
    assert!(app.dragging.is_none() && app.drag_moved.is_empty());
}

/// Ruling 1: the layout table is keyed by id and outlives a respawn's remove-and-reinsert; `TabSpawn` carries no
/// position for it (this respawn starts at the origin), and a save mid-respawn keeps the entry.
#[test]
fn a_respawn_keeps_the_layout_position() {
    let path = temp_dir("pos-respawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.snap_layout(&tab, egui::vec2(30.0, 40.0));
    assert!(app.save_session());
    start_respawn(&mut app, &tab);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(app.nodes.contains_key(&tab), "landed as a failed card");
    assert_eq!(app.layout_pos(&tab), egui::vec2(30.0, 40.0));
}

/// A new tab has no layout entry or mirror entry until it lands: it lands at its spawn position, which is its
/// document pos too (Ruling 3).
#[test]
fn a_new_tab_lands_at_its_spawn_position_in_the_layout_and_the_document() {
    let path = temp_dir("pos-new-spawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = app.alloc_node_id();
    let at = egui::vec2(30.0, 40.0);
    app.spawn_tab_async(crate::TabSpawn::new(
        t.clone(),
        "https://t/".into(),
        EngineKind::Servo,
        at,
        SpawnPurpose::Restore,
        1.0,
    ));
    assert!(app.save_session(), "creates the spawning tab");
    assert_eq!(stored(&app, &t, field::POS), FieldValue::Pos(30.0, 40.0));
    fail_spawns(&mut app);
    assert_eq!(app.layout_pos(&t), at);
    assert_eq!(app.mirror.pos(&t), Some(at));
}

/// Ruling 4, restore: the mirror holds the document pos and the layout the personal one, and the next save writes
/// nothing for it.
#[test]
fn a_restart_places_a_node_at_its_personal_position_while_the_mirror_holds_the_document_one() {
    let path = temp_dir("pos-restore").join("session.mnf");
    let note = {
        let mut app = app_on(&path, 1);
        let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
        assert!(app.save_session());
        app.snap_layout(&note, egui::vec2(70.0, 80.0)); // a settle: personal
        app.close_store();
        note
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.layout_pos(&note), egui::vec2(70.0, 80.0));
    assert_eq!(app.mirror.pos(&note), Some(egui::vec2(10.0, 20.0)));
    assert!(app.save_session());
    assert!(app.pending.is_empty());
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(10.0, 20.0));
}

#[test]
fn a_read_only_store_saves_nothing() {
    let path = temp_dir("sync-readonly").join("session.mnf");
    drop(Journal::open(&path, ActorId(1)).unwrap());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[4] = 3; // a newer format version, as in `journal`'s own test
    let crc = crate::store::crc32c::crc32c(&bytes[..21]);
    bytes[21..25].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let mut app = app_on(&path, 2);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(!app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().live().count(), 0);
}

#[test]
fn engine_table_matches_the_store_and_round_trips() {
    for (i, kind) in EngineKind::ALL.into_iter().enumerate() {
        let desc = kind.desc();
        assert_eq!(desc.code, i as u64 + 1, "codes are 1-based positions in ALL");
        assert_eq!(crate::store::model::ENGINES[i], desc.name, "the store names the same engine");
        assert_eq!(serde_json::to_value(Wrap(kind)).unwrap(), serde_json::json!(desc.name));
        assert_eq!(engine_of(engine_code(kind)), kind);
        assert_eq!(EngineKind::from_name(&desc.name.to_uppercase()), Some(kind));
    }
    assert_eq!(EngineKind::from_name("gecko"), None);
    assert_eq!(EngineKind::from_code(0), None);
}

#[derive(serde::Serialize)]
struct Wrap(#[serde(with = "crate::process_manager::engine_kind_serde")] EngineKind);

#[test]
fn colors_round_trip() {
    let c = egui::Color32::from_rgb(1, 2, 3);
    assert_eq!(color_of(&color_value(Some(c))), Some(c));
    assert_eq!(color_value(Some(egui::Color32::BLACK)), FieldValue::Color(Some(0)), "black is encoded");
    assert_eq!(color_of(&color_value(None)), None);
}

/// Item view (`view_item`) restoring onto a tab that's still spawning
/// (a tab's restore is always async) must not bounce back to Graph the
/// first frame -- `build_tab_view_ui` (tab_view.rs) does exactly that
/// for any Item view whose focused id isn't a live tab yet, unless
/// `pending_respawn_focus` names it (the same guard a crash respawn
/// uses). `apply_local` sets it for this case; this only re-checks the
/// state it leaves behind, since the actual bounce-back is UI code this
/// module can't drive without a window.
#[test]
fn item_view_restoring_onto_a_still_spawning_tab_sets_the_respawn_focus_guard() {
    let path = crate::store::testutil::temp_dir("view-item-spawning").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://x/", 1.0));
    // A node whose id isn't live yet -- `spawning`, not `self.nodes` --
    // is exactly the shape a real tab restore has before its spawn
    // lands (`materialize`'s `Kind::Tab` arm never inserts directly).
    app.nodes.remove(&tab);
    app.spawning.insert(tab.clone(), shared(Kind::Tab, None, &[]));
    let state = LocalState { focused: Some(tab.clone()), view_item: true, ..LocalState::default() };
    app.apply_local(state);
    assert_eq!(app.view_mode, ViewMode::Item);
    assert_eq!(app.pending_respawn_focus.as_deref(), Some(tab.as_str()));
}

/// A launch that changes nothing (no user action at all) must rewrite
/// neither the session nor its personal state -- otherwise every launch
/// would touch both files even when nothing happened. `insert_note_at`
/// (main.rs) is private to that module and also saves as a side effect,
/// so this uses the `add` helper and sets the settled position directly,
/// same as Task 5's own no-window tests.
#[test]
fn an_idle_open_and_close_writes_nothing() {
    let dir = crate::store::testutil::temp_dir("idle-app");
    let path = dir.join("session.mnf");
    {
        let mut app = app_on(&path, 1);
        let id = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: String::new(), editing: false }));
        app.snap_layout(&id, egui::vec2(3.0, 4.0));
        app.close_store();
    }
    let before = (std::fs::read(&path).unwrap(), std::fs::read(crate::store::local::path_for(&path)).unwrap());
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    app.close_store();
    let after = (std::fs::read(&path).unwrap(), std::fs::read(crate::store::local::path_for(&path)).unwrap());
    assert!(before == after, "an idle launch rewrote the session or its personal state");
}

/// A note made from a web page keeps its URL through save and restore
/// (content dispatch spec): its snapshot reports it under `field::URL`
/// and `restore_session` hands it back via `App::source_url`.
#[test]
fn content_nodes_keep_their_source_url() {
    let path = crate::store::testutil::temp_dir("content-url").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = app.alloc_node_id();
    let payload = NotePayload { text: "hi".into(), editing: false };
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, Kind::Note, Runtime::Note(payload));
    app.mirror.set(&id, field::URL, FieldValue::Str("https://x.test/a.txt".into()));
    let shared = snapshots(&app);
    let fields = &shared[&id.parse().unwrap()].fields;
    assert_eq!(fields.get(&field::URL), Some(&FieldValue::Str("https://x.test/a.txt".into())));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.source_url(&id), Some("https://x.test/a.txt"));
}

/// A finished download keeps the address its engine fetched it from across a restart.
#[test]
fn a_completed_download_keeps_its_url() {
    let path = crate::store::testutil::temp_dir("download-url").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = app.alloc_node_id();
    let payload =
        DownloadPayload { path: "f.bin".into(), bytes_received: 0, total_bytes: None, completed: true, failed: false };
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, Kind::Download, Runtime::Download(payload));
    app.mirror.set(&id, field::URL, FieldValue::Str("https://x.test/f.bin".into()));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.copyable_url(&id), Some("https://x.test/f.bin"));
    assert_eq!(restored.source_url(&id), None, "a download has no web source to refetch");
}

/// A download saved before cards kept their URL (or by an engine that can't say) restores with
/// none, so its card falls back to showing the file's path.
#[test]
fn a_download_without_a_url_restores_without_one() {
    let path = crate::store::testutil::temp_dir("download-no-url").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = app.alloc_node_id();
    let payload =
        DownloadPayload { path: "f.bin".into(), bytes_received: 0, total_bytes: None, completed: true, failed: false };
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, Kind::Download, Runtime::Download(payload));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert!(restored.nodes[&id].as_download().is_some());
    assert_eq!(restored.copyable_url(&id), None);
}

/// A download's "opened from" link (the tab it came from) and a content node's (the tab it was) survive a
/// restart, like a tab's.
#[test]
fn an_opened_from_link_survives_a_restart_on_every_kind_that_has_one() {
    let path = temp_dir("parent-kinds").join("session.mnf");
    let mut app = app_on(&path, 1);
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
    let download =
        DownloadPayload { path: "f.bin".into(), bytes_received: 0, total_bytes: None, completed: true, failed: false };
    let download = add(&mut app, Kind::Download, Runtime::Download(download));
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "hi".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/child", "Child"));
    for id in [&download, &note, &tab] {
        app.edit_parent(id, Some(opener.clone()));
    }
    assert!(app.save_session());
    assert_eq!(stored(&app, &download, field::PARENT), FieldValue::Id(Some(opener.parse().unwrap())));
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    for id in [&download, &note, &tab] {
        assert_eq!(restored.mirror.parent(id), Some(opener.as_str()), "{id}");
    }
    // Another replica relinking a download applies here too.
    let mut other = Journal::open(&path, ActorId(3)).unwrap();
    remote_set(&mut other, &download, field::PARENT, FieldValue::Id(Some(tab.parse().unwrap())));
    assert!(restored.save_session());
    assert_eq!(restored.mirror.parent(&download), Some(tab.as_str()));
}

/// Image, PDF and JSON cards keep their "opened from" link across a restart, and so does a tab opened from a
/// content node (Open as Tab).
#[test]
fn an_opened_from_link_survives_a_restart_on_file_cards_and_a_tab_opened_from_one() {
    let dir = temp_dir("parent-files");
    let path = dir.join("session.mnf");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(1, 1).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png_path = dir.join("a.png");
    std::fs::write(&png_path, png.into_inner()).unwrap();
    let json_path = dir.join("a.json");
    std::fs::write(&json_path, b"[1]").unwrap();
    let file = |p: &std::path::Path| p.to_string_lossy().into_owned();

    let mut app = app_on(&path, 1);
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
    let (image, pdf, json) = (app.alloc_node_id(), app.alloc_node_id(), app.alloc_node_id());
    app.insert_image_node_with_id(image.clone(), file(&png_path), egui::Vec2::ZERO, None).unwrap();
    app.insert_pdf_node_with_id(pdf.clone(), file(&dir.join("a.pdf")), egui::Vec2::ZERO, None);
    app.insert_json_node_with_id(json.clone(), file(&json_path), egui::Vec2::ZERO, None).unwrap();
    for id in [&image, &pdf, &json] {
        app.edit_parent(id, Some(opener.clone()));
    }
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.png", "From image"));
    app.edit_parent(&tab, Some(image.clone()));
    assert!(app.save_session());
    app.close_store();

    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    for id in [&image, &pdf, &json] {
        assert_eq!(restored.mirror.parent(id), Some(opener.as_str()), "{id}");
    }
    assert!(restored.nodes[&image].as_image().is_some() && restored.nodes[&json].as_json().is_some());
    assert!(restored.nodes[&pdf].as_pdf().is_some());
    assert_eq!(restored.mirror.parent(&tab), Some(image.as_str()), "a tab opened from an image card");
}

/// A session saved by a build that wrote no `parent` for a download loads unchanged: no link, nothing lost.
#[test]
fn an_older_sessions_download_without_a_parent_loads_unchanged() {
    let path = temp_dir("parent-old").join("session.mnf");
    let node = NodeId { actor: ActorId(2), counter: 1 };
    {
        let mut old = Journal::open(&path, ActorId(2)).unwrap();
        let fields = [
            (field::GROUP, FieldValue::Id(None)),
            (field::POS, FieldValue::Pos(1.0, 2.0)),
            (field::COLOR, FieldValue::Color(None)),
            (field::URL, FieldValue::Str("https://x.test/f.bin".into())),
            (field::PATH, FieldValue::Str("f.bin".into())),
        ]
        .iter()
        .flat_map(|(no, value)| encode_field(*no, value))
        .collect();
        old.begin().unwrap().commit(vec![OpBody::create(node, Kind::Download, fields)]).unwrap();
    }
    let mut app = app_on(&path, 1);
    assert!(app.restore_session());
    let restored = &app.nodes[&node.to_string()];
    assert_eq!(app.mirror.parent(&node.to_string()), None);
    assert!(restored.as_download().is_some());
    assert_eq!(app.copyable_url(&node.to_string()), Some("https://x.test/f.bin"));
    assert_eq!(app.layout_pos(&node.to_string()), egui::vec2(1.0, 2.0));
    assert_eq!(app.mirror.pos(&node.to_string()), Some(egui::vec2(1.0, 2.0)));
    // No link encodes as the absent register, so a save has nothing to write back.
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "nothing written");
}

/// A group export round-trips as a JSON snapshot: `export_group_json`
/// gives the group and its one member, root first, and `import_group_
/// json` recreates both under fresh ids, leaving the originals in
/// place (so the level now holds four nodes, not two).
#[test]
fn a_group_export_imports_as_fresh_nodes() {
    let dir = crate::store::testutil::temp_dir("group-json");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    let g = app.alloc_node_id();
    app.insert_group_at(g.clone(), Some(egui::vec2(0.0, 0.0)));
    let n = app.alloc_node_id();
    app.insert_note_at(n.clone(), Some(egui::vec2(10.0, 0.0)));
    app.edit_group(&n, Some(g.clone()));
    let json = app.export_group_json(&g).unwrap();
    assert_eq!(json.nodes.len(), 2);
    assert_eq!(json.nodes[0].kind(), Some(Kind::Group));
    app.import_group_json(&json);
    assert_eq!(snapshots(&app).len(), 4, "the import added a second group and note");
}

fn failed_tab(url: &str, zoom_factor: f64) -> Runtime {
    Runtime::Unloaded(UnloadedTabPayload {
        url: url.into(),
        title: String::new(),
        muted: false,
        zoom_factor,
        state: UnloadedState::Failed("x".into()),
    })
}

/// A tab restored asleep, as `add` inserts it.
pub(crate) fn sleeping_tab(url: &str, title: &str) -> Runtime {
    Runtime::Unloaded(UnloadedTabPayload {
        url: url.into(),
        title: title.into(),
        muted: false,
        zoom_factor: 1.0,
        state: UnloadedState::Sleeping,
    })
}

/// An unloaded tab's title and mute survive a restart through personal
/// state, though it has no engine to report either.
#[test]
fn personal_state_keeps_an_unloaded_tabs_title_and_mute() {
    let mut app = app_on(&temp_dir("unloaded-local").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", "Saved title"));
    let set = |app: &mut App, title: &str, muted: bool| {
        if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
            (t.title, t.muted) = (title.to_string(), muted);
        }
    };
    set(&mut app, "Saved title", true);
    let entry = app.local_state().nodes[&tab].clone();
    assert_eq!((entry.title.as_str(), entry.muted), ("Saved title", true));
    set(&mut app, "", false);
    App::apply_local_node(app.nodes.get_mut(&tab).unwrap(), &entry);
    let t = app.nodes[&tab].as_unloaded().unwrap();
    assert_eq!((t.title.as_str(), t.muted), ("Saved title", true));
}

/// Closing an unloaded card moves its children to its parent, as
/// closing a live tab does, instead of leaving them naming a node
/// that's gone.
#[test]
fn closing_an_unloaded_tab_reparents_its_children() {
    let mut app = app_on(&temp_dir("unloaded-close").join("session.mnf"), 1);
    let root = add(&mut app, Kind::Tab, sleeping_tab("https://root/", ""));
    let mid = add(&mut app, Kind::Tab, failed_tab("https://mid/", 1.0));
    let leaf = add(&mut app, Kind::Tab, sleeping_tab("https://leaf/", ""));
    app.edit_parent(&mid, Some(root.clone()));
    app.edit_parent(&leaf, Some(mid.clone()));
    app.close_node(&mid);
    assert!(!app.nodes.contains_key(&mid));
    assert_eq!(app.mirror.parent(&leaf), Some(root.as_str()));
}

/// A restore starts only the focused tab; every other tab is a
/// sleeping card with its saved title.
#[test]
fn a_restore_starts_only_the_focused_tab() {
    let path = temp_dir("lazy-restore").join("session.mnf");
    let (a, b, c) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        let c = add(&mut app, Kind::Tab, sleeping_tab("https://c/", "C"));
        app.focused = Some(b.clone());
        assert!(app.save_session());
        app.maybe_write_local(true);
        app.close_store();
        (a, b, c)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    let spawned: Vec<&str> = app.test_spawns.iter().map(|(s, _)| s.id.as_str()).collect();
    assert_eq!(spawned, [b.as_str()]);
    assert!(matches!(app.test_spawns[0].0.purpose, SpawnPurpose::Wake));
    for (id, title) in [(&a, "A"), (&c, "C")] {
        let t = app.nodes[id].as_unloaded().unwrap();
        assert_eq!((t.state.clone(), t.title.as_str()), (UnloadedState::Sleeping, title));
    }
    assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Waking);
}

/// A restored split session wakes both halves -- the focused tab and
/// the saved split tab -- leaving every other tab asleep (Fix 2,
/// final-review: `apply_local` restores `split_tab` even though only
/// `wake_focused` used to actually start an engine, leaving the split
/// pane empty).
#[test]
fn restoring_a_split_session_wakes_both_the_focused_and_split_tab() {
    let path = temp_dir("lazy-restore-split").join("session.mnf");
    let (a, b, c) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        let c = add(&mut app, Kind::Tab, sleeping_tab("https://c/", "C"));
        app.focused = Some(a.clone());
        app.split_tab = Some(b.clone());
        assert!(app.save_session());
        app.maybe_write_local(true);
        app.close_store();
        (a, b, c)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    let mut spawned: Vec<&str> = app.test_spawns.iter().map(|(s, _)| s.id.as_str()).collect();
    spawned.sort();
    let mut expected = [a.as_str(), b.as_str()];
    expected.sort();
    assert_eq!(spawned, expected);
    assert!(app.test_spawns.iter().all(|(s, _)| matches!(s.purpose, SpawnPurpose::Wake)));
    assert_eq!(app.nodes[&c].as_unloaded().unwrap().state, UnloadedState::Sleeping);
}

/// Opening a sleeping tab, by any path, wakes it once: focused, in Tab
/// view (showing the respawning placeholder), one spawn. A second open
/// while it wakes issues none.
#[test]
fn opening_a_sleeping_tab_wakes_it_once() {
    let mut app = app_on(&temp_dir("lazy-open").join("session.mnf"), 1);
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "Alpha"));
    let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "Beta"));
    app.focus_item(&a);
    app.focus_item(&a);
    assert_eq!(app.test_spawns.len(), 1);
    assert_eq!(app.focused.as_deref(), Some(a.as_str()));
    assert_eq!(app.view_mode, ViewMode::Item);
    assert_eq!(app.pending_respawn_focus.as_deref(), Some(a.as_str()));
    app.focus_tab(&b);
    assert_eq!(app.test_spawns.len(), 2);
    assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Waking);
}

/// Tab lists (Quick Switcher, Ctrl+Tab, graph navigation, Close All,
/// pinning) include sleeping tabs, and opening one from them wakes it.
#[test]
fn tab_lists_include_sleeping_tabs() {
    let mut app = app_on(&temp_dir("lazy-lists").join("session.mnf"), 1);
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "Alpha"));
    let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "Beta"));
    assert_eq!(app.quick_switcher_matches("beta"), std::slice::from_ref(&b));
    app.quick_switcher = Some(crate::QuickSwitcher { query: "beta".into(), selected: 0 });
    app.quick_switcher_confirm();
    assert_eq!(app.focused.as_deref(), Some(b.as_str()));
    assert_eq!(app.test_spawns.len(), 1);
    app.cycle_focused_tab(true);
    assert_eq!(app.focused.as_deref(), Some(a.as_str()));
    assert_eq!(app.test_spawns.len(), 2);
    assert_eq!(app.tabs_at_current_level(), [a.clone(), b.clone()]);
    app.set_all_pinned(true);
    assert!(app.mirror.pinned(&a) && app.mirror.pinned(&b));
    app.close_all_at_current_level();
    assert!(app.nodes.is_empty());
}

/// A failed wake leaves a failed card, still focused, and ends the
/// respawning placeholder (Tab view then falls back to Graph).
#[test]
fn a_failed_wake_leaves_a_failed_card() {
    let mut app = app_on(&temp_dir("lazy-fail").join("session.mnf"), 1);
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    app.focus_tab(&a);
    fail_spawns(&mut app);
    assert!(matches!(app.nodes[&a].as_unloaded().unwrap().state, UnloadedState::Failed(_)));
    assert_eq!(app.nodes[&a].as_unloaded().unwrap().title, "A", "the wake carried its title");
    assert_eq!(app.pending_respawn_focus, None);
    assert!(app.wakes_in_flight.is_empty());
}

/// Closing a tab while its wake is in flight deletes it from the store
/// once the (now-moot) spawn result lands -- the landing's "closed
/// while waking" branch used to return without saving, and a save
/// while the wake was in flight kept the id anyway (`spawning`), so it
/// lingered in the store forever (Fix 3, final-review).
#[test]
fn closing_a_tab_while_it_wakes_deletes_it_from_the_store() {
    let path = temp_dir("close-waking").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    assert!(app.save_session());
    app.focus_tab(&tab);
    assert_eq!(app.test_spawns.len(), 1, "the wake is in flight, parked");
    assert!(app.spawning.contains_key(&tab));
    app.destroy_tab(&tab);
    assert!(!app.nodes.contains_key(&tab), "closed");
    // Deliver the parked wake's result -- a failed one, the only kind a
    // unit test can produce (no fake `EngineTab` for a success).
    fail_spawns(&mut app);
    let still_live = app.journal.as_ref().unwrap().doc().live().any(|(id, _)| id.to_string() == tab);
    assert!(!still_live, "should have been deleted once the wake landed on a closed tab");
}

/// Closing a waking tab clears `pending_respawn_focus` when it names
/// the removed id -- otherwise it keeps pointing at a gone node, which
/// `build_tab_view_ui`'s guard reads to decide whether Item view stays
/// put during a respawn (Fix 6, final-review).
#[test]
fn removing_a_tab_clears_its_stale_pending_respawn_focus() {
    let mut app = app_on(&temp_dir("remove-pending-focus").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    app.focus_tab(&tab);
    assert_eq!(app.pending_respawn_focus.as_deref(), Some(tab.as_str()));
    app.destroy_tab(&tab);
    assert_eq!(app.pending_respawn_focus, None);
}

/// "Mute Others" mutes a sleeping or waking card too -- it has no
/// engine to send `SetMuted` to, but the landing already sends it
/// (`TabCarry::muted`) and personal state saves it either way, so a
/// woken tab comes back muted like every other tab "Mute Others" hit
/// (Fix 8, final-review).
#[test]
fn mute_all_except_also_mutes_sleeping_tabs() {
    let mut app = app_on(&temp_dir("mute-others-sleeping").join("session.mnf"), 1);
    let keep = add(&mut app, Kind::Tab, sleeping_tab("https://keep/", "Keep"));
    let other = add(&mut app, Kind::Tab, sleeping_tab("https://other/", "Other"));
    app.mute_all_except(&keep);
    assert!(!app.nodes[&keep].as_unloaded().unwrap().muted, "kept tab untouched");
    assert!(app.nodes[&other].as_unloaded().unwrap().muted);
    let entry = app.local_state().nodes[&other].clone();
    assert!(entry.muted, "personal state saves the sleeping card's mute");
}

/// Another replica's url change moves a sleeping tab without waking it.
#[test]
fn a_remote_url_change_moves_a_sleeping_tab_without_waking_it() {
    let path = temp_dir("lazy-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", "S"));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::URL, FieldValue::Str("https://moved/".into()));
    assert!(app.save_session());
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://moved/");
    assert!(app.test_spawns.is_empty());
}

/// A tab another replica creates arrives asleep at its url, so another
/// instance opening a big session starts no engines here.
#[test]
fn a_remote_new_tab_arrives_asleep() {
    let url = FieldValue::Str("https://new/".into());
    let app = app_with(&BTreeMap::from([(id(1), shared(Kind::Tab, None, &[(field::URL, url)]))]));
    assert!(app.test_spawns.is_empty());
    let tab = app.nodes[&id(1).to_string()].as_unloaded().unwrap();
    assert_eq!((tab.url.as_str(), &tab.state), ("https://new/", &UnloadedState::Sleeping));
}

/// Saving a sleeping tab writes what its live tab would: a restore
/// followed by a save changes nothing in the store.
#[test]
fn a_sleeping_tab_saves_as_its_tab() {
    let path = temp_dir("lazy-save").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
        app.edit_field(&tab, field::PINNED, FieldValue::Bool(true));
        assert!(app.save_session());
        app.close_store();
        tab
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    let engine = stored(&app, &tab, field::ENGINE);
    // The single tab is the focused one, so the restore woke it; fail
    // that wake too: a waking and a failed card save the same way.
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://s/".into()));
    assert_eq!(stored(&app, &tab, field::PINNED), FieldValue::Bool(true));
    assert_eq!(stored(&app, &tab, field::ENGINE), engine);
}

/// Fails every spawn parked in `test_spawns`, as a missing engine would.
fn fail_spawns(app: &mut App) {
    for parked in std::mem::take(&mut app.test_spawns) {
        fail_spawn(app, parked);
    }
}

fn fail_spawn(app: &mut App, (spawn, generation): (crate::TabSpawn, u64)) {
    let result = Err(std::io::Error::other("no engine").into());
    app.handle_tab_spawn_result(crate::TabSpawnOutcome {
        spawn,
        generation,
        result,
        seeded: Default::default(),
        jar: Default::default(),
        seed_clears: None,
    });
}

/// Starts a New spawn for `url` and lands it on a fake host; returns the new tab's id.
fn succeed_spawn(app: &mut App, url: &str) -> String {
    let id = app.spawn_tab_at(url, egui::Vec2::ZERO, None, None);
    let at = app.test_spawns.iter().position(|(spawn, _)| spawn.id == id).expect("the spawn is parked");
    let (spawn, generation) = app.test_spawns.remove(at);
    let (tab, _peer) = crate::process_manager::EngineTab::fake(spawn.kind);
    app.handle_tab_spawn_result(crate::TabSpawnOutcome {
        spawn,
        generation,
        result: Ok(tab),
        seeded: Default::default(),
        jar: Default::default(),
        seed_clears: None,
    });
    id
}

fn failed_cards(app: &App) -> Vec<String> {
    let mut ids: Vec<String> = app
        .nodes
        .iter()
        .filter(|(_, node)| node.as_unloaded().is_some_and(|tab| matches!(tab.state, UnloadedState::Failed(_))))
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// The same intent failing again (same url, engine, group) refreshes the one card instead of adding another; a
/// different url is a different intent.
#[test]
fn a_repeat_failure_refreshes_one_card_and_a_new_url_adds_one() {
    let mut app = app_on(&temp_dir("failed-twin").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1, "the same intent is one card");
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.live().count(), 1, "and one document node");
    app.spawn_tab_at("https://b/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2, "another url is another intent");
}

/// A group is part of the intent: the same url failing in two groups is two cards.
#[test]
fn a_failed_twin_in_another_group_is_not_a_twin() {
    let mut app = app_on(&temp_dir("failed-twin-group").join("session.mnf"), 1);
    let group = add_group(&mut app, "G");
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, Some(group), None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2);
}

/// A tab that lands clears the failed cards for its url, and only those.
#[test]
fn a_successful_spawn_clears_failed_cards_for_its_url() {
    let mut app = app_on(&temp_dir("failed-cleared").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    app.spawn_tab_at("https://b/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 2);
    let landed = succeed_spawn(&mut app, "https://a/");
    let left = failed_cards(&app);
    assert_eq!(left.len(), 1, "a/'s failed card is gone, b/'s stays");
    assert!(app.nodes[&left[0]].as_unloaded().unwrap().url == "https://b/");
    assert!(app.nodes.contains_key(&landed));
    assert_eq!(app.closed.len(), 1, "the cleared card is recorded in Recently Closed first, so it can come back");
}

/// The recorded entry is restorable: reopening it brings the cleared card back as a sleeping tab at its url.
#[test]
fn a_cleared_failed_card_reopens_from_recently_closed() {
    let mut app = app_on(&temp_dir("failed-cleared-reopen").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    let card = failed_cards(&app).remove(0);
    let landed = succeed_spawn(&mut app, "https://a/");
    assert!(!app.nodes.contains_key(&card), "the card is cleared");
    let entry = app.closed.pop().expect("the cleared card was recorded");
    app.reopen_closed(entry);
    let back: Vec<&String> = app
        .nodes
        .iter()
        .filter(|(id, n)| *id != &landed && n.as_unloaded().is_some_and(|t| t.url == "https://a/"))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(back.len(), 1, "one sleeping card for a/ is back beside the landed tab");
}

/// The group is part of the intent: a failed card for the same url in another group stays when a tab lands.
#[test]
fn a_successful_spawn_leaves_a_same_url_failed_card_in_another_group() {
    let mut app = app_on(&temp_dir("failed-kept-other-group").join("session.mnf"), 1);
    let group = add_group(&mut app, "G");
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, Some(group), None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    let landed = succeed_spawn(&mut app, "https://a/");
    assert_eq!(failed_cards(&app).len(), 1, "the card in group G is another intent");
    assert!(app.nodes.contains_key(&landed));
}

/// The clear queues its deletes without a save, so another replica's delete parked for the landing tab
/// (`remote_in_flight`) still lands before the first save of the landing.
#[test]
fn clearing_failed_cards_does_not_save_before_a_parked_remote_delete_lands() {
    let path = temp_dir("failed-cleared-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    let card = failed_cards(&app).remove(0);
    let id = app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    // The failed twin is not refreshed here: the spawn is still in flight, so it is its own node.
    assert!(app.save_session());
    assert!(app.journal.as_ref().unwrap().doc().nodes.contains_key(&id.parse().unwrap()), "the spawn is committed");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: id.parse().unwrap() }]).unwrap();
    assert!(app.save_session());
    assert!(app.remote_in_flight.contains_key(&id), "the remote delete is parked");

    let at = app.test_spawns.iter().position(|(spawn, _)| spawn.id == id).expect("the spawn is parked");
    let (spawn, generation) = app.test_spawns.remove(at);
    let (tab, _peer) = crate::process_manager::EngineTab::fake(spawn.kind);
    app.handle_tab_spawn_result(crate::TabSpawnOutcome {
        spawn,
        generation,
        result: Ok(tab),
        seeded: Default::default(),
        jar: Default::default(),
        seed_clears: None,
    });
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert!(!app.nodes.contains_key(&id), "the remotely deleted landing is gone");
    assert!(!app.nodes.contains_key(&card), "the failed card was cleared");
    assert_eq!(doc.live().count(), 0, "the document agrees");
}

/// A respawn lands an existing tab: it is no new intent, so a same-url failed card stays.
#[test]
fn a_respawn_landing_leaves_a_same_url_failed_card() {
    let mut app = app_on(&temp_dir("failed-kept-on-respawn").join("session.mnf"), 1);
    let (tab, _peer) = live_tab(&mut app, EngineKind::Servo, "https://a/");
    assert!(app.save_session());
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    app.handle_engine_message(&tab, ipc_protocol::EngineToHost::Crashed { reason: "boom".into() });
    assert!(app.test_spawns.iter().any(|(s, _)| matches!(s.purpose, SpawnPurpose::Respawn)), "restarting");
    let _peers = land_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1, "the failed card stays");
    assert!(app.save_session());
}

/// A repeat Root failure refreshes the card and still shows Graph view.
#[test]
fn a_repeat_root_failure_still_shows_graph_view() {
    let mut app = app_on(&temp_dir("failed-root-twin").join("session.mnf"), 1);
    app.spawn_root_tab();
    fail_spawns(&mut app);
    app.view_mode = ViewMode::Item;
    app.spawn_root_tab();
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1);
    assert!(matches!(app.view_mode, ViewMode::Graph));
}

fn json_node(id: &str, kind: Kind, fields: serde_json::Value) -> crate::store::json::JsonNode {
    let serde_json::Value::Object(mut fields) = fields else { panic!("fields are an object") };
    fields.insert("type".into(), kind.type_str().expect("a known kind").into());
    crate::store::json::JsonNode { id: id.into(), fields }
}

#[test]
fn a_jar_def_is_saved_as_a_jar_node_and_survives_later_saves() {
    let dir = crate::store::testutil::temp_dir("jar-def-saved");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    app.save_session();
    let doc = app.journal.as_ref().unwrap().doc();
    let node = doc.nodes.get(&jar).filter(|n| n.is_live()).expect("jar node created");
    assert_eq!(node.kind(), Kind::Jar);
    assert_eq!(fields_of(node).get(&field::NAME), Some(&FieldValue::Str("Work".into())));
    app.save_session();
    assert!(app.journal.as_ref().unwrap().doc().nodes[&jar].is_live(), "a second save must not delete it");
    assert!(!app.nodes.contains_key(&jar.to_string()), "a jar never enters the graph");
}

#[test]
fn remote_jar_create_rename_and_delete_reach_jar_defs() {
    let jar = id(40);
    let named = |name: &str| shared(Kind::Jar, None, &[(field::NAME, FieldValue::Str(name.into()))]);
    let mut app = app_with(&BTreeMap::from([(jar, named("Home"))]));
    assert_eq!(app.mirror.jar_defs()[&jar].name, "Home");
    assert!(!app.nodes.contains_key(&jar.to_string()), "never a graph node");
    app.apply_remote(vec![(jar, Some((Kind::Jar, named("House").fields, None)))]);
    assert_eq!(app.mirror.jar_defs()[&jar].name, "House");
    assert_eq!(snapshots(&app)[&jar], named("House"), "projects back unchanged");
    app.apply_remote(vec![(jar, None)]);
    assert!(!app.mirror.jar_defs().contains_key(&jar));
}

#[test]
fn a_session_with_only_jars_still_restores_them() {
    let dir = crate::store::testutil::temp_dir("jar-only-restore");
    let path = dir.join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    app.save_session();
    app.remove_tab(&tab);
    app.save_session();
    drop(app);
    let mut back = app_on(&path, 3);
    assert!(!back.restore_session(), "no graph node: a fresh start");
    assert_eq!(back.mirror.jar_defs()[&jar].name, "Work", "but the jar is kept, so the next save keeps it");
    back.save_session();
    assert!(back.journal.as_ref().unwrap().doc().nodes[&jar].is_live());
}

/// Final review M1: a jar deleted here whose delete is not saved yet must not come back (nameless, from an
/// ephemeral-only change, or whole) when the save's first round applies another device's edit to it.
#[test]
fn a_remote_edit_does_not_bring_back_a_jar_deleted_here() {
    for (n, (no, value)) in [(field::EPHEMERAL, FieldValue::Bool(true)), (field::NAME, FieldValue::Str("Job".into()))]
        .into_iter()
        .enumerate()
    {
        let path = crate::store::testutil::temp_dir(&format!("jar-deleted-here-{n}")).join("session.mnf");
        let mut app = app_on(&path, 1);
        add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
        let jar = app.create_jar("Work");
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        remote_set(&mut other, &jar.to_string(), no, value);
        app.delete_jar(jar);
        assert!(app.save_session());
        assert!(!app.mirror.jar_defs().contains_key(&jar), "field {no}: deleted here, not recreated");
        assert_eq!(app.mirror.get(&jar.to_string(), field::NAME), FieldValue::Str(String::new()), "field {no}");
        assert!(!app.journal.as_ref().unwrap().doc().nodes[&jar].is_live(), "field {no}: the delete was saved");
    }
}

/// Ruling 9, extended to graph nodes (Task 4): a node closed here whose delete is not saved yet must not come back
/// when the save's first round applies another replica's edit to it; the delete then commits. Without the guard the
/// edit re-shows the node and its queued delete is lost.
#[test]
fn a_remote_edit_does_not_bring_back_a_node_closed_here() {
    let path = temp_dir("node-closed-here").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &note, field::TEXT, FieldValue::Str("theirs".into()));
    remote_set(&mut other, &tab, field::URL, FieldValue::Str("https://b/".into()));
    app.remove_node(&note);
    app.remove_tab(&tab);
    assert!(app.save_session());
    for id in [&note, &tab] {
        assert!(!app.nodes.contains_key(id), "{id}: closed here, not shown again");
        assert!(!live_in_doc(&app, id), "{id}: the delete was saved");
        assert!(!app.mirror.has(id), "{id}: nothing of it kept");
    }
    assert!(app.pending_deletes.is_empty());
}

/// `queue_create` checks the document eagerly: an id it already has an entry for (live, or a tombstone) is not
/// queued, so a restore, a remote node and a respawn never fill the queue (the commit's own filter stays the guard);
/// an id that isn't a store id is never queued. Queueing a create withdraws a pending delete of the same id: the
/// node is shown again.
#[test]
fn queue_create_skips_an_id_the_document_already_has() {
    let mut app = app_on(&temp_dir("queue-create-eager").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    let id: NodeId = note.parse().unwrap();
    assert_eq!(app.pending_creates, BTreeSet::from([id]), "a new node is queued");
    assert!(app.save_session());
    app.queue_create(&note);
    assert!(app.pending_creates.is_empty(), "live in the document: not queued again");
    app.queue_delete(&note);
    app.queue_create(&note);
    assert!(app.pending_deletes.is_empty(), "shown again: the delete is withdrawn");
    app.remove_node(&note);
    assert!(app.save_session());
    app.queue_create(&note);
    assert!(app.pending_creates.is_empty(), "a tombstone: never recreated");
    app.queue_create("demo");
    assert!(app.pending_creates.is_empty(), "not a store id");
}

/// `mirror_remote` (Ruling 9) returns the fields another replica changed (old mirror value != new), so
/// `update_live_node` can act on them, leaving out those a pending local edit holds back, which keep the local value.
#[test]
fn mirror_remote_reports_the_changed_fields_but_not_the_held_ones() {
    let mut app = App::default();
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    app.edit_pos(&tab, Some(egui::vec2(5.0, 5.0)));
    let mut fields = app.snapshot(tab.parse().unwrap()).unwrap().fields;
    fields.insert(field::POS, FieldValue::Pos(9.0, 9.0));
    fields.insert(field::URL, FieldValue::Str("https://b/".into()));
    let (held, changed) = app.mirror_remote(&tab, &fields, None);
    assert_eq!(held, [field::POS], "a pending edit holds the field");
    assert_eq!(changed, [field::URL], "the url changed; pinned, engine and the rest did not");
    assert_eq!(app.mirror.url(&tab), "https://b/");
    assert_eq!(app.mirror.pos(&tab), Some(egui::vec2(5.0, 5.0)), "held: the local edit stands");
    let (held, changed) = app.mirror_remote(&tab, &fields, None);
    assert_eq!((held, changed), (vec![field::POS], vec![]), "the same values again change nothing");
}

/// 2b-5 Ruling 7: a jar's name, colour and ephemeral flag are mirror fields. Creating, renaming and toggling a jar
/// write them through `edit_field` and the save stores them; the mirror keeps the jar's entry across saves
/// (`Mirror::retain`), and a remote change reaches the mirror and `jar_defs`.
#[test]
fn a_jar_create_rename_and_ephemeral_save_and_a_remote_change_reaches_jar_defs() {
    let path = temp_dir("jar-mirror-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    let key = jar.to_string();
    let work = FieldValue::Str("Work".into());
    assert_eq!(app.mirror.get(&key, field::NAME), work, "created in the mirror");
    assert!(app.save_session());
    assert_eq!(stored(&app, &key, field::NAME), work);
    assert_eq!(stored(&app, &key, field::COLOR), color_value(app.mirror.jar_defs()[&jar].color));
    assert_eq!(app.mirror.get(&key, field::NAME), work, "kept by the save's retain");
    app.rename_jar(jar, "Job");
    assert!(app.pending.contains_key(&(jar, field::NAME)), "a rename queues its field");
    app.set_jar_ephemeral(jar, true);
    assert!(app.pending.contains_key(&(jar, field::EPHEMERAL)), "so does a toggle");
    assert!(app.save_session());
    assert_eq!(stored(&app, &key, field::NAME), FieldValue::Str("Job".into()));
    assert_eq!(stored(&app, &key, field::EPHEMERAL), FieldValue::Bool(true));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &key, field::NAME, FieldValue::Str("House".into()));
    remote_set(&mut other, &key, field::COLOR, red());
    assert!(app.save_session());
    assert_eq!(app.mirror.jar_defs()[&jar].name, "House");
    assert_eq!(app.mirror.jar_defs()[&jar].color, Some(egui::Color32::RED));
    assert_eq!(app.mirror.get(&key, field::NAME), FieldValue::Str("House".into()));
}

/// A jar rename not saved yet wins over another device's (last writer, as for every mirrored field).
#[test]
fn a_pending_local_jar_rename_wins_over_a_remote_one() {
    let path = temp_dir("jar-rename-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &jar.to_string(), field::NAME, FieldValue::Str("House".into()));
    app.rename_jar(jar, "Job");
    assert!(app.save_session());
    assert_eq!(app.mirror.jar_defs()[&jar].name, "Job");
    assert_eq!(stored(&app, &jar.to_string(), field::NAME), FieldValue::Str("Job".into()));
}

/// Ruling 7: another device's ephemeral toggle has the local one's file side effects (`set_jar_ephemeral`): on deletes
/// the jar's file, off saves it back. It is applied, not queued as a local edit.
#[test]
fn a_remote_ephemeral_change_deletes_and_saves_the_jar_file() {
    let dir = temp_dir("jar-remote-ephemeral");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 1) };
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    assert!(app.save_session());
    let file = crate::jar::Jar::path(&jars, crate::jar::JarKey::Named(jar));
    std::fs::write(&file, r#"{"cookies":[]}"#).unwrap();
    let toggled = |app: &App, on: bool| {
        let mut fields = snapshots(app)[&jar].fields.clone();
        fields.insert(field::EPHEMERAL, FieldValue::Bool(on));
        vec![(jar, Some((Kind::Jar, fields, None)))]
    };
    app.apply_remote(toggled(&app, true));
    assert!(app.mirror.jar_defs()[&jar].ephemeral);
    assert!(!file.exists(), "on: the file goes");
    assert!(app.pending.is_empty(), "a remote value is not a local edit");
    app.apply_remote(toggled(&app, false));
    assert!(!app.mirror.jar_defs()[&jar].ephemeral);
    assert!(file.exists(), "off: saved back");
    assert!(app.pending.is_empty());
}

/// A local ephemeral toggle not saved yet wins over another device's, and the remote value runs no file side effect:
/// here the remote "on" must not delete the file the local "off" just saved.
#[test]
fn a_remote_ephemeral_toggle_under_a_pending_local_one_touches_no_file() {
    let dir = temp_dir("jar-ephemeral-pending");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 1) };
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    app.set_jar_ephemeral(jar, true);
    assert!(app.save_session());
    let file = crate::jar::Jar::path(&jars, crate::jar::JarKey::Named(jar));
    app.set_jar_ephemeral(jar, false);
    assert!(file.exists(), "the local off saved the file");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &jar.to_string(), field::EPHEMERAL, FieldValue::Bool(true));
    assert!(app.save_session());
    assert!(file.exists(), "the held remote on deleted nothing");
    assert!(!app.mirror.jar_defs()[&jar].ephemeral);
    assert_eq!(stored(&app, &jar.to_string(), field::EPHEMERAL), FieldValue::Bool(false), "the local value is saved");
}

/// A jar first seen already ephemeral (created on another device, or restored) deletes a leftover file of it, as a
/// toggle on would, and queues nothing.
#[test]
fn a_jar_first_seen_ephemeral_deletes_its_leftover_file_and_queues_nothing() {
    let dir = temp_dir("jar-ephemeral-first");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let leftover = |jar| {
        let file = crate::jar::Jar::path(&jars, crate::jar::JarKey::Named(jar));
        std::fs::write(&file, r#"{"cookies":[]}"#).unwrap();
        file
    };
    // Created on another device.
    let mut app = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 1) };
    let remote = NodeId { actor: ActorId(2), counter: 7 };
    let file = leftover(remote);
    let fields = Fields::from([
        (field::COLOR, color_value(None)),
        (field::NAME, FieldValue::Str("Theirs".into())),
        (field::EPHEMERAL, FieldValue::Bool(true)),
    ]);
    app.apply_remote(vec![(remote, Some((Kind::Jar, fields, None)))]);
    assert!(app.mirror.jar_defs()[&remote].ephemeral);
    assert!(!file.exists(), "remote-created: the leftover file goes");
    assert!(app.pending.is_empty(), "remote-created: nothing queued");
    // Restored at startup.
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    app.set_jar_ephemeral(jar, true);
    assert!(app.save_session());
    drop(app);
    let file = leftover(jar);
    let mut back = App { jars_dir_override: Some(jars.clone()), ..app_on(&path, 3) };
    assert!(back.restore_session());
    assert!(back.mirror.jar_defs()[&jar].ephemeral);
    assert!(!file.exists(), "restored: the leftover file goes");
    assert!(!back.pending.keys().any(|(node, _)| *node == jar), "restored: nothing queued for it");
}

/// Final review I2b: a jar deleted on another device while this one was not running never reaches `forget_jar`
/// here, so startup deletes the files of named jars the document no longer has, unless it may not know them all.
#[test]
fn startup_deletes_the_files_of_jars_the_document_no_longer_has() {
    let dir = crate::store::testutil::temp_dir("jar-prune-startup");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let work = app.create_jar("Work");
    app.save_session();
    drop(app);
    let gone = NodeId { actor: ActorId(0xdead), counter: 9 };
    let (w, g) = (crate::jar::JarKey::Named(work).file_stem(), crate::jar::JarKey::Named(gone).file_stem());
    let files = [format!("{w}.json"), format!("{g}.json"), format!("{g}.json.unreadable-5"), "default.json".into()];
    // A readable (legacy plaintext) empty jar: the restore wakes a tab, which loads the default jar.
    let write_all = || files.iter().for_each(|f| std::fs::write(jars.join(f), r#"{"cookies":[]}"#).unwrap());
    let left = || files.iter().filter(|f| jars.join(f).exists()).cloned().collect::<Vec<_>>();
    let on = |actor| App { jars_dir_override: Some(jars.clone()), ..app_on(&path, actor) };
    write_all();

    // No journal: nothing is known, nothing goes.
    App { jars_dir_override: Some(jars.clone()), ..App::default() }.prune_jar_files();
    assert_eq!(left().len(), 4);
    // A repaired store may have lost jar nodes.
    let mut repaired = on(2);
    repaired.restore_session();
    repaired.store_recovered = true;
    repaired.prune_jar_files();
    assert_eq!(left().len(), 4);
    drop(repaired);
    // A new, empty session file (deleted or moved away) knows no jars at all.
    let mut fresh = App { jars_dir_override: Some(jars.clone()), ..app_on(&dir.join("other.mnf"), 3) };
    fresh.restore_session();
    fresh.prune_jar_files();
    assert_eq!(left().len(), 4);
    drop(fresh);

    let mut back = on(4);
    assert!(back.restore_session());
    back.prune_jar_files();
    assert_eq!(left(), [format!("{w}.json"), "default.json".to_string()], "the deleted jar's files go");
}

#[test]
fn deleting_a_jar_clears_the_tabs_and_groups_that_named_it() {
    let mut app = App::default();
    let jar = app.create_jar("Work");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let grp = add(&mut app, Kind::Group, Runtime::None);
    app.edit_default_jar(&grp, Some(jar));
    app.edit_jar(&tab, crate::jar::JarChoice { jar: Some(jar), set: true });
    app.delete_jar(jar);
    assert!(app.mirror.jar_defs().is_empty());
    assert_eq!(app.mirror.jar_choice(&tab).jar, None);
    assert_eq!(app.mirror.default_jar(&grp), None);
}

/// A file with a jar "Work" (label j), a group defaulting to it and a tab in the group using it.
fn file_with_a_work_jar() -> JsonDoc {
    let json = serde_json::json!({ "nodes": [
        json_node("g", Kind::Group, serde_json::json!({ "name": "G", "default_jar": "j" })),
        json_node("j", Kind::Jar, serde_json::json!({ "name": "Work" })),
        json_node(
            "t",
            Kind::Tab,
            serde_json::json!({ "url": "https://a.com/", "group": "g", "jar": "j", "jar_set": true }),
        ),
    ]});
    serde_json::from_value(json).unwrap()
}

/// Jar name labels (retiring per-tab jars deviation 4): a jar is its id, never its name, so an imported jar is
/// always a new jar, even beside one with the same name, and the imported tab and group name the new one. The
/// existing jar is left as it was, and outlives the session swap.
#[test]
fn opening_a_session_gives_each_imported_jar_a_new_jar() {
    let mut app = App::default();
    let work = app.create_jar("Work");
    let before = app.mirror.jar_defs()[&work].clone();
    app.replace_running_session(&file_with_a_work_jar(), crate::store::json::Trust::Foreign);
    assert_eq!(app.mirror.jar_defs().len(), 2, "a second Work");
    assert_eq!(app.mirror.jar_defs()[&work], before, "the existing jar is untouched");
    let new = *app.mirror.jar_defs().keys().find(|&&j| j != work).unwrap();
    assert_eq!(app.mirror.jar_defs()[&new].name, "Work");
    let tab =
        app.nodes.iter().find(|(_, n)| n.runtime.as_unloaded().is_some()).map(|(id, _)| id).expect("tab imported");
    assert_eq!(app.mirror.jar_choice(tab), crate::jar::JarChoice { jar: Some(new), set: true });
    let group = app.nodes.keys().find(|id| app.is_container(id)).expect("group imported");
    assert_eq!(app.mirror.default_jar(group), Some(new));
    assert_ne!(
        app.jar_label(crate::jar::JarKey::Named(new)),
        app.jar_label(crate::jar::JarKey::Named(work)),
        "told apart by label"
    );
}

/// Import Group the same, twice over: each import is another new jar.
#[test]
fn importing_a_group_gives_each_imported_jar_a_new_jar() {
    let mut app = App::default();
    let work = app.create_jar("Work");
    for n in 2..=3 {
        app.import_group_json(&file_with_a_work_jar());
        assert_eq!(app.mirror.jar_defs().len(), n);
    }
    let new: BTreeSet<NodeId> = app.mirror.jar_defs().keys().copied().filter(|&j| j != work).collect();
    let tabs: BTreeSet<NodeId> = app
        .nodes
        .iter()
        .filter(|(_, n)| n.runtime.as_unloaded().is_some())
        .filter_map(|(id, _)| app.mirror.jar_choice(id).jar)
        .collect();
    let groups: BTreeSet<NodeId> =
        app.nodes.keys().filter(|id| app.is_container(id)).filter_map(|id| app.mirror.default_jar(id)).collect();
    assert_eq!((&tabs, &groups), (&new, &new), "each import's tab and group name its own new jar");
}

/// A reopened tab comes back asleep, starting no engine, and keeps its
/// zoom and title (spec "Lazy tabs").
#[test]
fn a_reopened_tab_comes_back_asleep_with_its_zoom_and_title() {
    let mut app = app_on(&temp_dir("reopen-sleep").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://r/", "Page R"));
    if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
        t.zoom_factor = 1.5;
    }
    let entry = crate::closed_entry(&app, &tab).unwrap();
    app.remove_tab(&tab);
    app.reopen_closed(entry);
    assert!(app.test_spawns.is_empty());
    let cards: Vec<&UnloadedTabPayload> = app.nodes.values().filter_map(|n| n.as_unloaded()).collect();
    assert!(
        matches!(&cards[..], [t] if t.url == "https://r/" && t.title == "Page R" && t.zoom_factor == 1.5
            && t.state == UnloadedState::Sleeping),
        "no sleeping card"
    );
}

/// Same for a tab in an imported group: it sleeps in the new group.
#[test]
fn an_imported_groups_tab_arrives_asleep_in_the_new_group() {
    let mut app = app_on(&temp_dir("import-sleep").join("session.mnf"), 1);
    let g = add_group(&mut app, "G");
    let tab = add(&mut app, Kind::Tab, failed_tab("https://i/", 1.0));
    app.edit_group(&tab, Some(g.clone()));
    let json = app.export_group_json(&g).unwrap();
    app.import_group_json(&json);
    assert!(app.test_spawns.is_empty());
    let (copy_id, copy) = app.nodes.iter().find(|(id, n)| **id != tab && n.as_unloaded().is_some()).expect("a card");
    assert_eq!(copy.as_unloaded().unwrap().state, UnloadedState::Sleeping);
    let group = app.mirror.group(copy_id).expect("in a group");
    assert!(group != g && app.is_container(group), "in the imported group");
}

/// A member whose group isn't part of the imported file lands where
/// the file's root does (the level being viewed), not at the top level.
#[test]
fn an_imported_member_whose_group_is_outside_the_file_joins_the_roots_level() {
    let mut app = app_on(&temp_dir("import-outside").join("session.mnf"), 1);
    let level = add_group(&mut app, "L");
    let elsewhere = add_group(&mut app, "E");
    app.current_group = Some(level.clone());
    let json = JsonDoc {
        nodes: vec![
            json_node("a", Kind::Group, serde_json::json!({ "name": "G" })),
            json_node("b", Kind::Note, serde_json::json!({ "text": "t", "group": elsewhere })),
        ],
        local: None,
    };
    app.import_group_json(&json);
    let (note, _) = app.nodes.iter().find(|(_, n)| matches!(&n.runtime, Runtime::Note(_))).expect("imported");
    assert_eq!(app.mirror.group(note), Some(level.as_str()));
}

/// A session with no nodes still keeps Recently Closed: the launch
/// starts fresh, but the next personal-state write carries the list.
#[test]
fn an_empty_session_keeps_recently_closed_across_a_launch() {
    let path = temp_dir("empty-closed").join("session.mnf");
    let entry = JsonDoc { nodes: vec![json_node("x", Kind::Note, serde_json::json!({ "text": "gone" }))], local: None };
    {
        let journal = Journal::open(&path, ActorId(1)).unwrap();
        let state = LocalState { closed: vec![entry.clone()], ..LocalState::default() };
        journal.write_local(&state.to_bytes()).unwrap();
    }
    let mut app = app_on(&path, 2);
    assert!(!app.restore_session(), "no nodes: the caller starts fresh");
    assert_eq!(app.closed.len(), 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    app.maybe_write_local(true);
    let bytes = app.journal.as_ref().unwrap().read_local().unwrap().unwrap();
    assert_eq!(LocalState::from_bytes(&bytes).unwrap().closed, vec![entry]);
}

/// Final-review fix: personal state startup couldn't read (another window
/// held the lock past `LOCK_WAIT`) is never written over with defaults,
/// even at exit; once a retry reads it, it is applied, and what was
/// closed since stays in Recently Closed. Takes `LOCK_WAIT` (5 s) once.
#[test]
fn personal_state_startup_could_not_read_is_retried_not_overwritten() {
    let path = temp_dir("local-unread").join("session.mnf");
    let note = {
        let mut app = app_on(&path, 1);
        let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
        app.close_store();
        note
    };
    let old = JsonDoc { nodes: vec![json_node("x", Kind::Note, serde_json::json!({ "text": "gone" }))], local: None };
    let entry = LocalNode { pos: [7.0, 8.0], ..LocalNode::default() };
    let side = crate::store::local::path_for(&path);
    {
        let journal = Journal::open(&path, ActorId(2)).unwrap();
        let state = LocalState {
            nodes: BTreeMap::from([(note.clone(), entry.clone())]),
            closed: vec![old.clone()],
            ..LocalState::default()
        };
        journal.write_local(&state.to_bytes()).unwrap();
    }
    let saved = std::fs::read(&side).unwrap();

    // A peer takes the session's lock once it's open, and holds it
    // through the personal-state read.
    let mut app = app_on(&path, 3);
    let lock_path = path.with_extension("mnf.lock");
    let peer = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path).unwrap();
    peer.try_lock().unwrap();
    assert!(app.restore_session());
    assert!(app.local_unread.is_some(), "the read failed");
    assert!(app.closed.is_empty());
    let since =
        JsonDoc { nodes: vec![json_node("y", Kind::Note, serde_json::json!({ "text": "since" }))], local: None };
    app.closed.push(ClosedEntry { doc: since.clone() });
    // Due writes, a retry while the lock is still busy, and even exit's
    // forced write leave the file alone.
    app.maybe_write_local(true);
    app.local_unread.as_mut().unwrap().retry_at = std::time::Instant::now();
    app.poll_store();
    assert!(app.local_unread.is_some(), "still busy: retried later");
    assert_eq!(std::fs::read(&side).unwrap(), saved, "not overwritten while unread");

    // The lock is free: the next retry reads and applies it.
    drop(peer);
    app.local_unread.as_mut().unwrap().retry_at = std::time::Instant::now();
    app.poll_store();
    assert!(app.local_unread.is_none());
    assert_eq!(app.layout_pos(&note), egui::vec2(7.0, 8.0));
    let closed: Vec<&JsonDoc> = app.closed.iter().map(|e| &e.doc).collect();
    assert_eq!(closed, [&old, &since], "read entries first, then what closed since");
    app.close_store();
    let bytes = Journal::open(&path, ActorId(4)).unwrap().read_local().unwrap().unwrap();
    let written = LocalState::from_bytes(&bytes).unwrap();
    assert_eq!(written.closed, vec![old, since]);
    assert_eq!(written.nodes[&note].pos, [7.0, 8.0]);
}

/// A damaged `.local` is still a fresh start that the next write replaces
/// (`a_damaged_or_newer_sidecar_is_not_read_and_is_replaced`): nothing in
/// it is readable, and retrying can't change that.
#[test]
fn a_damaged_personal_state_file_is_not_retried() {
    let path = temp_dir("local-damaged-restore").join("session.mnf");
    {
        let mut app = app_on(&path, 1);
        add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
        app.close_store();
    }
    std::fs::write(crate::store::local::path_for(&path), b"MNFL damaged").unwrap();
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert!(app.local_unread.is_none());
    // An unchanged state isn't written (`local_written` is armed); a change is.
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "m".into(), editing: false }));
    app.maybe_write_local(true);
    let bytes = app.journal.as_ref().unwrap().read_local().unwrap();
    assert!(bytes.is_some(), "replaced with this profile's state");
}

/// Personal state as a peer wrote it while this instance's startup read it
/// unread: `local_unread` set by hand, without waiting out `LOCK_WAIT`.
fn mark_unread_with(app: &mut App, path: &std::path::Path, state: &LocalState) {
    Journal::open(path, ActorId(9)).unwrap().write_local(&state.to_bytes()).unwrap();
    app.local_unread = Some(LocalUnread { retry_at: std::time::Instant::now(), fresh: false });
    app.local_written = None;
}

/// Open Session while startup's personal state is unread: that state is the
/// old session's, so a late read keeps only its Recently Closed and none of
/// its view (here Item view, which would otherwise replace Graph).
#[test]
fn opening_a_session_while_unread_takes_only_recently_closed_on_retry() {
    let path = temp_dir("local-unread-open").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    let old = JsonDoc { nodes: vec![json_node("x", Kind::Note, serde_json::json!({ "text": "gone" }))], local: None };
    mark_unread_with(
        &mut app,
        &path,
        &LocalState { view_item: true, closed: vec![old.clone()], ..LocalState::default() },
    );
    let json =
        JsonDoc { nodes: vec![json_node("t", Kind::Tab, serde_json::json!({ "url": "https://new/" }))], local: None };
    app.replace_running_session(&json, crate::store::json::Trust::Own);
    assert!(app.local_unread.is_some_and(|u| u.fresh), "the open makes the retry a fresh start");
    app.poll_store();
    assert!(app.local_unread.is_none(), "read");
    assert_eq!(app.view_mode, ViewMode::Graph, "the old session's view isn't applied");
    let closed: Vec<&JsonDoc> = app.closed.iter().map(|e| &e.doc).collect();
    assert_eq!(closed, [&old], "its Recently Closed is");
}

/// Exit's forced retry applies what it reads so the write that follows
/// keeps it, but wakes nothing: the app is closing.
#[test]
fn the_forced_retry_at_exit_applies_without_waking() {
    let path = temp_dir("local-unread-exit").join("session.mnf");
    let (a, b) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        assert!(app.save_session());
        (a, b)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.focused.as_deref(), Some(a.as_str()));
    let woken = app.test_spawns.len();
    mark_unread_with(&mut app, &path, &LocalState { focused: Some(b.clone()), ..LocalState::default() });
    app.close_store();
    assert_eq!(app.test_spawns.len(), woken, "nothing woken at exit");
    assert_eq!(app.focused.as_deref(), Some(b.as_str()), "applied");
    let bytes = Journal::open(&path, ActorId(3)).unwrap().read_local().unwrap().unwrap();
    assert_eq!(LocalState::from_bytes(&bytes).unwrap().focused, Some(b), "and written");
}

/// A late read that finds no personal state (or a damaged one) applies
/// nothing: defaults would undo what the user changed since startup.
#[test]
fn a_late_read_of_no_personal_state_keeps_the_live_view() {
    let path = temp_dir("local-unread-none").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    app.local_unread = Some(LocalUnread { retry_at: std::time::Instant::now(), fresh: false });
    app.local_written = None;
    app.view_mode = ViewMode::Item;
    app.groups_collapsed = true;
    app.poll_store();
    assert!(app.local_unread.is_none(), "read: there is none");
    assert_eq!((app.view_mode, app.groups_collapsed), (ViewMode::Item, true), "not reset to defaults");
    app.maybe_write_local(true);
    let bytes = app.journal.as_ref().unwrap().read_local().unwrap().unwrap();
    assert!(LocalState::from_bytes(&bytes).unwrap().groups_collapsed, "the next write saves the live view");
}

/// Quitting while an imported session's tabs are still spawning keeps
/// them: each is saved as it was started, and the replaced session's
/// nodes are gone.
#[test]
fn tabs_still_spawning_at_quit_are_saved() {
    let path = temp_dir("spawning-quit").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "old".into(), editing: false }));
    assert!(app.save_session());
    let json =
        JsonDoc { nodes: vec![json_node("t", Kind::Tab, serde_json::json!({ "url": "https://new/" }))], local: None };
    app.replace_running_session(&json, crate::store::json::Trust::Foreign);
    assert_eq!(app.test_spawns.len(), 1, "the tab is still spawning");
    app.save_session();
    app.close_store();
    drop(app);
    let journal = Journal::open(&path, ActorId(3)).unwrap();
    let live: Vec<(Kind, FieldValue)> = journal.doc().live().map(|(_, n)| (n.kind(), n.get(field::URL))).collect();
    assert_eq!(live, [(Kind::Tab, FieldValue::Str("https://new/".into()))]);
}

/// Open Session (`replace_running_session`) restores every tab asleep,
/// same as a launch, and wakes only the focused (first) one -- the
/// existing `tabs_still_spawning_at_quit_are_saved` uses a single tab,
/// which can't tell "asleep" from "still spawning" apart.
#[test]
fn open_session_restores_asleep_and_wakes_only_the_focused_tab() {
    let mut app = app_on(&temp_dir("open-session-lazy").join("session.mnf"), 1);
    let json = JsonDoc {
        nodes: vec![
            json_node("t1", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
            json_node("t2", Kind::Tab, serde_json::json!({ "url": "https://b/" })),
            json_node("t3", Kind::Tab, serde_json::json!({ "url": "https://c/" })),
        ],
        local: None,
    };
    app.replace_running_session(&json, crate::store::json::Trust::Own);
    assert_eq!(app.test_spawns.len(), 1, "only the focused (first) tab wakes");
    assert!(matches!(app.test_spawns[0].0.purpose, SpawnPurpose::Wake));
    let focused = app.focused.clone().unwrap();
    assert_eq!(app.test_spawns[0].0.id, focused);
    let asleep: Vec<&UnloadedTabPayload> =
        app.nodes.iter().filter(|(id, _)| **id != focused).filter_map(|(_, n)| n.as_unloaded()).collect();
    assert_eq!(asleep.len(), 2);
    assert!(asleep.iter().all(|t| t.state == UnloadedState::Sleeping));
}

/// Without personal state, restore focuses the first tab (as it always
/// has) and wakes only that one.
#[test]
fn restore_without_personal_state_focuses_and_wakes_the_first_tab() {
    let path = temp_dir("no-local-restore").join("session.mnf");
    let (a, b) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", "B"));
        assert!(app.save_session());
        // No `maybe_write_local`: no `session.mnf.local` at all, the
        // literal "without personal state" case, not just one whose
        // `focused` field happens to be unset.
        (a, b)
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.focused.as_deref(), Some(a.as_str()));
    let spawned: Vec<&str> = app.test_spawns.iter().map(|(s, _)| s.id.as_str()).collect();
    assert_eq!(spawned, [a.as_str()]);
    assert_eq!(app.nodes[&b].as_unloaded().unwrap().state, UnloadedState::Sleeping);
}

/// Personal state written before a restored tab's wake lands still
/// keeps theirs (position, title, zoom), and a fresh failed card's zoom
/// is kept too. A restored tab is an `Unloaded` card at once, so
/// `apply_local` applies its entry directly; the `pending_local` path
/// for a node still spawning (a group import, Recently Closed) isn't
/// covered here.
#[test]
fn personal_state_survives_onto_a_still_waking_restored_card_and_a_fresh_failed_cards_zoom() {
    let path = temp_dir("local-spawning").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, failed_tab("https://s/", 1.0));
        app.close_store();
        tab
    };
    let entry = LocalNode { pos: [7.0, 8.0], title: "T".into(), zoom: Some(2.0), ..LocalNode::default() };
    {
        let journal = Journal::open(&path, ActorId(2)).unwrap();
        let state = LocalState { nodes: BTreeMap::from([(tab.clone(), entry.clone())]), ..LocalState::default() };
        journal.write_local(&state.to_bytes()).unwrap();
    }
    let mut app = app_on(&path, 3);
    assert!(app.restore_session());
    // Still waking, not landed -- `entry` is applied straight onto the
    // live `Unloaded` card (`apply_local_node`), not carried through
    // `pending_local`.
    assert_eq!(app.test_spawns.len(), 1, "the restored tab's wake is still in flight");
    let card = add(&mut app, Kind::Tab, failed_tab("https://f/", 1.5));
    let state = app.local_state();
    assert_eq!(state.nodes.get(&tab), Some(&entry));
    assert_eq!(state.nodes[&card].zoom, Some(1.5));
}

/// A tab its page moved on without sharing (a redirect, a script's
/// navigation) reopens where it was, unless the shared url has changed
/// since.
#[test]
fn a_restart_reopens_a_tabs_live_url_while_the_shared_url_is_unchanged() {
    let path = temp_dir("live-url").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, failed_tab("https://s/", 1.0));
        assert!(app.save_session());
        if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
            t.url = "https://s/moved".into();
        }
        app.close_store();
        tab
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.test_spawns[0].0.url, "https://s/moved");
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://s/".into()), "still unshared");
    // The single tab is focused, so it is the one waking.
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://s/moved");
    app.close_store();

    // Moved on from a url the store no longer holds: the shared one wins.
    {
        let journal = Journal::open(&path, ActorId(3)).unwrap();
        let live_url = Some(LiveUrl { url: "https://s/moved".into(), from: "https://elsewhere/".into() });
        let entry = LocalNode { live_url, ..LocalNode::default() };
        let state = LocalState { nodes: BTreeMap::from([(tab.clone(), entry)]), ..LocalState::default() };
        journal.write_local(&state.to_bytes()).unwrap();
    }
    let mut app = app_on(&path, 4);
    assert!(app.restore_session());
    assert_eq!(app.test_spawns[0].0.url, "https://s/");
}

/// Export Group of a group the store doesn't hold gives nothing (and
/// says so) rather than an empty file.
#[test]
fn exporting_a_group_the_store_lacks_gives_none() {
    let mut app = app_on(&temp_dir("export-missing").join("session.mnf"), 1);
    let g = app.alloc_node_id();
    assert!(app.export_group_json(&g).is_none());
    assert!(!app.save_toast_queue.is_empty(), "toasted");
}

/// The mirror is what this replica knows of the store (2b-5 Task 4: `last_shared` is gone). After each kind of save,
/// nothing pending, every live node's snapshot (`App::snapshot`: the mirror's fields and group) must be exactly what
/// the store holds for it, whether the commit wrote it, another replica did, or a remote delete moved it.
#[test]
fn the_mirror_matches_the_store_after_every_save() {
    fn check(app: &App, step: &str) {
        let doc = app.journal.as_ref().unwrap().doc();
        for (id, node) in doc.live() {
            let want = Snapshot { kind: node.kind(), group: doc.effective_group(id), fields: fields_of(node) };
            assert_eq!(app.snapshot(id), Some(want), "{step}: node {id}");
        }
    }
    let dir = temp_dir("mirror-matches-store");
    let path = dir.join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "outer");
    let inner = add_group(&mut app, "inner");
    let tab = add(&mut app, Kind::Tab, failed_tab("https://a/", 1.0));
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: String::new(), editing: false }));
    app.edit_group(&inner, Some(outer.clone()));
    app.edit_group(&tab, Some(inner.clone()));
    assert!(app.save_session());
    check(&app, "creates");

    if let Runtime::Note(n) = &mut app.nodes.get_mut(&note).unwrap().runtime {
        n.text = "typed".into();
    }
    assert!(app.save_session());
    check(&app, "a set");

    // Another replica pins the tab and deletes the inner group, so the
    // tab's effective group becomes `outer` with no op naming the tab.
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(true));
    other.begin().unwrap().commit(vec![OpBody::Delete { node: inner.parse().unwrap() }]).unwrap();
    assert!(app.save_session());
    check(&app, "remote set and delete");

    app.dissolve_group(&outer);
    assert!(app.save_session());
    check(&app, "a local delete and moves");

    if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
        t.url = "https://b/".into();
    }
    app.edit_url(&tab, "https://b/");
    assert!(app.save_session());
    check(&app, "a shared url");
}

/// What the app's side of a save costs at realistic session sizes: the
/// snapshot of every node (`snapshots`, more than a save takes: it projects only the queued creates), a whole
/// `save_session` (projection, `begin`, the queued ops, commit) with nothing and with
/// one url to save, and the personal-state build and write. The store
/// side alone is `journal::tests::save_path_costs`. Timing, not
/// correctness, so ignored:
/// `cargo test --release -j 12 -p manifold -- --ignored --nocapture app_save_costs`
#[test]
#[ignore]
fn app_save_costs() {
    use std::time::{Duration, Instant};
    fn timed(n: usize, mut f: impl FnMut()) -> (Duration, Duration) {
        let mut times: Vec<Duration> = (0..n)
            .map(|_| {
                let start = Instant::now();
                f();
                start.elapsed()
            })
            .collect();
        times.sort();
        (times[0], times[n / 2])
    }
    fn report(what: &str, (best, median): (Duration, Duration)) {
        println!("  {what:<44} best {best:>10.1?}  median {median:>10.1?}");
    }
    const N: usize = 40;
    for tabs in [300usize, 3000] {
        let dir = temp_dir(&format!("app-cost-{tabs}"));
        let mut app = app_on(&dir.join("session.mnf"), 1);
        let groups: Vec<String> = (0..tabs / 10).map(|i| add_group(&mut app, &format!("Group {i}"))).collect();
        let mut ids = Vec::new();
        for i in 0..tabs {
            let url = format!("https://www.example{}.com/articles/2026/09/some-long-article-slug-{i}?ref=home", i % 50);
            let id = add(&mut app, Kind::Tab, failed_tab(&url, 1.0));
            app.edit_group(&id, Some(groups[i % groups.len()].clone()));
            let pos = egui::vec2(i as f32 * 3.5, i as f32 * -2.25);
            app.snap_layout(&id, pos);
            app.edit_pos(&id, Some(pos));
            ids.push(id);
        }
        assert!(app.save_session());
        println!("{tabs} tabs, {} groups:", groups.len());

        report("snapshot of every node", timed(N, || drop(snapshots(&app))));
        report("save_session, nothing changed", timed(N, || assert!(app.save_session())));
        let mut k = 0;
        report(
            "save_session, one url changed",
            timed(N, || {
                let id = &ids[k % tabs];
                let url = format!("https://moved.example/{k}");
                if let Runtime::Unloaded(f) = &mut app.nodes.get_mut(id).unwrap().runtime {
                    f.url = url.clone();
                }
                app.edit_url(id, &url);
                k += 1;
                assert!(app.save_session());
            }),
        );
        report("local_state (every 30 s, and at exit)", timed(N, || drop(app.local_state())));
        report(
            "maybe_write_local, changed (build + write)",
            timed(N, || {
                app.layout.get_mut(&ids[k % tabs]).unwrap().pos.x += 1.0;
                k += 1;
                app.maybe_write_local(true);
            }),
        );
        let bytes = app.local_state().to_bytes().len();
        println!("  session.mnf.local: {bytes} B");
    }
}

/// Replacing the running session skips the renders of its PDFs that haven't started, as deleting one does.
#[test]
fn teardown_cancels_pending_pdf_renders() {
    let mut app = App::default();
    let pdf = add(&mut app, Kind::Pdf, Runtime::Pdf(crate::node::PdfPayload { path: "d.pdf".into(), outcome: None }));
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    app.teardown_running_session();
    assert!(app.pdf_renderer.is_cancelled(&pdf));
    assert!(!app.pdf_renderer.is_cancelled(&note), "only PDFs are marked");
}

fn red() -> FieldValue {
    color_value(Some(egui::Color32::RED))
}

fn blue() -> FieldValue {
    color_value(Some(egui::Color32::BLUE))
}

fn a_note(app: &mut App) -> String {
    add(app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }))
}

#[test]
fn a_local_color_edit_is_committed_by_the_next_save() {
    let path = temp_dir("edit-field").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.edit_field(&note, field::COLOR, red());
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED), "the mirror shows it at once");
    assert_eq!(stored(&app, &note, field::COLOR), FieldValue::Color(None), "nothing is written before a save");
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::COLOR), red());
    assert!(app.pending.is_empty(), "a commit clears what it wrote");
}

#[test]
fn a_color_set_before_the_first_save_rides_the_create() {
    let path = temp_dir("edit-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    app.edit_field(&note, field::COLOR, red());
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::COLOR), red());
    assert!(app.pending.is_empty());
}

#[test]
fn a_remote_color_edit_reaches_the_mirror() {
    let path = temp_dir("edit-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &note, field::COLOR, red());
    assert!(app.save_session());
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED));
}

#[test]
fn a_pending_local_color_edit_wins_over_a_remote_edit_of_the_same_field() {
    let path = temp_dir("edit-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_field(&note, field::COLOR, blue());
    remote_set(&mut other, &note, field::COLOR, red());
    // The remote round applies first, the next round commits the local edit over it.
    assert!(app.save_session());
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::BLUE));
    assert_eq!(stored(&app, &note, field::COLOR), blue());
}

/// The node menu's colour actions (regression guards: they go through `edit_field`).
#[test]
fn the_set_color_action_edits_the_mirror_and_the_next_save_stores_it() {
    let path = temp_dir("menu-set-color").join("session.mnf");
    let mut app = app_on(&path, 1);
    let ctx = egui::Context::default();
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.apply_node_action(&ctx, &note, NodeAction::SetColor(Some(egui::Color32::RED)));
    assert_eq!(app.mirror.color(&note), Some(egui::Color32::RED), "the mirror shows it at once");
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::COLOR), red());
    app.apply_node_action(&ctx, &note, NodeAction::SetColor(None));
    assert_eq!(app.mirror.color(&note), None);
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::COLOR), FieldValue::Color(None));
}

#[test]
fn the_cycle_color_action_steps_through_the_tag_colors() {
    use crate::theme::next_tag_color;
    let path = temp_dir("menu-cycle-color").join("session.mnf");
    let mut app = app_on(&path, 1);
    let ctx = egui::Context::default();
    let note = a_note(&mut app);
    assert!(app.save_session());
    let first = next_tag_color(None);
    assert!(first.is_some());
    app.apply_node_action(&ctx, &note, NodeAction::CycleColor);
    assert_eq!(app.mirror.color(&note), first);
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::COLOR), color_value(first));
    let second = next_tag_color(first);
    app.apply_node_action(&ctx, &note, NodeAction::CycleColor);
    assert_eq!(app.mirror.color(&note), second);
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::COLOR), color_value(second));
}

#[test]
fn a_color_action_on_an_unknown_node_changes_nothing() {
    let mut app = App::default();
    let ctx = egui::Context::default();
    app.apply_node_action(&ctx, "ghost", NodeAction::SetColor(Some(egui::Color32::RED)));
    app.apply_node_action(&ctx, "ghost", NodeAction::CycleColor);
    assert_eq!(app.mirror.color("ghost"), None);
    assert!(app.pending.is_empty());
}

/// A saved node edited, then deleted, before the same save: the delete stands.
#[test]
fn an_edit_to_a_node_the_store_dropped_is_not_written_back() {
    let path = temp_dir("edit-dropped").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.edit_field(&note, field::COLOR, red());
    app.remove_node(&note);
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert!(!doc.nodes[&note.parse().unwrap()].is_live(), "the delete stands; no placeholder or recreate");
}

/// The save that deletes a node writes only the `Delete`, not the node's pending edit after it.
#[test]
fn a_save_that_deletes_a_node_writes_no_set_for_it() {
    let path = temp_dir("edit-delete-ops").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let ops_before = app.journal.as_ref().unwrap().doc().version[&ActorId(1)];
    app.edit_field(&note, field::COLOR, red());
    app.remove_node(&note);
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().version[&ActorId(1)], ops_before + 1, "one op: the Delete");
    assert_eq!(stored(&app, &note, field::COLOR), FieldValue::Color(None));
}

/// A node never saved, edited and removed before the first save: no `Set` reaches the store, so no placeholder.
#[test]
fn an_edit_to_an_unsaved_node_removed_before_the_save_leaves_no_placeholder() {
    let path = temp_dir("edit-unsaved-removed").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    app.edit_field(&note, field::COLOR, red());
    app.remove_node(&note);
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert!(!doc.nodes.contains_key(&note.parse().unwrap()), "no entry at all for the id");
}

/// Since 2b-5 Task 2 a jar's fields are mirrored like every other kind's: `App::pending` writes them, and nothing
/// compares the mirror with the document (Task 3: `diff` is gone), so a value written to the mirror directly is
/// never saved.
#[test]
fn a_jars_fields_reach_the_document_only_through_pending() {
    let mut app = app_on(&temp_dir("jar-fields-pending-only").join("session.mnf"), 1);
    let jar = app.create_jar("j").to_string();
    assert!(app.save_session());
    app.mirror.set(&jar, field::COLOR, red());
    app.mirror.set(&jar, field::EPHEMERAL, FieldValue::Bool(true));
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "nothing written");
    assert_eq!(stored(&app, &jar, field::EPHEMERAL), FieldValue::Bool(false));
}

#[test]
fn a_non_store_id_edits_the_mirror_and_queues_nothing() {
    let mut app = App::default();
    app.edit_field("r", field::COLOR, red());
    assert_eq!(app.mirror.color("r"), Some(egui::Color32::RED));
    assert!(app.pending.is_empty());
}

#[test]
fn an_unchanged_edit_queues_nothing() {
    let path = temp_dir("edit-noop").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    app.edit_field(&note, field::COLOR, FieldValue::Color(None));
    assert!(app.pending.is_empty());
}

/// The same for a graph node: an existing node's mirrored fields reach the document only as `pending` `Set`s.
#[test]
fn an_existing_nodes_fields_reach_the_document_only_through_pending() {
    let mut app = app_on(&temp_dir("node-fields-pending-only").join("session.mnf"), 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.mirror.set(&note, field::COLOR, red());
    app.mirror.set(&note, field::URL, FieldValue::Str("https://elsewhere/".into()));
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "nothing written");
    assert_eq!(stored(&app, &note, field::COLOR), FieldValue::Color(None));
}

#[test]
fn a_tab_pin_is_committed_and_a_remote_pin_reaches_the_mirror() {
    let path = temp_dir("pin-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0));
    assert!(app.save_session());
    app.edit_field(&tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.mirror.pinned(&tab));
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::PINNED), FieldValue::Bool(true));

    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(false));
    assert!(app.save_session());
    assert!(!app.mirror.pinned(&tab));
}

#[test]
fn pinning_skips_a_node_that_is_not_a_tab() {
    let mut app = App::default();
    let note = a_note(&mut app);
    app.focused = Some(note.clone());
    app.pin_focused_tab_shortcut();
    assert!(!app.mirror.pinned(&note));
}

#[test]
fn pin_all_pins_every_tab_and_only_tabs() {
    let mut app = App::default();
    let (a, b) = (
        add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0)),
        add(&mut app, Kind::Tab, failed_tab("https://u0/", 1.0)),
    );
    let note = a_note(&mut app);
    app.set_all_pinned(true);
    assert!(app.mirror.pinned(&a) && app.mirror.pinned(&b) && !app.mirror.pinned(&note));
    app.set_all_pinned(false);
    assert!(!app.mirror.pinned(&a) && !app.mirror.pinned(&b));
}

/// A remote colour parked for a waking tab (`remote_in_flight`) must not land over a local edit committed after it.
#[test]
fn a_committed_local_edit_wins_over_a_remote_value_parked_for_a_waking_tab() {
    let path = temp_dir("edit-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    assert!(app.save_session());
    app.focus_tab(&tab);
    assert!(app.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::COLOR, blue());
    app.edit_field(&tab, field::COLOR, red());
    // The remote round parks blue, the next round commits red.
    assert!(app.save_session());
    assert!(app.remote_in_flight.contains_key(&tab), "the remote value is parked");
    assert_eq!(stored(&app, &tab, field::COLOR), red());
    fail_spawns(&mut app);
    assert_eq!(app.mirror.color(&tab), Some(egui::Color32::RED));
    assert_eq!(stored(&app, &tab, field::COLOR), red());
}

/// A respawn removes the node from `nodes` until it lands; a save meanwhile keeps its mirror entry, so an edit
/// made then survives the landing (`Mirror::init` writes only an id with no entry).
#[test]
fn a_save_during_a_respawn_keeps_the_mirror_entry() {
    let path = temp_dir("edit-respawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", "A"));
    app.edit_field(&tab, field::COLOR, red());
    app.edit_field(&tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    // What `finish_swap` and the crash path do: carry from the mirror, remove the node, respawn.
    app.nodes.remove(&tab).unwrap();
    let mut spawn = crate::TabSpawn::new(
        tab.clone(),
        "https://a/".into(),
        EngineKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Respawn,
        1.0,
    );
    (spawn.carry.color, spawn.carry.pinned) = (app.mirror.color(&tab), app.mirror.pinned(&tab));
    app.respawn_tab(spawn);
    assert!(app.save_session());
    assert!(app.mirror.pinned(&tab) && app.mirror.color(&tab) == Some(egui::Color32::RED), "kept while parked");
    app.edit_field(&tab, field::COLOR, blue());
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert_eq!(app.mirror.color(&tab), Some(egui::Color32::BLUE));
    assert!(app.mirror.pinned(&tab));
    assert_eq!(stored(&app, &tab, field::COLOR), blue());
    assert_eq!(stored(&app, &tab, field::PINNED), FieldValue::Bool(true));
}

#[test]
fn a_failed_commit_keeps_a_pending_edit_for_the_retry() {
    let path = temp_dir("edit-fail").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.edit_field(&note, field::COLOR, red());
    let set_readonly = |readonly: bool| {
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        // The Windows read-only attribute makes the append fail; on Unix this would make the file writable by
        // everyone, but it's a temp file this test owns.
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(readonly);
        std::fs::set_permissions(&path, perms).unwrap();
    };
    set_readonly(true);
    let saved = app.save_session();
    set_readonly(false);
    assert!(!saved);
    assert!(!app.pending.is_empty(), "kept for the next save");
    assert_eq!(stored(&app, &note, field::COLOR), FieldValue::Color(None));
    app.save_retry_at = Some(std::time::Instant::now());
    app.poll_store();
    assert!(app.save_retry_at.is_none());
    assert!(app.pending.is_empty());
    assert_eq!(stored(&app, &note, field::COLOR), red());
}

/// A parent edit shows at once and is committed by the next save.
#[test]
fn a_parent_edit_reads_back_and_commits() {
    let path = temp_dir("parent-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
    );
    assert!(app.save_session());
    app.edit_parent(&b, Some(a.clone()));
    assert_eq!(app.mirror.parent(&b), Some(a.as_str()));
    assert_eq!(stored(&app, &b, field::PARENT), FieldValue::Id(None), "not written before the save");
    assert!(app.save_session());
    assert_eq!(stored(&app, &b, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}

/// An unsaved local parent beats another replica's edit to the same link; their other fields still arrive.
#[test]
fn a_local_parent_edit_wins_over_a_remote_one() {
    let path = temp_dir("parent-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b, c) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://c/", "")),
    );
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_parent(&c, Some(a.clone()));
    remote_set(&mut other, &c, field::PARENT, FieldValue::Id(Some(b.parse().unwrap())));
    assert!(app.save_session());
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()));
    assert_eq!(stored(&app, &c, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}

/// A parent that is deleted in the same commit is written as none (spec section 6 item 4).
#[test]
fn a_parent_deleted_by_the_same_commit_is_written_as_none() {
    let path = temp_dir("parent-deleted").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
    );
    assert!(app.save_session());
    app.edit_parent(&b, Some(a.clone()));
    app.remove_tab(&a); // queues its Delete
    assert!(app.save_session());
    assert_eq!(stored(&app, &b, field::PARENT), FieldValue::Id(None));
}

/// Hand-built ids only touch the mirror.
#[test]
fn a_parent_edit_on_a_non_store_id_queues_nothing() {
    let mut app = App::default();
    app.edit_parent("c", Some("p".into()));
    assert_eq!(app.mirror.parent("c"), Some("p"));
    assert!(app.pending.is_empty());
}

/// A remote parent change reaches the mirror; a node removal re-parents children through the writer.
#[test]
fn remote_parent_change_and_removal_reparent_through_the_mirror() {
    let path = temp_dir("parent-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b, c) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://c/", "")),
    );
    app.edit_parent(&b, Some(a.clone()));
    app.edit_parent(&c, Some(b.clone()));
    assert!(app.save_session());
    app.remove_node(&b);
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()), "children pass up to the removed node's parent");
    assert!(app.save_session());
    assert_eq!(stored(&app, &c, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}

/// A remote parent parked for a waking tab (`remote_in_flight`) must not land over a local edit committed after it.
#[test]
fn a_committed_parent_edit_wins_over_a_remote_value_parked_for_a_waking_tab() {
    let path = temp_dir("parent-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b, tab) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://t/", "")),
    );
    assert!(app.save_session());
    app.focus_tab(&tab);
    assert!(app.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::PARENT, FieldValue::Id(Some(b.parse().unwrap())));
    app.edit_parent(&tab, Some(a.clone()));
    assert!(app.save_session());
    assert!(app.remote_in_flight.contains_key(&tab), "the remote value is parked");
    fail_spawns(&mut app);
    assert_eq!(app.mirror.parent(&tab), Some(a.as_str()));
    assert_eq!(stored(&app, &tab, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}

/// A later parent edit to a target that isn't a store id drops the earlier queued one, so the stale target is not
/// committed.
#[test]
fn a_parent_edit_to_a_non_store_target_drops_an_earlier_queued_one() {
    let path = temp_dir("parent-stale").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, b) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
    );
    assert!(app.save_session());
    app.edit_parent(&b, Some(a.clone()));
    app.edit_parent(&b, Some("p".into()));
    assert_eq!(app.mirror.parent(&b), Some("p"));
    assert!(app.pending.is_empty());
    assert!(app.save_session());
    assert_eq!(stored(&app, &b, field::PARENT), FieldValue::Id(None));
}

/// A parent target created by the same commit is kept (Ruling 5).
#[test]
fn a_parent_created_by_the_same_commit_is_kept() {
    let path = temp_dir("parent-new").join("session.mnf");
    let mut app = app_on(&path, 1);
    let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", ""));
    assert!(app.save_session());
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    app.edit_parent(&b, Some(a.clone()));
    assert!(app.save_session());
    assert_eq!(stored(&app, &b, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}

/// A pending parent edit or move for a node the document no longer shows live (another replica deleted it) is
/// dropped: a `Set` or `Move` would make a placeholder.
#[test]
fn a_pending_link_edit_for_a_node_deleted_elsewhere_is_dropped() {
    let path = temp_dir("link-dropped").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let (a, b, c) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://c/", "")),
    );
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let deletes = vec![OpBody::Delete { node: b.parse().unwrap() }, OpBody::Delete { node: c.parse().unwrap() }];
    other.begin().unwrap().commit(deletes).unwrap();
    app.edit_parent(&b, Some(a.clone()));
    app.edit_group(&c, Some(g.clone()));
    let ops_before = app.journal.as_ref().unwrap().doc().version[&ActorId(1)];
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.version[&ActorId(1)], ops_before, "nothing written");
    assert!(!doc.nodes[&b.parse().unwrap()].is_live() && !doc.nodes[&c.parse().unwrap()].is_live());
    assert!(app.pending.is_empty() && app.pending_moves.is_empty());
}

/// A parked remote value for a waking tab is patched with what the commit wrote: a parent or group the commit
/// deleted lands as none, not as the dead id.
#[test]
fn a_parked_link_is_patched_with_the_resolved_value() {
    let path = temp_dir("link-parked-gone").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let (a, b, tab) = (
        add(&mut app, Kind::Tab, sleeping_tab("https://a/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://b/", "")),
        add(&mut app, Kind::Tab, sleeping_tab("https://t/", "")),
    );
    assert!(app.save_session());
    app.focus_tab(&tab);
    assert!(app.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::PARENT, FieldValue::Id(Some(b.parse().unwrap())));
    app.edit_parent(&tab, Some(a.clone()));
    app.edit_group(&tab, Some(g.clone()));
    app.remove_tab(&a);
    app.dissolve_group(&g);
    assert!(app.save_session());
    assert!(app.remote_in_flight.contains_key(&tab), "the remote value is parked");
    assert_eq!(stored(&app, &tab, field::PARENT), FieldValue::Id(None));
    fail_spawns(&mut app);
    assert_eq!(app.mirror.parent(&tab), None);
    assert_eq!(app.mirror.group(&tab), None);
}

#[test]
fn a_group_edit_reads_back_and_commits_as_a_move() {
    let path = temp_dir("group-edit").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.edit_group(&t, Some(g.clone()));
    assert_eq!(app.mirror.group(&t), Some(g.as_str()));
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), None);
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), Some(g.parse().unwrap()));
}

/// A group created in the same commit is the target of a move that follows its Create.
#[test]
fn a_move_into_a_group_created_by_the_same_save_follows_the_create() {
    let path = temp_dir("group-new").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    let g = add_group(&mut app, "G");
    app.edit_group(&t, Some(g.clone()));
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), Some(g.parse().unwrap()));
    assert_eq!(app.mirror.group(&t), Some(g.as_str()), "the mirror shows what the document holds");
}

/// A move into a group the same commit deletes lands at the top level (spec section 6 item 4).
#[test]
fn a_move_into_a_group_deleted_by_the_same_save_is_written_as_none() {
    let path = temp_dir("group-gone").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.edit_group(&t, Some(g.clone()));
    app.dissolve_group(&g);
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), None);
    assert_eq!(app.mirror.group(&t), None, "the mirror shows what the commit wrote");
}

#[test]
fn a_group_edit_on_a_non_store_id_queues_nothing() {
    let mut app = App::default();
    app.edit_group("t", Some("g".into()));
    assert_eq!(app.mirror.group("t"), Some("g"));
    assert!(app.pending_moves.is_empty());
}

/// A later move to a target that isn't a store id drops the earlier queued one.
#[test]
fn a_group_edit_to_a_non_store_target_drops_an_earlier_queued_one() {
    let path = temp_dir("group-stale").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.edit_group(&t, Some(g.clone()));
    app.edit_group(&t, Some("x".into()));
    assert!(app.pending_moves.is_empty());
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), None);
}

/// An unsaved local move beats another replica's move of the same node.
#[test]
fn a_local_group_move_wins_over_a_remote_one() {
    let path = temp_dir("group-conflict").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g1 = add_group(&mut app, "A");
    let g2 = add_group(&mut app, "B");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_group(&t, Some(g1.clone()));
    other
        .begin()
        .unwrap()
        .commit(vec![OpBody::Move { node: t.parse().unwrap(), group: Some(g2.parse().unwrap()) }])
        .unwrap();
    assert!(app.save_session());
    assert_eq!(app.mirror.group(&t), Some(g1.as_str()));
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), Some(g1.parse().unwrap()));
}

/// Another replica's move reaches the mirror when nothing is pending locally.
#[test]
fn a_remote_group_move_reaches_the_mirror() {
    let path = temp_dir("group-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let mv = vec![OpBody::Move { node: t.parse().unwrap(), group: Some(g.parse().unwrap()) }];
    other.begin().unwrap().commit(mv).unwrap();
    assert!(app.save_session());
    assert_eq!(app.mirror.group(&t), Some(g.as_str()));
}

/// A remote move parked for a waking tab must not land over a local move committed after it.
#[test]
fn a_committed_move_wins_over_a_remote_move_parked_for_a_waking_tab() {
    let path = temp_dir("group-parked").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g1 = add_group(&mut app, "A");
    let g2 = add_group(&mut app, "B");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.focus_tab(&tab);
    assert!(app.spawning.contains_key(&tab), "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let mv = vec![OpBody::Move { node: tab.parse().unwrap(), group: Some(g2.parse().unwrap()) }];
    other.begin().unwrap().commit(mv).unwrap();
    app.edit_group(&tab, Some(g1.clone()));
    assert!(app.save_session());
    assert!(app.remote_in_flight.contains_key(&tab), "the remote value is parked");
    fail_spawns(&mut app);
    assert_eq!(app.mirror.group(&tab), Some(g1.as_str()));
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(tab.parse().unwrap()), Some(g1.parse().unwrap()));
}

/// Dissolving a group moves its members up through the writer and the store agrees after a save.
#[test]
fn dissolving_a_group_moves_members_up_and_saves() {
    let path = temp_dir("group-dissolve").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "O");
    let inner = add_group(&mut app, "I");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_group(&inner, Some(outer.clone()));
    app.edit_group(&t, Some(inner.clone()));
    assert!(app.save_session());
    app.dissolve_group(&inner);
    assert_eq!(app.mirror.group(&t), Some(outer.as_str()));
    assert!(app.save_session());
    assert_eq!(app.journal.as_ref().unwrap().doc().effective_group(t.parse().unwrap()), Some(outer.parse().unwrap()));
}

/// Another replica merges G into T (`merge_groups`: `Delete G` and `Move(M, T)`). Applying G's delete first must not
/// queue a move of M that reverts the merge: the remote cascade only updates the mirror.
#[test]
fn a_remote_merge_is_not_reverted_by_the_dissolve_it_triggers() {
    let path = temp_dir("group-remote-merge").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = add_group(&mut app, "T");
    let g = add_group(&mut app, "G");
    let m = add(&mut app, Kind::Tab, sleeping_tab("https://m/", ""));
    assert!(g.parse::<NodeId>().unwrap() < m.parse::<NodeId>().unwrap(), "G's delete is applied before M's move");
    app.edit_group(&m, Some(g.clone()));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let merge = vec![
        OpBody::Delete { node: g.parse().unwrap() },
        OpBody::Move { node: m.parse().unwrap(), group: Some(t.parse().unwrap()) },
    ];
    other.begin().unwrap().commit(merge).unwrap();
    for _ in 0..2 {
        assert!(app.save_session());
        assert_eq!(app.mirror.group(&m), Some(t.as_str()));
        let doc = app.journal.as_ref().unwrap().doc();
        assert_eq!(doc.effective_group(m.parse().unwrap()), Some(t.parse().unwrap()));
    }
}

/// Another replica deletes P and re-parents P's child C to A. Applying P's delete first must not queue a parent edit
/// of C that reverts the re-parent.
#[test]
fn a_remote_reparent_is_not_reverted_by_the_removal_it_races() {
    let path = temp_dir("parent-remote-race").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    let p = add(&mut app, Kind::Tab, sleeping_tab("https://p/", ""));
    let c = add(&mut app, Kind::Tab, sleeping_tab("https://c/", ""));
    app.edit_parent(&c, Some(p.clone()));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let ops = vec![
        OpBody::Delete { node: p.parse().unwrap() },
        OpBody::Set {
            node: c.parse().unwrap(),
            field: field::PARENT,
            enc: encode_field(field::PARENT, &FieldValue::Id(Some(a.parse().unwrap()))),
        },
    ];
    other.begin().unwrap().commit(ops).unwrap();
    for _ in 0..2 {
        assert!(app.save_session());
        assert_eq!(app.mirror.parent(&c), Some(a.as_str()));
        assert_eq!(stored(&app, &c, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
    }
}

/// A local move into a group another replica then deletes still commits, to the deleted group's own group.
#[test]
fn a_pending_move_into_a_remotely_deleted_group_lands_in_its_group() {
    let path = temp_dir("group-pending-remote-delete").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "O");
    let g = add_group(&mut app, "G");
    let m = add(&mut app, Kind::Tab, sleeping_tab("https://m/", ""));
    app.edit_group(&g, Some(outer.clone()));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: g.parse().unwrap() }]).unwrap();
    app.edit_group(&m, Some(g.clone()));
    assert!(app.save_session());
    assert_eq!(app.mirror.group(&m), Some(outer.as_str()));
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(m.parse().unwrap()), Some(outer.parse().unwrap()));
}

/// A local move the store refuses as a cycle (another replica nested the target into the moved group first) leaves
/// the document acyclic, and the save still succeeds. The mirror shows what the document holds, not the refused
/// target: it would otherwise hold the cycle the store refused, indefinitely.
#[test]
fn a_move_the_store_refuses_as_a_cycle_leaves_the_document_acyclic() {
    let path = temp_dir("group-cycle").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = add_group(&mut app, "A");
    let b = add_group(&mut app, "B");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    let nest = vec![OpBody::Move { node: b.parse().unwrap(), group: Some(a.parse().unwrap()) }];
    other.begin().unwrap().commit(nest).unwrap();
    app.edit_group(&a, Some(b.clone()));
    assert!(app.save_session());
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(b.parse().unwrap()), Some(a.parse().unwrap()), "the remote move stands");
    assert_eq!(doc.effective_group(a.parse().unwrap()), None, "the local move was refused");
    for _ in 0..2 {
        assert_eq!(app.mirror.group(&a), None, "the mirror shows the refused move as the document does");
        assert_eq!(app.mirror.group(&b), Some(a.as_str()), "and the remote move");
        assert!(app.save_session());
    }
}

/// What the crash path and `finish_swap` do: carry from the mirror, take the node out of `nodes`, respawn. The
/// spawn carries no position: the tab keeps its `App::layout` entry.
fn start_respawn(app: &mut App, tab: &str) {
    app.nodes.remove(tab).unwrap();
    let carried = app.mirror.carried(tab);
    let mut spawn = crate::TabSpawn::new(
        tab.to_owned(),
        "https://t/".into(),
        EngineKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Respawn,
        1.0,
    );
    (spawn.parent, spawn.group, spawn.jar) = (carried.parent, carried.group, carried.jar);
    (spawn.carry.color, spawn.carry.pinned) = (carried.color, carried.pinned);
    app.respawn_tab(spawn);
    assert!(app.spawning.contains_key(tab) && !app.nodes.contains_key(tab), "respawning");
}

/// O holds G, G holds T, saved.
fn nested_tab(app: &mut App) -> (String, String, String) {
    let outer = add_group(app, "O");
    let g = add_group(app, "G");
    let t = add(app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_group(&g, Some(outer.clone()));
    app.edit_group(&t, Some(g.clone()));
    assert!(app.save_session());
    (outer, g, t)
}

/// Final review I1, remote: another replica deletes G while T respawns here. The cascade reaches T in the mirror
/// only, and its landing commits no move: T is in O on every replica, as the store shows it.
#[test]
fn a_remote_group_delete_during_a_respawn_lands_the_tab_in_its_group_without_a_move() {
    let path = temp_dir("respawn-remote-dissolve").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (outer, g, t) = nested_tab(&mut app);
    start_respawn(&mut app, &t);
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: g.parse().unwrap() }]).unwrap();
    assert!(app.save_session());
    assert_eq!(app.mirror.group(&t), Some(outer.as_str()), "the cascade reached the respawning tab");
    assert!(app.pending_moves.is_empty(), "a remote cascade queues nothing");
    fail_spawns(&mut app);
    assert_eq!(app.mirror.group(&t), Some(outer.as_str()));
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(t.parse().unwrap()), Some(outer.parse().unwrap()));
    assert_eq!(doc.nodes[&t.parse().unwrap()].group, Some(g.parse().unwrap()), "no move was committed");
}

/// Final review I1, local dissolve: a member mid-respawn moves up to the dissolved group's group.
#[test]
fn a_local_dissolve_during_a_respawn_moves_the_tab_up_a_level() {
    let path = temp_dir("respawn-local-dissolve").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (outer, g, t) = nested_tab(&mut app);
    start_respawn(&mut app, &t);
    app.dissolve_group(&g);
    assert_eq!(app.mirror.group(&t), Some(outer.as_str()));
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert_eq!(app.mirror.group(&t), Some(outer.as_str()));
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(t.parse().unwrap()), Some(outer.parse().unwrap()));
}

/// Final review I1, local merge: a member of the merged group mid-respawn lands in the target.
#[test]
fn a_local_merge_during_a_respawn_moves_the_tab_into_the_target() {
    let path = temp_dir("respawn-local-merge").join("session.mnf");
    let mut app = app_on(&path, 1);
    let source = add_group(&mut app, "S");
    let target = add_group(&mut app, "T");
    let m = add(&mut app, Kind::Tab, sleeping_tab("https://m/", ""));
    app.edit_group(&m, Some(source.clone()));
    assert!(app.save_session());
    start_respawn(&mut app, &m);
    app.merge_groups(&source, &target);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert_eq!(app.mirror.group(&m), Some(target.as_str()));
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(m.parse().unwrap()), Some(target.parse().unwrap()));
}

/// A opened P, P opened C, saved.
fn parent_chain(app: &mut App) -> (String, String, String) {
    let a = add(app, Kind::Tab, sleeping_tab("https://a/", ""));
    let p = add(app, Kind::Tab, sleeping_tab("https://p/", ""));
    let c = add(app, Kind::Tab, sleeping_tab("https://c/", ""));
    app.edit_parent(&p, Some(a.clone()));
    app.edit_parent(&c, Some(p.clone()));
    assert!(app.save_session());
    (a, p, c)
}

/// Final review I1, parent: removing P locally while its child C respawns passes C up to P's parent, and commits it.
#[test]
fn a_local_removal_during_a_respawn_reparents_the_tab() {
    let path = temp_dir("respawn-local-parent").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, p, c) = parent_chain(&mut app);
    start_respawn(&mut app, &c);
    app.remove_tab(&p);
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()));
    assert!(app.save_session());
    assert_eq!(stored(&app, &c, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
    fail_spawns(&mut app);
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()));
    assert_eq!(stored(&app, &c, field::PARENT), FieldValue::Id(Some(a.parse().unwrap())));
}

/// Final review I1, parent: another replica deleting P while C respawns here passes C up in the mirror only.
#[test]
fn a_remote_removal_during_a_respawn_reparents_the_tab_in_the_mirror_only() {
    let path = temp_dir("respawn-remote-parent").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (a, p, c) = parent_chain(&mut app);
    start_respawn(&mut app, &c);
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: p.parse().unwrap() }]).unwrap();
    assert!(app.save_session());
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()), "the cascade reached the respawning tab");
    assert!(app.pending.is_empty(), "a remote cascade queues nothing");
    fail_spawns(&mut app);
    assert_eq!(app.mirror.parent(&c), Some(a.as_str()));
}

/// The landing safety net (`seed_landing`): a new spawn has no mirror entry for the cascades to reach, so a group
/// dissolved while it was in flight is caught at landing. It lands where the document will show it once the
/// dissolve is saved (G's group), in the mirror only: no move is committed.
#[test]
fn a_new_spawn_into_a_group_dissolved_in_flight_lands_where_the_document_shows_it() {
    let path = temp_dir("spawn-dissolved").join("session.mnf");
    let mut app = app_on(&path, 1);
    let outer = add_group(&mut app, "O");
    let g = add_group(&mut app, "G");
    app.edit_group(&g, Some(outer.clone()));
    assert!(app.save_session());
    let t = app.alloc_node_id();
    let mut spawn = crate::TabSpawn::new(
        t.clone(),
        "https://t/".into(),
        EngineKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    spawn.group = Some(g.clone());
    app.spawn_tab_async(spawn);
    assert!(app.save_session(), "creates the spawning tab in G");
    app.dissolve_group(&g); // not saved before the landing
    fail_spawns(&mut app);
    assert_eq!(app.mirror.group(&t), Some(outer.as_str()));
    assert!(app.pending_moves.is_empty());
    let doc = app.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(t.parse().unwrap()), Some(outer.parse().unwrap()));
    assert_eq!(doc.nodes[&t.parse().unwrap()].group, Some(g.parse().unwrap()), "no move was committed");
}

// ---- 2b-3: the shared url in the mirror -------------------------------------

/// The page moves on (an engine report): the live url changes, the shared one doesn't.
fn set_live_url(app: &mut App, tab: &str, url: &str) {
    match &mut app.nodes.get_mut(tab).unwrap().runtime {
        Runtime::Tab(t) => t.url = url.into(),
        Runtime::Unloaded(t) => t.url = url.into(),
        _ => panic!("{tab} is not a tab"),
    }
}

/// Ruling 4: a navigation is saved as it started (`navigate_tab` calls `edit_url` with the typed url), so a redirect
/// the page makes before the save stays the live url only.
#[test]
fn a_navigation_saves_the_url_it_started_with_not_the_redirect() {
    let path = temp_dir("url-redirect").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    app.edit_url(&tab, "https://a/");
    set_live_url(&mut app, &tab, "https://b/");
    assert_eq!(app.mirror.url(&tab), "https://a/", "shown at once");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://a/".into()));
    assert_eq!(app.mirror.url(&tab), "https://a/");
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://b/", "the live url is the page's");
}

/// Ruling 3: an unsaved local url beats another replica's edit to it; their other fields still arrive. The tab is not
/// moved to their url (a live tab is sent no `Navigate`: `update_live_node` uses the one verdict for both).
#[test]
fn a_local_url_edit_wins_over_a_remote_one() {
    let path = temp_dir("url-local-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.edit_url(&tab, "https://mine/");
    set_live_url(&mut app, &tab, "https://mine/");
    remote_set(&mut other, &tab, field::URL, FieldValue::Str("https://theirs/".into()));
    remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(true));
    // The remote round applies, then the next round commits.
    assert!(app.save_session());
    assert_eq!(app.mirror.url(&tab), "https://mine/");
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://mine/".into()));
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://mine/", "not moved to theirs");
    assert!(app.mirror.pinned(&tab), "their other field arrives");
    assert!(app.test_spawns.is_empty());
}

/// Another replica's url reaches the mirror and moves a sleeping tab, and the next save writes nothing back.
#[test]
fn a_remote_url_change_reaches_the_mirror_and_moves_a_sleeping_tab() {
    let path = temp_dir("url-remote").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::URL, FieldValue::Str("https://moved/".into()));
    let url_stamp = |doc: &Doc| doc.nodes[&tab.parse().unwrap()].fields[&field::URL].stamp;
    let theirs = url_stamp(other.doc());
    assert!(app.save_session());
    assert_eq!(app.mirror.url(&tab), "https://moved/");
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://moved/");
    assert!(app.save_session());
    let ours = url_stamp(app.journal.as_ref().unwrap().doc());
    assert_eq!(ours, theirs, "nothing written back: the register is still the remote write");
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://moved/".into()));
}

/// A url the page moved to without sharing it is never saved: not by a save, not after a restore reopens the tab at
/// it (`LiveUrl`) and its wake lands.
#[test]
fn an_unmarked_live_url_is_never_saved() {
    let path = temp_dir("url-unmarked").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
        assert!(app.save_session());
        set_live_url(&mut app, &tab, "https://s/moved");
        assert!(app.save_session());
        assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://s/".into()));
        assert_eq!(app.mirror.url(&tab), "https://s/");
        app.maybe_write_local(true);
        app.close_store();
        tab
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().url, "https://s/moved", "reopened at its live url");
    assert_eq!(app.mirror.url(&tab), "https://s/", "the mirror shows the shared one");
    fail_spawns(&mut app);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://s/".into()));
}

/// A tab whose spawn hasn't landed is created with the url it starts at, and lands with it in the mirror.
#[test]
fn a_spawning_tab_is_created_and_lands_with_its_url() {
    let path = temp_dir("url-spawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = app.alloc_node_id();
    let spawn = crate::TabSpawn::new(
        t.clone(),
        "https://t/".into(),
        EngineKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    app.spawn_tab_async(spawn);
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::URL), FieldValue::Str("https://t/".into()));
    fail_spawns(&mut app);
    assert_eq!(app.mirror.url(&t), "https://t/", "the failed card lands with it");
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::URL), FieldValue::Str("https://t/".into()));
}

// ---- 2b-3: jar and jar_set in the mirror ------------------------------------

use crate::jar::JarChoice;

/// Ruling 7: one writer for both fields, queuing only the one that changed. Re-picking the same jar queues nothing.
#[test]
fn a_jar_pick_saves_both_fields_and_only_the_changed_one() {
    let path = temp_dir("jar-pick").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let key = |no| (tab.parse::<NodeId>().unwrap(), no);
    app.set_tab_jar(&tab, Some(work));
    assert_eq!(app.mirror.jar_choice(&tab), JarChoice { jar: Some(work), set: true }, "shown at once");
    assert_eq!(app.pending.keys().copied().collect::<Vec<_>>(), [key(field::JAR), key(field::JAR_SET)]);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::JAR), FieldValue::Id(Some(work)));
    assert_eq!(stored(&app, &tab, field::JAR_SET), FieldValue::Bool(true));
    app.edit_jar(&tab, JarChoice { jar: Some(work), set: false });
    assert_eq!(app.pending.keys().copied().collect::<Vec<_>>(), [key(field::JAR_SET)], "only the changed field");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::JAR_SET), FieldValue::Bool(false));
    app.edit_jar(&tab, JarChoice { jar: Some(work), set: false });
    assert!(app.pending.is_empty(), "re-picking the same jar queues nothing");
}

/// Ruling 8: a pending `jar` is resolved at commit like `parent`: a jar this commit deletes is written as none, and
/// the mirror shows what was written.
#[test]
fn a_pending_jar_naming_a_jar_deleted_in_this_commit_is_written_as_none() {
    let path = temp_dir("jar-pending-deleted").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    app.set_tab_jar(&tab, Some(work));
    // Gone here without `delete_jar`'s cascade (which would clear the tab's field first): this commit deletes its node.
    app.forget_jar(work);
    app.queue_delete(&work.to_string());
    assert!(app.save_session());
    assert!(!app.journal.as_ref().unwrap().doc().nodes[&work].is_live(), "the jar's delete is in this commit");
    assert_eq!(stored(&app, &tab, field::JAR), FieldValue::Id(None));
    assert_eq!(stored(&app, &tab, field::JAR_SET), FieldValue::Bool(true));
    assert_eq!(app.mirror.jar_choice(&tab), JarChoice { jar: None, set: true }, "the mirror shows what was written");
}

/// Ruling 8, `Create`: a new tab naming a jar the same commit deletes is created with none; one naming a jar the same
/// commit creates keeps it.
#[test]
fn a_created_tab_naming_a_jar_deleted_in_this_commit_is_created_with_none() {
    let path = temp_dir("jar-create-deleted").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    assert!(app.save_session());
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    app.set_tab_jar(&tab, Some(work));
    app.forget_jar(work); // without `delete_jar`'s cascade, so the pending field still names it
    app.queue_delete(&work.to_string());
    let home = app.create_jar("Home");
    let other = add(&mut app, Kind::Tab, sleeping_tab("https://o/", ""));
    app.set_tab_jar(&other, Some(home));
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::JAR), FieldValue::Id(None), "created with none");
    assert_eq!(app.mirror.jar_choice(&tab).jar, None, "the mirror shows what was written");
    assert_eq!(stored(&app, &other, field::JAR), FieldValue::Id(Some(home)), "a jar created here is kept");
}

/// Ruling 8's accepted behavior change: a tab naming a jar another replica deleted keeps naming it through a save (it
/// was cleared by every save before). Its chip still says the jar is missing.
#[test]
fn a_dangling_remote_deleted_jar_survives_a_save() {
    let path = temp_dir("jar-dangling").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    app.set_tab_jar(&tab, Some(work));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: work }]).unwrap();
    assert!(app.save_session());
    assert!(!app.mirror.jar_defs().contains_key(&work), "the remote delete arrived");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::JAR), FieldValue::Id(Some(work)), "not cleared by a save");
    assert_eq!(app.mirror.jar_choice(&tab).jar, Some(work));
    assert!(app.jar_chip(&tab).is_some_and(|c| c.missing), "the chip says the jar is missing");
}

/// Ruling 3: an unsaved local jar pick beats another replica's; their other fields still arrive.
#[test]
fn a_local_jar_pick_wins_over_a_remote_one() {
    let path = temp_dir("jar-local-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (mine, theirs) = (app.create_jar("Mine"), app.create_jar("Theirs"));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.set_tab_jar(&tab, Some(mine));
    remote_set(&mut other, &tab, field::JAR, FieldValue::Id(Some(theirs)));
    remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    assert_eq!(app.mirror.jar_choice(&tab), JarChoice { jar: Some(mine), set: true });
    assert_eq!(stored(&app, &tab, field::JAR), FieldValue::Id(Some(mine)));
    assert!(app.mirror.pinned(&tab), "their other field arrives");
}

/// Ruling 9 (2b-2's parked item): `refollow` reaches a tab mid-respawn, so a local dissolve re-snapshots its jar
/// from the level it moves to, and it lands with that jar.
#[test]
fn a_respawning_tab_follows_a_local_dissolve_s_jar_snapshot() {
    let path = temp_dir("jar-respawn-dissolve").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    let (_outer, g, t) = nested_tab(&mut app);
    assert_eq!(app.set_group_default_jar(&g, work), 1);
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(Some(work)));
    start_respawn(&mut app, &t);
    app.dissolve_group(&g); // O has no default: T follows it to none
    assert_eq!(app.mirror.jar_choice(&t), JarChoice::default());
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None));
    fail_spawns(&mut app);
    assert_eq!(app.mirror.jar_choice(&t), JarChoice::default(), "it lands with it");
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None));
}

/// A new tab is created with the jar it starts with and lands with it in the mirror. A cascade meanwhile does not
/// reach it (it has no mirror entry until it lands, as before 2b-3), so it never gives it a partial entry that would
/// stop the landing from seeding its colour; the tab keeps the jar it started with.
#[test]
fn a_spawning_tab_is_created_and_lands_with_its_jar() {
    let path = temp_dir("jar-spawn").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
    let g = add_group(&mut app, "G");
    app.edit_default_jar(&g, Some(work));
    assert!(app.save_session());
    // A restored tab (a failed `New` spawn never lands), started with G's default as `spawn_tab_at` gives one.
    let t = app.alloc_node_id();
    let mut spawn = crate::TabSpawn::new(
        t.clone(),
        "https://t/".into(),
        EngineKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    (spawn.group, spawn.jar) = (Some(g.clone()), app.new_tab_jar(Some(&g)));
    spawn.carry.color = Some(egui::Color32::RED);
    app.spawn_tab_async(spawn);
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(Some(work)), "created with its jar");
    assert!(!app.linked_ids().any(|id| id == t) && app.tab_jar(&t).is_none(), "out of the cascades' reach");
    app.set_group_default_jar(&g, home);
    assert!(!app.mirror.has(&t), "no partial entry");
    fail_spawns(&mut app);
    assert_eq!(app.mirror.jar_choice(&t), JarChoice { jar: Some(work), set: false }, "lands with it");
    assert_eq!(app.mirror.color(&t), Some(egui::Color32::RED), "the landing seeded the whole entry");
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(Some(work)));
}

// ---- 2b-3: the engine in the mirror -----------------------------------------

type Peer = ipc_protocol::MessageChannel<std::net::TcpStream>;

/// A live tab on `kind` (a fake host, `EngineTab::fake`), as a landing inserts it. The peer reads what it is sent.
fn live_tab(app: &mut App, kind: EngineKind, url: &str) -> (String, Peer) {
    let (tab, peer) = crate::process_manager::EngineTab::fake(kind);
    let payload = Runtime::Tab(crate::node::TabPayload {
        tab,
        url: url.into(),
        title: String::new(),
        can_go_back: false,
        can_go_forward: false,
        loading: false,
        audio_playing: false,
        muted: false,
        zoom_factor: 1.0,
        swap_target: None,
    });
    (add(app, Kind::Tab, payload), peer)
}

/// What the app sent the fake host so far.
fn sent(peer: &mut Peer) -> Vec<HostToEngine> {
    std::iter::from_fn(|| peer.recv().ok()).collect()
}

fn engine(kind: EngineKind) -> FieldValue {
    FieldValue::Engine(engine_code(kind))
}

fn swap_target(app: &App, tab: &str) -> Option<EngineKind> {
    app.nodes[tab].as_tab().unwrap().swap_target
}

/// Ruling 5: a swap queues its target when it starts (`start_swap_to`), so a save before the old engine's state
/// arrives writes the target (as `swap_target.unwrap_or(kind)` did before), while the old engine still runs.
#[test]
fn a_swap_saves_its_target_before_the_state_snapshot() {
    let path = temp_dir("engine-swap-start").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, EngineKind::Servo, "https://s/");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Servo));
    app.start_swap_to(&tab, EngineKind::Cef);
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef, "shown at once");
    assert!(app.pending.contains_key(&(tab.parse().unwrap(), field::ENGINE)), "queued when the swap starts");
    assert!(sent(&mut peer).iter().any(|m| matches!(m, HostToEngine::GetState)), "the swap asked for the state");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Cef));
    assert_eq!(app.nodes[&tab].as_tab().unwrap().tab.kind, EngineKind::Servo, "the old engine still runs");
    assert_eq!(swap_target(&app, &tab), Some(EngineKind::Cef));
}

/// Ruling 5: a switch whose new engine fails to start falls back to the old one, and writes it back when the
/// fallback starts; a fallback that fails too leaves a failed card on the old engine.
#[test]
fn a_failed_swap_writes_the_old_engine_back() {
    let path = temp_dir("engine-swap-fails").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, EngineKind::Servo, "https://s/");
    app.start_swap_to(&tab, EngineKind::Cef);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Cef));
    // The old engine's state arrived: its host is dropped and the new one spawns.
    app.finish_swap(&tab, EngineKind::Cef, "https://s/".into());
    assert_eq!(app.test_spawns.len(), 1);
    fail_spawns(&mut app);
    let fallback = &app.test_spawns.first().expect("the fallback is in flight").0;
    assert_eq!((fallback.kind, fallback.fallback_kind), (EngineKind::Servo, None));
    assert_eq!(app.mirror.engine(&tab), EngineKind::Servo);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Servo), "written back as the fallback starts");
    fail_spawns(&mut app);
    assert!(matches!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Failed(_)));
    assert_eq!(app.mirror.engine(&tab), EngineKind::Servo);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Servo));
}

/// Ruling 3: another replica's engine reaches the mirror and switches a live tab to it; nothing is written back.
#[test]
fn a_remote_engine_change_on_a_live_tab_starts_a_swap() {
    let path = temp_dir("engine-remote-live").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, EngineKind::Servo, "https://s/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::ENGINE, engine(EngineKind::Cef));
    assert!(app.save_session());
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef);
    assert_eq!(swap_target(&app, &tab), Some(EngineKind::Cef), "the swap started");
    assert!(sent(&mut peer).iter().any(|m| matches!(m, HostToEngine::GetState)));
    assert!(app.pending.is_empty(), "nothing queued");
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Cef));
}

/// Ruling 3: a swap in flight here beats another replica's engine, while its edit is unsaved (the mirror holds the
/// remote value back) and after it is saved (written again), so the document ends on the engine this tab lands on.
#[test]
fn a_local_swap_in_flight_wins_over_a_remote_engine() {
    let path = temp_dir("engine-local-wins").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, EngineKind::Servo, "https://s/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.start_swap_to(&tab, EngineKind::Cef);
    remote_set(&mut other, &tab, field::ENGINE, engine(EngineKind::Webview2));
    remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef);
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Cef));
    assert!(app.mirror.pinned(&tab), "their other field arrives");
    // Saved, and the old engine's state is still on its way: a later remote engine loses too.
    remote_set(&mut other, &tab, field::ENGINE, engine(EngineKind::Webview2));
    assert!(app.save_session());
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef);
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Cef), "written again");
    assert_eq!(swap_target(&app, &tab), Some(EngineKind::Cef), "still the one swap");
    let asked = sent(&mut peer).iter().filter(|m| matches!(m, HostToEngine::GetState)).count();
    assert_eq!(asked, 1, "no second swap started");
}

/// Ruling 5: waking a tab into another engine saves that engine when the wake starts, not when it lands. A wake
/// ignored because one is in flight changes nothing.
#[test]
fn a_wake_into_another_engine_is_saved_at_wake_start() {
    let path = temp_dir("engine-wake").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    app.wake_tab(&tab, EngineKind::Cef);
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef, "shown at once");
    assert_eq!(app.test_spawns[0].0.kind, EngineKind::Cef);
    app.wake_tab(&tab, EngineKind::Webview2);
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef, "an ignored wake changes nothing");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Cef), "saved while the wake is in flight");
    fail_spawns(&mut app);
    assert!(matches!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Failed(_)));
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef, "the failed card is on the engine that failed");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Cef));
}

/// Ruling 6: an engine code this build doesn't know (a newer build's) reads as Servo, and is not overwritten by a
/// save or by waking the tab on the engine it reads as.
#[test]
fn an_unknown_engine_code_survives_a_save() {
    let path = temp_dir("engine-unknown").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::ENGINE, FieldValue::Engine(99));
    assert!(app.save_session());
    assert_eq!(app.mirror.engine(&tab), EngineKind::Servo);
    app.edit_field(&tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), FieldValue::Engine(99), "not overwritten by a save");
    app.focus_tab(&tab);
    assert_eq!(app.test_spawns[0].0.kind, EngineKind::Servo, "wakes on the engine it reads as");
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), FieldValue::Engine(99), "nor by a wake");
}

/// Sleeping mid-swap is reachable: `request_sleep` doesn't wait for a swap, and the page state can answer before the
/// swap's snapshot (the sleep then drops `swap_target` with the live payload). The card keeps the target the mirror
/// holds: it saves it and wakes into it.
#[test]
fn a_tab_put_to_sleep_mid_swap_keeps_the_target() {
    let path = temp_dir("engine-sleep-mid-swap").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, EngineKind::Servo, "https://s/");
    assert!(app.save_session());
    app.start_swap_to(&tab, EngineKind::Cef);
    assert!(app.request_sleep(&tab, crate::sleep_state::SleepWhy::Manual), "a sleep is not refused mid-swap");
    app.finish_sleep(&tab, ipc_protocol::PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false });
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Sleeping, "asleep");
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Cef));
    app.focus_tab(&tab);
    assert_eq!(app.test_spawns[0].0.kind, EngineKind::Cef, "wakes into the target");
}

/// A crash mid-swap restarts the tab on the target the mirror holds, with the old engine as its fallback, so the
/// tab runs what the document says either way.
#[test]
fn a_crash_mid_swap_restarts_on_the_target_with_the_old_engine_as_fallback() {
    let path = temp_dir("engine-crash-mid-swap").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, EngineKind::Servo, "https://s/");
    assert!(app.save_session());
    app.start_swap_to(&tab, EngineKind::Cef);
    app.handle_engine_message(&tab, ipc_protocol::EngineToHost::Crashed { reason: "boom".into() });
    let spawn = &app.test_spawns.first().expect("restarting").0;
    assert_eq!((spawn.kind, spawn.fallback_kind), (EngineKind::Cef, Some(EngineKind::Servo)));
    fail_spawns(&mut app);
    assert_eq!(app.mirror.engine(&tab), EngineKind::Servo, "the target failed: back to the old engine");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Servo));
}

/// Lands every spawn in flight on a fake host of the kind it asked for. The peers keep the hosts' channels open.
fn land_spawns(app: &mut App) -> Vec<Peer> {
    let mut peers = Vec::new();
    for (spawn, generation) in std::mem::take(&mut app.test_spawns) {
        let (tab, peer) = crate::process_manager::EngineTab::fake(spawn.kind);
        app.handle_tab_spawn_result(crate::TabSpawnOutcome {
            spawn,
            generation,
            result: Ok(tab),
            seeded: Default::default(),
            jar: Default::default(),
            seed_clears: None,
        });
        peers.push(peer);
    }
    peers
}

/// Review I1: once the old engine's snapshot arrives, `swap_target` is gone but the swap is still in flight, parked
/// until the host's cookies arrive (`hold_for_cookies`). Neither a local pick nor another replica's engine starts a
/// second swap then (whose snapshot would never come once the first drops the host), and the tab lands on the engine
/// the document holds.
#[test]
fn a_swap_waiting_for_its_cookies_is_still_the_one_swap() {
    let path = temp_dir("engine-swap-cookie-hold").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, EngineKind::Cef, "https://s/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.start_swap_to(&tab, EngineKind::Webview2);
    assert!(app.save_session());
    let snapshot = ipc_protocol::TabState { url: "https://s/".into() };
    app.handle_engine_message(&tab, ipc_protocol::EngineToHost::StateSnapshot(snapshot));
    assert!(app.cookie_holds.contains_key(&tab), "parked for the cookies");
    assert_eq!(swap_target(&app, &tab), None);
    app.start_swap_to(&tab, EngineKind::Servo);
    assert_eq!(app.mirror.engine(&tab), EngineKind::Webview2, "a local pick is refused");
    remote_set(&mut other, &tab, field::ENGINE, engine(EngineKind::Servo));
    assert!(app.save_session());
    assert_eq!(swap_target(&app, &tab), None, "no second swap");
    let asked = sent(&mut peer).iter().filter(|m| matches!(m, HostToEngine::GetState)).count();
    assert_eq!(asked, 1, "the old host was asked once");
    assert_eq!(app.mirror.engine(&tab), EngineKind::Webview2, "the remote engine is written over");
    app.handle_engine_message(&tab, ipc_protocol::EngineToHost::Cookies("[]".into()));
    let spawn = &app.test_spawns.first().expect("the swap goes on").0;
    assert_eq!((spawn.kind, spawn.fallback_kind), (EngineKind::Webview2, Some(EngineKind::Cef)));
    let _peers = land_spawns(&mut app);
    assert_eq!(app.nodes[&tab].as_tab().unwrap().tab.kind, EngineKind::Webview2, "landed on the target");
    assert_eq!(app.mirror.engine(&tab), EngineKind::Webview2, "the document holds what runs");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Webview2));
}

/// Review M8: another replica's engine reaches a sleeping card through the mirror alone (nothing spawns, nothing is
/// queued), and the card wakes into it.
#[test]
fn a_remote_engine_change_on_a_sleeping_card_is_the_one_it_wakes_into() {
    let path = temp_dir("engine-remote-sleeping").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::ENGINE, engine(EngineKind::Cef));
    assert!(app.save_session());
    assert_eq!(app.mirror.engine(&tab), EngineKind::Cef);
    assert!(app.pending.is_empty() && app.test_spawns.is_empty(), "nothing queued or started");
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Sleeping);
    app.focus_tab(&tab);
    assert_eq!(app.test_spawns[0].0.kind, EngineKind::Cef, "wakes into it");
}

/// Review M8, Ruling 6: an unknown engine code survives a wake that lands live too: the live tab's save reads the
/// mirror's raw value, not the engine it runs.
#[test]
fn an_unknown_engine_code_survives_a_tab_landing_live() {
    let path = temp_dir("engine-unknown-live").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::ENGINE, FieldValue::Engine(99));
    assert!(app.save_session());
    app.focus_tab(&tab);
    let _peers = land_spawns(&mut app);
    assert_eq!(app.nodes[&tab].as_tab().expect("landed live").tab.kind, EngineKind::Servo);
    app.edit_field(&tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), FieldValue::Engine(99), "not overwritten by a live tab's save");
}

// ---- 2b-3 final review -------------------------------------------------------

/// Final review Minor 6: another replica's url reaches a live tab as a `Navigate`; a url held back by a pending
/// local edit sends none (the mirror refused it, 2b-3 Ruling 3).
#[test]
fn a_remote_url_navigates_a_live_tab_unless_a_local_url_is_pending() {
    let path = temp_dir("url-remote-live").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, EngineKind::Servo, "https://a/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::URL, FieldValue::Str("https://b/".into()));
    assert!(app.save_session());
    let navigated = |sent: &[HostToEngine], to: &str| {
        sent.iter().any(|m| matches!(m, HostToEngine::Navigate { url, .. } if url == to))
    };
    assert!(navigated(&sent(&mut peer), "https://b/"), "the live tab is sent there");
    assert_eq!(app.nodes[&tab].as_tab().unwrap().url, "https://b/");
    app.edit_url(&tab, "https://local/");
    remote_set(&mut other, &tab, field::URL, FieldValue::Str("https://c/".into()));
    assert!(app.save_session());
    let after = sent(&mut peer);
    assert!(!after.iter().any(|m| matches!(m, HostToEngine::Navigate { .. })), "held back: no Navigate, got {after:?}");
    assert_eq!(app.mirror.url(&tab), "https://local/");
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://local/".into()));
}

/// Final review I1, url: another replica's url, parked while the tab wakes (`remote_in_flight`), still navigates
/// it when it lands, though a commit in between (here an unrelated Delete) saw that url in the document: the mirror
/// keeps the url the tab had until the landing applies the parked change against it.
#[test]
fn a_remote_url_parked_during_a_wake_navigates_the_tab_when_it_lands() {
    let path = temp_dir("url-parked-wake").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    app.focus_tab(&tab);
    assert_eq!(app.test_spawns.len(), 1, "waking");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::URL, FieldValue::Str("https://r/".into()));
    assert!(app.save_session());
    assert!(app.remote_in_flight.contains_key(&tab), "parked for the landing");
    app.remove_node(&note);
    assert!(app.save_session());
    assert!(!app.journal.as_ref().unwrap().doc().nodes[&note.parse().unwrap()].is_live(), "the Delete committed");
    let mut peers = land_spawns(&mut app);
    let to_r =
        sent(&mut peers[0]).iter().any(|m| matches!(m, HostToEngine::Navigate { url, .. } if url == "https://r/"));
    assert!(to_r, "the landed tab is sent to the remote url");
    assert_eq!(app.mirror.url(&tab), "https://r/");
}

/// Final review I1, engine: another replica's engine, parked while a swap's respawn is in flight, still reaches
/// the tab when it lands, though a jar cascade (a group default, which reaches a respawning tab, Ruling 9) committed
/// a Set for the tab in between (the parked change takes the committed jar, so the landing does not replay the old
/// one over it). The document and the running engine end up agreeing: the landing starts a swap to it.
#[test]
fn a_remote_engine_parked_during_a_swap_respawn_is_swapped_to_when_it_lands() {
    let path = temp_dir("engine-parked-swap").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    let g = add_group(&mut app, "G");
    let (tab, _peer) = live_tab(&mut app, EngineKind::Servo, "https://s/");
    app.edit_group(&tab, Some(g.clone()));
    assert!(app.save_session());
    app.start_swap_to(&tab, EngineKind::Cef);
    assert!(app.save_session());
    let snapshot = ipc_protocol::TabState { url: "https://s/".into() };
    app.handle_engine_message(&tab, ipc_protocol::EngineToHost::StateSnapshot(snapshot));
    assert_eq!(app.test_spawns.len(), 1, "the swap's respawn is in flight");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &tab, field::ENGINE, engine(EngineKind::Webview2));
    assert!(app.save_session());
    assert!(app.remote_in_flight.contains_key(&tab), "parked for the landing");
    assert_eq!(app.set_group_default_jar(&g, work), 1, "the cascade reaches the respawning tab");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::JAR), FieldValue::Id(Some(work)), "the cascade committed for the tab");
    let _peers = land_spawns(&mut app);
    assert_eq!(app.nodes[&tab].as_tab().unwrap().tab.kind, EngineKind::Cef, "landed on the swap's target");
    assert_eq!(app.mirror.engine(&tab), EngineKind::Webview2);
    assert_eq!(app.swap_in_flight(&tab), Some(EngineKind::Webview2), "and swaps on to the remote engine");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::ENGINE), engine(EngineKind::Webview2));
}

/// A new tab spawned with `jar`, as `spawn_tab_at` starts one.
fn spawn_with_jar(app: &mut App, purpose: SpawnPurpose, jar: NodeId) -> String {
    let t = app.alloc_node_id();
    let mut spawn =
        crate::TabSpawn::new(t.clone(), "https://t/".into(), EngineKind::Servo, egui::Vec2::ZERO, purpose, 1.0);
    spawn.jar = JarChoice { jar: Some(jar), set: true };
    app.spawn_tab_async(spawn);
    t
}

/// Final review Minor 1 (Task 2 Minor 1), variant 1: a jar deleted while a new tab spawned with it is in flight,
/// before the tab's `Create` commits. The Create writes none; the tab lands showing none too, not the dead jar.
#[test]
fn a_new_tab_whose_jar_is_deleted_before_its_create_lands_with_none() {
    let path = temp_dir("jar-deleted-mid-spawn-1").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    assert!(app.save_session());
    let t = spawn_with_jar(&mut app, SpawnPurpose::New, work);
    app.delete_jar(work);
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None), "created with none");
    let _peers = land_spawns(&mut app);
    assert_eq!(app.mirror.jar_choice(&t).jar, None, "lands with none");
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None));
}

/// Final review Minor 1, variant 2: the tab's `Create` committed with the jar before it was deleted. The delete
/// could not reach the tab (no mirror entry yet), so the landing clears it, as `delete_jar` would have, and the
/// document does not keep the dead jar.
#[test]
fn a_new_tab_whose_jar_is_deleted_after_its_create_lands_with_none() {
    let path = temp_dir("jar-deleted-mid-spawn-2").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    assert!(app.save_session());
    let t = spawn_with_jar(&mut app, SpawnPurpose::Restore, work);
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(Some(work)), "created with the jar");
    app.delete_jar(work);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert_eq!(app.mirror.jar_choice(&t).jar, None, "the failed card lands with none");
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None), "and the document drops the dead jar");
}

/// Task 2 Minor 4: `delete_jar` writes none to the document for a saved tab, and reaches a tab mid-respawn (its
/// mirror entry outlives the respawn, Ruling 9), which lands still showing none.
#[test]
fn deleting_a_jar_writes_none_for_saved_and_respawning_tabs() {
    let path = temp_dir("jar-delete-doc").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    let asleep = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    let (live, _peer) = live_tab(&mut app, EngineKind::Servo, "https://l/");
    app.set_tab_jar(&asleep, Some(work));
    app.set_tab_jar(&live, Some(work));
    assert!(app.save_session());
    app.handle_engine_message(&live, ipc_protocol::EngineToHost::Crashed { reason: "boom".into() });
    assert!(app.spawning.contains_key(&live) && !app.nodes.contains_key(&live), "mid-respawn");
    app.delete_jar(work);
    assert!(app.save_session());
    for t in [&asleep, &live] {
        assert_eq!(stored(&app, t, field::JAR), FieldValue::Id(None));
        assert_eq!(app.mirror.jar_choice(t).jar, None);
    }
    let _peers = land_spawns(&mut app);
    assert_eq!(app.mirror.jar_choice(&live).jar, None, "the respawn lands with none");
}

// ---- 2b-5 Task 1: group, leaf and content fields in the mirror ---------------

/// A group's default jar is the mirror's: `set_group_default_jar` and `clear_group_default_jar` write it through
/// `edit_default_jar` and the save stores it; a remote change reaches the mirror and the jar new tabs in it start with.
#[test]
fn a_group_default_jar_saves_and_a_remote_change_reaches_it() {
    let path = temp_dir("default-jar-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
    let g = add_group(&mut app, "G");
    app.set_group_default_jar(&g, work);
    assert_eq!(app.mirror.default_jar(&g), Some(work));
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::DEFAULT_JAR), FieldValue::Id(Some(work)));
    app.clear_group_default_jar(&g);
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::DEFAULT_JAR), FieldValue::Id(None));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &g, field::DEFAULT_JAR, FieldValue::Id(Some(home)));
    assert!(app.save_session());
    assert_eq!(app.mirror.default_jar(&g), Some(home));
    assert_eq!(app.inherited_jar(Some(&g)), Some(home), "new tabs in it start in Home");
}

/// Ruling 6: a default jar deleted on another device stays stored (dangling, it reads as the default jar) instead of
/// being cleared by this device's next save.
#[test]
fn a_remote_deleted_default_jar_stays_stored() {
    let path = temp_dir("default-jar-dangling").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    let g = add_group(&mut app, "G");
    app.set_group_default_jar(&g, work);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: work }]).unwrap();
    assert!(app.save_session());
    assert!(!app.mirror.jar_defs().contains_key(&work), "the jar is gone here");
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::DEFAULT_JAR), FieldValue::Id(Some(work)), "not cleared");
    assert_eq!(app.mirror.default_jar(&g), Some(work));
}

/// Ruling 10: every way a node is made seeds its type in the mirror (`Mirror::ty`): placed here, landed from a spawn,
/// or materialized from the document.
#[test]
fn every_creation_path_seeds_the_node_type() {
    let ty = |kind: Kind| kind.type_str().unwrap();
    let mut app = App::default();
    let json =
        |leaf| Runtime::Json(JsonPayload { path: "d.json".into(), value: Default::default(), leaf, error: None });
    let placed = [
        (add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false })), Kind::Note),
        (add_group(&mut app, "G"), Kind::Group),
        (add(&mut app, Kind::Tab, sleeping_tab("https://a.test/", "A")), Kind::Tab),
        (add(&mut app, Kind::Json, json(false)), Kind::Json),
        (add(&mut app, Kind::JsonLeaf, json(true)), Kind::JsonLeaf),
        (add(&mut app, Kind::Pdf, Runtime::Pdf(PdfPayload { path: "d.pdf".into(), outcome: None })), Kind::Pdf),
        (
            add(
                &mut app,
                Kind::Image,
                Runtime::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None }),
            ),
            Kind::Image,
        ),
    ];
    for (id, kind) in &placed {
        assert_eq!(app.mirror.ty(id), ty(*kind), "{kind:?}");
    }

    // A tab that lands from a spawn (`seed_landing`).
    let t = app.alloc_node_id();
    let spawn = crate::TabSpawn::new(
        t.clone(),
        "https://t.test/".into(),
        EngineKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    app.spawn_tab_async(spawn);
    fail_spawns(&mut app);
    assert_eq!(app.mirror.ty(&t), ty(Kind::Tab), "a failed landing");
    let live = app.alloc_node_id();
    let spawn = crate::TabSpawn::new(
        live.clone(),
        "https://l.test/".into(),
        EngineKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    app.spawn_tab_async(spawn);
    let _peers = land_spawns(&mut app);
    assert!(app.nodes[&live].as_tab().is_some(), "landed live");
    assert_eq!(app.mirror.ty(&live), ty(Kind::Tab), "a live landing");

    // Materialized from the document.
    let leaf = [(field::KEY, FieldValue::Str("k".into())), (field::VALUE, FieldValue::Str("1".into()))];
    let nodes = BTreeMap::from([
        (id(1), shared(Kind::Group, None, &[(field::NAME, FieldValue::Str("G".into()))])),
        (id(2), shared(Kind::Note, None, &[(field::TEXT, FieldValue::Str("n".into()))])),
        (id(3), shared(Kind::Tab, None, &[(field::URL, FieldValue::Str("https://a.test/".into()))])),
        (id(4), shared(Kind::JsonLeaf, Some(id(1)), &leaf)),
    ]);
    let app = app_with(&nodes);
    for (id, node) in &nodes {
        assert_eq!(app.mirror.ty(&id.to_string()), ty(node.kind), "materialized {:?}", node.kind);
    }
}

/// Ruling 8: a leaf's key and stored value are the mirror's, so importing a group of leaves, closing it and reopening
/// it from Recently Closed all carry them.
#[test]
fn importing_or_reopening_a_group_of_leaves_carries_key_and_value() {
    let json = serde_json::json!({ "nodes": [
        json_node("g", Kind::Group, serde_json::json!({ "name": "G", "from_json": true })),
        json_node("l", Kind::JsonLeaf, serde_json::json!({ "group": "g", "path": "d.json", "key": "k", "value": "[1,2]" })),
    ]});
    let doc: JsonDoc = serde_json::from_value(json).unwrap();
    let mut app = App::default();
    app.import_group_json(&doc);
    let find = |app: &App| {
        let leaf = app.nodes.iter().find(|(_, n)| n.as_json().is_some_and(|j| j.leaf)).map(|(id, _)| id.clone());
        let group = app.nodes.keys().find(|id| app.is_container(id)).cloned();
        (group.expect("a group"), leaf.expect("a leaf"))
    };
    let (g, l) = find(&app);
    assert!(app.mirror.from_json(&g));
    assert_eq!((app.mirror.key(&l), app.mirror.value(&l)), ("k", "[1,2]"));
    assert_eq!(*app.nodes[&l].as_json().unwrap().value, serde_json::json!([1, 2]), "the payload holds it parsed");
    let shared = snapshots(&app);
    let fields = &shared[&l.parse().unwrap()].fields;
    assert_eq!(fields[&field::KEY], FieldValue::Str("k".into()));
    assert_eq!(fields[&field::VALUE], FieldValue::Str("[1,2]".into()));

    app.close_node(&g);
    assert!(app.nodes.is_empty(), "closed with its leaf");
    let entry = app.closed.pop().expect("recorded");
    app.reopen_closed(entry);
    let (g2, l2) = find(&app);
    assert_ne!(l2, l, "reopened with a fresh id");
    assert!(app.mirror.from_json(&g2));
    assert_eq!((app.mirror.key(&l2), app.mirror.value(&l2)), ("k", "[1,2]"));
}

/// A node's `Create` resolves its Id fields (`create_ops`'s `valid`): a link to a node deleted meanwhile is written as
/// none, and the mirror shows what the document holds, as after an edit (2b-5 Task 1 review).
#[test]
fn a_created_group_shows_the_default_jar_its_create_resolved() {
    let path = temp_dir("default-jar-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = app.create_jar("Work");
    assert!(app.save_session());
    let g = add_group(&mut app, "G");
    app.set_group_default_jar(&g, work);
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: work }]).unwrap();
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::DEFAULT_JAR), FieldValue::Id(None), "created without the deleted jar");
    assert_eq!(app.mirror.default_jar(&g), None, "the mirror shows what the document holds");
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::DEFAULT_JAR), FieldValue::Id(None));
}

/// The same for a created node's `parent`: an opener deleted elsewhere before the first save.
#[test]
fn a_created_note_shows_the_parent_its_create_resolved() {
    let path = temp_dir("parent-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
    assert!(app.save_session());
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    app.edit_parent(&note, Some(opener.clone()));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: opener.parse().unwrap() }]).unwrap();
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::PARENT), FieldValue::Id(None));
    assert_eq!(app.mirror.parent(&note), None, "the mirror shows what the document holds");
}

/// Spawn failures that share a cause reach the toast queue as one message once the window has passed.
#[test]
fn spawn_failures_of_one_cause_toast_once_with_a_count() {
    let mut app = app_on(&temp_dir("failure-toasts").join("session.mnf"), 1);
    for i in 0..3 {
        app.spawn_tab_at(&format!("https://t{i}/"), egui::Vec2::ZERO, None, None);
    }
    fail_spawns(&mut app);
    assert!(app.save_toast_queue.is_empty(), "held inside the window");
    for message in app.failure_notices.flush(std::time::Instant::now() + crate::failure_toasts::WINDOW) {
        app.queue_toast(message);
    }
    assert_eq!(app.save_toast_queue.iter().collect::<Vec<_>>(), ["3 tabs couldn't start: no engine"]);
}

fn flushed_failure_toasts(app: &mut App) -> Vec<String> {
    app.failure_notices.flush(std::time::Instant::now() + crate::failure_toasts::WINDOW)
}

/// A host that cannot start is a spawn failure like any other: two such crashes with one reason read as one message.
#[test]
fn cannot_start_crashes_with_one_reason_toast_once_with_a_count() {
    let mut app = app_on(&temp_dir("crash-toasts").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.leave_crashed_card(&a, "no engine", true);
    app.leave_crashed_card(&b, "no engine", true);
    assert!(app.save_toast_queue.is_empty(), "held inside the window");
    assert_eq!(flushed_failure_toasts(&mut app), ["2 tabs couldn't start: no engine"]);
}

/// A failed card's retry is `Waking`, so a Root/New failure of the same intent meanwhile lands as a second card;
/// when the wake fails too, one card stays: the retried one, with the refreshed error.
#[test]
fn a_failed_retry_and_a_new_failure_of_one_intent_leave_one_card() {
    let mut app = app_on(&temp_dir("failed-retry-twin").join("session.mnf"), 1);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    let card = failed_cards(&app).remove(0);
    let kind = app.mirror.engine(&card);
    app.wake_tab(&card, kind);
    assert_eq!(app.nodes[&card].as_unloaded().unwrap().state, UnloadedState::Waking);
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    let mut parked = std::mem::take(&mut app.test_spawns);
    assert_eq!(parked.len(), 2);
    let wake = parked.remove(parked.iter().position(|(s, _)| s.id == card).unwrap());
    fail_spawn(&mut app, parked.remove(0));
    fail_spawn(&mut app, wake);
    assert_eq!(failed_cards(&app), std::slice::from_ref(&card), "one card for one intent, the retried one");
    assert_eq!(app.nodes.len(), 1);
}

/// A live tab that crash-loops into a failed card beside a failed card for its own intent leaves one card: the
/// crashed tab's, which keeps its id; the other is recorded in Recently Closed.
#[test]
fn a_crashed_tab_beside_a_failed_card_for_its_intent_leaves_one_card() {
    let mut app = app_on(&temp_dir("crash-twin").join("session.mnf"), 1);
    let live = succeed_spawn(&mut app, "https://a/");
    app.spawn_tab_at("https://a/", egui::Vec2::ZERO, None, None);
    fail_spawns(&mut app);
    assert_eq!(failed_cards(&app).len(), 1, "setup: a failed card beside the live tab");
    app.leave_crashed_card(&live, "keeps crashing", false);
    assert_eq!(failed_cards(&app), std::slice::from_ref(&live));
    assert_eq!(app.closed.len(), 1, "the other card is recorded in Recently Closed");
}

/// A startup Root spawn that failed leaves a saved failed card; the next launch restores it as the focused tab and
/// tries it again, so a failed startup is retried without a Root spawn (a non-empty session never makes one).
#[test]
fn a_failed_startup_card_is_retried_at_the_next_launch() {
    let path = temp_dir("failed-startup-retry").join("session.mnf");
    let mut app = app_on(&path, 1);
    app.spawn_root_tab();
    fail_spawns(&mut app);
    let card = failed_cards(&app).remove(0);
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.focused.as_deref(), Some(card.as_str()));
    assert!(restored.test_spawns.iter().any(|(spawn, _)| spawn.id == card), "the card is woken");
}

/// A link is a stored node: it survives save and restore with both ends.
#[test]
fn a_link_survives_save_and_restore() {
    let path = temp_dir("link-roundtrip").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let link = app.create_link(&a, &b);
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.mirror.link_ends(&link), (a.parse().ok(), b.parse().ok()));
    assert_eq!(restored.links_naming(&a), vec![link]);
}

/// Deleting an endpoint deletes the links naming it (the save also runs `check_graph_matches_document`).
#[test]
fn removing_a_card_deletes_its_links() {
    let mut app = app_on(&temp_dir("link-delete").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let link = app.create_link(&a, &b);
    app.remove_tab(&a);
    assert!(app.links_naming(&b).is_empty());
    assert!(app.mirror.link_ids().all(|id| id.to_string() != link));
    assert!(app.save_session());
}

/// A remote delete of an endpoint hides and removes the link as well.
#[test]
fn a_remote_delete_of_an_endpoint_removes_its_links() {
    let mut app = app_on(&temp_dir("link-remote-delete").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.create_link(&a, &b);
    // What `remote_changes` reports when another replica deletes `a`
    // (as `remote_jar_create_rename_and_delete_reach_jar_defs`):
    app.apply_remote(vec![(a.parse().unwrap(), None)]);
    assert!(app.links_naming(&b).is_empty());
    assert!(app.mirror.link_ids().next().is_none());
}

/// Replacing the session drops every link from the mirror, a dangling one too, and the save still agrees with the
/// document (`check_graph_matches_document`).
#[test]
fn replacing_the_session_drops_its_links() {
    let mut app = app_on(&temp_dir("link-replace").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.create_link(&a, &b);
    app.create_link("not-a-card", "nor-this"); // dangling: no endpoint's cascade reaches it
    assert!(app.save_session());
    app.replace_running_session(&file_with_a_work_jar(), crate::store::json::Trust::Foreign);
    assert!(app.mirror.link_ids().next().is_none(), "no stale link");
    assert!(app.save_session());
}

/// A link keeps its ends in the mirror after a save (which retains only the graph's nodes), so a later delete of an
/// endpoint still finds it.
#[test]
fn a_saved_link_keeps_its_ends_and_is_found_by_a_later_delete() {
    let mut app = app_on(&temp_dir("link-retain").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let link = app.create_link(&a, &b);
    assert!(app.save_session());
    assert_eq!(app.mirror.link_ends(&link), (a.parse().ok(), b.parse().ok()));
    app.remove_tab(&a);
    assert!(app.links_naming(&b).is_empty());
    assert!(app.save_session());
}

use crate::caps::{LINK, OPENED_FROM};
use crate::links::ConnectError;

#[test]
fn connect_link_makes_one_link_and_refuses_a_repeat_in_either_order() {
    let mut app = app_on(&temp_dir("connect-link").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    assert_eq!(app.connect(LINK, &a, &b), Ok(()));
    assert_eq!(app.connect(LINK, &a, &b), Err(ConnectError::Exists));
    assert_eq!(app.connect(LINK, &b, &a), Err(ConnectError::Exists));
    assert_eq!(app.links_naming(&a).len(), 1);
    app.disconnect(LINK, &a, &b);
    assert!(app.links_naming(&a).is_empty());
}

#[test]
fn connect_opened_from_sets_the_parent_and_refuses_a_cycle() {
    let mut app = app_on(&temp_dir("connect-opener").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    assert_eq!(app.connect(OPENED_FROM, &b, &a), Ok(()));
    assert_eq!(app.mirror.parent(&b), Some(a.as_str()));
    assert_eq!(app.connect(OPENED_FROM, &a, &b), Err(ConnectError::Cycle));
    app.disconnect(OPENED_FROM, &b, &a);
    assert_eq!(app.mirror.parent(&b), None);
}

#[test]
fn a_link_is_a_level_edge_only_while_both_ends_are_live_at_this_level() {
    use crate::level::EdgeKind;
    let mut app = app_on(&temp_dir("link-level").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(LINK, &a, &b).unwrap();
    let edges = app.level_edges();
    assert_eq!(edges.iter().filter(|e| e.kind == EdgeKind::Link).count(), 1);
    assert!(edges.iter().all(|e| (e.kind == EdgeKind::Link) == (e.curve == crate::level::EdgeCurve::Straight)));
    assert!(edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Link)
        .all(|e| (e.from_id.as_str(), e.to_id.as_str()) == (a.as_str(), b.as_str()) && e.link.is_some()));
    app.edit_group(&b, Some("elsewhere".into()));
    assert!(app.level_edges().iter().all(|e| e.kind != EdgeKind::Link), "another level: not shown");
}

#[test]
fn links_never_feed_the_layout_springs() {
    let mut app = app_on(&temp_dir("link-springs").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(LINK, &a, &b).unwrap();
    assert!(app.spring_edges().is_empty());
    app.connect(OPENED_FROM, &b, &a).unwrap();
    assert_eq!(app.spring_edges(), vec![(a, b)]);
}

#[test]
fn connect_refuses_another_level_and_a_missing_card() {
    let mut app = app_on(&temp_dir("connect-level").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let g = add_group(&mut app, "g");
    app.edit_group(&b, Some(g));
    assert_eq!(app.connect(LINK, &a, &b), Err(ConnectError::DifferentLevel));
    assert_eq!(app.connect(LINK, &a, "nope"), Err(ConnectError::NotLive));
    assert_eq!(app.connect("contains", &a, &b), Err(ConnectError::NotAllowed), "not a user-drawn edge");
}

#[test]
fn edges_of_lists_the_opener_the_openees_and_the_links() {
    let mut app = app_on(&temp_dir("edges-of").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    app.connect(OPENED_FROM, &b, &a).unwrap();
    app.connect(LINK, &a, &c).unwrap();
    let of_a = app.edges_of(&a);
    assert!(of_a.iter().any(|e| e.edge == OPENED_FROM && e.other == b && !e.outgoing));
    assert!(of_a.iter().any(|e| e.edge == LINK && e.other == c && e.link.is_some()));
    let of_b = app.edges_of(&b);
    assert!(of_b.iter().any(|e| e.edge == OPENED_FROM && e.other == a && e.outgoing));
}

#[test]
fn valid_targets_are_the_cards_connect_would_accept() {
    let mut app = app_on(&temp_dir("valid-targets").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    app.connect(LINK, &a, &b).unwrap();
    assert_eq!(app.valid_targets(LINK, &a), vec![c.clone()], "not itself, not one it is linked to");
    app.connect(OPENED_FROM, &b, &a).unwrap();
    assert!(!app.valid_targets(OPENED_FROM, &a).contains(&b), "that would close a loop");
}

/// "Opened from..." on every kind the edge table allows as its source is saved, restored, and kept when another
/// replica edits the card (`mirror_remote` reapplies only a kind's fields). A kind added to the table's sources
/// must be added here.
#[test]
fn connect_opened_from_survives_a_restart_and_a_remote_edit_on_every_allowed_source_kind() {
    use crate::store::model::{TYPES, TYPE_TAB};
    let dir = temp_dir("opened-from-every-kind");
    let path = dir.join("session.mnf");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(1, 1).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png_path = dir.join("a.png");
    std::fs::write(&png_path, png.into_inner()).unwrap();
    let json_path = dir.join("a.json");
    std::fs::write(&json_path, b"[1]").unwrap();
    let file = |p: &std::path::Path| p.to_string_lossy().into_owned();

    let mut app = app_on(&path, 1);
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
    let mut sources = Vec::new();
    for &(kind, ty, _) in TYPES.iter().filter(|t| crate::caps::edges().can_connect(OPENED_FROM, t.1, TYPE_TAB)) {
        let id = match kind {
            Kind::Tab => add(&mut app, kind, sleeping_tab("https://x.test/child", "Child")),
            Kind::Group => add_group(&mut app, "g"),
            Kind::Note => add(&mut app, kind, Runtime::Note(NotePayload { text: "n".into(), editing: false })),
            Kind::Image => {
                let id = app.alloc_node_id();
                app.insert_image_node_with_id(id.clone(), file(&png_path), egui::Vec2::ZERO, None).unwrap();
                id
            }
            Kind::Pdf => {
                let id = app.alloc_node_id();
                app.insert_pdf_node_with_id(id.clone(), file(&dir.join("a.pdf")), egui::Vec2::ZERO, None);
                id
            }
            Kind::Json => {
                let id = app.alloc_node_id();
                app.insert_json_node_with_id(id.clone(), file(&json_path), egui::Vec2::ZERO, None).unwrap();
                id
            }
            Kind::Download => {
                let dl = DownloadPayload {
                    path: "f.bin".into(),
                    bytes_received: 0,
                    total_bytes: None,
                    completed: true,
                    failed: false,
                };
                add(&mut app, kind, Runtime::Download(dl))
            }
            _ => panic!("{ty} ({kind:?}) is an opened-from source: add it to this test"),
        };
        assert_eq!(app.connect(OPENED_FROM, &id, &opener), Ok(()), "{ty}");
        sources.push((ty, id));
    }
    assert!(sources.len() >= 6, "tab, note, image, PDF, JSON and download at least: {sources:?}");
    assert!(app.save_session());
    app.close_store();

    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    for (ty, id) in &sources {
        assert_eq!(restored.mirror.parent(id), Some(opener.as_str()), "{ty} after a restart");
    }
    let mut other = Journal::open(&path, ActorId(3)).unwrap();
    for (_, id) in &sources {
        remote_set(&mut other, id, field::COLOR, FieldValue::Color(Some(0x00ff_0000)));
    }
    assert!(restored.save_session());
    for (ty, id) in &sources {
        assert_eq!(restored.mirror.parent(id), Some(opener.as_str()), "{ty} after another replica's edit");
    }
}

/// A running download is placed but not stored until it completes (`snapshot`), so it is no end for a link or an
/// opener yet: the save would write that end as none (a half-made link, a cleared parent).
#[test]
fn a_running_download_is_no_link_or_opener_end_until_it_completes() {
    let mut app = app_on(&temp_dir("connect-running-download").join("session.mnf"), 1);
    let tab = succeed_spawn(&mut app, "https://a/");
    let download = |completed| {
        Runtime::Download(DownloadPayload {
            path: "f.bin".into(),
            bytes_received: 1,
            total_bytes: Some(2),
            completed,
            failed: false,
        })
    };
    let dl = add(&mut app, Kind::Download, download(false));
    for (edge, from, to) in [(LINK, &tab, &dl), (LINK, &dl, &tab), (OPENED_FROM, &tab, &dl), (OPENED_FROM, &dl, &tab)] {
        assert_eq!(app.connect(edge, from, to), Err(ConnectError::NotLive), "{edge} {from} -> {to}");
    }
    assert!(app.valid_targets(LINK, &tab).is_empty() && app.valid_targets(OPENED_FROM, &tab).is_empty());
    assert!(app.valid_targets(LINK, &dl).is_empty(), "nor a source");
    assert!(app.mirror.link_ids().next().is_none() && app.mirror.parent(&tab).is_none());

    app.nodes.get_mut(&dl).unwrap().runtime = download(true);
    assert_eq!(app.valid_targets(LINK, &tab), vec![dl.clone()], "complete, it is a card like any other");
    assert_eq!(app.connect(LINK, &tab, &dl), Ok(()));
    assert!(app.save_session());
    let link = app.links_naming(&tab).pop().expect("the link");
    assert_eq!(stored(&app, &link, field::TO), FieldValue::Id(dl.parse().ok()));
}

/// A tab that becomes a content card keeps its links: each now names the new card.
#[test]
fn a_tab_that_becomes_content_keeps_its_links() {
    let dir = temp_dir("link-redirect");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    let tab = succeed_spawn(&mut app, "https://x.test/a.json");
    let other = succeed_spawn(&mut app, "https://b/");
    app.create_link(&tab, &other);
    app.create_link(&other, &tab);
    let node = app.alloc_node_id();
    let body = crate::redirect_fetch::save(&dir, &node, crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.redirect_tab_to_content(&tab, node.clone(), "https://x.test/a.json".into(), body);
    assert!(app.links_naming(&tab).is_empty());
    let links = app.links_naming(&node);
    assert_eq!(links.len(), 2);
    let ends: Vec<_> = links.iter().map(|l| app.mirror.link_ends(l)).collect();
    let (n, o) = (node.parse().ok(), other.parse().ok());
    assert!(ends.contains(&(n, o)) && ends.contains(&(o, n)), "direction kept");
    assert!(app.save_session());
}

#[test]
fn edges_of_leaves_out_a_dangling_link_and_one_at_another_level() {
    let mut app = app_on(&temp_dir("edges-live").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    let ghost = app.alloc_node_id();
    app.create_link(&a, &ghost);
    app.connect(LINK, &a, &b).unwrap();
    app.connect(LINK, &a, &c).unwrap();
    let g = add_group(&mut app, "g");
    app.edit_group(&b, Some(g));
    app.edit_parent(&a, Some(ghost));
    let of_a = app.edges_of(&a);
    assert_eq!(of_a.len(), 1, "only the link to c is live and at a's level");
    assert_eq!(of_a[0].other, c);
}

#[test]
fn connecting_opened_from_again_replaces_the_opener() {
    let mut app = app_on(&temp_dir("connect-replace").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    app.connect(OPENED_FROM, &c, &a).unwrap();
    assert_eq!(app.connect(OPENED_FROM, &c, &b), Ok(()));
    assert_eq!(app.mirror.parent(&c), Some(b.as_str()));
}

#[test]
fn disconnect_removes_a_link_from_the_opposite_end() {
    let mut app = app_on(&temp_dir("disconnect-reverse").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(LINK, &a, &b).unwrap();
    app.disconnect(LINK, &b, &a);
    assert!(app.links_naming(&a).is_empty());
}

#[test]
fn a_refused_drag_reparent_leaves_the_parent_alone() {
    let mut app = app_on(&temp_dir("reparent-refused").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(OPENED_FROM, &b, &a).unwrap();
    app.apply_drop(&b, Some((crate::card::DropEffect::Reparent, Some("nope".into()))));
    assert_eq!(app.mirror.parent(&b), Some(a.as_str()), "a missing target");
    app.apply_drop(&a, Some((crate::card::DropEffect::Reparent, Some(b.clone()))));
    assert_eq!(app.mirror.parent(&a), None, "a cycle");
}

/// A link inside a closed group comes back with it, between the new ids; a link to a card outside is not restored.
#[test]
fn reopening_a_closed_group_restores_links_inside_it_only() {
    let mut app = app_on(&temp_dir("link-closed").join("session.mnf"), 1);
    let outside = succeed_spawn(&mut app, "https://out/");
    let (x, y) = (succeed_spawn(&mut app, "https://x/"), succeed_spawn(&mut app, "https://y/"));
    let group = add_group(&mut app, "g");
    app.edit_group(&x, Some(group.clone()));
    app.edit_group(&y, Some(group.clone()));
    app.connect(crate::caps::LINK, &x, &y).unwrap();
    app.create_link(&x, &outside); // `connect` would refuse another level, so build this one directly
    app.close_node(&group);
    assert!(app.mirror.link_ids().next().is_none(), "closing deletes every link naming a closed card");
    let entry = app.closed.pop().unwrap();
    app.reopen_closed(entry);
    let links: Vec<_> = app.mirror.link_ids().collect();
    assert_eq!(links.len(), 1, "only the link between x and y returns");
    let (from, to) = app.mirror.link_ends(&links[0].to_string());
    let (from, to) = (from.unwrap().to_string(), to.unwrap().to_string());
    assert!(app.nodes.contains_key(&from) && app.nodes.contains_key(&to), "between live cards");
    assert!(![&x, &y, &outside].contains(&&from) && ![&x, &y, &outside].contains(&&to), "the new ids");
    assert!(app.save_session());
}

/// A foreign file's link naming a card outside the file is dropped; one inside it is kept.
#[test]
fn a_foreign_import_drops_a_link_naming_a_card_outside_the_file() {
    let mut app = app_on(&temp_dir("link-foreign").join("session.mnf"), 1);
    let json = serde_json::json!({ "nodes": [
        json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
        json_node("u", Kind::Tab, serde_json::json!({ "url": "https://b/" })),
        json_node("in", Kind::Link, serde_json::json!({ "from": "t", "to": "u" })),
        json_node("out", Kind::Link, serde_json::json!({ "from": "t", "to": "424242" })),
    ]});
    app.replace_running_session(&serde_json::from_value(json).unwrap(), crate::store::json::Trust::Foreign);
    let ends: Vec<_> = app.mirror.link_ids().map(|l| app.mirror.link_ends(&l.to_string())).collect();
    assert_eq!(ends.len(), 1, "only the link inside the file");
    assert!(ends[0].0.is_some() && ends[0].1.is_some());
    assert!(app.save_session());
}

/// EXPORTSTATE lists every link with its ends.
#[test]
fn export_state_lists_links() {
    let mut app = app_on(&temp_dir("link-state").join("session.mnf"), 1);
    let (a, b) = (succeed_spawn(&mut app, "https://a/"), succeed_spawn(&mut app, "https://b/"));
    let link = app.create_link(&a, &b);
    let state = app.export_state();
    assert_eq!(state["links"], serde_json::json!([{ "id": link, "from": a, "to": b }]));
}

fn link_count(app: &App) -> usize {
    app.mirror.link_ids().count()
}

fn link_doc(nodes: Vec<crate::store::json::JsonNode>) -> JsonDoc {
    JsonDoc { nodes, local: None }
}

/// An outside end that is a real, valid id of a live card must still be dropped from a foreign file: otherwise an
/// imported card would be linked to an unrelated card of this session. Fails if the `json.rs` FROM/TO drop is reverted.
#[test]
fn a_foreign_import_drops_a_link_naming_a_live_card_outside_the_file() {
    let mut app = app_on(&temp_dir("link-foreign-live").join("session.mnf"), 1);
    let live = succeed_spawn(&mut app, "https://live/");
    let doc = link_doc(vec![
        json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
        json_node("l", Kind::Link, serde_json::json!({ "from": "t", "to": live })),
    ]);
    app.import_group_json(&doc);
    assert_eq!(link_count(&app), 0);
    assert!(app.links_naming(&live).is_empty());
}

/// Export Group keeps a link with both ends in the group and leaves out one to a card outside it.
#[test]
fn export_group_includes_links_inside_the_group_only() {
    let mut app = app_on(&temp_dir("link-export-group").join("session.mnf"), 1);
    let outside = succeed_spawn(&mut app, "https://out/");
    let (x, y) = (succeed_spawn(&mut app, "https://x/"), succeed_spawn(&mut app, "https://y/"));
    let group = add_group(&mut app, "g");
    app.edit_group(&x, Some(group.clone()));
    app.edit_group(&y, Some(group.clone()));
    let inside = app.create_link(&x, &y);
    let crossing = app.create_link(&x, &outside);
    let json = app.export_group_json(&group).unwrap();
    let ids: Vec<&str> = json.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&inside.as_str()), "the inner link is exported");
    assert!(!ids.contains(&crossing.as_str()), "the crossing link is not");
    // And it round-trips through Import Group.
    app.import_group_json(&json);
    assert_eq!(link_count(&app), 3, "the two originals and one imported copy");
}

/// An Own-trust file with a dangling link (a real-format id of no node) creates no junk link.
#[test]
fn an_own_import_skips_a_link_whose_end_is_not_a_node() {
    let mut app = app_on(&temp_dir("link-own-dangling").join("session.mnf"), 1);
    let doc = link_doc(vec![
        json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
        json_node("l", Kind::Link, serde_json::json!({ "from": "t", "to": "ffffffffffffffff-999" })),
    ]);
    app.replace_running_session(&doc, crate::store::json::Trust::Own);
    assert_eq!(link_count(&app), 0);
    assert!(app.save_session());
}

/// Export Session leaves out a link with a dead end and keeps a good one.
#[test]
fn export_session_drops_links_with_a_dead_end() {
    let mut app = app_on(&temp_dir("link-export-session").join("session.mnf"), 1);
    let (a, b) = (succeed_spawn(&mut app, "https://a/"), succeed_spawn(&mut app, "https://b/"));
    let good = app.create_link(&a, &b);
    let dead = app.create_link(&a, "ffffffffffffffff-777");
    let json = app.export_session_json().unwrap();
    let ids: Vec<&str> = json.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(ids.contains(&good.as_str()));
    assert!(!ids.contains(&dead.as_str()));
}

/// A link end must be a card: a file's link naming a jar, another link or itself loses that end, and is skipped.
#[test]
fn an_import_drops_a_link_whose_end_is_a_jar_a_link_or_itself() {
    let mut app = app_on(&temp_dir("link-bad-ends").join("session.mnf"), 1);
    let doc = link_doc(vec![
        json_node("t", Kind::Tab, serde_json::json!({ "url": "https://a/" })),
        json_node("j", Kind::Jar, serde_json::json!({ "name": "Work" })),
        json_node("l1", Kind::Link, serde_json::json!({ "from": "t", "to": "j" })),
        json_node("l2", Kind::Link, serde_json::json!({ "from": "t", "to": "l1" })),
        json_node("l3", Kind::Link, serde_json::json!({ "from": "l3", "to": "t" })),
    ]);
    for trust in [crate::store::json::Trust::Own, crate::store::json::Trust::Foreign] {
        app.replace_running_session(&doc, trust);
        assert_eq!(link_count(&app), 0, "{trust:?}");
    }
}

/// Which links a close records: a link between a member and the closed root, and one across nested groups, come
/// back; one to a member `worth_reopening` drops (a blank note) is not recorded.
#[test]
fn closing_records_links_to_the_root_and_across_nesting_but_not_to_dropped_members() {
    let mut app = app_on(&temp_dir("link-closed-kept").join("session.mnf"), 1);
    let outer = add_group(&mut app, "outer");
    let inner = add_group(&mut app, "inner");
    app.edit_group(&inner, Some(outer.clone()));
    let x = succeed_spawn(&mut app, "https://x/");
    let y = succeed_spawn(&mut app, "https://y/");
    app.edit_group(&x, Some(outer.clone()));
    app.edit_group(&y, Some(inner.clone()));
    let blank = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: " ".into(), editing: false }));
    app.edit_group(&blank, Some(outer.clone()));
    app.create_link(&x, &outer); // member and root
    app.create_link(&x, &y); // across nested groups
    app.create_link(&x, &blank); // to a member the entry drops
    app.close_node(&outer);
    let entry = app.closed.pop().unwrap();
    assert_eq!(entry.doc.nodes.iter().filter(|n| n.kind() == Some(Kind::Link)).count(), 2);
    app.reopen_closed(entry);
    assert_eq!(link_count(&app), 2);
    assert!(app.save_session());
}

#[test]
fn the_menu_offers_a_verb_per_edge_with_a_valid_target_and_a_disconnect_per_live_edge() {
    let mut app = app_on(&temp_dir("link-menu").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(LINK, &a, &b).unwrap();
    let rows = app.node_menu_view(&a).unwrap().edges;
    let actions: Vec<&NodeAction> = rows.iter().map(|r| &r.action).collect();
    assert!(!actions.contains(&&NodeAction::Connect(LINK)), "b is the only other card and is linked already");
    assert!(actions.contains(&&NodeAction::Connect(OPENED_FROM)));
    assert!(rows.iter().any(|r| matches!(&r.action, NodeAction::Disconnect { edge, .. } if *edge == LINK)));
}

#[test]
fn pick_target_mode_enters_completes_and_cancels() {
    let mut app = app_on(&temp_dir("pick-target").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.apply_node_action(&egui::Context::default(), &a, NodeAction::Connect(LINK));
    assert_eq!(app.connecting, Some((a.clone(), LINK)));
    app.finish_connect(&b);
    assert!(app.connecting.is_none());
    assert_eq!(app.links_naming(&a).len(), 1);
    app.apply_node_action(&egui::Context::default(), &a, NodeAction::Connect(OPENED_FROM));
    app.cancel_connect();
    assert!(app.connecting.is_none());
}

/// Pick-target mode ends on any other node action, a level change, and its source card closing.
#[test]
fn pick_target_mode_ends_on_another_action_a_level_change_or_its_source_closing() {
    let mut app = app_on(&temp_dir("pick-ends").join("session.mnf"), 1);
    let ctx = egui::Context::default();
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.apply_node_action(&ctx, &a, NodeAction::Connect(LINK));
    app.apply_node_action(&ctx, &b, NodeAction::TogglePin);
    assert!(app.connecting.is_none(), "another action");
    app.apply_node_action(&ctx, &a, NodeAction::Connect(LINK));
    let g = add_group(&mut app, "g");
    app.switch_group_level(Some(g));
    assert!(app.connecting.is_none(), "a level change");
    app.switch_group_level(None);
    app.apply_node_action(&ctx, &a, NodeAction::Connect(LINK));
    app.close_node(&a);
    assert!(app.connecting.is_none(), "its source closed");
    // A stale mode found by the per-frame check: the Graph view not shown.
    app.apply_node_action(&ctx, &b, NodeAction::Connect(LINK));
    app.end_stale_connect(true);
    assert!(app.connecting.is_some());
    app.end_stale_connect(false);
    assert!(app.connecting.is_none());
}

/// "Disconnect from X" removes the edge its row names, either way round, and its label says which edge it is.
#[test]
fn disconnect_rows_name_their_edge_and_remove_it() {
    let mut app = app_on(&temp_dir("disconnect-rows").join("session.mnf"), 1);
    let ctx = egui::Context::default();
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    app.connect(OPENED_FROM, &b, &a).unwrap();
    app.connect(LINK, &a, &b).unwrap();
    let rows = |app: &App, id: &str| app.node_menu_view(id).unwrap().edges;
    let disconnects = |app: &App, id: &str| -> Vec<(String, NodeAction)> {
        rows(app, id)
            .into_iter()
            .filter(|r| matches!(r.action, NodeAction::Disconnect { .. }))
            .map(|r| (r.label, r.action))
            .collect()
    };
    let of_a = disconnects(&app, &a);
    assert_eq!(of_a.len(), 2, "{of_a:?}");
    assert!(of_a.iter().any(|(label, _)| label.starts_with("Disconnect from Tab") && label.ends_with("(link)")));
    assert!(of_a.iter().any(|(label, _)| label.ends_with("(opened from here)")), "{of_a:?}");
    assert!(disconnects(&app, &b).iter().any(|(label, _)| label.ends_with("(opened from it)")));
    // a's row for the opened-from edge: the parent field is b's.
    let (_, opener) = of_a.iter().find(|(label, _)| label.ends_with("(opened from here)")).unwrap().clone();
    app.apply_node_action(&ctx, &a, opener);
    assert_eq!(app.mirror.parent(&b), None);
    let (_, link) = disconnects(&app, &b).into_iter().find(|(label, _)| label.ends_with("(link)")).unwrap();
    app.apply_node_action(&ctx, &b, link);
    assert!(app.links_naming(&a).is_empty());
    assert!(disconnects(&app, &a).is_empty());
    assert!(app.save_session());
}

/// EXPORTSTATE shows pick-target mode, with its targets.
#[test]
fn export_state_shows_pick_target_mode() {
    let mut app = app_on(&temp_dir("pick-state").join("session.mnf"), 1);
    let (a, b) = (succeed_spawn(&mut app, "https://a/"), succeed_spawn(&mut app, "https://b/"));
    assert_eq!(app.export_state()["connecting"], serde_json::Value::Null);
    app.apply_node_action(&egui::Context::default(), &a, NodeAction::Connect(LINK));
    assert_eq!(app.export_state()["connecting"], serde_json::json!({ "from": a, "edge": LINK, "targets": [b] }));
}
