use super::*;
use crate::card_kind::fixtures::{kinds, loaded_tab, plain, sleeping_tab};
use crate::session_sync::tests::add_jar;
use crate::store::model::{TYPE_IMAGE, TYPE_JSON, TYPE_PDF};

/// The Default jar's label.
fn built_in() -> JarLabel {
    JarLabel { name: "Default".into(), marker: Some(crate::jar_defs::JarMarker::BuiltIn) }
}

/// The Default jar's label, making the Default first.
fn default_label(app: &mut App) -> JarLabel {
    let id = crate::session_sync::tests::default_jar_id(app);
    app.jar_label(id)
}

// One row per kind, pinned to the exact menus each kind showed before
// the per-kind builders were merged (see the node-menu spec), and
// before the tab and group menus joined them.
/// "Move into <group>" offers only sibling groups, and joins the node to the chosen one.
#[test]
fn move_to_group_offers_sibling_groups_and_joins() {
    let mut app = App::default();
    let (g1, g2, note) = (app.alloc_node_id(), app.alloc_node_id(), app.alloc_node_id());
    app.insert_group_at(g1, None);
    app.insert_group_at(g2, None);
    app.insert_note_at(note, None);
    // A group nested in g1 is not a sibling of the top-level note.
    let inner = app.alloc_node_id();
    app.insert_group_at(inner, None);
    app.edit_group(inner, Some(g1));
    let ids = |app: &App, id: NodeId| -> Vec<NodeId> {
        app.node_menu_view(id).unwrap().move_targets.into_iter().map(|(gid, _)| gid).collect()
    };
    assert_eq!(ids(&app, note), vec![g1, g2]);
    assert_eq!(ids(&app, g1), vec![g2], "a group isn't offered itself");
    app.apply_node_action(&egui::Context::default(), note, NodeAction::MoveToGroup(g2));
    assert_eq!(app.doc.mirror.group(note), Some(g2));
    // On a group "Move into" nests it, unlike a drop of one group on another (which merges).
    let group = plain(TYPE_GROUP);
    assert_eq!(drop_action(group, DropTarget::Card(group), false), Some(DropEffect::Merge));
    app.apply_node_action(&egui::Context::default(), g1, NodeAction::MoveToGroup(g2));
    assert_eq!(app.doc.mirror.group(g1), Some(g2), "g1 nested in g2, not merged away");
    assert_eq!(app.doc.mirror.group(inner), Some(g1), "g1 keeps its own members");
}

fn view_of(kind: CardKind) -> NodeMenuView {
    NodeMenuView {
        kind,
        has_group: false,
        has_url: false,
        has_copy_url: false,
        pinned: false,
        unresponsive: false,
        other_default_view_kind: false,
        failed: false,
        view_missing: false,
        move_targets: Vec::new(),
        renaming: None,
        jars: None,
        edges: Vec::new(),
        ephemeral: false,
        closable: true,
    }
}

