use super::*;
use crate::redirect_fetch::{FetchResult, FetchTarget};
use crate::store::journal::Journal;
use crate::store::model::{ActorId, DocId, Hlc};
use crate::store::ops::Op;
use crate::store::testutil::{temp_dir, XorShift};
use crate::SpawnPurpose;

const A: ActorId = ActorId(0xA);

#[test]
fn jar_fields_belong_to_tabs_groups_and_jars() {
    use field::*;
    assert_eq!(kind_fields(Kind::Tab), &[POS, COLOR, PARENT, URL, ENGINE, PINNED, JAR, JAR_SET]);
    assert_eq!(kind_fields(Kind::Group), &[POS, COLOR, NAME, FROM_JSON, DEFAULT_JAR]);
    assert_eq!(kind_fields(Kind::Jar), &[COLOR, NAME, EPHEMERAL]);
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
    add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
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

/// Applies `bodies` to `doc` as actor `A`'s next ops.
fn apply(doc: &mut Doc, bodies: Vec<OpBody>) {
    for body in bodies {
        let counter = doc.version.get(&A).copied().unwrap_or(0).max(1000);
        let hlc = Hlc { ms: doc.clock.ms + 1, logical: 0 };
        doc.apply(&Op { id: id(counter), hlc, body });
    }
}

fn shared(kind: Kind, group: Option<NodeId>, set: &[(u32, FieldValue)]) -> Shared {
    let mut fields: Fields =
        kind_fields(kind).iter().map(|&no| (no, FieldValue::absent(field_def(no).unwrap().ty))).collect();
    for (no, value) in set {
        fields.insert(*no, value.clone());
    }
    Shared { kind, group, fields }
}

fn note(text: &str, group: Option<NodeId>) -> Shared {
    shared(Kind::Note, group, &[(field::TEXT, FieldValue::Str(text.into()))])
}

fn group(name: &str) -> Shared {
    shared(Kind::Group, None, &[(field::NAME, FieldValue::Str(name.into()))])
}

/// What `doc` shows, shaped like `live`.
fn shown(doc: &Doc) -> BTreeMap<NodeId, Shared> {
    doc.live()
        .map(|(id, node)| (id, Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) }))
        .collect()
}

fn all_explicit(live: &BTreeMap<NodeId, Shared>) -> BTreeSet<(NodeId, u32)> {
    live.keys().flat_map(|&id| [(id, field::POS), (id, field::URL)]).collect()
}

#[test]
fn new_nodes_are_created_groups_first() {
    let doc = Doc::new(DocId([1; 16]));
    // The member's id sorts before its group's.
    let live = BTreeMap::from([(id(0), note("inside", Some(id(1)))), (id(1), group("G"))]);
    let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
    assert!(matches!(ops[0], OpBody::Create { node, .. } if node == id(1)));
    let mut doc = doc;
    apply(&mut doc, ops);
    assert_eq!(shown(&doc), live);
}

#[test]
fn an_unchanged_graph_emits_nothing() {
    let mut doc = Doc::new(DocId([1; 16]));
    let live = BTreeMap::from([(id(0), note("a", None))]);
    let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
    apply(&mut doc, ops);
    assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
}

#[test]
fn explicit_fields_change_only_when_marked() {
    let mut doc = Doc::new(DocId([1; 16]));
    let mut live = BTreeMap::from([(id(0), note("a", None))]);
    let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
    apply(&mut doc, ops);
    live.get_mut(&id(0)).unwrap().fields.insert(field::POS, FieldValue::Pos(5.0, 6.0));
    assert_eq!(diff(&doc, &live, &BTreeSet::new(), &BTreeSet::new()), vec![], "physics motion is not an edit");
    let marked = BTreeSet::from([(id(0), field::POS)]);
    assert!(matches!(&diff(&doc, &live, &marked, &BTreeSet::new())[..], [OpBody::Set { field: field::POS, .. }]));
}

#[test]
fn a_missing_node_is_deleted_unless_kept() {
    let mut doc = Doc::new(DocId([1; 16]));
    let live = BTreeMap::from([(id(0), note("a", None))]);
    let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
    apply(&mut doc, ops);
    let empty = BTreeMap::new();
    assert_eq!(diff(&doc, &empty, &BTreeSet::new(), &BTreeSet::from([id(0)])), vec![]);
    assert_eq!(diff(&doc, &empty, &BTreeSet::new(), &BTreeSet::new()), vec![OpBody::Delete { node: id(0) }]);
}

#[test]
fn a_tombstone_is_never_recreated() {
    let mut doc = Doc::new(DocId([1; 16]));
    let live = BTreeMap::from([(id(0), note("a", None))]);
    let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
    apply(&mut doc, ops);
    apply(&mut doc, vec![OpBody::Delete { node: id(0) }]);
    let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
    assert!(!ops.iter().any(|op| matches!(op, OpBody::Create { .. })), "{ops:?}");
}

