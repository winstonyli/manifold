use super::*;
use crate::session_sync::color_value;

#[test]
fn insert_content_dispatches_local_files_by_content() {
    let dir = crate::store::testutil::temp_dir("insert_content");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(3, 2).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let files = [
        ("pic.pdf", png.into_inner()), // lying extension: still an image
        ("doc", b"%PDF-1.4\n".to_vec()),
        ("data.txt", br#"{"a": 1}"#.to_vec()),
        ("bom.json", b"\xef\xbb\xbf[2]".to_vec()),
        ("notes.md", b"# hi".to_vec()),
        ("blob.bin", vec![0, 1, 2, 255]),
    ];
    let mut app = App::default();
    let mut kinds = Vec::new();
    for (name, bytes) in files {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let id = app.alloc_node_id();
        app.insert_content(ContentSource::Local(path), id.clone(), egui::Vec2::ZERO, None).unwrap();
        app.finish_image_decodes();
        kinds.push(match &app.nodes[&id].runtime {
            Runtime::Image(p) => format!("image {}x{}", p.width, p.height),
            Runtime::Pdf(_) => "pdf".into(),
            Runtime::Json(p) => format!("json {}", p.value),
            Runtime::Note(p) => format!("note {}", p.text),
            Runtime::Download(p) => format!("download {} done={}", p.bytes_received, p.completed),
            _ => "other".into(),
        });
        assert_eq!(app.source_url(&id), None);
    }
    assert_eq!(kinds, ["image 3x2", "pdf", r#"json {"a":1}"#, "json [2]", "note # hi", "download 4 done=true"]);
}

/// A node whose mirror entry exists without a type (an op named it before its `Create` arrived) is typed when
/// placed, so its card never reads as unknown.
#[test]
fn place_node_types_a_node_whose_entry_lacks_a_type() {
    let mut app = App::default();
    app.doc.mirror.set("n", field::URL, FieldValue::Str("https://a/".into()));
    assert_eq!(app.doc.mirror.ty("n"), "");
    let note = Runtime::Note(NotePayload { text: "t".into(), editing: false });
    app.insert_placeholder_node("n".into(), egui::Vec2::ZERO, Kind::Note, note);
    assert_eq!(app.doc.mirror.ty("n"), crate::store::model::TYPE_NOTE);
    assert_eq!(app.card_kind("n").unwrap().ty, crate::store::model::TYPE_NOTE);
    assert_eq!(app.doc.mirror.url("n"), "https://a/", "the entry's other fields are kept");
}

#[test]
fn insert_content_refuses_a_failing_local_file() {
    let dir = crate::store::testutil::temp_dir("insert_content_refuse");
    let big = dir.join("big.txt");
    std::fs::write(&big, vec![b'a'; crate::content::NOTE_MAX_BYTES + 1]).unwrap();
    let mut app = App::default();
    for path in [big, dir.join("missing.txt")] {
        let id = app.alloc_node_id();
        assert!(app.insert_content(ContentSource::Local(path), id.clone(), egui::Vec2::ZERO, None).is_err());
        assert!(!app.nodes.contains_key(&id));
    }
    assert_eq!(
        app.toasts.save_toast_queue.len() + usize::from(app.toasts.save_toast.is_some()),
        2,
        "each refusal toasts"
    );
}

/// A file with an image's signature but a corrupt body gets a card at once; the worker then turns it into an error
/// card with a toast.
#[test]
fn a_corrupt_image_becomes_an_error_card_and_a_toast() {
    let dir = crate::store::testutil::temp_dir("insert_content_corrupt_image");
    let broken = dir.join("broken.png");
    std::fs::write(&broken, b"\x89PNG\r\n\x1a\n not really").unwrap();
    let mut app = App::default();
    let id = app.alloc_node_id();
    app.insert_content(ContentSource::Local(broken), id.clone(), egui::Vec2::ZERO, None).unwrap();
    assert!(app.nodes[&id].runtime.as_image().unwrap().decoding(), "placeholder first");
    app.finish_image_decodes();
    let image = app.nodes[&id].runtime.as_image().unwrap();
    assert!(image.error.is_some() && !image.decoding());
    assert_eq!(app.toasts.save_toast_queue.len() + usize::from(app.toasts.save_toast.is_some()), 1);
}

#[test]
fn several_drops_in_a_frame_fan_out() {
    let dir = crate::store::testutil::temp_dir("drop_fan");
    let mut app = App::default();
    let mut ids = Vec::new();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), b"x").unwrap();
        let before: HashSet<String> = app.nodes.keys().cloned().collect();
        app.drop_file(dir.join(name), egui::pos2(300.0, 300.0));
        ids.extend(app.nodes.keys().filter(|k| !before.contains(*k)).cloned());
    }
    let (a, b) = (app.layout_pos(&ids[0]), app.layout_pos(&ids[1]));
    assert_eq!(b - a, egui::vec2(24.0, 24.0));
}

