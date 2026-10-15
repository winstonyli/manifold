//! `gate trail`: Back and Forward over a tab's pages and over opened-from, end to end (spec
//! docs/superpowers/specs/2026-10-14-trail-design.md, "Tests"). One throwaway profile seeded with tab A on a local site
//! served by this process (every page titled by its path). From A, NEWTAB opens tab B (parented to the focused tab) at
//! the home page /b1; B goes to /b2 and /b3. Back twice walks B's pages, a third Back focuses A (opened-from, and A
//! records `left_to`), Forward on A returns to B, and a new page from /b1 makes a sibling of /b2 (EXPORTSTATE
//! `trail`).
//!
//! A failing check does not stop the run; a runner error (a timeout, a launch failure) prints an `ERROR:` line, counts
//! as a failure and ends the run early. The instance is killed on every exit path (`Instance`'s Drop plus the job).

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::check::{check_is, exit_code, failures, note_failure, Section};
use crate::http::{response, Server};
use crate::instance::{log_tail, seed_settings, Instance};
use crate::link::{bad, no_verdict_words};
use crate::state::{State, TAB};
use crate::windows_main::{manifold_exe, work_dir};

/// How long the instance may take to exit after QUIT.
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);

struct Run {
    exe: PathBuf,
    work: PathBuf,
    profile: PathBuf,
    /// `http://127.0.0.1:<port>` of the site.
    base: String,
}

impl Run {
    /// Settings (home page /b1, so NEWTAB opens B there) and a session holding tab A at /a.
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, "webview2", &format!(", \"home_page\": \"{}/b1\"", self.base))?;
        let seed = self.profile.join("seed.json");
        let tab = serde_json::json!({"id": "a", "type": TAB, "url": format!("{}/a", self.base), "view_kind": "webview2",
            "pos": [0.0, 0.0]});
        std::fs::write(&seed, serde_json::json!({ "nodes": [tab] }).to_string())?;
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

    /// Types `path` on the site into the focused tab's address bar and goes there.
    fn go(&self, inst: &mut Instance, path: &str) -> io::Result<()> {
        inst.send(&format!("ADDRESSBAR {}{path}", self.base))?;
        inst.send("WAITIDLE")?;
        inst.send("ENTER")
    }
}

/// Tab `id`'s entry in EXPORTSTATE's `trail`.
fn trail_of<'a>(s: &'a State, id: &str) -> Option<&'a Value> {
    s.0["trail"].get(id)
}

/// The id of the one trail whose cursor URL ends with `path`.
fn trail_at(s: &State, path: &str) -> Option<String> {
    let trails = s.0["trail"].as_object()?;
    let ids: Vec<&String> =
        trails.iter().filter(|(_, t)| t["url"].as_str().is_some_and(|u| u.ends_with(path))).map(|(id, _)| id).collect();
    match ids.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

fn field<'a>(s: &'a State, id: &str, key: &str) -> Option<&'a Value> {
    trail_of(s, id).map(|t| &t[key])
}

fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
    let at = |s: &State, path: &str| s.focused_url().ends_with(path);

    next("open");
    let mut inst = Instance::launch(&run.exe, &run.profile, &run.work, "trail.log")?;
    for c in ["WAITTABS", "GRAPHRIGHT", "GRAPHOPEN", "WAITIDLE"] {
        inst.send(c)?;
    }
    let s = inst.export_until("a-open", |s| at(s, "/a") && trail_at(s, "/a").is_some())?;
    let a = trail_at(&s, "/a").ok_or_else(|| bad("tab A has no trail".to_string()))?;
    // NEWTAB leaves focus on A; NEXTTAB (creation order) moves it to B.
    for c in ["NEWTAB", "WAITTABS", "NEXTTAB", "WAITIDLE"] {
        inst.send(c)?;
    }
    let s = inst.export_until("b-open", |s| at(s, "/b1") && trail_at(s, "/b1").is_some_and(|b| b != a))?;
    let b = trail_at(&s, "/b1").ok_or_else(|| bad("tab B has no trail".to_string()))?;

    next("pages");
    for p in ["/b2", "/b3"] {
        run.go(&mut inst, p)?;
        inst.export_until(&format!("at{p}"), |s| at(s, p))?;
    }
    let s = inst.export_until("b3", |s| {
        field(s, &b, "entries") == Some(&3.into()) && field(s, &b, "depth") == Some(&2.into())
    })?;
    check_is("B's trail is /b1 /b2 /b3", Some(3), field(&s, &b, "entries").and_then(Value::as_u64));

    next("back");
    inst.send("GOBACK")?;
    let s = inst.export_until("back1", |s| at(s, "/b2") && field(s, &b, "depth") == Some(&1.into()))?;
    check_is("Forward is offered on /b2", Some(true), field(&s, &b, "forward").and_then(Value::as_bool));
    inst.send("GOBACK")?;
    let s = inst.export_until("back2", |s| at(s, "/b1") && field(s, &b, "depth") == Some(&0.into()))?;
    check_is(
        "Back at B's first page goes on (opened-from)",
        Some(true),
        field(&s, &b, "back").and_then(Value::as_bool),
    );
    inst.send("GOBACK")?;
    let s = inst.export_until("parent", |s| at(s, "/a"))?;
    check_is("A remembers it was left for B", Some(b.as_str()), field(&s, &a, "left_to").and_then(Value::as_str));
    check_is("Forward is offered on A", Some(true), field(&s, &a, "forward").and_then(Value::as_bool));

    next("forward");
    inst.send("GOFORWARD")?;
    let s = inst.export_until("child", |s| at(s, "/b1"))?;
    check_is("Forward returned to B's page", Some(0), field(&s, &b, "depth").and_then(Value::as_u64));

    next("fork");
    run.go(&mut inst, "/b4")?;
    let s = inst.export_until("fork", |s| at(s, "/b4") && field(s, &b, "entries") == Some(&4.into()))?;
    check_is("/b4 is a child of /b1", Some(1), field(&s, &b, "depth").and_then(Value::as_u64));
    check_is("the Back chip shows the branch marker", Some(true), field(&s, &b, "back_fork").and_then(Value::as_bool));
    inst.quit(QUIT_TIMEOUT).map(|_| ())
}

pub fn trail(parent: Option<PathBuf>) -> ExitCode {
    let mut sec = Some(Section::start("setup"));
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let work = match work_dir(parent, "trail") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let server = match Server::start(Arc::new(|line: &str| {
        let path = line.split(' ').nth(1).unwrap_or("/");
        let body = format!("<!doctype html><title>{path}</title><p>{path}</p>");
        response("200 OK", &[], "text/html", body.as_bytes())
    })) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("site: {e}");
            return ExitCode::from(1);
        }
    };
    let run = Run { exe, profile: work.join("profile"), work, base: format!("http://127.0.0.1:{}", server.port()) };
    if let Err(e) = run.setup().and_then(|()| drive(&run, &mut sec)) {
        println!("ERROR: trail stopped early: {e}");
        note_failure();
    }
    if failures() > 0 {
        println!("--- trail.log\n{}", no_verdict_words(&log_tail(&run.work.join("trail.log"), 30)));
    }
    drop(sec);
    drop(server);
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&run.work);
    }
    exit_code()
}