#[test]
fn re_homing_out_of_a_deleted_group_is_not_a_move() {
    let mut doc = Doc::new(DocId([1; 16]));
    let live = BTreeMap::from([(id(0), note("a", Some(id(1)))), (id(1), group("G"))]);
    let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
    apply(&mut doc, ops);
    // Another replica deleted the group only; the note is shown at the top.
    apply(&mut doc, vec![OpBody::Delete { node: id(1) }]);
    let rehomed = BTreeMap::from([(id(0), note("a", None))]);
    assert_eq!(diff(&doc, &rehomed, &BTreeSet::new(), &BTreeSet::new()), vec![]);
}

#[test]
fn a_parent_naming_a_missing_node_is_written_as_none() {
    let doc = Doc::new(DocId([1; 16]));
    let tab = shared(Kind::Tab, None, &[(field::PARENT, FieldValue::Id(Some(id(9))))]);
    let live = BTreeMap::from([(id(0), tab)]);
    let mut doc2 = doc.clone();
    apply(&mut doc2, diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()));
    assert_eq!(doc2.nodes[&id(0)].get(field::PARENT), FieldValue::Id(None));
}

#[test]
fn random_edits_converge_in_one_diff() {
    let mut rng = XorShift(0x5E55);
    let mut doc = Doc::new(DocId([1; 16]));
    let mut live: BTreeMap<NodeId, Shared> = BTreeMap::new();
    let mut next = 0;
    for _ in 0..200 {
        match rng.below(5) {
            0 | 1 => {
                let groups: Vec<NodeId> =
                    live.iter().filter(|(_, s)| s.kind == Kind::Group).map(|(id, _)| *id).collect();
                let group =
                    (!groups.is_empty() && rng.below(2) == 0).then(|| groups[rng.below(groups.len() as u64) as usize]);
                let node = if rng.below(3) == 0 { group_in("g", group) } else { note("n", group) };
                live.insert(id(next), node);
                next += 1;
            }
            2 if !live.is_empty() => {
                let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                live.get_mut(&key)
                    .unwrap()
                    .fields
                    .insert(field::COLOR, FieldValue::Color(Some(rng.below(1 << 24) as u32)));
            }
            3 if !live.is_empty() => {
                let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                // Deleting a group deletes what it shows, as the app does.
                let doomed: Vec<NodeId> =
                    live.keys().filter(|&&n| n == key || is_inside(&live, n, key)).copied().collect();
                for n in doomed {
                    live.remove(&n);
                }
            }
            4 if !live.is_empty() => {
                let key = *live.keys().nth(rng.below(live.len() as u64) as usize).unwrap();
                let groups: Vec<NodeId> =
                    live.iter().filter(|(_, s)| s.kind == Kind::Group).map(|(id, _)| *id).collect();
                // A move into `key` itself or one of its descendants would create a cycle.
                let candidates: Vec<Option<NodeId>> = std::iter::once(None)
                    .chain(groups.into_iter().map(Some))
                    .filter(|&g| g != Some(key) && !g.is_some_and(|g| is_inside(&live, g, key)))
                    .collect();
                let choice = candidates[rng.below(candidates.len() as u64) as usize];
                live.get_mut(&key).unwrap().group = choice;
            }
            _ => {}
        }
        let ops = diff(&doc, &live, &all_explicit(&live), &BTreeSet::new());
        apply(&mut doc, ops);
        assert_eq!(shown(&doc), live);
        assert_eq!(diff(&doc, &live, &all_explicit(&live), &BTreeSet::new()), vec![]);
    }
}

fn group_in(name: &str, parent: Option<NodeId>) -> Shared {
    Shared { group: parent, ..group(name) }
}

fn is_inside(live: &BTreeMap<NodeId, Shared>, node: NodeId, ancestor: NodeId) -> bool {
    let mut at = live.get(&node).and_then(|s| s.group);
    while let Some(g) = at {
        if g == ancestor {
            return true;
        }
        at = live.get(&g).and_then(|s| s.group);
    }
    false
}

