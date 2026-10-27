//! `gate two`: two headless instances (later a third, then a fourth, one at a time) on one isolated profile, exercising
//! the store's multi-writer merge end to end (was scripts/two-instance-test.sh; plan
//! docs/superpowers/plans/2026-10-10-rust-gate-two.md). The scenarios run in the script's order, 1, 2, 8 to 12, 3 to 7,
//! each building on the document the previous ones left. A failed precondition (`require`) stops the run at once; a
//! failed observation (`observe`) lets its scenario finish, then the run stops: later scenarios would only cascade.
//! `VIEW_KIND` (default webview2) is the engine new tabs start in, which scenarios 3, 4, 5 and 7 navigate.

mod groups;
mod restart;
mod web;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::check::{check, exit_code, failures, note_failure, Marks};
use crate::instance::{log_tail, remove_dir_with_retries, seed_settings, wait_file, Instance};
use crate::link::no_verdict_words;
use crate::nonce::{Counters, Site};
use crate::session::{self, Field};
use crate::windows_main::{manifold_exe, work_dir};

/// An export the app never writes (the script's `wait_file`: 300 x 0.2 s).
const WAIT_FILE: Duration = Duration::from_secs(60);
/// Between poll rounds (the script's `sleep 0.2`): the app's cadence, not a fixed wait.
const PAUSE: Duration = Duration::from_millis(200);
/// A positive wait's deadline; it only matters when the check is failing.
const POSITIVE: Duration = Duration::from_secs(90);
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Who {
    A,
    B,
    C,
    D,
}

impl Who {
    fn name(self) -> &'static str {
        ["A", "B", "C", "D"][self as usize]
    }
    fn tag(self) -> &'static str {
        ["a", "b", "c", "d"][self as usize]
    }
}

/// Why the run stopped: a failed check (already printed) or a runner error (printed by `two`).
enum Halt {
    Failed,
    Error(io::Error),
}

impl From<io::Error> for Halt {
    fn from(e: io::Error) -> Halt {
        Halt::Error(e)
    }
}

type Step<T = ()> = Result<T, Halt>;

/// Prints `PASS:`/`FAIL:` and, on a failure, `detail` indented (never the verdict words: gate all counts them).
fn verdict(label: &str, ok: bool, detail: &str) -> bool {
    let ok = check(label, ok);
    if !ok {
        for line in no_verdict_words(detail).lines() {
            println!("    {line}");
        }
    }
    ok
}

/// A precondition: a failure stops the run here.
fn require(label: &str, ok: bool, detail: &str) -> Step {
    if verdict(label, ok, detail) {
        Ok(())
    } else {
        Err(Halt::Failed)
    }
}

/// An observation: a failure is counted, the scenario goes on to its end, then the run stops (`Two::scenario`).
fn observe(label: &str, ok: bool, detail: &str) -> bool {
    verdict(label, ok, detail)
}

/// Polls `cond` every PAUSE until it holds or `timeout` passes; whether it held.
fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(PAUSE);
    }
}

