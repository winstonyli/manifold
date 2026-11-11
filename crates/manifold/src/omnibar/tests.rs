use super::*;
use crate::session_sync::tests::{add, add_group, add_note, app_on, live_tab, sleeping_tab};
use crate::store::model::Kind;

impl App {
    fn other_row_for_test(&self, id: NodeId) -> Row {
        Row::Other { id, label: self.accessible_name(id).unwrap(), glyph: "g", snippet: None }
    }
}

fn others(app: &App) -> Vec<(NodeId, String)> {
    AppCorpus { app, exclude: None }.others().into_iter().map(|o| (o.id, o.label)).collect()
}

fn other_ids(app: &App, host: Host) -> Vec<NodeId> {
    app.omni_rows(host)
        .into_iter()
        .filter_map(|r| if let Row::Other { id, .. } = r { Some(id) } else { None })
        .collect()
}

#[test]
fn every_card_kind_but_a_tab_or_a_jar_is_an_other_card_named_by_its_accessible_name() {
    let mut app = App::default();
    let note = add_note(&mut app, "buy milk\nmore");
    let group = add_group(&mut app, "Research");
    add(&mut app, Kind::Jar, Runtime::Jar);
    tab(&mut app, "https://a.test/", "Alpha");
    assert_eq!(others(&app), [(note, "Note: buy milk".to_owned()), (group, "Group: Research".to_owned())]);
    let corpus = AppCorpus { app: &app, exclude: Some(note) };
    assert_eq!(corpus.others().len(), 1, "the excluded card is left out");
}

#[test]
fn an_empty_note_is_labelled_note() {
    let mut app = App::default();
    let note = add_note(&mut app, "");
    assert_eq!(others(&app), [(note, "Note".to_owned())]);
    app.omni.text = "note".into();
    assert_eq!(other_ids(&app, Host::Graph), [note], "listed, harmlessly");
}

#[test]
fn a_renamed_card_is_found_by_its_stored_name() {
    let mut app = App::default();
    let note = add_note(&mut app, "buy milk");
    app.edit_name(note, "Groceries");
    assert_eq!(others(&app), [(note, "Note: Groceries".to_owned())]);
    app.omni.text = "grocer".into();
    assert_eq!(other_ids(&app, Host::Graph), [note]);
}

#[test]
fn enter_on_an_other_row_in_graph_switches_level_and_sets_reveal() {
    let mut app = app_on(&crate::store::testutil::temp_dir("omni-other").join("session.mnf"), 1);
    let group = add_group(&mut app, "G");
    let inner = add_note(&mut app, "kept");
    app.edit_group(inner, Some(group));
    app.view_mode = ViewMode::Graph;
    assert_eq!(app.current_group, None);
    let row = app.other_row_for_test(inner);
    app.omni_activate(row, Host::Graph);
    assert_eq!(app.current_group, Some(group));
    assert_eq!(app.camera.reveal, Some(inner));
    assert_eq!(app.view_mode, ViewMode::Graph);
}

/// The tab's loaded page has settled into its card's frame, so only being shown keeps it rendering.
fn settle(app: &mut App, id: NodeId) {
    let t0 = std::time::Instant::now();
    app.note_tab_frame(id, t0);
    app.sync_tab_visibility(false, t0 + std::time::Duration::from_secs(5));
}

/// Whether the frame's `sync_tab_visibility` stops the tab rendering.
fn hidden(app: &mut App, peer: &mut crate::session_sync::tests::Peer) -> bool {
    app.sync_tab_visibility(false, std::time::Instant::now());
    peer.sent().iter().any(|m| matches!(m, HostToView::SetVisible(false)))
}

#[test]
fn an_other_row_chosen_in_the_item_view_hides_the_tab_and_shows_the_graph() {
    let mut app = App::default();
    let (tab_id, mut peer) = live_tab(&mut app, crate::ViewKind::Servo, "https://a.test/");
    settle(&mut app, tab_id);
    let note = add_note(&mut app, "kept");
    app.focused = Some(tab_id);
    app.view_mode = ViewMode::Item;
    let row = app.other_row_for_test(note);
    app.omni_activate(row, Host::Item);
    assert_eq!(app.view_mode, ViewMode::Graph);
    assert_eq!(app.camera.reveal, Some(note));
    assert!(hidden(&mut app, &mut peer), "the tab stopped rendering");
}

