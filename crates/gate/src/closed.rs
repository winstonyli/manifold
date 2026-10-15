//! `gate closed`: the closed field end to end (spec docs/superpowers/specs/2026-10-14-closed-field-design.md,
//! "Testing"). One fresh profile seeded with a group holding a note, an outside note linked to the inner
//! note, and 21 more notes with text. Instance A closes the group, reopens it (same ids, the link back), then two
//! instances on the one profile close notes in turn: both list the same history, newest first, and when the 21st
//! root is closed both evict the same oldest one. Two instances on one profile share its replica id (`replica` file),
//! so they count as one replica: the stage drives reopens with `REOPENNODE`, which ignores who closed a node.
//!
//! A failing check does not stop the run; a runner error (a timeout, a launch failure) prints an `ERROR:` line, counts
//! as a failure and ends the run early. Instances are killed on every exit path (`Instance`'s Drop plus the job).

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::check::{check, check_is, exit_code, failures, note_failure, Section};
use crate::instance::{log_tail, seed_settings, Instance};
use crate::link::{bad, no_verdict_words};
use crate::state::{State, GROUP, NOTE};
use crate::windows_main::{manifold_exe, work_dir};

/// How long an instance may take to exit after QUIT.
const QUIT_TIMEOUT: Duration = Duration::from_secs(60);
/// `store::model::TYPE_LINK`; the gate crate does not depend on the manifold crate.
const LINK: &str = "application/vnd.manifold.link";
/// `MAX_CLOSED` in `closed.rs`.
const MAX_CLOSED: usize = 20;

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
        inst.send("WAITTABS")?;
        inst.send("WAITIDLE")?;
        Ok(inst)
    }

    fn stop(mut inst: Instance) -> io::Result<()> {
        inst.quit(QUIT_TIMEOUT).map(|_| ())
    }

    /// The profile's settings and the session built from the seed (`--import-json`, which gives fresh ids; a
    /// cross-level link is fine there, `import` does not check scope).
    fn setup(&self) -> io::Result<()> {
        seed_settings(&self.profile, "webview2", "")?;
        let seed = self.profile.join("seed.json");
        let mut nodes = vec![
            serde_json::json!({"id": "g", "type": GROUP, "name": "G", "pos": [0.0, 0.0]}),
            serde_json::json!({"id": "in", "type": NOTE, "name": "inside", "text": "inside", "group": "g", "pos": [0.0, 0.0]}),
            serde_json::json!({"id": "out", "type": NOTE, "name": "out", "text": "outside", "pos": [400.0, 0.0]}),
            serde_json::json!({"id": "l", "type": LINK, "from": "in", "to": "out"}),
        ];
        nodes.extend((0..=MAX_CLOSED).map(|n| {
            serde_json::json!({"id": format!("n{n}"), "type": NOTE, "name": format!("n{n}"), "text": format!("note {n}"),
                "pos": [n as f64 * 50.0, 300.0]})
        }));
        std::fs::write(&seed, serde_json::json!({ "nodes": nodes }).to_string())?;
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

/// The ids in EXPORTSTATE's `closed`, newest first.
fn closed_ids(s: &State) -> Vec<String> {
    s.0["closed"].as_array().map_or_else(Vec::new, |a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
}

/// The id of the one top-level card named `name`.
fn named_card(s: &State, name: &str) -> io::Result<String> {
    let ids: Vec<&str> = s.cards().iter().filter(|c| c["name"] == name).filter_map(|c| c["id"].as_str()).collect();
    match ids.as_slice() {
        [one] => Ok(one.to_string()),
        _ => Err(bad(format!("expected one card named {name}, found {}", ids.len()))),
    }
}

/// The `shown` flag EXPORTSTATE gives the link with id `link`, if the link is exported at all.
fn link_shown(s: &State, link: &str) -> Option<bool> {
    s.0["links"].as_array()?.iter().find(|l| l["id"] == link)?["shown"].as_bool()
}

/// The id of the link joining `a` and `b`, either way round.
fn link_of(s: &State, a: &str, b: &str) -> Option<String> {
    let l = s.0["links"]
        .as_array()?
        .iter()
        .find(|k| (k["from"] == a && k["to"] == b) || (k["from"] == b && k["to"] == a))?;
    l["id"].as_str().map(str::to_owned)
}

fn drive(run: &Run, sec: &mut Option<Section>) -> io::Result<()> {
    let mut next = |name: &str| *sec = sec.take().map(|s| s.next(name));

    next("close-reopen");
    let mut a = run.start("a.log")?;
    let s0 = a.export_until("a0", |s| s.cards().iter().any(|c| c["name"] == "n20"))?;
    let g = s0.groups().into_iter().next().ok_or_else(|| bad("no group".to_string()))?;
    let out = named_card(&s0, "out")?;
    let inside = s0.0["mirror_groups"]
        .as_object()
        .and_then(|m| m.iter().find(|(_, v)| **v == g.as_str()))
        .map(|(k, _)| k.clone())
        .ok_or_else(|| bad("no member in the group".to_string()))?;
    let link = link_of(&s0, &inside, &out).ok_or_else(|| bad("the seeded link is missing".to_string()))?;
    let ids: Vec<String> = (0..=MAX_CLOSED).map(|n| named_card(&s0, &format!("n{n}"))).collect::<io::Result<_>>()?;
    check_is("nothing is closed at the start", Vec::<String>::new(), closed_ids(&s0));
    a.send(&format!("CLOSENODE {g}"))?;
    let s1 = a.export_until("a1", |s| closed_ids(s) == [g.clone()])?;
    check("the group is gone from the graph", s1.card(&g).is_none());
    check_is("its link is kept but hidden", Some(false), link_shown(&s1, &link));
    a.send(&format!("REOPENNODE {g}"))?;
    a.send("WAITIDLE")?;
    let s2 = a.export_until("a2", |s| s.card(&g).is_some() && closed_ids(s).is_empty())?;
    check_is("the same link is shown again", Some(true), link_shown(&s2, &link));
    check_is("the member is back in the same group", g.clone(), py(&s2.0["mirror_groups"][inside.as_str()]));

    next("two-instances");
    let mut b = run.start("b.log")?;
    for (n, id) in ids[..MAX_CLOSED].iter().enumerate() {
        let (who, other) = if n % 2 == 0 { (&mut a, &mut b) } else { (&mut b, &mut a) };
        who.send(&format!("CLOSENODE {id}"))?;
        who.send("WAITIDLE")?;
        // The next close is the other instance's: let it see this one first, so the stamps order the closes.
        other.export_until(&format!("sync{n}"), |s| closed_ids(s).first() == Some(id))?;
    }
    let want: Vec<String> = ids[..MAX_CLOSED].iter().rev().cloned().collect();
    let sa = a.export_until("a3", |s| closed_ids(s) == want)?;
    let sb = b.export_until("b3", |s| closed_ids(s) == want)?;
    check_is("A lists the closes newest first", want.clone(), closed_ids(&sa));
    check_is("B lists the same", want.clone(), closed_ids(&sb));

    next("eviction");
    a.send(&format!("CLOSENODE {}", ids[MAX_CLOSED]))?;
    let oldest = ids[0].clone();
    let evicted = |s: &State| closed_ids(s).len() == MAX_CLOSED && closed_ids(s)[0] == ids[MAX_CLOSED];
    let sa = a.export_until("a4", evicted)?;
    let sb = b.export_until("b4", evicted)?;
    check_is("both agree after the 21st close", closed_ids(&sa), closed_ids(&sb));
    check("the oldest was evicted on both", !closed_ids(&sa).contains(&oldest) && !closed_ids(&sb).contains(&oldest));
    let session = b.export_session("b-session")?;
    check(
        "the evicted node is not in the export",
        session["nodes"].as_array().is_some_and(|n| n.iter().all(|n| n["name"] != "n0")),
    );
    Run::stop(b)?;
    Run::stop(a)
}

/// A JSON value as text: a string's own text, anything else as JSON.
fn py(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_string)
}

pub fn closed(parent: Option<PathBuf>) -> ExitCode {
    let mut sec = Some(Section::start("setup"));
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let work = match work_dir(parent, "closed") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let run = Run { exe, profile: work.join("profile"), work };
    if let Err(e) = run.setup().and_then(|()| drive(&run, &mut sec)) {
        println!("ERROR: closed stopped early: {e}");
        note_failure();
    }
    if failures() > 0 {
        for l in ["a.log", "b.log"] {
            println!("--- {l}\n{}", no_verdict_words(&log_tail(&run.work.join(l), 20)));
        }
    }
    drop(sec);
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&run.work);
    }
    exit_code()
}
