use super::writes::keep_trail;
use super::*;
use crate::local_state::{LiveUrl, LocalState};
use crate::redirect_fetch::{FetchResult, FetchTarget};
use crate::store::journal::Journal;
use crate::store::json::JsonDoc;
use crate::store::model::ActorId;
use crate::store::testutil::{temp_dir, XorShift};
use crate::tabs::{LeftClosed, SpawnPurpose};
use crate::NodeAction;

mod costs;
mod edits;
mod files;
mod local_state;
mod remote;
mod spawn;
mod trail;
mod writes;

const A: ActorId = ActorId(0xA);

#[test]
fn jar_fields_belong_to_tabs_groups_and_jars() {
    use field::*;
    assert_eq!(Kind::Tab.fields(), &[POS, COLOR, PARENT, URL, VIEW_KIND, PINNED, JAR, NAME, CLOSED]);
    assert_eq!(Kind::Group.fields(), &[POS, COLOR, NAME, JAR, CLOSED, HELD]);
    assert_eq!(Kind::Jar.fields(), &[POS, COLOR, NAME, EPHEMERAL, IS_DEFAULT, CLOSED]);
    assert_eq!(Kind::Link.fields(), &[NAME, FROM, TO, CLOSED]);
    assert_eq!(Kind::Unknown.fields(), &[POS, COLOR, PARENT, NAME, CLOSED]);
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
    assert_eq!(app.toasts.save_toast_queue.len(), 2);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(app.save_session());
    app.notice("read-only".into());
    assert_eq!(app.toasts.save_toast_queue.len(), 3, "a commit lets a notice show again");
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
    Snapshot { ty: kind.type_str().unwrap().to_owned(), group, fields }
}

fn note(text: &str, group: Option<NodeId>) -> Snapshot {
    shared(Kind::Note, group, &[(field::TEXT, FieldValue::Str(text.into()))])
}

fn group(name: &str) -> Snapshot {
    shared(Kind::Group, None, &[(field::NAME, FieldValue::Str(name.into()))])
}

/// A remote change's snapshot of a `kind` node: its type string, `fields` and `group`.
fn snap(kind: Kind, fields: Fields, group: Option<NodeId>) -> Snapshot {
    Snapshot { ty: kind.type_str().unwrap().to_owned(), group, fields }
}

/// What `doc` shows, shaped like `snapshots`.
fn shown(doc: &Doc) -> BTreeMap<NodeId, Snapshot> {
    doc.shown().map(|(id, node)| (id, Snapshot::of(doc, id, node))).collect()
}

/// What the store holds for every live node (`App::snapshot`), by store id: an in-progress download, a
/// hand-built id and a tab whose spawn is in flight are left out. `pub(crate)`: other modules' tests compare it too.
pub(crate) fn snapshots(app: &App) -> BTreeMap<NodeId, Snapshot> {
    app.nodes
        .keys()
        .filter_map(|key| key.parse::<NodeId>().ok())
        .filter_map(|id| Some((id, app.snapshot(id)?)))
        .collect()
}

/// Actor 1's op count: what a save wrote.
fn ops_of(app: &App) -> u64 {
    app.doc.journal.as_ref().unwrap().doc().version.get(&ActorId(1)).copied().unwrap_or(0)
}

