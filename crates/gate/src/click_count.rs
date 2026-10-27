//! `gate click-count`: a double click on a page selects a word and a triple click its paragraph, on each engine. The
//! tab opens `/clicks` on the local site, `DOUBLECLICK` then `TRIPLECLICK` land on its one line of text, and the page
//! reports every mouse release's click count and the selection it holds. Build first:
//! `cargo build -p manifold -p manifold-view-webview2`.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use crate::check::{check, Section};
use crate::instance::Instance;
use crate::probes::{finish, guarded, quit, seed, tab_node, views, Site};

const TEXT: &str = "alpha beta gamma delta";

/// The selection the page reported with the last release of click count `d`.
fn selection(site: &Site, d: u8) -> Option<String> {
    let prefix = format!("click d={d} sel=");
    site.lines().iter().rev().find_map(|l| l.strip_prefix(&prefix).map(|s| s.trim().to_owned()))
}

pub(crate) fn click_count(exe: &Path, work: &Path) -> ExitCode {
    for view in views() {
        let _t = Section::start(&format!("click-count-{view}"));
        guarded(&format!("{view} click count"), || {
            let site = Site::start()?;
            let profile = work.join(format!("appdata-{view}"));
            seed(exe, &profile, &view, &[tab_node(Some(&site.url("/clicks")), &view)])?;
            let mut inst = Instance::launch(exe, &profile, work, &format!("click-count-{view}.log"))?;
            for line in ["WAITTABS", "WAITIDLE"] {
                inst.send(line)?;
            }
            inst.step(&["GRAPHDOWN", "GRAPHOPEN"], "the page to load", || site.hits("/clicks") > 0)?;
            inst.export_state(&format!("state-{view}"))?;
            let limit = Duration::from_secs(30);
            inst.send("DOUBLECLICK 300 200")?;
            let doubled = site.wait_for("click d=2", 1, limit);
            // Read before the triple click: its own second release reports a count of 2 too.
            let word = selection(&site, 2);
            inst.send("TRIPLECLICK 300 200")?;
            let tripled = site.wait_for("click d=3", 1, limit);
            quit(&mut inst, &view);
            check(
                &format!("{view}: a double click reports count 2 and selects one word ({word:?})"),
                doubled && word.as_deref().is_some_and(|w| !w.is_empty() && !w.contains(' ') && TEXT.contains(w)),
            );
            let line = selection(&site, 3);
            check(
                &format!("{view}: a triple click reports count 3 and selects the paragraph ({line:?})"),
                tripled && line.as_deref() == Some(TEXT),
            );
            Ok(())
        });
    }
    finish(work)
}
