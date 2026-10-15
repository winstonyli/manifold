use super::{id_digest, jar_fragment, named_jar_label, JarConfirm, JarLabel, JarMarker};
use crate::card::DropEffect;
use crate::session_sync::tests::default_jar_id;
use crate::session_sync::tests::{add, add_jar, sleeping_tab};
use crate::store::model::{field, FieldValue, Kind, NodeId};
use crate::App;
use std::collections::{BTreeMap, BTreeSet};

/// The Default jar's label.
fn built_in() -> JarLabel {
    JarLabel { name: "Default".into(), marker: Some(JarMarker::BuiltIn) }
}

fn tab_in(app: &mut App, group: Option<&str>) -> String {
    let id = add(app, Kind::Tab, sleeping_tab("https://a.com/", "A"));
    app.edit_group(&id, group.map(str::to_string));
    id
}
fn group_in(app: &mut App, parent: Option<&str>) -> String {
    let id = crate::session_sync::tests::add_group(app, "G");
    app.edit_group(&id, parent.map(str::to_string));
    id
}
fn with_default() -> (App, NodeId) {
    let mut app = App::default();
    app.ensure_default_jar();
    let default = app.default_jar().unwrap();
    (app, default)
}
fn jar_at(app: &mut App, name: &str, group: Option<&str>) -> NodeId {
    let at = crate::fetched::Placement { group: group.map(str::to_owned), pos: egui::Vec2::ZERO };
    app.create_jar(name, at)
}

#[test]
fn a_tab_resolves_its_own_edge_then_each_enclosing_groups_then_the_default() {
    let (mut app, default) = with_default();
    let outer = group_in(&mut app, None);
    let inner = group_in(&mut app, Some(&outer));
    let tab = tab_in(&mut app, Some(&inner));
    assert_eq!(app.effective_jar(&tab), Some((default, false)), "no edge anywhere");
    let work = jar_at(&mut app, "Work", None);
    app.set_jar_edge(&outer, Some(work));
    assert_eq!(app.effective_jar(&tab), Some((work, false)), "the grandparent's edge");
    let home = jar_at(&mut app, "Home", Some(&outer));
    app.set_jar_edge(&inner, Some(home));
    assert_eq!(app.effective_jar(&tab), Some((home, false)), "the nearer group's edge");
    let mine = jar_at(&mut app, "Mine", Some(&inner));
    app.set_jar_edge(&tab, Some(mine));
    assert_eq!(app.effective_jar(&tab), Some((mine, false)), "its own edge");
    app.set_jar_edge(&tab, None);
    assert_eq!(app.effective_jar(&tab), Some((home, false)), "cleared: inherits again");
}

#[test]
fn an_edge_to_a_closed_jar_dangles_to_the_default_and_does_not_climb() {
    let (mut app, default) = with_default();
    let group = group_in(&mut app, None);
    let tab = tab_in(&mut app, Some(&group));
    let work = jar_at(&mut app, "Work", None);
    let mine = jar_at(&mut app, "Mine", Some(&group));
    app.set_jar_edge(&group, Some(work));
    app.set_jar_edge(&tab, Some(mine));
    app.close_node(&mine.to_string());
    assert_eq!(app.effective_jar(&tab), Some((default, true)), "dangling, not the group's Work");
}

#[test]
fn an_out_of_scope_edge_from_another_device_is_kept_and_dangles() {
    let (mut app, default) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let tab = tab_in(&mut app, Some(&a));
    let far = jar_at(&mut app, "Far", Some(&b));
    // What a remote write looks like: the field set directly, with no scope check.
    app.mirror.set(&tab, field::JAR, FieldValue::Id(Some(far)));
    assert_eq!(app.effective_jar(&tab), Some((default, true)));
    assert_eq!(app.mirror.jar(&tab), Some(far), "never rewritten");
}

#[test]
fn connect_reaches_a_jar_at_the_level_or_above_but_not_in_a_sibling_group() {
    let (mut app, _) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let tab = tab_in(&mut app, Some(&a));
    let here = jar_at(&mut app, "Here", Some(&a)).to_string();
    let above = jar_at(&mut app, "Above", None).to_string();
    let sibling = jar_at(&mut app, "Sibling", Some(&b)).to_string();
    let edge = crate::caps::JAR_EDGE;
    assert_eq!(app.connect(edge, &tab, &here), Ok(()));
    assert_eq!(app.connect(edge, &tab, &here), Err(crate::links::ConnectError::Exists));
    assert_eq!(app.connect(edge, &tab, &above), Ok(()), "replaces the edge: a field holds one");
    assert_eq!(app.mirror.jar(&tab), above.parse().ok());
    assert_eq!(app.connect(edge, &tab, &sibling), Err(crate::links::ConnectError::OutOfScope));
    app.disconnect(edge, &tab, &above);
    assert_eq!(app.mirror.jar(&tab), None);
}