#[test]
fn drop_file_lands_beside_the_focused_tab_in_its_own_group() {
    let dir = crate::store::testutil::temp_dir("drop_beside_focused");
    let mut app = App::default();
    // A sleeping (unloaded) tab is a real Tab-kind node with no live
    // engine process -- the same stand-in `session_sync::tests::
    // sleeping_tab` already uses elsewhere for "the focused tab" in a
    // unit test. `drop_file`'s "beside focused" branch only reads
    // `pos`/`group`, so this exercises exactly the same code a live
    // tab would. `App::default()` is already in Tab view
    // (`showing_tab_view()`, see `several_drops_in_a_frame_fan_out`'s
    // own comment).
    use crate::session_sync::tests::{add, sleeping_tab};
    let focused = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Focused"));
    app.snap_layout(&focused, egui::vec2(100.0, 50.0));
    app.edit_group(&focused, Some("g".into()));
    app.focused = Some(focused.clone());
    // Deliberately not "g": proves the drop uses the focused tab's own
    // group, not `current_group`, the way `insert_content` directly
    // (not `add_file_at`) is needed for.
    app.current_group = None;

    let mut ids = Vec::new();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), b"x").unwrap();
        let before: HashSet<String> = app.nodes.keys().cloned().collect();
        app.drop_file(dir.join(name), egui::pos2(300.0, 300.0));
        ids.extend(app.nodes.keys().filter(|k| !before.contains(*k)).cloned());
    }

    let beside = egui::vec2(100.0, 50.0) + egui::vec2(240.0, 0.0);
    assert_eq!(app.layout_pos(&ids[0]), beside);
    assert_eq!(app.layout_pos(&ids[1]), beside + egui::vec2(24.0, 24.0));
    assert_eq!(app.doc.mirror.group(&ids[0]), Some("g"));
    assert_eq!(app.doc.mirror.group(&ids[1]), Some("g"));
}

#[test]
fn insert_content_dispatches_web_content_and_keeps_its_url() {
    let dir = crate::store::testutil::temp_dir("insert_content_web");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(1, 1).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let path = dir.join("fetched.png");
    std::fs::write(&path, png.into_inner()).unwrap();

    let mut app = App::default();
    let id = app.alloc_node_id();
    let source = ContentSource::Web {
        url: "https://x.test/a.png".into(),
        body: WebBody::File { kind: content::ContentKind::Image, path: path.to_string_lossy().into_owned() },
    };
    app.insert_content(source, id.clone(), egui::Vec2::ZERO, None).unwrap();
    assert_eq!(app.source_url(&id), Some("https://x.test/a.png"));

    let note_id = app.alloc_node_id();
    let source = ContentSource::Web { url: "https://x.test/note.txt".into(), body: WebBody::Text("hi".into()) };
    app.insert_content(source, note_id.clone(), egui::Vec2::ZERO, None).unwrap();
    assert_eq!(app.source_url(&note_id), Some("https://x.test/note.txt"));
    // 2b-5 Task 1: the url is the mirror's, seeded at creation, so the next save creates the node with it.
    assert_eq!(
        (app.doc.mirror.url(&id), app.doc.mirror.url(&note_id)),
        ("https://x.test/a.png", "https://x.test/note.txt")
    );
    let snapshot = app.snapshot(id.parse().unwrap()).unwrap();
    assert_eq!(snapshot.fields[&field::URL], FieldValue::Str("https://x.test/a.png".into()));
}

