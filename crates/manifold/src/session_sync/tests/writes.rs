//! Session-sync tests: save_session: the ops it builds, what it writes and when.

use super::*;
use crate::node::{ImagePayload, PdfPayload};

/// The commit builder: a save creates every queued node the document lacks, each group before the
/// nodes inside it, so a member's `Create` names a group that exists. The member's id sorts before its group's.
#[test]
fn new_nodes_are_created_groups_first() {
    let mut app = app_on(&temp_dir("create-groups-first").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "inside".into(), editing: false }));
    let group = add_group(&mut app, "G");
    assert!(note < group, "the member's id sorts first");
    app.edit_group(note, Some(group));
    assert!(app.save_session());
    assert!(!app.doc.writes.has_creates(), "created");
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(doc.effective_group(note), Some(group));
    assert_eq!(doc.nodes[&group].get(field::NAME), FieldValue::Str("G".into()));
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
/// (`spawning` is not a keep list: respawns never queue).
#[test]
fn a_removed_node_is_deleted_and_a_respawning_one_kept() {
    let mut app = app_on(&temp_dir("delete-unless-respawning").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "a".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.remove_node(note);
    start_respawn(&mut app, tab);
    assert!(app.save_session());
    assert!(!live_in_doc(&app, note), "deleted");
    assert!(live_in_doc(&app, tab), "kept while respawning");
    assert!(!app.doc.writes.has_deletes());
}

/// A queued create for an id the document holds as a tombstone is dropped (ids are never reused): here a tab
/// another replica deleted while it was waking, which is still shown until the wake lands.
#[test]
fn a_tombstone_is_never_recreated() {
    let path = temp_dir("tombstone-no-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.focus_tab(tab);
    assert!(app.spawns.spawning.contains_key(&tab) && app.nodes.contains_key(&tab), "waking: shown and in flight");
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: tab }]).unwrap();
    app.doc.writes.queue_create(tab, false);
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "no Create");
    assert!(!live_in_doc(&app, tab));
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
    app.edit_group(tab, Some(group));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: group }]).unwrap();
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert!(!app.nodes.contains_key(&group) && app.nodes.contains_key(&tab));
    assert_eq!(app.doc.mirror.group(tab), None, "shown at the top");
    assert_eq!(ops_of(&app), ops, "nothing written");
}

/// A new node's `parent` naming an id the document has never seen is written as none (one resolver for
/// every Id a `Create` carries).
#[test]
fn a_parent_naming_a_missing_node_is_written_as_none() {
    let mut app = app_on(&temp_dir("parent-missing").join("session.mnf"), 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    app.edit_parent(tab, Some(node(9)));
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::PARENT), FieldValue::Id(None));
}

/// A created node's parent or group that the same save deletes is written as none, not as the dead id:
/// a new tab opened from a tab into a group, both closed before its spawn landed.
#[test]
fn a_create_naming_a_node_deleted_by_the_same_save_writes_none() {
    let mut app = app_on(&temp_dir("create-names-deleted").join("session.mnf"), 1);
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://o/", ""));
    let group = add_group(&mut app, "G");
    assert!(app.save_session());
    let tab = app.spawn_tab_at("https://n/", egui::Vec2::ZERO, Some(group), Some(opener));
    assert!(app.spawns.spawning.contains_key(&tab));
    app.remove_tab(opener);
    app.remove_node(group);
    assert!(app.save_session());
    assert!(!live_in_doc(&app, opener) && !live_in_doc(&app, group));
    assert!(live_in_doc(&app, tab), "created from its spawn snapshot");
    assert_eq!(stored(&app, tab, field::PARENT), FieldValue::Id(None));
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().nodes[&tab].group, None);
}

/// Random local edits through the app's writers (place, group, rename, type, close), each followed by one save:
/// the document then shows exactly the live graph, and a second save writes nothing.
#[test]
fn random_edits_converge_in_one_save() {
    let mut rng = XorShift(0x5E55);
    let mut app = app_on(&temp_dir("random-converge").join("session.mnf"), 1);
    // The model: each live node's kind and group.
    let mut model: BTreeMap<NodeId, (Kind, Option<NodeId>)> = BTreeMap::new();
    let groups = |model: &BTreeMap<NodeId, (Kind, Option<NodeId>)>| -> Vec<NodeId> {
        model.iter().filter(|(_, (k, _))| *k == Kind::Group).map(|(&id, _)| id).collect()
    };
    for _ in 0..120 {
        match rng.below(5) {
            0 | 1 => {
                let groups = groups(&model);
                let group =
                    (!groups.is_empty() && rng.below(2) == 0).then(|| groups[rng.below(groups.len() as u64) as usize]);
                let (id, kind) = if rng.below(3) == 0 {
                    (add_group(&mut app, "g"), Kind::Group)
                } else {
                    (
                        add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false })),
                        Kind::Note,
                    )
                };
                app.edit_group(id, group);
                model.insert(id, (kind, group));
            }
            2 if !model.is_empty() => {
                let key = *model.keys().nth(rng.below(model.len() as u64) as usize).unwrap();
                let value = rng.below(1 << 24).to_string();
                if model[&key].0 == Kind::Group {
                    app.edit_name(key, &value);
                } else {
                    app.edit_text(key, &value);
                }
            }
            3 if !model.is_empty() => {
                let key = *model.keys().nth(rng.below(model.len() as u64) as usize).unwrap();
                // Destroying a group destroys what it holds, as the app does.
                let doomed: Vec<NodeId> =
                    model.keys().filter(|&&n| n == key || is_inside(&model, n, key)).copied().collect();
                app.destroy_node(key);
                for n in doomed {
                    model.remove(&n);
                }
            }
            4 if !model.is_empty() => {
                let key = *model.keys().nth(rng.below(model.len() as u64) as usize).unwrap();
                // A move into `key` itself or one of its descendants would create a cycle.
                let candidates: Vec<Option<NodeId>> = std::iter::once(None)
                    .chain(groups(&model).into_iter().map(Some))
                    .filter(|g| g.is_none_or(|g| g != key && !is_inside(&model, g, key)))
                    .collect();
                let choice = candidates[rng.below(candidates.len() as u64) as usize];
                app.edit_group(key, choice);
                model.get_mut(&key).unwrap().1 = choice;
            }
            _ => {}
        }
        assert!(app.save_session());
        let doc = app.doc.journal.as_ref().unwrap().doc();
        assert_eq!(shown(doc), snapshots(&app), "the document shows the live graph");
        let live: BTreeMap<NodeId, (Kind, Option<NodeId>)> =
            doc.live().map(|(id, n)| (id, (n.kind(), doc.effective_group(id)))).collect();
        assert_eq!(live, model, "and the model");
        let ops = ops_of(&app);
        assert!(app.save_session());
        assert_eq!(ops_of(&app), ops, "a second save writes nothing");
    }
}