/// Whether the document shows `id` live.
fn live_in_doc(app: &App, id: &str) -> bool {
    app.doc.journal.as_ref().unwrap().doc().nodes.get(&id.parse().unwrap()).is_some_and(Node::is_live)
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

/// Materializes `nodes` into a windowless app, as `apply_remote` would
/// show another replica's new nodes.
fn app_with(nodes: &BTreeMap<NodeId, Snapshot>) -> App {
    let mut app = App::default();
    let changes = nodes.iter().map(|(&id, s)| (id, Remote::Shown(s.clone()))).collect();
    app.apply_remote(changes);
    app
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
    assert!(app.toasts.save_toast_queue.is_empty() && app.toasts.save_toast.is_none(), "no toast");
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

/// A windowless app saving to `path` as `actor`. `pub(crate)`: also
/// used by `main.rs`'s own windowless tests (e.g. `duplicate`'s).
pub(crate) fn app_on(path: &std::path::Path, actor: u64) -> App {
    let mut app = App::default();
    app.doc.journal = Some(Journal::open(path, ActorId(actor)).unwrap());
    app
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
    app.doc.journal.as_ref().unwrap().doc().nodes[&node.parse().unwrap()].get(no)
}

pub(crate) fn add_note(app: &mut App, text: &str) -> String {
    add(app, Kind::Note, Runtime::Note(NotePayload { text: text.into(), editing: false }))
}

pub(crate) fn closed_ids(app: &App) -> Vec<String> {
    app.doc.journal.as_ref().unwrap().doc().closed_roots().into_iter().map(|(n, _)| n.to_string()).collect()
}

/// A group with one kept note inside, saved.
pub(crate) fn group_with_note(app: &mut App) -> (String, String) {
    let g = add_group(app, "G");
    let inner = add_note(app, "kept");
    app.edit_group(&inner, Some(g.clone()));
    assert!(app.save_session());
    (g, inner)
}

/// Another replica sets `closed` on `node` (true closes, false reopens).
fn remote_close(other: &mut Journal, node: &str, closed: bool) {
    remote_set(other, node, field::CLOSED, FieldValue::Replica(if closed { 99 } else { 0 }));
}

#[test]
fn backing_keep_holds_hidden_nodes_files() {
    let path = temp_dir("sync-backing-hidden").join("session.mnf");
    let mut app = app_on(&path, 1);
    let jar = add(&mut app, Kind::Jar, Runtime::Jar);
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_close(&mut other, &jar, true);
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&jar));
    assert!(app.backing_keep(None).contains(&jar), "a closed jar keeps its cookie file");
    assert!(!app.backing_keep(Some(&jar)).contains(&jar), "`except` leaves it out");
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

/// A Json node `v.json` (`{"users": [...]}`) with a store id, in an app saving to `path`.
fn json_on(path: &std::path::Path, name: &str) -> (App, String) {
    let file = temp_dir(name).join("v.json");
    std::fs::write(&file, r#"{"users": [{"name": "a"}]}"#).unwrap();
    let mut app = app_on(path, 1);
    let node = app.alloc_node_id();
    app.insert_json_node_with_id(node.clone(), file.to_string_lossy().into_owned(), egui::Vec2::ZERO, None).unwrap();
    (app, node)
}

/// Right after a typed navigation, a report still at the url it left
/// is from before the engine started it.
#[test]
fn a_report_still_at_the_url_a_typed_navigation_left_is_stale() {
    let mut app = App::default();
    let report = |app: &mut App, url: &str| app.reported_url("7", url.into(), true, "https://typed/".into());
    assert_eq!(report(&mut app, "https://old/"), "https://old/", "no typed navigation: as reported");
    app.spawns.navigated_from.insert("7".into(), ("https://old/".into(), std::time::Instant::now(), false));
    assert_eq!(report(&mut app, "https://old/"), "https://typed/");
    assert_eq!(report(&mut app, "https://typed/"), "https://typed/");
    assert_eq!(report(&mut app, "https://old/"), "https://old/", "after the engine moved on: real (going back)");
    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(2);
    app.spawns.navigated_from.insert("7".into(), ("https://old/".into(), long_ago, false));
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
    app.spawns.navigated_from.insert("7".into(), ("https://old/".into(), std::time::Instant::now(), false));
    assert_eq!(report(&mut app, "https://old/", false), "https://typed/", "a finish from before it started");
    assert_eq!(report(&mut app, "https://old/", true), "https://typed/", "started, address not changed yet");
    assert_eq!(report(&mut app, "https://old/", false), "https://old/", "finished without leaving");
    assert!(app.spawns.navigated_from.is_empty());
}

/// A note placed at `pos` (top level), as `place_node` places every new node.
fn a_note_at(app: &mut App, pos: egui::Vec2) -> String {
    let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
    app.place_node(None, Kind::Note, note, crate::fetched::Placement { group: None, pos })
}

/// What a drag does to the layout every frame (`card.rs`), then its release.
fn drag(app: &mut App, ids: &[&str], delta: egui::Vec2) {
    app.interaction.dragging = Some(ids[0].to_owned());
    app.interaction.drag_moved = ids.iter().map(|&id| id.to_owned()).collect();
    for &id in ids {
        app.layout.entry(id.to_owned()).or_default().pos += delta;
    }
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
    assert!(app.interaction.dragging.is_none() && app.interaction.drag_moved.is_empty());
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
    assert!(
        app.interaction.dragging.is_none() && app.interaction.drag_moved.is_empty(),
        "the frame's check ends the drag"
    );
    app.release_drag();
    assert!(app.save_session());
    assert_eq!(stored(&app, &a, field::POS), FieldValue::Pos(7.0, 8.0));

    // Leaving Graph view ends a drag too.
    drag(&mut app, &[&b], egui::vec2(0.0, 50.0));
    app.end_stale_drag(false);
    assert!(app.interaction.dragging.is_none() && app.interaction.drag_moved.is_empty());
}

#[test]
fn view_table_matches_the_store_and_round_trips() {
    for (i, kind) in ViewKind::ALL.into_iter().enumerate() {
        let desc = kind.desc();
        assert_eq!(desc.code, i as u64 + 1, "codes are 1-based positions in ALL");
        assert_eq!(crate::store::model::VIEW_KINDS[i], desc.name, "the store names the same engine");
        assert_eq!(serde_json::to_value(Wrap(kind)).unwrap(), serde_json::json!(desc.name));
        assert_eq!(view_kind_of(view_code(kind)), kind);
        assert_eq!(ViewKind::from_name(&desc.name.to_uppercase()), Some(kind));
    }
    assert_eq!(ViewKind::from_name("gecko"), None);
    assert_eq!(ViewKind::from_code(0), None);
}

#[derive(serde::Serialize)]
struct Wrap(#[serde(with = "crate::process_manager::view_kind_serde")] ViewKind);

#[test]
fn colors_round_trip() {
    let c = egui::Color32::from_rgb(1, 2, 3);
    assert_eq!(color_of(&color_value(Some(c))), Some(c));
    assert_eq!(color_value(Some(egui::Color32::BLACK)), FieldValue::Color(Some(0)), "black is encoded");
    assert_eq!(color_of(&color_value(None)), None);
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
        old.begin().unwrap().commit(vec![OpBody::create(node, crate::store::model::TYPE_DOWNLOAD, fields)]).unwrap();
    }
    let mut app = app_on(&path, 1);
    assert!(app.restore_session());
    let restored = &app.nodes[&node.to_string()];
    assert_eq!(app.doc.mirror.parent(&node.to_string()), None);
    assert!(restored.as_download().is_some());
    assert_eq!(app.copyable_url(&node.to_string()), Some("https://x.test/f.bin"));
    assert_eq!(app.layout_pos(&node.to_string()), egui::vec2(1.0, 2.0));
    assert_eq!(app.doc.mirror.pos(&node.to_string()), Some(egui::vec2(1.0, 2.0)));
    // No link encodes as the absent register, so a save has nothing to write back.
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "nothing written");
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

/// Fails every spawn parked in `test_spawns`, as a missing engine would.
fn fail_spawns(app: &mut App) {
    for parked in std::mem::take(&mut app.test_spawns) {
        fail_spawn(app, parked);
    }
}

fn fail_spawn(app: &mut App, (spawn, generation): (crate::tabs::TabSpawn, u64)) {
    let result = Err(std::io::Error::other("no engine").into());
    app.handle_tab_spawn_result(crate::tabs::TabSpawnOutcome {
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
    let (tab, _peer) = crate::process_manager::ViewTab::fake(spawn.kind);
    app.handle_tab_spawn_result(crate::tabs::TabSpawnOutcome {
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

fn json_node(id: &str, kind: Kind, fields: serde_json::Value) -> crate::store::json::JsonNode {
    let serde_json::Value::Object(mut fields) = fields else { panic!("fields are an object") };
    fields.insert("type".into(), kind.type_str().expect("a known kind").into());
    crate::store::json::JsonNode { id: id.into(), fields }
}

#[test]
fn a_session_with_only_jars_still_restores_them() {
    let dir = crate::store::testutil::temp_dir("jar-only-restore");
    let path = dir.join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = add_jar(&mut app, "Work");
    app.save_session();
    app.remove_tab(&tab);
    app.save_session();
    drop(app);
    let mut back = app_on(&path, 3);
    assert!(!back.restore_session(), "no graph node: a fresh start");
    assert_eq!(back.jar_defs()[&jar].name, "Work", "but the jar is kept, so the next save keeps it");
    back.save_session();
    assert!(back.doc.journal.as_ref().unwrap().doc().nodes[&jar].is_live());
}

/// A file with a jar "Work" (label j), a group with an edge to it and a tab in the group with one too.
fn file_with_a_work_jar() -> JsonDoc {
    let json = serde_json::json!({ "nodes": [
        json_node("g", Kind::Group, serde_json::json!({ "name": "G", "jar": "j" })),
        json_node("j", Kind::Jar, serde_json::json!({ "name": "Work" })),
        json_node(
            "t",
            Kind::Tab,
            serde_json::json!({ "url": "https://a.com/", "group": "g", "jar": "j" }),
        ),
    ]});
    serde_json::from_value(json).unwrap()
}

/// Personal state as a peer wrote it while this instance's startup read it
/// unread: `local_unread` set by hand, without waiting out `LOCK_WAIT`.
fn mark_unread_with(app: &mut App, path: &std::path::Path, state: &LocalState) {
    Journal::open(path, ActorId(9)).unwrap().write_local(&state.to_bytes()).unwrap();
    app.doc.local_persist.local_unread = Some(LocalUnread { retry_at: std::time::Instant::now(), fresh: false });
    app.doc.local_persist.local_written = None;
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

/// What the crash path and `finish_swap` do: carry from the mirror, take the node out of `nodes`, respawn. The
/// spawn carries no position: the tab keeps its `App::layout` entry.
fn start_respawn(app: &mut App, tab: &str) {
    app.nodes.remove(tab).unwrap();
    let carried = app.doc.mirror.carried(tab);
    let mut spawn = crate::tabs::TabSpawn::new(
        tab.to_owned(),
        "https://t/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Respawn,
        1.0,
    );
    (spawn.carry.color, spawn.carry.pinned) = (carried.color(), carried.pinned());
    (spawn.jar, spawn.parent, spawn.group) = (carried.jar(), carried.parent, carried.group);
    app.respawn_tab(spawn);
    assert!(app.spawns.spawning.contains_key(tab) && !app.nodes.contains_key(tab), "respawning");
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

/// The page moves on (an engine report): the live url changes, the shared one doesn't.
fn set_live_url(app: &mut App, tab: &str, url: &str) {
    match &mut app.nodes.get_mut(tab).unwrap().runtime {
        Runtime::Tab(t) => t.url = url.into(),
        Runtime::Unloaded(t) => t.url = url.into(),
        _ => panic!("{tab} is not a tab"),
    }
}

/// Ruling 9 (2b-2's parked item): `recheck_edge_scope` reaches a tab mid-respawn, so a local move that takes its
/// edge out of reach clears it, and it lands without it.
#[test]
fn a_respawning_tab_follows_a_local_moves_edge_recheck() {
    let path = temp_dir("jar-respawn-move").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (_outer, g, t) = nested_tab(&mut app);
    let local = app.create_jar("Local", crate::fetched::Placement { group: Some(g.clone()), pos: egui::Vec2::ZERO });
    app.set_jar_edge(&t, Some(local));
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(Some(local)));
    start_respawn(&mut app, &t);
    app.edit_group(&t, None); // out of G: the jar inside G is no longer in reach
    app.recheck_edge_scope(&t);
    assert_eq!(app.doc.mirror.jar(&t), None);
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None));
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.jar(&t), None, "it lands without it");
    assert!(app.save_session());
    assert_eq!(stored(&app, &t, field::JAR), FieldValue::Id(None));
}

pub(crate) type Peer = crate::process_manager::FakePeer;

/// A live tab on `kind` (a fake host, `ViewTab::fake`), as a landing inserts it. The peer reads what it is sent.
pub(crate) fn live_tab(app: &mut App, kind: ViewKind, url: &str) -> (String, Peer) {
    let (tab, peer) = crate::process_manager::ViewTab::fake(kind);
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
fn sent(peer: &mut Peer) -> Vec<HostToView> {
    peer.sent()
}

/// The engines that take part in the cookie jar (spec 2026-10-02-jars-design): only their tabs are asked for
/// cookies, and only they count as "will switch" when their jar changes. The rest neither seed nor pull.
const JAR_VIEW_KINDS: [ViewKind; 2] = [ViewKind::Webview2, ViewKind::Cef];

#[test]
fn devtools_on_a_view_without_them_toasts_instead_of_sending() {
    let mut app = app_on(&temp_dir("caps-devtools").join("session.mnf"), 1);
    for (kind, has) in
        [(ViewKind::Cef, true), (ViewKind::Webview2, true), (ViewKind::Servo, false), (ViewKind::Webkit, false)]
    {
        let (id, mut peer) = live_tab(&mut app, kind, "https://a/");
        app.focused = Some(id);
        app.toasts.save_toast_queue.clear();
        app.open_devtools_shortcut();
        let sent_it = sent(&mut peer).iter().any(|m| matches!(m, HostToView::OpenDevTools));
        assert_eq!(sent_it, has, "{kind:?}");
        let toast = format!("{} has no DevTools", kind.desc().label);
        assert_eq!(app.toasts.save_toast_queue.contains(&toast), !has, "{kind:?}");
    }
}

#[test]
fn find_on_a_view_without_it_toasts_and_leaves_the_bar_shut() {
    let mut app = app_on(&temp_dir("caps-find").join("session.mnf"), 1);
    app.view_mode = ViewMode::Item;
    for (kind, has) in
        [(ViewKind::Cef, true), (ViewKind::Webview2, true), (ViewKind::Webkit, true), (ViewKind::Servo, false)]
    {
        let (id, _peer) = live_tab(&mut app, kind, "https://a/");
        app.focused = Some(id);
        app.find.open = false;
        app.toasts.save_toast_queue.clear();
        app.open_find_bar_shortcut();
        assert_eq!(app.find.open, has, "{kind:?}");
        let toast = format!("{} has no find in page", kind.desc().label);
        assert_eq!(app.toasts.save_toast_queue.contains(&toast), !has, "{kind:?}");
    }
}

fn view_kind(kind: ViewKind) -> FieldValue {
    FieldValue::ViewCode(view_code(kind))
}

fn swap_target(app: &App, tab: &str) -> Option<ViewKind> {
    app.nodes[tab].as_tab().unwrap().swap_target
}

/// Ruling 5: a swap queues its target when it starts (`start_swap_to`), so a save before the old engine's state
/// arrives writes the target (as `swap_target.unwrap_or(kind)` did before), while the old engine still runs.
#[test]
fn a_swap_saves_its_target_before_the_state_snapshot() {
    let path = temp_dir("engine-swap-start").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, mut peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::VIEW_KIND), view_kind(ViewKind::Servo));
    app.start_swap_to(&tab, ViewKind::Cef);
    assert_eq!(app.doc.mirror.view_kind(&tab), ViewKind::Cef, "shown at once");
    assert!(app.doc.writes.field_queued(tab.parse().unwrap(), field::VIEW_KIND), "queued when the swap starts");
    assert!(sent(&mut peer).iter().any(|m| matches!(m, HostToView::GetState)), "the swap asked for the state");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
    assert_eq!(app.nodes[&tab].as_tab().unwrap().tab.kind, ViewKind::Servo, "the old engine still runs");
    assert_eq!(swap_target(&app, &tab), Some(ViewKind::Cef));
}

/// Ruling 5: a switch whose new engine fails to start falls back to the old one, and writes it back when the
/// fallback starts; a fallback that fails too leaves a failed card on the old engine.
#[test]
fn a_failed_swap_writes_the_old_view_back() {
    let path = temp_dir("engine-swap-fails").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    app.start_swap_to(&tab, ViewKind::Cef);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
    // The old engine's state arrived: its host is dropped and the new one spawns.
    app.finish_swap(&tab, ViewKind::Cef, "https://s/".into());
    assert_eq!(app.test_spawns.len(), 1);
    fail_spawns(&mut app);
    let fallback = &app.test_spawns.first().expect("the fallback is in flight").0;
    assert_eq!((fallback.kind, fallback.fallback_kind), (ViewKind::Servo, None));
    assert_eq!(app.doc.mirror.view_kind(&tab), ViewKind::Servo);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::VIEW_KIND), view_kind(ViewKind::Servo), "written back as the fallback starts");
    fail_spawns(&mut app);
    assert!(matches!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Failed(_)));
    assert_eq!(app.doc.mirror.view_kind(&tab), ViewKind::Servo);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::VIEW_KIND), view_kind(ViewKind::Servo));
}