/// 2b-5 Task 1: a download's address is the mirror's from the moment its card is placed, and its type is seeded.
#[test]
fn a_new_download_has_its_url_and_type_in_the_mirror() {
    use crate::session_sync::tests::{add, sleeping_tab};
    let mut app = App::default();
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "T"));
    app.insert_download_node(tab, "f.bin".into(), Some(3), Some("https://x.test/f.bin".into()));
    let id = app.nodes.iter().find(|(_, n)| n.as_download().is_some()).map(|(id, _)| id.clone()).unwrap();
    assert_eq!(app.doc.mirror.url(&id), "https://x.test/f.bin");
    assert_eq!(app.copyable_url(&id), Some("https://x.test/f.bin"));
    assert_eq!(app.doc.mirror.ty(&id), crate::store::model::Kind::Download.type_str().unwrap());
    assert_eq!(app.doc.mirror.path(&id), "f.bin");
}

/// 2b-5 Ruling 5: a reload that lands in a new file keeps the stored (create-time) path in the mirror, which the
/// save writes, while the payload reads the new one.
#[test]
fn a_reload_keeps_the_stored_path_and_duplicating_exports_the_resolved_one() {
    let dir = crate::store::testutil::temp_dir("reload_path");
    let mut app = App::default();
    let id = app.alloc_node_id();
    let first = crate::redirect_fetch::save(&dir, &id, crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    let WebBody::File { path: stored, .. } = &first else { panic!("a file") };
    let stored = stored.clone();
    app.insert_content(
        ContentSource::Web { url: "https://x.test/j".into(), body: first },
        id.clone(),
        egui::Vec2::ZERO,
        None,
    )
    .unwrap();
    assert_eq!(app.doc.mirror.path(&id), stored);
    let moved = dir.join("moved.json");
    std::fs::write(&moved, b"[2]").unwrap();
    let moved = moved.to_string_lossy().into_owned();
    app.reload_fetched_node(&id, WebBody::File { kind: crate::content::ContentKind::Json, path: moved.clone() });
    assert_eq!(app.nodes[&id].as_json().unwrap().path, moved, "the payload reads the new file");
    assert_eq!(app.doc.mirror.path(&id), stored, "the mirror keeps the stored path");
    let snapshot = app.snapshot(id.parse().unwrap()).unwrap();
    assert_eq!(snapshot.fields[&field::PATH], FieldValue::Str(stored.clone()), "a save writes the stored path");
    let doc = crate::duplicate::subtree_json(&app, &[id.as_str()], egui::Vec2::ZERO).unwrap();
    assert_eq!(doc.nodes[0].fields.get("path").and_then(|v| v.as_str()), Some(moved.as_str()));
}

#[test]
fn a_content_file_resolves_to_this_machines_newest_copy_first() {
    use crate::content::tests::write_aged;
    let dir = crate::store::testutil::temp_dir("resolve_content");
    write_aged(&dir.join("5-9.png"), std::time::Duration::from_secs(60));
    let elsewhere = r"Z:\nowhere\5-9.png";
    let index = crate::ContentIndex::scan(Some(&dir));
    assert_eq!(index.resolve("5-9", elsewhere), dir.join("5-9.png").to_string_lossy());
    assert_eq!(index.resolve("5-8", elsewhere), elsewhere);
    assert_eq!(crate::ContentIndex::scan(None).resolve("5-9", elsewhere), elsewhere);
    // A stored file that exists (a re-id'd node's old copy) still loses
    // to the node's own fetch, and a Refresh's newer extension wins.
    let old = dir.join("5-1.png");
    write_aged(&old, std::time::Duration::from_secs(60));
    let old = old.to_string_lossy().into_owned();
    write_aged(&dir.join("5-9.jpg"), std::time::Duration::ZERO);
    let index = crate::ContentIndex::scan(Some(&dir));
    assert_eq!(index.resolve("5-9", &old), dir.join("5-9.jpg").to_string_lossy());
    assert_eq!(index.resolve("5-1", &old), old);
    // An unfinished save is never a node's file.
    write_aged(&dir.join("5-2.png.1234.tmp"), std::time::Duration::ZERO);
    assert_eq!(crate::ContentIndex::scan(Some(&dir)).resolve("5-2", elsewhere), elsewhere);
}

#[test]
fn one_index_answers_every_node_of_a_pass() {
    use crate::content::tests::write_aged;
    let dir = crate::store::testutil::temp_dir("content_index");
    write_aged(&dir.join("5-9.png"), std::time::Duration::from_secs(60));
    write_aged(&dir.join("5-9.jpg"), std::time::Duration::ZERO);
    write_aged(&dir.join("5-10.json"), std::time::Duration::ZERO);
    let index = crate::ContentIndex::scan(Some(&dir));
    assert_eq!(index.resolve("5-9", "x"), dir.join("5-9.jpg").to_string_lossy(), "the newest wins");
    assert_eq!(index.resolve("5-10", "x"), dir.join("5-10.json").to_string_lossy());
    assert_eq!(index.resolve("5-1", "x"), "x", "an id that is a prefix of another's isn't matched");
}

#[test]
fn pruning_keeps_files_live_or_recently_closed_nodes_name() {
    use crate::session_sync::tests::{add, app_on};
    let dir = crate::store::testutil::temp_dir("prune_content");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    let image = |path: std::path::PathBuf| {
        let path = path.to_string_lossy().into_owned();
        Runtime::Image(ImagePayload { path, width: 1, height: 1, error: None })
    };
    // A live node by path, a live node by id (a refetched copy), a closed node by its path, and an orphan.
    add(&mut app, Kind::Image, image(dir.join("a.png")));
    let by_id = add(&mut app, Kind::Image, image(r"Z:\x.png".into()));
    let closed = add(&mut app, Kind::Image, image(dir.join("c.png")));
    app.close_node(&closed);
    for name in ["a.png", &format!("{by_id}.png"), "c.png", "orphan.png"] {
        crate::content::tests::write_aged(&dir.join(name), crate::content::tests::OLD);
    }
    let keep = app.backing_keep(None);
    let cutoff = std::time::SystemTime::now() - crate::content::PRUNE_GRACE;
    let pruned = crate::content::prune(&dir, cutoff, |f| crate::content::kept(&keep, f));
    assert_eq!(pruned, 1);
    assert!(!dir.join("orphan.png").exists());
    assert!(dir.join("a.png").exists());
    assert!(dir.join(format!("{by_id}.png")).exists());
    assert!(dir.join("c.png").exists(), "the closed node is live in the document");
}

#[test]
fn pruning_keeps_files_a_named_session_names() {
    use crate::content::tests::{write_aged, OLD};
    use crate::session_sync::tests::{add, app_on};
    let root = crate::store::testutil::temp_dir("prune_named");
    let (content_dir, sessions) = (root.join("content"), root.join("sessions"));
    std::fs::create_dir_all(&content_dir).unwrap();
    std::fs::create_dir_all(&sessions).unwrap();
    let mut app = app_on(&root.join("session.mnf"), 1);
    let image = |path: &std::path::Path| {
        let path = path.to_string_lossy().into_owned();
        Runtime::Image(ImagePayload { path, width: 1, height: 1, error: None })
    };
    // A live node by path, one by id (its stored path elsewhere), and a
    // closed one by path.
    add(&mut app, Kind::Image, image(&content_dir.join("by-path.png")));
    let by_id = add(&mut app, Kind::Image, image(std::path::Path::new(r"Z:\elsewhere.png")));
    let closed = add(&mut app, Kind::Image, image(&content_dir.join("closed.png")));
    assert!(app.save_session());
    app.close_node(&closed);
    assert!(app.save_session());
    let journal = app.doc.journal.as_ref().unwrap();
    assert!(journal.doc().nodes[&closed.parse().unwrap()].closed(), "closed in the document, not recorded aside");
    journal.save_copy(&sessions.join("saved.mnf"), Some(&app.local_state().to_bytes())).unwrap();

    for name in ["by-path.png", &format!("{by_id}.json"), "closed.png", "orphan.png"] {
        write_aged(&content_dir.join(name), OLD);
    }
    // The running session names none of them.
    let cutoff = std::time::SystemTime::now() - crate::content::PRUNE_GRACE;
    let pruned = prune_unnamed(&content_dir, cutoff, HashSet::new(), Some(&sessions), |_| true);
    assert_eq!(pruned, Some(1));
    let named = session::backing_names(&sessions).unwrap();
    assert!(named.contains(&closed), "a closed node's id is named too");
    assert!(!content_dir.join("orphan.png").exists());
    assert!(content_dir.join("by-path.png").exists());
    assert!(content_dir.join(format!("{by_id}.json")).exists());
    assert!(content_dir.join("closed.png").exists());
}

#[test]
fn a_named_session_that_cant_be_read_whole_stops_the_prune() {
    use crate::content::tests::{write_aged, OLD};
    use crate::session_sync::tests::{add, app_on};
    let root = crate::store::testutil::temp_dir("prune_named_damaged");
    let (content_dir, sessions) = (root.join("content"), root.join("sessions"));
    std::fs::create_dir_all(&content_dir).unwrap();
    let orphan = content_dir.join("orphan.png");
    let prune = || {
        write_aged(&orphan, OLD);
        let cutoff = std::time::SystemTime::now() - crate::content::PRUNE_GRACE;
        prune_unnamed(&content_dir, cutoff, HashSet::new(), Some(&sessions), |_| true)
    };
    // No sessions dir yet names nothing.
    assert_eq!(prune(), Some(1));
    std::fs::create_dir_all(&sessions).unwrap();
    let saved = sessions.join("saved.mnf");
    let mut app = app_on(&root.join("session.mnf"), 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "x".into(), editing: false }));
    assert!(app.save_session());
    app.doc.journal.as_ref().unwrap().save_copy(&saved, None).unwrap();
    assert_eq!(prune(), Some(1));

    let good = std::fs::read(&saved).unwrap();
    let mut damaged = good.clone();
    *damaged.last_mut().unwrap() ^= 1;
    std::fs::write(&saved, damaged).unwrap();
    assert_eq!(prune(), None, "a save that loads only by repair");
    std::fs::write(&saved, b"garbage").unwrap();
    assert_eq!(prune(), None, "a save that isn't a journal");
    assert!(orphan.exists());
}