/// A crash respawn, an engine swap and a content reload take the node out of `nodes` and put it back: none
/// queues a delete, so a save in between keeps the document node.
#[test]
fn a_crash_respawn_a_swap_and_a_reload_delete_nothing() {
    let dir = temp_dir("respawn-swap-reload-keep");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    let crashed = add(&mut app, Kind::Tab, sleeping_tab("https://c/", ""));
    let (swapped, _peer) = live_tab(&mut app, ViewKind::Servo, "https://s/");
    let json = app.alloc_node_id();
    let body = crate::redirect_fetch::save(&dir, json, crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    let source = crate::ContentSource::Web { url: "https://x.test/j".into(), body };
    app.insert_content(source, json, egui::Vec2::ZERO, None).unwrap();
    assert!(app.save_session());
    start_respawn(&mut app, crashed);
    app.finish_swap(swapped, ViewKind::Webview2, "https://s/".into());
    assert!(app.spawns.spawning.contains_key(&swapped) && !app.nodes.contains_key(&swapped), "swapping");
    let moved = dir.join("moved.json");
    std::fs::write(&moved, b"[2]").unwrap();
    let path = moved.to_string_lossy().into_owned();
    app.reload_fetched_node(json, crate::WebBody::File { kind: crate::content::ContentKind::Json, path });
    assert!(!app.doc.writes.has_deletes(), "nothing queued");
    assert!(app.save_session());
    for id in [&crashed, &swapped, &json] {
        assert!(live_in_doc(&app, *id), "{id} kept");
    }
}

/// A download is created once it completes: an in-progress one can't resume, so it is not written, but its queued
/// create waits for the save after completion.
#[test]
fn a_download_is_created_once_it_completes() {
    let mut app = app_on(&temp_dir("download-create-on-complete").join("session.mnf"), 1);
    let payload = DownloadPayload::started("f.bin".into(), Some(2));
    let dl = add(&mut app, Kind::Download, Runtime::Download(payload));
    assert!(app.save_session());
    assert!(!app.doc.journal.as_ref().unwrap().doc().nodes.contains_key(&dl), "not yet");
    assert_eq!(app.doc.writes.life(dl), Some(Life::Create), "still queued");
    app.nodes.get_mut(&dl).unwrap().runtime = Runtime::Download(DownloadPayload::finished("f.bin".into(), 2));
    assert!(app.save_session());
    assert!(live_in_doc(&app, dl), "created");
}

/// In a test build every save checks that the live graph and the document agree, so a node that leaves
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
    app.doc.mirror.set(id, field::TYPE, FieldValue::Str(Kind::Note.type_str().unwrap().to_owned()));
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

/// A new node's `Create` carries where it was placed, not where a settle moved it before the first save.
#[test]
fn a_node_is_created_at_its_placement_not_its_settled_position() {
    let path = temp_dir("pos-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note_at(&mut app, egui::vec2(10.0, 20.0));
    let other = a_note_at(&mut app, egui::vec2(11.0, 20.0));
    assert_eq!(app.layout_pos(note), egui::vec2(10.0, 20.0), "the layout starts at the placement too");
    app.settle_current_level();
    assert_ne!(app.layout_pos(note), egui::vec2(10.0, 20.0));
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(Some((10.0, 20.0))));
    assert_eq!(stored(&app, other, field::POS), FieldValue::Pos(Some((11.0, 20.0))));
}

#[test]
fn a_read_only_store_saves_nothing() {
    let path = temp_dir("sync-readonly").join("session.mnf");
    drop(Journal::open(&path, ActorId(1)).unwrap());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[4] = u8::try_from(crate::store::journal::FORMAT_VERSION + 1).unwrap(); // a newer format version
    let crc = crate::store::crc32c::crc32c(&bytes[..21]);
    bytes[21..25].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let mut app = app_on(&path, 2);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    assert!(!app.save_session());
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().live().count(), 0);
}