#[test]
fn a_groups_edge_reaches_a_jar_inside_it_and_its_tabs_use_it() {
    let (mut app, _) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let inner = group_in(&mut app, Some(&a));
    let tab = tab_in(&mut app, Some(&inner));
    let inside = jar_at(&mut app, "Inside", Some(&a)).to_string();
    let deeper = jar_at(&mut app, "Deeper", Some(&inner)).to_string();
    let elsewhere = jar_at(&mut app, "Elsewhere", Some(&b)).to_string();
    let edge = crate::caps::JAR_EDGE;
    // Decision 2026-10-14 (pending the user's confirmation): a group's own contents count as its level.
    assert_eq!(app.connect(edge, &a, &inside), Ok(()), "a jar inside the group");
    assert_eq!(app.effective_jar(&tab), Some((inside.parse().unwrap(), false)), "inherited through `inner`");
    assert_eq!(app.connect(edge, &a, &deeper), Err(crate::links::ConnectError::OutOfScope), "not a nested group's");
    assert_eq!(app.connect(edge, &a, &elsewhere), Err(crate::links::ConnectError::OutOfScope), "not a sibling's");
    // A tab's edge still reaches its own level and above only.
    let top_tab = tab_in(&mut app, Some(&a));
    assert_eq!(app.connect(edge, &top_tab, &deeper), Err(crate::links::ConnectError::OutOfScope));
}

#[test]
fn a_move_clears_an_edge_it_takes_out_of_reach_and_keeps_one_in_reach() {
    let (mut app, _) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let kept = tab_in(&mut app, Some(&a));
    let lost = tab_in(&mut app, Some(&a));
    let plain = tab_in(&mut app, Some(&a));
    let top = jar_at(&mut app, "Top", None);
    let local = jar_at(&mut app, "Local", Some(&a));
    // Inside `b`: a group's edge reaches its own contents (decision 2026-10-14).
    let b_jar = jar_at(&mut app, "B", Some(&b));
    app.set_jar_edge(&kept, Some(top));
    app.set_jar_edge(&lost, Some(local));
    app.set_jar_edge(&b, Some(b_jar));
    for tab in [&kept, &lost, &plain] {
        app.edit_group(tab, Some(b.clone()));
        app.recheck_edge_scope(tab);
    }
    assert_eq!(app.mirror.jar(&kept), Some(top), "an ancestor jar is still in reach");
    assert_eq!(app.mirror.jar(&lost), None, "cleared");
    assert_eq!(app.effective_jar(&plain), Some((b_jar, false)), "no edge: takes its new group's jar");
    // Moving a jar out of reach clears the edges to it.
    app.edit_group(&local.to_string(), Some(b.clone()));
    app.set_jar_edge(&kept, Some(local));
    app.edit_group(&local.to_string(), Some(a.clone()));
    app.recheck_edge_scope(&local.to_string());
    assert_eq!(app.mirror.jar(&kept), None);
    assert!(app.toasts.save_toast_queue.iter().any(|t| t.contains("Local")), "a toast names it");
}

/// The real move paths call `recheck_edge_scope`: a drop into another group clears an edge it leaves out of reach.
#[test]
fn a_drop_into_another_group_rechecks_the_edge() {
    let (mut app, _) = with_default();
    let (a, b) = (group_in(&mut app, None), group_in(&mut app, None));
    let tab = tab_in(&mut app, Some(&a));
    let local = jar_at(&mut app, "Local", Some(&a));
    app.set_jar_edge(&tab, Some(local));
    app.apply_drop(&tab, Some((DropEffect::Join, Some(b))));
    assert_eq!(app.mirror.jar(&tab), None);
}

