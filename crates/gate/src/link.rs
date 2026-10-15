//! `gate link`: scripts/link-probe.sh as a Rust flow. Five headless instances on one fresh profile (seeded with three
//! about:blank tabs), three drag loops, 30 checks in six sections. Labels and TIMING names are the script's. Each
//! python snippet of the script is an inline closure over the exported `State` (the comment above each says which).
//!
//! A failing check does not stop the run; a runner error (a timeout, a launch failure) prints an `ERROR:` line, counts
//! as a failure and ends the run early. Instances are killed on every exit path (`Instance`'s Drop plus the job).

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::check::{check_eq, exit_code, failures, note_failure, Section};
use crate::instance::{log_tail, seed_settings, Instance};
use crate::state::{round_half_even, State};
use crate::windows_main::{manifold_exe, work_dir};

/// How long an instance may take to exit after QUIT.
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Python's `round(v)` of a JSON number, as an int.
fn rnd(v: &Value) -> io::Result<i64> {
    v.as_f64().map(|f| round_half_even(f) as i64).ok_or_else(|| bad(format!("not a number: {v}")))
}

pub(crate) fn num(v: &Value) -> io::Result<f64> {
    v.as_f64().ok_or_else(|| bad(format!("not a number: {v}")))
}

/// `[x, y]` rounded, as `print(round(e[0]), round(e[1]))`.
fn rounded_pair(v: &Value) -> io::Result<(i64, i64)> {
    Ok((rnd(&v[0])?, rnd(&v[1])?))
}

/// The links array of an EXPORTSTATE.
fn links(s: &State) -> &[Value] {
    s.0["links"].as_array().map_or(&[], Vec::as_slice)
}

/// link-probe.sh `summary`: "<link count> <first link's id> <its from> <its to>" as (count, "id from to"); `-` for
/// each when there is no link.
fn summary(s: &State) -> (usize, String) {
    let l = links(s);
    let text = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_string);
    match l.first() {
        Some(f) => (l.len(), format!("{} {} {}", text(&f["id"]), text(&f["from"]), text(&f["to"]))),
        None => (0, "- - -".to_string()),
    }
}