/// The Settings page opened from a tab leaves that tab showing: leaving for the Graph must hide it too.
#[test]
fn an_other_row_chosen_in_settings_opened_from_a_tab_hides_the_tab() {
    let mut app = App::default();
    let (tab_id, mut peer) = live_tab(&mut app, crate::ViewKind::Servo, "https://a.test/");
    settle(&mut app, tab_id);
    let note = add_note(&mut app, "kept");
    app.focused = Some(tab_id);
    app.view_mode = ViewMode::Item;
    app.open_settings_page();
    let row = app.other_row_for_test(note);
    app.omni_activate(row, Host::Settings);
    assert_eq!(app.view_mode, ViewMode::Graph);
    assert!(hidden(&mut app, &mut peer), "the tab stopped rendering");
}

#[test]
fn an_other_row_chosen_in_the_image_viewer_returns_to_the_graph() {
    let mut app = App::default();
    let note = add_note(&mut app, "kept");
    app.viewed_image = Some(note);
    app.view_mode = ViewMode::Item;
    let row = app.other_row_for_test(note);
    app.omni_activate(row, Host::Item);
    assert_eq!((app.view_mode, app.viewed_image), (ViewMode::Graph, None));
    assert_eq!(app.camera.reveal, Some(note));
}

#[test]
fn activating_a_card_that_has_gone_does_nothing_and_closes_the_bar() {
    let mut app = App::default();
    let gone = app.alloc_node_id();
    app.view_mode = ViewMode::Settings;
    app.omni.text = "x".into();
    app.omni_activate(Row::Other { id: gone, label: "Note".into(), glyph: "g", snippet: None }, Host::Settings);
    assert_eq!(app.view_mode, ViewMode::Settings);
    assert_eq!(app.camera.reveal, None);
    assert!(app.omni.text.is_empty() && !app.omni.open, "the bar closed");
}

fn tab(app: &mut App, url: &str, title: &str) -> NodeId {
    add(app, Kind::Tab, sleeping_tab(url, title))
}

fn card_row(app: &App, id: NodeId) -> Row {
    Row::Card {
        id,
        label: app.listed_tab_label(id).unwrap(),
        url: app.listed_tab_url(id).unwrap(),
        jar: None,
        snippet: None,
    }
}

#[test]
fn the_dropdown_closes_after_focus_stays_away() {
    let mut bar = OmniBar::new(String::new());
    bar.open = true;
    bar.note_focus(false);
    assert!(bar.editing(), "one frame is the known blip");
    bar.note_focus(true);
    bar.note_focus(false);
    assert!(bar.editing(), "regaining focus starts the count again");
    bar.note_focus(false);
    assert!(!bar.editing(), "dismissed");
}

#[test]
fn enter_on_a_card_row_in_graph_jumps_to_the_card_and_sets_reveal() {
    let mut app = App::default();
    let near = tab(&mut app, "https://near.test/", "Near");
    let far = add(&mut app, Kind::Tab, sleeping_tab("https://far.test/", "Far"));
    app.view_mode = ViewMode::Graph;
    app.snap_layout(far, egui::vec2(5000.0, 0.0));
    app.camera.jump_to(egui::Rect::from_center_size(egui::pos2(100.0, 100.0), egui::vec2(900.0, 700.0)));
    app.omni_activate(card_row(&app, far), Host::Graph);
    assert_eq!(app.camera.reveal, Some(far));
    assert_eq!(app.view_mode, ViewMode::Graph, "a jump, not an open");
    app.apply_camera_requests(false);
    assert!(app.camera.is_moving(), "eased to the far card");
    let _ = near;
}

#[test]
fn enter_on_a_typed_row_in_graph_opens_a_new_tab() {
    let mut app = App::default();
    app.omni_activate(Row::Typed { text: "rust lang".into() }, Host::Graph);
    let (spawn, _) = app.test_spawns.last().expect("a tab was spawned");
    assert_eq!(spawn.url, app.settings.value.default_search_engine.search_url("rust lang"));
}

