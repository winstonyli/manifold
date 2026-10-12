use super::*;
use crate::card_kind::fixtures::{kinds, loaded_tab, plain, sleeping_tab};

// One row per kind, pinned to the exact menus each kind showed before
// the per-kind builders were merged (see the node-menu spec), and
// before the tab and group menus joined them.
/// "Move into <group>" offers only sibling groups, and joins the node to the chosen one.
#[test]
fn move_to_group_offers_sibling_groups_and_joins() {
    let mut app = App::default();
    let (g1, g2, note) = (app.alloc_node_id(), app.alloc_node_id(), app.alloc_node_id());
    app.insert_group_at(g1.clone(), None);
    app.insert_group_at(g2.clone(), None);
    app.insert_note_at(note.clone(), None);
    // A group nested in g1 is not a sibling of the top-level note.
    let inner = app.alloc_node_id();
    app.insert_group_at(inner.clone(), None);
    app.edit_group(&inner, Some(g1.clone()));
    let ids = |app: &App, id: &str| -> Vec<String> {
        app.node_menu_view(id).unwrap().move_targets.into_iter().map(|(gid, _)| gid).collect()
    };
    assert_eq!(ids(&app, &note), vec![g1.clone(), g2.clone()]);
    assert_eq!(ids(&app, &g1), vec![g2.clone()], "a group isn't offered itself");
    app.apply_node_action(&egui::Context::default(), &note, NodeAction::MoveToGroup(g2.clone()));
    assert_eq!(app.mirror.group(&note), Some(g2.as_str()));
    // On a group "Move into" nests it, unlike a drop of one group on another (which merges).
    let group = plain(TYPE_GROUP);
    assert_eq!(drop_action(group, DropTarget::Card(group), false), Some(DropEffect::Merge));
    app.apply_node_action(&egui::Context::default(), &g1, NodeAction::MoveToGroup(g2.clone()));
    assert_eq!(app.mirror.group(&g1), Some(g2.as_str()), "g1 nested in g2, not merged away");
    assert_eq!(app.mirror.group(&inner), Some(g1.as_str()), "g1 keeps its own members");
}