/// Sleeping mid-swap is reachable: `request_sleep` doesn't wait for a swap, and the page state can answer before the
/// swap's snapshot (the sleep then drops `swap_target` with the live payload). The card keeps the target the mirror
/// holds: it saves it and wakes into it.
#[test]
fn a_tab_put_to_sleep_mid_swap_keeps_the_target() {
    let path = temp_dir("engine-sleep-mid-swap").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    assert!(app.save_session());
    app.start_swap_to(&tab, ViewKind::Cef);
    assert!(app.request_sleep(&tab, crate::sleep_state::SleepWhy::Manual), "a sleep is not refused mid-swap");
    app.finish_sleep(&tab, manifold_view_protocol::PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false });
    assert_eq!(app.nodes[&tab].as_unloaded().unwrap().state, UnloadedState::Sleeping, "asleep");
    assert_eq!(app.doc.mirror.view_kind(&tab), ViewKind::Cef);
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::VIEW_KIND), view_kind(ViewKind::Cef));
    app.focus_tab(&tab);
    assert_eq!(app.test_spawns[0].0.kind, ViewKind::Cef, "wakes into the target");
}

/// A crash mid-swap restarts the tab on the target the mirror holds, with the old engine as its fallback, so the
/// tab runs what the document says either way.
#[test]
fn a_crash_mid_swap_restarts_on_the_target_with_the_old_view_as_fallback() {
    let path = temp_dir("engine-crash-mid-swap").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (tab, _peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    assert!(app.save_session());
    app.start_swap_to(&tab, ViewKind::Cef);
    app.handle_view_message(&tab, manifold_view_protocol::ViewToHost::Crashed { reason: "boom".into() });
    let spawn = &app.test_spawns.first().expect("restarting").0;
    assert_eq!((spawn.kind, spawn.fallback_kind), (ViewKind::Cef, Some(ViewKind::Servo)));
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.view_kind(&tab), ViewKind::Servo, "the target failed: back to the old engine");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::VIEW_KIND), view_kind(ViewKind::Servo));
}