#[test]
fn enter_on_a_typed_row_in_item_navigates_the_current_tab() {
    let mut app = App::default();
    let (id, mut peer) = live_tab(&mut app, crate::ViewKind::Servo, "https://a.test/");
    app.focused = Some(id);
    app.view_mode = ViewMode::Item;
    let nodes = app.nodes.len();
    app.omni_activate(Row::Typed { text: "https://b.test/".into() }, Host::Item);
    assert_eq!(app.nodes.len(), nodes);
    assert!(app.test_spawns.is_empty());
    assert!(peer.sent().iter().any(|m| matches!(m, HostToView::Navigate { url, .. } if url == "https://b.test/")));
}

#[test]
fn enter_on_a_settings_row_sets_settings_focus_and_opens_settings() {
    let mut app = App::default();
    let setting = Row::Setting { id: "theme", title: "Theme", section: "General" };
    app.omni_activate(setting, Host::Graph);
    assert_eq!(app.view_mode, ViewMode::Settings);
    assert_eq!(app.settings.focus, Some("theme"));
}

#[test]
fn escape_clears_the_text_then_closes() {
    let mut app = App::default();
    app.open_omni_popup();
    app.omni_set_text(Host::Popup, "abc");
    assert!(app.omni_escape(Host::Popup));
    assert!(app.omni_popup.text.is_empty());
    assert!(app.menus.omni_popup.open.is_some(), "the first Escape only clears");
    assert!(app.omni_escape(Host::Popup));
    assert!(app.menus.omni_popup.open.is_none());
}

#[test]
fn a_cards_label_names_match_the_filter_and_a_hash_word_selects_by_label() {
    let mut app = App::default();
    let (a, b) = (tab(&mut app, "https://a.test/", "alpha"), tab(&mut app, "https://b.test/", "beta"));
    let urgent = app.create_label("Urgent", None).unwrap();
    app.set_label(&[a], urgent, true);
    assert!(app.node_matches("urgent", a) && !app.node_matches("urgent", b));
    assert!(app.node_matches("#urg", a) && !app.node_matches("#urg", b));
    assert!(!app.node_matches("#zzz", a));
    assert!(app.node_matches("#urg alp", a) && !app.node_matches("#urg bet", a), "a plain word still filters");
    assert!(app.node_matches("#", a) && !app.node_matches("#", b), "a bare # selects the labelled cards");
    let card_rows = |app: &App| app.omni_rows(Host::Graph).iter().filter(|r| matches!(r, Row::Card { .. })).count();
    app.omni.text = "#urg".into();
    assert_eq!(card_rows(&app), 1);
    app.omni.text = "#zzz".into();
    assert_eq!(card_rows(&app), 0);
    app.omni.text = "urg".into();
    assert_eq!(card_rows(&app), 1, "the label text matches without the hash");
    app.delete_label(urgent);
    assert!(!app.node_matches("urgent", a), "a deleted label no longer matches");
}

#[test]
fn dimming_matches_every_card_past_the_row_cap() {
    let mut app = App::default();
    let ids: Vec<NodeId> =
        (0..rows::ROW_CAP + 3).map(|i| tab(&mut app, &format!("https://a{i}.test/"), "alpha")).collect();
    let other = tab(&mut app, "https://z.test/", "zebra");
    assert!(ids.iter().all(|&id| app.node_matches("alpha", id)), "all of them, past the row cap");
    assert!(!app.node_matches("alpha", other));
    app.omni.text = "alpha".into();
    let shown = app.omni_rows(Host::Graph).into_iter().filter(|r| matches!(r, Row::Card { .. })).count();
    assert_eq!(shown, rows::ROW_CAP);
}

#[test]
fn the_item_bar_offers_other_tabs_not_the_one_shown() {
    let mut app = App::default();
    let here = tab(&mut app, "https://here.test/", "Here");
    let there = tab(&mut app, "https://there.test/", "Here too");
    app.focused = Some(here);
    app.omni.text = "here".into();
    let ids: Vec<NodeId> = app
        .omni_rows(Host::Item)
        .into_iter()
        .filter_map(|r| if let Row::Card { id, .. } = r { Some(id) } else { None })
        .collect();
    assert_eq!(ids, [there]);
}

/// A sleeping tab's row shows its title, or its url when untitled.
#[test]
fn a_sleeping_tabs_label_is_its_title_or_its_url() {
    let mut app = app_on(&crate::store::testutil::temp_dir("omni-label").join("session.mnf"), 1);
    let titled = tab(&mut app, "https://a/", "Alpha");
    let untitled = tab(&mut app, "https://b/", "");
    assert_eq!(app.listed_tab_label(titled).as_deref(), Some("Alpha"));
    assert_eq!(app.listed_tab_label(untitled).as_deref(), Some("https://b/"));
}

