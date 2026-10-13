//! `gate json-pan [MODE] [SECONDS] [--samply]` (was scripts/json-pan-profile.sh and .py): times, or profiles with
//! samply, panning a big JSON level. MODE is `pan` (default), `zoom`, `reveal-numbers` or `reveal-objects`;
//! SECONDS (default 20) sets how long the pan or zoom runs, one command per frame (60 a second).
//!
//! The app runs headless on an isolated profile folder, driven by a command file (`--script`, `--profile-dir`: the
//! elevated samply task can set neither stdin nor APPDATA). `pan` and `zoom` use a tree 3 levels deep with 12 children
//! per level (`JSON_BRANCH`, default 12; 27 gives ~20k containers) and reveal the 12 x 12 depth-2 cards, which brings a
//! few thousand cards into play. The reveal modes open `{"items": [...]}` of `REVEAL_N` (default 200000) numbers or
//! objects and select the last entry, which pages the array to every row.
//!
//! Without `--samply` it runs the copied release exe once and appends `TIME_ONLY mode=… seconds=… commands=… ms=…
//! exit=… selected=…` to target/json-time.log (an empty `selected` on a reveal run means the level never opened and
//! nothing was timed). The cost per command is the difference between two runs of different SECONDS divided by the
//! difference in commands, so the ~6 s setup cancels out. With `--samply` it records through the elevated `Samply` task
//! (~/.claude/SAMPLY.md) to target/json-profile/profile.json.gz; keep SECONDS near 10, since samply's kernel trace
//! wrote 7 GB in 15 minutes for a 20 s run. Needs a release exe (`CARGO_PROFILE_RELEASE_DEBUG=true cargo build
//! --release -p manifold` for names in a profile). Check the machine is quiet first (CPU under ~20%) and say so with
//! any numbers. The run is at BelowNormal.

use std::fmt::Write as _;
use std::io::Write as _;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

use serde_json::{json, Value};

const BELOW_NORMAL: u32 = 0x4000;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn leaf(n: usize) -> Value {
    json!({"id": n, "name": format!("item {n}"), "score": n as f64 * 0.37, "tags": ["a", "b"], "ok": n.is_multiple_of(2)})
}

fn tree(depth: usize, branch: usize, n: usize) -> Value {
    if depth == 0 {
        return leaf(n);
    }
    Value::Object((0..branch).map(|i| (format!("k{i}"), tree(depth - 1, branch, n * branch + i))).collect())
}

/// The JSON file for `mode`.
fn fixture(mode: &str, branch: usize, reveal_n: usize) -> Value {
    match mode {
        "reveal-numbers" => json!({"items": (0..reveal_n).map(|n| n as f64 * 0.37).collect::<Vec<_>>()}),
        "reveal-objects" => json!({"items": (0..reveal_n).map(leaf).collect::<Vec<_>>()}),
        _ => tree(3, branch, 0),
    }
}

/// The headless command file: open `fixture`, enter its level, reveal, then pan or zoom for `seconds`.
/// `state` is where a reveal run exports its state.
fn script(fixture: &Path, state: &Path, seconds: f64, mode: &str, branch: usize, reveal_n: usize) -> String {
    let reveal = mode.starts_with("reveal-");
    let mut s = String::from("RESIZE 1400 900\n");
    let _ = writeln!(s, "ADDFILE {} 500 350", fixture.display());
    s.push_str(if reveal { "WAITIDLE 60\n" } else { "SLEEP 1\n" }); // a big file loads before the double-click
    s.push_str("DOUBLECLICK 500 350\nSLEEP 2\n");
    if reveal {
        let _ = writeln!(s, "JSONSELECT /items/{}\nWAITIDLE 60\nEXPORTSTATE {}", reveal_n - 1, state.display());
    } else {
        for i in 0..branch {
            for j in 0..branch {
                let _ = writeln!(s, "JSONSELECT /k{i}/k{j}/k0"); // reveals (opens) the depth-2 card
            }
        }
    }
    s.push_str("SLEEP 2\n");
    // ~60 frames/s is not guaranteed: the loop is a fixed count, the profile's own timestamps give the real rate.
    let frames = (seconds * 60.0) as usize;
    if mode == "zoom" {
        for n in 0..frames {
            let dy = if (n / 60) % 2 == 0 { 12 } else { -12 };
            let _ = writeln!(s, "SCROLL 600 400 0 {dy}");
        }
    } else {
        s.push_str("MOUSEDOWN 1300 850\n");
        for n in 0..frames {
            let step = n % 120;
            let x = 1300 - 6 * (if (n / 120) % 2 == 0 { step } else { 120 - step }) as i64;
            let _ = writeln!(s, "MOUSEMOVE {x} 850");
        }
        s.push_str("MOUSEUP 700 850\n");
    }
    s.push_str("QUIT\n");
    s
}