/// Lands every spawn in flight on a fake host of the kind it asked for. The peers keep the hosts' channels open.
fn land_spawns(app: &mut App) -> Vec<Peer> {
    let mut peers = Vec::new();
    for (spawn, generation) in std::mem::take(&mut app.test_spawns) {
        let (tab, peer) = crate::process_manager::ViewTab::fake(spawn.kind);
        app.handle_tab_spawn_result(crate::tabs::TabSpawnOutcome {
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
    let (tab, mut peer) = live_tab(&mut app, ViewKind::Cef, "https://s/");
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    app.start_swap_to(&tab, ViewKind::Webview2);
    assert!(app.save_session());
    let snapshot = manifold_view_protocol::TabState { url: "https://s/".into() };
    app.handle_view_message(&tab, manifold_view_protocol::ViewToHost::StateSnapshot(snapshot));
    assert!(app.cookies.cookie_holds.contains_key(&tab), "parked for the cookies");
    assert_eq!(swap_target(&app, &tab), None);
    app.start_swap_to(&tab, ViewKind::Servo);
    assert_eq!(app.doc.mirror.view_kind(&tab), ViewKind::Webview2, "a local pick is refused");
    remote_set(&mut other, &tab, field::VIEW_KIND, view_kind(ViewKind::Servo));
    assert!(app.save_session());
    assert_eq!(swap_target(&app, &tab), None, "no second swap");
    let asked = sent(&mut peer).iter().filter(|m| matches!(m, HostToView::GetState)).count();
    assert_eq!(asked, 1, "the old host was asked once");
    assert_eq!(app.doc.mirror.view_kind(&tab), ViewKind::Webview2, "the remote engine is written over");
    app.handle_view_message(&tab, manifold_view_protocol::ViewToHost::Cookies("[]".into()));
    let spawn = &app.test_spawns.first().expect("the swap goes on").0;
    assert_eq!((spawn.kind, spawn.fallback_kind), (ViewKind::Webview2, Some(ViewKind::Cef)));
    let _peers = land_spawns(&mut app);
    assert_eq!(app.nodes[&tab].as_tab().unwrap().tab.kind, ViewKind::Webview2, "landed on the target");
    assert_eq!(app.doc.mirror.view_kind(&tab), ViewKind::Webview2, "the document holds what runs");
    assert!(app.save_session());
    assert_eq!(stored(&app, &tab, field::VIEW_KIND), view_kind(ViewKind::Webview2));
}

/// A new tab spawned with `jar`, as `spawn_tab_at` starts one.
fn spawn_with_jar(app: &mut App, purpose: SpawnPurpose, jar: NodeId) -> String {
    let t = app.alloc_node_id();
    let mut spawn =
        crate::tabs::TabSpawn::new(t.clone(), "https://t/".into(), ViewKind::Servo, egui::Vec2::ZERO, purpose, 1.0);
    spawn.jar = Some(jar);
    app.spawn_tab_async(spawn);
    t
}

/// A group's jar edge is the mirror's: `set_jar_edge` writes it through `edit_field` and the save stores it; a remote
/// change reaches the mirror and the jar its tabs use.
#[test]
fn a_groups_jar_edge_saves_and_a_remote_change_reaches_it() {
    let path = temp_dir("group-jar-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    let (work, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let g = add_group(&mut app, "G");
    app.set_jar_edge(&g, Some(work));
    assert_eq!(app.doc.mirror.jar(&g), Some(work));
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::JAR), FieldValue::Id(Some(work)));
    app.set_jar_edge(&g, None);
    assert!(app.save_session());
    assert_eq!(stored(&app, &g, field::JAR), FieldValue::Id(None));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, &g, field::JAR, FieldValue::Id(Some(home)));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.jar(&g), Some(home));
    assert_eq!(app.effective_jar(&g), Some((home, false)), "its tabs use Home");
}

