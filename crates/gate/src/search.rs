//! `gate search`: a word that only a page's text holds finds that page's card. Two tabs are seeded, `/text` (the one
//! that loads) and `/` (asleep), `WAITINDEX` waits for the page-text index to catch up, and the Graph omnibar's text
//! (`GRAPHFILTERTEXT`) is a word from `/text`: `EXPORTSTATE`'s `omni_matches` must list that tab and not the other. Build first:
//! `cargo build -p manifold -p manifold-view-webview2`.

use std::path::Path;
use std::process::ExitCode;

use crate::check::check;
use crate::probes::{finish, for_each_view, quit, tab_node, Site};

pub(crate) fn search(exe: &Path, work: &Path) -> ExitCode {
    let site = match Site::start() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("site: {e}");
            return ExitCode::from(1);
        }
    };
    for_each_view(
        "search",
        exe,
        work,
        |view| {
            let mut second = tab_node(Some(&site.url("/")), view);
            second["id"] = "t2".into();
            second["pos"] = serde_json::json!([400.0, 0.0]);
            Ok(((), vec![tab_node(Some(&site.url("/text")), view), second]))
        },
        |view, inst, ()| {
            inst.settle()?;
            inst.send("WAITINDEX")?;
            inst.send("GRAPHFILTERTEXT lazy")?;
            inst.send("WAITIDLE")?;
            let state = inst.export_state("matches")?;
            let loaded: Vec<String> = state
                .cards()
                .iter()
                .filter(|c| c["loaded"] == true)
                .filter_map(|c| c["id"].as_str().map(str::to_owned))
                .collect();
            let matches: Vec<String> = state.0["omni_matches"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            quit(inst, view);
            check(
                &format!(
                    "{view}: a word only a page's text holds matches that tab alone ({matches:?}, loaded {loaded:?})"
                ),
                !loaded.is_empty() && matches == loaded,
            );
            Ok(())
        },
    );
    finish(work)
}