#[test]
fn a_new_tab_inherits_and_an_opened_tab_copies_its_openers_edge() {
    let (mut app, _) = with_default();
    let (work, home) = (jar_at(&mut app, "Work", None), jar_at(&mut app, "Home", None));
    let g = group_in(&mut app, None);
    app.set_jar_edge(&g, Some(work));
    app.spawn_new_tab(None, Some(g.clone()), "https://new/");
    let (spawn, _) = app.test_spawns.last().unwrap();
    assert_eq!(spawn.jar, None, "a new tab has no edge of its own: it inherits");
    let opener = tab_in(&mut app, Some(&g));
    app.set_jar_edge(&opener, Some(home));
    app.spawn_tab_from(&opener, "https://opened/");
    let (spawn, _) = app.test_spawns.last().unwrap();
    assert_eq!(spawn.jar, Some(home), "deviation 3");
    assert_eq!(spawn.parent.as_deref(), Some(opener.as_str()));
    assert_eq!(spawn.group.as_deref(), Some(g.as_str()));
    app.duplicate_tab(&opener);
    let (spawn, _) = app.test_spawns.last().unwrap();
    assert_eq!(spawn.jar, Some(home), "a duplicate copies too");
    app.spawn_tab_at("https://dropped/", egui::Vec2::ZERO, Some(g.clone()), None);
    let (spawn, _) = app.test_spawns.last().unwrap();
    assert_eq!(spawn.jar, None, "a dropped page inherits");
}

#[test]
fn the_chip_shows_only_non_default_or_warning_states() {
    let mut app = App::default();
    app.ensure_default_jar();
    let work = add_jar(&mut app, "Work");
    let t = tab_in(&mut app, None);
    assert!(app.jar_chip(&t).is_none(), "default jar: no chip");
    app.set_jar_edge(&t, Some(work));
    let chip = app.jar_chip(&t).unwrap();
    assert_eq!((chip.name.to_string().as_str(), chip.following, chip.missing), ("Work", false, false));
    assert!(chip.view_ignores, "`sleeping_tab` is a Servo tab");
    assert_eq!(chip.tooltip.as_deref(), Some("This view does not use jars yet"));
    app.set_jar_edge(&t, None); // no group edge: back to the Default
    assert!(app.jar_chip(&t).is_none());
    app.set_jar_edge(&t, Some(work));
    app.remove_remotely_deleted(&work.to_string()); // a remote delete arrives before the tab's own field edit
    let chip = app.jar_chip(&t).unwrap();
    assert!(chip.missing);
    assert_eq!(chip.tooltip.as_deref(), Some("This tab's jar is gone or out of reach; it uses the Default jar"));
    assert!(app.jar_chip("nobody").is_none());
}

/// Fix batch A2: a new jar takes the first palette colour no jar uses, so a delete doesn't make the next jar repeat
/// one; with every colour taken it still gets one.
#[test]
fn a_new_jar_takes_a_colour_no_jar_uses() {
    use crate::theme::TAG_COLOR_PALETTE;
    let mut app = App::default();
    let colour = |app: &App, id| app.jar_defs()[&id].color.unwrap();
    let ids: Vec<_> = ["A", "B", "C"].iter().map(|n| add_jar(&mut app, n)).collect();
    assert_eq!(ids.iter().map(|&id| colour(&app, id)).collect::<Vec<_>>(), TAG_COLOR_PALETTE[..3]);
    app.close_node(&ids[1].to_string());
    let d = add_jar(&mut app, "D");
    assert_eq!(colour(&app, d), TAG_COLOR_PALETTE[1], "the freed colour");
    let distinct = |app: &App| app.jar_defs().values().map(|j| j.color.unwrap().to_array()).collect::<BTreeSet<_>>();
    assert_eq!(distinct(&app).len(), app.jar_defs().len(), "no colour repeats");
    while app.jar_defs().len() < TAG_COLOR_PALETTE.len() {
        add_jar(&mut app, "More");
    }
    assert_eq!(distinct(&app).len(), TAG_COLOR_PALETTE.len(), "every palette colour once");
    let extra = add_jar(&mut app, "Extra");
    assert!(TAG_COLOR_PALETTE.contains(&colour(&app, extra)), "all taken: still a palette colour");
}

/// The dot is outlined ("following") while the tab has no edge of its own: it inherits its group's.
#[test]
fn the_chip_follows_while_the_tab_has_no_edge_of_its_own() {
    let (mut app, _) = with_default();
    let (work, home) = (jar_at(&mut app, "Work", None), jar_at(&mut app, "Home", None));
    let g = group_in(&mut app, None);
    app.set_jar_edge(&g, Some(work));
    let t = tab_in(&mut app, Some(&g));
    assert_eq!(app.effective_jar(&t), Some((work, false)));
    assert!(app.jar_chip(&t).unwrap().following, "inherits its group's Work");
    app.set_jar_edge(&t, Some(home));
    assert!(!app.jar_chip(&t).unwrap().following, "its own edge: a plain dot");
    app.set_jar_edge(&t, None);
    assert!(app.jar_chip(&t).unwrap().following);
}