fn flushed_failure_toasts(app: &mut App) -> Vec<String> {
    app.toasts.failure_notices.flush(std::time::Instant::now() + crate::failure_toasts::WINDOW)
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
    assert!(app.doc.mirror.link_ids().all(|id| id.to_string() != link));
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
    assert_eq!(app.doc.mirror.parent(&b), Some(a.as_str()));
    assert_eq!(app.connect(OPENED_FROM, &a, &b), Err(ConnectError::Cycle));
    app.disconnect(OPENED_FROM, &b, &a);
    assert_eq!(app.doc.mirror.parent(&b), None);
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
    assert!(edges.iter().all(|e| e.curve == crate::level::EdgeCurve::Bow), "every edge between stored nodes bows");
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
    assert!(app.doc.mirror.link_ids().next().is_none() && app.doc.mirror.parent(&tab).is_none());

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
    let ends: Vec<_> = links.iter().map(|l| app.doc.mirror.link_ends(l)).collect();
    let (n, o) = (node.parse().ok(), other.parse().ok());
    assert!(ends.contains(&(n, o)) && ends.contains(&(o, n)), "direction kept");
    assert!(app.save_session());
}

/// Links final review M2: merging S into T moves S's links onto T, keeping their direction, except one that would join T
/// to itself (S-T) and one to a card T is already linked to (S-c beside T-c). Restored as saved.
#[test]
fn merging_a_group_retargets_its_links_onto_the_target() {
    let path = temp_dir("link-merge").join("session.mnf");
    let mut app = app_on(&path, 1);
    let s = add_group(&mut app, "S");
    let t = add_group(&mut app, "T");
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let c = succeed_spawn(&mut app, "https://c/");
    app.create_link(&s, &a);
    app.create_link(&b, &s);
    app.create_link(&s, &t);
    app.create_link(&s, &c);
    let tc = app.create_link(&t, &c);
    assert!(app.save_session());
    app.merge_groups(&s, &t);
    assert!(app.links_naming(&s).is_empty());
    let id = |x: &str| x.parse().ok();
    let ends = |app: &App| {
        let mut ends: Vec<_> = app.links_naming(&t).iter().map(|l| app.doc.mirror.link_ends(l)).collect();
        ends.sort();
        ends
    };
    let mut want = vec![(id(&t), id(&a)), (id(&b), id(&t)), (id(&t), id(&c))];
    want.sort();
    assert_eq!(ends(&app), want);
    assert_eq!(app.links_naming(&c), [tc], "T-c kept as it was");
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(ends(&restored), want);
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
    assert_eq!(app.doc.mirror.parent(&c), Some(b.as_str()));
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
    assert_eq!(app.doc.mirror.parent(&b), Some(a.as_str()), "a missing target");
    app.apply_drop(&a, Some((crate::card::DropEffect::Reparent, Some(b.clone()))));
    assert_eq!(app.doc.mirror.parent(&a), None, "a cycle");
}

