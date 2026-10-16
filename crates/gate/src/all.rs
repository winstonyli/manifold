//! `gate all LOG [full|light|ui]`: the pre-merge gate (was scripts/gate.sh), sequential so load from other sessions is the
//! only thing that stretches it. A Git Bash fork costs seconds on this machine and the old script forked per stage for
//! the clock, the log counts and the timing join; here only the stages and one PowerShell call for Defender are spawned.
//!
//! - `full` (default): `gate two` x3, `smoke`, `launch-forms`, `link`, `unknown-type`, `closed`, `trail`, `sleep-history`, `layout`. For any change to sync, the store, the
//!   mirror, session save/restore or the view hosts.
//! - `light`: the same with `two` once. For UI code that writes the store or reaches a view, and scripts. The repeats in
//!   `full` exist to catch flaky ordering in sync, which UI-only changes cannot reach.
//! - `ui`: `cargo test` of the three CI crates, then `link`, `unknown-type`, `closed`, `trail`, `sleep-history` and `layout`. For drawing, input and view-state modules
//!   that import no store, sync or network module (the list is in tier.rs), and docs.
//!
//! With `WEBKIT_SITE_HOST=<ip>` set (the WSL gateway as Windows sees it, `wsl.exe -d Ubuntu -- ip route`, the default
//! route's `via`; README "WebKit under the gate") and the WebKit host built in WSL, any mode adds `trail-webkit` and
//! `sleep-webkit` (`ENGINE=webkit`, `VIEWS=webkit`, `SITE_HOST` for those two stages only) after `trail`.
//!
//! Build first (`cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`) and run from
//! the repo root, detached and low priority: `cmd //c start "" //b //belownormal target/debug/gate.exe all LOG [mode]`.
//! Progress and verdicts go to LOG; each stage's own output to LOG.two-N.log, .smoke.log, .launch-forms.log, .link.log, .unknown-type.log, .closed.log, .trail.log,
//! .sleep-history.log, .layout.log and .unit.log. After each stage LOG gets its `TIMING` lines (see timing.rs) and at the end the slowest
//! ten; LOG.cpu holds the machine's CPU load sampled every few seconds. Running the stages in parallel was tried
//! (2026-10-07) and was worse: 53 min, all four runs failed on timing waits under load.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, sleep};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::GetSystemTimes;

use crate::instance::job;
use crate::timing;

const SAMPLE_WINDOW: Duration = Duration::from_secs(2);
/// The sampler stops by itself, so it cannot outlive a gate that crashed.
const SAMPLER_LIMIT: Duration = Duration::from_secs(90 * 60);

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// (idle, kernel, user) CPU ticks of the whole machine; kernel includes idle.
fn system_times() -> Option<(u64, u64, u64)> {
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut idle, mut kernel, mut user) = (zero, zero, zero);
    // Safety: three valid out-pointers.
    (unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0)
        .then(|| (ticks(idle), ticks(kernel), ticks(user)))
}

/// Total CPU load over the next `window`, in percent, or `None` if it cannot be read or `stop` is set first.
fn cpu_over(window: Duration, stop: &AtomicBool) -> Option<f64> {
    let before = system_times()?;
    let end = Instant::now() + window;
    while Instant::now() < end {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        sleep(Duration::from_millis(50));
    }
    let after = system_times()?;
    let total = (after.1 + after.2).saturating_sub(before.1 + before.2);
    let idle = after.0.saturating_sub(before.0);
    (total > 0).then(|| total.saturating_sub(idle) as f64 * 100.0 / total as f64)
}

fn cpu_now() -> String {
    cpu_over(SAMPLE_WINDOW, &AtomicBool::new(false)).map_or("?".into(), |v| format!("{v:.1}"))
}

