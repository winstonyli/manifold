//! `gate iframe-typing`: keys typed into an iframe from another origin reach it. The tab opens `/outer` on one local
//! site, whose iframe shows `/frame` of a second site (another port, so another origin); a click lands in the frame's
//! input, `TYPE` sends characters to the tab's page, and the frame reports its value to its own site. Build first:
//! `cargo build -p manifold -p manifold-view-webview2`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::check::{check, Section};
use crate::instance::Instance;
use crate::probes::{finish, guarded, quit, seed, setup, tab_node, views, Site};

pub(crate) fn iframe_typing(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "iframe-typing") {
        Ok(v) => v,
        Err(code) => return code,
    };
    for view in views() {
        let _t = Section::start(&format!("iframe-typing-{view}"));
        guarded(&format!("{view} iframe typing"), || {
            let (outer, inner) = (Site::start()?, Site::start()?);
            let url = format!("{}?src={}", outer.url("/outer"), inner.url("/frame"));
            let profile = work.join(format!("appdata-{view}"));
            seed(&exe, &profile, &view, &[tab_node(Some(&url), &view)])?;
            let mut inst = Instance::launch(&exe, &profile, &work, &format!("iframe-typing-{view}.log"))?;
            for line in ["WAITTABS", "WAITIDLE"] {
                inst.send(line)?;
            }
            inst.step(&["GRAPHDOWN", "GRAPHOPEN"], "the frame to load", || inner.hits("/frame") > 0)?;
            inst.export_state(&format!("state-{view}"))?;
            // CEF loses the first click into the frame (2 clicks needed, 2026-10-24), so click until it is seen.
            for _ in 0..6 {
                inst.send("CLICK 300 200")?;
                inst.send("WAITIDLE")?;
                if inner.wait_for("frame saw mousedown", 1, Duration::from_secs(2)) {
                    break;
                }
            }
            // Engines with the frame in another process take a moment to focus it: keys sent before it does are lost
            // (WebKit drops the first). Servo never reports `focusin`, so that wait ends by the clock.
            let clicked = inner.wait_for("frame saw mousedown", 1, Duration::from_secs(10));
            inner.wait_for("frame saw focusin", 1, Duration::from_secs(3));
            inst.send("TYPE abc")?;
            let typed = inner.wait_for("typed v=abc", 1, Duration::from_secs(30));
            quit(&mut inst, &view);
            check(&format!("{view}: a click in a cross-origin iframe reaches it"), clicked);
            check(&format!("{view}: characters typed into a cross-origin iframe arrive ({:?})", inner.lines()), typed);
            Ok(())
        });
    }
    finish(&work)
}
