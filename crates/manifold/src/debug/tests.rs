use super::verbs::{
    parse_add_file, parse_click, parse_move_node, parse_rename, parse_secs, parse_use_jar, parse_zoom_pct, split_id,
};
use super::*;
use crate::card_id::testutil::node;

fn parses(line: &str) -> bool {
    parse_line(line).is_some()
}

/// Fix batch A5: `EGUITEXT` types into the focused egui field (a jar name, a group rename) and `BACKSPACE` erases
/// there; `TYPE` still goes to the page.
#[test]
fn egui_typing_commands_parse() {
    assert!(parses("EGUITEXT Work Stuff"));
    assert!(!parses("EGUITEXT"), "text is required");
    assert!(parses("backspace"));
    assert!(parses("TYPE abc"));
}

#[test]
fn click_takes_an_optional_button_and_ctrl() {
    let click = |rest| parse_click(rest).map(|((x, y), button, ctrl)| (x, y, button, ctrl));
    assert_eq!(click("1 2"), Some((1.0, 2.0, MouseButton::Left, false)));
    assert_eq!(click("1 2 right"), Some((1.0, 2.0, MouseButton::Right, false)));
    assert_eq!(click("1 2 ctrl"), Some((1.0, 2.0, MouseButton::Left, true)));
    assert_eq!(click("1 2 right CTRL"), Some((1.0, 2.0, MouseButton::Right, true)));
    assert_eq!(click("1"), None);
    assert!(parses("click 1 2 right CTRL") && !parses("CLICK 1"));
}

#[test]
fn wait_seconds_must_be_finite_and_bounded() {
    assert_eq!(parse_secs("1.5"), Some(std::time::Duration::from_millis(1500)));
    assert_eq!(parse_secs("0"), Some(std::time::Duration::ZERO));
    for bad in ["inf", "-inf", "NaN", "1e30", "-1", "86401", "x", ""] {
        assert_eq!(parse_secs(bad), None, "{bad}");
    }
    assert!(!parses("SLEEP inf"));
    assert!(!parses("WAITTABS 1e30"));
    assert!(!parses("WAITIDLE 1e30"));
    assert!(parses("WAITINDEX") && parses("WAITINDEX 5"));
    let waits = |line| match parse_line(line) {
        Some(Line::Reader(ReaderOp::WaitTabs(d) | ReaderOp::WaitIdle(d))) => Some(d.as_secs()),
        _ => None,
    };
    assert_eq!(waits("WAITTABS"), Some(60));
    assert_eq!(waits("WAITIDLE"), Some(60));
    assert_eq!(waits("WAITIDLE 5"), Some(5));
}

/// Reader-thread verbs block the stdin reader; every other verb is a UI-thread job, and only the mouse verbs are
/// pointer input.
#[test]
fn rows_are_reader_ops_or_jobs_and_only_mouse_rows_are_pointer() {
    for line in ["WAITTABS", "WAITIDLE", "SLEEP 1", "SCREENSHOT x.png"] {
        assert!(matches!(parse_line(line), Some(Line::Reader(_))), "{line}");
    }
    for line in ["ENTER", "QUIT", "HELP", "?"] {
        assert!(matches!(parse_line(line), Some(Line::Run(job)) if !job.pointer), "{line}");
    }
    let pointer: Vec<&str> = VERBS.iter().filter(|v| v.pointer).map(Verb::word).collect();
    assert_eq!(pointer, ["MOUSEMOVE", "MOUSEDOWN", "MOUSEUP", "CLICK", "DOUBLECLICK", "TRIPLECLICK", "SCROLL"]);
}