fn defender_realtime() -> String {
    Command::new("powershell")
        .args(["-NoProfile", "-Command", "(Get-MpComputerStatus).RealTimeProtectionEnabled"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

fn clock() -> String {
    // Safety: a valid out-pointer.
    let t = unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

fn epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn append(path: &Path, text: &str) {
    if let Err(e) = OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| writeln!(f, "{text}")) {
        eprintln!("gate all: writing {}: {e}", path.display());
    }
}

/// `LOG.suffix` beside `LOG`.
fn sibling(log: &Path, suffix: &str) -> PathBuf {
    let mut s = log.as_os_str().to_owned();
    s.push(".");
    s.push(suffix);
    PathBuf::from(s)
}

struct Gate {
    log: PathBuf,
    cpu_log: PathBuf,
    failed: bool,
}

impl Gate {
    /// Runs `cmd` with its output in `log_file`, then writes the stage's verdict line, its own `TIMING` line
    /// (`gate/NAME`) and the `TIMING` lines `cmd` printed, each with the CPU load around it.
    fn stage(&mut self, name: &str, log_file: &Path, mut cmd: Command) {
        let t0 = Instant::now();
        let s0 = epoch();
        append(&self.log, &format!("GATE {name} start {}", clock()));
        let rc = match File::create(log_file).and_then(|f| Ok((f.try_clone()?, f))) {
            Err(e) => {
                eprintln!("gate all: {}: {e}", log_file.display());
                -1
            }
            Ok((out, err)) => match cmd.stdin(Stdio::null()).stdout(out).stderr(err).spawn() {
                Err(e) => {
                    append(log_file, &format!("spawn failed: {e}"));
                    -1
                }
                Ok(mut child) => {
                    job::assign(&child);
                    child.wait().ok().and_then(|s| s.code()).unwrap_or(-1)
                }
            },
        };
        let took = t0.elapsed().as_secs();
        let text = fs::read_to_string(log_file).unwrap_or_default();
        let count = |w: &str| text.lines().filter(|l| l.contains(w)).count();
        let verdicts = if count("PASS") + count("FAIL") > 0 {
            format!(" pass={} fail={}", count("PASS"), count("FAIL"))
        } else {
            String::new()
        };
        let last = text.lines().rfind(|l| !l.starts_with("TIMING ") && !l.trim().is_empty()).unwrap_or("");
        append(&self.log, &format!("GATE {name} exit={rc} end {} took={took}s last: {last}{verdicts}", clock()));
        append(&self.log, &format!("TIMING gate/{name} {took} {s0}"));
        let samples = timing::parse_samples(&fs::read_to_string(&self.cpu_log).unwrap_or_default());
        for line in timing::collect(name, &text, &samples) {
            append(&self.log, &line);
        }
        self.failed |= rc != 0;
    }
}

pub(crate) fn all(log: Option<PathBuf>, mode: Option<String>) -> ExitCode {
    let mode = mode.as_deref().unwrap_or("full");
    let runs = match mode {
        "full" => 3,
        "light" => 1,
        "ui" => 0,
        _ => {
            eprintln!("mode must be full, light or ui");
            return ExitCode::from(2);
        }
    };
    let Some(log) = log else {
        eprintln!("usage: gate all LOG [full|light|ui]");
        return ExitCode::from(2);
    };
    let me = match std::env::current_exe() {
        Ok(me) => me,
        Err(e) => {
            eprintln!("current_exe: {e}");
            return ExitCode::from(1);
        }
    };
    let cpu_log = sibling(&log, "cpu");
    let _ = fs::remove_file(&cpu_log);

    // The CPU sampler: one line per window until told to stop.
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (stop, cpu_log) = (stop.clone(), cpu_log.clone());
        thread::spawn(move || {
            let deadline = Instant::now() + SAMPLER_LIMIT;
            while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                if let Some(v) = cpu_over(SAMPLE_WINDOW, &stop) {
                    append(&cpu_log, &format!("{} {v:.1}", epoch()));
                }
            }
        })
    };

    append(&log, &format!("GATE start ({mode}) {} cpu={}% defender_rt={}", clock(), cpu_now(), defender_realtime()));
    let mut g = Gate { log: log.clone(), cpu_log, failed: false };
    let gate = |stage: &str| {
        let mut c = Command::new(&me);
        c.arg(stage);
        // The page server's address is for the WebKit stages: the others reach it on loopback.
        c.env_remove("SITE_HOST");
        c
    };
    if mode == "ui" {
        let mut c = Command::new("cargo");
        c.args(["test", "-j", "8", "-p", "manifold", "-p", "ipc-protocol", "-p", "manifold-view-webview2"]);
        g.stage("unit", &sibling(&log, "unit.log"), c);
    }
    for i in 1..=runs {
        g.stage(&format!("two-{i}"), &sibling(&log, &format!("two-{i}.log")), gate("two"));
    }
    if mode != "ui" {
        g.stage("smoke", &sibling(&log, "smoke.log"), gate("smoke"));
        g.stage("launch-forms", &sibling(&log, "launch-forms.log"), gate("launch-forms"));
    }
    g.stage("link", &sibling(&log, "link.log"), gate("link"));
    g.stage("unknown-type", &sibling(&log, "unknown-type.log"), gate("unknown-type"));
    g.stage("closed", &sibling(&log, "closed.log"), gate("closed"));
    g.stage("trail", &sibling(&log, "trail.log"), gate("trail"));
    let mut history = gate("sleep");
    history.env("VIEWS", "webview2").env("CASES", "history");
    g.stage("sleep-history", &sibling(&log, "sleep-history.log"), history);
    if let Ok(ip) = std::env::var("WEBKIT_SITE_HOST") {
        let mut trail = gate("trail");
        trail.env("ENGINE", "webkit").env("SITE_HOST", &ip);
        g.stage("trail-webkit", &sibling(&log, "trail-webkit.log"), trail);
        let mut sleep = gate("sleep");
        sleep.env("VIEWS", "webkit").env("CASES", "manual idle scroll history dirty").env("SITE_HOST", &ip);
        g.stage("sleep-webkit", &sibling(&log, "sleep-webkit.log"), sleep);
    }
    g.stage("layout", &sibling(&log, "layout.log"), gate("layout"));

    append(&log, "GATE slowest steps (seconds, cpu at start, mean cpu):");
    for line in timing::top(&fs::read_to_string(&log).unwrap_or_default(), 10) {
        append(&log, &line);
    }
    append(&log, &format!("GATE done {} cpu={}% defender_rt={}", clock(), cpu_now(), defender_realtime()));
    stop.store(true, Ordering::Relaxed);
    let _ = sampler.join();
    if g.failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