/// Tabs that read the same are told apart by what differs; a tab of its own label gets nothing.
#[test]
fn tabs_that_read_the_same_get_a_qualifier_and_others_none() {
    let mut app = app_on(&crate::store::testutil::temp_dir("omni-qualifiers").join("session.mnf"), 1);
    let one = tab(&mut app, "https://one.test/", "Home");
    let two = tab(&mut app, "https://two.test/", "Home");
    let other = tab(&mut app, "https://one.test/x", "Docs");
    let q = app.listed_tab_qualifiers();
    assert_eq!(q.get(&one).map(String::as_str), Some("one.test"));
    assert_eq!(q.get(&two).map(String::as_str), Some("two.test"));
    assert_eq!(q.get(&other), None);
}

/// A query narrowed after moving down confirms its first match, not a stale row.
#[test]
fn a_narrowed_query_confirms_its_first_match() {
    let mut app = app_on(&crate::store::testutil::temp_dir("omni-narrowed").join("session.mnf"), 1);
    let ids: Vec<NodeId> = (0..4).map(|i| tab(&mut app, &format!("https://{i}/"), &format!("Tab {i}"))).collect();
    app.open_omni_popup();
    for _ in 0..3 {
        app.omni_move(Host::Popup, 1);
    }
    assert_eq!(app.omni_popup.highlighted, Some(3));
    app.omni_set_text(Host::Popup, "tab 2");
    app.omni_confirm(Host::Popup);
    assert!(app.menus.omni_popup.open.is_none());
    assert_eq!(app.focused, Some(ids[2]), "jumped to the one match");
}

/// A dismissed dropdown (focus left the box, the text stayed) still confirms the first match, never a web search.
#[test]
fn enter_after_the_dropdown_was_dismissed_still_confirms_the_first_match() {
    let mut app = app_on(&crate::store::testutil::temp_dir("omni-dismissed").join("session.mnf"), 1);
    let ids: Vec<NodeId> = (0..2).map(|i| tab(&mut app, &format!("https://{i}/"), &format!("Rust {i}"))).collect();
    app.open_omni_popup();
    app.omni_set_text(Host::Popup, "rust 1");
    app.omni_popup.dismiss();
    assert_eq!(app.omni_popup.highlighted, None);
    app.omni_confirm(Host::Popup);
    let search = app.settings.value.default_search_engine.search_url("rust 1");
    assert!(app.test_spawns.iter().all(|(spawn, _)| spawn.url != search), "no web search");
    assert_eq!(app.focused, Some(ids[1]));
}

/// Nothing open matches: Enter searches for it (a new tab), as the Graph's typed row does.
#[test]
fn enter_with_no_matching_card_searches_for_the_text() {
    let mut app = App::default();
    app.open_omni_popup();
    app.omni_set_text(Host::Popup, "no such tab");
    app.omni_confirm(Host::Popup);
    assert!(app.menus.omni_popup.open.is_none());
    let (spawn, _) = app.test_spawns.last().expect("a search tab");
    assert_eq!(spawn.url, app.settings.value.default_search_engine.search_url("no such tab"));
}

/// A Ctrl+L or `ADDRESSBAR` made while the Graph bar still draws (the 220 ms open-card transition) waits for the Item
/// bar: the Graph bar took it before, and the Item bar came up unfocused, so the typed url and its Enter were lost.
#[test]
fn an_address_bar_focus_request_waits_out_the_graph_bar() {
    let mut app = App::default();
    let ctx = crate::theme::testutil::ctx(true);
    app.pending_focus = Some(FocusTarget::Omni(Host::Item));
    for host in [Host::Graph, Host::Item] {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.omni_ui(ui, host, 200.0);
        });
        output.textures_delta.clear();
        if host == Host::Graph {
            assert!(app.pending_focus.is_some(), "the Graph bar took the address bar's request");
            assert_eq!(ctx.memory(|m| m.focused()), None);
        }
    }
    assert_eq!(app.pending_focus, None, "the Item bar took it");
    assert!(ctx.memory(|m| m.focused()).is_some());
}
