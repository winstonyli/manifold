//! `gate <stage>`: the headless gate flows as one Rust program (plan docs/superpowers/plans/2026-10-10-rust-gate-runner.md).
//! It launches `manifold --headless` itself, drives it over stdin, reads the exported state back and prints the
//! `PASS:`/`FAIL:`/`TIMING` lines the shell scripts print, so `gate all` can run it as a stage. The exit code is
//! 0 when no check failed.
//!
//! Stages: `hello` (one instance: launch, WAITTABS, export, QUIT), `launch-forms` (was scripts/launch-forms-probe.sh), `layout` (popups inside the window and opaque, see layout.rs), `link` (was scripts/link-probe.sh), `unknown-type` (a node of a type this build doesn't know, end to end, see unknown_type.rs), `closed` (the closed field end to end, see closed.rs), `trail` (Back and Forward over a tab's pages and opened-from, see trail.rs), `smoke` (was
//! scripts/headless-smoke-test.sh) and `two` (the two-instance test, was scripts/two-instance-test.sh). Run from the repo after
//! `cargo build -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`; the exe is `manifold.exe` beside `gate.exe`.

#![allow(dead_code, reason = "the runner core's API is used by the link and smoke ports of later tasks, and by tests")]

#[cfg(windows)]
mod all;
#[cfg(windows)]
mod cef_pin;
mod check;
#[cfg(windows)]
mod closed;
#[cfg(windows)]
mod dblprobe;
#[cfg(windows)]
mod focus;
mod fresh;
mod http;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
mod jar;
#[cfg(windows)]
mod json_pan;
#[cfg(windows)]
mod launch_forms;
#[cfg(windows)]
mod layout;
#[cfg(windows)]
mod link;
mod nonce;
#[cfg(windows)]
mod package;
#[cfg(windows)]
mod probes;
#[cfg(windows)]
mod release_check;
#[cfg(windows)]
mod release_verify;
mod session;
#[cfg(windows)]
mod shots;
mod shots_img;
mod shots_net;
#[cfg(windows)]
mod sleep;
#[cfg(windows)]
mod smoke;
mod state;
#[cfg(windows)]
mod stress;
mod suggest;
#[cfg(windows)]
mod sync_deps;
mod tier;
mod timing;
#[cfg(windows)]
mod trail;
#[cfg(windows)]
mod two;
#[cfg(windows)]
mod unknown_type;

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("gate drives manifold.exe headless and runs on Windows only");
    std::process::ExitCode::from(2)
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    windows_main::run()
}

#[cfg(windows)]
mod windows_main {
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::time::Duration;

    use crate::check::{check, check_eq, exit_code, note_failure, Section};
    use crate::fresh::require_fresh_exe;
    use crate::instance::{seed_settings, Instance};

    pub fn run() -> ExitCode {
        let mut args = std::env::args().skip(1);
        let stage = args.next().unwrap_or_default();
        let parent = args.next().map(PathBuf::from);
        match stage.as_str() {
            "all" => crate::all::all(parent, args.next()),
            "bad-url" => crate::probes::bad_url(parent),
            "failed-spawn" => crate::probes::failed_spawn(parent),
            "hscroll" => crate::probes::hscroll(parent),
            "key" => crate::probes::key(parent),
            "page-kind" => crate::probes::page_kind(parent),
            "chrome-shots" => crate::shots::chrome_shots(parent),
            "chrome-shots-sizes" => crate::shots::chrome_shots_sizes(parent),
            "jar" => crate::jar::jar(parent),
            "release-check" => crate::release_check::release_check(&std::env::args().skip(2).collect::<Vec<_>>()),
            "release-notes" => crate::release_check::release_notes(&std::env::args().skip(2).collect::<Vec<_>>()),
            "release-verify" => crate::release_verify::release_verify(&std::env::args().skip(2).collect::<Vec<_>>()),
            "sync-runtime-deps" => crate::sync_deps::sync_runtime_deps(),
            "cef-pin" => crate::cef_pin::cef_pin_main(&std::env::args().skip(2).collect::<Vec<_>>()),
            "json-pan" => crate::json_pan::json_pan(&std::env::args().skip(2).collect::<Vec<_>>()),
            "tier" => crate::tier::tier(&std::env::args().skip(2).collect::<Vec<_>>()),
            "package" => crate::package::package(&std::env::args().skip(2).collect::<Vec<_>>()),
            "install-check" => crate::package::install_check(&std::env::args().skip(2).collect::<Vec<_>>()),
            "focus-loss" => crate::focus::focus_loss(parent),
            "sleep" => crate::sleep::sleep(parent),
            "scale" => crate::stress::scale(&std::env::args().skip(2).collect::<Vec<_>>()),
            "unresponsive" => crate::stress::unresponsive(parent),
            "hello" => hello(parent),
            "launch-forms" => crate::launch_forms::launch_forms(parent),
            "layout" => crate::layout::layout(parent),
            "link" => crate::link::link(parent),
            "smoke" => crate::smoke::smoke(parent),
            "two" => crate::two::two(parent),
            "unknown-type" => crate::unknown_type::unknown_type(parent),
            "closed" => crate::closed::closed(parent),
            "dblprobe" => crate::dblprobe::dblprobe(parent),
            "trail" => crate::trail::trail(parent),
            // Was scripts/fresh-exe.sh: exit 1 and name the stale files if EXE (the next argument) is older than its sources.
            "fresh" => match parent.as_deref().map(require_fresh_exe) {
                Some(Ok(())) => ExitCode::SUCCESS,
                Some(Err(e)) => {
                    eprintln!("{e}");
                    ExitCode::from(1)
                }
                None => {
                    eprintln!("usage: gate fresh EXE");
                    ExitCode::from(2)
                }
            },
            // Test aid for the "killed gate leaves no manifold.exe" proof; not a gate stage.
            "hold" => hold(parent),
            _ => {
                eprintln!("usage: gate all LOG [full|light|ui] | gate hello|launch-forms|unresponsive|layout|link|smoke|two|unknown-type|closed|trail [WORKDIR_PARENT] | gate chrome-shots|chrome-shots-sizes [WORKDIR_PARENT] | gate sleep | gate jar | gate release-verify vX.Y.Z [DIST] | gate release-check vX.Y.Z | gate release-notes vX.Y.Z | gate sync-runtime-deps | gate cef-pin [--platform ID] PREFIX | gate json-pan [MODE] [SECONDS] [--samply] | gate package [EXE] | gate install-check cef|servo EXE | gate focus-loss | gate scale TABS... | gate tier [BASE|--stdin] | gate fresh EXE");
                ExitCode::from(2)
            }
        }
    }

