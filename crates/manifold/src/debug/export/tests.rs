use super::*;
use crate::card_id::testutil::node;

/// `EXPORTSTATE` names a card by its type: `type` (the mime), `loaded` (true for a live tab
/// only) and `json_card` (a JSON level's card, not a stored node); the old card enum's `kind` is gone.
/// `view_kinds` has every engine, each with its state label and what the disk holds (no data dir under test, so
/// the downloaded engines read as not installed with nothing pinned).
#[test]
fn export_state_lists_each_view_kinds_state_and_disk() {
    let state = App::default().export_state();
    let view_kinds = state["view_kinds"].as_object().unwrap();
    assert_eq!(view_kinds.len(), 4);
    assert_eq!(view_kinds["webview2"]["disk"], "host ready");
    assert_eq!(view_kinds["cef"]["disk"], "host missing, runtime missing");
    assert!(view_kinds["servo"]["state"].as_str().unwrap().starts_with("Unavailable"), "{view_kinds:?}");
}

/// `window` is the window's size and `popups` the visible Foreground layers' rects (`gate layout` reads both).
#[test]
fn export_state_gives_the_theme_colors() {
    let state = App::default().export_state();
    assert_eq!(state["theme"]["danger"], serde_json::json!(manifold_pages::palette::DARK.danger));
    assert_eq!(state["theme"]["accent"], serde_json::json!(manifold_pages::palette::DARK.accent));
}

#[test]
fn export_state_gives_the_window_and_each_foreground_popup() {
    let app = App::default();
    let mut output = app.egui_ctx.run_ui(egui::RawInput::default(), |ui| {
        egui::Area::new(egui::Id::new("probe_popup")).order(egui::Order::Foreground).fixed_pos([10.0, 20.0]).show(
            ui.ctx(),
            |ui| {
                ui.label("x");
            },
        );
    });
    output.textures_delta.clear();
    let state = app.export_state();
    assert_eq!(state["window"].as_array().map(Vec::len), Some(2));
    let popups = state["popups"].as_array().unwrap();
    let probe = popups.iter().find(|p| p["id"].as_str().is_some_and(|id| id.contains("probe_popup"))).unwrap();
    assert_eq!(probe["rect"][0], 10.0);
    assert_eq!(probe["rect"][1], 20.0);
}

/// `trail` gives each tab's trail and the chips' enablement (`gate trail` reads it).
#[test]
fn export_state_gives_each_trail() {
    let mut app = App::default();
    app.scoped.nav_history.insert(node(1), crate::trail::history::NavHistory::visited(&["a", "b"]));
    let state = app.export_state();
    let trail = &state["trail"][node(1).to_string()];
    assert_eq!(
        (trail["entries"].as_u64(), trail["depth"].as_u64(), trail["url"].as_str()),
        (Some(2), Some(1), Some("b"))
    );
    assert_eq!(
        (trail["back"].as_bool(), trail["forward"].as_bool(), trail["back_fork"].as_bool()),
        (Some(true), Some(false), Some(false))
    );
    assert_eq!(
        (trail["lin_at"].as_u64(), trail["lin_len"].as_u64(), trail["left_to"].is_null()),
        (Some(1), Some(2), true)
    );
}

