//! `gate hostrate`: how often a view host sends frames and turns its loop, from the `[frame]` lines every
//! `FrameSink::take` logs when `host_env::ENV_FRAME_STATS` is set. Run by hand, not in `gate all` (its animating floor
//! depends on machine load). For each view kind (`VIEWS`, default `webview2`) and each phase, one headless instance:
//!
//! - `animate` (`/animate`, a page that changes every frame): the host sends at least `ANIMATE_FLOOR` frames/s
//! - `still` (`/text`, plain text): at most 2 frames/s, a poll rate between 0.5 and `STILL_POLL_CEILING` turns/s (the
//!   check that the loop stopped polling a clock), and a control message answered within `LATENCY_BOUND`
//! - `hidden` (`/animate`, then Escape to the Graph so the app hides the tab): no frames, and the still page's poll
//!   rate (a hidden host must not turn at the page's frame rate for frames it throws away)
//!
//! Each prints frames/s, polls/s and the host process's CPU (printed, not checked; the engine's own processes are
//! left out for every view alike). Polls are `take` calls: one per loop turn for CEF, Servo and WebView2, shown or
//! hidden (`[frame] -`).

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use manifold_view_protocol::host_env::ENV_FRAME_STATS;

use crate::check::{check, Section};
use crate::instance::{poll, register_log, Instance, STEP_TIMEOUT};
use crate::probes::{guarded, host_logs, quit, seed, tab_node, views, Site};
use crate::procs::{cpu_time, creation_time, snapshot, Proc};
use crate::state::State;
use crate::windows_main::Ctx;

/// The measuring window of each phase.
const HOSTRATE_WINDOW: Duration = Duration::from_secs(10);
/// Above the 30 frames/s WebView2's old `FRAME_INTERVAL` allowed, so a capped host fails.
const ANIMATE_FLOOR: f64 = 35.0;
/// A host polling on a clock shorter than 50 ms fails; a loop that waits on its inbox polls about once a second.
const STILL_POLL_CEILING: f64 = 20.0;
/// A host that wakes on its inbox answers in tens of ms; one that sees a message only at its 1 s device check, ~1 s.
const LATENCY_BOUND: Duration = Duration::from_millis(400);
const LATENCY_TRIALS: usize = 5;
/// A `[frame]` line this long after the one before it marks a quiet turn (the device check), the worst moment to send.
const QUIET_GAP: Duration = Duration::from_millis(800);
/// After the quiet turn, this long with no host turn before sending: WebView2's engine often posts its own pump message
/// 13-43 ms after a device check, and a send just before it measured that turn instead of a missing wake (2026-11-09).
const SETTLE: Duration = Duration::from_millis(100);

const PHASES: [(&str, &str); 3] = [("animate", "/animate"), ("still", "/text"), ("hidden", "/animate")];

/// A host log read so far: its path and how many bytes of complete lines have been taken.
struct LogTail {
    path: PathBuf,
    read: u64,
}

impl LogTail {
    /// The complete lines appended since the last call.
    fn new_lines(&mut self) -> io::Result<Vec<String>> {
        let mut file = fs::File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.read))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else { return Ok(Vec::new()) };
        self.read += end as u64 + 1;
        Ok(String::from_utf8_lossy(&bytes[..end]).lines().map(str::to_owned).collect())
    }

    /// How many of the new lines are `[frame]` lines.
    fn new_frame_lines(&mut self) -> io::Result<usize> {
        Ok(self.new_lines()?.iter().filter(|l| l.starts_with("[frame] ")).count())
    }
}

/// The host process the instance started: its child named `manifold-view-*`, created after it.
fn find_host(inst: &Instance, label: &str) -> io::Result<Proc> {
    let pid = inst.pid();
    let mut host = None;
    poll(&format!("{label}: a view host under instance {pid}"), STEP_TIMEOUT, || {
        let all = snapshot();
        let Some(root) = all.iter().find(|p| p.pid == pid) else { return false };
        host = all
            .iter()
            .find(|p| p.ppid == root.pid && p.created >= root.created && p.name.starts_with("manifold-view-"))
            .cloned();
        host.is_some()
    })?;
    Ok(host.expect("poll returned Ok only after finding one"))
}

