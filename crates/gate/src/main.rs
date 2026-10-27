//! `gate <stage>`: the headless gate flows as one Rust program (plan docs/superpowers/plans/2026-10-10-rust-gate-runner.md).
//! It launches `manifold --headless` itself, drives it over stdin, reads the exported state back and prints the
//! `PASS:`/`FAIL:`/`TIMING` lines the shell scripts print, so `gate all` can run it as a stage. The exit code is
//! 0 when no check failed.
//!
//! Stages: `hello` (one instance: launch, WAITTABS, export, QUIT), `launch-forms` (was scripts/launch-forms-probe.sh), `layout` (popups inside the window and opaque, see layout.rs), `link` (was scripts/link-probe.sh), `unknown-type` (a node of a type this build doesn't know, end to end, see unknown_type.rs), `closed` (the closed field end to end, see closed.rs), `trail` (Back and Forward over a tab's pages and opened-from, see trail.rs), `smoke` (was
//! scripts/headless-smoke-test.sh) and `two` (the two-instance test, was scripts/two-instance-test.sh). Run from the repo after
//! `cargo build -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`; the exe is `manifold.exe` beside `gate.exe`.

#[cfg(windows)]
mod ab;
#[cfg(windows)]
mod all;
mod arrange;
#[cfg(windows)]
mod cef_pin;
mod check;
#[cfg(windows)]
mod click_count;
#[cfg(windows)]
mod closed;
#[cfg(windows)]
mod dblprobe;
#[cfg(windows)]
mod focus;
mod fresh;
mod http;
mod iframe_typing;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
mod jar;
mod labels;
#[cfg(windows)]
mod launch_forms;
#[cfg(windows)]
mod layout;
#[cfg(windows)]
mod link;
mod nonce;
#[cfg(windows)]
mod package;
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
mod suggest_groups;
mod surface;
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

    /// How a stage takes its arguments: the manifold exe and a fresh work dir (made from the work-dir parent argument),
    /// the work-dir parent, the rest of the command line, or nothing.
    enum Run {
        Work(fn(&Path, &Path) -> ExitCode),
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

    impl Stage {
        /// A stage that takes `[WORKDIR_PARENT]` and is run with `manifold.exe` and its work dir (named after the stage).
        const fn work(name: &'static str, f: fn(&Path, &Path) -> ExitCode) -> Self {
            Self { name, usage: Some("[WORKDIR_PARENT]"), run: Run::Work(f) }
        }

        /// A stage that takes `[WORKDIR_PARENT]` and makes its own work dir.
        const fn parent(name: &'static str, f: fn(Option<PathBuf>) -> ExitCode) -> Self {
            Self { name, usage: Some("[WORKDIR_PARENT]"), run: Run::Parent(f) }
        }

        const fn args(name: &'static str, usage: &'static str, f: fn(&[String]) -> ExitCode) -> Self {
            Self { name, usage: Some(usage), run: Run::Args(f) }
        }

        /// The usage text after the name, for a stage that takes its parent argument but does not list it.
        const fn listed_as(mut self, usage: &'static str) -> Self {
            self.usage = Some(usage);
            self
        }
    }

    /// Every stage: the one place `gate <name>` dispatch and the usage text come from.
    const STAGES: &[Stage] = &[
        Stage::args("all", "LOG [full|light|ui]", |a| crate::all::all(a.first().map(PathBuf::from), a.get(1).cloned())),
        Stage::parent("hello", hello),
        Stage::parent("launch-forms", crate::launch_forms::launch_forms),
        Stage::parent("layout", crate::layout::layout),
        Stage::parent("link", crate::link::link),
        Stage::parent("smoke", crate::smoke::smoke),
        Stage::parent("two", crate::two::two),
        Stage::parent("unknown-type", crate::unknown_type::unknown_type),
        Stage::parent("closed", crate::closed::closed),
        Stage::parent("dblprobe", crate::dblprobe::dblprobe),
        Stage::parent("trail", crate::trail::trail),
        Stage::work("unresponsive", crate::stress::unresponsive),
        Stage::work("focus-loss", crate::focus::focus_loss).listed_as(""),
        Stage::work("bad-url", crate::probes::bad_url),
        Stage::work("failed-spawn", crate::probes::failed_spawn),
        Stage::work("hscroll", crate::probes::hscroll),
        Stage::work("key", crate::probes::key),
        Stage::work("page-kind", crate::probes::page_kind),
        Stage::work("chrome-shots", crate::shots::chrome_shots),
        Stage::work("chrome-shots-sizes", crate::shots::chrome_shots_sizes),
        Stage::args("shots-diff", "A B", crate::shots_img::shots_diff),
        Stage::work("click-count", crate::click_count::click_count),
        Stage::work("iframe-typing", crate::iframe_typing::iframe_typing),
        Stage::work("page-text", crate::page_text::page_text),
        Stage::work("surface", crate::surface::surface),
        Stage::work("search", crate::search::search),
        Stage::work("suggest-groups", crate::suggest_groups::suggest_groups),
        Stage::work("labels", crate::labels::labels),
        Stage::work("arrange", crate::arrange::arrange),
        Stage::work("sleep", crate::sleep::sleep).listed_as(""),
        Stage::work("jar", crate::jar::jar).listed_as(""),
        Stage::args("scale", "TABS...", crate::stress::scale),
        Stage::args("release-verify", "vX.Y.Z [DIST]", crate::release_verify::release_verify),
        Stage::args("release-check", "vX.Y.Z", crate::release_check::release_check),
        Stage::args("release-tag", "vX.Y.Z", crate::release_check::release_tag),
        Stage::args("release-notes", "vX.Y.Z", crate::release_check::release_notes),
        Stage { name: "sync-runtime-deps", usage: Some(""), run: Run::Bare(crate::sync_deps::sync_runtime_deps) },
        Stage::args("cef-pin", "[--platform ID] PREFIX", crate::cef_pin::cef_pin_main),
        Stage::args("profile", "SCENARIO [SECONDS] [--samply] [--visible]", crate::profile::profile),
        Stage::args(
            "ab",
            "NAME SCENARIO [SECONDS] [--repeats N] [--quiet PCT] LABEL:VAR=value[,VAR=value]...",
            crate::ab::ab,
        ),
        Stage::args("package", "[EXE]", crate::package::package),
        Stage::args("install-check", "cef|servo EXE", crate::package::install_check),
        Stage::args("tier", "[BASE|--stdin]", crate::tier::tier),
        Stage::args("fresh", "EXE", fresh),
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
            Some(Stage { name, run: Run::Work(f), .. }) => {
                match crate::probes::setup(rest.first().map(PathBuf::from), name) {
                    Ok((exe, work)) => f(&exe, &work),
                    Err(code) => code,
                }
            }
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