    /// `manifold.exe` beside this exe (`target/debug`), checked fresh.
    pub(crate) fn manifold_exe() -> Result<PathBuf, String> {
        let me = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
        let exe = me.with_file_name("manifold.exe");
        if !exe.exists() {
            return Err(format!(
                "{} not found: cargo build -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate",
                exe.display()
            ));
        }
        require_fresh_exe(&exe)?;
        Ok(exe)
    }

    /// A fresh work dir (log, exports, profile) under `parent`, default the temp dir.
    pub(crate) fn work_dir(parent: Option<PathBuf>, stage: &str) -> std::io::Result<PathBuf> {
        let dir = parent.unwrap_or_else(std::env::temp_dir).join(format!("gate-{stage}-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn launch(exe: &Path, work: &Path) -> std::io::Result<Instance> {
        let profile = work.join("profile");
        seed_settings(&profile, "webview2", "")?;
        let inst = Instance::launch(exe, &profile, work, "hello.log")?;
        // The chosen exe and profile, at the start of every run (LONG_RUNS.md asks for the device; here: the paths).
        println!("launched pid {} exe {} profile {}", inst.pid(), exe.display(), profile.display());
        Ok(inst)
    }

    fn hello(parent: Option<PathBuf>) -> ExitCode {
        let setup = Section::start("setup");
        let exe = match manifold_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        };
        let work = match work_dir(parent, "hello") {
            Ok(w) => w,
            Err(e) => {
                eprintln!("work dir: {e}");
                return ExitCode::from(1);
            }
        };
        let hello = setup.next("hello");
        let result = (|| -> std::io::Result<()> {
            let mut inst = launch(&exe, &work)?;
            inst.send("WAITTABS")?;
            let s = inst.export_state("s0")?;
            check("hello: the headless app exports its state with a cards list", s.0["cards"].is_array());
            check_eq("hello: one start tab is up", 1, s.tabs().len());
            check("hello: the Graph view is showing", !s.tab_view());
            let session = inst.export_session("session0")?;
            check("hello: the exported session has a nodes list", session["nodes"].is_array());
            let png = inst.screenshot("shot0")?;
            check("hello: a screenshot is written", std::fs::metadata(&png).is_ok_and(|m| m.len() > 2000));
            let status = inst.quit(Duration::from_secs(30))?;
            check("hello: the app exits with 0 after QUIT", status.success());
            check("hello: no instance is left running", !inst.is_running());
            if crate::check::failures() > 0 {
                println!("--- tail of {}\n{}", inst.log.display(), inst.log_tail(30));
            }
            Ok(())
        })();
        if let Err(e) = result {
            // Not a PASS/FAIL line itself: gate all counts those words, and this failure is counted below.
            println!("ERROR: hello stopped early: {e}");
            note_failure();
        }
        drop(hello);
        if std::env::var_os("KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&work);
        }
        exit_code()
    }

    /// Launches an instance and then sleeps, so a test can kill `gate` itself and look for leaked processes.
    fn hold(parent: Option<PathBuf>) -> ExitCode {
        let (exe, work) = match (manifold_exe(), work_dir(parent, "hold")) {
            (Ok(exe), Ok(work)) => (exe, work),
            (Err(e), _) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
            (_, Err(e)) => {
                eprintln!("work dir: {e}");
                return ExitCode::from(1);
            }
        };
        let result = launch(&exe, &work).and_then(|mut inst| {
            inst.send("WAITTABS")?;
            println!("gate pid {} holding", std::process::id());
            std::thread::sleep(Duration::from_secs(600));
            Ok(())
        });
        if let Err(e) = result {
            eprintln!("hold: {e}");
            return ExitCode::from(1);
        }
        ExitCode::SUCCESS
    }
}