fn alive(p: &Proc) -> bool {
    creation_time(p.pid) == Some(p.created)
}

pub(crate) fn hostrate(ctx: &mut Ctx) -> io::Result<()> {
    let (exe, work) = (ctx.exe.as_path(), ctx.work.as_path());
    for view in views() {
        for (phase, path) in PHASES {
            let _t = Section::start(&format!("hostrate-{view}-{phase}"));
            let label = format!("hostrate {view} {phase}");
            guarded(&label, || run_phase(exe, work, &view, phase, path, &label));
        }
    }
    Ok(())
}

fn run_phase(exe: &Path, work: &Path, view: &str, phase: &str, path: &str, label: &str) -> io::Result<()> {
    let site = Site::start()?;
    let dir = work.join(format!("{view}-{phase}"));
    let tmp = dir.join("tmp");
    fs::create_dir_all(&tmp)?;
    let profile = dir.join("appdata");
    seed(exe, &profile, view, &[tab_node(Some(&site.url(path)), view)])?;
    let tmp_s = tmp.display().to_string();
    let envs = [("TEMP", tmp_s.as_str()), ("TMP", tmp_s.as_str()), (ENV_FRAME_STATS, "1")];
    let mut inst = Instance::launch_env(exe, &profile, &dir, "log", &envs)?;
    inst.settle()?;
    inst.step(&["GRAPHDOWN", "GRAPHOPEN"], &format!("the {path} page to load"), || site.hits(path) > 0)?;
    inst.export_until(&format!("tabview-{view}-{phase}"), State::tab_view)?;
    inst.send("WAITIDLE")?;
    let host = find_host(&inst, label)?;
    // Registered now, not at the end, so a failure anywhere below prints the host's tail.
    let logs = host_logs(&tmp);
    logs.iter().for_each(|l| register_log(l));
    if logs.len() != 1 {
        return Err(io::Error::other(format!("{} host logs in {}, wanted 1", logs.len(), tmp.display())));
    }
    let mut log = LogTail { path: logs[0].clone(), read: 0 };
    log.new_lines()?;
    if phase == "hidden" {
        inst.send("ESCAPE")?;
        // The window starts once the host has applied `SetVisible(false)`: its first hidden turn.
        poll(&format!("{label}: a hidden turn ([frame] -) after leaving Tab view"), STEP_TIMEOUT, || {
            log.new_lines().is_ok_and(|lines| lines.iter().any(|l| l == "[frame] -"))
        })?;
    }

    // The window: frames and polls from the log, CPU from the host process.
    let (cpu0, start) = (cpu_time(host.pid).unwrap_or_default(), Instant::now());
    let (mut polls, mut frames) = (0usize, 0usize);
    while start.elapsed() < HOSTRATE_WINDOW {
        if !alive(&host) {
            return Err(io::Error::other(format!("the host (pid {}) exited during the window", host.pid)));
        }
        sleep(Duration::from_millis(100));
    }
    let (cpu1, secs) = (cpu_time(host.pid).unwrap_or_default(), start.elapsed().as_secs_f64());
    for line in log.new_lines()? {
        if line.starts_with("[frame] ") {
            polls += 1;
            frames += usize::from(line == "[frame] 1");
        }
    }
    let (frame_rate, poll_rate) = (frames as f64 / secs, polls as f64 / secs);
    let cpu_pct = (cpu1.saturating_sub(cpu0)).as_secs_f64() / secs * 100.0;
    println!("{label}: {frame_rate:.1} frames/s, {poll_rate:.0} polls/s, host cpu {cpu_pct:.2}% of a core");
    let none = if polls == 0 { " (no [frame] lines)" } else { "" };

    let mut times = Vec::new();
    if phase == "still" {
        for trial in 0..LATENCY_TRIALS {
            let target = if trial % 2 == 0 { "/tall" } else { "/text" };
            times.push(answer_time(&mut inst, &site, &mut log, target)?);
        }
        let shown: Vec<String> = times.iter().map(|t| format!("{}", t.as_millis())).collect();
        println!("{label}: control-message answer times {} ms", shown.join(" "));
    }
    quit(&mut inst, label);

    let short = format!("hostrate {view}");
    if phase == "animate" {
        check(
            &format!("{short}: an animating page sends at least {ANIMATE_FLOOR} frames/s (got {frame_rate:.1}{none})"),
            frame_rate >= ANIMATE_FLOOR,
        );
    } else if phase == "hidden" {
        check(&format!("{short}: a hidden tab sends no frames (got {frames}{none})"), polls > 0 && frames == 0);
        check(
            &format!(
                "{short}: a hidden animating tab polls between 0.5 and {STILL_POLL_CEILING} times/s (got {poll_rate:.1}{none})"
            ),
            (0.5..=STILL_POLL_CEILING).contains(&poll_rate),
        );
    } else {
        check(
            &format!("{short}: a still page sends at most 2 frames/s (got {frame_rate:.1}{none})"),
            frame_rate <= 2.0,
        );
        check(
            &format!(
                "{short}: a still page polls between 0.5 and {STILL_POLL_CEILING} times/s (got {poll_rate:.1}{none})"
            ),
            (0.5..=STILL_POLL_CEILING).contains(&poll_rate),
        );
        times.sort_unstable();
        let median = times[times.len() / 2];
        check(
            &format!(
                "{short}: the host answers a control message within {} ms (median {} ms)",
                LATENCY_BOUND.as_millis(),
                median.as_millis()
            ),
            median < LATENCY_BOUND,
        );
    }
    Ok(())
}