/// A launch that changes nothing (no user action at all) must rewrite
/// neither the session nor its personal state -- otherwise every launch
/// would touch both files even when nothing happened. `insert_note_at`
/// (main.rs) is private to that module and also saves as a side effect,
/// so this uses the `add` helper and sets the settled position directly,
/// like the other no-window tests.
#[test]
fn an_idle_open_and_close_writes_nothing() {
    let dir = crate::store::testutil::temp_dir("idle-app");
    let path = dir.join("session.mnf");
    {
        let mut app = app_on(&path, 1);
        let id = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: String::new(), editing: false }));
        app.snap_layout(id, egui::vec2(3.0, 4.0));
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
    app.insert_placeholder_node(id, egui::Vec2::ZERO, Kind::Note, Runtime::Note(payload));
    app.doc.mirror.set(id, field::URL, FieldValue::Str("https://x.test/a.txt".into()));
    let shared = snapshots(&app);
    let fields = &shared[&id].fields;
    assert_eq!(fields.get(&field::URL), Some(&FieldValue::Str("https://x.test/a.txt".into())));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.source_url(id), Some("https://x.test/a.txt"));
}

/// A finished download keeps the address its engine fetched it from across a restart.
#[test]
fn a_completed_download_keeps_its_url() {
    let path = crate::store::testutil::temp_dir("download-url").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = app.alloc_node_id();
    let payload = DownloadPayload::finished("f.bin".into(), 0);
    app.insert_placeholder_node(id, egui::Vec2::ZERO, Kind::Download, Runtime::Download(payload));
    app.doc.mirror.set(id, field::URL, FieldValue::Str("https://x.test/f.bin".into()));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.copyable_url(id), Some("https://x.test/f.bin"));
    assert_eq!(restored.source_url(id), None, "a download has no web source to refetch");
}

/// A download saved before cards kept their URL (or by an engine that can't say) restores with
/// none, so its card falls back to showing the file's path.
#[test]
fn a_download_without_a_url_restores_without_one() {
    let path = crate::store::testutil::temp_dir("download-no-url").join("session.mnf");
    let mut app = app_on(&path, 1);
    let id = app.alloc_node_id();
    let payload = DownloadPayload::finished("f.bin".into(), 0);
    app.insert_placeholder_node(id, egui::Vec2::ZERO, Kind::Download, Runtime::Download(payload));
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert!(restored.nodes[&id].as_download().is_some());
    assert_eq!(restored.copyable_url(id), None);
}

/// A download's "opened from" link (the tab it came from) and a content node's (the tab it was) survive a
/// restart, like a tab's.
#[test]
fn an_opened_from_link_survives_a_restart_on_every_kind_that_has_one() {
    let path = temp_dir("parent-kinds").join("session.mnf");
    let mut app = app_on(&path, 1);
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
    let download = DownloadPayload::finished("f.bin".into(), 0);
    let download = add(&mut app, Kind::Download, Runtime::Download(download));
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "hi".into(), editing: false }));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/child", "Child"));
    for id in [&download, &note, &tab] {
        app.edit_parent(*id, Some(opener));
    }
    assert!(app.save_session());
    assert_eq!(stored(&app, download, field::PARENT), FieldValue::Id(Some(opener)));
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    for id in [&download, &note, &tab] {
        assert_eq!(restored.doc.mirror.parent(*id), Some(opener), "{id}");
    }
    // Another replica relinking a download applies here too.
    let mut other = Journal::open(&path, ActorId(3)).unwrap();
    remote_set(&mut other, download, field::PARENT, FieldValue::Id(Some(tab)));
    assert!(restored.save_session());
    assert_eq!(restored.doc.mirror.parent(download), Some(tab));
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
    app.insert_image_node_with_id(image, file(&png_path), egui::Vec2::ZERO, None);
    app.insert_pdf_node_with_id(pdf, file(&dir.join("a.pdf")), egui::Vec2::ZERO, None);
    app.insert_json_node_with_id(json, file(&json_path), egui::Vec2::ZERO, None).unwrap();
    for id in [&image, &pdf, &json] {
        app.edit_parent(*id, Some(opener));
    }
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.png", "From image"));
    app.edit_parent(tab, Some(image));
    assert!(app.save_session());
    app.close_store();

    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    for id in [&image, &pdf, &json] {
        assert_eq!(restored.doc.mirror.parent(*id), Some(opener), "{id}");
    }
    assert!(restored.nodes[&image].as_image().is_some() && restored.nodes[&json].as_json().is_some());
    assert!(restored.nodes[&pdf].as_pdf().is_some());
    assert_eq!(restored.doc.mirror.parent(tab), Some(image), "a tab opened from an image card");
}

/// Saving a sleeping tab writes what its live tab would: a restore
/// followed by a save changes nothing in the store.
#[test]
fn a_sleeping_tab_saves_as_its_tab() {
    let path = temp_dir("lazy-save").join("session.mnf");
    let tab = {
        let mut app = app_on(&path, 1);
        let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
        app.edit_field(tab, field::PINNED, FieldValue::Bool(true));
        assert!(app.save_session());
        app.close_store();
        tab
    };
    let mut app = app_on(&path, 2);
    assert!(app.restore_session());
    let view_kind = stored(&app, tab, field::VIEW_KIND);
    // The single tab is the focused one, so the restore woke it; fail
    // that wake too: a waking and a failed card save the same way.
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::URL), FieldValue::Str("https://s/".into()));
    assert_eq!(stored(&app, tab, field::PINNED), FieldValue::Bool(true));
    assert_eq!(stored(&app, tab, field::VIEW_KIND), view_kind);
}

#[test]
fn a_jar_def_is_saved_as_a_jar_node_and_survives_later_saves() {
    let dir = crate::store::testutil::temp_dir("jar-def-saved");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = add_jar(&mut app, "Work");
    app.save_session();
    let doc = app.doc.journal.as_ref().unwrap().doc();
    let node = doc.nodes.get(&jar).filter(|n| n.is_live()).expect("jar node created");
    assert_eq!(node.kind(), Kind::Jar);
    assert_eq!(fields_of(node).get(&field::NAME), Some(&FieldValue::Str("Work".into())));
    app.save_session();
    assert!(app.doc.journal.as_ref().unwrap().doc().nodes[&jar].is_live(), "a second save must not delete it");
    assert!(app.nodes.contains_key(&jar), "a jar is a card");
}

