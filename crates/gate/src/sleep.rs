//! `gate sleep` (was scripts/sleep-probe.sh): "Sleep" per engine. Sleep is two-phase: `App::request_sleep` asks the page
//! for its state (`QueryPageState`) and `finish_sleep` sleeps on the reply. `CASES` (default `manual idle scroll dirty`)
//! picks from:
//!
//! - `manual`: the menu action (headless `SLEEPNODE id`): tab, asleep, woken
//! - `idle`: idle auto-sleep (`App::sweep_idle_tabs`, headless `IDLESWEEP secs`, run twice 3 s apart: the first sweep
//!   starts the clock): tab, asleep, woken
//! - `scroll`: a tall page scrolled with PageDown in Tab view, slept from Graph view and woken: the page must come back
//!   at the position it had (|before - after| <= 2 px, before > 0)
//! - `history`: two more pages, then sleep and wake; Back twice and Forward once must retrace them (the host keeps the
//!   trail, `nav_history`)
//! - `crash`: as history, but the engine host is killed instead of slept: the app restarts the tab and Back and
//!   Forward must still work
//! - `restart`: as history, but the app is quit and started again on the same profile (`tab-history.json`)
//! - `dirty`: `/tall?dirty=1` has an input changed from its default: idle sleep must leave it awake, manual Sleep must
//!   still sleep it
//!
//! Opening a card starts the engine again and brings the tab back, under the same id. One headless instance per case
//! and view kind (`VIEWS`, default `webview2`) on a fresh profile, the tab at a local page, state read back through
//! `EXPORTSTATE`. Build first: `cargo build -p manifold -p manifold-view-cef -p manifold-view-webview2`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::check::{check, check_eq, Section};
use crate::instance::{poll, Instance, STEP_TIMEOUT};
use crate::probes::{finish, guarded, quit, seed, setup, tab_node, views, Site};
use crate::state::{State, TAB};
use crate::stress::powershell;

/// The window in which a tab that must STAY awake is given the chance to sleep (the dirty case's "idle sleep left it
/// awake"): a negative wait, so a real sleep and never shortened. WebView2 sleeps at once, but a CEF tab's sleep waits
/// for its last cookies: the host's jar pull answers only after `EMPTY_PULL_TIMEOUT` (2 s, manifold-engine-cef
/// jar_cookies.rs) on an empty jar, and the app holds the page state up to `COOKIE_HOLD` (2 s, tabs.rs) for it, so CEF
/// sleeps ~2 s after the request. A 1-2 s wait exported a CEF tab before it slept, and the check passed on CEF without
/// proving it. Positive waits for a tab to sleep or wake poll its exported state instead (`Instance::export_until`).
const ASLEEP_WAIT: &str = "SLEEP 4";

/// The cards in an exported state, "id:label" each.
fn cards(state: &State) -> String {
    let ids: Vec<&str> = state.cards().iter().filter_map(|c| c["id"].as_str()).collect();
    ids.iter().map(|id| format!("{id}:{}", state.label(id))).collect::<Vec<_>>().join(" ")
}

fn send_all(inst: &mut Instance, lines: &[&str]) -> io::Result<()> {
    lines.iter().try_for_each(|l| inst.send(l))
}

/// Back, Back, Forward with a state export after each, as `back1`, `back2`, `fwd` (the last path segment of the focused
/// url, `/` for none). Each navigation is waited for as the focused url changing from `url`, the one before it (a
/// history navigation may be served from cache, so the probe server need not see a request).
fn back_and_forward(inst: &mut Instance, mut url: String) -> io::Result<Vec<String>> {
    let mut urls = Vec::new();
    for (command, name) in [("GOBACK", "back1"), ("GOBACK", "back2"), ("GOFORWARD", "fwd")] {
        inst.send(command)?;
        inst.export_until(name, |s| s.focused_url() != url && s.trail_at_focus())?;
        // The url changes as the navigation starts, and a plain move's before the engine has reported it: the next
        // command, sent before the trail arrives, would be ignored as a move still pending. WAITIDLE holds the export
        // behind it until the load ends.
        inst.send("WAITIDLE")?;
        let state = inst.export_state(name)?;
        url = state.focused_url().to_owned();
        urls.push(last_segment(&state));
    }
    Ok(urls)
}

/// Waits until the page has reported a line `want` accepts (the lines are the page's own, in order).
fn wait_page(site: &Site, what: &str, want: impl Fn(&[String]) -> bool) -> io::Result<()> {
    poll(what, STEP_TIMEOUT, || want(&site.lines()))
}

fn last_segment(state: &State) -> String {
    let seg = state.focused_url().rsplit('/').next().unwrap_or("");
    if seg.is_empty() { "/" } else { seg }.to_owned()
}