#[test]
fn a_repaired_store_is_never_pruned_against() {
    use crate::session_sync::tests::{add, app_on};
    let path = crate::store::testutil::temp_dir("prune_recovered").join("session.mnf");
    let mut app = app_on(&path, 1);
    add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "x".into(), editing: false }));
    assert!(app.save_session());
    assert!(app.may_prune());
    drop(app);
    // Damage the last byte: the repair skips that part and says so.
    let mut bytes = std::fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&path, bytes).unwrap();
    let mut app = app_on(&path, 2);
    app.restore_session();
    assert!(app.doc.store_recovered);
    assert!(!app.may_prune());
}

/// Review fix: a store that wasn't a journal is set aside and a new one started; that launch must not prune
/// the old session's content against the new, empty one.
#[test]
fn a_store_set_aside_is_never_pruned_against() {
    let path = crate::store::testutil::temp_dir("prune_aside").join("session.mnf");
    std::fs::write(&path, b"not a session").unwrap();
    let mut app = App::default();
    app.open_store_at(&path);
    assert!(app.doc.journal.is_some());
    assert!(app.doc.store_recovered);
    assert!(!app.may_prune());
}

#[test]
fn a_refresh_keeps_its_old_file_while_another_node_names_it() {
    let dir = crate::store::testutil::temp_dir("refresh_shared_file");
    let (old, new) = (dir.join("r.png"), dir.join("r.jpg"));
    let mut app = App::default();
    for id in ["r", "s"] {
        let path = old.to_string_lossy().into_owned();
        let payload = Runtime::Image(ImagePayload { path, width: 1, height: 1, error: None });
        app.insert_placeholder_node(id.into(), egui::Vec2::ZERO, Kind::Image, payload);
    }
    assert!(!replaced_file_unused(&dir, "r", &old, &new, &app.backing_keep(Some("r"))));
    app.nodes.remove("s");
    assert!(replaced_file_unused(&dir, "r", &old, &new, &app.backing_keep(Some("r"))));
    assert!(!replaced_file_unused(&dir, "r", &old, &old, &app.backing_keep(Some("r"))));
}