#[test]
fn export_state_names_cards_by_type_loaded_and_json_card() {
    use crate::node::{NotePayload, TabPayload, UnloadedState, UnloadedTabPayload};
    use crate::process_manager::{ViewKind, ViewTab};
    use crate::store::model::{Kind, TYPE_GROUP, TYPE_JSON, TYPE_NOTE, TYPE_TAB};
    let (live_id, asleep_id, note_id, group_id, json_id) = (node(1), node(2), node(3), node(4), node(5));
    let dir = crate::store::testutil::temp_dir("export_state");
    let path = dir.join("a.json");
    std::fs::write(&path, r#"{"a": {"b": 1}}"#).unwrap();
    let mut app = App::default();
    let (tab, _peer) = ViewTab::fake(ViewKind::Servo);
    let live = Runtime::Tab(Box::new(TabPayload {
        tab,
        url: "https://a/".into(),
        title: String::new(),
        can_go_back: false,
        can_go_forward: false,
        loading: false,
        audio_playing: false,
        muted: false,
        zoom_factor: 1.0,
        swap_target: None,
    }));
    let asleep = Runtime::Unloaded(UnloadedTabPayload {
        url: "https://s/".into(),
        title: String::new(),
        muted: false,
        zoom_factor: 1.0,
        state: UnloadedState::Sleeping,
    });
    app.test_node(live_id, Kind::Tab, live);
    app.test_node(asleep_id, Kind::Tab, asleep);
    app.test_node(note_id, Kind::Note, Runtime::Note(NotePayload { text: "t".into(), editing: false }));
    app.test_node(group_id, Kind::Group, Runtime::None);
    app.insert_json_node_with_id(json_id, path.to_string_lossy().into_owned(), egui::Vec2::ZERO, None).unwrap();
    let cards = |app: &App| -> std::collections::BTreeMap<String, serde_json::Value> {
        let state = app.export_state();
        let cards = state["cards"].as_array().expect("cards is a list");
        cards.iter().map(|c| (c["id"].as_str().unwrap().to_owned(), c.clone())).collect()
    };

    let top = cards(&app);
    assert_eq!(top.len(), 5, "{top:?}");
    for (id, ty, loaded) in [
        (live_id, TYPE_TAB, true),
        (asleep_id, TYPE_TAB, false),
        (note_id, TYPE_NOTE, false),
        (group_id, TYPE_GROUP, false),
        (json_id, TYPE_JSON, false),
    ] {
        let card = &top[&id.to_string()];
        assert_eq!(card["type"], ty, "{id}: {card}");
        assert_eq!(card["loaded"], loaded, "{id}: {card}");
        assert_eq!(card["json_card"], false, "{id}: {card}");
        assert!(card.get("kind").is_none(), "{id}: {card}");
        for key in [
            "group",
            "parent",
            "pinned",
            "labels",
            "tag_color",
            "editing",
            "world",
            "screen",
            "rect",
            "jar",
            "effective_jar",
            "is_default",
            "stored_type",
        ] {
            assert!(card.get(key).is_some(), "{id} keeps {key}: {card}");
        }
    }

    // A JSON level's cards are not stored nodes: `json_card`, typed as the JSON they show.
    app.switch_group_level(Some(json_id));
    let level = cards(&app);
    let json_card = |pointer: &str| {
        use crate::card_id::{fnv64, CardId, JSON_SOURCE};
        CardId::virt(JSON_SOURCE, fnv64(&[&json_id.to_string(), "#", pointer])).to_string()
    };
    let mut want = [json_card(""), json_card("/a")];
    want.sort();
    assert_eq!(level.keys().cloned().collect::<Vec<_>>(), want);
    for card in level.values() {
        assert_eq!(card["json_card"], true, "{card}");
        assert_eq!(card["type"], TYPE_JSON, "{card}");
        assert_eq!(card["loaded"], false, "{card}");
        assert!(card.get("kind").is_none(), "{card}");
        for key in ["screen", "rect"] {
            assert!(card.get(key).is_some(), "{key}: {card}");
        }
    }
    app.switch_group_level(None);
    assert_eq!(cards(&app)[&live_id.to_string()]["unresponsive"], serde_json::json!(false));
    app.nodes.get(&live_id).and_then(|n| n.as_tab()).unwrap().tab.wedge();
    assert_eq!(cards(&app)[&live_id.to_string()]["unresponsive"], serde_json::json!(true), "its queue filled");
}

/// EXPORTSTATE's `edges`: each line at this level with its kind, Link node, ends and on-screen middle.
#[test]
fn export_state_lists_the_level_edges_with_their_link() {
    use crate::node::NotePayload;
    use crate::store::model::Kind;
    let mut app = App::default();
    let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
    for (id, x) in [(a, 0.0), (b, 400.0)] {
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(id, egui::vec2(x, 0.0), Kind::Note, note);
    }
    let link = app.create_link(a, b);
    let state = app.export_state();
    let edges = state["edges"].as_array().expect("an array");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["kind"], "link");
    assert_eq!(edges[0]["link"], link.to_string());
    assert_eq!(
        (edges[0]["from"].as_str(), edges[0]["to"].as_str()),
        (Some(a.to_string().as_str()), Some(b.to_string().as_str()))
    );
    // a's right side to b's left side, bowed: the curve's t = 0.5 point, not the straight segment's middle.
    let (ra, rb) = (app.card_rect(a).unwrap(), app.card_rect(b).unwrap());
    let (from, to) = (ra.right_center(), rb.left_center());
    let world = crate::links::gestures::bow_point(from, to, 0.5);
    assert!((world - from.lerp(to, 0.5)).length() > 1.0, "the bow moves the middle off the chord");
    let mid = app.graph_world_rect_to_screen(egui::Rect::from_min_max(world, world)).min;
    let screen: Vec<f64> = edges[0]["screen"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((screen[0] as f32 - mid.x).abs() < 0.01 && (screen[1] as f32 - mid.y).abs() < 0.01, "{screen:?}");
    for (key, t) in [("near_from", 0.2), ("near_to", 0.8)] {
        let world = crate::links::gestures::bow_point(from, to, t);
        let want = app.graph_world_rect_to_screen(egui::Rect::from_min_max(world, world)).min;
        let got: Vec<f64> = edges[0][key].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!((got[0] as f32 - want.x).abs() < 0.01 && (got[1] as f32 - want.y).abs() < 0.01, "{key}: {got:?}");
    }
    assert!(state["retarget"].is_null());
    app.start_connect_drag(a);
    app.link.retarget = Some(crate::links::gestures::LinkRetarget { link, fixed_is_from: true });
    assert_eq!(app.export_state()["retarget"], link.to_string());
}

/// EXPORTSTATE's `link_handles`: each card that would show connect handles, each side's on-screen rect.
#[test]
fn export_state_gives_each_linkable_card_its_handle_rects() {
    use crate::node::NotePayload;
    use crate::store::model::Kind;
    let mut app = App::default();
    let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
    for (id, x) in [(a, 0.0), (b, 400.0)] {
        let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
        app.insert_placeholder_node(id, egui::vec2(x, 0.0), Kind::Note, note);
    }
    let state = app.export_state();
    let handles = state["link_handles"][a.to_string()].as_object().expect("a has handles");
    assert_eq!(handles.keys().map(String::as_str).collect::<Vec<_>>(), ["top", "right", "bottom", "left"]);
    let r: Vec<f32> = handles["right"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
    let card = app.graph_world_rect_to_screen(app.card_rect(a).unwrap());
    assert!((egui::pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0) - card.right_center()).length() < 0.01);
    assert!((r[2] - r[0] - crate::links::gestures::HANDLE_SHORT_PX).abs() < 0.01, "16 points across the edge");
    assert!((r[3] - r[1] - crate::links::gestures::HANDLE_LONG_PX).abs() < 0.01, "28 points along it");
    assert!(state["connect_drag"].is_null() && state["dragging"].is_null());
    app.start_connect_drag(a);
    let state = app.export_state();
    assert_eq!(state["connect_drag"], a.to_string());
    assert!(state["link_handles"].get(b.to_string()).is_some(), "during a connect-drag every card has them");
    app.cancel_connect();
    app.interaction.dragging = Some(crate::card_id::CardId::from(b));
    assert_eq!(app.export_state()["dragging"], b.to_string());
}

/// EXPORTSTATE writes through `store::atomic::replace`, so a probe can read the file as soon as it exists:
/// a blocked target stays unwritten (a plain write would ignore the block).
#[test]
fn export_state_is_written_through_the_atomic_replace() {
    let path = crate::store::testutil::temp_dir("export-state-atomic").join("state.json");
    let app = App::default();
    crate::store::atomic::block_for_test(&path, true);
    app.write_export_state(&path);
    assert!(!path.exists(), "the write went around store::atomic::replace");
    crate::store::atomic::block_for_test(&path, false);
    app.write_export_state(&path);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), app.export_state().to_string());
}