#[test]
fn node_menu_config_matches_each_kinds_menu() {
    fn menu_of(view: &NodeMenuView, headless: bool) -> (&'static str, f32, String) {
        let config = node_menu_config(view, headless);
        let items: Vec<&str> = config
            .items
            .iter()
            .map(|item| match item {
                NodeMenuItem::Button(label, _) => *label,
                NodeMenuItem::Page(label, _) => *label,
                NodeMenuItem::MoveTo(_) => "<move>",
                NodeMenuItem::Jars { .. } => "<jars>",
                NodeMenuItem::Edges(_) => "<links>",
                NodeMenuItem::Separator => "---",
            })
            .collect();
        (config.area_id, config.min_width, items.join(" | "))
    }
    fn menu(kind: CardKind, has_group: bool, pinned: bool, headless: bool) -> (&'static str, f32, String) {
        let view = NodeMenuView { has_group, pinned, ..view_of(kind) };
        menu_of(&view, headless)
    }
    for has_group in [false, true] {
        // Every kind can join a group (drag onto it), so every menu
        // offers "Remove from Group" when grouped.
        let g = if has_group { " | --- | Remove from Group" } else { "" };
        let menu = |kind, pinned| menu(kind, has_group, pinned, false);
        assert_eq!(
            menu(plain(TYPE_DOWNLOAD), false),
            (
                "download_context_menu",
                170.0,
                format!(
                    "Open Containing Folder | Copy Path | --- | Rename | Duplicate | Pin{g} | --- | Remove from List"
                )
            )
        );
        assert_eq!(
            menu(plain(TYPE_NOTE), false),
            ("note_context_menu", 150.0, format!("Copy Text | --- | Rename | Duplicate | Pin{g} | --- | Delete"))
        );
        assert_eq!(
            menu(plain(TYPE_IMAGE), false),
            (
                "image_context_menu",
                170.0,
                format!("Open Containing Folder | Copy Path | --- | Rename | Duplicate | Pin{g} | --- | Delete")
            )
        );
        assert_eq!(
            menu(plain(TYPE_PDF), false),
            (
                "pdf_context_menu",
                170.0,
                format!("Open Containing Folder | Copy Path | --- | Rename | Duplicate | Pin{g} | --- | Delete")
            )
        );
        assert_eq!(
            menu(plain(TYPE_JSON), false),
            (
                "json_context_menu",
                170.0,
                format!("Open Containing Folder | Copy Path | --- | Rename | Duplicate | Pin{g} | --- | Delete")
            )
        );
        assert_eq!(
            menu(loaded_tab(), false),
            (
                "tab_context_menu",
                150.0,
                format!("Fit Subtree | Sleep | Copy URL | --- | Rename | Duplicate | Pin{g} | --- | Close")
            )
        );
        assert_eq!(
            menu(loaded_tab(), true).2,
            format!("Fit Subtree | Sleep | Copy URL | --- | Rename | Duplicate | Unpin{g} | --- | Close")
        );
        assert_eq!(
            menu(plain(TYPE_GROUP), false),
            (
                "group_context_menu",
                150.0,
                format!(
                    "Sleep Tabs | Export Group... | --- | Rename | Duplicate | Pin{g} | --- | Dissolve Group | \
                         Close Group and Contents"
                )
            )
        );
        assert_eq!(
            menu(sleeping_tab(), false),
            (
                "unloaded_tab_context_menu",
                170.0,
                format!("Open | Copy URL | --- | Rename | Duplicate | Pin{g} | --- | Close")
            )
        );
    }
    // A failed tab offers Retry (and the default engine, when it isn't
    // the engine that failed) instead of Open; a sleeping/waking one
    // just offers Open. Neither failed one offers Duplicate, which would fail the same way.
    let failed = NodeMenuView { other_default_view_kind: true, failed: true, ..view_of(sleeping_tab()) };
    assert_eq!(
        menu_of(&failed, false).2,
        "Retry | Open in Default Engine | Copy URL | --- | Rename | Pin | --- | Close"
    );
    let missing = NodeMenuView { failed: true, view_missing: true, ..view_of(sleeping_tab()) };
    assert_eq!(menu_of(&missing, false).2, "Download Engine | Copy URL | --- | Rename | Pin | --- | Close");
    // Export Group's file dialog can't run headless.
    assert_eq!(
        menu(plain(TYPE_GROUP), false, false, true).2,
        "Sleep Tabs | --- | Rename | Duplicate | Pin | --- | Dissolve Group | Close Group and Contents"
    );
    // A url on Image/Pdf/Json/Note offers Refresh (not Note) and Open as
    // Tab, ahead of the kind's own items.
    let with_url = |kind: CardKind| NodeMenuView { has_url: true, has_copy_url: true, ..view_of(kind) };
    assert_eq!(
        menu_of(&with_url(plain(TYPE_IMAGE)), false).2,
        "Refresh | Open as Tab | Copy URL | Open Containing Folder | Copy Path | --- | Rename | Duplicate | Pin | --- | Delete"
    );
    assert_eq!(
        menu_of(&with_url(plain(TYPE_JSON)), false).2,
        "Refresh | Open as Tab | Copy URL | Open Containing Folder | Copy Path | --- | Rename | Duplicate | Pin | --- | Delete"
    );
    // A download has no Open as Tab (it would download again), but can copy where it came from.
    let download = NodeMenuView { has_copy_url: true, ..view_of(plain(TYPE_DOWNLOAD)) };
    assert_eq!(
        menu_of(&download, false).2,
        "Copy URL | Open Containing Folder | Copy Path | --- | Rename | Duplicate | Pin | --- | Remove from List"
    );
    assert_eq!(
        menu_of(&with_url(plain(TYPE_NOTE)), false).2,
        "Open as Tab | Copy URL | Copy Text | --- | Rename | Duplicate | Pin | --- | Delete"
    );
    // Jars: tabs and groups have a Jar row ahead of the colours; the jar list and its actions are the Jar page.
    let rows = vec![JarRow { id: None, name: built_in(), color: None, ticked: true }];
    let jars = |will_switch| Some(JarMenu { rows: rows.clone(), will_switch });
    let tab = |j| NodeMenuView { jars: j, ..view_of(loaded_tab()) };
    assert_eq!(
        menu_of(&tab(jars(false)), false).2,
        "Fit Subtree | Sleep | Copy URL | Jar... | --- | Rename | Duplicate | Pin | --- | Close"
    );
    assert_eq!(
        menu_of(&tab(jars(true)), false).2,
        "Fit Subtree | Sleep | Copy URL | Jar... | --- | Rename | Duplicate | Pin | --- | Close"
    );
    let unloaded = NodeMenuView { jars: jars(true), ..view_of(sleeping_tab()) };
    assert_eq!(menu_of(&unloaded, false).2, "Open | Copy URL | Jar... | --- | Rename | Duplicate | Pin | --- | Close");
    let group = NodeMenuView { jars: jars(false), ..view_of(plain(TYPE_GROUP)) };
    assert_eq!(
        menu_of(&group, false).2,
        "Sleep Tabs | Export Group... | Jar... | --- | Rename | Duplicate | Pin | --- | Dissolve Group | \
             Close Group and Contents"
    );
    let page = |view: &NodeMenuView| -> Vec<&'static str> {
        node_menu_config(view, false)
            .jar_page
            .iter()
            .map(|item| match item {
                NodeMenuItem::Button(label, _) => *label,
                NodeMenuItem::Jars { .. } => "<jars>",
                _ => "?",
            })
            .collect()
    };
    assert_eq!(page(&tab(jars(true))), ["<jars>", "New Jar...", "Switch Jar Now"]);
    assert_eq!(page(&unloaded), ["<jars>", "New Jar..."]);
    assert_eq!(page(&group), ["<jars>", "New Jar..."]);
    assert!(page(&view_of(loaded_tab())).is_empty(), "no jar menu, no page");
    // Links (links plan, Task 6): their section follows the colours, on every kind.
    let row = EdgeMenuRow { action: NodeAction::Connect(crate::caps::LINK), label: "Link to…".into(), hover: None };
    let linked = |kind| NodeMenuView { edges: vec![row.clone()], has_group: true, ..view_of(kind) };
    assert_eq!(
        menu_of(&linked(plain(TYPE_NOTE)), false).2,
        "Copy Text | --- | Rename | Duplicate | Pin | <links> | --- | Remove from Group | --- | Delete"
    );
    assert_eq!(
        menu_of(&linked(loaded_tab()), false).2,
        "Fit Subtree | Sleep | Copy URL | --- | Rename | Duplicate | Pin | <links> | --- | Remove from Group | --- | \
             Close"
    );
}

