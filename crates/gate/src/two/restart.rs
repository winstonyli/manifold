//! Scenarios 6 and 7: A is killed, B quits, C and then D open the same profile alone.
//!
//! A kill is TerminateProcess, not power loss: the store's on-disk file is never mid-write at that point (new_node's
//! last EXPORTSESSION saves first, immediately before the kill), so this exercises "a replica vanishes", not crash
//! recovery of a torn write.

use std::time::Duration;

use super::{observe, require, wait_until, Step, Two, Who, POSITIVE};
use crate::session;

/// One attempt at typing the url in scenario 7 (the script's 50 rounds of 0.2 s).
const ATTEMPT: Duration = Duration::from_secs(10);

pub(super) fn s6(t: &mut Two) -> Step {
    t.scenario("s6-kill", 6, "kill A", |t| {
        let n = t.new_node("NEWNOTE", "scenario 6: A's NEWNOTE adds exactly one node", "s6n")?;
        println!("A's new note id: {n}");
        t.kill(Who::A);
        t.quit(Who::B)?;
        t.launch(Who::C)?;
        let doc = t.session(Who::C, "c")?;
        observe(
            "scenario 6: a fresh instance on the profile has A's note",
            session::has_id(&doc, &n),
            &format!("{n:?} is not in C's document"),
        );
        Ok(String::new())
    })
}

pub(super) fn s7(t: &mut Two) -> Step {
    t.scenario("s7-restart", 7, "a restart reopens a tab's live url", |t| {
        // C is the only instance, so its personal state is the one on disk after it quits (two instances would each
        // overwrite the other's on save).
        let (c, port) = (t.counters()?, t.port()?);
        let dests = c.dests();
        let js_first = c.served("/js");
        t.send(Who::C, "NEWTAB")?;
        t.send(Who::C, "WAITTABS")?;
        t.send(Who::C, "TOGGLEVIEW")?;
        // Typed text is dropped if the view changes before the bar takes focus, and a new tab's first load can
        // overwrite it before ENTER; under load either loses the request, so type again once if the server never
        // sees /js. The clock starts once C has run ENTER: a retry sent while the first ENTER still waits in the queue
        // loads /js twice, the second after js_before is read.
        let mut requested = false;
        for _ in 0..2 {
            t.send(Who::C, "WAITIDLE")?;
            t.navigate(Who::C, &format!("http://127.0.0.1:{port}/js"))?;
            t.inst(Who::C)?.barrier()?;
            requested = wait_until(ATTEMPT, || c.served("/js") > js_first);
            if requested {
                break;
            }
        }
        require(
            "scenario 7: C requests the typed /js url",
            requested,
            &format!("the address bar text was lost\n{}", c.report()),
        )?;
        let navigated = wait_until(POSITIVE, || c.dests() > dests);
        require("scenario 7: C's /js page navigates to /dest", navigated, &c.report())?;
        // Let C see its tab settle on /dest before quitting saves it: the 2 s pass in the app, and the counters are
        // read once C has run them.
        t.send(Who::C, "SLEEP 2")?;
        t.inst(Who::C)?.barrier()?;
        let js_before = c.served("/js");
        let dest_repeats = c.dest_repeats();
        t.quit(Who::C)?;
        t.launch(Who::D)?;
        t.send(Who::D, "WAITTABS")?;
        // D asks again for the /dest?n= C's script chose (a dest repeat).
        let reopened = wait_until(POSITIVE, || c.dest_repeats() > dest_repeats);
        require("scenario 7: D reopens C's /dest url", reopened, &c.report())?;
        let js_now = c.served("/js");
        observe(
            "scenario 7: D does not load the typed /js again",
            js_now == js_before,
            &format!("/js requests went {js_before} -> {js_now}"),
        );
        t.quit(Who::D)?;
        Ok(String::new())
    })
}