/// `queue_create` checks the document eagerly: an id it already has an entry for (live, or a tombstone) is not
/// queued, so a restore, a remote node and a respawn never fill the queue (the commit's own filter stays the guard).
/// Queueing a create withdraws a pending delete of the same id: the node is shown again.
#[test]
fn queue_create_skips_an_id_the_document_already_has() {
    let mut app = app_on(&temp_dir("queue-create-eager").join("session.mnf"), 1);
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    let id: NodeId = note;
    assert_eq!(app.doc.writes.creates().collect::<BTreeSet<_>>(), BTreeSet::from([id]), "a new node is queued");
    assert!(app.save_session());
    app.queue_create(note);
    assert!(!app.doc.writes.has_creates(), "live in the document: not queued again");
    app.queue_delete(note);
    app.queue_create(note);
    assert!(!app.doc.writes.has_deletes(), "shown again: the delete is withdrawn");
    app.remove_node(note);
    assert!(app.save_session());
    app.queue_create(note);
    assert!(!app.doc.writes.has_creates(), "a tombstone: never recreated");
}

/// A jar's name, colour and ephemeral flag are mirror fields. Creating, renaming and toggling a jar
/// write them through `edit_field` and the save stores them; the mirror keeps the jar's entry across saves
/// (`Mirror::retain`), and a remote change reaches the mirror and `jar_defs`.
#[test]
fn a_jar_create_rename_and_ephemeral_save_and_a_remote_change_reaches_jar_defs() {
    let path = temp_dir("jar-mirror-sync").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    let jar = add_jar(&mut app, "Work");
    let key = jar;
    let work = FieldValue::Str("Work".into());
    assert_eq!(app.doc.mirror.get(key, field::NAME), work, "created in the mirror");
    assert!(app.save_session());
    assert_eq!(stored(&app, key, field::NAME), work);
    assert_eq!(stored(&app, key, field::COLOR), color_value(app.jar_defs()[&jar].color));
    assert_eq!(app.doc.mirror.get(key, field::NAME), work, "kept by the save's retain");
    app.commit_rename(jar, "Job");
    assert!(app.doc.writes.field_queued(jar, field::NAME), "a rename queues its field");
    app.set_jar_ephemeral(jar, true);
    assert!(app.doc.writes.field_queued(jar, field::EPHEMERAL), "so does a toggle");
    assert!(app.save_session());
    assert_eq!(stored(&app, key, field::NAME), FieldValue::Str("Job".into()));
    assert_eq!(stored(&app, key, field::EPHEMERAL), FieldValue::Bool(true));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, key, field::NAME, FieldValue::Str("House".into()));
    remote_set(&mut other, key, field::COLOR, red());
    assert!(app.save_session());
    assert_eq!(app.jar_defs()[&jar].name, "House");
    assert_eq!(app.jar_defs()[&jar].color, Some(egui::Color32::RED));
    assert_eq!(app.doc.mirror.get(key, field::NAME), FieldValue::Str("House".into()));
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
    let tabs: Vec<_> = live.into_iter().filter(|(k, _)| *k == Kind::Tab).collect();
    assert_eq!(tabs, [(Kind::Tab, FieldValue::Str("https://new/".into()))]);
}

/// The mirror is what this replica knows of the store. After each kind of save,
/// nothing pending, every live node's snapshot (`App::snapshot`: the mirror's fields and group) must be exactly what
/// the store holds for it, whether the commit wrote it, another replica did, or a remote delete moved it.
#[test]
fn the_mirror_matches_the_store_after_every_save() {
    fn check(app: &App, step: &str) {
        let doc = app.doc.journal.as_ref().unwrap().doc();
        for (id, node) in doc.live() {
            let want = Snapshot::of(doc, id, node);
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
    app.edit_group(inner, Some(outer));
    app.edit_group(tab, Some(inner));
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
    remote_set(&mut other, tab, field::PINNED, FieldValue::Bool(true));
    other.begin().unwrap().commit(vec![OpBody::Delete { node: inner }]).unwrap();
    assert!(app.save_session());
    check(&app, "remote set and delete");

    app.remove_node(outer);
    assert!(app.save_session());
    check(&app, "a local delete and moves");

    if let Runtime::Unloaded(t) = &mut app.nodes.get_mut(&tab).unwrap().runtime {
        t.url = "https://b/".into();
    }
    app.edit_url(tab, "https://b/");
    assert!(app.save_session());
    check(&app, "a shared url");
}

#[test]
fn a_name_set_before_the_first_save_rides_the_create() {
    let path = temp_dir("edit-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    app.edit_field(note, field::NAME, one());
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::NAME), one());
    assert!(!app.doc.writes.has_fields());
}

/// A saved node edited, then deleted, before the same save: the delete stands.
#[test]
fn an_edit_to_a_node_the_store_dropped_is_not_written_back() {
    let path = temp_dir("edit-dropped").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.edit_field(note, field::NAME, one());
    app.remove_node(note);
    assert!(app.save_session());
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert!(!doc.nodes[&note].is_live(), "the delete stands; no placeholder or recreate");
}

/// The save that deletes a node writes only the `Delete`, not the node's pending edit after it.
#[test]
fn a_save_that_deletes_a_node_writes_no_set_for_it() {
    let path = temp_dir("edit-delete-ops").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    let ops_before = app.doc.journal.as_ref().unwrap().doc().version[&ActorId(1)];
    app.edit_field(note, field::NAME, one());
    app.remove_node(note);
    assert!(app.save_session());
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().version[&ActorId(1)], ops_before + 1, "one op: the Delete");
    assert_eq!(stored(&app, note, field::NAME), FieldValue::Str(String::new()));
}