/// The `selected` pointer of a state export, empty when there is none.
fn selected(state: &Path) -> String {
    let text = std::fs::read_to_string(state).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v["json_level"]["selected"].as_str().unwrap_or("").to_owned()
}

fn run(args: &[String]) -> Result<(), String> {
    let samply = args.iter().any(|a| a == "--samply");
    let mut pos = args.iter().filter(|a| !a.starts_with("--"));
    let mode = pos.next().map_or("pan", String::as_str);
    if !["pan", "zoom", "reveal-numbers", "reveal-objects"].contains(&mode) {
        return Err(format!("unknown MODE {mode}: pan, zoom, reveal-numbers or reveal-objects"));
    }
    let seconds: f64 = pos.next().map_or(Ok(20.0), |s| s.parse()).map_err(|e| format!("SECONDS: {e}"))?;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().map_err(|e| e.to_string())?;
    let release = repo.join("target/release/manifold.exe");
    if !release.exists() {
        return Err(format!("build the release exe first (see the module doc): {}", release.display()));
    }
    let out = repo.join("target/json-profile");
    let _ = std::fs::remove_dir_all(&out);
    let appdata = out.join("appdata");
    let io = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(&appdata).map_err(io)?;
    std::fs::write(appdata.join("settings.json"), "{\"has_shown_first_launch_demo\": true}\n").map_err(io)?;
    let (branch, reveal_n) = (env_usize("JSON_BRANCH", 12), env_usize("REVEAL_N", 200_000));
    let fixture_path = out.join("fixture.json");
    std::fs::write(&fixture_path, fixture(mode, branch, reveal_n).to_string()).map_err(io)?;
    let state = out.join("state.json");
    let commands = script(&fixture_path, &state, seconds, mode, branch, reveal_n);
    let command_file = out.join("commands.txt");
    std::fs::write(&command_file, &commands).map_err(io)?;
    let exe = out.join("manifold.exe");
    std::fs::copy(&release, &exe).map_err(io)?;
    let app_args = |c: &mut Command| {
        c.arg("--headless").arg("--profile-dir").arg(&appdata).arg("--script").arg(&command_file);
    };
    if samply {
        let ps1 = std::env::var_os("USERPROFILE").map(|h| Path::new(&h).join(".claude/samply/profile.ps1"));
        let ps1 = ps1.filter(|p| p.exists()).ok_or("~/.claude/samply/profile.ps1 not found: see SAMPLY.md")?;
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-File"]).arg(ps1).arg("-Exe").arg(&exe);
        c.arg("-Out").arg(out.join("profile.json.gz")).arg("-ExeArgs");
        let list = format!("--headless,--profile-dir,{},--script,{}", appdata.display(), command_file.display());
        let status = c.arg(list).creation_flags(BELOW_NORMAL).stdin(Stdio::null()).status().map_err(io)?;
        return if status.success() { Ok(()) } else { Err(format!("profile.ps1: {status}")) };
    }
    let log = std::fs::File::create(out.join("run.log")).map_err(io)?;
    let mut c = Command::new(&exe);
    app_args(&mut c);
    let start = Instant::now();
    let status = c
        .creation_flags(BELOW_NORMAL)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(io)?)
        .stderr(log)
        .status()
        .map_err(io)?;
    let line = format!(
        "TIME_ONLY mode={mode} seconds={seconds} commands={} ms={} exit={} selected={}",
        commands.lines().count(),
        start.elapsed().as_millis(),
        status.code().unwrap_or(-1),
        selected(&state)
    );
    println!("{line}");
    let mut log =
        std::fs::OpenOptions::new().create(true).append(true).open(repo.join("target/json-time.log")).map_err(io)?;
    writeln!(log, "{line}").map_err(io)
}

pub fn json_pan(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_has_branch_cubed_leaves_and_the_script_reveals_each_depth_2_card() {
        let t = tree(3, 3, 0);
        assert_eq!(t["k2"]["k1"]["k0"]["id"], 3 * 3 * 2 + 3);
        let s = script(Path::new("f.json"), Path::new("s.json"), 1.0, "pan", 3, 10);
        assert_eq!(s.matches("JSONSELECT /k").count(), 9);
        assert_eq!(s.matches("MOUSEMOVE").count(), 60);
        assert!(s.starts_with("RESIZE 1400 900\nADDFILE f.json") && s.ends_with("QUIT\n"));
        let r = script(Path::new("f.json"), Path::new("s.json"), 1.0, "reveal-numbers", 3, 10);
        assert!(r.contains("JSONSELECT /items/9\nWAITIDLE 60\nEXPORTSTATE s.json"));
        assert_eq!(script(Path::new("f"), Path::new("s"), 2.0, "zoom", 3, 10).matches("SCROLL").count(), 120);
    }
}