/// The row hinted Del is the one the card's Del does, for every kind: a group's is "Close Group and Contents", never
/// "Dissolve Group" (the explicit menu row).
#[test]
fn del_hint_names_what_del_does_on_the_card() {
    for kind in kinds() {
        let config = node_menu_config(&view_of(kind), false);
        let hinted: Vec<&str> = config
            .items
            .iter()
            .filter_map(|item| match item {
                NodeMenuItem::Button(label, action) if action.key_hint().is_some() => Some(*label),
                _ => None,
            })
            .collect();
        let del = CardControl::Close.action();
        let want: Vec<&str> = config
            .items
            .iter()
            .filter_map(|item| match item {
                NodeMenuItem::Button(label, action) if *action == del => Some(*label),
                _ => None,
            })
            .collect();
        assert_eq!(hinted, want, "{kind:?}");
        assert_eq!(hinted.len(), 1, "{kind:?}: one row hinted Del");
    }
    assert_eq!(NodeAction::Delete.key_hint(), Some("Del"));
    assert_eq!(NodeAction::Dissolve.key_hint(), None, "dissolving is the explicit menu row, not Del");
}

/// The focused card's "Del <verb>" hint names the same action as the menu row hinted Del.
#[test]
fn card_del_hint_matches_the_menu_row_hinted_del() {
    for kind in kinds() {
        let config = node_menu_config(&view_of(kind), false);
        let label = config.items.iter().find_map(|item| match item {
            NodeMenuItem::Button(label, action) if action.key_hint().is_some() => Some(*label),
            _ => None,
        });
        let first = label.and_then(|l| l.split(' ').next()).map(str::to_lowercase);
        assert_eq!(first.as_deref(), Some(del_verb(kind)), "{kind:?}");
    }
    assert_eq!(del_verb(plain(TYPE_GROUP)), "close");
}

/// An unloaded tab's menu Duplicate opens a copy at its url and engine, as Ctrl+D does.
#[test]
fn duplicate_from_an_unloaded_tabs_menu_spawns_a_copy() {
    let mut app = App::default();
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.apply_node_action(&egui::Context::default(), t, NodeAction::Duplicate);
    assert_eq!(app.test_spawns.len(), 1);
    let spawn = &app.test_spawns[0].0;
    assert_ne!(spawn.id, t);
    assert_eq!((spawn.url.as_str(), spawn.kind), ("https://a/", ViewKind::Servo));
}