/// Every verb has a sample line that parses, and every sampled verb is in the table.
#[test]
fn every_verb_has_a_parsing_sample() {
    const SAMPLES: &[&str] = &[
        "MOUSEMOVE 1 2",
        "MOUSEDOWN 1 2",
        "MOUSEUP 1 2 right",
        "CLICK 1 2",
        "DOUBLECLICK 1 2",
        "TRIPLECLICK 1 2",
        "SCROLL 1 2 0 -3",
        "ADDRESSBAR example.com",
        "ENTER",
        "ESCAPE",
        "CONFIRM destructive",
        "BACKSPACE",
        "EGUITEXT Work",
        "ARROWRIGHT",
        "ARROWLEFT",
        "SPACEKEY",
        "DIGITKEY 3",
        "ARROWDOWN",
        "ARROWUP",
        "SHIFTF10",
        "TABKEY shift",
        "UNDOKEY shift",
        "GRAPHLEFT",
        "GRAPHRIGHT",
        "GRAPHDOWN",
        "GRAPHOPEN",
        "TYPE abc",
        "KEYPAGE Enter",
        "REOPENTAB",
        "REOPENNODE 0000000000000001-1",
        "INSTALLVIEW cef",
        "VIEWSTATE cef",
        "ZOOMIN",
        "ZOOMOUT",
        "ZOOMRESET",
        "GRAPHZOOM 60",
        "MARK phase one",
        "OPENDEVTOOLS",
        "NEWTAB",
        "CLOSETAB",
        "FOCUSADDRESS",
        "NEXTTAB",
        "PREVTAB",
        "OPENFINDBAR",
        "FINDTEXT needle",
        "CLOSEFINDBAR",
        r"SCREENSHOT C:\x.png",
        "RESIZE 800 600",
        "SURFACE lost",
        "QUIT",
        "QUICKSWITCHOPEN",
        "QUICKSWITCHTEXT abc",
        "QUICKSWITCHENTER",
        "QUICKSWITCHESCAPE",
        "SESSIONNAMETEXT work",
        "SAVESESSIONAS work",
        "OPENSESSION work",
        "GRAPHFILTERTEXT abc",
        "HISTORYFILTERTEXT abc",
        "BOOKMARKSFILTERTEXT abc",
        "TOGGLEVIEW",
        "TOGGLESETTINGSPAGE",
        "CLEARTOASTS",
        r"EXPORTSESSION C:\x.json",
        r"EXPORTSTATE C:\x.json",
        r"IMPORTSESSION C:\x.json",
        r"EXPORTBOOKMARKS C:\x.json",
        r"IMPORTBOOKMARKS C:\x.json",
        "CLOSEALLLEVEL",
        "CONFIRMIMPORT",
        "CANCELIMPORT",
        "RENAMEGROUP",
        "BOOKMARKFOLDERTEXT Reading",
        "DISMISSPEEK",
        r"EXPORTGROUP C:\x.json",
        r"IMPORTGROUP C:\x.json",
        "DUPLICATETAB",
        "DUPLICATENODE 0000000000000001-1",
        r"PRINTTOPDF C:\x.pdf",
        "TOGGLEPIN",
        "SELECTCARDS 0000000000000001-1 0000000000000001-2",
        "LABEL urgent",
        "LABELSELECTED urgent on",
        "LABELRENAME urgent now",
        "LABELDELETE now",
        "RELOAD",
        "GOBACK",
        "GOFORWARD",
        "NEWGROUP",
        "NEWNOTE",
        "NEWJSON",
        "NEWJSON 10 20",
        "DETACHJSON 0000000000000001-4",
        "EXPORTJSON 0000000000000001-4 out.json",
        "JSONEDIT insert /list 0 [1,2]",
        r"ADDFILE C:\x.json 10 20",
        r"ADDDOWNLOAD C:\x.zip",
        "WAITTABS 5",
        "WAITIDLE 5",
        "WAITINDEX 5",
        "SUGGEST",
        "TIDY",
        "ACCEPTSUGGESTION 0",
        "ACCEPTSUGGESTIONLABEL 0",
        "SLEEP 0.5",
        "TOGGLESHORTCUTSHELP",
        "TOGGLEPOPUP history",
        "TOGGLEPOPUP back",
        "MENUPAGE jar",
        "SPLIT cef",
        "CLOSESPLIT",
        "CLOSENODE 0000000000000001-3",
        "MOVETOGROUP 0000000000000001-1 0000000000000001-3",
        "LINK 0000000000000001-1 0000000000000001-2",
        "UNLINK 0000000000000001-1 0000000000000001-2",
        "OPENEDFROM 0000000000000001-1 0000000000000001-2",
        "SETNOTETEXT 0000000000000001-1 hello",
        "MOVENODE 0000000000000001-1 10 -20",
        "SLEEPNODE 0000000000000001-4",
        r"TEXTOF 0000000000000001-4 C:\x.txt",
        "RESTARTVIEW 0000000000000001-4",
        "ENDVIEW 0000000000000001-4",
        "IDLESWEEP 30",
        "SETJAR default",
        "SWITCHJARNOW",
        "CLEARJAR Work",
        "NEWJAR Work",
        "USEJAR 0000000000000001-4 none",
        "JAREPHEMERAL 0000000000000001-5 on",
        "JSONSELECT /a/0",
        "JSONTOGGLE",
        "JSONMORE /a",
        "JSONRELAYOUT",
        "JSONSEARCH abc",
        "TRIGGERFIRSTLAUNCHDEMO",
        "HELP",
    ];
    let verb = |line: &str| line.split_whitespace().next().unwrap().to_ascii_uppercase();
    for sample in SAMPLES {
        assert!(parses(sample), "sample {sample:?} parses");
        assert!(VERBS.iter().any(|v| v.word() == verb(sample)), "{sample:?} is in the table");
    }
    for v in VERBS {
        assert!(SAMPLES.iter().any(|s| verb(s) == v.word()), "{} has a sample", v.word());
    }
    for (i, v) in VERBS.iter().enumerate() {
        assert!(
            v.word().chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "{}: verbs are upper case",
            v.usage
        );
        assert!(VERBS[..i].iter().all(|w| w.word() != v.word()), "{} is listed twice", v.word());
    }
    assert!(parses("?"));
}