/// A Refresh whose fetch fails, with the file still there, only toasts.
#[test]
fn a_failed_refresh_fetch_toasts() {
    let dir = crate::store::testutil::temp_dir("refresh_fetch_failed_toast");
    let mut app = App::default();
    let body = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.insert_content(
        ContentSource::Web { url: "https://x.test/j".into(), body },
        "r".into(),
        egui::vec2(0.0, 0.0),
        None,
    )
    .unwrap();
    app.refresh_fetch_failed("r");
    assert_eq!(app.toasts.save_toast_queue.len(), 1);
    assert!(app.toasts.save_toast_queue[0].contains("Couldn't refresh"));
    // The node itself is untouched by a fetch-level failure.
    assert!(app.nodes.contains_key("r"));
}

#[test]
fn a_refetched_file_reloads_the_node_in_place() {
    let dir = crate::store::testutil::temp_dir("refresh_reload");
    let mut app = App::default();
    let first = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.insert_content(
        ContentSource::Web { url: "https://x.test/j".into(), body: first },
        "r".into(),
        egui::vec2(1.0, 2.0),
        None,
    )
    .unwrap();
    app.edit_field("r", field::COLOR, color_value(Some(egui::Color32::RED)));
    let order = app.node_order.clone();
    let second = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[2]".to_vec()).unwrap();
    app.reload_fetched_node("r", second);
    let node = &app.nodes["r"];
    assert_eq!(*node.as_json().unwrap().value, serde_json::json!([2]));
    assert_eq!(
        (app.layout_pos("r"), app.doc.mirror.pos("r"), app.doc.mirror.color("r"), app.source_url("r")),
        (egui::vec2(1.0, 2.0), Some(egui::vec2(1.0, 2.0)), Some(egui::Color32::RED), Some("https://x.test/j"))
    );
    assert_eq!(app.node_order, order);
}