/// Python's `str()` of a JSON value, as `print(d[key])` shows it; a missing key is `<missing>` (the python would
/// have raised KeyError and printed a traceback, so the check fails either way).
pub(crate) fn py_str(v: Option<&Value>) -> String {
    match v {
        None => "<missing>".to_string(),
        Some(Value::Null) => "None".to_string(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// link-probe.sh `field`: `print(d[key])`.
fn field(s: &State, key: &str) -> String {
    py_str(s.0.get(key))
}

/// Python truthiness of a JSON value (`if v`).
pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// link-probe.sh `rect_of`: the card's rect, each value rounded, space-separated.
fn rect_of(s: &State, id: &str) -> io::Result<String> {
    let c = s.card(id).ok_or_else(|| bad(format!("no card {id}")))?;
    let r = c["rect"].as_array().ok_or_else(|| bad(format!("card {id} has no rect")))?;
    Ok(r.iter().map(|v| rnd(v).map(|i| i.to_string())).collect::<io::Result<Vec<_>>>()?.join(" "))
}

/// link-probe.sh `card_pt`: a card's centre (`screen`), rounded.
fn card_pt(s: &State, id: &str) -> io::Result<(i64, i64)> {
    rounded_pair(&s.card(id).ok_or_else(|| bad(format!("no card {id}")))?["screen"])
}

/// link-probe.sh `edge_pt`: the first link edge's (with `to`: the first running to card `to`) `key` point, rounded.
fn edge_pt(s: &State, key: &str, to: Option<&str>) -> io::Result<(i64, i64)> {
    let edges = s.0["edges"].as_array().map_or(&[][..], Vec::as_slice);
    let e = edges
        .iter()
        .find(|e| e["kind"] == "link" && to.is_none_or(|t| e["to"] == t))
        .ok_or_else(|| bad(format!("no link edge (to {to:?})")))?;
    rounded_pair(&e[key])
}

/// link-probe.sh `clear_of_cards`: "clear" when the first link edge's `screen` point lies in no card's rect.
fn clear_of_cards(s: &State) -> io::Result<&'static str> {
    let (x, y) = {
        let e = s.0["edges"].as_array().and_then(|a| a.iter().find(|e| e["kind"] == "link"));
        let e = e.ok_or_else(|| bad("no link edge".into()))?;
        (num(&e["screen"][0])?, num(&e["screen"][1])?)
    };
    let mut on_card = false;
    for c in s.all_cards() {
        let r = &c["rect"];
        on_card |= num(&r[0])? <= x && x <= num(&r[2])? && num(&r[1])? <= y && y <= num(&r[3])?;
    }
    Ok(if on_card { "on a card" } else { "clear" })
}

/// link-probe.sh `handle_cards`: whether the cards with a non-empty `link_handles` entry are exactly all the cards.
fn handle_cards(s: &State) -> String {
    let mut with: Vec<&str> = s.0["link_handles"]
        .as_object()
        .map(|o| o.iter().filter(|(_, v)| truthy(v)).map(|(k, _)| k.as_str()).collect())
        .unwrap_or_default();
    with.sort_unstable();
    // Every card on screen, the Default jar's included: `link_handles` covers every node card.
    let mut all: Vec<&str> = s.all_cards().iter().filter_map(|c| c["id"].as_str()).collect();
    all.sort_unstable();
    if with == all { "True" } else { "False" }.to_string()
}

/// The three seeded tabs by position (the seed's ids are replaced on import): sorted by (round(y / 50), x), so a is
/// top left, b top right, c below a. (The script's first python snippet.)
fn abc(s: &State) -> io::Result<(usize, Vec<String>)> {
    let mut keyed = Vec::new();
    for c in s.cards() {
        let row = round_half_even(num(&c["screen"][1])? / 50.0) as i64;
        keyed.push((row, num(&c["screen"][0])?, c["id"].as_str().unwrap_or_default().to_string()));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    Ok((keyed.len(), keyed.into_iter().map(|(_, _, id)| id).collect()))
}

/// Python's `round(f)` as an int.
fn rnd_f(f: f64) -> i64 {
    round_half_even(f) as i64
}

/// The script's `drag_to X0 Y0 X1 Y1`: hover, press, then six steps to (X1, Y1) (bash integer arithmetic, truncating
/// division); the button stays down. Ends with WAITIDLE.
fn drag_to(i: &mut Instance, from: (i64, i64), to: (i64, i64)) -> io::Result<()> {
    i.send(&format!("MOUSEMOVE {} {}", from.0, from.1))?;
    i.send(&format!("MOUSEMOVE {} {}", from.0, from.1))?;
    i.send(&format!("MOUSEDOWN {} {}", from.0, from.1))?;
    steps(i, from, to)?;
    i.send("WAITIDLE")
}

/// Six MOUSEMOVEs from `from` toward `to`: `from + (to - from) * k / 6` for k in 1..=6.
fn steps(i: &mut Instance, from: (i64, i64), to: (i64, i64)) -> io::Result<()> {
    for k in 1..=6 {
        i.send(&format!("MOUSEMOVE {} {}", from.0 + (to.0 - from.0) * k / 6, from.1 + (to.1 - from.1) * k / 6))?;
    }
    Ok(())
}

/// Puts cards `a`, `b` and `c` at fixed world points, a 240-point gap apart (a top left, b to its right, c below), and
/// any other card (the Default jar's) off to the right. Each launch settles the cards afresh (a force layout whose
/// result varies with the ids), and often packs them edge to edge: a card then lies across a link curve, within the
/// 5-point hit radius of it, or over a handle, and takes the press the probe aims there, so the link is not grabbed or
/// clicked. Every instance that drags or clicks a link starts with this.
fn spread(i: &mut Instance, [a, b, c]: [&str; 3]) -> io::Result<()> {
    let s = i.export_state("spread")?;
    for (id, x, y) in [(a, -150, -100), (b, 250, -100), (c, -150, 200)] {
        i.send(&format!("MOVENODE {id} {x} {y}"))?;
    }
    let others = s.all_cards().iter().filter_map(|card| card["id"].as_str()).filter(|id| ![a, b, c].contains(id));
    for (n, id) in others.enumerate() {
        i.send(&format!("MOVENODE {id} 600 {}", 250 + 200 * n))?;
    }
    i.send("WAITIDLE")
}

/// Replaces the verdict words, so a log line that happens to contain one is not counted by gate all.
pub(crate) fn no_verdict_words(text: &str) -> String {
    text.replace("PASS", "P-ASS").replace("FAIL", "F-AIL")
}

/// The run's instances and files.
struct Run {
    exe: PathBuf,
    work: PathBuf,
    profile: PathBuf,
}

impl Run {
    /// scripts/link-probe.sh `start`: one instance on the profile (BelowNormal, in the job), its tabs up.
    fn start(&self, log: &str) -> io::Result<Instance> {
        let mut inst = Instance::launch(&self.exe, &self.profile, &self.work, log)?;
        inst.send("WAITTABS")?;
        inst.send("SLEEP 1")?;
        Ok(inst)
    }

    /// scripts/link-probe.sh `stop`.
    fn stop(mut inst: Instance) -> io::Result<()> {
        inst.quit(QUIT_TIMEOUT).map(|_| ())
    }

    /// The setup: the profile's settings and the session built from the seed (`--import-json`).
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, "webview2", "")?;
        let seed = self.profile.join("seed.json");
        let tab = |id: &str, x: f64, y: f64| {
            serde_json::json!({"id": id, "type": "application/vnd.manifold.tab", "url": "about:blank",
                "view_kind": "webview2", "pos": [x, y]})
        };
        let doc = serde_json::json!({"nodes": [tab("t1", 0.0, 0.0), tab("t2", 400.0, 0.0), tab("t3", 0.0, 400.0)]});
        std::fs::write(&seed, doc.to_string())?;
        let status = Command::new(&self.exe)
            .arg("--import-json")
            .arg(&seed)
            .arg("--out")
            .arg(self.profile.join("session.mnf"))
            .stdout(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("--import-json exited with {status}")))
        }
    }
}