/// `LINK`, `UNLINK` and `OPENEDFROM` each take two card ids.
#[test]
fn link_unlink_and_openedfrom_take_two_ids() {
    for two in [
        "LINK 0000000000000001-1 0000000000000001-2",
        "UNLINK 0000000000000001-1 0000000000000001-2",
        "OPENEDFROM 0000000000000001-6 0000000000000001-7",
    ] {
        assert!(parses(two), "{two}");
    }
    for one in [
        "LINK 0000000000000001-1",
        "UNLINK 0000000000000001-1",
        "OPENEDFROM 0000000000000001-6",
        "MOVETOGROUP 0000000000000001-1",
    ] {
        assert!(!parses(one), "{one}: both ids are required");
    }
}

/// `MOVETOGROUP` takes two ids and moves only where the node menu's "Move into" would offer.
#[test]
fn move_to_group_moves_only_into_an_offered_sibling() {
    assert!(parse_line("MOVETOGROUP 0000000000000001-1 0000000000000001-3").is_some());
    assert!(parse_line("MOVETOGROUP 0000000000000001-1").is_none(), "both ids are required");
    let mut app = App::default();
    let (g, h, note) = (app.alloc_node_id(), app.alloc_node_id(), app.alloc_node_id());
    app.insert_group_at(g, None);
    app.insert_group_at(h, None);
    app.insert_note_at(note, None);
    assert!(!app.debug_move_to_group(note, note), "not a group");
    assert!(!app.debug_move_to_group(note, node(9999)));
    assert!(app.debug_move_to_group(note, g));
    assert_eq!(app.doc.mirror.group(note), Some(g));
    assert!(!app.debug_move_to_group(note, h), "h is no longer a sibling of the note");
    assert_eq!(app.doc.mirror.group(note), Some(g));
    assert!(app.debug_move_to_group(g, h), "a group nests, as the menu row does");
    assert_eq!(app.doc.mirror.group(g), Some(h));
}

