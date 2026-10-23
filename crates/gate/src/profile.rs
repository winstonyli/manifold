//! `gate profile SCENARIO [SECONDS] [--samply] [--visible]` (was scripts/json-pan-profile.sh and .py): times, or profiles with
//! samply, one scripted scenario. SCENARIO is `json-pan`, `json-zoom`, `json-reveal-numbers`, `json-reveal-objects`
//! (panning, zooming and paging a big JSON level) or `graph-frames` (the Graph view with `GRAPH_NOTES` (default 80) notes at 25% zoom: pointer
//! moves, typing in a note and in the quick switcher, scrolling and a pan drag, a fifth of SECONDS each). SECONDS
//! (default 20) sets how long the pan, zoom or frames run, one command per frame
//! (60 a second).
//!
//! `graph-idle` adds the notes and then does nothing for SECONDS: its `idle` phase shows the redraws and CPU of a
//! loop with nothing to do. Without `--samply` the app also writes a timing trace (`MANIFOLD_TRACE`, target/profile/
//! trace.json, openable in Perfetto) and the run prints a table from it: per phase of the script (`MARK label`), the
//! redraws a second, the frame time, the input latency (an input's arrival to the end of the first frame started after
//! it, not counting the compositor and display) and the process CPU per second, then the five slowest frames.
//!
//! `--visible` runs a real window (`--debug-commands`, which tells egui it has focus, so the elevated samply task needs
//! no foreground) instead of the hidden headless one, to include the real present path; it pops up for the run.
//!
//! The app runs headless on an isolated profile folder, driven by a command file (`--script`, `--profile-dir`: the
//! elevated samply task can set neither stdin nor APPDATA). `json-pan` and `json-zoom` use a tree 3 levels deep with 12
//! children per level (`JSON_BRANCH`, default 12; 27 gives ~20k containers) and reveal the 12 x 12 depth-2 cards, which
//! brings a few thousand cards into play. The reveal scenarios open `{"items": [...]}` of `REVEAL_N` (default 200000)
//! numbers or objects and select the last entry, which pages the array to every row.
//!
//! Without `--samply` it runs the copied release exe once and appends `TIME_ONLY scenario=… seconds=… commands=…
//! ms=… exit=… result=…` to target/profile-time.log (`result` is the selected pointer of a reveal run, empty if the
//! level never opened and nothing was timed, or the card count of `graph-frames`). The cost per command is the
//! difference between two runs of different SECONDS divided by the difference in commands, so the ~6 s setup cancels
//! out. With `--samply` it records through the elevated `Samply` task (~/.claude/SAMPLY.md) to
//! target/profile/profile.json.gz; keep SECONDS near 10, since samply's kernel trace wrote 7 GB in 15 minutes for a 20 s
//! run. Needs a release exe (`CARGO_PROFILE_RELEASE_DEBUG=true cargo build --release -p manifold` for names in a
//! profile); it refuses an exe older than any source file under crates/. Check the machine is quiet first (CPU under ~20%) and say so with any numbers. The run is at
//! BelowNormal.

use std::fmt::Write as _;
use std::io::Write as _;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

use serde_json::{json, Value};

const BELOW_NORMAL: u32 = 0x4000;
/// The notes `graph-frames` adds.
const GRAPH_NOTES: usize = 80; // the default of `GRAPH_NOTES` in the environment
const SCENARIOS: [&str; 6] =
    ["json-pan", "json-zoom", "json-reveal-numbers", "json-reveal-objects", "graph-frames", "graph-idle"];

/// The most recently changed `.rs` or `Cargo.toml` file under `dir`, with its time, leaving out `crates/gate`: the
/// harness is not part of the exe it profiles.
fn newest_source(dir: &Path) -> Option<(PathBuf, std::time::SystemTime)> {
    let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let found = if path.file_name().is_some_and(|n| n == "gate") {
            None
        } else if path.is_dir() {
            newest_source(&path)
        } else if path.extension().is_some_and(|e| e == "rs") || path.file_name().is_some_and(|n| n == "Cargo.toml") {
            entry.metadata().and_then(|m| m.modified()).ok().map(|at| (path, at))
        } else {
            None
        };
        if let Some(found) = found.filter(|f| newest.as_ref().is_none_or(|n| f.1 > n.1)) {
            newest = Some(found);
        }
    }
    newest
}

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

