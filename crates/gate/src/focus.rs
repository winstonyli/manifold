//! `gate focus-loss` (was scripts/focus-loss-probe.sh; `CASES="connect card"`, `NO_STEAL=1` is the control run that must
//! FAIL): losing window focus mid-drag ends the drag. winit's `WindowEvent::Focused(false)` reaches
//! `App::on_window_focus`, so the button-up that went to another window cannot act when it arrives (spec
//! 2026-10-07-link-gestures). Each case is one headless instance on its own fresh profile
//! seeded with two about:blank tabs, a (left) and b.
//!
//! - `connect`: a MOUSEDOWN on a's right handle and MOUSEMOVEs halfway to b: `connect_drag` is a. The steal; then,
//!   polled, `connect_drag` is null and there are no links. A late MOUSEUP over b, then a click on b: still no link.
//! - `card`: a MOUSEDOWN on a's centre and MOUSEMOVEs until a's centre is over b: `dragging` is a and a moved. The steal;
//!   then, polled, `dragging` is null and a's document pos was saved (`mirror_pos`). MOUSEMOVEs down and away with the
//!   button still "down" (egui keeps it down across a focus loss): a stays put. Then back over b and a late MOUSEUP: a
//!   and b are still in no group and a hasn't moved.
//!
//! The steal (scripts/foreground.ps1, one process): gives the instance's (off-screen) window the OS foreground and
//! checks `GetForegroundWindow`, then at once takes it for a throwaway off-screen form, holds it 1 s, hands it back and
//! closes the form. Refused: that case prints SKIP. EXPORTSTATE has no window-focus field, so the app's side of the
//! focus change is seen only through the drag state.
//!
//! WARNING: steals the OS foreground for a few seconds per case. Keystrokes typed meanwhile go to the probe's off-screen
//! windows.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::Duration;

use serde_json::Value;

use crate::check::{check_eq, Section};
use crate::instance::Instance;
use crate::probes::{finish, guarded, quit, seed, setup};
use crate::state::{round_half_even, State};