/// Kills the engine hosts that are children of `parent` (only this instance's own: other manifolds may be running).
fn kill_hosts(work: &Path, parent: u32, view: &str) -> io::Result<()> {
    const SCRIPT: &str = "param([int]$Parent, [string]$Name)
Get-CimInstance Win32_Process -Filter \"ParentProcessId=$Parent AND Name='$Name'\" | ForEach-Object { $_.ProcessId }
";
    let name = format!("manifold-engine-{view}.exe");
    for pid in powershell(work, "hosts", SCRIPT, &["-Parent", &parent.to_string(), "-Name", &name])? {
        Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
    }
    Ok(())
}

/// The scroll reports of the tall page: A, the last before its second load (where it was slept), and B, the last after
/// (where the woken page ended up). B must come after the reload, or a page that never restored would pass with B = A.
fn scroll_reports(lines: &[String]) -> (Option<i64>, Option<i64>) {
    let (mut loads, mut a, mut b) = (0, None, None);
    for line in lines {
        if line.starts_with("load") {
            loads += 1;
        } else if let Some(y) = line.strip_prefix("scroll y=") {
            if loads < 2 {
                a = y.parse().ok()
            } else {
                b = y.parse().ok()
            }
        }
    }
    (a, b)
}

fn run_case(exe: &Path, work: &Path, case: &str, view: &str) -> io::Result<()> {
    let site = Site::start()?;
    let name = format!("{case}-{view}");
    let profile = work.join(format!("appdata-{name}"));
    let path = match case {
        "scroll" => "/tall",
        "dirty" => "/tall?dirty=1",
        _ => "/",
    };
    seed(exe, &profile, view, &[tab_node(Some(&site.url(path)), view)])?;
    let mut inst = Instance::launch(exe, &profile, work, &format!("{name}.log"))?;
    let (live_label, asleep_label) = (format!("{TAB};loaded"), TAB.to_owned());
    inst.send("WAITTABS")?;
    let live = inst.export_until(&format!("{name}-live"), |s| {
        s.cards().first().and_then(|c| c["id"].as_str()).is_some_and(|id| s.label(id) == live_label)
    })?;
    let id = live.cards().first().and_then(|c| c["id"].as_str()).unwrap_or("t1").to_owned();
    // Exports until the tab's card has `label` (asleep, or live again); the last export if it never does.
    let until = |inst: &mut Instance, what: &str, label: &str| {
        inst.export_until(&format!("{name}-{what}"), |s| s.label(&id) == label)
    };
    let want_cards = |labels: [&String; 3]| labels.map(|l| format!("{id}:{l}")).join(" | ");
    let title = format!("{case} {view}");

    match case {
        "manual" | "idle" => {
            if case == "manual" {
                inst.send(&format!("SLEEPNODE {id}"))?;
            } else {
                // The 3 s is the app's own idle clock (the first sweep starts it, the threshold is 2 s).
                send_all(&mut inst, &["IDLESWEEP 9999", "SLEEP 3", "IDLESWEEP 2"])?;
            }
            let asleep = until(&mut inst, "asleep", &asleep_label)?;
            // Opening the card wakes it; the open is a click on the card in Graph view.
            send_all(&mut inst, &["GRAPHOPEN", "WAITTABS"])?;
            let woken = until(&mut inst, "woken", &live_label)?;
            quit(&mut inst, &title);
            let got = [&live, &asleep, &woken].map(cards).join(" | ");
            check_eq(
                &format!("{title}: tab, asleep, woken again"),
                want_cards([&live_label, &asleep_label, &live_label]),
                got,
            );
        }
        "scroll" => {
            // Into Tab view, focus the page, scroll it, back to Graph view.
            let loaded = |lines: &[String]| lines.iter().any(|l| l.starts_with("load"));
            inst.step(&["GRAPHDOWN", "GRAPHOPEN"], "the tall page's load report", || loaded(&site.lines()))?;
            send_all(&mut inst, &["CLICK 300 200", "KEYPAGE PageDown", "KEYPAGE PageDown", "KEYPAGE PageDown"])?;
            // The page reports where the scroll settled (300 ms after the last key).
            // A report that never comes leaves a/b unset, and the check below fails with that.
            let _ = wait_page(&site, "the page's scroll report", |l| scroll_reports(l).0.is_some_and(|y| y > 0));
            send_all(&mut inst, &["ESCAPE", "WAITIDLE", &format!("SLEEPNODE {id}")])?;
            until(&mut inst, "asleep", &asleep_label)?;
            send_all(&mut inst, &["GRAPHOPEN", "WAITTABS"])?;
            // The woken page loads again and reports where it ended up (B, after its second load).
            let _ = wait_page(&site, "the woken page's scroll report", |l| scroll_reports(l).1.is_some());
            inst.send("WAITIDLE")?;
            inst.export_state(&format!("{name}-end"))?;
            quit(&mut inst, &title);
            let lines = site.lines();
            let (a, b) = scroll_reports(&lines);
            let ok = matches!((a, b), (Some(a), Some(b)) if a > 0 && (a - b).abs() <= 2);
            let show = |v: Option<i64>| v.map_or("none".to_owned(), |v| v.to_string());
            if !check(&format!("{title}: scroll kept across sleep (before={} after={})", show(a), show(b)), ok) {
                lines.iter().for_each(|l| println!("    page: {l}"));
            }
        }
        "history" | "crash" | "restart" => {
            // Two more pages in Tab view, then sleep (history) or kill the engine host (crash), then Back twice.
            inst.step(&["GRAPHDOWN", "GRAPHOPEN"], "the first page to load", || site.hits("/") > 0)?;
            // Each page settles before the next is typed: a url typed over a load in progress reads as its redirect.
            inst.send("WAITIDLE")?;
            for page in ["p2", "p3"] {
                let path = format!("/{page}");
                let typed = format!("ADDRESSBAR {}", site.url(&path));
                let before = site.hits(&path);
                inst.step(&[&typed, "WAITIDLE", "ENTER"], &format!("a request for {path}"), || {
                    site.hits(&path) > before
                })?;
                inst.send("WAITIDLE")?;
            }
            inst.export_state(&format!("{name}-pre"))?;
            let before = site.hits("/p3");
            let urls = match case {
                "restart" => {
                    // Quit here; the second instance restores the tab on the same profile.
                    quit(&mut inst, &title);
                    let mut second = Instance::launch(exe, &profile, work, &format!("{name}-2.log"))?;
                    send_all(&mut second, &["WAITTABS", "WAITIDLE"])?;
                    second
                        .step(&["GRAPHDOWN", "GRAPHOPEN"], "the restored page to load", || site.hits("/p3") > before)?;
                    let woken = until(&mut second, "woken", &live_label)?;
                    let mut urls = vec![last_segment(&woken)];
                    urls.extend(back_and_forward(&mut second, woken.focused_url().to_owned())?);
                    quit(&mut second, &title);
                    urls
                }
                _ => {
                    if case == "crash" {
                        // The app restarts the tab at its URL.
                        kill_hosts(work, inst.pid(), view)?;
                        inst.step(&["WAITTABS"], "the restarted tab to load", || site.hits("/p3") > before)?;
                    } else {
                        send_all(&mut inst, &["ESCAPE", "WAITIDLE", &format!("SLEEPNODE {id}")])?;
                        until(&mut inst, "asleep", &asleep_label)?;
                        send_all(&mut inst, &["GRAPHOPEN", "WAITTABS"])?;
                    }
                    let woken = until(&mut inst, "woken", &live_label)?;
                    let mut urls = vec![last_segment(&woken)];
                    urls.extend(back_and_forward(&mut inst, woken.focused_url().to_owned())?);
                    quit(&mut inst, &title);
                    urls
                }
            };
            let got = format!("woken={} back={} back={} forward={}", urls[0], urls[1], urls[2], urls[3]);
            check_eq(
                &format!("{title}: Back and Forward after sleep and wake"),
                "woken=p3 back=p2 back=/ forward=p2".to_owned(),
                got,
            );
        }
        "dirty" => {
            // Both waits are the app's timers or a negative window: the 3 s outlasts the idle threshold, and the tab must
            // then STAY awake for `ASLEEP_WAIT`.
            send_all(&mut inst, &["IDLESWEEP 9999", "SLEEP 3", "IDLESWEEP 2", ASLEEP_WAIT])?;
            let stay = inst.export_state(&format!("{name}-stay"))?;
            inst.send(&format!("SLEEPNODE {id}"))?;
            let slept = until(&mut inst, "slept", &asleep_label)?;
            quit(&mut inst, &title);
            check_eq(
                &format!("{title}: idle sleep skipped the dirty tab, manual Sleep did not"),
                format!("{id}:{live_label} | {id}:{asleep_label}"),
                [&stay, &slept].map(cards).join(" | "),
            );
        }
        other => {
            return Err(io::Error::other(format!("unknown case {other}")));
        }
    }
    Ok(())
}

pub(crate) fn sleep(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "sleep") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let cases = std::env::var("CASES").unwrap_or_else(|_| "manual idle scroll dirty".into());
    for case in cases.split_whitespace() {
        for view in views() {
            let _t = Section::start(&format!("sleep-{case}-{view}"));
            guarded(&format!("{case} {view}"), || run_case(&exe, &work, case, &view));
        }
    }
    finish(&work)
}