/// Materializes `nodes` into a windowless app, as `apply_remote` would
/// show another replica's new nodes.
fn app_with(nodes: &BTreeMap<NodeId, Shared>) -> App {
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
    let tab = &app.nodes[&id(1).to_string()];
    assert_eq!(tab.payload.as_unloaded().unwrap().jar, crate::jar::JarChoice { jar: Some(jar), set: true });
    assert_eq!(app.nodes[&id(2).to_string()].payload.as_group().unwrap().default_jar, Some(jar));
    assert_eq!(app.shared_nodes(), nodes, "the live payloads project back to the same fields");
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
    assert_eq!(app.nodes[&id(1).to_string()].payload.as_unloaded().unwrap().jar.jar, Some(jar));
    assert_eq!(app.nodes[&id(2).to_string()].payload.as_group().unwrap().default_jar, Some(jar));
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
    assert_eq!(app_with(&nodes).shared_nodes(), nodes);
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
    let NodePayload::Json(json) = &mut app.nodes.get_mut(&node).unwrap().payload else { unreachable!() };
    json.view.entry("/users").collapsed = Some(true);

    let shared = app.shared_nodes();
    let fields = &shared[&id(1)].fields;
    assert_eq!(fields.get(&field::VIEW), Some(&FieldValue::Str(r#"{"/users":{"collapsed":true}}"#.into())));

    // Restoring those fields gives the same view back.
    let mut restored = App::default();
    restored.materialize(node.clone(), Kind::Json, fields, None, None);
    let json = restored.nodes[&node].as_json().unwrap();
    assert_eq!(json.view.get("/users").unwrap().collapsed, Some(true));
}

#[test]
fn a_remote_view_change_updates_the_node_in_place() {
    let (mut app, node) = app_with_json_file("json_view_remote");
    let (value, path) = app.nodes[&node].as_json().map(|j| (j.value.clone(), j.path.clone())).unwrap();
    let mut fields = app.shared_nodes()[&id(1)].fields.clone();
    fields.insert(field::VIEW, FieldValue::Str(r#"{"":{"collapsed":true}}"#.into()));
    app.materialize(node.clone(), Kind::Json, &fields, None, None);
    let json = app.nodes[&node].as_json().unwrap();
    assert_eq!(json.view.get("").unwrap().collapsed, Some(true));
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
    match &app.nodes[&node_id].payload {
        NodePayload::Json(p) => {
            assert!(p.error.is_none(), "a known-URL miss never reaches insert_json_node_with_id's own toast");
            assert_eq!(p.url.as_deref(), Some("https://x.test/a.json"));
        }
        _ => panic!("expected a Json placeholder"),
    }
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
    let error = |n: u64| match &app.nodes[&id(n).to_string()].payload {
        NodePayload::Json(p) => p.error.clone(),
        NodePayload::Image(p) => p.error.clone(),
        NodePayload::Pdf(p) => p.outcome.as_ref().and_then(|o| o.as_ref().err()).cloned(),
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
    let payload = NodePayload::Note(NotePayload { text: "local".into(), editing: false, url: None });
    app.insert_placeholder_node(local.clone(), egui::Vec2::ZERO, payload);
    app.nodes.get_mut(&local).unwrap().group = Some(id(1).to_string());
    app.current_group = Some(id(1).to_string());
    // What `remote_changes` reports when another replica deletes Inner:
    // the group gone, its stored member now shown in Outer.
    let member = nodes[&id(2)].fields.clone();
    app.apply_remote(vec![(id(1), None), (id(2), Some((Kind::Note, member, Some(id(0)))))]);
    let inner = id(1).to_string();
    assert!(!app.nodes.contains_key(&inner) && !app.node_order.contains(&inner));
    let outer = Some(id(0).to_string());
    assert_eq!(app.nodes[&id(2).to_string()].group, outer);
    assert_eq!(app.nodes[&local].group, outer, "the local-only member moves to the effective group");
    assert_eq!(app.current_group, outer, "the view leaves the removed level");
    assert!(app.nodes.values().all(|n| n.group.as_deref() != Some(inner.as_str())));
}

/// A windowless app saving to `path` as `actor`. `pub(crate)`: also
/// used by `main.rs`'s own windowless tests (e.g. `closed_entry`'s).
pub(crate) fn app_on(path: &std::path::Path, actor: u64) -> App {
    App { journal: Some(Journal::open(path, ActorId(actor)).unwrap()), ..App::default() }
}

pub(crate) fn add(app: &mut App, payload: NodePayload) -> String {
    let id = app.alloc_node_id();
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, payload);
    id
}

/// Another replica sets one field of `node`.
fn remote_set(other: &mut Journal, node: &str, no: u32, value: FieldValue) {
    let node = node.parse().unwrap();
    other.begin().unwrap().commit(vec![OpBody::Set { node, field: no, enc: encode_field(no, &value) }]).unwrap();
}

fn stored(app: &App, node: &str, no: u32) -> FieldValue {
    app.journal.as_ref().unwrap().doc().nodes[&node.parse().unwrap()].get(no)
}

#[test]
fn a_remote_edit_to_one_field_keeps_an_unsaved_local_edit_to_another() {
    let path = temp_dir("sync-fields").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, NodePayload::Note(NotePayload { text: "v1".into(), editing: false, url: None }));
    let failed = UnloadedTabPayload {
        url: "https://u0/".into(),
        title: String::new(),
        kind: EngineKind::Servo,
        pinned: false,
        muted: false,
        zoom_factor: 1.0,
        state: UnloadedState::Failed("x".into()),
        jar: Default::default(),
    };
    let tab = add(&mut app, NodePayload::Unloaded(failed));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();

    // Unsaved here: typing in the note, and a navigation (`url` is
    // explicit and unmarked, so saves don't write it).
    if let NodePayload::Note(n) = &mut app.nodes.get_mut(&note).unwrap().payload {
        n.text = "typing".into();
    }
    if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
        t.url = "https://u1/".into();
    }
    let red = color_value(Some(egui::Color32::RED));
    remote_set(&mut other, &note, field::COLOR, red.clone());
    remote_set(&mut other, &tab, field::PINNED, FieldValue::Bool(true));

    // The remote round applies, then the next round commits.
    assert!(app.save_session());
    let live = &app.nodes[&note];
    assert_eq!(live.color, Some(egui::Color32::RED));
    assert!(matches!(&live.payload, NodePayload::Note(n) if n.text == "typing"));
    assert_eq!(stored(&app, &note, field::TEXT), FieldValue::Str("typing".into()));
    assert!(matches!(&app.nodes[&tab].payload, NodePayload::Unloaded(t) if t.pinned && t.url == "https://u1/"));
    assert_eq!(stored(&app, &tab, field::URL), FieldValue::Str("https://u0/".into()));

    // A later remote edit to the same field does apply.
    remote_set(&mut other, &note, field::TEXT, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert!(matches!(&app.nodes[&note].payload, NodePayload::Note(n) if n.text == "theirs"));
    assert_eq!(app.nodes[&note].color, Some(egui::Color32::RED));
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

#[test]
fn a_failed_commit_keeps_the_explicit_marks() {
    let path = temp_dir("sync-fail").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
    assert!(app.save_session());
    app.nodes.get_mut(&note).unwrap().pos = egui::vec2(5.0, 6.0);
    let mark = (note.parse().unwrap(), field::POS);
    app.explicit.insert(mark);
    let set_readonly = |readonly: bool| {
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        // Toggles the Windows read-only attribute, which is what makes
        // the append fail; on Unix this would make the file writable
        // by everyone, but it's a temp file this test owns.
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(readonly);
        std::fs::set_permissions(&path, perms).unwrap();
    };
    set_readonly(true);
    let saved = app.save_session();
    set_readonly(false);
    assert!(!saved);
    assert!(app.explicit.contains(&mark), "kept for the next save");
    // Nothing else happens, so only the retry saves it.
    app.poll_store();
    assert!(app.explicit.contains(&mark), "not retried before SAVE_RETRY");
    app.save_retry_at = Some(std::time::Instant::now());
    app.poll_store();
    assert!(app.save_retry_at.is_none());
    assert!(app.explicit.is_empty());
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(5.0, 6.0));
}

#[test]
fn a_marked_pos_is_saved_and_an_unmarked_one_is_not() {
    let path = temp_dir("marks").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
    app.nodes.get_mut(&note).unwrap().pos = egui::vec2(1.0, 1.0);
    assert!(app.save_session());
    app.nodes.get_mut(&note).unwrap().pos = egui::vec2(50.0, 50.0);
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(1.0, 1.0), "physics motion stays local");
    app.mark(&note, field::POS);
    assert!(app.save_session());
    assert_eq!(stored(&app, &note, field::POS), FieldValue::Pos(50.0, 50.0));
}

#[test]
fn a_read_only_store_saves_nothing() {
    let path = temp_dir("sync-readonly").join("session.mnf");
    drop(Journal::open(&path, ActorId(1)).unwrap());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[4] = 2; // a newer format version, as in `journal`'s own test
    let crc = crate::store::crc32c::crc32c(&bytes[..21]);
    bytes[21..25].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let mut app = app_on(&path, 2);
    add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
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
    let tab = add(
        &mut app,
        NodePayload::Unloaded(UnloadedTabPayload {
            url: "https://x/".into(),
            title: String::new(),
            kind: EngineKind::Servo,
            pinned: false,
            muted: false,
            zoom_factor: 1.0,
            state: UnloadedState::Failed("x".into()),
            jar: Default::default(),
        }),
    );
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
        let id = add(&mut app, NodePayload::Note(NotePayload { text: String::new(), editing: false, url: None }));
        app.nodes.get_mut(&id).unwrap().pos = egui::vec2(3.0, 4.0);
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
/// (content dispatch spec): `shared_nodes` reports it under `field::URL`
/// and `restore_session` hands it back via `GraphNode::source_url`.
#[test]
fn content_nodes_keep_their_source_url() {
    let path = crate::store::testutil::temp_dir("content-url").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = app.alloc_node_id();
    let payload = NotePayload { text: "hi".into(), editing: false, url: Some("https://x.test/a.txt".into()) };
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, NodePayload::Note(payload));
    let shared = app.shared_nodes();
    let fields = &shared[&id.parse().unwrap()].fields;
    assert_eq!(fields.get(&field::URL), Some(&FieldValue::Str("https://x.test/a.txt".into())));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.nodes[&id].source_url(), Some("https://x.test/a.txt"));
}

/// A finished download keeps the address its engine fetched it from across a restart.
#[test]
fn a_completed_download_keeps_its_url() {
    let path = crate::store::testutil::temp_dir("download-url").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = app.alloc_node_id();
    let payload = DownloadPayload {
        path: "f.bin".into(),
        url: Some("https://x.test/f.bin".into()),
        bytes_received: 0,
        total_bytes: None,
        completed: true,
        failed: false,
    };
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, NodePayload::Download(payload));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.nodes[&id].as_download().and_then(|d| d.url.as_deref()), Some("https://x.test/f.bin"));
}