/// `CLOSENODE id` closes any node by id, as its card's Close does, and saves.
#[test]
fn close_node_takes_any_node() {
    assert!(parse_line("CLOSENODE 0000000000000001-1").is_some());
    assert!(parse_line("CLOSENODE").is_none(), "the id is required");
    let mut app = App::default();
    let (g, note) = (app.alloc_node_id(), app.alloc_node_id());
    app.insert_group_at(g, None);
    app.insert_note_at(note, None);
    assert!(app.debug_close_node(note));
    assert!(app.debug_close_node(g), "a group closes too");
    assert!(!app.nodes.contains_key(&note) && !app.nodes.contains_key(&g));
    assert!(!app.debug_close_node(node(9999)));
}

/// `REOPENNODE id` reopens a closed node by id, and `EXPORTSTATE` lists the Recently Closed history.
#[test]
fn reopennode_reopens_by_id_and_exportstate_lists_the_history() {
    assert!(parse_line("REOPENNODE 0000000000000001-1").is_some());
    assert!(parse_line("REOPENNODE").is_none(), "the id is required");
    let path = crate::store::testutil::temp_dir("debug-reopennode").join("session.mnf");
    let mut app = crate::session_sync::tests::app_on(&path, 1);
    let g = crate::session_sync::tests::add_group(&mut app, "G");
    assert!(app.save_session());
    assert!(app.debug_close_node(g));
    assert_eq!(app.export_state()["closed"], serde_json::json!([g.to_string()]));
    app.reopen_node(g);
    assert!(app.nodes.contains_key(&g));
    assert_eq!(app.export_state()["closed"], serde_json::json!([]));
}

/// The three edits reach the document through their writers: the rename's clean name (groups only), the note's
/// typed text at the blur (beating a remote text held meanwhile, as typing does), and the released card's pos.
#[test]
fn name_text_and_pos_commands_write_the_document() {
    use crate::session_sync::tests::{add, add_group, app_on, remote_set, stored};
    use crate::store::model::{field, ActorId, FieldValue, Kind};
    let path = crate::store::testutil::temp_dir("debug-edits").join("session.mnf");
    let mut app = app_on(&path, 1);
    let g = add_group(&mut app, "G");
    let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
    assert!(app.save_session());

    app.commit_rename(g, " Errands\u{200B} ");
    assert!(app.save_session());
    assert_eq!(stored(&app, g, field::NAME), FieldValue::Str("Errands".into()));
    app.commit_rename(note, "Not a group");
    assert_eq!(app.doc.mirror.name(note), "Not a group", "any node takes a rename");

    assert!(app.debug_set_note_text(note, "mine".into()));
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("mine".into()), "the blur saved it");
    assert!(!app.nodes[&note].as_note().unwrap().editing);
    // A remote text held while typed in loses to what is typed.
    let mut other = crate::store::journal::Journal::open(&path, ActorId(2)).unwrap();
    app.nodes.get_mut(&note).unwrap().as_note_mut().unwrap().editing = true;
    remote_set(&mut other, note, field::TEXT, FieldValue::Str("theirs".into()));
    assert!(app.save_session());
    assert!(app.doc.writes.held_text.contains_key(&note), "held");
    assert!(app.debug_set_note_text(note, "mine again".into()));
    assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("mine again".into()));
    assert_eq!(app.doc.mirror.text(note), "mine again");
    assert!(!app.debug_set_note_text(g, "x".into()), "not a note");

    assert!(app.debug_move_node(note, egui::vec2(12.0, -34.0)));
    assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(Some((12.0, -34.0))));
    assert_eq!(app.layout_pos(note), egui::vec2(12.0, -34.0), "the card is where it was dropped");
    let anchor = crate::store::model::Anchor { pos: [12.0, -34.0], group: app.doc.mirror.group(note), explicit: false };
    assert_eq!(app.doc.mirror.anchor(note), Some(anchor), "MOVENODE drops as a drag release does");
    assert!(!app.debug_move_node(node(9999), egui::vec2(1.0, 1.0)), "no such node");
}