/// A refetched file that fails to load leaves a placeholder of the same
/// kind in the node's place, with the error and the url.
#[test]
fn a_failed_refetch_keeps_a_placeholder_with_group_and_error() {
    let dir = crate::store::testutil::temp_dir("refresh_reload_fail");
    let mut app = App::default();
    let first = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.insert_content(
        ContentSource::Web { url: "https://x.test/j".into(), body: first },
        "r".into(),
        egui::vec2(1.0, 2.0),
        Some("g".into()),
    )
    .unwrap();
    app.edit_field("r", field::COLOR, color_value(Some(egui::Color32::RED)));
    let order = app.node_order.clone();
    let bad = WebBody::File {
        kind: crate::content::ContentKind::Json,
        path: dir.join("missing.json").to_string_lossy().into_owned(),
    };
    app.reload_fetched_node("r", bad);
    let node = &app.nodes["r"];
    assert!(node.as_json().unwrap().error.is_some());
    assert_eq!(
        (app.layout_pos("r"), app.doc.mirror.color("r"), app.doc.mirror.group("r"), app.source_url("r")),
        (egui::vec2(1.0, 2.0), Some(egui::Color32::RED), Some("g"), Some("https://x.test/j"))
    );
    assert_eq!(app.node_order, order);
}

/// The spawn is parked in `test_spawns`; its id is already marked.
#[test]
fn open_as_tab_marks_the_new_tab_to_skip_its_first_conversion() {
    let dir = crate::store::testutil::temp_dir("open_as_tab");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(1, 1).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let path = dir.join("a.png");
    std::fs::write(&path, png.into_inner()).unwrap();

    let mut app = App::default();
    let source = ContentSource::Web {
        url: "https://x.test/a.png".into(),
        body: WebBody::File { kind: content::ContentKind::Image, path: path.to_string_lossy().into_owned() },
    };
    app.insert_content(source, "n".into(), egui::Vec2::ZERO, None).unwrap();
    app.open_as_tab("n");
    assert_eq!(app.test_spawns.len(), 1);
    let tab_id = app.test_spawns[0].0.id.clone();
    assert_eq!(app.test_spawns[0].0.url, "https://x.test/a.png");
    assert_eq!(app.test_spawns[0].0.parent.as_deref(), Some("n"), "opened from the node");
    assert_eq!(app.spawns.skip_convert.get(&tab_id), Some(&false));
}

/// Closing a node another node was opened from links that one to the closed node's own parent, as closing a
/// tab does for its children.
#[test]
fn closing_a_content_node_moves_what_it_opened_up() {
    use crate::session_sync::tests::{add, sleeping_tab};
    let mut app = App::default();
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "n".into(), editing: false }));
    app.edit_parent(&note, Some(opener.clone()));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/from-note", "T"));
    app.edit_parent(&tab, Some(note.clone()));
    app.close_node(&note);
    assert_eq!(app.doc.mirror.parent(&tab), Some(opener.as_str()));
}