/// `SETJAR` (Task 8 review): makes a jar only for a focused tab, reuses one by name, and "default" picks Default.
#[test]
fn setjar_acts_only_on_a_focused_tab() {
    let mut app = App::default();
    let g = group_in(&mut app, None);
    app.focused = Some(g);
    assert!(!app.set_focused_tab_jar_named("Stray"));
    assert!(app.jar_defs().is_empty(), "a focused group makes no jar");
    app.focused = None;
    assert!(!app.set_focused_tab_jar_named("Stray"));
    assert!(app.jar_defs().is_empty());
    let t = tab_in(&mut app, None);
    app.focused = Some(t.clone());
    assert!(app.set_focused_tab_jar_named("Work"));
    let work = app.jar_named("Work").expect("made for the tab");
    assert_eq!(app.mirror.jar(&t), Some(work));
    assert!(app.set_focused_tab_jar_named("Work"));
    assert_eq!(app.jar_defs().len(), 1, "an existing name is reused");
    assert!(app.set_focused_tab_jar_named("DEFAULT"));
    assert_eq!(app.mirror.jar(&t), None);
}

/// Jar name labels: `SETJAR` takes a label first, then a stored name; a stored name several jars share picks the
/// lowest node id, and no jar is made.
#[test]
fn setjar_takes_a_label_or_an_ambiguous_stored_name() {
    let mut app = App::default();
    let (w1, w2, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    assert!(w1 < w2, "made in order");
    let t = tab_in(&mut app, None);
    app.focused = Some(t.clone());
    let l2 = app.jar_label(w2).to_string();
    assert_eq!(app.jar_named(&l2), Some(w2));
    assert!(app.set_focused_tab_jar_named(&l2));
    assert_eq!(app.mirror.jar(&t), Some(w2), "by label");
    assert!(app.set_focused_tab_jar_named("Work"));
    assert_eq!(app.mirror.jar(&t), Some(w1), "an ambiguous stored name: the lowest id");
    assert!(app.set_focused_tab_jar_named("Home"));
    assert_eq!(app.mirror.jar(&t), Some(home), "a unique name is its own label");
    assert_eq!(app.jar_defs().len(), 3, "no jar made");
    // A jar literally named like another's label: the label wins, as it is matched first.
    add_jar(&mut app, &l2);
    assert_eq!(app.jar_named(&l2), Some(w2));
}

#[test]
fn the_will_switch_tooltip_says_where_and_what_a_clear_loses() {
    let tip = |cleared, target: &str| super::will_switch_tooltip(cleared, target, true);
    assert_eq!(tip(false, "Work"), "Switches to Work when it is next out of sight; right-click for Switch Jar Now");
    assert_eq!(
        super::will_switch_tooltip(false, "Work", false),
        "Switches to Work when it is next out of sight",
        "no Switch Jar Now hint while a sleep, swap or earlier switch is on its way"
    );
    assert!(!super::will_switch_tooltip(true, "the empty jar", false).contains("Switch Jar Now"));
    let cleared = tip(true, "the empty jar");
    assert!(cleared.contains("signs out and restarts on the empty jar"), "{cleared}");
    assert!(cleared.contains("anything signed in meanwhile is not kept"), "{cleared}");
}

#[test]
fn the_confirm_names_the_jar_and_what_it_costs() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    let default = default_jar_id(&mut app);
    let t = tab_in(&mut app, None);
    app.set_jar_edge(&t, Some(work));
    let (title, detail, button) = app.jar_confirm_text(JarConfirm::Clear(work)).unwrap();
    assert_eq!((title.as_str(), button), ("Clear cookies in Work?", "Clear"));
    assert!(detail.starts_with("Tabs using it are signed out."), "{detail}");
    assert!(detail.contains("restart on the empty jar") && detail.contains("not kept"), "{detail}");
    let (title, _, _) = app.jar_confirm_text(JarConfirm::Clear(default)).unwrap();
    assert_eq!(title, "Clear cookies in Default (built-in)?");
    app.remove_remotely_deleted(&work.to_string());
    assert!(app.jar_confirm_text(JarConfirm::Ephemeral(work)).is_none(), "deleted elsewhere meanwhile: nothing to ask");
    assert!(app.jar_confirm_text(JarConfirm::Clear(work)).is_none());
}

/// A jar def named `name`, for tests that need chosen ids.
fn def(name: &str) -> super::JarDef {
    super::JarDef { name: name.into(), color: None, ephemeral: false, is_default: false }
}