#[test]
fn new_jar_names_a_jar_and_picks_it_for_the_tab() {
    let mut app = App::default();
    let ctx = egui::Context::default();
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.apply_node_action(&ctx, t, NodeAction::StartNewJar);
    assert_eq!(app.naming_jar, Some((t, String::new())));
    assert_eq!(app.node_menu_view(t).unwrap().renaming, Some(String::new()), "the menu shows the name field");
    app.apply_node_action(&ctx, t, NodeAction::NewJar("  Work ".into()));
    assert_eq!(app.naming_jar, None);
    let work = app.jar_named("Work").expect("created, trimmed");
    assert_eq!(app.doc.mirror.jar(t), Some(work));
    app.apply_node_action(&ctx, t, NodeAction::StartNewJar);
    app.apply_node_action(&ctx, t, NodeAction::NewJar("   ".into()));
    assert_eq!(app.jar_defs().len(), 1, "an empty name creates nothing");
    app.apply_node_action(&ctx, t, NodeAction::StartNewJar);
    app.remove_tab(t);
    assert_eq!(app.naming_jar, None, "closing the tab drops its draft");
}

#[test]
fn the_tab_menu_lists_default_then_named_jars_with_a_tick() {
    let mut app = App::default();
    app.ensure_default_jar();
    let work = add_jar(&mut app, "Work");
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.set_jar_edge(t, Some(work));
    let shown = |app: &App| {
        let rows = app.node_menu_view(t).unwrap().jars.unwrap().rows;
        rows.into_iter().map(|r| (r.name, r.ticked)).collect::<Vec<_>>()
    };
    let plain = |name: &str| JarLabel { name: name.into(), marker: None };
    let inherit = plain("Inherit");
    assert_eq!(shown(&app), [(inherit.clone(), false), (built_in(), false), (plain("Work"), true)]);
    // A user jar named "Default" is listed after the marked built-in one, with a fragment to tell them apart.
    let user = add_jar(&mut app, "Default");
    let user_label = app.jar_label(user);
    assert!(user_label.marker.is_some());
    assert_eq!(
        shown(&app),
        [(inherit.clone(), false), (built_in(), false), (plain("Work"), true), (user_label, false)]
    );
    app.remove_remotely_deleted(user);
    app.remove_remotely_deleted(work); // deleted on another device; the tab's field still names it
    assert_eq!(shown(&app), [(inherit, false), (built_in(), false)], "a dangling edge ticks nothing");
}

/// Jar name labels: the tab and group menus list two same-named jars by their labels, a unique one plainly.
#[test]
fn the_jar_menus_list_same_named_jars_by_label() {
    let mut app = App::default();
    let default = default_label(&mut app);
    let (w1, w2, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let label = |app: &App, j| app.jar_label(j);
    let want = [label(&app, w1), label(&app, w2), label(&app, home)];
    let plain = want.clone().map(|l| l.to_string());
    assert!(plain[0].starts_with("Work · #") && plain[1].starts_with("Work · #") && want[0] != want[1], "{plain:?}");
    assert_eq!(plain[2], "Home");
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    let rows = app.node_menu_view(t).unwrap().jars.unwrap().rows;
    let listed = [&[default][..], &want[..]].concat();
    assert_eq!(rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>()[1..], listed, "after Inherit");
    let g = crate::session_sync::tests::add_group(&mut app, "G");
    let rows = app.node_menu_view(g).unwrap().jars.unwrap().rows;
    assert_eq!(rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>()[1..], listed);
}

/// Name rules: "Move into <group>" draws a hidden character in the group's name as a placeholder.
#[test]
fn move_to_rows_show_a_hidden_character_as_a_placeholder() {
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    let (g, note) = (app.alloc_node_id(), app.alloc_node_id());
    app.insert_group_at(g, None);
    app.insert_note_at(note, None);
    app.doc.mirror.set(
        g,
        crate::store::model::field::NAME,
        crate::store::model::FieldValue::Str("Re\u{200B}ading".into()),
    );
    app.menus.node_menu.open(note, egui::pos2(100.0, 100.0));
    let mut galleys = Vec::new();
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_node_menu(ui));
        output.textures_delta.clear();
        galleys = output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::Shape::Text(t) => Some(t.galley),
                _ => None,
            })
            .collect();
    }
    let row = galleys.iter().find(|g| g.text() == "Move into Re⟦200B⟧ading");
    let row = row.unwrap_or_else(|| panic!("{:?}", galleys.iter().map(|g| g.text()).collect::<Vec<_>>()));
    assert!(row.job.sections.iter().any(|s| s.format.font_id.family == egui::FontFamily::Monospace));
}