/// The engine message arms need a live engine, so the entry's
/// transitions are checked through `skip_convert_after` directly.
#[test]
fn skip_convert_survives_a_redirect_but_not_a_later_navigation() {
    // (url changed, loading) reports for one Open-as-Tab tab.
    let mut entry = Some(false);
    entry = skip_convert_after(entry, false, true);
    assert_eq!(entry, Some(false), "first load under way");
    entry = skip_convert_after(entry, true, true);
    assert_eq!(entry, Some(false), "an HTTP redirect keeps it");
    entry = skip_convert_after(entry, false, false);
    assert_eq!(entry, Some(true), "first load settled");
    entry = skip_convert_after(entry, false, true);
    assert_eq!(entry, Some(true), "F5 at the same url keeps it");
    entry = skip_convert_after(entry, true, true);
    assert_eq!(entry, None, "a navigation after it settled drops it");
    assert_eq!(skip_convert_after(None, true, false), None, "other tabs stay out");
}

/// Closing an Open-as-Tab tab drops its entry too.
#[test]
fn removing_a_tab_clears_its_skip_convert_flag() {
    use crate::session_sync::tests::{add, sleeping_tab};
    let mut app = App::default();
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.json", "A"));
    app.spawns.skip_convert.insert(tab.clone(), true);
    app.remove_tab(&tab);
    assert!(!app.spawns.skip_convert.contains_key(&tab));
}

