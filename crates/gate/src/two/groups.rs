//! Scenarios 1, 2 and 8 to 12: the shared document. Instances A and B are up; nothing navigates.

use serde_json::{json, Value};

use super::{observe, require, Step, Two, Who, WAIT_FILE};
use crate::instance::wait_file;
use crate::session::{self, show_group, Field};
use crate::state::{GROUP, NOTE, TAB};

pub(super) fn s1(t: &mut Two) -> Step {
    t.scenario("s1-import", 1, "import + note", |t| {
        let pair = json!({"nodes": [
            {"id": "g", "type": GROUP, "name": "Pair"},
            {"id": "a", "type": TAB, "url": "manifold://welcome", "view_kind": "webview2", "pos": [-150.0, 0.0], "group": "g"},
            {"id": "b", "type": TAB, "url": "manifold://welcome", "view_kind": "webview2", "pos": [150.0, 0.0], "group": "g"}
        ]});
        let path = t.write_json("pair.json", &pair)?;
        t.send(Who::A, &format!("IMPORTGROUP {}", path.display()))?;
        t.send(Who::A, "WAITTABS")?;
        t.send(Who::B, "NEWNOTE")?;
        let doc = t.wait_agree("scenario 1: A and B agree on the shared nodes", "1")?;
        let (notes, tabs) = (session::count_kind(&doc, NOTE), session::count_kind(&doc, TAB));
        observe("scenario 1: the document has at least 1 note", notes >= 1, &format!("{notes} notes"));
        observe("scenario 1: the document has at least 2 tabs", tabs >= 2, &format!("{tabs} tabs"));
        Ok(String::new())
    })
}

pub(super) fn s2(t: &mut Two) -> Step {
    t.scenario("s2-compaction", 2, "compaction mid-run", |t| {
        // 200 notes of 500 chars each: ~100 KB of ops, past COMPACT_MIN (64 KiB) and twice a snapshot this small, so
        // A's save after the import compacts.
        let text = "x".repeat(500);
        let mut nodes = vec![json!({"id": "biggroup", "type": GROUP, "name": "Big"})];
        nodes.extend(
            (1..=200).map(|i| json!({"id": format!("note{i}"), "type": NOTE, "text": text, "group": "biggroup"})),
        );
        let path = t.write_json("big.json", &json!({ "nodes": nodes }))?;
        t.send(Who::A, &format!("IMPORTGROUP {}", path.display()))?;
        t.send(Who::A, "WAITTABS")?;
        // B edits only once A has compacted (the fresh profile had no `.prev`), so the edit lands on the rewritten file.
        let prev = t.profile.join("session.mnf.prev");
        require(
            "scenario 2: A's save after the import compacts the store (session.mnf.prev appears)",
            wait_file(&prev, WAIT_FILE).is_ok(),
            &format!("{} never appeared", prev.display()),
        )?;
        t.send(Who::B, "NEWNOTE")?;
        let doc = t.wait_agree("scenario 2: A and B agree on the shared nodes", "2")?;
        // A floor, not exact: the 200 imported notes plus B's notes.
        let notes = session::count_kind(&doc, NOTE);
        observe("scenario 2: the document has at least 201 notes", notes >= 201, &format!("{notes} notes"));
        Ok(String::new())
    })
}

pub(super) fn s8(t: &mut Two) -> Step {
    t.scenario("s8-move-into-group", 8, "a move into a group reaches the other instance", |t| {
        let g = t.new_node("NEWGROUP", "scenario 8: A's NEWGROUP adds exactly one node", "s8g")?;
        let n = t.new_node("NEWNOTE", "scenario 8: A's NEWNOTE adds exactly one node", "s8n")?;
        t.send(Who::A, &format!("MOVETOGROUP {n} {g}"))?;
        let doc = t.wait_agree("scenario 8: A and B agree on the shared nodes", "8")?;
        let got = session::group_of(&doc, &n);
        observe(
            "scenario 8: the document puts the note in the group",
            got.as_ref().is_some_and(|x| x.as_deref() == Some(g.as_str())),
            &format!("{n} is in {}, not {g}", show_group(&got)),
        );
        t.wait_field(Field::Group, "scenario 8: A, B and the document agree on the groups", "s8", &[&n, &g])?;
        Ok(String::new())
    })
}

/// "<id> in A's/B's group" for the ok-line, from where the document put it.
fn in_group(got: &Option<Option<String>>, a: &str) -> &'static str {
    if got.as_ref().is_some_and(|x| x.as_deref() == Some(a)) {
        "A's"
    } else {
        "B's"
    }
}

fn is_one_of(got: &Option<Option<String>>, a: &str, b: &str) -> bool {
    got.as_ref().is_some_and(|x| x.as_deref() == Some(a) || x.as_deref() == Some(b))
}