/// A download saved before cards kept their URL (or by an engine that can't say) restores with
/// none, so its card falls back to showing the file's path.
#[test]
fn a_download_without_a_url_restores_without_one() {
    let path = crate::store::testutil::temp_dir("download-no-url").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = app.alloc_node_id();
    let payload = DownloadPayload {
        path: "f.bin".into(),
        url: None,
        bytes_received: 0,
        total_bytes: None,
        completed: true,
        failed: false,
    };
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, NodePayload::Download(payload));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.nodes[&id].as_download().map(|d| d.url.clone()), Some(None));
}

#[test]
fn path_is_explicit() {
    assert!(crate::store::model::field_def(field::PATH).unwrap().explicit);
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
    app.nodes.get_mut(&n).unwrap().group = Some(g.clone());
    let json = app.export_group_json(&g).unwrap();
    assert_eq!(json.nodes.len(), 2);
    assert_eq!(json.nodes[0].kind, "group");
    app.import_group_json(&json);
    assert_eq!(app.shared_nodes().len(), 4, "the import added a second group and note");
}

fn failed_tab(url: &str, zoom_factor: f64) -> NodePayload {
    NodePayload::Unloaded(UnloadedTabPayload {
        url: url.into(),
        title: String::new(),
        kind: EngineKind::Servo,
        pinned: false,
        muted: false,
        zoom_factor,
        state: UnloadedState::Failed("x".into()),
        jar: Default::default(),
    })
}

