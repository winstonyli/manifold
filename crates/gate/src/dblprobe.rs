//! `gate dblprobe`: how often a debug `DOUBLECLICK` on a group card fails to enter it (the smoke flake, README
//! "gate smoke is flaky"). One instance, one group card: each round double-clicks the card, notes whether the group
//! was entered, and goes back. Round kinds, in turn: click straight away; move the pointer onto the card and let a
//! frame draw first; click Reset View (the toolbar's fit button, which eases the camera) first, as the smoke sites do.
//! `ROUNDS` (default 100) counts all kinds together; prints one line per kind. Not part of `all`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use crate::check::{check, exit_code, failures, note_failure, Section};
use crate::instance::{seed_settings, Instance};
use crate::link::bad;
use crate::state::{GROUP, NOTE};
use crate::windows_main::{manifold_exe, work_dir};

/// The round kinds' names.
const NAMES: [&str; 3] = ["straight", "hover first", "after Reset View"];

/// The toolbar's back button, which leaves a group.
const BACK: &str = "CLICK 14 16";

fn drive(exe: &Path, work: &Path) -> io::Result<()> {
    let profile = work.join("profile");
    seed_settings(&profile, "webview2", "")?;
    let seed = profile.join("seed.json");
    let nodes = serde_json::json!([
        {"id": "g", "type": GROUP, "name": "G", "pos": [0.0, 0.0]},
        {"id": "in", "type": NOTE, "name": "inside", "text": "inside", "group": "g", "pos": [0.0, 0.0]},
        {"id": "n", "type": NOTE, "name": "n", "text": "outside", "pos": [400.0, 0.0]},
    ]);
    std::fs::write(&seed, serde_json::json!({ "nodes": nodes }).to_string())?;
    let status = Command::new(exe)
        .arg("--import-json")
        .arg(&seed)
        .arg("--out")
        .arg(profile.join("session.mnf"))
        .stdout(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(bad(format!("--import-json exited with {status}")));
    }
    let mut a = Instance::launch(exe, &profile, work, "a.log")?;
    a.send("WAITTABS")?;
    a.send("WAITIDLE")?;
    let rounds: usize = std::env::var("ROUNDS").ok().and_then(|r| r.parse().ok()).unwrap_or(100);
    let mut missed = [0usize; 3];
    let mut tried = [0usize; 3];
    for round in 0..rounds {
        let kind = round % 3;
        if kind == 2 {
            a.send("ZOOMOUT")?;
            a.send("CLICK 326 17")?;
        }
        // The camera eases after leaving a group: aim only once two exports agree on every card.
        let mut prev = String::new();
        let s = a.export_until("top", |s| {
            let now: String = s.all_cards().iter().map(|c| format!("{}{};", c["id"], c["rect"])).collect();
            let steady = s.current_group().is_none() && now == prev;
            prev = now;
            steady
        })?;
        let g = s.groups().into_iter().next().ok_or_else(|| bad("no group".to_string()))?;
        let at = s.xy(&g).ok_or_else(|| bad("no group card position".to_string()))?;
        if kind == 2 {
            if let Some(secs) = std::env::var("PAUSE").ok().filter(|s| !s.is_empty()) {
                a.send(&format!("SLEEP {secs}"))?;
            }
        }
        if kind == 1 {
            a.send(&format!("MOUSEMOVE {at}"))?;
            a.send("SLEEP 0.3")?;
        }
        a.send(&format!("DOUBLECLICK {at}"))?;
        a.send("WAITIDLE")?;
        tried[kind] += 1;
        let entered = a.export_until("after", |_| true)?.current_group() == Some(g.as_str());
        if !entered {
            missed[kind] += 1;
            let rect_of = |s: &crate::state::State| s.card(&g).map(|c| c["rect"].to_string());
            let now = a.export_until("miss", |_| true)?;
            println!(
                "round {round} ({}): not entered; aimed at {at} when the card was {:?}, now {:?}",
                NAMES[kind],
                rect_of(&s),
                rect_of(&now)
            );
        }
        a.send(BACK)?;
        a.export_until("back", |s| s.current_group().is_none())?;
    }
    for (i, name) in NAMES.iter().enumerate() {
        println!("{name}: {} missed of {}", missed[i], tried[i]);
    }
    check("the probe ran", true);
    a.quit(Duration::from_secs(60)).map(|_| ())
}

pub fn dblprobe(parent: Option<PathBuf>) -> ExitCode {
    let _sec = Section::start("dblprobe");
    let (exe, work) = match (manifold_exe(), work_dir(parent, "dblprobe")) {
        (Ok(e), Ok(w)) => (e, w),
        (Err(e), _) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
        (_, Err(e)) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    if let Err(e) = drive(&exe, &work) {
        println!("ERROR: dblprobe stopped early: {e}");
        note_failure();
    }
    if std::env::var_os("KEEP").is_none() {
        let _ = std::fs::remove_dir_all(&work);
    }
    let _ = failures();
    exit_code()
}