/// A node never saved, edited and removed before the first save: no `Set` reaches the store, so no placeholder.
#[test]
fn an_edit_to_an_unsaved_node_removed_before_the_save_leaves_no_placeholder() {
    let path = temp_dir("edit-unsaved-removed").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    app.edit_field(note, field::NAME, one());
    app.remove_node(note);
    assert!(app.save_session());
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert!(!doc.nodes.contains_key(&note), "no entry at all for the id");
}

/// A jar's fields are mirrored like every other kind's: `App::pending` writes them, and nothing
/// compares the mirror with the document, so a value written to the mirror directly is
/// never saved.
#[test]
fn a_jars_fields_reach_the_document_only_through_pending() {
    let mut app = app_on(&temp_dir("jar-fields-pending-only").join("session.mnf"), 1);
    let jar = add_jar(&mut app, "j");
    assert!(app.save_session());
    app.doc.mirror.set(jar, field::COLOR, red());
    app.doc.mirror.set(jar, field::EPHEMERAL, FieldValue::Bool(true));
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "nothing written");
    assert_eq!(stored(&app, jar, field::EPHEMERAL), FieldValue::Bool(false));
}

#[test]
fn an_unchanged_edit_queues_nothing() {
    let path = temp_dir("edit-noop").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    app.edit_field(note, field::NAME, FieldValue::Str(String::new()));
    assert!(!app.doc.writes.has_fields());
}

/// The same for a graph node: an existing node's mirrored fields reach the document only as `pending` `Set`s.
#[test]
fn an_existing_nodes_fields_reach_the_document_only_through_pending() {
    let mut app = app_on(&temp_dir("node-fields-pending-only").join("session.mnf"), 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.doc.mirror.set(note, field::NAME, one());
    app.doc.mirror.set(note, field::URL, FieldValue::Str("https://elsewhere/".into()));
    let ops = ops_of(&app);
    assert!(app.save_session());
    assert_eq!(ops_of(&app), ops, "nothing written");
    assert_eq!(stored(&app, note, field::NAME), FieldValue::Str(String::new()));
}

#[test]
fn a_failed_commit_keeps_a_pending_edit_for_the_retry() {
    let path = temp_dir("edit-fail").join("session.mnf");
    let mut app = app_on(&path, 1);
    let note = a_note(&mut app);
    assert!(app.save_session());
    app.edit_field(note, field::NAME, one());
    let set_readonly = |readonly: bool| {
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        #[allow(
            clippy::permissions_set_readonly_false,
            reason = "the Windows read-only attribute makes the append fail; on Unix this would make the file writable by everyone, but it's a temp file this test owns"
        )]
        perms.set_readonly(readonly);
        std::fs::set_permissions(&path, perms).unwrap();
    };
    set_readonly(true);
    let saved = app.save_session();
    set_readonly(false);
    assert!(!saved);
    assert!(app.doc.writes.has_fields(), "kept for the next save");
    assert_eq!(stored(&app, note, field::NAME), FieldValue::Str(String::new()));
    app.doc.writes.save_retry_at = Some(std::time::Instant::now());
    app.poll_store();
    assert!(app.doc.writes.save_retry_at.is_none());
    assert!(!app.doc.writes.has_fields());
    assert_eq!(stored(&app, note, field::NAME), one());
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
    app.edit_parent(b, Some(a));
    app.remove_tab(a); // queues its Delete
    assert!(app.save_session());
    assert_eq!(stored(&app, b, field::PARENT), FieldValue::Id(None));
}

/// A parent target created by the same commit is kept.
#[test]
fn a_parent_created_by_the_same_commit_is_kept() {
    let path = temp_dir("parent-new").join("session.mnf");
    let mut app = app_on(&path, 1);
    let b = add(&mut app, Kind::Tab, sleeping_tab("https://b/", ""));
    assert!(app.save_session());
    let a = add(&mut app, Kind::Tab, sleeping_tab("https://a/", ""));
    app.edit_parent(b, Some(a));
    assert!(app.save_session());
    assert_eq!(stored(&app, b, field::PARENT), FieldValue::Id(Some(a)));
}

/// A group created in the same commit is the target of a move that follows its Create.
#[test]
fn a_move_into_a_group_created_by_the_same_save_follows_the_create() {
    let path = temp_dir("group-new").join("session.mnf");
    let mut app = app_on(&path, 1);
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    let g = add_group(&mut app, "G");
    app.edit_group(t, Some(g));
    assert!(app.save_session());
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().effective_group(t), Some(g));
    assert_eq!(app.doc.mirror.group(t), Some(g), "the mirror shows what the document holds");
}

/// A move into a group the same commit deletes lands at the top level (spec section 6 item 4).
#[test]
fn a_move_into_a_group_deleted_by_the_same_save_is_written_as_none() {
    let path = temp_dir("group-gone").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let t = add(&mut app, Kind::Tab, sleeping_tab("https://t/", ""));
    assert!(app.save_session());
    app.edit_group(t, Some(g));
    app.remove_node(g);
    assert!(app.save_session());
    assert_eq!(app.doc.journal.as_ref().unwrap().doc().effective_group(t), None);
    assert_eq!(app.doc.mirror.group(t), None, "the mirror shows what the commit wrote");
}