pub(super) fn s9(t: &mut Two) -> Step {
    t.scenario("s9-two-moves", 9, "both move one node into different groups", |t| {
        let ga = t.new_node("NEWGROUP", "scenario 9: A's first NEWGROUP adds exactly one node", "s9ga")?;
        let gb = t.new_node("NEWGROUP", "scenario 9: A's second NEWGROUP adds exactly one node", "s9gb")?;
        let n = t.new_node("NEWNOTE", "scenario 9: A's NEWNOTE adds exactly one node", "s9n")?;
        // B needs all three before it can offer the move.
        let (_, before) = t.wait_field(
            Field::Group,
            "scenario 9: A, B and the document agree on the groups before the moves",
            "s9-setup",
            &[&ga, &gb, &n],
        )?;
        // Back to back, so each move is likely made before that instance picks up the other's.
        t.send(Who::A, &format!("MOVETOGROUP {n} {ga}"))?;
        t.send(Who::B, &format!("MOVETOGROUP {n} {gb}"))?;
        t.show_local(Field::Group, "s9", "moves", &[&n])?;
        t.wait_agree("scenario 9: A and B agree on the shared nodes after both moves", "9")?;
        let (_, doc) = t.wait_field(
            Field::Group,
            "scenario 9: A, B and the document agree on the groups after both moves",
            "s9",
            &[&n, &ga, &gb],
        )?;
        let got = session::group_of(&doc, &n);
        observe(
            "scenario 9: the note ends in A's or B's group",
            is_one_of(&got, &ga, &gb),
            &format!("{n} ended in {}, neither {ga} (A's) nor {gb} (B's)", show_group(&got)),
        );
        let lost = session::lost_ids(&before, &doc);
        observe("scenario 9: no node is lost", lost.is_empty(), &format!("lost: {lost:?}"));
        let cycle = session::no_cycle(&doc);
        observe("scenario 9: no group cycle", cycle.is_ok(), cycle.as_ref().err().map_or("", String::as_str));
        Ok(format!(" ({n} in {} group)", in_group(&got, &ga)))
    })
}

pub(super) fn s10(t: &mut Two) -> Step {
    t.scenario("s10-cycle", 10, "A moves G into H while B moves H into G", |t| {
        let g = t.new_node("NEWGROUP", "scenario 10: A's first NEWGROUP adds exactly one node", "s10g")?;
        let h = t.new_node("NEWGROUP", "scenario 10: A's second NEWGROUP adds exactly one node", "s10h")?;
        let (_, before) = t.wait_field(
            Field::Group,
            "scenario 10: A, B and the document agree on the groups before the moves",
            "s10-setup",
            &[&g, &h],
        )?;
        t.send(Who::A, &format!("MOVETOGROUP {g} {h}"))?;
        t.send(Who::B, &format!("MOVETOGROUP {h} {g}"))?;
        t.show_local(Field::Group, "s10", "moves", &[&g, &h])?;
        t.wait_agree("scenario 10: A and B agree on the shared nodes after both moves", "10")?;
        let (_, doc) = t.wait_field(
            Field::Group,
            "scenario 10: A, B and the document agree on the groups after both moves",
            "s10",
            &[&g, &h],
        )?;
        let lost = session::lost_ids(&before, &doc);
        observe("scenario 10: no node is lost", lost.is_empty(), &format!("lost: {lost:?}"));
        let cycle = session::no_cycle(&doc);
        observe("scenario 10: no group cycle", cycle.is_ok(), cycle.as_ref().err().map_or("", String::as_str));
        let (gg, hg) = (show_group(&session::group_of(&doc, &g)), show_group(&session::group_of(&doc, &h)));
        Ok(format!(" (G in {gg}, H in {hg})"))
    })
}

/// The JSON text of an EXPORTSTATE value, as the script's `shown` printed it ("<absent>" if missing).
fn shown(v: &Option<Value>) -> String {
    v.as_ref().map_or_else(|| "<absent>".to_string(), Value::to_string)
}

