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
mod download;
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
mod release_hosts;
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
    use std::fmt::{Display, Write as _};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::time::{Duration, SystemTime};

    use crate::check::{check, check_eq, exit_code, failures, no_verdict_words, note_failure, Marks, Section};
    use crate::fresh::require_fresh_exe;
    use crate::instance::{log_tail, remove_dir_with_retries, seed_settings, take_logs, Instance};

    /// What a stage that takes `[WORKDIR_PARENT]` runs inside `run_stage`.
    type Body = fn(&mut Ctx) -> io::Result<()>;

    /// How a stage takes its arguments: a body run by `run_stage` (with the manifold exe and a fresh work dir made from
    /// the work-dir parent argument), the work-dir parent, the rest of the command line, or nothing.
    enum Run {
        Work(Body),
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
        /// A stage that takes `[WORKDIR_PARENT]`; `run_stage` runs its body with `manifold.exe` and its work dir.
        const fn work(name: &'static str, f: Body) -> Self {
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
        Stage::work("download", crate::download::download),
        Stage::work("chrome-shots", crate::shots::chrome_shots),
        Stage::work("chrome-shots-sizes", crate::shots::chrome_shots_sizes),
        Stage::work("shots-aa", crate::shots::shots_aa),
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
        Stage::args("release-pins", "vX.Y.Z DIST FOREIGN", crate::release_hosts::release_pins),
        Stage::args("release-embedded", "vX.Y.Z DIST", crate::release_hosts::release_embedded),
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
            Some(Stage { name, run: Run::Work(f), .. }) => run_stage(name, rest.first().map(PathBuf::from), true, *f),
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

    /// Work dirs older than this are removed when a new one is made.
    const RUN_DIR_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);

    /// A fresh work dir (log, exports, profile) `<parent, default the temp dir>/gate-runs/<stage>-<pid>`. Children of
    /// `gate-runs` older than `RUN_DIR_MAX_AGE` are removed first (a failed run keeps its dir); nothing outside
    /// `gate-runs` is touched.
    pub(crate) fn work_dir(parent: Option<PathBuf>, stage: &str) -> std::io::Result<PathBuf> {
        let runs = parent.unwrap_or_else(std::env::temp_dir).join("gate-runs");
        std::fs::create_dir_all(&runs)?;
        prune(&runs, SystemTime::now());
        let dir = runs.join(format!("{stage}-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Removes the children of `runs` last modified more than `RUN_DIR_MAX_AGE` before `now`; best effort.
    fn prune(runs: &Path, now: SystemTime) {
        let Ok(entries) = std::fs::read_dir(runs) else { return };
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).is_ok_and(|d| d > RUN_DIR_MAX_AGE));
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    /// What a stage body gets: `manifold.exe` (empty when the stage does not need it), its fresh work dir, and the
    /// timing marks (empty: the first `marks.mark(name)` opens a section).
    pub(crate) struct Ctx {
        pub exe: PathBuf,
        pub work: PathBuf,
        pub marks: Marks,
    }

    /// Lines of each registered log a failed stage prints.
    const LOG_TAIL_LINES: usize = 20;

    /// The one frame of every stage that takes a work dir. Finds `manifold.exe` (when `needs_exe`) and makes the work
    /// dir, runs `body`, prints an error it returns, prints the registered logs if anything failed, keeps the dir after
    /// a failure (or with `KEEP` set) and deletes it otherwise, and ends with `{name}: ok` or
    /// `{name}: failed (kept {dir})`, the line `gate all` reports.
    pub(crate) fn run_stage(
        name: &str,
        parent: Option<PathBuf>,
        needs_exe: bool,
        body: impl FnOnce(&mut Ctx) -> io::Result<()>,
    ) -> ExitCode {
        let exe = if needs_exe {
            match manifold_exe() {
                Ok(exe) => exe,
                Err(e) => return setup_failed(name, &e),
            }
        } else {
            PathBuf::new()
        };
        let work = match work_dir(parent, name) {
            Ok(w) => w,
            Err(e) => return setup_failed(name, &format!("work dir: {e}")),
        };
        let mut ctx = Ctx { exe, work, marks: Marks::default() };
        let before = failures();
        let errored = if let Err(e) = body(&mut ctx) {
            println!("{}", early_stop_line(name, &e));
            note_failure();
            true
        } else {
            false
        };
        ctx.marks.end();
        let failed = stage_failed(errored, before, failures());
        // Only a failed run drains the registry (one process runs one stage); a passing run, e.g. a unit test, leaves it.
        let logs = if failed { take_logs() } else { Vec::new() };
        if failed {
            print!("{}", log_tails(&logs));
        }
        if !keep_dir(failed, std::env::var_os("KEEP").is_some()) {
            remove_dir_with_retries(&ctx.work);
        }
        println!("{}", closing_line(name, failed, &ctx.work));
        exit_code()
    }

    /// Setup failed before any dir existed: says why, then the stage's closing line.
    fn setup_failed(name: &str, e: &str) -> ExitCode {
        eprintln!("{e}");
        println!("{name}: failed");
        ExitCode::from(1)
    }

    /// Whether this stage failed: its body returned `Err`, or the process-wide failure count rose while the body ran.
    /// Comparing before and after keeps a failure noted earlier in the process (a unit test that fails a check on
    /// purpose) from failing every later stage; `gate`'s exit code still reads the global count.
    fn stage_failed(errored: bool, before: u32, after: u32) -> bool {
        errored || after > before
    }

    /// Whether the work dir stays: after a failure, or when `KEEP` is set.
    fn keep_dir(failed: bool, keep_env: bool) -> bool {
        failed || keep_env
    }

    /// The line for a body's error. Not a verdict line: `gate all` counts those words, and the error is counted by
    /// `note_failure`. The error text is masked too, since it may quote a log line or a child's output.
    fn early_stop_line(name: &str, e: &dyn Display) -> String {
        format!("ERROR: {name} stopped early: {}", no_verdict_words(&e.to_string()))
    }

    /// A stage's last line.
    fn closing_line(name: &str, failed: bool, work: &Path) -> String {
        if failed {
            format!("{name}: failed (kept {})", work.display())
        } else {
            format!("{name}: ok")
        }
    }

    /// `--- <path>` and the last `LOG_TAIL_LINES` lines of each non-empty log in `logs`, the verdict words masked.
    fn log_tails(logs: &[PathBuf]) -> String {
        let mut out = String::new();
        for log in logs.iter().filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0)) {
            let _ = writeln!(out, "--- {}\n{}", log.display(), no_verdict_words(&log_tail(log, LOG_TAIL_LINES)));
        }
        out
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
        use std::os::windows::fs::OpenOptionsExt;

        use super::*;

        #[test]
        fn a_failed_run_or_keep_keeps_the_work_dir() {
            assert!(!keep_dir(false, false));
            assert!(keep_dir(true, false));
            assert!(keep_dir(false, true));
            assert!(keep_dir(true, true));
        }

        #[test]
        fn the_early_stop_and_closing_lines_carry_no_verdict_word() {
            let e = std::io::Error::other("export s1 never appeared");
            assert_eq!(early_stop_line("link", &e), "ERROR: link stopped early: export s1 never appeared");
            let work = Path::new(r"C:\t\gate-runs\link-7");
            assert_eq!(closing_line("link", false, work), "link: ok");
            assert_eq!(closing_line("link", true, work), r"link: failed (kept C:\t\gate-runs\link-7)");
            let quoting = std::io::Error::other("host said FAIL: no PASS");
            assert_eq!(early_stop_line("x", &quoting), "ERROR: x stopped early: host said F-AIL: no P-ASS");
            for line in [early_stop_line("x", &e), early_stop_line("x", &quoting), closing_line("x", true, work)] {
                assert!(!line.contains("PASS") && !line.contains("FAIL"), "{line}");
            }
        }

        #[test]
        fn log_tails_mask_the_verdict_words_and_skip_empty_and_missing_logs() {
            let dir = std::env::temp_dir().join(format!("gate-tails-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let (full, empty, missing) = (dir.join("full.log"), dir.join("empty.log"), dir.join("missing.log"));
            let lines: Vec<String> = (1..=25).map(|i| format!("line {i}")).collect();
            std::fs::write(&full, format!("{}\nPASS: x\nFAIL: y\n", lines.join("\n"))).unwrap();
            std::fs::write(&empty, "").unwrap();
            let text = log_tails(&[full.clone(), empty, missing]);
            assert!(text.starts_with(&format!("--- {}\n", full.display())), "{text}");
            assert!(!text.contains("PASS") && !text.contains("FAIL"), "{text}");
            assert!(text.contains("P-ASS: x") && text.contains("F-AIL: y"), "{text}");
            assert!(text.contains("line 8\n") && !text.contains("line 7\n"), "the last 20 lines only: {text}");
            assert!(!text.contains("empty.log") && !text.contains("missing.log"), "{text}");
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// Under `cargo test` the exe is `target/debug/deps/gate-*.exe`, with no `manifold.exe` beside it, so
        /// `manifold_exe()` fails: the setup-failure path, which touches no process-wide counter.
        #[test]
        fn a_setup_failure_runs_no_body_and_makes_no_dir() {
            let root = std::env::temp_dir().join(format!("gate-setup-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let ran = std::cell::Cell::new(false);
            let _ = run_stage("t", Some(root.clone()), true, |_| {
                ran.set(true);
                Ok(())
            });
            assert!(!ran.get(), "the body never runs");
            assert!(!root.join("gate-runs").exists(), "no work dir is made");
        }

        /// A stage fails if its body errored or the failure count rose during the body; a count already above zero
        /// (another unit test failing a check on purpose) does not matter.
        #[test]
        fn a_stage_fails_only_on_an_error_or_a_rise_during_its_body() {
            assert!(!stage_failed(false, 0, 0));
            assert!(!stage_failed(false, 3, 3), "earlier failures do not fail this stage");
            assert!(stage_failed(false, 3, 4));
            assert!(stage_failed(true, 3, 3));
        }

        /// The success path: the body runs with its work dir made, and the dir is gone afterwards. `needs_exe` is
        /// false, so no `manifold.exe` is needed. `check.rs`'s `a_failed_check_is_counted` raises the process-wide
        /// failure count in parallel, and if that lands inside `run_stage`'s before/after window the stage counts as
        /// failed and keeps its dir. So the test reads `failures()` around the call and asserts "the dir is gone" only
        /// when the count did not move (and `KEEP` is unset: it keeps the dir by design). The exit code is not
        /// asserted: it reads the process-wide count (Step 6 checks that a passing stage exits 0).
        #[test]
        fn a_passing_body_runs_in_its_work_dir_and_leaves_none_behind() {
            let root = std::env::temp_dir().join(format!("gate-ok-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let seen = std::cell::RefCell::new(None);
            let before = failures();
            let _ = run_stage("t", Some(root.clone()), false, |ctx| {
                *seen.borrow_mut() = Some((ctx.work.clone(), ctx.work.is_dir()));
                Ok(())
            });
            let after = failures();
            let (work, existed) = seen.borrow().clone().expect("the body ran");
            assert_eq!(work, root.join("gate-runs").join(format!("t-{}", std::process::id())));
            assert!(existed, "the work dir exists inside the body");
            if before == after && std::env::var_os("KEEP").is_none() {
                assert!(!work.exists(), "a passing run deletes its work dir");
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn work_dirs_live_under_gate_runs_and_old_ones_are_pruned() {
            let root = std::env::temp_dir().join(format!("gate-prune-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let runs = root.join("gate-runs");
            let (old, mid) = (runs.join("old-1"), runs.join("mid-1"));
            let sibling = root.join("gate-mine");
            std::fs::create_dir_all(&old).unwrap();
            std::fs::create_dir_all(&mid).unwrap();
            std::fs::create_dir_all(&sibling).unwrap();
            // Directories open for writing only with backup semantics (FILE_FLAG_BACKUP_SEMANTICS).
            let backdate = |dir: &Path, age: Duration| {
                let f = std::fs::OpenOptions::new().write(true).custom_flags(0x0200_0000).open(dir).unwrap();
                f.set_modified(SystemTime::now() - age).unwrap();
            };
            backdate(&old, Duration::from_secs(4 * 24 * 60 * 60));
            backdate(&mid, Duration::from_secs(2 * 24 * 60 * 60));
            backdate(&sibling, Duration::from_secs(4 * 24 * 60 * 60));
            let fresh = runs.join("fresh-1");
            std::fs::create_dir_all(&fresh).unwrap();
            prune(&runs, SystemTime::now());
            assert!(!old.exists(), "a dir older than three days goes");
            assert!(mid.is_dir() && fresh.is_dir(), "a dir two days old and a fresh one stay");
            assert!(sibling.is_dir(), "nothing outside gate-runs is touched, however old");
            let work = work_dir(Some(root.clone()), "t").unwrap();
            assert_eq!(work, runs.join(format!("t-{}", std::process::id())), "the work dir lives under gate-runs");
            assert!(work.is_dir() && mid.is_dir(), "work_dir makes its dir and keeps younger ones");
            let _ = std::fs::remove_dir_all(&root);
        }

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

        /// `install-check`'s usage names exactly the engines it installs, as a set: the downloaded hosts built for this
        /// OS. `STAGES` holds only literals, so the literal is checked here against the host table.
        #[test]
        fn install_checks_usage_lists_the_installable_hosts() {
            let usage = STAGES.iter().find(|s| s.name == "install-check").and_then(|s| s.usage).unwrap_or("");
            let listed: std::collections::BTreeSet<&str> =
                usage.split_whitespace().next().unwrap_or("").split('|').collect();
            let hosts: std::collections::BTreeSet<&str> = crate::package::installable().map(|h| h.id).collect();
            assert_eq!(listed, hosts, "gate install-check {usage}");
        }
    }
}
