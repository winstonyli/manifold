use super::writes::keep_trail;
use super::*;
use crate::card_id::testutil::node;
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
mod labels;
mod links;
mod local_state;
mod misc;
mod props;
mod remote;
mod spawn;
mod trail;
mod view_modes;
mod writes;

/// Waits up to ~1 s for `app.workers.redirect_fetcher` to report something.
/// Under test (no content dir) every fetch fails at once.
fn drain_fetches(app: &mut App) -> Vec<FetchResult> {
    for _ in 0..200 {
        let results = app.workers.redirect_fetcher.tick();
        if !results.is_empty() {
            return results;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Vec::new()
}

fn shared(kind: Kind, group: Option<NodeId>, set: &[(u32, FieldValue)]) -> Snapshot {
    let mut fields: Fields =
        kind.fields().iter().map(|&no| (no, FieldValue::absent(field_def(no).unwrap().ty))).collect();
    for (no, value) in set {
        fields.insert(*no, value.clone());
    }
    Snapshot { ty: kind.type_str().unwrap().to_owned(), group, fields, trees: Default::default() }
}

fn note(text: &str, group: Option<NodeId>) -> Snapshot {
    shared(Kind::Note, group, &[(field::TEXT, FieldValue::Str(text.into()))])
}

fn group(name: &str) -> Snapshot {
    shared(Kind::Group, None, &[(field::NAME, FieldValue::Str(name.into()))])
}

/// A remote change's snapshot of a `kind` node: its type string, `fields` and `group`.
fn snap(kind: Kind, fields: Fields, group: Option<NodeId>) -> Snapshot {
    Snapshot { ty: kind.type_str().unwrap().to_owned(), group, fields, trees: Default::default() }
}

/// What `doc` shows, shaped like `snapshots`.
fn shown(doc: &Doc) -> BTreeMap<NodeId, Snapshot> {
    doc.shown().map(|(id, node)| (id, Snapshot::of(doc, id, node))).collect()
}

/// What the store holds for every live node (`App::snapshot`), by id: an in-progress download and a tab whose spawn is
/// in flight are left out. `pub(crate)`: other modules' tests compare it too.
pub(crate) fn snapshots(app: &App) -> BTreeMap<NodeId, Snapshot> {
    app.nodes.keys().filter_map(|&id| Some((id, app.snapshot(id)?))).collect()
}

/// Actor 1's op count: what a save wrote.
fn ops_of(app: &App) -> u64 {
    app.doc.journal.as_ref().unwrap().doc().version.get(&ActorId(1)).copied().unwrap_or(0)
}

/// Whether the document shows `id` live.
fn live_in_doc(app: &App, id: NodeId) -> bool {
    app.doc.journal.as_ref().unwrap().doc().nodes.get(&id).is_some_and(Node::is_live)
}

fn is_inside(model: &BTreeMap<NodeId, (Kind, Option<NodeId>)>, node: NodeId, ancestor: NodeId) -> bool {
    let mut at = model.get(&node).and_then(|&(_, g)| g);
    while let Some(g) = at {
        if g == ancestor {
            return true;
        }
        at = model.get(&g).and_then(|&(_, g)| g);
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

/// An App holding one Json node, id `node(1)`, loaded from a temp file.
fn app_with_json_file(name: &str) -> (App, NodeId) {
    let dir = crate::store::testutil::temp_dir(name);
    let path = dir.join("v.json");
    std::fs::write(&path, r#"{"users": [{"name": "a"}]}"#).unwrap();
    let mut app = App::default();
    let node = node(1);
    app.insert_json_node_with_id(node, path.to_string_lossy().into_owned(), egui::Vec2::ZERO, None).unwrap();
    (app, node)
}

/// A windowless app saving to `path` as `actor`. `pub(crate)`: also
/// used by `main.rs`'s own windowless tests (e.g. `duplicate`'s).
pub(crate) fn app_on(path: &std::path::Path, actor: u64) -> App {
    let mut app = App::default();
    app.doc.journal = Some(Journal::open(path, ActorId(actor)).unwrap());
    app
}

/// A node of `kind` at the top level with `runtime` as its live state; its type is seeded from `kind`.
pub(crate) fn add(app: &mut App, kind: Kind, runtime: Runtime) -> NodeId {
    let id = app.alloc_node_id();
    app.insert_placeholder_node(id, egui::Vec2::ZERO, kind, runtime);
    id
}

/// A group named `name` at the top level (`App::place_group`).
pub(crate) fn add_group(app: &mut App, name: &str) -> NodeId {
    let id = app.alloc_node_id();
    app.place_group(Some(id), name, crate::fetched::Placement { group: None, pos: egui::Vec2::ZERO })
}

/// Another replica sets one field of `node`.
pub(crate) fn remote_set(other: &mut Journal, node: NodeId, no: u32, value: FieldValue) {
    other.begin().unwrap().commit(vec![OpBody::set(node, no, encode_field(no, &value))]).unwrap();
}

/// Another replica's write of `leaf` (`None`: a tombstone) at `path` in `node`'s tree field `field`.
pub(crate) fn remote_set_tree(other: &mut Journal, node: NodeId, field: u32, path: TreePath, leaf: Option<Leaf>) {
    let enc = leaf.map_or_else(Vec::new, |l| l.encode());
    other.begin().unwrap().commit(vec![OpBody::Set { node, field, path, enc }]).unwrap();
}

/// `node`'s field `no` as the document stores it.
pub(crate) fn stored(app: &App, node: NodeId, no: u32) -> FieldValue {
    app.doc.journal.as_ref().unwrap().doc().nodes[&node].get(no)
}

pub(crate) fn add_note(app: &mut App, text: &str) -> NodeId {
    add(app, Kind::Note, Runtime::Note(NotePayload { text: text.into(), editing: false }))
}

pub(crate) fn closed_ids(app: &App) -> Vec<NodeId> {
    app.doc.journal.as_ref().unwrap().doc().closed_roots().into_iter().map(|(n, _)| n).collect()
}

/// A group with one kept note inside, saved.
pub(crate) fn group_with_note(app: &mut App) -> (NodeId, NodeId) {
    let g = add_group(app, "G");
    let inner = add_note(app, "kept");
    app.edit_group(inner, Some(g));
    assert!(app.save_session());
    (g, inner)
}

/// Another replica sets `closed` on `node` (true closes, false reopens).
fn remote_close(other: &mut Journal, node: NodeId, closed: bool) {
    remote_set(other, node, field::CLOSED, FieldValue::Replica(if closed { 99 } else { 0 }));
}

/// The note's text box has focus and holds `text` (the payload is the edit buffer).
fn type_into(app: &mut App, note: NodeId, text: &str) {
    let n = app.nodes.get_mut(&note).unwrap().as_note_mut().unwrap();
    n.editing = true;
    n.text = text.into();
}

fn buffer(app: &App, note: NodeId) -> String {
    app.nodes[&note].as_note().unwrap().text.clone()
}

/// A Json node `v.json` (`{"users": [...]}`) with a store id, in an app saving to `path`.
fn json_on(path: &std::path::Path, name: &str) -> (App, NodeId) {
    let file = temp_dir(name).join("v.json");
    std::fs::write(&file, r#"{"users": [{"name": "a"}]}"#).unwrap();
    let mut app = app_on(path, 1);
    let node = app.alloc_node_id();
    app.insert_json_node_with_id(node, file.to_string_lossy().into_owned(), egui::Vec2::ZERO, None).unwrap();
    (app, node)
}

/// A note placed at `pos` (top level), as `place_node` places every new node.
fn a_note_at(app: &mut App, pos: egui::Vec2) -> NodeId {
    let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
    app.place_node(None, Kind::Note, note, crate::fetched::Placement { group: None, pos })
}

/// What a drag does to the layout every frame (`card.rs`), then its release.
fn drag(app: &mut App, ids: &[NodeId], delta: egui::Vec2) {
    app.interaction.dragging = Some(crate::card_id::CardId::from(ids[0]));
    app.interaction.drag_moved = ids.iter().map(|&id| crate::card_id::CardId::from(id)).collect();
    for &id in ids {
        app.layout.entry(id).or_default().pos += delta;
    }
}

#[derive(serde::Serialize)]
struct Wrap(#[serde(with = "crate::process_manager::view_kind_serde")] ViewKind);

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
fn succeed_spawn(app: &mut App, url: &str) -> NodeId {
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

fn failed_cards(app: &App) -> Vec<NodeId> {
    let mut ids: Vec<NodeId> = app
        .nodes
        .iter()
        .filter(|(_, node)| node.as_unloaded().is_some_and(|tab| matches!(tab.state, UnloadedState::Failed(_))))
        .map(|(&id, _)| id)
        .collect();
    ids.sort();
    ids
}

fn json_node(id: &str, kind: Kind, fields: serde_json::Value) -> crate::store::json::JsonNode {
    let serde_json::Value::Object(mut fields) = fields else { panic!("fields are an object") };
    fields.insert("type".into(), kind.type_str().expect("a known kind").into());
    crate::store::json::JsonNode { id: id.into(), fields }
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

/// Two distinct values of a field every card has, for tests of how edits sync and merge.
fn one() -> FieldValue {
    FieldValue::Str("one".into())
}

fn two() -> FieldValue {
    FieldValue::Str("two".into())
}

fn a_note(app: &mut App) -> NodeId {
    add(app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }))
}

/// What the crash path and `finish_swap` do: carry from the mirror, take the node out of `nodes`, respawn. The
/// spawn carries no position: the tab keeps its `App::layout` entry.
fn start_respawn(app: &mut App, tab: NodeId) {
    app.nodes.remove(&tab).unwrap();
    let carried = app.doc.mirror.carried(tab);
    let mut spawn = crate::tabs::TabSpawn::new(
        tab,
        "https://t/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Respawn,
        1.0,
    );
    spawn.carry.pinned = carried.pinned();
    (spawn.jar, spawn.parent, spawn.group) = (carried.jar(), carried.parent, carried.group);
    app.respawn_tab(spawn);
    assert!(app.spawns.spawning.contains_key(&tab) && !app.nodes.contains_key(&tab), "respawning");
}

/// O holds G, G holds T, saved.
fn nested_tab(app: &mut App) -> (NodeId, NodeId, NodeId) {
    let outer = add_group(app, "O");
    let g = add_group(app, "G");
    let t = add(app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_group(g, Some(outer));
    app.edit_group(t, Some(g));
    assert!(app.save_session());
    (outer, g, t)
}

/// A opened P, P opened C, saved.
fn parent_chain(app: &mut App) -> (NodeId, NodeId, NodeId) {
    let a = add(app, Kind::Tab, sleeping_tab("https://a/", ""));
    let p = add(app, Kind::Tab, sleeping_tab("https://p/", ""));
    let c = add(app, Kind::Tab, sleeping_tab("https://c/", ""));
    app.edit_parent(p, Some(a));
    app.edit_parent(c, Some(p));
    assert!(app.save_session());
    (a, p, c)
}

/// The page moves on (an engine report): the live url changes, the shared one doesn't.
fn set_live_url(app: &mut App, tab: NodeId, url: &str) {
    match &mut app.nodes.get_mut(&tab).unwrap().runtime {
        Runtime::Tab(t) => t.url = url.into(),
        Runtime::Unloaded(t) => t.url = url.into(),
        _ => panic!("{tab} is not a tab"),
    }
}

pub(crate) type Peer = crate::process_manager::FakePeer;

/// A live tab on `kind` (a fake host, `ViewTab::fake`), as a landing inserts it. The peer reads what it is sent.
pub(crate) fn live_tab(app: &mut App, kind: ViewKind, url: &str) -> (NodeId, Peer) {
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

fn view_kind(kind: ViewKind) -> FieldValue {
    FieldValue::ViewCode(view_code(kind))
}

fn swap_target(app: &App, tab: NodeId) -> Option<ViewKind> {
    app.nodes[&tab].as_tab().unwrap().swap_target
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

/// A new tab spawned with `jar`, as `spawn_tab_at` starts one.
fn spawn_with_jar(app: &mut App, purpose: SpawnPurpose, jar: NodeId) -> NodeId {
    let t = app.alloc_node_id();
    let mut spawn = crate::tabs::TabSpawn::new(t, "https://t/".into(), ViewKind::Servo, egui::Vec2::ZERO, purpose, 1.0);
    spawn.jar = Some(jar);
    app.spawn_tab_async(spawn);
    t
}

fn flushed_failure_toasts(app: &mut App) -> Vec<String> {
    app.toasts.failure_notices.flush(std::time::Instant::now() + crate::failure_toasts::WINDOW)
}

use crate::caps::{LINK, OPENED_FROM};
use crate::links::ConnectError;

fn link_count(app: &App) -> usize {
    app.doc.mirror.link_ids().count()
}

fn link_doc(nodes: Vec<crate::store::json::JsonNode>) -> JsonDoc {
    JsonDoc { nodes, local: None }
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
        fields.extend(encode_field(field::POS, &FieldValue::Pos(Some((5.0, 5.0)))));
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
    app.doc.journal.as_ref().unwrap().doc().nodes[&node].fields.keys().map(|k| k.field).collect()
}