/// A tab restored asleep, as `add` inserts it.
pub(crate) fn sleeping_tab(url: &str, title: &str) -> NodePayload {
    NodePayload::Unloaded(UnloadedTabPayload {
        url: url.into(),
        title: title.into(),
        kind: EngineKind::Servo,
        pinned: false,
        muted: false,
        zoom_factor: 1.0,
        state: UnloadedState::Sleeping,
        jar: Default::default(),
    })
}

/// An unloaded tab's title and mute survive a restart through personal
/// state, though it has no engine to report either.
#[test]
fn personal_state_keeps_an_unloaded_tabs_title_and_mute() {
    let mut app = app_on(&temp_dir("unloaded-local").join("session.mnf"), 1);
    let tab = add(&mut app, sleeping_tab("https://s/", "Saved title"));
    let set = |app: &mut App, title: &str, muted: bool| {
        if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
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
    let root = add(&mut app, sleeping_tab("https://root/", ""));
    let mid = add(&mut app, failed_tab("https://mid/", 1.0));
    let leaf = add(&mut app, sleeping_tab("https://leaf/", ""));
    app.nodes.get_mut(&mid).unwrap().parent = Some(root.clone());
    app.nodes.get_mut(&leaf).unwrap().parent = Some(mid.clone());
    app.close_node(&mid);
    assert!(!app.nodes.contains_key(&mid));
    assert_eq!(app.nodes[&leaf].parent.as_deref(), Some(root.as_str()));
}

/// A restore starts only the focused tab; every other tab is a
/// sleeping card with its saved title.
#[test]
fn a_restore_starts_only_the_focused_tab() {
    let path = temp_dir("lazy-restore").join("session.mnf");
    let (a, b, c) = {
        let mut app = app_on(&path, 1);
        let a = add(&mut app, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, sleeping_tab("https://b/", "B"));
        let c = add(&mut app, sleeping_tab("https://c/", "C"));
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
        let a = add(&mut app, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, sleeping_tab("https://b/", "B"));
        let c = add(&mut app, sleeping_tab("https://c/", "C"));
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
    let a = add(&mut app, sleeping_tab("https://a/", "Alpha"));
    let b = add(&mut app, sleeping_tab("https://b/", "Beta"));
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
    let a = add(&mut app, sleeping_tab("https://a/", "Alpha"));
    let b = add(&mut app, sleeping_tab("https://b/", "Beta"));
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
    assert!(app.nodes[&a].payload.pinned() && app.nodes[&b].payload.pinned());
    app.close_all_at_current_level();
    assert!(app.nodes.is_empty());
}

/// A failed wake leaves a failed card, still focused, and ends the
/// respawning placeholder (Tab view then falls back to Graph).
#[test]
fn a_failed_wake_leaves_a_failed_card() {
    let mut app = app_on(&temp_dir("lazy-fail").join("session.mnf"), 1);
    let a = add(&mut app, sleeping_tab("https://a/", "A"));
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
    let tab = add(&mut app, sleeping_tab("https://a/", "A"));
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
    let tab = add(&mut app, sleeping_tab("https://a/", "A"));
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
    let keep = add(&mut app, sleeping_tab("https://keep/", "Keep"));
    let other = add(&mut app, sleeping_tab("https://other/", "Other"));
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
    let tab = add(&mut app, sleeping_tab("https://s/", "S"));
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
        let tab = add(&mut app, sleeping_tab("https://s/", ""));
        if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
            t.pinned = true;
        }
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
    for (spawn, generation) in std::mem::take(&mut app.test_spawns) {
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
}

fn json_node(id: &str, kind: &str, fields: serde_json::Value) -> crate::store::json::JsonNode {
    let serde_json::Value::Object(fields) = fields else { panic!("fields are an object") };
    crate::store::json::JsonNode { id: id.into(), kind: kind.into(), fields }
}

#[test]
fn a_jar_def_is_saved_as_a_jar_node_and_survives_later_saves() {
    let dir = crate::store::testutil::temp_dir("jar-def-saved");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    add(&mut app, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    app.save_session();
    let doc = app.journal.as_ref().unwrap().doc();
    let node = doc.nodes.get(&jar).filter(|n| n.is_live()).expect("jar node created");
    assert_eq!(node.kind, Kind::Jar);
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
    assert_eq!(app.jar_defs[&jar].name, "Home");
    assert!(!app.nodes.contains_key(&jar.to_string()), "never a graph node");
    app.apply_remote(vec![(jar, Some((Kind::Jar, named("House").fields, None)))]);
    assert_eq!(app.jar_defs[&jar].name, "House");
    assert_eq!(app.shared_nodes()[&jar], named("House"), "projects back unchanged");
    app.apply_remote(vec![(jar, None)]);
    assert!(!app.jar_defs.contains_key(&jar));
}

#[test]
fn a_session_with_only_jars_still_restores_them() {
    let dir = crate::store::testutil::temp_dir("jar-only-restore");
    let path = dir.join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, sleeping_tab("https://a.com/", "A"));
    let jar = app.create_jar("Work");
    app.save_session();
    app.nodes.remove(&tab);
    app.save_session();
    drop(app);
    let mut back = app_on(&path, 3);
    assert!(!back.restore_session(), "no graph node: a fresh start");
    assert_eq!(back.jar_defs[&jar].name, "Work", "but the jar is kept, so the next save keeps it");
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
        add(&mut app, sleeping_tab("https://a.com/", "A"));
        let jar = app.create_jar("Work");
        assert!(app.save_session());
        let mut other = Journal::open(&path, ActorId(2)).unwrap();
        remote_set(&mut other, &jar.to_string(), no, value);
        app.delete_jar(jar);
        assert!(app.save_session());
        assert!(!app.jar_defs.contains_key(&jar), "field {no}: deleted here, not recreated");
        assert!(!app.journal.as_ref().unwrap().doc().nodes[&jar].is_live(), "field {no}: the delete was saved");
    }
}

/// Final review I2b: a jar deleted on another device while this one was not running never reaches `forget_jar`
/// here, so startup deletes the files of named jars the document no longer has, unless it may not know them all.
#[test]
fn startup_deletes_the_files_of_jars_the_document_no_longer_has() {
    let dir = crate::store::testutil::temp_dir("jar-prune-startup");
    let (path, jars) = (dir.join("session.mnf"), dir.join("jars"));
    std::fs::create_dir(&jars).unwrap();
    let mut app = app_on(&path, 1);
    add(&mut app, sleeping_tab("https://a.com/", "A"));
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
    let tab = add(&mut app, sleeping_tab("https://a.com/", "A"));
    let mut group = GroupPayload::named("G");
    group.default_jar = Some(jar);
    let grp = add(&mut app, NodePayload::Group(group));
    let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload else { panic!("sleeping tab") };
    t.jar = crate::jar::JarChoice { jar: Some(jar), set: true };
    app.delete_jar(jar);
    assert!(app.jar_defs.is_empty());
    assert_eq!(app.nodes[&tab].payload.as_unloaded().unwrap().jar.jar, None);
    assert_eq!(app.nodes[&grp].as_group().unwrap().default_jar, None);
}

/// A file with a jar "Work" (label j), a group defaulting to it and a tab in the group using it.
fn file_with_a_work_jar() -> JsonDoc {
    let json = serde_json::json!({ "nodes": [
        json_node("g", "group", serde_json::json!({ "name": "G", "default_jar": "j" })),
        json_node("j", "jar", serde_json::json!({ "name": "Work" })),
        json_node("t", "tab", serde_json::json!({ "url": "https://a.com/", "group": "g", "jar": "j", "jar_set": true })),
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
    let before = app.jar_defs[&work].clone();
    app.replace_running_session(&file_with_a_work_jar(), crate::store::json::Trust::Foreign);
    assert_eq!(app.jar_defs.len(), 2, "a second Work");
    assert_eq!(app.jar_defs[&work], before, "the existing jar is untouched");
    let new = *app.jar_defs.keys().find(|&&j| j != work).unwrap();
    assert_eq!(app.jar_defs[&new].name, "Work");
    let tab = app.nodes.values().find_map(|n| n.payload.as_unloaded()).expect("tab imported");
    assert_eq!(tab.jar, crate::jar::JarChoice { jar: Some(new), set: true });
    let group = app.nodes.values().find_map(|n| n.as_group()).expect("group imported");
    assert_eq!(group.default_jar, Some(new));
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
        assert_eq!(app.jar_defs.len(), n);
    }
    let new: BTreeSet<NodeId> = app.jar_defs.keys().copied().filter(|&j| j != work).collect();
    let tabs: BTreeSet<NodeId> =
        app.nodes.values().filter_map(|n| n.payload.as_unloaded()).filter_map(|t| t.jar.jar).collect();
    let groups: BTreeSet<NodeId> = app.nodes.values().filter_map(|n| n.as_group()?.default_jar).collect();
    assert_eq!((&tabs, &groups), (&new, &new), "each import's tab and group name its own new jar");
}

/// A reopened tab comes back asleep, starting no engine, and keeps its
/// zoom and title (spec "Lazy tabs").
#[test]
fn a_reopened_tab_comes_back_asleep_with_its_zoom_and_title() {
    let mut app = app_on(&temp_dir("reopen-sleep").join("session.mnf"), 1);
    let tab = add(&mut app, sleeping_tab("https://r/", "Page R"));
    if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
        t.zoom_factor = 1.5;
    }
    let entry = crate::closed_entry(&app, &tab).unwrap();
    app.nodes.remove(&tab);
    app.node_order.retain(|t| t != &tab);
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
    let g = add(&mut app, NodePayload::Group(GroupPayload::named("G")));
    let tab = add(&mut app, failed_tab("https://i/", 1.0));
    app.nodes.get_mut(&tab).unwrap().group = Some(g.clone());
    let json = app.export_group_json(&g).unwrap();
    app.import_group_json(&json);
    assert!(app.test_spawns.is_empty());
    let (_, copy) = app.nodes.iter().find(|(id, n)| **id != tab && n.as_unloaded().is_some()).expect("a card");
    assert_eq!(copy.as_unloaded().unwrap().state, UnloadedState::Sleeping);
    let group = copy.group.as_deref().expect("in a group");
    assert!(group != g && app.nodes[group].as_group().is_some(), "in the imported group");
}

/// A member whose group isn't part of the imported file lands where
/// the file's root does (the level being viewed), not at the top level.
#[test]
fn an_imported_member_whose_group_is_outside_the_file_joins_the_roots_level() {
    let mut app = app_on(&temp_dir("import-outside").join("session.mnf"), 1);
    let level = add(&mut app, NodePayload::Group(GroupPayload::named("L")));
    let elsewhere = add(&mut app, NodePayload::Group(GroupPayload::named("E")));
    app.current_group = Some(level.clone());
    let json = JsonDoc {
        nodes: vec![
            json_node("a", "group", serde_json::json!({ "name": "G" })),
            json_node("b", "note", serde_json::json!({ "text": "t", "group": elsewhere })),
        ],
        local: None,
    };
    app.import_group_json(&json);
    let note = app.nodes.values().find(|n| matches!(&n.payload, NodePayload::Note(_))).expect("imported");
    assert_eq!(note.group.as_deref(), Some(level.as_str()));
}

/// A session with no nodes still keeps Recently Closed: the launch
/// starts fresh, but the next personal-state write carries the list.
#[test]
fn an_empty_session_keeps_recently_closed_across_a_launch() {
    let path = temp_dir("empty-closed").join("session.mnf");
    let entry = JsonDoc { nodes: vec![json_node("x", "note", serde_json::json!({ "text": "gone" }))], local: None };
    {
        let journal = Journal::open(&path, ActorId(1)).unwrap();
        let state = LocalState { closed: vec![entry.clone()], ..LocalState::default() };
        journal.write_local(&state.to_bytes()).unwrap();
    }
    let mut app = app_on(&path, 2);
    assert!(!app.restore_session(), "no nodes: the caller starts fresh");
    assert_eq!(app.closed.len(), 1);
    add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
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
        let note = add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
        app.close_store();
        note
    };
    let old = JsonDoc { nodes: vec![json_node("x", "note", serde_json::json!({ "text": "gone" }))], local: None };
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
    let since = JsonDoc { nodes: vec![json_node("y", "note", serde_json::json!({ "text": "since" }))], local: None };
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
    assert_eq!(app.nodes[&note].pos, egui::vec2(7.0, 8.0));
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
        add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
        app.close_store();
    }
    std::fs::write(crate::store::local::path_for(&path), b"MNFL damaged").unwrap();
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    assert!(app.local_unread.is_none());
    // An unchanged state isn't written (`local_written` is armed); a change is.
    add(&mut app, NodePayload::Note(NotePayload { text: "m".into(), editing: false, url: None }));
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
    add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
    assert!(app.save_session());
    let old = JsonDoc { nodes: vec![json_node("x", "note", serde_json::json!({ "text": "gone" }))], local: None };
    mark_unread_with(
        &mut app,
        &path,
        &LocalState { view_item: true, closed: vec![old.clone()], ..LocalState::default() },
    );
    let json =
        JsonDoc { nodes: vec![json_node("t", "tab", serde_json::json!({ "url": "https://new/" }))], local: None };
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
        let a = add(&mut app, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, sleeping_tab("https://b/", "B"));
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
    add(&mut app, NodePayload::Note(NotePayload { text: "n".into(), editing: false, url: None }));
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
    add(&mut app, NodePayload::Note(NotePayload { text: "old".into(), editing: false, url: None }));
    assert!(app.save_session());
    let json =
        JsonDoc { nodes: vec![json_node("t", "tab", serde_json::json!({ "url": "https://new/" }))], local: None };
    app.replace_running_session(&json, crate::store::json::Trust::Foreign);
    assert_eq!(app.test_spawns.len(), 1, "the tab is still spawning");
    app.save_session();
    app.close_store();
    drop(app);
    let journal = Journal::open(&path, ActorId(3)).unwrap();
    let live: Vec<(Kind, FieldValue)> = journal.doc().live().map(|(_, n)| (n.kind, n.get(field::URL))).collect();
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
            json_node("t1", "tab", serde_json::json!({ "url": "https://a/" })),
            json_node("t2", "tab", serde_json::json!({ "url": "https://b/" })),
            json_node("t3", "tab", serde_json::json!({ "url": "https://c/" })),
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
        let a = add(&mut app, sleeping_tab("https://a/", "A"));
        let b = add(&mut app, sleeping_tab("https://b/", "B"));
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
        let tab = add(&mut app, failed_tab("https://s/", 1.0));
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
    let card = add(&mut app, failed_tab("https://f/", 1.5));
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
        let tab = add(&mut app, failed_tab("https://s/", 1.0));
        assert!(app.save_session());
        if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
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

/// `save_session` refreshes `last_shared` only for nodes its ops name
/// (all of them after a delete), trusting `materialize` for every other
/// store change. After each kind of save, `last_shared` must still be
/// exactly what the store holds for every live node.
#[test]
fn last_shared_matches_the_store_after_every_save() {
    fn check(app: &App, step: &str) {
        let doc = app.journal.as_ref().unwrap().doc();
        for (id, node) in doc.live() {
            let want = Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) };
            assert_eq!(app.last_shared.get(&id.to_string()), Some(&want), "{step}: node {id}");
        }
    }
    let dir = temp_dir("last-shared");
    let path = dir.join("session.mnf");
    let mut app = app_on(&path, 1);
    let group = |name: &str| NodePayload::Group(GroupPayload::named(name));
    let outer = add(&mut app, group("outer"));
    let inner = add(&mut app, group("inner"));
    let tab = add(&mut app, failed_tab("https://a/", 1.0));
    let note = add(&mut app, NodePayload::Note(NotePayload { text: String::new(), editing: false, url: None }));
    app.nodes.get_mut(&inner).unwrap().group = Some(outer.clone());
    app.nodes.get_mut(&tab).unwrap().group = Some(inner.clone());
    assert!(app.save_session());
    check(&app, "creates");

    if let NodePayload::Note(n) = &mut app.nodes.get_mut(&note).unwrap().payload {
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

    if let NodePayload::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().payload {
        t.url = "https://b/".into();
    }
    app.mark(&tab, field::URL);
    assert!(app.save_session());
    check(&app, "a marked url");
}

