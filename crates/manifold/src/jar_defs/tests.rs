use super::{id_digest, jar_fragment, named_jar_label, JarConfirm, JarLabel, JarMarker};
use crate::card::DropEffect;
use crate::jar::{JarChoice, JarKey};
use crate::session_sync::tests::{add, add_jar, sleeping_tab};
use crate::store::model::{Kind, NodeId};
use crate::App;
use std::collections::{BTreeMap, BTreeSet};

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
fn jar_of(app: &App, tab: &str) -> JarChoice {
    app.tab_jar(tab).unwrap()
}

#[test]
fn a_new_tab_snapshots_the_nearest_group_default() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    let outer = group_in(&mut app, None);
    let inner = group_in(&mut app, Some(&outer));
    assert_eq!(app.new_tab_jar(Some(&inner)), JarChoice::default(), "no default anywhere: no jar");
    app.set_group_default_jar(&outer, work);
    assert_eq!(app.new_tab_jar(Some(&inner)), JarChoice { jar: Some(work), set: false });
    assert_eq!(app.new_tab_jar(None), JarChoice::default());
}

#[test]
fn a_cascade_reaches_unset_tabs_in_nested_groups_without_their_own_default() {
    let mut app = App::default();
    let (work, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let outer = group_in(&mut app, None);
    let plain = group_in(&mut app, Some(&outer)); // no default of its own
    let own = group_in(&mut app, Some(&outer));
    let a = tab_in(&mut app, Some(&outer));
    let b = tab_in(&mut app, Some(&plain));
    let c = tab_in(&mut app, Some(&own));
    // Set after `c` exists, so the cascade gives `c` its group's own jar.
    app.set_group_default_jar(&own, home);
    let picked = tab_in(&mut app, Some(&outer));
    app.set_tab_jar(&picked, None);
    assert_eq!(app.cascade_count(&outer, work), 2);
    assert_eq!(app.set_group_default_jar(&outer, work), 2);
    assert_eq!(jar_of(&app, &a), JarChoice { jar: Some(work), set: false });
    assert_eq!(jar_of(&app, &b), JarChoice { jar: Some(work), set: false });
    assert_eq!(jar_of(&app, &c).jar, Some(home), "a nested group with its own default is skipped");
    assert_eq!(jar_of(&app, &picked), JarChoice { jar: None, set: true }, "a hand-picked tab is skipped");
    app.clear_group_default_jar(&outer);
    assert_eq!(jar_of(&app, &a).jar, Some(work), "clearing the default leaves tabs alone");
    assert_eq!(app.new_tab_jar(Some(&outer)), JarChoice::default(), "but new tabs no longer inherit it");
}

#[test]
fn moving_an_unset_tab_re_snapshots_and_a_set_tab_keeps_its_jar() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    let g = group_in(&mut app, None);
    app.set_group_default_jar(&g, work);
    let unset = tab_in(&mut app, None);
    let set = tab_in(&mut app, None);
    app.set_tab_jar(&set, None);
    for t in [&unset, &set] {
        app.edit_group(t, Some(g.clone()));
        app.refollow(t);
    }
    assert_eq!(jar_of(&app, &unset).jar, Some(work));
    assert_eq!(jar_of(&app, &set), JarChoice { jar: None, set: true });
    app.edit_group(&unset, None);
    app.refollow(&unset);
    assert_eq!(jar_of(&app, &unset), JarChoice::default(), "moved out: no default, no jar");
}

#[test]
fn follow_group_jar_clears_set_and_re_snapshots() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    let g = group_in(&mut app, None);
    app.set_group_default_jar(&g, work);
    let t = tab_in(&mut app, Some(&g));
    app.set_tab_jar(&t, None);
    app.follow_group_jar(&t);
    assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: false });
}