fn view_of(kind: CardKind) -> NodeMenuView {
    NodeMenuView {
        kind,
        has_group: false,
        color: None,
        has_url: false,
        has_copy_url: false,
        pinned: false,
        unresponsive: false,
        other_default_engine: false,
        failed: false,
        engine_missing: false,
        move_targets: Vec::new(),
        renaming: None,
        jars: None,
        edges: Vec::new(),
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
                NodeMenuItem::Colors => "<colors>",
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
                format!("Open Containing Folder | Copy Path | Pin | <colors>{g} | --- | Remove from List")
            )
        );
        assert_eq!(
            menu(plain(TYPE_NOTE), false),
            ("note_context_menu", 150.0, format!("Copy Text | Pin | <colors>{g} | --- | Delete"))
        );
        assert_eq!(
            menu(plain(TYPE_IMAGE), false),
            (
                "image_context_menu",
                170.0,
                format!("Open Containing Folder | Copy Path | Pin | <colors>{g} | --- | Delete")
            )
        );
        assert_eq!(
            menu(plain(TYPE_PDF), false),
            (
                "pdf_context_menu",
                170.0,
                format!("Open Containing Folder | Copy Path | Pin | <colors>{g} | --- | Delete")
            )
        );
        for json in [TYPE_JSON, TYPE_JSON_LEAF] {
            assert_eq!(
                menu(plain(json), false),
                (
                    "json_context_menu",
                    170.0,
                    format!("Open Containing Folder | Copy Path | Pin | <colors>{g} | --- | Delete")
                )
            );
        }
        assert_eq!(
            menu(loaded_tab(), false),
            (
                "tab_context_menu",
                150.0,
                format!("Duplicate | Sleep | Copy URL | Fit Subtree | Pin | <colors>{g} | --- | Close")
            )
        );
        assert_eq!(
            menu(loaded_tab(), true).2,
            format!("Duplicate | Sleep | Copy URL | Fit Subtree | Unpin | <colors>{g} | --- | Close")
        );
        assert_eq!(
            menu(plain(TYPE_GROUP), false),
            (
                "group_context_menu",
                150.0,
                format!(
                    "Rename | Sleep Tabs | Pin | <colors>{g} | --- | Dissolve Group | Close Group and Contents | \
                         Export Group..."
                )
            )
        );
        assert_eq!(
            menu(sleeping_tab(), false),
            (
                "unloaded_tab_context_menu",
                170.0,
                format!("Open | Duplicate | Copy URL | Pin | <colors>{g} | --- | Close")
            )
        );
    }
    // A failed tab offers Retry (and the default engine, when it isn't
    // the engine that failed) instead of Open; a sleeping/waking one
    // just offers Open. Neither failed one offers Duplicate, which would fail the same way.
    let failed = NodeMenuView { other_default_engine: true, failed: true, ..view_of(sleeping_tab()) };
    assert_eq!(menu_of(&failed, false).2, "Retry | Open in Default Engine | Copy URL | Pin | <colors> | --- | Close");
    let missing = NodeMenuView { failed: true, engine_missing: true, ..view_of(sleeping_tab()) };
    assert_eq!(menu_of(&missing, false).2, "Download Engine | Copy URL | Pin | <colors> | --- | Close");
    // Export Group's file dialog can't run headless.
    assert_eq!(
        menu(plain(TYPE_GROUP), false, false, true).2,
        "Rename | Sleep Tabs | Pin | <colors> | --- | Dissolve Group | Close Group and Contents"
    );
    // A url on Image/Pdf/Json/Note offers Refresh (not Note) and Open as
    // Tab, ahead of the kind's own items.
    let with_url = |kind: CardKind| NodeMenuView { has_url: true, has_copy_url: true, ..view_of(kind) };
    assert_eq!(
        menu_of(&with_url(plain(TYPE_IMAGE)), false).2,
        "Refresh | Open as Tab | Copy URL | Open Containing Folder | Copy Path | Pin | <colors> | --- | Delete"
    );
    assert_eq!(
        menu_of(&with_url(plain(TYPE_JSON)), false).2,
        "Refresh | Open as Tab | Copy URL | Open Containing Folder | Copy Path | Pin | <colors> | --- | Delete"
    );
    // A download has no Open as Tab (it would download again), but can copy where it came from.
    let download = NodeMenuView { has_copy_url: true, ..view_of(plain(TYPE_DOWNLOAD)) };
    assert_eq!(
        menu_of(&download, false).2,
        "Copy URL | Open Containing Folder | Copy Path | Pin | <colors> | --- | Remove from List"
    );
    assert_eq!(
        menu_of(&with_url(plain(TYPE_NOTE)), false).2,
        "Open as Tab | Copy URL | Copy Text | Pin | <colors> | --- | Delete"
    );
    // Jars (per-tab jars plan, Task 7): tabs and groups list them ahead of the colours.
    let rows = vec![JarRow { id: None, name: JarLabel::built_in(), color: None, ticked: true, count: None }];
    let jars = |can_follow, will_switch, has_default| {
        Some(JarMenu { rows: rows.clone(), can_follow, will_switch, has_default })
    };
    let tab = |j| NodeMenuView { jars: j, ..view_of(loaded_tab()) };
    assert_eq!(
        menu_of(&tab(jars(false, false, false)), false).2,
        "Duplicate | Sleep | Copy URL | Fit Subtree | <jars> | New Jar... | Pin | <colors> | --- | Close"
    );
    assert_eq!(
        menu_of(&tab(jars(true, true, false)), false).2,
        "Duplicate | Sleep | Copy URL | Fit Subtree | <jars> | New Jar... | Follow Group's Jar | \
             Switch Jar Now | Pin | <colors> | --- | Close"
    );
    let unloaded = NodeMenuView { jars: jars(true, true, false), ..view_of(sleeping_tab()) };
    assert_eq!(
        menu_of(&unloaded, false).2,
        "Open | Duplicate | Copy URL | <jars> | New Jar... | Follow Group's Jar | Pin | <colors> | --- | Close"
    );
    let group = |has_default| NodeMenuView { jars: jars(false, false, has_default), ..view_of(plain(TYPE_GROUP)) };
    assert_eq!(
        menu_of(&group(false), false).2,
        "Rename | Sleep Tabs | <jars> | Pin | <colors> | --- | Dissolve Group | Close Group and Contents | \
             Export Group..."
    );
    assert_eq!(
        menu_of(&group(true), false).2,
        "Rename | Sleep Tabs | <jars> | Clear Default Jar | Pin | <colors> | --- | Dissolve Group | \
             Close Group and Contents | Export Group..."
    );
    // Links (links plan, Task 6): their section follows the colours, on every kind.
    let row = EdgeMenuRow { action: NodeAction::Connect(crate::caps::LINK), label: "Link to…".into(), hover: None };
    let linked = |kind| NodeMenuView { edges: vec![row.clone()], has_group: true, ..view_of(kind) };
    assert_eq!(
        menu_of(&linked(plain(TYPE_NOTE)), false).2,
        "Copy Text | Pin | <colors> | <links> | --- | Remove from Group | --- | Delete"
    );
    assert_eq!(
        menu_of(&linked(loaded_tab()), false).2,
        "Duplicate | Sleep | Copy URL | Fit Subtree | Pin | <colors> | <links> | --- | Remove from Group | --- | \
             Close"
    );
}

