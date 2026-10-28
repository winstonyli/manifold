//! `gate click-count`: a double click on a page selects a word and a triple click its paragraph, on each engine. The
//! tab opens `/clicks` on the local site, `DOUBLECLICK` then `TRIPLECLICK` land on its one line of text, and the page
//! reports every mouse release's click count and the selection it holds. Build first:
//! `cargo build -p manifold -p manifold-view-webview2`.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use crate::check::check;
use crate::probes::{finish, for_each_view, quit, tab_node, Site};

const TEXT: &str = "alpha beta gamma delta";

/// The selection the page reported with the last release of click count `d`.
fn selection(site: &Site, d: u8) -> Option<String> {
    let prefix = format!("click d={d} sel=");
    site.lines().iter().rev().find_map(|l| l.strip_prefix(&prefix).map(|s| s.trim().to_owned()))
}

pub(crate) fn click_count(exe: &Path, work: &Path) -> ExitCode {
    for_each_view(
        "click-count",
        exe,
        work,
        |view| {
            let site = Site::start()?;
            let nodes = vec![tab_node(Some(&site.url("/clicks")), view)];
            Ok((site, nodes))
        },
        |view, inst, site| {
            inst.settle()?;
            inst.step(&["GRAPHDOWN", "GRAPHOPEN"], "the page to load", || site.hits("/clicks") > 0)?;
            inst.export_state(&format!("state-{view}"))?;
            let limit = Duration::from_secs(30);
            inst.send("DOUBLECLICK 300 200")?;
            inst.barrier()?;
            let doubled = site.wait_for("click d=2", 1, limit);
            // Read before the triple click: its own second release reports a count of 2 too.
            let word = selection(site, 2);
            inst.send("TRIPLECLICK 300 200")?;
            inst.barrier()?;
            let tripled = site.wait_for("click d=3", 1, limit);
            quit(inst, view);
            check(
                &format!("{view}: a double click reports count 2 and selects one word ({word:?})"),
                doubled && word.as_deref().is_some_and(|w| !w.is_empty() && !w.contains(' ') && TEXT.contains(w)),
            );
            let line = selection(site, 3);
            check(
                &format!("{view}: a triple click reports count 3 and selects the paragraph ({line:?})"),
                tripled && line.as_deref() == Some(TEXT),
            );
            Ok(())
        },
    );
    finish(work)
}
