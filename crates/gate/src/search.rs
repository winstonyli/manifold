//! `gate search`: a word that only a page's text holds finds that page's card. Two tabs are seeded, `/text` (the one
//! that loads) and `/` (asleep), `WAITINDEX` waits for the page-text index to catch up, and the Graph omnibar's text
//! (`GRAPHFILTERTEXT`) is a word from `/text`: `EXPORTSTATE`'s `omni_matches` must list that tab and not the other. Build first:
//! `cargo build -p manifold -p manifold-view-webview2`.

use std::path::Path;
use std::process::ExitCode;

use crate::check::{check, Section};
use crate::instance::Instance;
use crate::probes::{finish, guarded, quit, seed, tab_node, views, Site};

pub(crate) fn search(exe: &Path, work: &Path) -> ExitCode {
    let site = match Site::start() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("site: {e}");
            return ExitCode::from(1);
        }
    };
    for view in views() {
        let _t = Section::start(&format!("search-{view}"));
        guarded(&format!("{view} search"), || {
            let profile = work.join(format!("appdata-{view}"));
            let mut second = tab_node(Some(&site.url("/")), &view);
            second["id"] = "t2".into();
            second["pos"] = serde_json::json!([400.0, 0.0]);
            seed(exe, &profile, &view, &[tab_node(Some(&site.url("/text")), &view), second])?;
            let mut inst = Instance::launch(exe, &profile, work, &format!("search-{view}.log"))?;
            inst.send("WAITTABS")?;
            inst.send("WAITIDLE")?;
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
            quit(&mut inst, &view);
            check(
                &format!(
                    "{view}: a word only a page's text holds matches that tab alone ({matches:?}, loaded {loaded:?})"
                ),
                !loaded.is_empty() && matches == loaded,
            );
            Ok(())
        });
    }
    finish(work)
}