/// EXPORTSTATE lists every link with its ends and whether both ends are shown.
#[test]
fn export_state_lists_links() {
    let mut app = app_on(&temp_dir("link-state").join("session.mnf"), 1);
    let (a, b) = (succeed_spawn(&mut app, "https://a/"), succeed_spawn(&mut app, "https://b/"));
    let link = app.create_link(&a, &b);
    let state = app.export_state();
    assert_eq!(state["links"], serde_json::json!([{ "id": link, "from": a, "to": b, "shown": true }]));
}

fn link_count(app: &App) -> usize {
    app.doc.mirror.link_ids().count()
}

fn link_doc(nodes: Vec<crate::store::json::JsonNode>) -> JsonDoc {
    JsonDoc { nodes, local: None }
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
    assert_eq!(app.link.connecting, Some((a.clone(), LINK)));
    app.finish_connect(&b);
    assert!(app.link.connecting.is_none());
    assert_eq!(app.links_naming(&a).len(), 1);
    app.apply_node_action(&egui::Context::default(), &a, NodeAction::Connect(OPENED_FROM));
    app.cancel_connect();
    assert!(app.link.connecting.is_none());
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
    assert!(app.link.connecting.is_none(), "another action");
    app.apply_node_action(&ctx, &a, NodeAction::Connect(LINK));
    let g = add_group(&mut app, "g");
    app.switch_group_level(Some(g));
    assert!(app.link.connecting.is_none(), "a level change");
    app.switch_group_level(None);
    app.apply_node_action(&ctx, &a, NodeAction::Connect(LINK));
    app.close_node(&a);
    assert!(app.link.connecting.is_none(), "its source closed");
    // A stale mode found by the per-frame check: the Graph view not shown.
    app.apply_node_action(&ctx, &b, NodeAction::Connect(LINK));
    app.end_stale_connect(true);
    assert!(app.link.connecting.is_some());
    app.end_stale_connect(false);
    assert!(app.link.connecting.is_none());
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
    assert_eq!(app.doc.mirror.parent(&b), None);
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

/// A jar card named `name` at the top level, off the origin (which reads as unplaced, `pos_of`).
pub(crate) fn add_jar(app: &mut App, name: &str) -> NodeId {
    app.create_jar(name, crate::fetched::Placement { group: None, pos: egui::vec2(40.0, 40.0) })
}

/// The Default jar's id, making it first if the app has none (startup does this).
pub(crate) fn default_jar_id(app: &mut App) -> NodeId {
    app.ensure_default_jar();
    app.default_jar().expect("ensure_default_jar made one")
}

/// A newer build's type: the unknown-type card (spec 2026-10-14).
pub(crate) const FUTURE: &str = "application/x-from-the-future";

/// A session another replica (actor 2) wrote holding one node of type `FUTURE`, named "Old" at (5, 5), with a newer
/// build's raw field 40 and two known fields its kind doesn't carry (`url`, an `Id`-typed `jar`), restored by actor 1.
pub(crate) fn restored_unknown() -> (App, NodeId, std::path::PathBuf) {
    let path = temp_dir("unknown-card").join("session.mnf");
    let node = {
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        let mut txn = other.begin().unwrap();
        let node = txn.next_id();
        let mut fields = encode_field(field::NAME, &FieldValue::Str("Old".into()));
        fields.extend(encode_field(field::POS, &FieldValue::Pos(5.0, 5.0)));
        fields.extend(encode_field(field::URL, &FieldValue::Str("https://x/".into())));
        fields.extend(encode_field(field::JAR, &FieldValue::Id(Some(node))));
        fields.extend(encode_field(40, &FieldValue::Str("theirs".into())));
        txn.commit(vec![OpBody::create(node, FUTURE, fields)]).unwrap();
        node
    };
    let mut app = app_on(&path, 1);
    assert!(app.restore_session(), "a lone unknown card is real content, not a fresh start");
    (app, node, path)
}

/// The fields `node` holds in `app`'s document, by number.
fn doc_fields(app: &App, node: NodeId) -> Vec<u32> {
    app.doc.journal.as_ref().unwrap().doc().nodes[&node].fields.keys().copied().collect()
}