fn id(counter: u64) -> NodeId {
    NodeId { actor: crate::store::model::ActorId(0xabcd), counter }
}

/// Jar name labels: a name no other jar has (trimmed, any case) is shown as stored; a shared one gets a marker.
#[test]
fn a_label_is_the_name_unless_another_jar_shares_it() {
    let mut defs = BTreeMap::new();
    defs.insert(id(1), def("Work"));
    defs.insert(id(2), def("Home"));
    let plain = |n, defs: &BTreeMap<NodeId, super::JarDef>| named_jar_label(id(n), defs).map(|l| l.to_string());
    assert_eq!(plain(1, &defs).as_deref(), Some("Work"));
    assert_eq!(jar_fragment(id(1), &defs), None);
    assert_eq!(named_jar_label(id(9), &defs), None, "no such jar");
    defs.insert(id(3), def(" work "));
    defs.insert(id(4), def("WORK"));
    let labels: Vec<String> = [1, 3, 4].map(|n| plain(n, &defs).unwrap()).to_vec();
    // The plain label shows the name by the display rule (trimmed), so " work " reads "work · #frag".
    for (n, (label, name)) in labels.iter().zip(["Work", "work", "WORK"]).enumerate() {
        let frag = label.strip_prefix(&format!("{name} · #")).unwrap_or_else(|| panic!("{n}: {label}"));
        assert_eq!(frag.len(), 4, "{label}");
        assert!(frag.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{label}");
    }
    assert_eq!(labels.iter().collect::<BTreeSet<_>>().len(), 3, "every clashing label differs: {labels:?}");
    assert_eq!(plain(2, &defs).as_deref(), Some("Home"), "an unrelated jar stays plain");
}

/// Name rules: clash detection compares clean names, so a hidden character cannot make two same-looking jars
/// look distinct; the plain label shows it as `<U+200B>`.
#[test]
fn a_hidden_character_does_not_make_a_name_distinct() {
    let mut defs = BTreeMap::new();
    defs.insert(id(1), def("Work"));
    defs.insert(id(2), def("Wo\u{200B}rk"));
    defs.insert(id(3), def("Work\u{00A0}"));
    for n in [1, 2, 3] {
        assert!(jar_fragment(id(n), &defs).is_some(), "{n} clashes");
    }
    let label = named_jar_label(id(2), &defs).unwrap().to_string();
    assert!(label.starts_with("Wo<U+200B>rk · #"), "{label}");
    assert_eq!(built_in().to_string(), "Default (built-in)");
}

/// Styled jar label markers: a label is a name and an optional marker, and its plain string is the text every view
/// showed before the split ("Work", "Work · #frag").
#[test]
fn a_label_is_a_name_and_an_optional_marker() {
    let mut defs = BTreeMap::new();
    defs.insert(id(1), def("Work"));
    defs.insert(id(2), def("Home"));
    let unique = named_jar_label(id(1), &defs).unwrap();
    assert_eq!(unique, JarLabel { name: "Work".into(), marker: None });
    assert_eq!(unique.to_string(), "Work");
    defs.insert(id(3), def("Work"));
    let frag = jar_fragment(id(1), &defs).unwrap();
    let clash = named_jar_label(id(1), &defs).unwrap();
    assert_eq!(clash, JarLabel { name: "Work".into(), marker: Some(JarMarker::Fragment(frag.clone())) });
    assert_eq!(clash.to_string(), format!("Work · #{frag}"));
    assert_eq!(built_in(), JarLabel { name: "Default".into(), marker: Some(JarMarker::BuiltIn) });
    assert_eq!(built_in().to_string(), "Default (built-in)");
}

/// A user jar named "Default" is shown as stored; the built-in jar (and a deleted jar's tabs, which use it) is
/// marked, and plain-text sites say "Default (built-in)".
#[test]
fn the_built_in_jar_is_marked_apart_from_a_user_jar_named_default() {
    let mut app = App::default();
    let default = default_jar_id(&mut app);
    let user = add_jar(&mut app, "Default");
    let user_label = app.jar_label(user);
    assert!(
        matches!(user_label.marker, Some(JarMarker::Fragment(_))),
        "a clash with the Default's name: {user_label:?}"
    );
    assert_eq!(app.jar_label(default), built_in());
    assert_eq!(app.jar_confirm_text(JarConfirm::Clear(default)).unwrap().0, "Clear cookies in Default (built-in)?");
    assert!(app.jar_confirm_text(JarConfirm::Clear(user)).unwrap().0.starts_with("Clear cookies in Default · #"));
    app.remove_remotely_deleted(&user.to_string());
    assert_eq!(app.jar_label(user), built_in(), "gone: its tabs use the Default jar");
}

/// Styled jar label markers: rich text draws a marker after its name in monospace, smaller, in the weak colour and
/// without the plain text's " · ", so a name typed to look like a marker never looks like one; the built-in jar's
/// name is italic too. The name keeps the fallback colour, so a button paints it in its own state's colour.
#[test]
fn a_marker_is_drawn_smaller_monospace_and_weak_after_its_name() {
    let font = egui::FontId::proportional(14.0);
    let weak = egui::Color32::from_gray(120);
    let marker_format = egui::TextFormat {
        font_id: egui::FontId::monospace(14.0 * super::MARKER_SCALE),
        color: weak,
        valign: egui::Align::Center,
        ..Default::default()
    };
    let parts = |job: &egui::text::LayoutJob| {
        job.sections
            .iter()
            .map(|s| (job.text[s.byte_range.start.0..s.byte_range.end.0].to_string(), s.format.clone()))
            .collect::<Vec<_>>()
    };
    let job = |label: &JarLabel| {
        let mut job = egui::text::LayoutJob::default();
        label.append_to(&mut job, font.clone(), weak);
        job
    };
    let name_format = egui::TextFormat::simple(font.clone(), egui::Color32::PLACEHOLDER);
    assert_eq!(parts(&job(&JarLabel::from("Work"))), [("Work".into(), name_format.clone())]);
    let typed = job(&JarLabel::from("Work · #3f2a"));
    assert_eq!(parts(&typed), [("Work · #3f2a".into(), name_format.clone())], "a typed look-alike is all name");
    let clash = JarLabel { name: "Work".into(), marker: Some(JarMarker::Fragment("3f2a".into())) };
    assert_eq!(clash.marker_text().as_deref(), Some("#3f2a"));
    assert_eq!(parts(&job(&clash)), [("Work".into(), name_format.clone()), (" #3f2a".into(), marker_format.clone())]);
    let built_in = built_in();
    assert_eq!(built_in.marker_text().as_deref(), Some("built-in"));
    let italic = egui::TextFormat { italics: true, ..name_format };
    assert_eq!(parts(&job(&built_in)), [("Default".into(), italic), (" built-in".into(), marker_format)]);
    assert_eq!(JarLabel::from("Default").marker_text(), None);
}

/// Tab view's jar dot hover: "Jar: " and the styled label, whether it follows its group, then the chip's warnings
/// with the click hint in place of the right-click one.
#[test]
fn the_tab_view_hover_styles_the_label() {
    let mut app = App::default();
    let (_, w2) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Work"));
    let t = tab_in(&mut app, None);
    app.set_jar_edge(&t, Some(w2));
    let chip = app.jar_chip(&t).unwrap();
    let font = egui::FontId::proportional(14.0);
    let weak = egui::Color32::from_gray(120);
    let job = chip.hover_job(font.clone(), weak);
    let mut want = egui::text::LayoutJob::default();
    want.append("Jar: ", 0.0, egui::TextFormat::simple(font.clone(), egui::Color32::PLACEHOLDER));
    chip.name.append_to(&mut want, font.clone(), weak);
    want.append("\nThis view does not use jars yet", 0.0, egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER));
    assert_eq!(job, want);
    let mut chip = chip;
    chip.following = true;
    chip.tooltip = Some(format!("Switches to Work; {}", super::SWITCH_NOW_HINT));
    let text = chip.hover_job(egui::FontId::proportional(14.0), weak).text;
    assert!(text.ends_with(" (follows its group)\nSwitches to Work; click to switch now"), "{text}");
}