/// Name rules: a long group name is cut to `ROW_UNITS` in a "Move into" row, so it cannot widen the menu.
#[test]
fn move_to_rows_shorten_a_long_name() {
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    let (g, note) = (app.alloc_node_id(), app.alloc_node_id());
    app.insert_group_at(g, None);
    app.insert_note_at(note, None);
    app.doc.mirror.set(g, crate::store::model::field::NAME, crate::store::model::FieldValue::Str("x".repeat(200)));
    app.menus.node_menu.open(note, egui::pos2(100.0, 100.0));
    let mut texts = Vec::new();
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_node_menu(ui));
        output.textures_delta.clear();
        texts = output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
    }
    let want = format!("Move into {}…", "x".repeat(crate::names::ROW_UNITS - 1));
    assert!(texts.contains(&want), "{texts:?}");
}

/// The menu's sections share one grid: a heading, a jar row's colour dot and a colour swatch start at the same x,
/// whether or not the jar row is the selected one, so a new section cannot drift out of line.
#[test]
fn menu_sections_share_one_left_margin() {
    let ctx = crate::theme::testutil::ctx(true);
    let row = |ticked, color| JarRow { id: None, name: built_in(), color, ticked };
    let rows = vec![row(true, None), row(false, Some(egui::Color32::RED))];
    let view = NodeMenuView { jars: Some(JarMenu { rows, will_switch: false }), ..view_of(loaded_tab()) };
    // The x of what a page draws: its circles of `radius` and its heading `text`.
    let page_xs = |page: MenuPage, radius: f32, heading: &str| {
        let (mut circles, mut headings) = (Vec::new(), Vec::new());
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            };
            let config = NodeMenuConfig { page: Some(page), ..node_menu_config(&view, false) };
            let mut output = ctx.run_ui(input, |ui| {
                build_node_context_menu(ui, egui::pos2(100.0, 100.0), 1.0, true, &config, None);
            });
            output.textures_delta.clear();
            (circles, headings) = Default::default();
            for shape in output.shapes {
                match shape.shape {
                    egui::Shape::Circle(c) if c.radius == radius => circles.push(c.center.x),
                    egui::Shape::Text(t) if t.galley.text() == heading => headings.push(t.pos.x),
                    _ => {}
                }
            }
        }
        (circles, headings)
    };
    let (dots, jar_headings) = page_xs(MenuPage::Jar, 4.0, "Jar");
    assert_eq!((dots.len(), jar_headings.len()), (2, 1), "two jar dots, a heading");
    let half_slot = MenuGrid::MARKER_SLOT / 2.0;
    // Both dots are centred in a slot that starts where the heading does.
    for centre in [dots[0], dots[1]] {
        assert!((centre - half_slot - jar_headings[0]).abs() < 0.6, "{centre} vs heading {}", jar_headings[0]);
    }
}

/// One frame of `build_node_menu` in an 800x600 window.
fn menu_frame(ctx: &egui::Context, app: &mut App, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| app.build_node_menu(ui));
    output.textures_delta.clear(); // nothing uploads the font atlas here
}

/// Styled jar label markers: the tab menu draws a marker in the marker style (monospace, the weak colour), so a
/// same-named jar's `#frag` and the built-in's italic "Default" + `built-in` stand apart from one typed as
/// "Work · #3f2a", which is drawn as a plain name.
#[test]
fn the_jar_menu_draws_markers_in_their_own_style() {
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    app.ensure_default_jar();
    let (w1, _) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Work"));
    add_jar(&mut app, "Work · #3f2a");
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.menus.node_menu.open(t, egui::pos2(100.0, 100.0));
    app.menus.node_menu.show_page(Some(MenuPage::Jar));
    let mut galleys = Vec::new();
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_node_menu(ui));
        output.textures_delta.clear();
        galleys = output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::Shape::Text(t) => Some(t.galley),
                _ => None,
            })
            .collect();
    }
    let weak = ctx.global_style().visuals.weak_text_color();
    let open =
        galleys.iter().find(|g| g.text().ends_with("Back")).expect("a Back row").job.sections[0].format.font_id.clone();
    // (row text, italic name, the marker sections' text) of the row whose text ends in `label`.
    let row = |label: &str| {
        let g = galleys.iter().find(|g| g.text().trim_start_matches(['\u{2713}', ' ']) == label);
        let g = g.unwrap_or_else(|| {
            panic!("no row {label:?} in {:?}", galleys.iter().map(|g| g.text()).collect::<Vec<_>>())
        });
        let job = &g.job;
        let text = |s: &egui::text::LayoutSection| job.text[s.byte_range.start.0..s.byte_range.end.0].to_string();
        let italic = job.sections.iter().any(|s| s.format.italics);
        // The name, tick and count in the menu rows' own font (the control font), as a plain row such as "Open".
        let plain = job.sections.iter().filter(|s| s.format.font_id.family != egui::FontFamily::Monospace);
        assert!(
            plain.clone().all(|s| s.format.font_id == open),
            "{label}: {:?} vs {open:?}",
            plain.collect::<Vec<_>>()
        );
        let markers: Vec<String> = job
            .sections
            .iter()
            .filter(|s| s.format.font_id.family == egui::FontFamily::Monospace)
            .inspect(|s| assert_eq!(s.format.color, weak, "{label}"))
            .map(text)
            .collect();
        (italic, markers)
    };
    assert_eq!(row("Default built-in"), (true, vec![" built-in".to_string()]));
    let frag = crate::jar_defs::jar_fragment(w1, &app.jar_defs()).unwrap();
    assert_eq!(row(&format!("Work #{frag}")), (false, vec![format!(" #{frag}")]));
    assert_eq!(row("Work · #3f2a"), (false, vec![]), "the typed look-alike is all name");
}

