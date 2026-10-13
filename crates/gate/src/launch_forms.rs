//! `gate launch-forms`: scripts/launch-forms-probe.sh as a Rust flow (a Git Bash fork costs seconds on this machine; a
//! native spawn costs milliseconds). Launches the exe once in each form its launchers and the session tools use
//! (argument checking, wave 1 M4) and checks each exit code:
//!
//! - `drive`: the form scripts/drive.sh uses: `--import-json`/`--out` into an isolated `APPDATA`, then `--headless`
//!   with the commands piped on stdin (exit 0)
//! - `script`: `--headless --profile-dir --script` (exit 0)
//! - `import`, `dump`, `check`: the file tools (exit 0)
//! - `refused`: `--headless ... --hedless` exits 2 and names the argument
//!
//! `--debug-commands` is not launched: it opens a real window (a unit test covers it). Every run is headless or a file
//! tool, on a temp profile (never the real `%APPDATA%\manifold`), under a 60 s limit so a hang fails instead of
//! stalling. scripts/drive.sh itself is no longer exercised here; its form is.

use std::fs::{self, File};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::check::{check_eq, exit_code, failures, Section};
use crate::instance::{job, remove_dir_with_retries};
use crate::windows_main::{manifold_exe, work_dir};

/// `BELOW_NORMAL_PRIORITY_CLASS`, as in `instance.rs`.
const BELOW_NORMAL: u32 = 0x4000;
const LIMIT: Duration = Duration::from_secs(60);
/// `timeout`'s exit status when it kills a run, kept so a log line reads as the script's did.
const TIMED_OUT: i32 = 124;

/// Runs `cmd` with its output in `WORK/NAME.out` and `.err`, `stdin_text` (if any) on its stdin, and checks that it
/// exits `want`. Returns whether it did.
fn run(work: &Path, name: &str, want: i32, mut cmd: Command, stdin_text: Option<&str>) -> bool {
    let open = |ext: &str| File::create(work.join(format!("{name}.{ext}")));
    let (Ok(out), Ok(err)) = (open("out"), open("err")) else {
        return check_eq(&format!("{name} exits {want}"), want, -1);
    };
    cmd.stdin(if stdin_text.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(out)
        .stderr(err)
        .creation_flags(BELOW_NORMAL);
    let rc = match cmd.spawn() {
        Err(e) => {
            println!("ERROR: {name}: spawn failed: {e}");
            -1
        }
        Ok(mut child) => {
            job::assign(&child);
            if let (Some(text), Some(mut stdin)) = (stdin_text, child.stdin.take()) {
                let _ = stdin.write_all(text.as_bytes());
            }
            let deadline = Instant::now() + LIMIT;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status.code().unwrap_or(-1),
                    Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(20)),
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break TIMED_OUT;
                    }
                }
            }
        }
    };
    check_eq(&format!("{name} exits {want}"), want, rc)
}

fn tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

pub(crate) fn launch_forms(parent: Option<PathBuf>) -> ExitCode {
    let setup = Section::start("setup");
    let exe = match manifold_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let work = match work_dir(parent, "launch-forms") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let session = work.join("session.json");
    let quit = work.join("quit.txt");
    let mnf = work.join("s.mnf");
    let seeded = fs::write(&session, r#"{"nodes":[{"id":"g","type":"application/vnd.manifold.group","name":"G"}]}"#)
        .and_then(|()| fs::write(&quit, "QUIT\n"))
        .and_then(|()| fs::create_dir_all(work.join("p")))
        .and_then(|()| fs::create_dir_all(work.join("bad")))
        .and_then(|()| fs::create_dir_all(work.join("drive").join("appdata").join("manifold")));
    if let Err(e) = seeded {
        eprintln!("seed: {e}");
        return ExitCode::from(1);
    }
    let manifold = || Command::new(&exe);

    let drive_appdata = work.join("drive").join("appdata");
    let mut import_for_drive = manifold();
    import_for_drive
        .arg("--import-json")
        .arg(&session)
        .arg("--out")
        .arg(drive_appdata.join("manifold").join("session.mnf"));
    run(&work, "drive-import", 0, import_for_drive, None);
    let drive = setup.next("drive");
    let mut headless = manifold();
    headless.arg("--headless").env("APPDATA", &drive_appdata);
    run(&work, "drive", 0, headless, Some("QUIT\n"));
    drop(drive);

    let script = Section::start("script");
    let mut c = manifold();
    c.arg("--headless").arg("--profile-dir").arg(work.join("p")).arg("--script").arg(&quit);
    run(&work, "script", 0, c, None);
    let tools = script.next("tools");
    let mut c = manifold();
    c.arg("--import-json").arg(&session).arg("--out").arg(&mnf);
    run(&work, "import", 0, c, None);
    for tool in ["dump", "check"] {
        let mut c = manifold();
        c.arg(format!("--{tool}")).arg(&mnf);
        run(&work, tool, 0, c, None);
    }
    let refused = tools.next("refused");
    let mut c = manifold();
    c.arg("--headless").arg("--profile-dir").arg(work.join("bad")).arg("--hedless");
    run(&work, "refused", 2, c, None);
    let named =
        fs::read_to_string(work.join("refused.err")).unwrap_or_default().contains(r#"unknown argument "--hedless""#);
    check_eq("refused names the argument", 1, i32::from(named));
    drop(refused);

    if failures() > 0 {
        for name in ["drive-import", "drive", "script", "import", "dump", "check", "refused"] {
            for ext in ["out", "err"] {
                let p = work.join(format!("{name}.{ext}"));
                if fs::metadata(&p).is_ok_and(|m| m.len() > 0) {
                    println!("--- {name}.{ext}\n{}", tail(&p));
                }
            }
        }
    } else if std::env::var_os("KEEP").is_none() {
        remove_dir_with_retries(&work);
    }
    exit_code()
}