/// Two ids whose 4-character fragments collide get longer ones, just long enough to differ; a third jar with the
/// same name whose 4 characters already differ keeps them.
#[test]
fn colliding_fragments_lengthen_just_enough() {
    let mut seen = std::collections::HashMap::new();
    let (a, b) =
        (0..).find_map(|n| seen.insert(id_digest(id(n))[..4].to_string(), id(n)).map(|other| (other, id(n)))).unwrap();
    let (da, db) = (id_digest(a), id_digest(b));
    let differ = (0..).find(|&i| da.as_bytes()[i] != db.as_bytes()[i]).unwrap();
    let c = (0..).map(id).find(|&c| c != a && c != b && id_digest(c)[..4] != da[..4]).unwrap();
    let mut defs = BTreeMap::new();
    for n in [a, b, c] {
        defs.insert(n, def("Work"));
    }
    assert_eq!(jar_fragment(a, &defs), Some(da[..=differ].to_string()));
    assert_eq!(jar_fragment(b, &defs), Some(db[..=differ].to_string()));
    assert_ne!(jar_fragment(a, &defs), jar_fragment(b, &defs));
    assert_eq!(jar_fragment(c, &defs), Some(id_digest(c)[..4].to_string()));
}

/// A label is derived from the jar's own id: adding or deleting a jar with another name never changes it, and
/// it is back to the plain name once its twin goes.
#[test]
fn a_label_is_stable_while_other_jars_come_and_go() {
    let mut app = App::default();
    let (w1, w2) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Work"));
    let label = |app: &App, j| app.jar_label(j).to_string();
    let (l1, l2) = (label(&app, w1), label(&app, w2));
    assert_ne!(l1, l2);
    let home = add_jar(&mut app, "Home");
    assert_eq!((label(&app, w1), label(&app, w2)), (l1.clone(), l2.clone()), "a third jar added elsewhere");
    app.close_node(&home.to_string());
    assert_eq!((label(&app, w1), label(&app, w2)), (l1.clone(), l2), "and deleted");
    app.close_node(&w2.to_string());
    assert_eq!(label(&app, w1), "Work", "no twin left: plain again");
    let w3 = add_jar(&mut app, "work");
    // The same marker when a twin comes back (longer only in the 1-in-65536 case that the two collide).
    assert!(label(&app, w1).starts_with(&l1), "{} vs {l1}", label(&app, w1));
    assert!(label(&app, w3).starts_with("work · #"));
}