/// The Links section draws under its heading, and a click on "Link to…" closes the menu and starts pick-target
/// mode from the menu's card. Driven through `build_node_menu` with real egui input.
#[test]
fn a_click_on_link_to_starts_pick_target_mode() {
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
    app.insert_note_at(a, None);
    app.insert_note_at(b, None);
    app.menus.node_menu.open(a, egui::pos2(100.0, 100.0));
    let frame = |app: &mut App, events| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_node_menu(ui));
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()))),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut texts = Vec::new();
    for _ in 0..3 {
        texts = frame(&mut app, vec![]);
    }
    let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
    assert!(names.contains(&"Links") && names.contains(&"Opened from\u{2026}"), "{names:?}");
    let at = texts.iter().find(|(t, _)| t == "Link to\u{2026}").expect("a Link to row").1.center();
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut app, vec![egui::Event::PointerMoved(at), press(true)]);
    frame(&mut app, vec![press(false)]);
    assert_eq!(app.link.connecting, Some((a, crate::caps::LINK)));
    assert_eq!(app.menus.node_menu.open, None, "the menu closed");
}

/// "Jar..." swaps the menu for the jar page and keeps it open; "Back" returns to the items. Driven through
/// `build_node_menu` with real egui input.
#[test]
fn the_jar_row_opens_a_page_and_back_returns() {
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    app.ensure_default_jar();
    let n = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    let pos = egui::pos2(100.0, 100.0);
    app.menus.node_menu.open(n, pos);
    let frame = |app: &mut App, events| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_node_menu(ui));
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()))),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let settle = |app: &mut App| (0..3).map(|_| frame(app, vec![])).last().unwrap();
    let click = |app: &mut App, at: egui::Pos2| {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(app, vec![egui::Event::PointerMoved(at), press(true)]);
        frame(app, vec![press(false)]);
    };
    let find = |texts: &[(String, egui::Rect)], what: &str| {
        texts.iter().find(|(t, _)| t.ends_with(what)).map(|t| t.1.center())
    };
    let texts = settle(&mut app);
    click(&mut app, find(&texts, "Jar...").expect("a Jar row"));
    assert_eq!(app.menus.node_menu.page, Some(MenuPage::Jar));
    assert!(app.menus.node_menu.open.is_some(), "the menu stays open");
    let texts = settle(&mut app);
    assert!(!texts.iter().any(|(t, _)| t == "Jar..."), "the jar page replaces the items");
    click(&mut app, find(&texts, "Back").expect("a Back row"));
    assert_eq!(app.menus.node_menu.page, None);
    let texts = settle(&mut app);
    assert!(texts.iter().any(|(t, _)| t == "Jar..."), "back on the items");
}

/// A left press, then its release a frame later, at (700, 500): outside a menu opened at (100, 100).
fn click_away() -> Vec<Vec<egui::Event>> {
    let press = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(700.0, 500.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), press(true)], vec![press(false)]]
}