/// Waits for an export and parses it.
fn read_json(path: &Path) -> Step<Value> {
    wait_file(path, WAIT_FILE)?;
    let v = serde_json::from_slice(&fs::read(path)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?;
    Ok(v)
}

/// Informational lines, indented, never the verdict words.
fn info(text: &str) {
    for line in no_verdict_words(text).lines() {
        println!("    {line}");
    }
}

struct Two {
    exe: PathBuf,
    profile: PathBuf,
    /// Exports, test files and the instances' logs.
    out: PathBuf,
    insts: [Option<Instance>; 4],
    site: Option<Site>,
    marks: Marks,
}

impl Two {
    fn launch(&mut self, who: Who) -> Step {
        let inst = Instance::launch(&self.exe, &self.profile, &self.out, &format!("{}.log", who.tag()))?;
        println!("{}: pid {}", who.name(), inst.pid());
        self.insts[who as usize] = Some(inst);
        Ok(())
    }

    fn inst(&mut self, who: Who) -> io::Result<&mut Instance> {
        self.insts[who as usize].as_mut().ok_or_else(|| io::Error::other(format!("{} was never launched", who.name())))
    }

    fn send(&mut self, who: Who, line: &str) -> Step {
        self.inst(who)?.send(line)?;
        Ok(())
    }

    fn path(&self, file: &str) -> PathBuf {
        self.out.join(file)
    }

    /// Removes `<name>.json`, then sends `<cmd> <path>` without waiting, so A's and B's exports run side by side.
    fn request(&mut self, who: Who, cmd: &str, name: &str) -> Step<PathBuf> {
        let path = self.path(&format!("{name}.json"));
        let _ = fs::remove_file(&path);
        self.send(who, &format!("{cmd} {}", path.display()))?;
        Ok(path)
    }

    fn session(&mut self, who: Who, name: &str) -> Step<Value> {
        let p = self.request(who, "EXPORTSESSION", name)?;
        read_json(&p)
    }

    /// A no-op that still draws a frame (unlike SLEEP), so the instance's per-frame poll_store can pick up the other
    /// instance's edits. One file per instance.
    fn tick(&mut self, who: Who) -> Step {
        let p = self.path(&format!("{}-tick.json", who.tag()));
        self.send(who, &format!("EXPORTSTATE {}", p.display()))
    }

    /// ADDRESSBAR, WAITIDLE, ENTER: a typed navigation as the script types it.
    fn navigate(&mut self, who: Who, url: &str) -> Step {
        self.send(who, &format!("ADDRESSBAR {url}"))?;
        self.send(who, "WAITIDLE")?;
        self.send(who, "ENTER")
    }

    fn write_json(&self, file: &str, v: &Value) -> Step<PathBuf> {
        let path = self.path(file);
        fs::write(&path, v.to_string())?;
        Ok(path)
    }

    fn kill(&mut self, who: Who) {
        if let Some(i) = self.insts[who as usize].as_mut() {
            i.kill();
        }
    }

    fn quit(&mut self, who: Who) -> Step {
        self.inst(who)?.quit(QUIT_TIMEOUT)?;
        Ok(())
    }

    fn counters(&self) -> io::Result<Counters> {
        self.site.as_ref().map(|s| s.counters().clone()).ok_or_else(|| io::Error::other("the nonce site is not up"))
    }

    fn port(&self) -> io::Result<u16> {
        self.site.as_ref().map(Site::port).ok_or_else(|| io::Error::other("the nonce site is not up"))
    }

    /// Prints the header, runs the body, and prints `scenario N: ok<suffix>` only if nothing in it failed; any failure
    /// (a precondition's at once, an observation's at the end) stops the run.
    fn scenario(&mut self, mark: &str, n: u32, title: &str, body: impl FnOnce(&mut Two) -> Step<String>) -> Step {
        self.marks.mark(mark);
        println!("=== scenario {n}: {title} ===");
        let before = failures();
        let suffix = body(self)?;
        if failures() > before {
            return Err(Halt::Failed);
        }
        println!("scenario {n}: ok{suffix}");
        Ok(())
    }

    /// The script's wait_agree: exports both sessions until their shared nodes agree. A precondition. Returns A's.
    fn wait_agree(&mut self, label: &str, tag: &str) -> Step<Value> {
        let deadline = Instant::now() + POSITIVE;
        loop {
            self.tick(Who::A)?;
            self.tick(Who::B)?;
            let af = self.request(Who::A, "EXPORTSESSION", &format!("a{tag}"))?;
            let bf = self.request(Who::B, "EXPORTSESSION", &format!("b{tag}"))?;
            let (a, b) = (read_json(&af)?, read_json(&bf)?);
            match session::same_nodes(&a, &b) {
                Ok(n) => {
                    require(label, true, "")?;
                    info(&format!("{n} nodes"));
                    return Ok(a);
                }
                Err(e) if Instant::now() >= deadline => {
                    require(label, false, &e)?;
                }
                Err(_) => sleep(PAUSE),
            }
        }
    }

    /// Scenario 4's failure dump: each instance's tab urls and the site's requests.
    fn dump_urls(&mut self, tag: &str) -> Step {
        for who in [Who::A, Who::B] {
            let doc = self.session(who, &format!("{}{tag}-urls", who.tag()))?;
            for n in session::nodes(&doc) {
                if let Some(url) = n.get("url").and_then(Value::as_str) {
                    info(&format!("{} {} {url}", who.name(), n.get("id").and_then(Value::as_str).unwrap_or("?")));
                }
            }
        }
        info(&self.counters()?.report());
        Ok(())
    }

    /// The script's wait_field (wait_mirror for groups): until each id's field is the same in what A shows, what B
    /// shows and A's document. A precondition. Returns (A's EXPORTSTATE, the document).
    fn wait_field(&mut self, field: Field, label: &str, tag: &str, ids: &[&str]) -> Step<(Value, Value)> {
        let deadline = Instant::now() + POSITIVE;
        loop {
            self.tick(Who::A)?;
            self.tick(Who::B)?;
            let doc = self.session(Who::A, &format!("{tag}-doc"))?;
            self.tick(Who::B)?;
            let af = self.request(Who::A, "EXPORTSTATE", &format!("{tag}-a-state"))?;
            let bf = self.request(Who::B, "EXPORTSTATE", &format!("{tag}-b-state"))?;
            let (a, b) = (read_json(&af)?, read_json(&bf)?);
            match session::field_agree(field, &a, &b, &doc, ids) {
                Ok(lines) => {
                    require(label, true, "")?;
                    info(&lines.join("\n"));
                    return Ok((a, doc));
                }
                Err(lines) if Instant::now() >= deadline => {
                    require(label, false, &lines.join("\n"))?;
                }
                Err(_) => sleep(PAUSE),
            }
        }
    }

    /// The script's new_node, always on A: the id of the one node `cmd` adds. A precondition.
    fn new_node(&mut self, cmd: &str, label: &str, tag: &str) -> Step<String> {
        let before = self.session(Who::A, &format!("{tag}-before"))?;
        self.send(Who::A, cmd)?;
        let after = self.session(Who::A, &format!("{tag}-after"))?;
        let got = session::new_id(&before, &after);
        require(label, got.is_ok(), got.as_ref().err().map_or("", String::as_str))?;
        Ok(got.unwrap_or_default())
    }

    /// The script's show_local / show_local_field: each instance's own value right after a race. Informational:
    /// whether either had picked up the other's edit yet depends on frame timing.
    fn show_local(&mut self, field: Field, tag: &str, what: &str, ids: &[&str]) -> Step {
        let af = self.request(Who::A, "EXPORTSTATE", &format!("{tag}-a-local"))?;
        let bf = self.request(Who::B, "EXPORTSTATE", &format!("{tag}-b-local"))?;
        let (a, b) = (read_json(&af)?, read_json(&bf)?);
        for id in ids {
            let show =
                |s: &Value| session::field_of(s, field, id).map_or_else(|| "<absent>".to_string(), |v| v.to_string());
            info(&format!("{tag}: right after the {what}, {id}: A shows {}, B shows {}", show(&a), show(&b)));
        }
        Ok(())
    }
}

fn run(t: &mut Two) -> Step {
    t.launch(Who::A)?;
    t.launch(Who::B)?;
    groups::s1(t)?;
    groups::s2(t)?;
    groups::s8(t)?;
    groups::s9(t)?;
    groups::s10(t)?;
    groups::s11(t)?;
    groups::s12(t)?;
    web::s3(t)?;
    web::s4(t)?;
    web::s5(t)?;
    restart::s6(t)?;
    restart::s7(t)
}

pub fn two(parent: Option<PathBuf>) -> ExitCode {
    let fatal = |e: String| {
        eprintln!("{e}");
        ExitCode::from(1)
    };
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => return fatal(e),
    };
    // The script's first mark comes after its freshness check.
    let marks = Marks::start("setup");
    let work = match work_dir(parent, "two") {
        Ok(w) => w,
        Err(e) => return fatal(format!("work dir: {e}")),
    };
    let (out, profile) = (work.join("out"), work.join("profile"));
    let view_kind = std::env::var("ENGINE").unwrap_or_else(|_| "webview2".to_string());
    if let Err(e) = fs::create_dir_all(&out).and_then(|()| seed_settings(&profile, &view_kind, "")) {
        return fatal(format!("setup: {e}"));
    }
    println!("two-instance test: exe {} profile {} engine {view_kind}", exe.display(), profile.display());
    let mut t = Two { exe, profile, out, insts: [None, None, None, None], site: None, marks };
    if let Err(Halt::Error(e)) = run(&mut t) {
        // Not a verdict line: gate all counts those words, and this failure is counted here.
        println!("ERROR: two stopped early: {e}");
        note_failure();
    }
    t.marks.end();
    let logs: Vec<PathBuf> = t.insts.iter().flatten().map(|i| i.log.clone()).collect();
    let site = t.site.as_ref().map(|s| s.counters().report());
    drop(t); // kills any instance still up and stops the site
    if failures() > 0 {
        for log in &logs {
            println!("--- tail of {}\n{}", log.display(), no_verdict_words(&log_tail(log, 30)));
        }
        if let Some(s) = site {
            println!("--- nonce site requests\n{}", no_verdict_words(&s));
        }
    }
    if std::env::var_os("KEEP").is_none() {
        remove_dir_with_retries(&work);
    } else {
        println!("kept {}", work.display());
    }
    println!("two-instance test: {}", if failures() == 0 { "ok" } else { "failed" });
    exit_code()
}