/// The row hinted Del is the one the card's Del does, for every kind: a group's is Dissolve Group, never
/// "Close Group and Contents", which closes the members too.
#[test]
fn del_hint_names_what_del_does_on_the_card() {
    for kind in kinds() {
        let config = node_menu_config(&view_of(kind), false);
        let hinted: Vec<&str> = config
            .items
            .iter()
            .filter_map(|item| match item {
                NodeMenuItem::Button(label, action) if action.key_hint(kind).is_some() => Some(*label),
                _ => None,
            })
            .collect();
        let del = CardControl::Close.action(kind);
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
    assert_eq!(NodeAction::Dissolve.key_hint(plain(TYPE_GROUP)), Some("Del"));
    assert_eq!(NodeAction::Delete.key_hint(plain(TYPE_GROUP)), None, "closing the members isn't Del");
    assert_eq!(NodeAction::Delete.key_hint(loaded_tab()), Some("Del"));
}

/// The focused card's "Del <verb>" hint names the same action as the menu row hinted Del.
#[test]
fn card_del_hint_matches_the_menu_row_hinted_del() {
    for kind in kinds() {
        let config = node_menu_config(&view_of(kind), false);
        let label = config.items.iter().find_map(|item| match item {
            NodeMenuItem::Button(label, action) if action.key_hint(kind).is_some() => Some(*label),
            _ => None,
        });
        let first = label.and_then(|l| l.split(' ').next()).map(str::to_lowercase);
        assert_eq!(first.as_deref(), Some(del_verb(kind)), "{kind:?}");
    }
    assert_eq!(del_verb(plain(TYPE_GROUP)), "dissolve");
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
    app.apply_node_action(&egui::Context::default(), &t, NodeAction::Duplicate);
    assert_eq!(app.test_spawns.len(), 1);
    let spawn = &app.test_spawns[0].0;
    assert_ne!(spawn.id, t);
    assert_eq!((spawn.url.as_str(), spawn.kind), ("https://a/", EngineKind::Servo));
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
    app.apply_node_action(&ctx, &t, NodeAction::StartNewJar);
    assert_eq!(app.naming_jar, Some((t.clone(), String::new())));
    assert_eq!(app.node_menu_view(&t).unwrap().renaming, Some(String::new()), "the menu shows the name field");
    app.apply_node_action(&ctx, &t, NodeAction::NewJar("  Work ".into()));
    assert_eq!(app.naming_jar, None);
    let work = app.jar_named("Work").expect("created, trimmed");
    assert_eq!(app.tab_jar(&t).unwrap(), crate::jar::JarChoice { jar: Some(work), set: true });
    app.apply_node_action(&ctx, &t, NodeAction::StartNewJar);
    app.apply_node_action(&ctx, &t, NodeAction::NewJar("   ".into()));
    assert_eq!(app.mirror.jar_defs().len(), 1, "an empty name creates nothing");
    app.apply_node_action(&ctx, &t, NodeAction::StartNewJar);
    app.remove_tab(&t);
    assert_eq!(app.naming_jar, None, "closing the tab drops its draft");
}

#[test]
fn the_tab_menu_lists_default_then_named_jars_with_a_tick() {
    let mut app = App::default();
    let work = app.create_jar("Work");
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.set_tab_jar(&t, Some(work));
    let shown = |app: &App| {
        let rows = app.node_menu_view(&t).unwrap().jars.unwrap().rows;
        rows.into_iter().map(|r| (r.name, r.ticked)).collect::<Vec<_>>()
    };
    let plain = |name: &str| JarLabel { name: name.into(), marker: None };
    assert_eq!(shown(&app), [(JarLabel::built_in(), false), (plain("Work"), true)]);
    // Styled jar label markers: a user jar named "Default" is listed plainly, after the marked built-in one.
    let user = app.create_jar("Default");
    assert_eq!(shown(&app), [(JarLabel::built_in(), false), (plain("Work"), true), (plain("Default"), false)]);
    app.mirror.forget_jar(user);
    app.mirror.forget_jar(work); // deleted on another device; the tab's field still names it
    assert_eq!(shown(&app), [(JarLabel::built_in(), true)]);
}

/// Jar name labels: the tab and group menus list two same-named jars by their labels, a unique one plainly.
#[test]
fn the_jar_menus_list_same_named_jars_by_label() {
    let mut app = App::default();
    let (w1, w2, home) = (app.create_jar("Work"), app.create_jar("Work"), app.create_jar("Home"));
    let label = |app: &App, j| app.jar_label(crate::jar::JarKey::Named(j));
    let want = [label(&app, w1), label(&app, w2), label(&app, home)];
    let plain = want.clone().map(|l| l.to_string());
    assert!(plain[0].starts_with("Work · #") && plain[1].starts_with("Work · #") && want[0] != want[1], "{plain:?}");
    assert_eq!(plain[2], "Home");
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    let rows = app.node_menu_view(&t).unwrap().jars.unwrap().rows;
    assert_eq!(rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>()[1..], want);
    let g = crate::session_sync::tests::add_group(&mut app, "G");
    let rows = app.node_menu_view(&g).unwrap().jars.unwrap().rows;
    assert_eq!(rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>(), want);
}

/// Name rules: "Move into <group>" draws a hidden character in the group's name as a placeholder.
#[test]
fn move_to_rows_show_a_hidden_character_as_a_placeholder() {
    let ctx = menu_ctx();
    let mut app = App::default();
    let (g, note) = (app.alloc_node_id(), app.alloc_node_id());
    app.insert_group_at(g.clone(), None);
    app.insert_note_at(note.clone(), None);
    app.mirror.set(
        &g,
        crate::store::model::field::NAME,
        crate::store::model::FieldValue::Str("Re\u{200B}ading".into()),
    );
    app.menus.node_menu = Some((note, egui::pos2(100.0, 100.0)));
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
    let ctx = menu_ctx();
    let mut app = App::default();
    let (g, note) = (app.alloc_node_id(), app.alloc_node_id());
    app.insert_group_at(g.clone(), None);
    app.insert_note_at(note.clone(), None);
    app.mirror.set(&g, crate::store::model::field::NAME, crate::store::model::FieldValue::Str("x".repeat(200)));
    app.menus.node_menu = Some((note, egui::pos2(100.0, 100.0)));
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

fn menu_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    crate::theme::apply_style(&ctx);
    ctx
}

/// The menu's sections share one grid: a heading, a jar row's colour dot and a colour swatch start at the same x,
/// whether or not the jar row is the selected one, so a new section cannot drift out of line.
#[test]
fn menu_sections_share_one_left_margin() {
    let ctx = menu_ctx();
    let row = |ticked, color| JarRow { id: None, name: JarLabel::built_in(), color, ticked, count: None };
    let rows = vec![row(true, None), row(false, Some(egui::Color32::RED))];
    let view = NodeMenuView {
        jars: Some(JarMenu { rows, can_follow: false, will_switch: false, has_default: false }),
        ..view_of(loaded_tab())
    };
    let config = node_menu_config(&view, false);
    let (mut dots, mut swatches, mut headings) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            build_node_context_menu(ui, egui::pos2(100.0, 100.0), 1.0, true, &config, None, None);
        });
        output.textures_delta.clear();
        (dots, swatches, headings) = Default::default();
        for shape in output.shapes {
            match shape.shape {
                egui::Shape::Circle(c) if c.radius == 4.0 => dots.push(c.center.x),
                egui::Shape::Circle(c) if c.radius == 7.0 => swatches.push(c.center.x),
                egui::Shape::Text(t) if matches!(t.galley.text(), "Jar" | "Color") => headings.push(t.pos.x),
                _ => {}
            }
        }
    }
    assert_eq!((dots.len(), headings.len()), (2, 2), "two jar dots, two headings");
    assert!(!swatches.is_empty());
    let half_slot = MenuGrid::MARKER_SLOT / 2.0;
    // Both dots and the first swatch are centred in a slot that starts where the headings do.
    for centre in [dots[0], dots[1], swatches[0]] {
        assert!((centre - half_slot - headings[0]).abs() < 0.6, "{centre} vs heading {}", headings[0]);
    }
    assert!((headings[0] - headings[1]).abs() < 0.01, "headings: {headings:?}");
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
/// same-named jar's `#frag` and the built-in's italic "Default" + `built-in` stand apart from a user jar named
/// "Default" and one typed as "Work · #3f2a", which are drawn as plain names.
#[test]
fn the_jar_menu_draws_markers_in_their_own_style() {
    let ctx = menu_ctx();
    let mut app = App::default();
    let (w1, _) = (app.create_jar("Work"), app.create_jar("Work"));
    app.create_jar("Default");
    app.create_jar("Work · #3f2a");
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.menus.node_menu = Some((t, egui::pos2(100.0, 100.0)));
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
    let open = galleys.iter().find(|g| g.text() == "Open").expect("an Open row").job.sections[0].format.font_id.clone();
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
    assert_eq!(row("Default"), (false, vec![]), "a user jar named Default is plain");
    let frag = crate::jar_defs::jar_fragment(w1, app.mirror.jar_defs()).unwrap();
    assert_eq!(row(&format!("Work #{frag}")), (false, vec![format!(" #{frag}")]));
    assert_eq!(row("Work · #3f2a"), (false, vec![]), "the typed look-alike is all name");
}