/// Fix batch A3 (user decision): "New Jar..."'s name field creates the jar only on Enter. A click away (the field
/// losing focus without Enter) cancels, as Escape does: no jar, and no draft left in `naming_jar`. Driven through
/// `build_node_menu` with real egui input.
#[test]
fn the_new_jar_field_creates_only_on_enter() {
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    for (how, ending, creates) in [
        ("Enter", vec![vec![key(egui::Key::Enter)]], true),
        ("a click away", click_away(), false),
        ("Escape", vec![vec![key(egui::Key::Escape)]], false),
    ] {
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App::default();
        let t = crate::session_sync::tests::add(
            &mut app,
            crate::store::model::Kind::Tab,
            crate::session_sync::tests::sleeping_tab("https://a/", "A"),
        );
        app.menus.node_menu.open(t, egui::pos2(100.0, 100.0));
        let frame = |app: &mut App, events| menu_frame(&ctx, app, events);
        app.apply_node_action(&ctx, t, NodeAction::StartNewJar);
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::Text("Work".into())]);
        assert_eq!(app.naming_jar, Some((t, "Work".to_string())), "{how}: typed into the field");
        for events in ending {
            frame(&mut app, events);
        }
        frame(&mut app, vec![]);
        assert_eq!(app.naming_jar, None, "{how}: no draft left");
        let made = app.jar_named("Work");
        assert_eq!(made.is_some(), creates, "{how}: jars {:?}", app.jar_defs());
        assert_eq!(app.doc.mirror.jar(t), made, "{how}: the tab's jar");
    }
}

/// Fix batch A3: a group rename still commits on a click away (only "New Jar..." cancels).
#[test]
fn a_group_rename_still_commits_on_a_click_away() {
    let ctx = crate::theme::testutil::ctx(true);
    let mut app = App::default();
    let g = app.alloc_node_id();
    app.insert_group_at(g, None);
    app.menus.node_menu.open(g, egui::pos2(100.0, 100.0));
    app.apply_node_action(&ctx, g, NodeAction::Rename);
    menu_frame(&ctx, &mut app, vec![]);
    menu_frame(&ctx, &mut app, vec![]);
    let select_all = egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    menu_frame(&ctx, &mut app, vec![select_all, egui::Event::Text("Errands".into())]);
    for events in click_away() {
        menu_frame(&ctx, &mut app, events);
    }
    menu_frame(&ctx, &mut app, vec![]);
    assert_eq!(app.renaming_group, None);
    assert_eq!(app.doc.mirror.name(g), "Errands");
}

/// "New Jar..."'s draft goes with its menu: closed, or another node's menu open, drops it.
#[test]
fn a_jar_name_draft_does_not_outlive_its_menu() {
    let mut app = App::default();
    let ctx = egui::Context::default();
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    let u = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://b/", "B"),
    );
    app.menus.node_menu.open(t, egui::Pos2::ZERO);
    app.apply_node_action(&ctx, t, NodeAction::StartNewJar);
    app.drop_stray_jar_name();
    assert!(app.naming_jar.is_some(), "its own menu is open");
    app.menus.node_menu.open(u, egui::Pos2::ZERO);
    app.drop_stray_jar_name();
    assert_eq!(app.naming_jar, None, "another node's menu opened");
    app.menus.node_menu.open(t, egui::Pos2::ZERO);
    app.apply_node_action(&ctx, t, NodeAction::StartNewJar);
    app.menus.node_menu.close();
    app.drop_stray_jar_name();
    assert_eq!(app.naming_jar, None, "the menu closed");
}

/// The Jar page lists "Inherit", then every jar the card's edge could name (the Default above the group, one inside
/// it), and ticks the card's own edge.
#[test]
fn the_jar_page_offers_inherit_and_the_jars_in_reach() {
    let mut app = App::default();
    app.ensure_default_jar();
    let group = crate::session_sync::tests::add_group(&mut app, "G");
    let tab = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.edit_group(tab, Some(group));
    let inside = app.create_jar("Inside", crate::fetched::Placement { group: Some(group), pos: egui::Vec2::ZERO });
    let menu = app.jar_menu(tab).unwrap();
    let names: Vec<String> = menu.rows.iter().map(|r| r.name.name.clone()).collect();
    assert_eq!(names, ["Inherit", "Default", "Inside"]);
    assert!(menu.rows[0].ticked);
    app.apply_node_action(&egui::Context::default(), tab, NodeAction::SetJar(Some(inside)));
    assert!(app.jar_menu(tab).unwrap().rows.iter().any(|r| r.id == Some(inside) && r.ticked));
    app.apply_node_action(&egui::Context::default(), tab, NodeAction::SetJar(None));
    assert!(app.jar_menu(tab).unwrap().rows[0].ticked, "Inherit clears the edge");
    let note = app.alloc_node_id();
    app.insert_note_at(note, None);
    assert!(app.node_menu_view(note).unwrap().jars.is_none(), "only tabs and groups have jars");
}

/// Name rules: a group rename commits the clean name; one with nothing left keeps the old name.
#[test]
fn a_group_rename_commits_the_clean_name() {
    let ctx = egui::Context::default();
    let mut app = App::default();
    let g = app.alloc_node_id();
    app.insert_group_at(g, None);
    app.apply_node_action(&ctx, g, NodeAction::CommitRename("  Re\u{200B}ading\u{00A0}\u{00A0}list\n".into()));
    assert_eq!(app.doc.mirror.name(g), "Reading list");
    app.apply_node_action(&ctx, g, NodeAction::CommitRename("\u{200B} \u{202E}".into()));
    assert_eq!(app.doc.mirror.name(g), "Reading list");
}