/// The command file of `graph-frames`: `notes` notes, the camera at 25%, then five phases of `seconds / 5` each, a
/// command per frame: the pointer moving over the cards, typing into the last note (a new note has the
/// keyboard), typing into the quick switcher (each keystroke filters every card), scrolling (which zooms in the Graph view) and a drag on the empty canvas (a pan). `state` is where it
/// exports the state to count the cards. `paced` holds a frame (`SLEEP`) after each command, as a visible window
/// needs for the run to last `seconds`; a headless run takes them as fast as it can.
fn graph_script(state: &Path, seconds: f64, paced: bool, notes: usize) -> String {
    let mut s = String::from("WAITIDLE\n");
    s.push_str(&"NEWNOTE\n".repeat(notes));
    let _ = writeln!(s, "WAITIDLE\nGRAPHZOOM 25\nWAITIDLE\nEXPORTSTATE {}", state.display());
    if paced {
        // Let the startup burn pass before the first phase (see `idle_script`); on DX12 it lasts about 16 s.
        s.push_str("SLEEP 20\n");
    }
    let frames = (seconds * 60.0 / 5.0) as usize;
    let mut frame = |line: String| {
        s.push_str(&line);
        s.push('\n');
        if paced && !line.starts_with("MARK") {
            s.push_str("SLEEP 0.0167\n");
        }
    };
    frame("MARK pointer".into());
    for n in 0..frames {
        frame(format!("MOUSEMOVE {} {}", 300 + (n % 2) * 40, 300 + (n % 3) * 20));
    }
    frame("MARK note-typing".into());
    for _ in 0..frames {
        frame("EGUITEXT n".into());
    }
    frame("ESCAPE".into());
    frame("QUICKSWITCHOPEN".into());
    frame("MARK switcher-typing".into());
    for n in 0..frames {
        frame(format!("QUICKSWITCHTEXT {}", &"note typing in the switcher"[..n % 20 + 1]));
    }
    frame("QUICKSWITCHESCAPE".into());
    frame("MARK scroll-zoom".into());
    for n in 0..frames {
        frame(format!("SCROLL 600 400 0 {}", if (n / 60) % 2 == 0 { 12 } else { -12 }));
    }
    frame("MARK pan".into());
    frame("MOUSEDOWN 1300 850".into());
    for n in 0..frames {
        let step = n % 120;
        frame(format!("MOUSEMOVE {} 850", 1300 - 6 * (if (n / 120) % 2 == 0 { step } else { 120 - step })));
    }
    frame("MOUSEUP 700 850".into());
    frame("MARK end".into());
    frame("QUIT".into());
    s
}

/// The command file of `graph-idle`: `notes` notes at 25% zoom, then nothing for `seconds`. The run's `idle` phase
/// shows how often the loop redraws and how much CPU it spends when nothing happens. The 15 s before the mark let the
/// startup burn pass (about a core for the first 10-12 s, measured 2026-10-21; `PARKED.md`).
fn idle_script(state: &Path, seconds: f64, notes: usize) -> String {
    let mut s = String::from(
        "WAITIDLE
",
    );
    s.push_str(
        &"NEWNOTE
"
        .repeat(notes),
    );
    let _ = writeln!(
        s,
        "WAITIDLE
GRAPHZOOM 25
WAITIDLE
EXPORTSTATE {}",
        state.display()
    );
    let _ = writeln!(
        s,
        "SLEEP 15
MARK idle
SLEEP {seconds}
MARK end
QUIT"
    );
    s
}

