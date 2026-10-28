//! Scenarios 3 to 5: tabs on the nonce site (nonce.rs) and a local JSON file. A and B are up.

use std::fs;
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::{observe, require, wait_until, Step, Two, Who, PAUSE, POSITIVE};
use crate::instance::file_url;
use crate::nonce::Site;
use crate::session;
use crate::state::JSON;

/// The script's 600 rounds of 0.2 s for A's and B's loads.
const FOLLOW: Duration = Duration::from_secs(150);

pub(super) fn s3(t: &mut Two) -> Step {
    t.scenario("s3-nonce-site", 3, "a url that changes on every load", |t| {
        t.site = Some(Site::start()?);
        let (c, port) = (t.counters()?, t.port()?);
        t.send(Who::A, "NEWTAB")?;
        t.send(Who::A, "WAITTABS")?;
        // The new tab is focused but shown in Graph view headless.
        t.send(Who::A, "TOGGLEVIEW")?;
        // Let the new tab's own first load settle, or its address-bar update overwrites the typed url before ENTER.
        t.send(Who::A, "WAITIDLE")?;
        t.navigate(Who::A, &format!("http://127.0.0.1:{port}/r"))?;
        // B's copy of A's new tab arrives asleep (spec "Lazy tabs"): once A's url has reached it, open it there, so B
        // loads it and follows from then on (scenario 4 too). Its landing is a host-started load, which is what keeps
        // B's own redirect local below. The script greps the raw export text; so does this.
        let needle = format!("127.0.0.1:{port}");
        let deadline = Instant::now() + POSITIVE;
        let reached = loop {
            t.tick(Who::B)?;
            let p = t.request(Who::B, "EXPORTSESSION", "b3")?;
            super::read_json(&p)?;
            if fs::read_to_string(&p)?.contains(&needle) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            sleep(PAUSE);
        };
        require("scenario 3: A's url reaches B's document", reached, "")?;
        t.send(Who::B, "QUICKSWITCHOPEN")?;
        t.send(Who::B, &format!("QUICKSWITCHTEXT 127.0.0.1:{port}"))?;
        t.send(Who::B, "QUICKSWITCHENTER")?;
        // Back to Graph view, where B was: C restores B's personal state, and scenario 7 toggles C into Tab view.
        t.send(Who::B, "TOGGLEVIEW")?;
        // A's load (a redirect, then the page), then B's (the same, as its copy follows): at least 4 requests.
        let followed = wait_until(FOLLOW, || c.hits() >= 4);
        require(
            "scenario 3: B follows A's navigation (at least 4 requests)",
            followed,
            &format!("only {} requests; B never followed A's navigation", c.hits()),
        )?;
        // Both loads must be finished before `settled` is read: under load B's follow can still be in flight after
        // the 4th request, and its later requests would read as a bounce. Idle is an observable, so wait on it.
        t.wait_agree("scenario 3: A and B agree before the window", "3")?;
        t.send(Who::A, "WAITIDLE")?;
        t.send(Who::B, "WAITIDLE")?;
        // The window is timed here, so it starts once both have run their WAITIDLE, not when it was sent.
        t.inst(Who::A)?.barrier()?;
        t.inst(Who::B)?.barrier()?;
        // Negative window (plan "Negative windows"): past HOST_LOAD_GRACE (5 s, the app's own timer, so a real wait),
        // then a window as long again; a bounce is a round trip every second or two, so the count would keep climbing.
        sleep(Duration::from_secs(6));
        let settled = c.hits();
        sleep(Duration::from_secs(10));
        let now = c.hits();
        observe(
            "scenario 3: the instances stop navigating each other (no new request in 10 s)",
            now == settled,
            &format!("the instances keep navigating each other ({settled} -> {now} requests in 10 s)"),
        );
        Ok(format!(" ({settled} requests)"))
    })
}

