//! `gate labels`: labels end to end in a headless instance. Two notes carry one raw `color` from an old session, which
//! is ignored (a card has no colour of its own). Two topic notes accept a suggestion as a label
//! (`ACCEPTSUGGESTIONLABEL`), a second label goes on two selected notes (`SELECTCARDS`, `LABELSELECTED`), one undo
//! clears it, and after a restart the labels are still there. No page loads, so no engine is needed. Build first:
//! `cargo build -p manifold`.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::check::{check, Section};
use crate::instance::Instance;
use crate::probes::{finish, guarded, import_session, quit, setup};

const NOTE: &str = "text/markdown";
const TEXTS: [&str; 6] = [
    "cat milk fur purr cat",
    "cat fur kitten milk",
    "kitten purr milk cat fur",
    "rust cargo crate compile rust",
    "cargo crate rust build",
    "compile build crate rust cargo",
];

/// How many cards in `state` carry a label named `name`.
fn carrying(state: &crate::state::State, name: &str) -> usize {
    state.cards().iter().filter(|c| c["labels"].as_array().is_some_and(|a| a.iter().any(|l| l == name))).count()
}

pub(crate) fn labels(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "labels") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let _t = Section::start("labels");
    guarded("labels", || {
        let profile = work.join("appdata-labels");
        std::fs::create_dir_all(&profile)?;
        let nodes: Vec<_> = TEXTS
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let mut node =
                    json!({"id": format!("n{i}"), "type": NOTE, "text": text, "pos": [i as f64 * 300.0, 0.0]});
                if i == 0 {
                    node["color"] = json!("#ff0000");
                }
                node
            })
            .collect();
        import_session(&exe, &profile, &nodes)?;
        let mut inst = Instance::launch(&exe, &profile, &work, "labels.log")?;
        inst.send("WAITIDLE")?;
        let start = inst.export_state("start")?;
        let old_colour_ignored = start.cards().iter().all(|c| c.get("color").is_none());
        inst.send("SUGGEST")?;
        inst.send("WAITIDLE")?;
        inst.send("ACCEPTSUGGESTIONLABEL 0")?;
        inst.send("WAITIDLE")?;
        let accepted = inst.export_state("accepted")?;
        let ids: Vec<String> = accepted.cards().iter().filter_map(|c| c["id"].as_str().map(str::to_owned)).collect();
        let labelled: Vec<&Value> =
            accepted.cards().into_iter().filter(|c| c["labels"].as_array().is_some_and(|a| !a.is_empty())).collect();
        let defs = accepted.0["labels"].as_array().map_or(0, Vec::len);
        // Two notes that carry no label yet get "urgent", then one undo takes it off both.
        let bare: Vec<String> = accepted
            .cards()
            .iter()
            .filter(|c| c["labels"].as_array().is_some_and(Vec::is_empty))
            .take(2)
            .filter_map(|c| c["id"].as_str().map(str::to_owned))
            .collect();
        inst.send(&format!("SELECTCARDS {}", bare.join(" ")))?;
        inst.send("LABEL urgent")?;
        inst.send("WAITIDLE")?;
        let urgent = carrying(&inst.export_state("urgent")?, "urgent");
        inst.send("LABELSELECTED urgent off")?;
        inst.send("WAITIDLE")?;
        let off = carrying(&inst.export_state("off")?, "urgent");
        inst.send("LABELSELECTED urgent on")?;
        inst.send("UNDOKEY")?;
        inst.send("WAITIDLE")?;
        let undone = carrying(&inst.export_state("undone")?, "urgent");
        quit(&mut inst, "labels");
        let mut again = Instance::launch(&exe, &profile, &work, "labels-restart.log")?;
        again.send("WAITIDLE")?;
        let restored = again.export_state("restored")?;
        let kept = restored.0["labels"].as_array().map_or(0, Vec::len);
        let still: usize =
            restored.cards().iter().filter(|c| c["labels"].as_array().is_some_and(|a| !a.is_empty())).count();
        quit(&mut again, "labels");
        check(&format!("an old raw colour on a note shows no colour ({old_colour_ignored})"), old_colour_ignored);
        check(
            &format!("accepting a suggestion as a label labels its three notes ({} of {})", labelled.len(), ids.len()),
            labelled.len() == 3,
        );
        check(&format!("and makes one label ({defs})"), defs == 1);
        check(&format!("the typed label goes on the two selected notes ({urgent})"), urgent == 2);
        check(&format!("the toggle takes it off both ({off} left)"), off == 0);
        check(&format!("and one undo of the next toggle takes it off again ({undone} left)"), undone == 0);
        check(
            &format!("after a restart the {kept} label(s) and {still} labelled notes are still there"),
            kept >= 1 && still == 3,
        );
        Ok(())
    });
    finish(&work)
}