/// The chip, its will-switch text and the confirm dialogs all name a clashing jar by its label.
#[test]
fn the_chip_and_confirms_show_the_label() {
    let mut app = App::default();
    let (_w1, w2) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Work"));
    let l2 = app.jar_label(w2);
    assert!(l2.to_string().starts_with("Work · #"), "{l2}");
    let t = tab_in(&mut app, None);
    app.set_jar_edge(&t, Some(w2));
    assert_eq!(app.jar_chip(&t).unwrap().name, l2);
    assert_eq!(app.jar_confirm_text(JarConfirm::Clear(w2)).unwrap().0, format!("Clear cookies in {l2}?"));
    assert_eq!(app.jar_confirm_text(JarConfirm::Ephemeral(w2)).unwrap().0, format!("Make {l2} ephemeral?"));
}

/// Task 6 review M3: "Switch Jar Now" waits out an engine swap or a sleep already on its way, since either
/// replaces the host anyway and a second hold would cancel the first.
#[test]
fn a_pending_sleep_or_hold_blocks_switch_jar_now() {
    let mut app = App::default();
    assert!(!app.host_change_pending("t"));
    app.pending_sleeps.insert(
        "t".into(),
        crate::sleep_state::PendingSleep {
            why: crate::sleep_state::SleepWhy::JarSwitch,
            sent: std::time::Instant::now(),
        },
    );
    assert!(app.host_change_pending("t"));
    app.pending_sleeps.clear();
    app.cookies.jar_pulls.insert("t".into(), std::time::Instant::now());
    let held = crate::tabs::Held::Snapshot(crate::process_manager::ViewKind::Cef, "http://x/".into());
    assert!(app.hold_for_cookies("t", held).is_none());
    assert!(app.host_change_pending("t"), "a swap or switch parked waiting for its cookies");
}

/// Name rules: a jar is made and renamed under the clean name; a rename with nothing left is ignored.
#[test]
fn a_jar_is_made_and_renamed_with_the_clean_name() {
    let mut app = App::default();
    let jar = add_jar(&mut app, " Wo\u{200B}rk\n");
    assert_eq!(app.jar_defs()[&jar].name, "Work");
    app.commit_rename(&jar.to_string(), "\u{00A0}Home\u{2028}Office");
    assert_eq!(app.jar_defs()[&jar].name, "Home Office");
    app.commit_rename(&jar.to_string(), "\u{200B}");
    assert_eq!(app.jar_defs()[&jar].name, "Home Office");
}

/// Name rules: `SETJAR` with nothing left of the name does nothing; a name that is clean only after cleaning
/// finds the jar it cleans to rather than making a twin.
#[test]
fn setjar_matches_and_makes_by_the_clean_name() {
    let mut app = App::default();
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    app.focused = Some(t.clone());
    assert!(!app.set_focused_tab_jar_named("\u{200B} "));
    assert!(app.jar_defs().is_empty());
    let work = add_jar(&mut app, "Work");
    assert!(app.set_focused_tab_jar_named("Wo\u{200B}rk"));
    assert_eq!(app.jar_defs().len(), 1, "no twin made");
    assert_eq!(app.mirror.jar(&t), Some(work));
    assert!(app.set_focused_tab_jar_named(" New\u{00A0}\u{00A0}one "));
    assert!(app.jar_defs().values().any(|d| d.name == "New one"));
}

