//! `gate suggest-groups`: six notes, three about cats and three about rust. "Suggest groups" (`SUGGEST`) offers two groups
//! of three (`EXPORTSTATE`'s `group_suggestions`), and accepting one (`ACCEPTSUGGESTION 0`) moves its three cards off
//! the level under a new group card. No page loads, so no engine is needed. Build first: `cargo build -p manifold`.

use std::path::Path;
use std::process::ExitCode;

use serde_json::json;

use crate::check::{check, Section};
use crate::instance::Instance;
use crate::probes::{finish, guarded, import_session, quit};

const NOTE: &str = "text/markdown";
const TEXTS: [&str; 6] = [
    "cat milk fur purr cat",
    "cat fur kitten milk",
    "kitten purr milk cat fur",
    "rust cargo crate compile rust",
    "cargo crate rust build",
    "compile build crate rust cargo",
];

pub(crate) fn suggest_groups(exe: &Path, work: &Path) -> ExitCode {
    let _t = Section::start("suggest");
    guarded("suggest", || {
        let profile = work.join("appdata-suggest");
        std::fs::create_dir_all(&profile)?;
        let nodes: Vec<_> = TEXTS
            .iter()
            .enumerate()
            .map(|(i, text)| json!({"id": format!("n{i}"), "type": NOTE, "text": text, "pos": [i as f64 * 300.0, 0.0]}))
            .collect();
        import_session(exe, &profile, &nodes)?;
        let mut inst = Instance::launch(exe, &profile, work, "suggest.log")?;
        inst.send("WAITIDLE")?;
        inst.send("SUGGEST")?;
        inst.send("WAITIDLE")?;
        let offered = inst.export_state("offered")?;
        let groups: Vec<usize> = offered.0["group_suggestions"]
            .as_array()
            .map(|a| a.iter().map(|s| s["cards"].as_array().map_or(0, Vec::len)).collect())
            .unwrap_or_default();
        inst.send("ACCEPTSUGGESTION 0")?;
        inst.send("WAITIDLE")?;
        let after = inst.export_state("accepted")?;
        let notes = after.cards().iter().filter(|c| c["type"] == NOTE).count();
        let standing = after.0["group_suggestions"].as_array().map_or(usize::MAX, Vec::len);
        quit(&mut inst, "suggest");
        check(&format!("two groups of three are offered ({groups:?})"), groups == [3, 3]);
        check(&format!("accepting one takes its three notes off the level ({notes} left)"), notes == 3);
        check(&format!("the other offers go stale with the card set ({standing} standing)"), standing == 0);
        Ok(())
    });
    finish(work)
}