#[test]
fn a_new_tab_inherits_and_an_opened_tab_copies_its_opener() {
    let mut app = App::default();
    let (work, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let g = group_in(&mut app, None);
    app.set_group_default_jar(&g, work);
    app.spawn_new_tab(None, Some(g.clone()), "https://new/");
    let (spawn, _) = app.test_spawns.last().unwrap();
    assert_eq!(spawn.jar, JarChoice { jar: Some(work), set: false });
    let opener = tab_in(&mut app, Some(&g));
    app.set_tab_jar(&opener, Some(home));
    app.spawn_tab_from(&opener, "https://opened/");
    let (spawn, _) = app.test_spawns.last().unwrap();
    assert_eq!(spawn.jar, JarChoice { jar: Some(home), set: true }, "deviation 3");
    assert_eq!(spawn.parent.as_deref(), Some(opener.as_str()));
    assert_eq!(spawn.group.as_deref(), Some(g.as_str()));
    app.duplicate_tab(&opener);
    let (spawn, _) = app.test_spawns.last().unwrap();
    assert_eq!(spawn.jar, JarChoice { jar: Some(home), set: true }, "a duplicate copies too");
    app.spawn_tab_at("https://dropped/", egui::Vec2::ZERO, Some(g.clone()), None);
    let (spawn, _) = app.test_spawns.last().unwrap();
    assert_eq!(spawn.jar, JarChoice { jar: Some(work), set: false }, "a dropped page inherits");
}

/// A merge re-snapshots only the members that moved: a tab already in the target keeps its jar, even one that
/// differs from what it would inherit now (spec: "Clear default" leaves existing tabs alone).
#[test]
fn a_merge_leaves_the_targets_own_tabs_alone() {
    let mut app = App::default();
    let (work, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let h = group_in(&mut app, None);
    let stayed = tab_in(&mut app, Some(&h));
    app.set_group_default_jar(&h, home);
    app.clear_group_default_jar(&h);
    let w = group_in(&mut app, None);
    let moved = tab_in(&mut app, Some(&w));
    app.set_group_default_jar(&w, work);
    app.apply_drop(&w, Some((DropEffect::Merge, Some(h.clone()))));
    assert_eq!(app.mirror.group(&moved), Some(h.as_str()));
    assert_eq!(jar_of(&app, &moved), JarChoice::default(), "a moved member re-snapshots: `h` has no default");
    assert_eq!(jar_of(&app, &stayed), JarChoice { jar: Some(home), set: false }, "a tab that never moved");
}

/// The real move paths, not `refollow` called by hand: each re-snapshots unset tabs and leaves set ones.
#[test]
fn every_move_between_groups_re_snapshots_unset_tabs() {
    let mut app = App::default();
    let (work, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let w = group_in(&mut app, None);
    app.set_group_default_jar(&w, work);
    let h = group_in(&mut app, None);
    app.set_group_default_jar(&h, home);
    let unset = tab_in(&mut app, None);
    let set = tab_in(&mut app, None);
    app.set_tab_jar(&set, None);
    let unset_jar = |app: &App| jar_of(app, &unset);

    // Join: dropped onto group card `w`.
    app.apply_drop(&unset, Some((DropEffect::Join, Some(w.clone()))));
    app.apply_drop(&set, Some((DropEffect::Join, Some(w.clone()))));
    assert_eq!(unset_jar(&app), JarChoice { jar: Some(work), set: false }, "join");
    assert_eq!(jar_of(&app, &set), JarChoice { jar: None, set: true }, "join leaves a set tab");

    // Eject: out of `w` to the top level, where nothing has a default.
    app.eject(&unset);
    assert_eq!(unset_jar(&app), JarChoice::default(), "eject");

    // Merge: `w` (holding the tab again) dropped onto `h`; its members now take `h`'s default.
    app.apply_drop(&unset, Some((DropEffect::Join, Some(w.clone()))));
    app.apply_drop(&w, Some((DropEffect::Merge, Some(h.clone()))));
    assert_eq!(app.mirror.group(&unset), Some(h.as_str()));
    assert_eq!(unset_jar(&app), JarChoice { jar: Some(home), set: false }, "merge");
    assert_eq!(jar_of(&app, &set), JarChoice { jar: None, set: true }, "merge leaves a set tab");

    // Dissolve: `inner` (no default) inside `h` is dissolved into `h`, then `h` itself to the top level.
    let inner = group_in(&mut app, Some(&h));
    app.edit_group(&unset, Some(inner.clone()));
    app.edit_jar(&unset, JarChoice { jar: Some(work), set: false }); // stale, so the dissolve must fix it
    app.dissolve_group(&inner);
    assert_eq!(unset_jar(&app), JarChoice { jar: Some(home), set: false }, "dissolve into a defaulted group");
    app.dissolve_group(&h);
    assert_eq!(unset_jar(&app), JarChoice::default(), "dissolve to the top level");

    // GroupWith: viewing group `level` (default Work), the tab, moved in by hand, is dropped onto `other`. The
    // new group has no default of its own, so the tab takes `level`'s.
    let level = group_in(&mut app, None);
    app.set_group_default_jar(&level, work);
    let other = tab_in(&mut app, Some(&level));
    app.edit_group(&unset, Some(level.clone()));
    app.current_group = Some(level.clone());
    app.apply_drop(&unset, Some((DropEffect::GroupWith, Some(other.clone()))));
    let made = app.mirror.group(&unset).unwrap().to_owned();
    assert_eq!(app.mirror.group(&made), Some(level.as_str()), "the new group sits inside `level`");
    assert_eq!(unset_jar(&app), JarChoice { jar: Some(work), set: false }, "group with");
}

#[test]
fn the_chip_shows_only_non_default_or_warning_states() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    let t = tab_in(&mut app, None);
    assert!(app.jar_chip(&t).is_none(), "default jar: no chip");
    app.set_tab_jar(&t, Some(work));
    let chip = app.jar_chip(&t).unwrap();
    assert_eq!((chip.name.to_string().as_str(), chip.following, chip.missing), ("Work", false, false));
    assert!(chip.view_ignores, "`sleeping_tab` is a Servo tab");
    assert_eq!(chip.tooltip.as_deref(), Some("This view does not use jars yet"));
    app.follow_group_jar(&t); // no group default: back to the default jar
    assert!(app.jar_chip(&t).is_none());
    app.set_tab_jar(&t, Some(work));
    app.remove_remotely_deleted(&work.to_string()); // a remote delete arrives before the tab's own field edit
    let chip = app.jar_chip(&t).unwrap();
    assert!(chip.missing);
    assert_eq!(chip.tooltip.as_deref(), Some("This tab's jar was deleted; it uses the default jar"));
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
    app.close_node_and_save(&ids[1].to_string());
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

/// Fix batch A4: the dot is outlined ("following") only while the tab's nearest group default is its jar. After
/// "Clear Default Jar" the tab keeps its jar, unset, but follows nothing, so the dot is plain.
#[test]
fn the_chip_follows_only_while_a_group_default_names_its_jar() {
    let mut app = App::default();
    let (work, home) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Home"));
    let g = group_in(&mut app, None);
    app.set_group_default_jar(&g, work);
    let t = tab_in(&mut app, Some(&g));
    app.refollow(&t);
    assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: false });
    assert!(app.jar_chip(&t).unwrap().following, "follows its group's Work");
    app.clear_group_default_jar(&g);
    assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: false }, "stored values unchanged");
    assert!(!app.jar_chip(&t).unwrap().following, "no group default: a plain dot");
    app.mirror.set(&g, crate::store::model::field::DEFAULT_JAR, crate::store::model::FieldValue::Id(Some(home))); // remote, uncascaded
    assert!(!app.jar_chip(&t).unwrap().following, "the group's default is another jar");
    let outer = group_in(&mut app, None);
    app.mirror.set(&g, crate::store::model::field::DEFAULT_JAR, crate::store::model::FieldValue::Id(None));
    app.edit_group(&g, Some(outer.clone()));
    app.mirror.set(&outer, crate::store::model::field::DEFAULT_JAR, crate::store::model::FieldValue::Id(Some(work)));
    assert!(app.jar_chip(&t).unwrap().following, "an outer group's default counts when it is the nearest");
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
    assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: true });
    assert!(app.set_focused_tab_jar_named("Work"));
    assert_eq!(app.jar_defs().len(), 1, "an existing name is reused");
    assert!(app.set_focused_tab_jar_named("DEFAULT"));
    assert_eq!(jar_of(&app, &t).jar, None);
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
    let l2 = app.jar_label(JarKey::Named(w2)).to_string();
    assert_eq!(app.jar_named(&l2), Some(w2));
    assert!(app.set_focused_tab_jar_named(&l2));
    assert_eq!(jar_of(&app, &t).jar, Some(w2), "by label");
    assert!(app.set_focused_tab_jar_named("Work"));
    assert_eq!(jar_of(&app, &t).jar, Some(w1), "an ambiguous stored name: the lowest id");
    assert!(app.set_focused_tab_jar_named("Home"));
    assert_eq!(jar_of(&app, &t).jar, Some(home), "a unique name is its own label");
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
fn jar_tab_count_counts_live_and_sleeping_tabs_by_resolved_jar() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    let (a, b) = (tab_in(&mut app, None), tab_in(&mut app, None));
    group_in(&mut app, None);
    app.set_tab_jar(&b, Some(work));
    assert_eq!((app.jar_tab_count(JarKey::Default), app.jar_tab_count(JarKey::Named(work))), (1, 1));
    app.set_tab_jar(&a, Some(work));
    assert_eq!((app.jar_tab_count(JarKey::Default), app.jar_tab_count(JarKey::Named(work))), (0, 2));
    app.remove_remotely_deleted(&work.to_string());
    assert_eq!(app.jar_tab_count(JarKey::Default), 2, "a missing jar's tabs use the default jar");
}