#[test]
fn name_text_and_pos_commands_parse() {
    let (g, n) = (node(3), node(1));
    assert_eq!(parse_rename(""), Some(None));
    assert_eq!(parse_rename(&format!("{g}  Reading list")), Some(Some((g, "Reading list".to_string()))));
    assert_eq!(parse_rename(&g.to_string()), None, "a name is required with an id");
    assert_eq!(parse_rename("g1 Reading list"), None, "a word that is no node id is a usage error");
    assert!(parses("RENAMEGROUP") && parses(&format!("RENAMEGROUP {g} x")) && !parses(&format!("RENAMEGROUP {g}")));
    assert_eq!(split_id(&format!("{n} two  words")), Some((n, "two  words")));
    assert_eq!(split_id(&n.to_string()), Some((n, "")));
    assert_eq!(split_id("n1 two  words"), None);
    assert!(parses(&format!("SETNOTETEXT {n} two  words")) && parses(&format!("SETNOTETEXT {n}")));
    assert!(!parses("SETNOTETEXT"), "an id is required");
    assert!(!parses("SETNOTETEXT n1 text") && !parses("CLOSENODE n1"), "a word that is no node id is a usage error");
    assert_eq!(parse_move_node(&format!("{n} 12 -34.5")), Some((n, 12.0, -34.5)));
    for bad in ["12", "inf 0", "NaN 0", "1 2 3"] {
        assert!(parse_move_node(&format!("{n} {bad}")).is_none(), "{bad}");
    }
    assert!(parse_move_node("").is_none() && parse_move_node("n1 1 2").is_none());
    assert_eq!(parse_zoom_pct("62.5"), Some(62.5));
    for bad in ["", "0", "-5", "inf", "x"] {
        assert!(parse_zoom_pct(bad).is_none(), "{bad}");
    }
}

#[test]
fn jar_commands_parse() {
    for ok in ["SETJAR Work", "SETJAR default", "SWITCHJARNOW", "CLEARJAR Work", "NEWJAR Work Stuff"] {
        assert!(parses(ok), "{ok}");
    }
    let (tab, jar) = (node(2), node(5));
    for bad in ["SETJAR", "CLEARJAR", "NEWJAR", "USEJAR 1-2", "JAREPHEMERAL 1-5 maybe", "JAREPHEMERAL 1-5"] {
        assert!(!parses(bad), "{bad}: a jar name or both words are required");
    }
    assert!(!parses(&format!("JAREPHEMERAL {jar} maybe")) && !parses(&format!("JAREPHEMERAL {jar}")));
    assert_eq!(parse_use_jar(&format!("{tab} {jar}")), Some((tab, Some(jar))));
    assert_eq!(parse_use_jar(&format!("{tab} none")), Some((tab, None)));
    assert_eq!(parse_use_jar(&tab.to_string()), None);
    assert_eq!(parse_use_jar(&format!("{tab} Work")), None, "a jar word that is no node id is a usage error");
    assert!(parses(&format!("JAREPHEMERAL {jar} on")) && parses(&format!("JAREPHEMERAL {jar} off")));
}

#[test]
fn parses_json_level_commands_with_an_optional_pointer() {
    for ok in ["JSONSELECT /users/0", "JSONTOGGLE", "JSONSEARCH two words", "JSONRELAYOUT"] {
        assert!(parses(ok), "{ok}");
    }
}

#[test]
fn parses_view_install_commands() {
    assert!(parses("INSTALLVIEW cef") && parses("VIEWSTATE CEF"));
    assert!(!parses("INSTALLVIEW nope"));
}

#[test]
fn parses_addfile_with_an_optional_trailing_point() {
    assert_eq!(parse_add_file("C:\\a b\\c.png"), ("C:\\a b\\c.png".to_string(), None));
    assert_eq!(parse_add_file("C:\\a b\\c.png 10 20.5"), ("C:\\a b\\c.png".to_string(), Some((10.0, 20.5))));
    assert!(!parses("ADDFILE"));
}