/// Any node renames: the clean name overrides the derived label, an empty draft clears it, and the screen reader
/// name follows.
#[test]
fn a_note_rename_overrides_its_label_and_an_empty_draft_clears_it() {
    let ctx = egui::Context::default();
    let mut app = App::default();
    let note = app.alloc_node_id();
    app.insert_note_at(note, None);
    app.apply_node_action(&ctx, note, NodeAction::CommitRename("  Groceries\n".into()));
    assert_eq!(app.doc.mirror.name(note), "Groceries");
    assert_eq!(app.accessible_name(note).as_deref(), Some("Note: Groceries"));
    app.apply_node_action(&ctx, note, NodeAction::CommitRename(" ".into()));
    assert_eq!(app.doc.mirror.name(note), "");
    assert_eq!(app.accessible_name(note).as_deref(), Some("Note"));
}

/// Name rules: "New Jar..." makes a jar under the clean name, and none from a name with nothing left.
#[test]
fn new_jar_takes_the_clean_name() {
    let ctx = egui::Context::default();
    let mut app = App::default();
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.apply_node_action(&ctx, t, NodeAction::NewJar("\u{200B} ".into()));
    assert!(app.jar_defs().is_empty());
    app.apply_node_action(&ctx, t, NodeAction::NewJar(" Wo\u{200B}rk ".into()));
    assert_eq!(app.jar_defs().values().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Work"]);
}

#[test]
fn a_group_exports_under_its_clean_name() {
    assert_eq!(export_file_name("Re\u{200B}ading\n"), "Reading.json");
    assert_eq!(export_file_name("\u{202E}"), "Untitled.json");
}

/// A live tab whose engine stopped responding leads its menu with Restart View and End View; a responsive one
/// shows neither.
#[test]
fn an_unresponsive_tab_menu_leads_with_restart_and_end() {
    let labels = |view: &NodeMenuView| -> Vec<&'static str> {
        node_menu_config(view, false)
            .items
            .iter()
            .filter_map(|item| match item {
                NodeMenuItem::Button(label, _) => Some(*label),
                _ => None,
            })
            .collect()
    };
    let stuck = NodeMenuView { unresponsive: true, ..view_of(loaded_tab()) };
    assert_eq!(labels(&stuck)[..2], ["Restart View", "End View"]);
    assert!(!labels(&view_of(loaded_tab())).contains(&"Restart View"));
}

#[test]
fn a_jar_cards_menu_has_the_generic_items_and_its_cookie_items() {
    let labels = |app: &App, id: NodeId| -> Vec<&'static str> {
        node_menu_config(&app.node_menu_view(id).unwrap(), false)
            .items
            .iter()
            .filter_map(|item| match item {
                NodeMenuItem::Button(label, _) => Some(*label),
                _ => None,
            })
            .collect()
    };
    let mut app = App::default();
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work");
    let shown = labels(&app, jar);
    for want in ["Clear Cookies...", "Make Ephemeral", "Rename", "Duplicate", "Pin", "Close"] {
        assert!(shown.contains(&want), "{want} missing from {shown:?}");
    }
    assert!(!shown.iter().any(|l| l.starts_with("Delete")), "{shown:?}");
    app.set_jar_ephemeral(jar, true);
    assert!(labels(&app, jar).contains(&"Stop Ephemeral"));
}

#[test]
fn make_ephemeral_on_a_jar_card_with_a_saved_file_asks_first() {
    let dir = crate::store::testutil::temp_dir("jar-menu-eph");
    let mut app = App { jars_dir_override: Some(dir.clone()), ..App::default() };
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work");
    std::fs::write(dir.join(format!("{jar}.json")), b"x").unwrap();
    app.apply_node_action(&egui::Context::default(), jar, NodeAction::SetJarEphemeral(true));
    assert_eq!(app.confirm, Some(crate::popups::Confirm::Jar(crate::jar_defs::JarConfirm::Ephemeral(jar))));
}

#[test]
fn an_empty_rename_leaves_a_jar_its_name() {
    let mut app = App::default();
    let jar = crate::session_sync::tests::add_jar(&mut app, "Work");
    app.commit_rename(jar, "   ");
    assert_eq!(app.doc.mirror.name(jar), "Work", "a jar is known by its name alone, like a group");
    app.commit_rename(jar, "Home");
    assert_eq!(app.doc.mirror.name(jar), "Home");
}