/// A sleeping tab stands in for a live one: only its place is read.
#[test]
fn a_converted_tab_keeps_its_place_and_url() {
    use crate::session_sync::tests::{add, sleeping_tab};

    let dir = crate::store::testutil::temp_dir("redirect_convert");
    let mut app = App::default();
    let opener = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/", "Opener"));
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.json", "A"));
    app.snap_layout(&tab, egui::vec2(5.0, 6.0));
    app.edit_group(&tab, Some("g".into()));
    app.edit_parent(&tab, Some(opener.clone()));
    let child = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/child", "Child"));
    app.edit_parent(&child, Some(tab.clone()));

    let body = crate::redirect_fetch::save(&dir, "n", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.redirect_tab_to_content(&tab, "n".into(), "https://x.test/a.json".into(), body);

    assert!(!app.nodes.contains_key(&tab));
    assert!(app.nodes["n"].as_json().is_some());
    assert_eq!((app.layout_pos("n"), app.doc.mirror.group("n")), (egui::vec2(5.0, 6.0), Some("g")));
    assert_eq!(app.source_url("n"), Some("https://x.test/a.json"));
    assert_eq!(app.doc.mirror.parent("n"), Some(opener.as_str()), "still opened from where the tab was");
    assert!(app.children_of(&tab).next().is_none(), "children moved up");
}

/// A converted tab leaves no reference behind: focus, split, selection
/// and a pending respawn focus all clear, and the view drops to Graph.
#[test]
fn a_converted_tab_clears_every_stale_reference() {
    use crate::session_sync::tests::{add, sleeping_tab};

    let dir = crate::store::testutil::temp_dir("redirect_convert_stale");
    let mut app = App::default();
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.json", "A"));
    app.view_mode = ViewMode::Item;
    app.focused = Some(tab.clone());
    app.split_tab = Some(tab.clone());
    app.interaction.selected.insert(tab.clone());
    app.spawns.pending_respawn_focus = Some(tab.clone());

    let body = crate::redirect_fetch::save(&dir, "n", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.redirect_tab_to_content(&tab, "n".into(), "https://x.test/a.json".into(), body);

    assert!(!app.nodes.contains_key(&tab));
    assert_eq!(app.focused, None, "focused cleared");
    assert_eq!(app.split_tab, None, "split cleared");
    assert!(!app.interaction.selected.contains(&tab), "selected cleared");
    assert_eq!(app.spawns.pending_respawn_focus, None, "pending respawn focus cleared");
    assert_eq!(app.view_mode, ViewMode::Graph, "falls back to Graph like any other close of the focused tab");
}

/// A web page stays a tab: a conversion whose insert fails keeps it.
#[test]
fn a_failed_conversion_keeps_the_tab() {
    use crate::session_sync::tests::{add, sleeping_tab};
    let dir = crate::store::testutil::temp_dir("redirect_convert_fail");
    let mut app = App::default();
    let tab = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/a.png", "A"));
    let child = add(&mut app, Kind::Tab, sleeping_tab("https://x.test/child", "Child"));
    app.edit_parent(&child, Some(tab.clone()));
    app.focused = Some(tab.clone());
    let path = dir.join("n.json");
    std::fs::write(&path, b"{\"a\": ").unwrap();
    let body = WebBody::File { kind: content::ContentKind::Json, path: path.to_string_lossy().into_owned() };
    app.redirect_tab_to_content(&tab, "n".into(), "https://x.test/a.json".into(), body);
    assert!(app.nodes.contains_key(&tab), "the tab stays");
    assert!(!app.nodes.contains_key("n"));
    assert_eq!(app.focused.as_deref(), Some(tab.as_str()));
    assert_eq!(app.doc.mirror.parent(&child), Some(tab.as_str()));
    assert_eq!(app.toasts.save_toast_queue.len() + usize::from(app.toasts.save_toast.is_some()), 1, "toasted");
}

/// Retrying an error card decodes the file again: the card goes back to a placeholder, then comes out sized once the
/// file is good. An image still decoding can't be opened.
#[test]
fn retrying_an_image_error_card_decodes_again_and_a_decoding_image_does_not_open() {
    let dir = crate::store::testutil::temp_dir("image_retry");
    let path = dir.join("p.png");
    std::fs::write(&path, b"\x89PNG\r\n\x1a\n not really").unwrap();
    let mut app = App::default();
    let id = app.alloc_node_id();
    app.insert_content(ContentSource::Local(path.clone()), id.clone(), egui::Vec2::ZERO, None).unwrap();
    app.focus_item(&id);
    assert_eq!(app.viewed_image, None, "no Item view while decoding");
    app.finish_image_decodes();
    assert!(app.nodes[&id].runtime.as_image().unwrap().error.is_some());

    let mut png = Vec::new();
    image::DynamicImage::new_rgba8(3, 2)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(&path, png).unwrap();
    app.retry_missing_file(&id);
    assert!(app.nodes[&id].runtime.as_image().unwrap().decoding(), "back to a placeholder");
    app.finish_image_decodes();
    let image = app.nodes[&id].runtime.as_image().unwrap();
    assert_eq!((image.width, image.height, image.error.is_none()), (3, 2, true));
    app.focus_item(&id);
    assert_eq!(app.viewed_image.as_deref(), Some(id.as_str()));
}

#[test]
fn one_prune_keeps_held_jar_files_and_drops_unheld_old_ones() {
    use crate::content::tests::{write_aged, OLD};
    let jars = crate::store::testutil::temp_dir("backing-jars");
    let session = crate::store::testutil::temp_dir("backing-jars-session").join("session.mnf");
    let mut app = App { jars_dir_override: Some(jars.clone()), ..crate::session_sync::tests::app_on(&session, 1) };
    let live = crate::session_sync::tests::add_jar(&mut app, "Live");
    let closed = crate::session_sync::tests::add_jar(&mut app, "Closed");
    app.close_node(&closed.to_string());
    for name in [format!("{live}.json"), format!("{closed}.json"), "9-9.json".into(), "default.json".into()] {
        write_aged(&jars.join(name), OLD);
    }
    write_aged(&jars.join("9-8.json"), std::time::Duration::ZERO); // recent: inside the grace
    let cutoff = std::time::SystemTime::now() - crate::content::PRUNE_GRACE;
    let pruned = prune_unnamed(&jars, cutoff, app.backing_keep(None), None, |_| true);
    assert_eq!(pruned, Some(2), "9-9.json and the old default.json");
    for kept in [format!("{live}.json"), format!("{closed}.json"), "9-8.json".into()] {
        assert!(jars.join(&kept).exists(), "{kept}");
    }
}

#[test]
fn releasing_a_removed_jar_deletes_only_its_unheld_family_at_once() {
    let jars = crate::store::testutil::temp_dir("release-jars");
    let keep: HashSet<String> = ["1-1".to_string()].into();
    let files = ["1-1.json", "1-2.json", "1-2.json.unreadable-5", "1-3.json"];
    for name in files {
        std::fs::write(jars.join(name), b"x").unwrap(); // brand new: no grace applies to a release
    }
    // A release's cutoff: now, plus a second for a coarse filesystem clock (`release_backing`).
    let cutoff = std::time::SystemTime::now() + std::time::Duration::from_secs(1);
    let only = |file: &str| crate::content::stem(file) == "1-2";
    assert_eq!(prune_unnamed(&jars, cutoff, keep.clone(), None, only), Some(2));
    assert!(jars.join("1-1.json").exists() && jars.join("1-3.json").exists(), "other jars untouched");
    let only = |file: &str| crate::content::stem(file) == "1-1";
    assert_eq!(prune_unnamed(&jars, cutoff, keep, None, only), Some(0), "held");
}