/// The Links section draws under its heading, and a click on "Link to…" closes the menu and starts pick-target
/// mode from the menu's card. Driven through `build_node_menu` with real egui input.
#[test]
fn a_click_on_link_to_starts_pick_target_mode() {
    let ctx = menu_ctx();
    let mut app = App::default();
    let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
    app.insert_note_at(a.clone(), None);
    app.insert_note_at(b.clone(), None);
    app.menus.node_menu = Some((a.clone(), egui::pos2(100.0, 100.0)));
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
    assert_eq!(app.menus.node_menu, None, "the menu closed");
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
        let ctx = menu_ctx();
        let mut app = App::default();
        let t = crate::session_sync::tests::add(
            &mut app,
            crate::store::model::Kind::Tab,
            crate::session_sync::tests::sleeping_tab("https://a/", "A"),
        );
        app.menus.node_menu = Some((t.clone(), egui::pos2(100.0, 100.0)));
        let frame = |app: &mut App, events| menu_frame(&ctx, app, events);
        app.apply_node_action(&ctx, &t, NodeAction::StartNewJar);
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::Text("Work".into())]);
        assert_eq!(app.naming_jar, Some((t.clone(), "Work".to_string())), "{how}: typed into the field");
        for events in ending {
            frame(&mut app, events);
        }
        frame(&mut app, vec![]);
        assert_eq!(app.naming_jar, None, "{how}: no draft left");
        let made = app.jar_named("Work");
        assert_eq!(made.is_some(), creates, "{how}: jars {:?}", app.mirror.jar_defs());
        let expected = crate::jar::JarChoice { jar: made, set: creates };
        assert_eq!(app.tab_jar(&t).unwrap(), expected, "{how}: the tab's jar");
    }
}