#[test]
fn the_confirm_names_the_jar_and_what_it_costs() {
    let mut app = App::default();
    let work = add_jar(&mut app, "Work");
    let t = tab_in(&mut app, None);
    app.set_tab_jar(&t, Some(work));
    let (title, detail, button) = app.jar_confirm_text(JarConfirm::Clear(JarKey::Named(work))).unwrap();
    assert_eq!((title.as_str(), button), ("Clear cookies in Work?", "Clear"));
    assert!(detail.starts_with("Tabs using it are signed out."), "{detail}");
    assert!(detail.contains("restart on the empty jar") && detail.contains("not kept"), "{detail}");
    let (title, _, _) = app.jar_confirm_text(JarConfirm::Clear(JarKey::Default)).unwrap();
    assert_eq!(title, "Clear cookies in Default (built-in)?");
    let (title, detail, button) = app.jar_confirm_text(JarConfirm::Delete(work)).unwrap();
    assert_eq!(
        (title.as_str(), detail.as_str(), button),
        ("Delete jar Work?", "1 tab uses this; it'll use Default (built-in).", "Delete")
    );
    tab_in(&mut app, None);
    let other = tab_in(&mut app, None);
    app.set_tab_jar(&other, Some(work));
    assert_eq!(
        app.jar_confirm_text(JarConfirm::Delete(work)).unwrap().1,
        "2 tabs use this; they'll use Default (built-in)."
    );
    app.remove_remotely_deleted(&work.to_string());
    assert!(app.jar_confirm_text(JarConfirm::Delete(work)).is_none(), "deleted elsewhere meanwhile: nothing to ask");
    assert!(app.jar_confirm_text(JarConfirm::Clear(JarKey::Named(work))).is_none());
}

