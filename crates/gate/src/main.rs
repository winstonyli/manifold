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
mod ab;
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
mod page_text;
mod probes;
mod profile;
#[cfg(windows)]
mod release_check;
#[cfg(windows)]
mod release_verify;
mod search;
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
mod trace;
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

    /// How a stage takes its arguments: the work-dir parent, the rest of the command line, or nothing.
    enum Run {
        Parent(fn(Option<PathBuf>) -> ExitCode),
        Args(fn(&[String]) -> ExitCode),
        Bare(fn() -> ExitCode),
    }

    /// One `gate <name>` stage. `usage` is what follows the name in the usage text (`None`: a test aid, not listed).
    struct Stage {
        name: &'static str,
        usage: Option<&'static str>,
        run: Run,
    }

    /// Every stage: the one place `gate <name>` dispatch and the usage text come from.
    const STAGES: &[Stage] = &[
        Stage {
            name: "all",
            usage: Some("LOG [full|light|ui]"),
            run: Run::Args(|a| crate::all::all(a.first().map(PathBuf::from), a.get(1).cloned())),
        },
        Stage { name: "hello", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(hello) },
        Stage {
            name: "launch-forms",
            usage: Some("[WORKDIR_PARENT]"),
            run: Run::Parent(crate::launch_forms::launch_forms),
        },
        Stage { name: "layout", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::layout::layout) },
        Stage { name: "link", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::link::link) },
        Stage { name: "smoke", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::smoke::smoke) },
        Stage { name: "two", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::two::two) },
        Stage {
            name: "unknown-type",
            usage: Some("[WORKDIR_PARENT]"),
            run: Run::Parent(crate::unknown_type::unknown_type),
        },
        Stage { name: "closed", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::closed::closed) },
        Stage { name: "dblprobe", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::dblprobe::dblprobe) },
        Stage { name: "trail", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::trail::trail) },
        Stage { name: "unresponsive", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::stress::unresponsive) },
        Stage { name: "focus-loss", usage: Some(""), run: Run::Parent(crate::focus::focus_loss) },
        Stage { name: "bad-url", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::probes::bad_url) },
        Stage { name: "failed-spawn", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::probes::failed_spawn) },
        Stage { name: "hscroll", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::probes::hscroll) },
        Stage { name: "key", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::probes::key) },
        Stage { name: "page-kind", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::probes::page_kind) },
        Stage { name: "chrome-shots", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::shots::chrome_shots) },
        Stage {
            name: "chrome-shots-sizes",
            usage: Some("[WORKDIR_PARENT]"),
            run: Run::Parent(crate::shots::chrome_shots_sizes),
        },
        Stage { name: "page-text", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::page_text::page_text) },
        Stage { name: "search", usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(crate::search::search) },
        Stage { name: "sleep", usage: Some(""), run: Run::Parent(crate::sleep::sleep) },
        Stage { name: "jar", usage: Some(""), run: Run::Parent(crate::jar::jar) },
        Stage { name: "scale", usage: Some("TABS..."), run: Run::Args(crate::stress::scale) },
        Stage {
            name: "release-verify",
            usage: Some("vX.Y.Z [DIST]"),
            run: Run::Args(crate::release_verify::release_verify),
        },
        Stage { name: "release-check", usage: Some("vX.Y.Z"), run: Run::Args(crate::release_check::release_check) },
        Stage { name: "release-notes", usage: Some("vX.Y.Z"), run: Run::Args(crate::release_check::release_notes) },
        Stage { name: "sync-runtime-deps", usage: Some(""), run: Run::Bare(crate::sync_deps::sync_runtime_deps) },
        Stage { name: "cef-pin", usage: Some("[--platform ID] PREFIX"), run: Run::Args(crate::cef_pin::cef_pin_main) },
        Stage {
            name: "profile",
            usage: Some("SCENARIO [SECONDS] [--samply] [--visible]"),
            run: Run::Args(crate::profile::profile),
        },
        Stage {
            name: "ab",
            usage: Some("NAME SCENARIO [SECONDS] [--repeats N] [--quiet PCT] LABEL:VAR=value[,VAR=value]..."),
            run: Run::Args(crate::ab::ab),
        },
        Stage { name: "package", usage: Some("[EXE]"), run: Run::Args(crate::package::package) },
        Stage { name: "install-check", usage: Some("cef|servo EXE"), run: Run::Args(crate::package::install_check) },
        Stage { name: "tier", usage: Some("[BASE|--stdin]"), run: Run::Args(crate::tier::tier) },
        Stage { name: "fresh", usage: Some("EXE"), run: Run::Args(fresh) },
        Stage { name: "hold", usage: None, run: Run::Parent(hold) },
    ];

    /// Whether `gate <name>` is a stage (`gate all` asserts it for each stage it launches).
    pub(crate) fn is_stage(name: &str) -> bool {
        STAGES.iter().any(|s| s.name == name)
    }

    fn usage() -> String {
        let listed: Vec<String> = STAGES
            .iter()
            .filter_map(|s| s.usage.map(|u| format!("gate {} {u}", s.name).trim_end().to_owned()))
            .collect();
        format!("usage: {}", listed.join(" | "))
    }

    pub fn run() -> ExitCode {
        let mut args = std::env::args().skip(1);
        let name = args.next().unwrap_or_default();
        let rest: Vec<String> = args.collect();
        match STAGES.iter().find(|s| s.name == name) {
            Some(Stage { run: Run::Parent(f), .. }) => f(rest.first().map(PathBuf::from)),
            Some(Stage { run: Run::Args(f), .. }) => f(&rest),
            Some(Stage { run: Run::Bare(f), .. }) => f(),
            None => {
                eprintln!("{}", usage());
                ExitCode::from(2)
            }
        }
    }

    /// Was scripts/fresh-exe.sh: exit 1 and name the stale files if EXE (the first argument) is older than its sources.
    fn fresh(args: &[String]) -> ExitCode {
        match args.first().map(PathBuf::from).as_deref().map(require_fresh_exe) {
            Some(Ok(())) => ExitCode::SUCCESS,
            Some(Err(e)) => {
                eprintln!("{e}");
                ExitCode::from(1)
            }
            None => {
                eprintln!("usage: gate fresh EXE");
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn stage_names_are_unique_and_the_usage_lists_the_public_ones() {
            let mut seen = std::collections::HashSet::new();
            assert!(STAGES.iter().all(|s| seen.insert(s.name)), "a stage name is registered twice");
            let usage = usage();
            for stage in STAGES.iter().filter(|s| s.usage.is_some()) {
                assert!(usage.contains(&format!("gate {}", stage.name)), "{} missing from {usage}", stage.name);
            }
            assert!(!usage.contains("gate hold"), "the test aid is not listed");
            assert!(is_stage("trail") && !is_stage("sleep-history"));
        }
    }
}