pub(super) fn s11(t: &mut Two) -> Step {
    t.scenario("s11-rename", 11, "a group rename reaches the other instance", |t| {
        let g = t.new_node("NEWGROUP", "scenario 11: A's NEWGROUP adds exactly one node", "s11g")?;
        // B needs the group before it can rename it.
        t.wait_field(
            Field::Name,
            "scenario 11: A, B and the document agree on the new group's name",
            "s11-setup",
            &[&g],
        )?;
        t.send(Who::A, &format!("RENAMEGROUP {g} Renamed by A"))?;
        t.wait_agree("scenario 11: A and B agree on the shared nodes after A's rename", "11")?;
        let (a, _) = t.wait_field(
            Field::Name,
            "scenario 11: A, B and the document agree on the name after A's rename",
            "s11",
            &[&g],
        )?;
        let name = session::field_of(&a, Field::Name, &g);
        observe(
            "scenario 11: the group has A's name",
            name == Some(json!("Renamed by A")),
            &format!("{g} is named {}, not A's rename", shown(&name)),
        );
        // Back to back, so each rename is likely made before that instance picks up the other's.
        t.send(Who::A, &format!("RENAMEGROUP {g} Both A"))?;
        t.send(Who::B, &format!("RENAMEGROUP {g} Both B"))?;
        t.show_local(Field::Name, "s11c", "edits", &[&g])?;
        t.wait_agree("scenario 11: A and B agree on the shared nodes after both renames", "11c")?;
        let (a, _) = t.wait_field(
            Field::Name,
            "scenario 11: A, B and the document agree on the name after both renames",
            "s11c",
            &[&g],
        )?;
        let got = session::field_of(&a, Field::Name, &g);
        observe(
            "scenario 11: the group ends with A's or B's name",
            got == Some(json!("Both A")) || got == Some(json!("Both B")),
            &format!("{g} ended named {}, neither A's nor B's rename", shown(&got)),
        );
        Ok(format!(" ({g} named {})", shown(&got)))
    })
}

pub(super) fn s12(t: &mut Two) -> Step {
    t.scenario("s12-note-text", 12, "a note's text and a move reach the other instance", |t| {
        let n = t.new_node("NEWNOTE", "scenario 12: A's NEWNOTE adds exactly one node", "s12n")?;
        t.wait_field(
            Field::Text,
            "scenario 12: A, B and the document agree on the new note's text",
            "s12-setup",
            &[&n],
        )?;
        t.send(Who::A, &format!("SETNOTETEXT {n} Typed by A"))?;
        t.wait_agree("scenario 12: A and B agree on the shared nodes after A's edit", "12")?;
        let (a, _) = t.wait_field(
            Field::Text,
            "scenario 12: A, B and the document agree on the text after A's edit",
            "s12",
            &[&n],
        )?;
        let text = session::field_of(&a, Field::Text, &n);
        observe(
            "scenario 12: the note holds A's text",
            text == Some(json!("Typed by A")),
            &format!("{n} holds {}, not A's text", shown(&text)),
        );
        t.send(Who::A, &format!("SETNOTETEXT {n} Both typed A"))?;
        t.send(Who::B, &format!("SETNOTETEXT {n} Both typed B"))?;
        t.show_local(Field::Text, "s12c", "edits", &[&n])?;
        t.wait_agree("scenario 12: A and B agree on the shared nodes after both edits", "12c")?;
        let (a, _) = t.wait_field(
            Field::Text,
            "scenario 12: A, B and the document agree on the text after both edits",
            "s12c",
            &[&n],
        )?;
        let got = session::field_of(&a, Field::Text, &n);
        observe(
            "scenario 12: the note ends with A's or B's text",
            got == Some(json!("Both typed A")) || got == Some(json!("Both typed B")),
            &format!("{n} ended holding {}, neither A's nor B's text", shown(&got)),
        );
        // The mirror and the document must stay agreed, not just reach agreement once: a later save on either side
        // (B's here, A's in wait_field's EXPORTSESSION) mustn't bring a losing text back (`commit_note_buffers` commits
        // any box that differs from the mirror).
        t.session(Who::B, "s12c2-b-save")?;
        let (a, _) = t.wait_field(
            Field::Text,
            "scenario 12: A, B and the document still agree on the text after B saves",
            "s12c2",
            &[&n],
        )?;
        let again = session::field_of(&a, Field::Text, &n);
        observe(
            "scenario 12: B's save leaves the winning text in place",
            again == got,
            &format!("{n} went from {} to {} with no edit", shown(&got), shown(&again)),
        );
        t.send(Who::A, &format!("MOVENODE {n} 321 -123"))?;
        t.wait_agree("scenario 12: A and B agree on the shared nodes after A's move", "12m")?;
        let (a, _) = t.wait_field(
            Field::Pos,
            "scenario 12: A, B and the document agree on the pos after A's move",
            "s12m",
            &[&n],
        )?;
        let pos = session::field_of(&a, Field::Pos, &n);
        // Exact, as the script's string compare with '[321.0, -123.0]': f32 through serde writes 321.0, and an integer
        // 321 would fail both.
        observe(
            "scenario 12: the note is where A dropped it",
            pos == Some(json!([321.0, -123.0])),
            &format!("{n} is at {}, not where A dropped it", shown(&pos)),
        );
        Ok(format!(" ({n} holds {})", shown(&got)))
    })
}