/// A jar def named `name`, for tests that need chosen ids.
fn def(name: &str) -> super::JarDef {
    super::JarDef { name: name.into(), color: None, ephemeral: false }
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
    assert_eq!(JarLabel::built_in().to_string(), "Default (built-in)");
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
    assert_eq!(JarLabel::built_in(), JarLabel { name: "Default".into(), marker: Some(JarMarker::BuiltIn) });
    assert_eq!(JarLabel::built_in().to_string(), "Default (built-in)");
}

/// A user jar named "Default" is shown as stored; the built-in jar (and a deleted jar's tabs, which use it) is
/// marked, and plain-text sites say "Default (built-in)".
#[test]
fn the_built_in_jar_is_marked_apart_from_a_user_jar_named_default() {
    let mut app = App::default();
    let user = add_jar(&mut app, "Default");
    assert_eq!(app.jar_label(JarKey::Named(user)), JarLabel { name: "Default".into(), marker: None });
    assert_eq!(app.jar_label(JarKey::Default), JarLabel::built_in());
    assert_eq!(app.jar_confirm_text(JarConfirm::Clear(JarKey::Named(user))).unwrap().0, "Clear cookies in Default?");
    assert_eq!(
        app.jar_confirm_text(JarConfirm::Clear(JarKey::Default)).unwrap().0,
        "Clear cookies in Default (built-in)?"
    );
    app.remove_remotely_deleted(&user.to_string());
    assert_eq!(app.jar_label(JarKey::Named(user)), JarLabel::built_in(), "gone: its tabs use the built-in jar");
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
    let built_in = JarLabel::built_in();
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
    app.set_tab_jar(&t, Some(w2));
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
    let label = |app: &App, j| app.jar_label(JarKey::Named(j)).to_string();
    let (l1, l2) = (label(&app, w1), label(&app, w2));
    assert_ne!(l1, l2);
    let home = add_jar(&mut app, "Home");
    assert_eq!((label(&app, w1), label(&app, w2)), (l1.clone(), l2.clone()), "a third jar added elsewhere");
    app.close_node_and_save(&home.to_string());
    assert_eq!((label(&app, w1), label(&app, w2)), (l1.clone(), l2), "and deleted");
    app.close_node_and_save(&w2.to_string());
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
    let (w1, w2) = (add_jar(&mut app, "Work"), add_jar(&mut app, "Work"));
    let l2 = app.jar_label(JarKey::Named(w2));
    assert!(l2.to_string().starts_with("Work · #"), "{l2}");
    let t = tab_in(&mut app, None);
    app.set_tab_jar(&t, Some(w2));
    assert_eq!(app.jar_chip(&t).unwrap().name, l2);
    assert_eq!(
        app.jar_confirm_text(JarConfirm::Clear(JarKey::Named(w2))).unwrap().0,
        format!("Clear cookies in {l2}?")
    );
    assert_eq!(app.jar_confirm_text(JarConfirm::Delete(w2)).unwrap().0, format!("Delete jar {l2}?"));
    assert_eq!(app.jar_confirm_text(JarConfirm::Ephemeral(w2)).unwrap().0, format!("Make {l2} ephemeral?"));
    let l1 = app.jar_label(JarKey::Named(w1));
    assert_eq!(app.jar_confirm_text(JarConfirm::Delete(w1)).unwrap().0, format!("Delete jar {l1}?"));
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
    app.rename_jar(jar, "\u{00A0}Home\u{2028}Office");
    assert_eq!(app.jar_defs()[&jar].name, "Home Office");
    app.rename_jar(jar, "\u{200B}");
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
    assert_eq!(app.tab_jar(&t).map(|c| c.jar), Some(Some(work)));
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
    assert_eq!(JarLabel::built_in().hidden_hint(), None);
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
    app.set_tab_jar(&t, Some(jar));
    let tip = app.jar_chip(&t).and_then(|c| c.tooltip).unwrap_or_default();
    assert!(tip.contains("Hidden characters: U+200B zero width space"), "{tip}");
}