/// What a run's state export shows: the card count of `graph-frames`, else the JSON level's `selected` pointer
/// (empty when there is none).
fn result(scenario: &str, state: &Path) -> String {
    let text = std::fs::read_to_string(state).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if scenario.starts_with("graph-") {
        return v["cards"].as_array().map_or(0, Vec::len).to_string();
    }
    v["json_level"]["selected"].as_str().unwrap_or("").to_owned()
}

fn run(args: &[String]) -> Result<(), String> {
    let samply = args.iter().any(|a| a == "--samply");
    // A visible window (`--debug-commands`) instead of the hidden headless one: the real present path.
    let visible = args.iter().any(|a| a == "--visible");
    let mode_flag = if visible { "--debug-commands" } else { "--headless" };
    let mut pos = args.iter().filter(|a| !a.starts_with("--"));
    let scenario = pos.next().map_or("", String::as_str);
    if !SCENARIOS.contains(&scenario) {
        return Err(format!("SCENARIO {scenario:?} is not one of {}", SCENARIOS.join(", ")));
    }
    let mode = scenario.strip_prefix("json-").unwrap_or(scenario);
    let seconds: f64 = pos.next().map_or(Ok(20.0), |s| s.parse()).map_err(|e| format!("SECONDS: {e}"))?;
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().map_err(|e| e.to_string())?;
    // `canonicalize` gives a verbatim `\\?\` path, which the elevated samply task cannot expand.
    let repo = repo.to_str().and_then(|s| s.strip_prefix(r"\\?\")).map_or(repo.clone(), PathBuf::from);
    let release = repo.join("target/release/manifold.exe");
    if !release.exists() {
        return Err(format!("build the release exe first (see the module doc): {}", release.display()));
    }
    let built = release.metadata().and_then(|m| m.modified()).map_err(|e| e.to_string())?;
    if let Some(newer) = newest_source(&repo.join("crates")).filter(|(_, at)| *at > built).map(|(path, _)| path) {
        return Err(format!("the release exe is older than {}: rebuild it (see the module doc)", newer.display()));
    }
    let out = repo.join("target/profile");
    let _ = std::fs::remove_dir_all(&out);
    let appdata = out.join("appdata");
    let io = |e: std::io::Error| e.to_string();
    crate::instance::seed_settings(&appdata, "webview2", "").map_err(io)?;
    let (branch, reveal_n) = (env_usize("JSON_BRANCH", 12), env_usize("REVEAL_N", 200_000));
    let fixture_path = out.join("fixture.json");
    let state = out.join("state.json");
    let notes = env_usize("GRAPH_NOTES", GRAPH_NOTES);
    let commands = if scenario == "graph-frames" {
        graph_script(&state, seconds, visible, notes)
    } else if scenario == "graph-idle" {
        idle_script(&state, seconds, notes)
    } else {
        std::fs::write(&fixture_path, fixture(mode, branch, reveal_n).to_string()).map_err(io)?;
        script(&fixture_path, &state, seconds, mode, branch, reveal_n)
    };
    let command_file = out.join("commands.txt");
    std::fs::write(&command_file, &commands).map_err(io)?;
    let exe = out.join("manifold.exe");
    std::fs::copy(&release, &exe).map_err(io)?;
    let app_args = |c: &mut Command| {
        c.arg(mode_flag).arg("--profile-dir").arg(&appdata).arg("--script").arg(&command_file);
    };
    if samply {
        let ps1 = std::env::var_os("USERPROFILE").map(|h| Path::new(&h).join(".claude/samply/profile.ps1"));
        let ps1 = ps1.filter(|p| p.exists()).ok_or("~/.claude/samply/profile.ps1 not found: see SAMPLY.md")?;
        // `-Command`, not `-File`: only a command line splits `-ExeArgs 'a','b'` into an array.
        let quote = |p: &dyn std::fmt::Display| format!("'{}'", p.to_string().replace('\'', "''"));
        let list = [mode_flag.to_owned(), "--profile-dir".into(), appdata.display().to_string(), "--script".into()]
            .into_iter()
            .chain([command_file.display().to_string()])
            .map(|a| quote(&a))
            .collect::<Vec<_>>()
            .join(",");
        let command = format!(
            "& {} -Exe {} -Out {} -ExeArgs {list}",
            quote(&ps1.display()),
            quote(&exe.display()),
            quote(&out.join("profile.json.gz").display())
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", &command])
            .creation_flags(BELOW_NORMAL)
            .stdin(Stdio::null())
            .status()
            .map_err(io)?;
        return if status.success() { Ok(()) } else { Err(format!("profile.ps1: {status}")) };
    }
    let log = std::fs::File::create(out.join("run.log")).map_err(io)?;
    let trace = out.join("trace.json");
    let mut c = Command::new(&exe);
    app_args(&mut c);
    c.env("MANIFOLD_TRACE", &trace);
    let start = Instant::now();
    let status = c
        .creation_flags(BELOW_NORMAL)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(io)?)
        .stderr(log)
        .status()
        .map_err(io)?;
    let line = format!(
        "TIME_ONLY scenario={scenario} seconds={seconds} commands={} ms={} exit={} result={}",
        commands.lines().count(),
        start.elapsed().as_millis(),
        status.code().unwrap_or(-1),
        result(scenario, &state)
    );
    println!("{line}");
    if let Ok(text) = std::fs::read_to_string(&trace) {
        print!("{}", crate::trace::summarize(&text));
        // `run.ps1` takes this as the run's headline (RUNS.md).
        if let Some(dir) = std::env::var_os("RUN_DIR") {
            std::fs::write(Path::new(&dir).join("headline.txt"), crate::trace::headline(&text)).map_err(io)?;
        }
    }
    let mut log =
        std::fs::OpenOptions::new().create(true).append(true).open(repo.join("target/profile-time.log")).map_err(io)?;
    writeln!(log, "{line}").map_err(io)
}

pub fn profile(args: &[String]) -> ExitCode {
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
    fn the_newest_source_is_the_latest_rs_or_manifest_below_a_directory() {
        let dir = std::env::temp_dir().join(format!("gate-newest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/src")).unwrap();
        std::fs::write(dir.join("a/src/old.rs"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(dir.join("a/notes.txt"), "").unwrap();
        std::fs::write(dir.join("a/Cargo.toml"), "").unwrap();
        let (path, _) = newest_source(&dir).unwrap();
        assert!(path.ends_with("Cargo.toml"), "{path:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

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

    #[test]
    fn the_idle_script_marks_the_idle_phase_and_does_nothing_in_it() {
        let s = idle_script(Path::new("s.json"), 30.0, 4);
        assert_eq!(s.matches("NEWNOTE").count(), 4);
        assert!(
            s.ends_with(
                "MARK idle
SLEEP 30
MARK end
QUIT
"
            ),
            "{s}"
        );
    }

    #[test]
    fn the_graph_script_runs_five_phases_of_a_command_per_frame() {
        let s = graph_script(Path::new("s.json"), 5.0, false, GRAPH_NOTES);
        assert_eq!(s.matches("NEWNOTE").count(), GRAPH_NOTES);
        assert!(s.contains("GRAPHZOOM 25\nWAITIDLE\nEXPORTSTATE s.json\n"));
        assert_eq!(s.matches("MOUSEMOVE").count(), 120);
        assert_eq!(s.matches("EGUITEXT").count(), 60);
        assert_eq!(s.matches("QUICKSWITCHTEXT").count(), 60);
        assert_eq!(s.matches("SCROLL").count(), 60);
        assert_eq!(s.matches("SLEEP").count(), 0);
        assert!(s.ends_with("QUIT\n"));
        assert_eq!(graph_script(Path::new("s.json"), 5.0, true, GRAPH_NOTES).matches("SLEEP").count(), 60 * 5 + 7);
    }
}