/// A pending `jar` is resolved at commit like `parent`: a jar this commit deletes is written as none, and
/// the mirror shows what was written.
#[test]
fn a_pending_jar_naming_a_jar_deleted_in_this_commit_is_written_as_none() {
    let path = temp_dir("jar-pending-deleted").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = add_jar(&mut app, "Work");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    app.set_jar_edge(tab, Some(work));
    // Gone here with the tab's field still naming it: this commit deletes its node.
    app.remove_remotely_deleted(work);
    app.queue_delete(work);
    assert!(app.save_session());
    assert!(!app.doc.journal.as_ref().unwrap().doc().nodes[&work].is_live(), "the jar's delete is in this commit");
    assert_eq!(stored(&app, tab, field::JAR), FieldValue::Id(None));
    assert_eq!(app.doc.mirror.jar(tab), None, "the mirror shows what was written");
}

/// `Create`: a new tab naming a jar the same commit deletes is created with none; one naming a jar the same
/// commit creates keeps it.
#[test]
fn a_created_tab_naming_a_jar_deleted_in_this_commit_is_created_with_none() {
    let path = temp_dir("jar-create-deleted").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = add_jar(&mut app, "Work");
    assert!(app.save_session());
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    app.set_jar_edge(tab, Some(work));
    app.remove_remotely_deleted(work); // the pending field still names it
    app.queue_delete(work);
    let home = add_jar(&mut app, "Home");
    let other = add(&mut app, Kind::Tab, sleeping_tab("https://o/", ""));
    app.set_jar_edge(other, Some(home));
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::JAR), FieldValue::Id(None), "created with none");
    assert_eq!(app.doc.mirror.jar(tab), None, "the mirror shows what was written");
    assert_eq!(stored(&app, other, field::JAR), FieldValue::Id(Some(home)), "a jar created here is kept");
}

/// A tab naming a jar another replica deleted keeps naming it through a save. Its chip still says the jar is missing.
#[test]
fn a_dangling_remote_deleted_jar_survives_a_save() {
    let path = temp_dir("jar-dangling").join("session.mnf");
    let mut app = app_on(&path, 1);
    app.ensure_default_jar();
    let work = add_jar(&mut app, "Work");
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    app.set_jar_edge(tab, Some(work));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: work }]).unwrap();
    assert!(app.save_session());
    assert!(!app.jar_defs().contains_key(&work), "the remote delete arrived");
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::JAR), FieldValue::Id(Some(work)), "not cleared by a save");
    assert_eq!(app.doc.mirror.jar(tab), Some(work));
    assert!(app.jar_chip(tab).is_some_and(|c| c.missing), "the chip says the jar is missing");
}

/// An engine code this build doesn't know (a newer build's) reads as Servo, and is not overwritten by a
/// save or by waking the tab on the engine it reads as.
#[test]
fn an_unknown_view_code_survives_a_save() {
    let path = temp_dir("engine-unknown").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::VIEW_KIND, FieldValue::ViewCode(99));
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.view_kind(tab), ViewKind::Servo);
    app.edit_field(tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), FieldValue::ViewCode(99), "not overwritten by a save");
    app.focus_tab(tab);
    assert_eq!(app.test_spawns[0].0.kind, ViewKind::Servo, "wakes on the engine it reads as");
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), FieldValue::ViewCode(99), "nor by a wake");
}

/// An unknown engine code survives a wake that lands live too: the live tab's save reads the
/// mirror's raw value, not the engine it runs.
#[test]
fn an_unknown_view_code_survives_a_tab_landing_live() {
    let path = temp_dir("engine-unknown-live").join("session.mnf");
    let mut app = app_on(&path, 1);
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://s/", ""));
    assert!(app.save_session());
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    remote_set(&mut other, tab, field::VIEW_KIND, FieldValue::ViewCode(99));
    assert!(app.save_session());
    app.focus_tab(tab);
    let _peers = land_spawns(&mut app);
    assert_eq!(app.nodes[&tab].as_tab().expect("landed live").tab.kind, ViewKind::Servo);
    app.edit_field(tab, field::PINNED, FieldValue::Bool(true));
    assert!(app.save_session());
    assert_eq!(stored(&app, tab, field::VIEW_KIND), FieldValue::ViewCode(99), "not overwritten by a live tab's save");
}

/// A jar deleted while a new tab spawned with it is in flight, before the tab's `Create` commits. The Create writes
/// none (it resolves its Id fields); the tab lands naming the dead jar, which resolves to the Default.
#[test]
fn a_new_tab_whose_jar_is_deleted_before_its_create_lands_on_the_default() {
    let path = temp_dir("jar-deleted-mid-spawn-1").join("session.mnf");
    let mut app = app_on(&path, 1);
    let default = default_jar_id(&mut app);
    let work = add_jar(&mut app, "Work");
    assert!(app.save_session());
    let t = spawn_with_jar(&mut app, SpawnPurpose::New, work);
    app.remove_node(work);
    assert!(app.save_session());
    assert_eq!(stored(&app, t, field::JAR), FieldValue::Id(None), "created with none");
    let _peers = land_spawns(&mut app);
    assert_eq!(app.effective_jar(t), Some((default, true)), "lands on the Default");
}

/// The tab's `Create` committed with the jar before it was deleted. The delete could not reach the tab (no mirror
/// entry yet), so the edge dangles: the document keeps it and the tab resolves to the Default.
#[test]
fn a_new_tab_whose_jar_is_deleted_after_its_create_dangles_to_the_default() {
    let path = temp_dir("jar-deleted-mid-spawn-2").join("session.mnf");
    let mut app = app_on(&path, 1);
    let default = default_jar_id(&mut app);
    let work = add_jar(&mut app, "Work");
    assert!(app.save_session());
    let t = spawn_with_jar(&mut app, SpawnPurpose::Restore, work);
    assert!(app.save_session());
    assert_eq!(stored(&app, t, field::JAR), FieldValue::Id(Some(work)), "created with the jar");
    app.close_node(work);
    assert!(app.save_session());
    fail_spawns(&mut app);
    assert_eq!(app.effective_jar(t), Some((default, true)), "the failed card lands on the Default");
    assert!(app.save_session());
    assert_eq!(stored(&app, t, field::JAR), FieldValue::Id(Some(work)), "dangling, never rewritten");
}