pub(super) fn s4(t: &mut Two) -> Step {
    t.scenario("s4-click-vs-script", 4, "a click is shared, a script's navigation isn't", |t| {
        let (c, port) = (t.counters()?, t.port()?);
        let base = format!("http://127.0.0.1:{port}");
        let before = c.r_repeats();
        let link_before = c.served("/link");
        t.navigate(Who::A, &format!("{base}/link"))?;
        // The script's wait_served: one more request for the exact path, then A's tab finishes loading it.
        let seen = wait_until(POSITIVE, || c.served("/link") > link_before);
        require("scenario 4: the server sees A's /link load", seen, "the server never saw a request for /link")?;
        t.send(Who::A, "WAITIDLE")?;
        // The page is one window-sized link, so any click below the toolbar hits it. A lands on a served-once /r?n=
        // and shares it; B asks for it again.
        t.send(Who::A, "CLICK 400 300")?;
        let clicked = wait_until(POSITIVE, || c.r_repeats() > before);
        observe("scenario 4: B follows A's link click", clicked, "B never followed A's link click");
        t.navigate(Who::A, &format!("{base}/js"))?;
        // A's page and then B's (it follows the typed url) each navigate by script to their own /dest; had A's been
        // shared, B would ask for A's too.
        let both = wait_until(POSITIVE, || c.dests() >= 2);
        require(
            "scenario 4: A's and B's /js pages each navigate by script",
            both,
            &format!("only {} script navigations ran; expected A's and B's", c.dests()),
        )?;
        // Negative window: the page's timer fires 500 ms after load; 5 s lets a wrongly shared /dest reach B.
        sleep(Duration::from_secs(5));
        let repeats = c.dest_repeats();
        if !observe("scenario 4: B does not follow A's script navigation (5 s window)", repeats == 0, "") {
            t.dump_urls("4")?;
        }
        // A single-page app: a click's pushState is shared, a timer's isn't. Only B's load of a shared url asks the
        // server for /spa/c or /spa/t.
        let spa_before = c.served("/spa");
        t.navigate(Who::A, &format!("{base}/spa"))?;
        let seen = wait_until(POSITIVE, || c.served("/spa") > spa_before);
        require("scenario 4: the server sees A's /spa load", seen, "the server never saw a request for /spa")?;
        t.send(Who::A, "WAITIDLE")?;
        t.send(Who::A, "CLICK 400 300")?;
        let pushed = wait_until(POSITIVE, || c.spa_clicks() >= 1);
        require(
            "scenario 4: B follows A's pushState from a click",
            pushed,
            "B never followed A's pushState from a click",
        )?;
        // Negative window: the timer's push comes 1 s after the click; 5 s lets a wrongly shared one reach B.
        sleep(Duration::from_secs(5));
        let timers = c.spa_timers();
        observe(
            "scenario 4: B does not follow A's pushState from a timer (5 s window)",
            timers == 0,
            &format!("{timers} timer pushes were loaded"),
        );
        Ok(String::new())
    })
}

pub(super) fn s5(t: &mut Two) -> Step {
    t.scenario("s5-json-convert", 5, "a shared tab converting to a node", |t| {
        // A navigates the shared tab to a local JSON file (no network); B follows. Both engines load it and report
        // content, but the document must end up with exactly one more Json node, not one per instance.
        let file = t.path("c5.json");
        fs::write(&file, "{\"converted\": true}\n")?;
        let url = file_url(&file);
        let before = t.session(Who::A, "a5-before")?;
        let json_before = session::count_kind(&before, JSON);
        t.navigate(Who::A, &url)?;
        // No verdict of its own (as the script's loop): the count check after the window decides.
        let deadline = Instant::now() + POSITIVE;
        loop {
            t.tick(Who::A)?;
            let a5 = t.session(Who::A, "a5")?;
            if session::count_kind(&a5, JSON) > json_before || Instant::now() >= deadline {
                break;
            }
            sleep(PAUSE);
        }
        // Negative window: time for B to follow the url, load it and convert too, if it's going to.
        sleep(Duration::from_secs(8));
        let doc = t.wait_agree("scenario 5: A and B agree on the shared nodes after the conversion", "5")?;
        let json_after = session::count_kind(&doc, JSON);
        observe(
            "scenario 5: the document gains exactly one Json node",
            json_after == json_before + 1,
            &format!("Json nodes went {json_before} -> {json_after}, want exactly one more"),
        );
        Ok(String::new())
    })
}