/// One control-latency trial: types `target`'s URL, waits for a quiet host turn, presses Enter and times the request.
fn answer_time(inst: &mut Instance, site: &Site, log: &mut LogTail, target: &str) -> io::Result<Duration> {
    // The typed text needs two frames to land, and a URL typed over a load still pending is lost (`sleep.rs`).
    inst.send(&format!("ADDRESSBAR {}", site.url(target)))?;
    inst.send("WAITIDLE")?;
    inst.barrier()?;
    let before = site.hits(target);
    // Send soon after a host turn (`SETTLE`): a host with no wake then waits nearly a whole device-check period. A turn some window
    // message woke would leave it less, so wait for a line after a quiet stretch; a polling host never goes quiet,
    // so after 3 s take the next line.
    log.new_lines()?;
    let (begin, mut last) = (Instant::now(), Instant::now());
    loop {
        if begin.elapsed() > Duration::from_secs(5) {
            return Err(io::Error::other("no [frame] line from the host in 5 s"));
        }
        if log.new_frame_lines()? > 0 {
            let now = Instant::now();
            if now - last >= QUIET_GAP || begin.elapsed() >= Duration::from_secs(3) {
                break;
            }
            last = now;
        }
        sleep(Duration::from_millis(2));
    }
    let mut settled = Instant::now();
    while settled.elapsed() < SETTLE && begin.elapsed() < Duration::from_secs(5) {
        if log.new_frame_lines()? > 0 {
            settled = Instant::now();
        }
        sleep(Duration::from_millis(2));
    }
    inst.send("ENTER")?;
    let sent = Instant::now();
    while site.hits(target) <= before {
        if sent.elapsed() > Duration::from_secs(5) {
            return Err(io::Error::other(format!("no request for {target} within 5 s of Enter")));
        }
        sleep(Duration::from_millis(2));
    }
    let took = sent.elapsed();
    inst.send("WAITIDLE")?;
    Ok(took)
}