/// Everything the verdicts read.
struct Exports {
    s1: State,
    s2: State,
    s3: State,
    s4: State,
    s5: State,
    s6: State,
    s7: State,
    s7m: State,
    s7t: State,
    s8: State,
    s9m: State,
    s9: State,
    s10f: State,
    s10m: State,
    s10: State,
    s10b: State,
    s10dm: State,
    s10d: State,
    s11m: State,
    s11: State,
    s12: State,
    ncards: usize,
    a: String,
    b: String,
    c: String,
}

/// The five runs; returns the states the verdicts read. The sections' TIMING lines are printed from here.
fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<Exports> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
    next("run1-link");
    let mut i = run.start("log1")?;
    let s0 = i.export_state("s0")?;
    let (ncards, ids) = abc(&s0)?;
    let [a, b, c]: [String; 3] =
        ids.try_into().map_err(|v: Vec<String>| bad(format!("expected 3 seeded cards, found {}", v.len())))?;
    i.send(&format!("LINK {a} {b}"))?;
    i.send("SLEEP 1")?;
    let s1 = i.export_state("s1")?;
    Run::stop(i)?;

    next("run2-restart-unlink");
    let mut i = run.start("log2")?;
    let s2 = i.export_state("s2")?;
    i.send(&format!("UNLINK {b} {a}"))?;
    i.send("SLEEP 1")?;
    let s3 = i.export_state("s3")?;
    Run::stop(i)?;

    next("run3-gestures");
    let mut i = run.start("log3")?;
    spread(&mut i, [&a, &b, &c])?;
    let s4 = i.export_state("s4")?;
    i.send(&format!("LINK {a} {b}"))?;
    i.send("SLEEP 1")?;
    let s5 = i.export_state("s5")?;
    // The link line's middle on screen (EXPORTSTATE edges), clear of both cards: a click there deletes it.
    // python: [e for e in edges if e["kind"]=="link"][0]["screen"] -> round(e[0]), round(e[1])
    let (lx, ly) = edge_pt(&s5, "screen", None)?;
    i.send(&format!("MOUSEMOVE {lx} {ly}"))?;
    i.send(&format!("CLICK {lx} {ly}"))?;
    i.send("SLEEP 1")?;
    let s6 = i.export_state("s6")?;
    // a's centre, a's right handle's centre and b's centre, then a drag from the handle onto b in six steps.
    // python: round() of c[a]'s screen, ((r[0]+r[2])/2, (r[1]+r[3])/2) of link_handles[a]["right"], c[b]'s screen.
    let (ax, ay) = card_pt(&s6, &a)?;
    let r = &s6.0["link_handles"][a.as_str()]["right"];
    let (hx, hy) = (rnd_f((num(&r[0])? + num(&r[2])?) / 2.0), rnd_f((num(&r[1])? + num(&r[3])?) / 2.0));
    let (bx, by) = card_pt(&s6, &b)?;
    i.send(&format!("MOUSEMOVE {ax} {ay}"))?;
    i.send(&format!("MOUSEMOVE {hx} {hy}"))?;
    i.send(&format!("MOUSEMOVE {hx} {hy}"))?;
    i.send(&format!("MOUSEDOWN {hx} {hy}"))?;
    steps(&mut i, (hx, hy), (bx, by))?;
    i.send(&format!("MOUSEUP {bx} {by}"))?;
    i.send("SLEEP 1")?;
    let s7 = i.export_state("s7")?;
    // The same drag, released 15 points left of b's left edge: within the drop tolerance of b, off every card.
    // python: r = b's rect; (round(r[0]) - 15, round((r[1]+r[3])/2))
    let rb = &s7.card(&b).ok_or_else(|| bad(format!("no card {b}")))?["rect"];
    let (nx, ny) = (rnd(&rb[0])? - 15, rnd_f((num(&rb[1])? + num(&rb[3])?) / 2.0));
    i.send(&format!("UNLINK {a} {b}"))?;
    i.send("SLEEP 1")?;
    i.send(&format!("MOUSEMOVE {ax} {ay}"))?;
    i.send(&format!("MOUSEMOVE {hx} {hy}"))?;
    i.send(&format!("MOUSEMOVE {hx} {hy}"))?;
    i.send(&format!("MOUSEDOWN {hx} {hy}"))?;
    steps(&mut i, (hx, hy), (nx, ny))?;
    i.send("SLEEP 1")?;
    let s7m = i.export_state("s7m")?;
    i.send(&format!("MOUSEUP {nx} {ny}"))?;
    i.send("SLEEP 1")?;
    let s7t = i.export_state("s7t")?;
    Run::stop(i)?;

    next("run4-persists");
    let mut i = run.start("log4")?;
    spread(&mut i, [&a, &b, &c])?;
    let s8 = i.export_state("s8")?;
    // Retarget (spec link-gestures, "Dragging a link's end"). The link is a -> b. Grabbed near b's end (t = 0.8) and
    // dropped on c, its b end moves to c: same Link node, now a -> c. Mid-drag a is the fixed end.
    let g = edge_pt(&s8, "near_to", None)?;
    let cc = card_pt(&s8, &c)?;
    drag_to(&mut i, g, cc)?;
    let s9m = i.export_state("s9m")?;
    i.send(&format!("MOUSEUP {} {}", cc.0, cc.1))?;
    i.send("WAITIDLE")?;
    let s9 = i.export_state("s9")?;
    // Esc mid-drag: the c end pulled toward b, Esc, then the release on b: the link stays a -> c.
    let g = edge_pt(&s9, "near_to", None)?;
    let bc = card_pt(&s9, &b)?;
    drag_to(&mut i, g, bc)?;
    // Over the fixed end's own card (a) the preview says the release is refused.
    let f = card_pt(&s9, &a)?;
    i.send(&format!("MOUSEMOVE {} {}", f.0, f.1))?;
    i.send("WAITIDLE")?;
    let s10f = i.export_state("s10f")?;
    i.send("ESCAPE")?;
    i.send("WAITIDLE")?;
    let s10m = i.export_state("s10m")?;
    i.send(&format!("MOUSEUP {} {}", bc.0, bc.1))?;
    i.send("WAITIDLE")?;
    let s10 = i.export_state("s10")?;
    // A second link a -> b, its b end dragged onto c: a already joins c, so the dragged link is deleted, a -> c stays.
    i.send(&format!("LINK {a} {b}"))?;
    i.send("SLEEP 1")?;
    let s10b = i.export_state("s10b")?;
    let g = edge_pt(&s10b, "near_to", Some(&b))?;
    let cc = card_pt(&s10b, &c)?;
    drag_to(&mut i, g, cc)?;
    let s10dm = i.export_state("s10dm")?;
    i.send(&format!("MOUSEUP {} {}", cc.0, cc.1))?;
    i.send("WAITIDLE")?;
    let s10d = i.export_state("s10d")?;
    // Released on empty canvas (b's column, c's row, clear of every card): the dragged link is deleted.
    let g = edge_pt(&s10d, "near_from", None)?;
    let ey = card_pt(&s10d, &c)?.1;
    drag_to(&mut i, g, (bc.0, ey))?;
    let s11m = i.export_state("s11m")?;
    i.send(&format!("MOUSEUP {} {ey}", bc.0))?;
    i.send("WAITIDLE")?;
    let s11 = i.export_state("s11")?;
    Run::stop(i)?;
    let mut i = run.start("log5")?;
    let s12 = i.export_state("s12")?;
    Run::stop(i)?;

    Ok(Exports {
        s1,
        s2,
        s3,
        s4,
        s5,
        s6,
        s7,
        s7m,
        s7t,
        s8,
        s9m,
        s9,
        s10f,
        s10m,
        s10,
        s10b,
        s10dm,
        s10d,
        s11m,
        s11,
        s12,
        ncards,
        a,
        b,
        c,
    })
}

