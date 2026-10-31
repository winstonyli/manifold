//! `gate page-text`: a loaded tab's text reaches the text log. The tab opens `/text` on the local site, a moment after
//! its load ends the app asks the engine for the page text (`QueryPageText`), and the headless `TEXTOF` reads back what
//! the app stored. The page holds a script and a style, whose contents are not page text. Build first:
//! `cargo build -p manifold -p manifold-view-webview2`.

use std::io;
use std::time::Instant;

use crate::check::check;
use crate::instance::STEP_TIMEOUT;
use crate::probes::{for_each_view, quit, tab_node, Site};
use crate::windows_main::Ctx;

pub(crate) fn page_text(ctx: &mut Ctx) -> io::Result<()> {
    let (exe, work) = (ctx.exe.as_path(), ctx.work.as_path());
    let site = Site::start().map_err(|e| io::Error::other(format!("site: {e}")))?;
    for_each_view(
        "page-text",
        exe,
        work,
        |view| Ok(((), vec![tab_node(Some(&site.url("/text")), view)])),
        |view, inst, ()| {
            inst.settle()?;
            let state = inst.export_state("ids")?;
            let id = state.cards().first().and_then(|c| c["id"].as_str()).unwrap_or("t1").to_owned();
            // The text is read a short while after the load ends: ask until it is stored.
            let deadline = Instant::now() + STEP_TIMEOUT;
            let mut text = inst.text_of("text-0", &id)?;
            for n in 1.. {
                if text != "none" || Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
                text = inst.text_of(&format!("text-{n}"), &id)?;
            }
            if text == "none" {
                // A flake leaves evidence: the card as the app saw it when the wait ran out.
                match inst.export_state("stuck") {
                    Ok(state) => println!("  stuck: card {:?}", state.cards().first()),
                    Err(e) => println!("  stuck: no state export: {e}"),
                }
            }
            quit(inst, view);
            if !check(
                &format!("{view}: the page's visible text is stored, whitespace collapsed ({text:?})"),
                text.contains("The quick brown fox") && text.contains("jumps over the lazy dog"),
            ) {
                return Ok(());
            }
            check(&format!("{view}: script and style content is not stored"), !text.contains("MARK"));
            Ok(())
        },
    );
    Ok(())
}
