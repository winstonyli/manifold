//! `gate unknown-type`: a node of a type this build doesn't know, end to end (spec
//! docs/superpowers/specs/2026-10-14-unknown-type-card-design.md, "Testing"). Two headless runs on one fresh profile
//! seeded with a tab, a group and one `application/x-from-the-future` node. The first moves, renames, links,
//! duplicates and groups the unknown card; the second checks that all of it persisted, then closes it (it stays
//! stored, hidden) and reopens it from Recently Closed with its id, group and link.
//!
//! A failing check does not stop the run; a runner error (a timeout, a launch failure) prints an `ERROR:` line, counts
//! as a failure and ends the run early. Instances are killed on every exit path (`Instance`'s Drop plus the job).

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::check::{check, check_eq, exit_code, failures, note_failure, Section};
use crate::instance::{log_tail, seed_settings, Instance};
use crate::link::{bad, no_verdict_words, num};
use crate::state::{State, GROUP, TAB};
use crate::windows_main::{manifold_exe, work_dir};

/// How long an instance may take to exit after QUIT.
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);
/// The seeded node's type: one no build knows.
const FUTURE: &str = "application/x-from-the-future";
/// The row EXPORTSTATE reports it as (`store::model::TYPE_UNKNOWN`).
const SENTINEL: &str = "application/vnd.manifold.unknown";

/// The run's instances and files.
struct Run {
    exe: PathBuf,
    work: PathBuf,
    profile: PathBuf,
}

impl Run {
    /// One instance on the profile (BelowNormal, in the job), its tabs up.
    fn start(&self, log: &str) -> io::Result<Instance> {
        let mut inst = Instance::launch(&self.exe, &self.profile, &self.work, log)?;
        inst.settle()?;
        Ok(inst)
    }

    fn stop(mut inst: Instance) -> io::Result<()> {
        inst.quit(QUIT_TIMEOUT).map(|_| ())
    }