/// Name rules: a label draws a hidden character as a placeholder in the marker style, between the plain name
/// parts and before a marker, and its hint names it.
#[test]
fn a_label_shows_a_hidden_character_in_the_marker_style() {
    let label = JarLabel { name: "Wo\u{200B}rk".into(), marker: Some(JarMarker::Fragment("3f2a".into())) };
    let mut job = egui::text::LayoutJob::default();
    label.append_to(&mut job, egui::FontId::proportional(14.0), egui::Color32::GRAY);
    assert_eq!(job.text, "Wo⟦200B⟧rk #3f2a");
    let parts: Vec<(String, bool)> = job
        .sections
        .iter()
        .map(|s| {
            let text = job.text[s.byte_range.start.0..s.byte_range.end.0].to_string();
            (text, s.format.font_id.family == egui::FontFamily::Monospace)
        })
        .collect();
    let want = [("Wo", false), ("⟦200B⟧", true), ("rk", false), (" #3f2a", true)];
    assert_eq!(parts, want.map(|(t, m)| (t.to_string(), m)));
    assert_eq!(label.hidden_hint().as_deref(), Some("Hidden characters: U+200B zero width space"));
    assert_eq!(built_in().hidden_hint(), None);
}

/// Name rules: a chip on a jar whose synced name holds a hidden character says so in its hover.
#[test]
fn a_jar_chip_hover_names_a_hidden_character() {
    let mut app = App::default();
    let t = crate::session_sync::tests::add(
        &mut app,
        crate::store::model::Kind::Tab,
        crate::session_sync::tests::sleeping_tab("https://a/", "A"),
    );
    let jar = add_jar(&mut app, "x");
    app.mirror.set(
        &jar.to_string(),
        crate::store::model::field::NAME,
        crate::store::model::FieldValue::Str("Wo\u{200B}rk".into()),
    );
    app.set_jar_edge(&t, Some(jar));
    let tip = app.jar_chip(&t).and_then(|c| c.tooltip).unwrap_or_default();
    assert!(tip.contains("Hidden characters: U+200B zero width space"), "{tip}");
}

#[test]
fn the_default_jar_is_made_once_at_the_top_level() {
    let mut app = App::default();
    assert_eq!(app.default_jar(), None);
    assert!(app.ensure_default_jar(), "made");
    let default = app.default_jar().expect("a Default");
    assert_eq!(app.mirror.group(&default.to_string()), None);
    assert_eq!(app.jar_defs()[&default].name, "Default");
    assert!(!app.ensure_default_jar(), "already there: nothing to do");
    assert_eq!(app.jar_defs().len(), 1);
}

#[test]
fn two_defaults_converge_on_the_lowest_id() {
    let mut app = App::default();
    let (a, b) = (add_jar(&mut app, "Default"), add_jar(&mut app, "Default"));
    for jar in [a, b] {
        app.edit_field(&jar.to_string(), field::IS_DEFAULT, FieldValue::Bool(true));
    }
    let (low, high) = (a.min(b), a.max(b));
    assert!(app.ensure_default_jar());
    assert_eq!(app.default_jar(), Some(low));
    let defs = app.jar_defs();
    assert!(!defs[&high].is_default);
    assert_eq!(defs[&high].name, "Default (old)");
    assert!(!app.ensure_default_jar(), "converged: a second pass writes nothing");
}

#[test]
fn the_default_jar_cannot_be_closed_and_an_unset_tab_resolves_to_it() {
    let mut app = App::default();
    app.ensure_default_jar();
    let default = app.default_jar().unwrap();
    app.close_node(&default.to_string());
    assert!(app.jar_exists(default), "still there");
    let tab = tab_in(&mut app, None);
    assert_eq!(app.effective_jar(&tab), Some((default, false)));
}

#[test]
fn duplicating_the_default_makes_an_ordinary_jar() {
    let mut app = App::default();
    app.ensure_default_jar();
    let default = app.default_jar().unwrap();
    app.duplicate_nodes(&[default.to_string()]);
    let defs = app.jar_defs();
    assert_eq!(defs.len(), 2);
    assert_eq!(defs.values().filter(|d| d.is_default).count(), 1);
    assert_eq!(app.default_jar(), Some(default));
}