/// What the app's side of a save costs at realistic session sizes: the
/// live projection (`shared_nodes`), a whole `save_session` (projection,
/// `begin`, `diff`, commit, `last_shared` refresh) with nothing and with
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
        let groups: Vec<String> = (0..tabs / 10)
            .map(|i| add(&mut app, NodePayload::Group(GroupPayload::named(format!("Group {i}")))))
            .collect();
        let mut ids = Vec::new();
        for i in 0..tabs {
            let url = format!("https://www.example{}.com/articles/2026/09/some-long-article-slug-{i}?ref=home", i % 50);
            let id = add(&mut app, failed_tab(&url, 1.0));
            let node = app.nodes.get_mut(&id).unwrap();
            node.group = Some(groups[i % groups.len()].clone());
            node.pos = egui::vec2(i as f32 * 3.5, i as f32 * -2.25);
            ids.push(id);
        }
        assert!(app.save_session());
        println!("{tabs} tabs, {} groups:", groups.len());

        report("shared_nodes (per save)", timed(N, || drop(app.shared_nodes())));
        let live = app.shared_nodes();
        let doc = app.journal.as_ref().unwrap().doc();
        report(
            "diff, nothing changed",
            timed(N, || assert!(diff(doc, &live, &app.explicit, &BTreeSet::new()).is_empty())),
        );
        report(
            "last_shared refresh (after a commit)",
            timed(N, || {
                for &id in live.keys() {
                    let node = &doc.nodes[&id];
                    let seen = Shared { kind: node.kind, group: doc.effective_group(id), fields: fields_of(node) };
                    std::hint::black_box(seen);
                }
            }),
        );
        report("save_session, nothing changed", timed(N, || assert!(app.save_session())));
        let mut k = 0;
        report(
            "save_session, one url changed",
            timed(N, || {
                let id = &ids[k % tabs];
                if let NodePayload::Unloaded(f) = &mut app.nodes.get_mut(id).unwrap().payload {
                    f.url = format!("https://moved.example/{k}");
                }
                app.mark(id, field::URL);
                k += 1;
                assert!(app.save_session());
            }),
        );
        report("local_state (every 30 s, and at exit)", timed(N, || drop(app.local_state())));
        report(
            "maybe_write_local, changed (build + write)",
            timed(N, || {
                app.nodes.get_mut(&ids[k % tabs]).unwrap().pos.x += 1.0;
                k += 1;
                app.maybe_write_local(true);
            }),
        );
        let bytes = app.local_state().to_bytes().len();
        println!("  session.mnf.local: {bytes} B");
    }
}