/// Fix batch A3: a group rename still commits on a click away (only "New Jar..." cancels).
#[test]
fn a_group_rename_still_commits_on_a_click_away() {
    let ctx = menu_ctx();
    let mut app = App::default();
    let g = app.alloc_node_id();
    app.insert_group_at(g.clone(), None);
    app.menus.node_menu = Some((g.clone(), egui::pos2(100.0, 100.0)));
    app.apply_node_action(&ctx, &g, NodeAction::Rename);
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
    assert_eq!(app.mirror.name(&g), "Errands");
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
    app.menus.node_menu = Some((t.clone(), egui::Pos2::ZERO));
    app.apply_node_action(&ctx, &t, NodeAction::StartNewJar);
    app.drop_stray_jar_name();
    assert!(app.naming_jar.is_some(), "its own menu is open");
    app.menus.node_menu = Some((u, egui::Pos2::ZERO));
    app.drop_stray_jar_name();
    assert_eq!(app.naming_jar, None, "another node's menu opened");
    app.menus.node_menu = Some((t.clone(), egui::Pos2::ZERO));
    app.apply_node_action(&ctx, &t, NodeAction::StartNewJar);
    app.menus.node_menu = None;
    app.drop_stray_jar_name();
    assert_eq!(app.naming_jar, None, "the menu closed");
}