/// The 30 checks, in the script's order. Each `want`/`got` pair is the script's `check NAME WANT GOT`.
fn verdicts(e: &Exports) -> io::Result<()> {
    let (a, b, c) = (&e.a, &e.b, &e.c);
    let (n1, t1) = summary(&e.s1);
    let (n2, t2) = summary(&e.s2);
    let id1 = t1.split(' ').next().unwrap_or_default().to_string();
    let (n3, _) = summary(&e.s3);
    let (n4, _) = summary(&e.s4);
    let (n5, _) = summary(&e.s5);
    let (n6, _) = summary(&e.s6);
    let (n7, t7) = summary(&e.s7);
    let (n7t, t7t) = summary(&e.s7t);
    let id7t = t7t.split(' ').next().unwrap_or_default().to_string();
    let (n8, t8) = summary(&e.s8);
    let id8 = t8.split(' ').next().unwrap_or_default().to_string();
    // The script's "$n $id $from $to" strings, rebuilt from the count and the "id from to" text.
    let full = |n: usize, t: &str| format!("{n} {t}");
    let ends = |t: &str| t.split(' ').skip(1).collect::<Vec<_>>().join(" ");
    check_eq("the session has the three seeded tabs", "3".to_string(), e.ncards.to_string());
    check_eq("LINK creates one link", "1".to_string(), n1.to_string());
    check_eq("it runs a -> b", format!("{a} {b}"), ends(&t1));
    check_eq("the link persists across a restart", "1".to_string(), n2.to_string());
    check_eq("with the same id and ends", format!("{id1} {a} {b}"), t2);
    check_eq("UNLINK (ends in the other order) removes it", "0".to_string(), n3.to_string());
    check_eq("the removal persists across a restart", "0".to_string(), n4.to_string());
    check_eq("LINK again", "1".to_string(), n5.to_string());
    check_eq("the link curve's middle is clear of both cards", "clear".to_string(), clear_of_cards(&e.s5)?.to_string());
    check_eq("a click on the link curve's middle removes it", "0".to_string(), n6.to_string());
    check_eq("a drag from a's handle onto b links a -> b", format!("1 {a} {b}"), format!("{n7} {}", ends(&t7)));
    check_eq("the card dragged from did not move", rect_of(&e.s6, a)?, rect_of(&e.s7, a)?);
    check_eq("the connect-drag ended", "None".to_string(), field(&e.s7, "connect_drag"));
    check_eq("every card lists handles mid-drag", "True".to_string(), handle_cards(&e.s7m));
    check_eq("a release 15 points outside b links a -> b", format!("1 {a} {b}"), format!("{n7t} {}", ends(&t7t)));
    check_eq("the dragged link persists across a restart", format!("1 {id7t}"), format!("{n8} {id8}"));
    let (n9, t9) = summary(&e.s9);
    let (n10, t10) = summary(&e.s10);
    let (n10b, _) = summary(&e.s10b);
    let (n10d, t10d) = summary(&e.s10d);
    let (n11, _) = summary(&e.s11);
    let (n12, _) = summary(&e.s12);
    let ac = format!("1 {id8} {a} {c}");
    check_eq("grabbed near b's end, a is the fixed end of the connect-drag", a.clone(), field(&e.s9m, "connect_drag"));
    check_eq("and the link being retargeted is the one that was drawn", id8.clone(), field(&e.s9m, "retarget"));
    check_eq("dropped on c, the link keeps its id and runs a -> c", ac.clone(), full(n9, &t9));
    check_eq(
        "over a valid card (c) the preview says retarget",
        "retarget".to_string(),
        field(&e.s9m, "retarget_preview"),
    );
    check_eq(
        "over the fixed card (a) the preview says refused",
        "refused".to_string(),
        field(&e.s10f, "retarget_preview"),
    );
    check_eq(
        "over the already-joined card (c) the preview says delete",
        "delete".to_string(),
        field(&e.s10dm, "retarget_preview"),
    );
    check_eq("over empty canvas the preview says delete", "delete".to_string(), field(&e.s11m, "retarget_preview"));
    check_eq(
        "the retarget ended",
        "None None".to_string(),
        format!("{} {}", field(&e.s9, "connect_drag"), field(&e.s9, "retarget")),
    );
    check_eq(
        "Esc ends the drag mid-way",
        "None None".to_string(),
        format!("{} {}", field(&e.s10m, "connect_drag"), field(&e.s10m, "retarget")),
    );
    check_eq("Esc then a release on b leaves a -> c", ac.clone(), full(n10, &t10));
    check_eq("a second link a -> b is made", "2".to_string(), n10b.to_string());
    check_eq("its b end dropped on c (already joined to a) deletes it, a -> c stays", ac, full(n10d, &t10d));
    check_eq("a release on empty canvas deletes the dragged link", "0".to_string(), n11.to_string());
    check_eq("the deletion persists across a restart", "0".to_string(), n12.to_string());
    Ok(())
}