/// Python's `str()` of a JSON value: `None` for null.
fn py(v: &Value) -> String {
    match v {
        Value::Null => "None".to_owned(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn field(s: &State, key: &str) -> String {
    py(&s.0[key])
}

fn nlinks(s: &State) -> usize {
    s.0["links"].as_array().map_or(0, Vec::len)
}

fn rounded(v: &Value) -> i64 {
    round_half_even(v.as_f64().unwrap_or(0.0)) as i64
}

/// The two cards, a the left one: ids and the points the drags use.
struct Cards {
    count: usize,
    a: String,
    b: String,
    /// a's centre
    ax: i64,
    ay: i64,
    /// a's right handle's centre
    hx: i64,
    hy: i64,
    /// b's centre
    bx: i64,
    by: i64,
}

fn cards(s: &State) -> Option<Cards> {
    let mut c: Vec<&Value> = s.cards();
    c.sort_by(|x, y| {
        x["screen"][0].as_f64().partial_cmp(&y["screen"][0].as_f64()).unwrap_or(std::cmp::Ordering::Equal)
    });
    let (ca, cb) = (c.first()?, c.get(1)?);
    let (a, b) = (ca["id"].as_str()?.to_owned(), cb["id"].as_str()?.to_owned());
    let r = &s.0["link_handles"][&a]["right"];
    let mid = |i: usize, j: usize| {
        round_half_even((r[i].as_f64().unwrap_or(0.0) + r[j].as_f64().unwrap_or(0.0)) / 2.0) as i64
    };
    Some(Cards {
        count: c.len(),
        a,
        b,
        ax: rounded(&ca["screen"][0]),
        ay: rounded(&ca["screen"][1]),
        hx: mid(0, 2),
        hy: mid(1, 3),
        bx: rounded(&cb["screen"][0]),
        by: rounded(&cb["screen"][1]),
    })
}

fn rect_of(s: &State, id: &str) -> String {
    let rect = s.rect(id).unwrap_or_default();
    rect.iter().map(|v| (round_half_even(*v) as i64).to_string()).collect::<Vec<_>>().join(" ")
}

fn groups(s: &State, a: &str, b: &str) -> String {
    format!("{} {}", py(&s.0["mirror_groups"][a]), py(&s.0["mirror_groups"][b]))
}

fn send_all(inst: &mut Instance, lines: &[String]) -> io::Result<()> {
    lines.iter().try_for_each(|l| inst.send(l))
}

fn lines(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| (*s).to_owned()).collect()
}

/// `key=value` in a helper's output line.
fn kv<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace().find_map(|w| w.strip_prefix(key).and_then(|r| r.strip_prefix('='))).unwrap_or("")
}

/// The foreground cycle. Returns false (after printing SKIP) if Windows refused it.
fn steal(script: &Path, pid: u32, case: &str) -> io::Result<bool> {
    // NO_STEAL=1 skips it, as a control: the drag then survives, and the checks after this FAIL.
    let cycle = if std::env::var_os("NO_STEAL").is_some() {
        "OK hwnd=0 before=0 lost=1 (NO_STEAL)".to_owned()
    } else {
        let out = Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(script)
            .args(["-ProcessId", &pid.to_string(), "-HoldSeconds", "1.0"])
            .stdin(Stdio::null())
            .output()?;
        String::from_utf8_lossy(&out.stdout).trim().replace('\r', "")
    };
    if !cycle.starts_with("OK") {
        println!("SKIP: {case}: could not take foreground: {cycle}");
        return Ok(false);
    }
    println!("INFO: {case}: {cycle}");
    check_eq(
        &format!("{case}: the instance's window had the foreground just before the steal"),
        kv(&cycle, "hwnd").to_owned(),
        kv(&cycle, "before").to_owned(),
    );
    check_eq(&format!("{case}: and lost it to the form"), "1".to_owned(), kv(&cycle, "lost").to_owned());
    Ok(true)
}

/// EXPORTSTATEs until `key` is null (`Focused(false)` is a window message; commands come through stdin).
fn poll_cleared(inst: &mut Instance, prefix: &str, key: &str) -> io::Result<State> {
    let mut last = inst.export_state(&format!("{prefix}_1"))?;
    for i in 2..=8 {
        if last.0[key].is_null() {
            break;
        }
        sleep(Duration::from_millis(250));
        last = inst.export_state(&format!("{prefix}_{i}"))?;
    }
    Ok(last)
}

fn launch(exe: &Path, work: &Path, case: &str) -> io::Result<Instance> {
    let profile = work.join(format!("appdata-{case}"));
    let tab = |id: &str, x: f64| {
        let mut n = crate::probes::tab_node(Some("about:blank"), "webview2");
        n["id"] = id.into();
        n["pos"] = serde_json::json!([x, 0.0]);
        n
    };
    seed(exe, &profile, "webview2", &[tab("t1", 0.0), tab("t2", 400.0)])?;
    let mut inst = Instance::launch(exe, &profile, work, &format!("{case}.log"))?;
    send_all(&mut inst, &lines(&["WAITTABS", "SLEEP 1"]))?;
    Ok(inst)
}

fn case_connect(exe: &Path, work: &Path, script: &Path) -> io::Result<()> {
    let mut inst = launch(exe, work, "connect")?;
    let s0 = inst.export_state("s0")?;
    let Some(c) = cards(&s0) else {
        println!("FAIL: connect: fewer than two cards");
        crate::check::note_failure();
        return Ok(());
    };
    check_eq("connect: the session has the two seeded tabs", 2, c.count);

    // Press a's right handle and drag halfway to b: over the canvas, button still down. Synthetic input, so the window
    // needs no focus for this; it gets the foreground just before it loses it.
    send_all(
        &mut inst,
        &[
            format!("MOUSEMOVE {} {}", c.ax, c.ay),
            format!("MOUSEMOVE {} {}", c.hx, c.hy),
            format!("MOUSEMOVE {} {}", c.hx, c.hy),
            format!("MOUSEDOWN {} {}", c.hx, c.hy),
        ],
    )?;
    for i in 1..=3 {
        inst.send(&format!("MOUSEMOVE {} {}", c.hx + (c.bx - c.hx) * i / 6, c.hy + (c.by - c.hy) * i / 6))?;
    }
    inst.send("SLEEP 1")?;
    let s1 = inst.export_state("s1")?;
    check_eq("connect: the handle press started a connect-drag from a", c.a.clone(), field(&s1, "connect_drag"));
    check_eq("connect: no links yet", 0, nlinks(&s1));

    if steal(script, inst.pid(), "connect")? {
        let s2 = poll_cleared(&mut inst, "s2", "connect_drag")?;
        check_eq("connect: losing focus cancelled the connect-drag", "None".to_owned(), field(&s2, "connect_drag"));
        check_eq("connect: and linked nothing", 0, nlinks(&s2));

        // The button-up that went elsewhere, arriving late over b: with the drag still live (NO_STEAL) this links a -> b.
        send_all(&mut inst, &[format!("MOUSEMOVE {} {}", c.bx, c.by), format!("MOUSEUP {} {}", c.bx, c.by)])?;
        inst.send("SLEEP 1")?;
        let s3 = inst.export_state("s3")?;
        check_eq(
            "connect: a late button-up over b links nothing",
            "0 None".to_owned(),
            format!("{} {}", nlinks(&s3), field(&s3, "connect_drag")),
        );
        // Then an ordinary click on b. (Not a discriminating check alone: a press-release without movement on a live
        // connect-drag is a click, which cancels it too.)
        send_all(&mut inst, &[format!("MOUSEDOWN {} {}", c.bx, c.by), format!("MOUSEUP {} {}", c.bx, c.by)])?;
        inst.send("SLEEP 1")?;
        let s4 = inst.export_state("s4")?;
        check_eq(
            "connect: a click on b after the focus loss links nothing",
            "0 None".to_owned(),
            format!("{} {}", nlinks(&s4), field(&s4, "connect_drag")),
        );
    }
    quit(&mut inst, "connect");
    Ok(())
}

fn case_card(exe: &Path, work: &Path, script: &Path) -> io::Result<()> {
    let mut inst = launch(exe, work, "card")?;
    let c0 = inst.export_state("c0")?;
    let Some(c) = cards(&c0) else {
        println!("FAIL: card: fewer than two cards");
        crate::check::note_failure();
        return Ok(());
    };
    check_eq("card: the session has the two seeded tabs", 2, c.count);
    check_eq("card: no groups to start with", "None None".to_owned(), groups(&c0, &c.a, &c.b));

    // Press a's centre and drag until its centre is over b (a release now would group them).
    send_all(
        &mut inst,
        &[
            format!("MOUSEMOVE {} {}", c.ax, c.ay),
            format!("MOUSEMOVE {} {}", c.ax, c.ay),
            format!("MOUSEDOWN {} {}", c.ax, c.ay),
        ],
    )?;
    for i in 1..=6 {
        inst.send(&format!("MOUSEMOVE {} {}", c.ax + (c.bx - c.ax) * i / 6, c.ay + (c.by - c.ay) * i / 6))?;
    }
    inst.send("SLEEP 1")?;
    let c1 = inst.export_state("c1")?;
    check_eq("card: the press started a card drag of a", c.a.clone(), field(&c1, "dragging"));
    check_eq("card: a follows the pointer", true, rect_of(&c0, &c.a) != rect_of(&c1, &c.a));

    if steal(script, inst.pid(), "card")? {
        let c2 = poll_cleared(&mut inst, "c2", "dragging")?;
        check_eq("card: losing focus ended the card drag", "None".to_owned(), field(&c2, "dragging"));
        let pos = py(&c2.0["mirror_pos"][&c.a]);
        check_eq(
            &format!("card: a's pos was saved where it was released ({pos})"),
            true,
            pos != py(&c0.0["mirror_pos"][&c.a]),
        );
        check_eq("card: and grouped nothing", "None None".to_owned(), groups(&c2, &c.a, &c.b));

        // The button-up went elsewhere, so the button is still down for the app: the pointer moves on, down and away.
        for i in 1..=3 {
            inst.send(&format!("MOUSEMOVE {} {}", c.bx, c.by + 60 * i))?;
        }
        inst.send("SLEEP 1")?;
        let c3 = inst.export_state("c3")?;
        check_eq("card: a stays put while the stale button is down", rect_of(&c2, &c.a), rect_of(&c3, &c.a));
        // Back over b and the late button-up: with the drag still live (NO_STEAL) this drops a on b, grouping them.
        send_all(&mut inst, &[format!("MOUSEMOVE {} {}", c.bx, c.by), format!("MOUSEUP {} {}", c.bx, c.by)])?;
        inst.send("SLEEP 1")?;
        let c4 = inst.export_state("c4")?;
        check_eq("card: the late button-up over b grouped nothing", "None None".to_owned(), groups(&c4, &c.a, &c.b));
        check_eq("card: a did not move", rect_of(&c2, &c.a), rect_of(&c4, &c.a));
        check_eq("card: no drag left", "None".to_owned(), field(&c4, "dragging"));
    }
    quit(&mut inst, "card");
    Ok(())
}

pub(crate) fn focus_loss(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "focus-loss") {
        Ok(v) => v,
        Err(code) => return code,
    };
    // target/debug/manifold.exe -> the repo root
    let Some(script) = exe.ancestors().nth(3).map(|root| root.join("scripts").join("foreground.ps1")) else {
        eprintln!("cannot find the repo root from {}", exe.display());
        return ExitCode::from(1);
    };
    let cases = std::env::var("CASES").unwrap_or_else(|_| "connect card".into());
    for case in cases.split_whitespace() {
        let _t = Section::start(&format!("focus-loss-{case}"));
        guarded(case, || match case {
            "connect" => case_connect(&exe, &work, &script),
            "card" => case_card(&exe, &work, &script),
            other => Err(io::Error::other(format!("unknown case {other}"))),
        });
    }
    finish(&work)
}