/// A group lists only named jars, each with how many tabs picking it would change, and offers "Clear Default
/// Jar" once it has one; "Follow Group's Jar" appears only for a set tab under a group default.
#[test]
fn the_group_menu_counts_the_cascade_and_a_set_tab_can_follow() {
    let mut app = App::default();
    let ctx = egui::Context::default();
    let work = app.create_jar("Work");
    let g = app.alloc_node_id();
    app.insert_group_at(g.clone(), None);
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.edit_group(&t, Some(g.clone()));
    let menu = app.node_menu_view(&g).unwrap().jars.unwrap();
    assert_eq!(
        menu.rows,
        [JarRow {
            id: Some(work),
            name: JarLabel { name: "Work".into(), marker: None },
            color: menu.rows[0].color,
            ticked: false,
            count: Some(1)
        }]
    );
    assert!(!menu.has_default);
    app.apply_node_action(&ctx, &g, NodeAction::SetGroupDefaultJar(work));
    let menu = app.node_menu_view(&g).unwrap().jars.unwrap();
    assert!(menu.has_default && menu.rows[0].ticked);
    assert_eq!(menu.rows[0].count, None, "the tab already took it: no \"(0)\"");
    assert!(!app.node_menu_view(&t).unwrap().jars.unwrap().can_follow, "an unset tab already follows");
    app.apply_node_action(&ctx, &t, NodeAction::SetJar(None));
    assert!(app.node_menu_view(&t).unwrap().jars.unwrap().can_follow);
    app.apply_node_action(&ctx, &t, NodeAction::FollowGroupJar);
    assert_eq!(app.tab_jar(&t).unwrap(), crate::jar::JarChoice { jar: Some(work), set: false });
    app.apply_node_action(&ctx, &g, NodeAction::ClearGroupDefaultJar);
    assert!(!app.node_menu_view(&g).unwrap().jars.unwrap().has_default);
    let note = app.alloc_node_id();
    app.insert_note_at(note.clone(), None);
    assert!(app.node_menu_view(&note).unwrap().jars.is_none(), "only tabs and groups have jars");
}

/// Name rules: a group rename commits the clean name; one with nothing left keeps the old name.
#[test]
fn a_group_rename_commits_the_clean_name() {
    let ctx = egui::Context::default();
    let mut app = App::default();
    let g = app.alloc_node_id();
    app.insert_group_at(g.clone(), None);
    app.apply_node_action(&ctx, &g, NodeAction::CommitRename("  Re\u{200B}ading\u{00A0}\u{00A0}list\n".into()));
    assert_eq!(app.mirror.name(&g), "Reading list");
    app.apply_node_action(&ctx, &g, NodeAction::CommitRename("\u{200B} \u{202E}".into()));
    assert_eq!(app.mirror.name(&g), "Reading list");
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
    app.apply_node_action(&ctx, &t, NodeAction::NewJar("\u{200B} ".into()));
    assert!(app.mirror.jar_defs().is_empty());
    app.apply_node_action(&ctx, &t, NodeAction::NewJar(" Wo\u{200B}rk ".into()));
    assert_eq!(app.mirror.jar_defs().values().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Work"]);
}

#[test]
fn a_group_exports_under_its_clean_name() {
    assert_eq!(export_file_name("Re\u{200B}ading\n"), "Reading.json");
    assert_eq!(export_file_name("\u{202E}"), "Untitled.json");
}

/// A live tab whose engine stopped responding leads its menu with Restart Engine and End Engine; a responsive one
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
    assert_eq!(labels(&stuck)[..2], ["Restart Engine", "End Engine"]);
    assert!(!labels(&view_of(loaded_tab())).contains(&"Restart Engine"));
}