pub fn link(parent: Option<PathBuf>) -> ExitCode {
    let mut sec = Some(Section::start("setup"));
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let work = match work_dir(parent, "link") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let run = Run { exe, profile: work.join("profile"), work };
    let result = run.setup().and_then(|()| drive(&run, &mut sec));
    match result {
        Ok(exports) => {
            sec = sec.take().map(|s| s.next("verdicts"));
            if let Err(e) = verdicts(&exports) {
                // Not a verdict line: gate all counts those words, and this failure is counted below.
                println!("ERROR: link verdicts stopped early: {e}");
                note_failure();
            }
        }
        Err(e) => {
            println!("ERROR: link stopped early: {e}");
            note_failure();
        }
    }
    if failures() > 0 {
        for l in ["log1", "log2", "log3", "log4", "log5"] {
            println!("--- {l}\n{}", no_verdict_words(&log_tail(&run.work.join(l), 20)));
        }
    }
    drop(sec);
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&run.work);
    }
    exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(json: &str) -> State {
        State::parse(json).unwrap()
    }

    #[test]
    fn seeded_cards_sort_by_row_then_x() {
        // rows: y/50 rounds half-even (25/50 = 0.5 -> 0, 75/50 = 1.5 -> 2), so a card at y 25 is in row 0.
        let s = state(
            r#"{"cards":[{"id":"c","screen":[10.0,450.0]},{"id":"b","screen":[400.0,25.0]},{"id":"a","screen":[10.0,10.0]}]}"#,
        );
        assert_eq!(abc(&s).unwrap(), (3, vec!["a".to_string(), "b".into(), "c".into()]));
    }

    #[test]
    fn summary_and_fields_print_like_python() {
        let s = state(r#"{"links":[{"id":"L","from":"a","to":"b"}],"connect_drag":null,"retarget":"L","ok":true}"#);
        assert_eq!(summary(&s), (1, "L a b".to_string()));
        assert_eq!(summary(&state(r#"{"links":[]}"#)), (0, "- - -".to_string()));
        assert_eq!(
            (field(&s, "connect_drag"), field(&s, "retarget"), field(&s, "ok")),
            ("None".into(), "L".into(), "True".into())
        );
        assert_eq!(field(&s, "nope"), "<missing>");
    }

    #[test]
    fn edges_rects_and_handles() {
        let s = state(
            r#"{"cards":[{"id":"a","rect":[0.5,1.5,100.0,50.0]},{"id":"b","rect":[200.0,0.0,300.0,50.0]}],
                "edges":[{"kind":"other","screen":[1,1]},{"kind":"link","to":"b","screen":[150.0,25.0],"near_to":[2.5,3.5]},
                         {"kind":"link","to":"c","screen":[1.0,1.0],"near_to":[9.0,9.0]}],
                "link_handles":{"a":{"right":[1,2,3,4]},"b":{}}}"#,
        );
        assert_eq!(rect_of(&s, "a").unwrap(), "0 2 100 50");
        assert_eq!(edge_pt(&s, "near_to", None).unwrap(), (2, 4));
        assert_eq!(edge_pt(&s, "near_to", Some("c")).unwrap(), (9, 9));
        assert_eq!(clear_of_cards(&s).unwrap(), "clear");
        assert_eq!(handle_cards(&s), "False"); // b's handles are empty
    }

    #[test]
    fn diagnostics_never_carry_the_verdict_words() {
        let t = no_verdict_words("PASS: x\nFAIL: y");
        assert!(!t.contains("PASS") && !t.contains("FAIL"));
    }
}