/// Every way a node is made seeds its type in the mirror (`Mirror::ty`): placed here, landed from a spawn,
/// or materialized from the document.
#[test]
fn every_creation_path_seeds_the_node_type() {
    let ty = |kind: Kind| kind.type_str().unwrap();
    let mut app = App::default();
    let json = Runtime::Json(JsonPayload::file("d.json".into(), Default::default(), None));
    let placed = [
        (add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false })), Kind::Note),
        (add_group(&mut app, "G"), Kind::Group),
        (add(&mut app, Kind::Tab, sleeping_tab("https://a.test/", "A")), Kind::Tab),
        (add(&mut app, Kind::Json, json), Kind::Json),
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
        assert_eq!(app.doc.mirror.ty(*id), ty(*kind), "{kind:?}");
    }

    // A tab that lands from a spawn (`seed_landing`).
    let t = app.alloc_node_id();
    let spawn = crate::tabs::TabSpawn::new(
        t,
        "https://t.test/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    app.spawn_tab_async(spawn);
    fail_spawns(&mut app);
    assert_eq!(app.doc.mirror.ty(t), ty(Kind::Tab), "a failed landing");
    let live = app.alloc_node_id();
    let spawn = crate::tabs::TabSpawn::new(
        live,
        "https://l.test/".into(),
        ViewKind::Servo,
        egui::Vec2::ZERO,
        SpawnPurpose::Restore,
        1.0,
    );
    app.spawn_tab_async(spawn);
    let _peers = land_spawns(&mut app);
    assert!(app.nodes[&live].as_tab().is_some(), "landed live");
    assert_eq!(app.doc.mirror.ty(live), ty(Kind::Tab), "a live landing");

    // Materialized from the document.
    let nodes = BTreeMap::from([
        (node(1), shared(Kind::Group, None, &[(field::NAME, FieldValue::Str("G".into()))])),
        (node(2), shared(Kind::Note, None, &[(field::TEXT, FieldValue::Str("n".into()))])),
        (node(3), shared(Kind::Tab, None, &[(field::URL, FieldValue::Str("https://a.test/".into()))])),
    ]);
    let app = app_with(&nodes);
    for (id, node) in &nodes {
        assert_eq!(app.doc.mirror.ty(*id), ty(node.kind()), "materialized {:?}", node.kind());
    }
}

/// A node's `Create` resolves its Id fields (`create_ops`'s `valid`): a link to a node deleted meanwhile is written as
/// none, and the mirror shows what the document holds, as after an edit.
#[test]
fn a_created_group_shows_the_jar_its_create_resolved() {
    let path = temp_dir("group-jar-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let work = add_jar(&mut app, "Work");
    assert!(app.save_session());
    let g = add_group(&mut app, "G");
    app.set_jar_edge(g, Some(work));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: work }]).unwrap();
    assert!(app.save_session());
    assert_eq!(stored(&app, g, field::JAR), FieldValue::Id(None), "created without the deleted jar");
    assert_eq!(app.doc.mirror.jar(g), None, "the mirror shows what the document holds");
    assert!(app.save_session());
    assert_eq!(stored(&app, g, field::JAR), FieldValue::Id(None));
}

/// The same for a created node's `parent`: an opener deleted elsewhere before the first save.
#[test]
fn a_created_note_shows_the_parent_its_create_resolved() {
    let path = temp_dir("parent-create").join("session.mnf");
    let mut app = app_on(&path, 1);
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
    assert!(app.save_session());
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    app.edit_parent(note, Some(opener));
    let mut other = Journal::open(&path, ActorId(2)).unwrap();
    other.begin().unwrap().commit(vec![OpBody::Delete { node: opener }]).unwrap();
    assert!(app.save_session());
    assert_eq!(stored(&app, note, field::PARENT), FieldValue::Id(None));
    assert_eq!(app.doc.mirror.parent(note), None, "the mirror shows what the document holds");
}