    /// The profile's settings and the session built from the seed (`--import-json`, which gives fresh ids).
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, "webview2", "")?;
        let seed = self.profile.join("seed.json");
        let doc = serde_json::json!({"nodes": [
            {"id": "t", "type": TAB, "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]},
            {"id": "g", "type": GROUP, "name": "G", "pos": [300.0, 300.0]},
            {"id": "u", "type": FUTURE, "pos": [400.0, 0.0]},
        ]});
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

/// The id of the one top-level card whose `type` is `ty`.
fn only(s: &State, ty: &str) -> io::Result<String> {
    let ids: Vec<&str> = s.cards().iter().filter(|c| c["type"] == ty).filter_map(|c| c["id"].as_str()).collect();
    match ids.as_slice() {
        [one] => Ok(one.to_string()),
        _ => Err(bad(format!("expected one {ty} card, found {}", ids.len()))),
    }
}

/// Whether EXPORTSTATE's `links` holds a link between `a` and `b`, either way round.
fn linked(s: &State, a: &str, b: &str) -> bool {
    s.0["links"]
        .as_array()
        .is_some_and(|l| l.iter().any(|k| (k["from"] == a && k["to"] == b) || (k["from"] == b && k["to"] == a)))
}

/// The top-level cards of the unknown row named `name`.
fn named(s: &State, name: &str) -> usize {
    s.cards().iter().filter(|c| c["type"] == SENTINEL && c["name"] == name).count()
}

/// The stored nodes of type `FUTURE` named `name`, from an EXPORTSESSION.
fn stored_named(session: &Value, name: &str) -> usize {
    session["nodes"].as_array().map_or(0, |n| n.iter().filter(|n| n["type"] == FUTURE && n["name"] == name).count())
}

fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));
    next("run1-edit");
    let mut i = run.start("log1")?;
    let s0 = i.export_until("s0", |s| s.cards().iter().any(|c| c["type"] == SENTINEL))?;
    let (u, t, g) = (only(&s0, SENTINEL)?, only(&s0, TAB)?, only(&s0, GROUP)?);
    let card = s0.require_card(&u).map_err(bad)?;
    check_eq("the unknown node is a card that keeps its stored type", FUTURE.to_string(), py(&card["stored_type"]));
    i.send(&format!("MOVENODE {u} 500 50"))?;
    let at = |s: &State| {
        let pos = &s.0["mirror_pos"][u.as_str()];
        Some((num(&pos[0]).ok()?, num(&pos[1]).ok()?))
    };
    let s1 = i.export_until("s1", |s| at(s) == Some((500.0, 50.0)))?;
    check("a move writes its pos", at(&s1) == Some((500.0, 50.0)));
    i.send(&format!("RENAMEGROUP {u} Future"))?;
    i.send(&format!("LINK {u} {t}"))?;
    i.send(&format!("DUPLICATENODE {u}"))?;
    i.send("WAITIDLE")?;
    let s2 = i.export_until("s2", |s| named(s, "Future") == 2 && linked(s, &u, &t))?;
    check_eq("a rename names it", "Future".to_string(), py(&s2.card(&u).map_or(Value::Null, |c| c["name"].clone())));
    check("it links to a tab", linked(&s2, &u, &t));
    check_eq("Duplicate makes a second unknown card with its name", 2, named(&s2, "Future"));
    i.send(&format!("MOVETOGROUP {u} {g}"))?;
    let s3 = i.export_until("s3", |s| py(&s.0["mirror_groups"][u.as_str()]) == g)?;
    check_eq("it moves into a group", g.clone(), py(&s3.0["mirror_groups"][u.as_str()]));
    Run::stop(i)?;

    next("run2-restore");
    let mut i = run.start("log2")?;
    let s4 = i.export_state("s4")?;
    check_eq("its group persists", g.clone(), py(&s4.0["mirror_groups"][u.as_str()]));
    check("its link persists", linked(&s4, &u, &t));
    let session = i.export_session("session4")?;
    check_eq("the document keeps its real type and name, the copy's too", 2, stored_named(&session, "Future"));
    i.send(&format!("CLOSENODE {u}"))?;
    let s5 = i.export_until("s5", |s| s.0["mirror_groups"].get(u.as_str()).is_none())?;
    check("Close hides it", s5.0["mirror_groups"].get(u.as_str()).is_none());
    check_eq("Recently Closed lists it", serde_json::json!([u]), s5.0["closed"].clone());
    let session = i.export_session("session5")?;
    check_eq("a closed node is not exported", 1, stored_named(&session, "Future"));
    i.send("REOPENTAB")?;
    i.send("WAITIDLE")?;
    let s6 = i.export_until("s6", |s| py(&s.0["mirror_groups"][u.as_str()]) == g)?;
    check_eq("Reopen brings the same node back into its group", g.clone(), py(&s6.0["mirror_groups"][u.as_str()]));
    check("its link to the tab is back", linked(&s6, &u, &t));
    let session = i.export_session("session6")?;
    check_eq("both are stored with the real type", 2, stored_named(&session, "Future"));
    Run::stop(i)
}

/// A JSON value as text: a string's own text, anything else as JSON.
fn py(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

pub fn unknown_type(parent: Option<PathBuf>) -> ExitCode {
    let mut sec = Some(Section::start("setup"));
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let work = match work_dir(parent, "unknown-type") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let run = Run { exe, profile: work.join("profile"), work };
    if let Err(e) = run.setup().and_then(|()| drive(&run, &mut sec)) {
        println!("ERROR: unknown-type stopped early: {e}");
        note_failure();
    }
    if failures() > 0 {
        for l in ["log1", "log2"] {
            println!("--- {l}\n{}", no_verdict_words(&log_tail(&run.work.join(l), 20)));
        }
    }
    drop(sec);
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&run.work);
    }
    exit_code()
}