/// A link is a stored node: it survives save and restore with both ends.
#[test]
fn a_link_survives_save_and_restore() {
    let path = temp_dir("link-roundtrip").join("session.mnf");
    let mut app = app_on(&path, 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let link = app.create_link(a, b);
    assert!(app.save_session());
    app.close_store();
    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    assert_eq!(restored.doc.mirror.link_ends(link), (Some(a), Some(b)));
    assert_eq!(restored.links_naming(a), vec![link]);
}

/// A link keeps its ends in the mirror after a save (which retains only the graph's nodes), so a later delete of an
/// endpoint still finds it.
#[test]
fn a_saved_link_keeps_its_ends_and_is_found_by_a_later_delete() {
    let mut app = app_on(&temp_dir("link-retain").join("session.mnf"), 1);
    let a = succeed_spawn(&mut app, "https://a/");
    let b = succeed_spawn(&mut app, "https://b/");
    let link = app.create_link(a, b);
    assert!(app.save_session());
    assert_eq!(app.doc.mirror.link_ends(link), (Some(a), Some(b)));
    app.remove_tab(a);
    assert!(app.links_naming(b).is_empty());
    assert!(app.save_session());
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
                app.insert_image_node_with_id(id, file(&png_path), egui::Vec2::ZERO, None);
                id
            }
            Kind::Pdf => {
                let id = app.alloc_node_id();
                app.insert_pdf_node_with_id(id, file(&dir.join("a.pdf")), egui::Vec2::ZERO, None);
                id
            }
            Kind::Json => {
                let id = app.alloc_node_id();
                app.insert_json_node_with_id(id, file(&json_path), egui::Vec2::ZERO, None).unwrap();
                id
            }
            Kind::Download => {
                let dl = DownloadPayload::finished("f.bin".into(), 0);
                add(&mut app, kind, Runtime::Download(dl))
            }
            _ => panic!("{ty} ({kind:?}) is an opened-from source: add it to this test"),
        };
        assert_eq!(app.connect(OPENED_FROM, id, opener), Ok(()), "{ty}");
        sources.push((ty, id));
    }
    assert!(sources.len() >= 6, "tab, note, image, PDF, JSON and download at least: {sources:?}");
    assert!(app.save_session());
    app.close_store();

    let mut restored = app_on(&path, 2);
    assert!(restored.restore_session());
    for (ty, id) in &sources {
        assert_eq!(restored.doc.mirror.parent(*id), Some(opener), "{ty} after a restart");
    }
    let mut other = Journal::open(&path, ActorId(3)).unwrap();
    for (_, id) in &sources {
        remote_set(&mut other, *id, field::COLOR, FieldValue::Color(Some(0x00ff_0000)));
    }
    assert!(restored.save_session());
    for (ty, id) in &sources {
        assert_eq!(restored.doc.mirror.parent(*id), Some(opener), "{ty} after another replica's edit");
    }
}

#[test]
fn a_jar_is_a_card_that_round_trips_with_its_position_and_group() {
    let path = temp_dir("jar-card").join("session.mnf");
    let mut app = app_on(&path, 1);
    let group = add_group(&mut app, "G");
    let at = crate::fetched::Placement { group: Some(group), pos: egui::vec2(120.0, -40.0) };
    let jar = app.create_jar("Work", at);
    assert!(app.save_session());
    // A tab too, so the restore is not a fresh start.
    add(&mut app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    assert!(app.save_session());
    let mut back = app_on(&path, 2);
    assert!(back.restore_session());
    assert!(matches!(back.nodes[&jar].runtime, Runtime::Jar), "a jar is a live card");
    assert_eq!(back.doc.mirror.group(jar), Some(group));
    assert_eq!(back.doc.mirror.pos(jar), Some(egui::vec2(120.0, -40.0)));
    assert_eq!(back.jar_defs()[&jar].name, "Work");
}

#[test]
fn a_stored_card_without_a_position_lands_jittered_around_the_world_center() {
    use crate::graph_layout::{spawn_jitter, GRAPH_WORLD_CENTER};
    // What `fields_of` hands over for a node whose POS was never written: the absent value.
    let unplaced = fields_of(&Node::typed(Kind::Jar));
    assert_eq!(unplaced.get(&field::POS), Some(&FieldValue::Pos(None)));
    let (a, b) = (pos_of(node(1), &unplaced), pos_of(node(2), &unplaced));
    assert_eq!(a, GRAPH_WORLD_CENTER + spawn_jitter(node(1)));
    assert_ne!(a, b, "two old jars do not stack");
    let placed = Fields::from([(field::POS, FieldValue::Pos(Some((120.0, -40.0))))]);
    assert_eq!(pos_of(node(1), &placed), egui::vec2(120.0, -40.0), "a stored pos is used as is");
}

#[test]
fn moving_an_unknown_card_writes_one_set_and_keeps_its_raw_fields() {
    let (mut app, node, _path) = restored_unknown();
    let key = node;
    assert!(app.save_session());
    let raw =
        app.doc.journal.as_ref().unwrap().doc().nodes[&node].fields[&crate::store::model::RegKey::of(40)].enc.clone();
    let before = ops_of(&app);
    app.edit_pos(key, Some(egui::vec2(50.0, 60.0)));
    assert!(app.save_session());
    assert_eq!(ops_of(&app), before + 1, "one Set");
    assert_eq!(stored(&app, key, field::POS), FieldValue::Pos(Some((50.0, 60.0))));
    let doc = app.doc.journal.as_ref().unwrap().doc();
    assert_eq!(
        doc.nodes[&node].fields[&crate::store::model::RegKey::of(40)].enc,
        raw,
        "a newer build's field is untouched"
    );
    assert_eq!(doc.nodes[&node].get(field::URL), FieldValue::Str("https://x/".into()), "so is an off-list one");
}

/// The per-node records keep the rules between a node's queued writes in one place.
#[test]
fn queued_writes_follow_one_record_per_node() {
    let (a, b) = (NodeId { actor: ActorId(1), counter: 1 }, NodeId { actor: ActorId(1), counter: 2 });
    let mut w = PendingWrites::default();
    w.queue_create(a, false);
    w.queue_delete(a);
    assert_eq!((w.life(a), w.has_creates()), (Some(Life::Delete), false), "a delete replaces a create");
    w.queue_create(a, false);
    assert_eq!(w.life(a), Some(Life::Create), "a create withdraws a delete");
    w.queue_create(a, true);
    assert!(w.life(a).is_none(), "one the document holds needs no create");
    w.queue_closed(b, 7);
    w.queue_closed(a, 0);
    assert_eq!(w.closed(), [(b, 7), (a, 0)], "closed flags commit in the order queued");
    assert_eq!(w.take_closed(), BTreeSet::from([a, b]));
    assert!(!w.has_closed() && w.nodes.is_empty());
}
