//! One `manifold --headless` instance the runner drives (the old scripts/lib.sh `launch_headless`/`send`/`wait_file`,
//! the old scripts/winpid.sh, the start/stop pattern of the old scripts/link-probe.sh).
//!
//! The protocol (debug/reader.rs `spawn_stdin_reader`): commands are lines on the app's stdin; there is NO response
//! channel. Results come back as files the app writes (`EXPORTSTATE`, `EXPORTSESSION`, `SCREENSHOT`), so every wait
//! here is a poll with a deadline. `WAITTABS`, `WAITIDLE`, `SLEEP` and `SCREENSHOT` block inside the app's stdin
//! reader (60 s default, 30 s for a screenshot), so later lines simply wait in the pipe; a line is never batched with
//! the next (the first `CLICK` depends on the frame the previous command drew).
//!
//! Output goes to a log file, never an undrained pipe. Every instance is a member of one Windows Job Object with
//! kill-on-close, so a killed or crashed `gate` leaves no `manifold.exe` (and, through manifold's own job, no
//! view host); `Drop` kills the child on the normal paths.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::shots_net::{Offline, Proxy};
use crate::state::State;

/// `BELOW_NORMAL_PRIORITY_CLASS`: the instance never competes with the user's work (LONG_RUNS.md).
pub(crate) const BELOW_NORMAL: u32 = 0x4000;
/// How long an instance may take to exit after QUIT.
pub(crate) const QUIT_TIMEOUT: Duration = Duration::from_secs(60);
/// The poll interval of every wait; the scripts use 0.25 s for files and 0.2 s for a screenshot's size.
const POLL: Duration = Duration::from_millis(100);
/// How long an export may take. The scripts wait 60 s (`WAIT_FILE_SECS`).
const EXPORT_TIMEOUT: Duration = Duration::from_secs(60);
/// The app gives a SCREENSHOT up to 30 s for its next drawn frame; wait that plus a margin.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(35);

/// The Job Object every instance is assigned to (`manifold_view_protocol::job`, as crates/manifold's view-host job).
pub(crate) mod job {
    use std::process::Child;
    use std::sync::OnceLock;

    use manifold_view_protocol::job::{kill_on_close, Job};

    /// Created at the first instance, with kill-on-close: Windows kills every process in it when `gate` exits for any
    /// reason. `None` when it could not be created (said on stderr); a static, so it is never closed early.
    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    /// Adds a just-spawned instance to the job; a failure (say the child already exited) is said on stderr and the
    /// run goes on with `Drop` as the only cleanup.
    pub fn assign(child: &Child) {
        if let Some(job) = JOB.get_or_init(|| kill_on_close("gate job")) {
            job.assign_child(child);
        }
    }
}

fn timed_out(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, what)
}

/// The download folder of an instance seeded with `seed_settings(profile, ..)`: `<profile>\downloads`, absolute, so
/// the app uses it (it ignores a relative one) and a shot can name each file exactly.
pub fn download_dir(profile: &Path) -> io::Result<PathBuf> {
    std::path::absolute(profile.join("downloads"))
}

/// `PROFILE/settings.json` that skips the first-launch demo and starts tabs on `view_kind`. (`--profile-dir DIR`
/// names the manifold folder itself, so there is no `manifold/` level as under `%APPDATA%`.) `more` is extra JSON
/// members, starting with `", "`. The theme is pinned (default `dark`, or `GATE_THEME=light`) so a run does not
/// depend on the machine's own theme, and every instance downloads into its own profile (`download_dir`), never the
/// user's Downloads folder; the app makes that folder at the first download.
pub fn seed_settings(profile: &Path, view_kind: &str, more: &str) -> io::Result<()> {
    fs::create_dir_all(profile)?;
    let theme = if std::env::var("GATE_THEME").is_ok_and(|t| t == "light") { "light" } else { "dark" };
    let downloads = serde_json::Value::from(download_dir(profile)?.to_string_lossy().into_owned());
    fs::write(
        profile.join("settings.json"),
        format!(
            "{{\"has_shown_first_launch_demo\": true, \"default_starting_view_kind\": \"{view_kind}\", \"theme\": \"{theme}\", \"download_directory\": {downloads}{more}}}"
        ),
    )
}

/// A `file://` URL for a Windows path as the address bar and ADDFILE want it: forward slashes, `%20` for a space
/// (those commands split their arguments on whitespace).
pub fn file_url(path: &Path) -> String {
    format!("file:///{}", path.display().to_string().replace('\\', "/").replace(' ', "%20"))
}

/// Polls until `path` exists and is non-empty (the old scripts/lib.sh `wait_file`). The EXPORT* files are replaced
/// atomically, so a non-empty one is whole; a SCREENSHOT is not (see `Instance::screenshot`).
pub fn wait_file(path: &Path, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if fs::metadata(path).is_ok_and(|m| m.len() > 0) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(timed_out(format!("timed out waiting for {}", path.display())));
        }
        sleep(POLL);
    }
}

/// How long a positive wait on the app (a request reaching a probe server, a card changing state) may take: far more
/// than a normal run needs, so a loaded machine costs time and not a spurious failure.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(90);

/// Polls `cond` every 250 ms until it holds; an error naming `what` once `timeout` passes. Every positive wait on an
/// observable goes through this (or `Instance::step` / `export_until`); a fixed `SLEEP` stays only where the app's own
/// timer is what is waited out, or a negative window must pass.
pub fn poll(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    while !cond() {
        if Instant::now() >= deadline {
            return Err(timed_out(format!("timed out waiting for {what}")));
        }
        sleep(Duration::from_millis(250));
    }
    Ok(())
}

/// The logs a failed stage prints the tails of (`run_stage`), in the order first registered.
static LOGS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Adds `path` to the logs a failed stage prints; a path already there stays one entry (a later launch truncated it).
pub fn register_log(path: &Path) {
    let mut logs = LOGS.lock().unwrap_or_else(PoisonError::into_inner);
    if !logs.iter().any(|p| p == path) {
        logs.push(path.to_path_buf());
    }
}

/// Empties the registered logs and returns them.
pub fn take_logs() -> Vec<PathBuf> {
    std::mem::take(&mut *LOGS.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Removes a work dir, retrying while a just-killed instance still holds files in it.
pub fn remove_dir_with_retries(dir: &Path) {
    for _ in 0..5 {
        let _ = fs::remove_dir_all(dir);
        if !dir.exists() {
            return;
        }
        sleep(Duration::from_secs(1));
    }
}

/// The last `n` lines of a log file (empty if unreadable).
pub fn log_tail(log: &Path, n: usize) -> String {
    let text = fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The first command the app didn't know, from its log: it only logs one, and the step that sent it passes doing nothing.
fn unrecognized_command(log: &str) -> Option<&str> {
    log.lines().find(|l| l.contains("Unrecognized debug command"))
}

pub struct Instance {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Where the exports and screenshots go.
    work: PathBuf,
    /// The instance's stdout+stderr.
    pub log: PathBuf,
    /// What the app reaches instead of the internet (`offline`); `None` only for `launch_online`.
    offline: Option<&'static Offline>,
}

/// The gate process's one `Offline`, shared by every instance it launches: WebView2 refuses a second set of browser
/// arguments on a user-data folder already in use (0x8007139f), and every instance from one exe shares that folder, so
/// the proxy address in them can't differ between instances. Each `gate all` stage is its own process.
fn offline() -> io::Result<&'static Offline> {
    static OFFLINE: OnceLock<Offline> = OnceLock::new();
    if let Some(offline) = OFFLINE.get() {
        return Ok(offline);
    }
    let started = Offline::start()?;
    Ok(OFFLINE.get_or_init(|| started))
}

impl Instance {
    /// Starts `exe --headless --profile-dir PROFILE` at BelowNormal in the kill-on-close job, its output in
    /// `WORK/LOG_NAME`, offline (`offline`, shared by the process's instances). The caller seeds the profile first
    /// (`seed_settings`). The exports go to `WORK`.
    pub fn launch(exe: &Path, profile: &Path, work: &Path, log_name: &str) -> io::Result<Instance> {
        Self::launch_env(exe, profile, work, log_name, &[])
    }

    /// `launch` with extra environment variables for the instance (say an `APPDATA`).
    pub fn launch_env(
        exe: &Path,
        profile: &Path,
        work: &Path,
        log_name: &str,
        envs: &[(&str, &str)],
    ) -> io::Result<Instance> {
        let offline = offline()?;
        // The proxy outlives the instance: each launch starts it refusing again.
        offline.proxy.set_refuse(true);
        let mut inst = Self::spawn(exe, profile, work, log_name, &offline.env(envs))?;
        inst.offline = Some(offline);
        Ok(inst)
    }

    /// `launch_env` with the real network, only for `gate package`: its PDF view downloads PDFium on a first run.
    pub fn launch_online(
        exe: &Path,
        profile: &Path,
        work: &Path,
        log_name: &str,
        envs: &[(&str, &str)],
    ) -> io::Result<Instance> {
        Self::spawn(exe, profile, work, log_name, envs)
    }

    /// The proxy this instance's fetches go to (`launch` and `launch_env` only), shared with the process's other
    /// instances.
    pub fn proxy(&self) -> Option<&'static Proxy> {
        self.offline.map(|o| &o.proxy)
    }

    fn spawn(exe: &Path, profile: &Path, work: &Path, log_name: &str, envs: &[(&str, &str)]) -> io::Result<Instance> {
        fs::create_dir_all(work)?;
        let log = work.join(log_name);
        let out = File::create(&log)?;
        register_log(&log);
        let err = out.try_clone()?;
        let mut child = Command::new(exe)
            .arg("--headless")
            .arg("--profile-dir")
            .arg(profile)
            .envs(envs.iter().copied())
            .stdin(Stdio::piped())
            .stdout(out)
            .stderr(err)
            .creation_flags(BELOW_NORMAL)
            .spawn()?;
        job::assign(&child);
        let stdin = child.stdin.take();
        Ok(Instance { child, stdin, work: work.to_path_buf(), log, offline: None })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether the process is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// One command line, flushed at once.
    pub fn send(&mut self, line: &str) -> io::Result<()> {
        let stdin = self.stdin.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stdin is closed"))?;
        writeln!(stdin, "{line}")?;
        stdin.flush()
    }

    /// `WAITTABS` then `WAITIDLE`: the tabs are up and the first layout is done.
    pub fn settle(&mut self) -> io::Result<()> {
        self.send("WAITTABS")?;
        self.send("WAITIDLE")
    }

    /// A fresh path in the work dir; any file there is removed first so a poll cannot see an earlier export.
    fn fresh_path(&self, file: String) -> PathBuf {
        let path = self.work.join(file);
        let _ = fs::remove_file(&path);
        path
    }

    /// Returns once the app has run every command sent before it. `send` only queues a command, so a timer started
    /// right after it also counts the time the app spends on earlier ones (a `WAITIDLE` under load), and a retry on
    /// that timer can repeat a command that simply hadn't run yet.
    pub fn barrier(&mut self) -> io::Result<()> {
        // Named by pid: `gate two`'s instances share one work dir.
        self.export_state(&format!("barrier-{}", self.child.id())).map(drop)
    }

    /// `EXPORTSTATE`, then polls for the file and parses it.
    pub fn export_state(&mut self, name: &str) -> io::Result<State> {
        let path = self.fresh_path(format!("{name}.json"));
        self.send(&format!("EXPORTSTATE {}", path.display()))?;
        wait_file(&path, EXPORT_TIMEOUT)?;
        State::parse(&fs::read_to_string(&path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Where the control named `name` (a tooltip in the export's `controls`) is, `"x y"`: found by name, so a moved
    /// toolbar still works and a missing control fails naming it.
    pub fn control_at(&mut self, name: &str) -> io::Result<String> {
        // The export lists the last finished frame: wait until the one that follows the previous command is drawn.
        self.send("WAITIDLE")?;
        let at = self.export_state("control")?.control(name);
        at.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no control {name:?} on screen")))
    }

    /// Clicks the control named `name`.
    pub fn click_control(&mut self, name: &str) -> io::Result<()> {
        let at = self.control_at(name)?;
        self.send(&format!("CLICK {at}"))
    }

    /// Moves the mouse onto the control named `name`.
    pub fn hover_control(&mut self, name: &str) -> io::Result<()> {
        let at = self.control_at(name)?;
        self.send(&format!("MOUSEMOVE {at}"))
    }

    /// `TEXTOF`, then polls for the file: the page text stored for tab `id`, or `none`.
    pub fn text_of(&mut self, name: &str, id: &str) -> io::Result<String> {
        let path = self.fresh_path(format!("{name}.txt"));
        self.send(&format!("TEXTOF {id} {}", path.display()))?;
        wait_file(&path, EXPORT_TIMEOUT)?;
        fs::read_to_string(&path)
    }

    /// Sends `cmds`, waits (`STEP_TIMEOUT`) for `seen` to hold (an observable of the commands' effect, such as the
    /// probe server having seen the request they cause), then for the app to be idle (`WAITIDLE`: typed input landed,
    /// the focused tab loaded).
    pub fn step(&mut self, cmds: &[&str], what: &str, seen: impl FnMut() -> bool) -> io::Result<()> {
        for c in cmds {
            self.send(c)?;
        }
        poll(what, STEP_TIMEOUT, seen)?;
        self.send("WAITIDLE")
    }

    /// Exports the state (as `name`) until `want` holds of it or `STEP_TIMEOUT` passes; returns the last export either
    /// way, so the caller's own check reports a state that never arrived.
    pub fn export_until(&mut self, name: &str, mut want: impl FnMut(&State) -> bool) -> io::Result<State> {
        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            let state = self.export_state(name)?;
            if want(&state) || Instant::now() >= deadline {
                return Ok(state);
            }
            sleep(Duration::from_millis(250));
        }
    }

    /// `EXPORTSESSION` (a `JsonDoc`: `{nodes: [...], local: ...}`), then polls for the file and parses it.
    pub fn export_session(&mut self, name: &str) -> io::Result<serde_json::Value> {
        let path = self.fresh_path(format!("{name}.json"));
        self.send(&format!("EXPORTSESSION {}", path.display()))?;
        wait_file(&path, EXPORT_TIMEOUT)?;
        serde_json::from_slice(&fs::read(&path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// `SCREENSHOT`, then returns the PNG's path once it is written. The app saves it on its next drawn frame, which
    /// can take seconds when the GPU is busy, and the PNG is not written atomically: after it first shows up
    /// non-empty, wait until its size stops changing (headless-smoke-test.sh `wait_for_file`).
    pub fn screenshot(&mut self, name: &str) -> io::Result<PathBuf> {
        let path = self.fresh_path(format!("{name}.png"));
        self.screenshot_to(&path)?;
        Ok(path)
    }

    /// `screenshot` to a path of the caller's choosing (any file there is replaced).
    pub fn screenshot_to(&mut self, path: &Path) -> io::Result<()> {
        let _ = fs::remove_file(path);
        self.send(&format!("SCREENSHOT {}", path.display()))?;
        wait_file(path, SCREENSHOT_TIMEOUT)?;
        let mut prev = None;
        let deadline = Instant::now() + SCREENSHOT_TIMEOUT;
        loop {
            let now = fs::metadata(path)?.len();
            if prev == Some(now) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(timed_out(format!("{} kept changing size", path.display())));
            }
            prev = Some(now);
            sleep(Duration::from_millis(200));
        }
    }

    /// `QUIT`, then waits for the process to exit (up to `timeout`, after which it is killed and an error returned).
    pub fn quit(&mut self, timeout: Duration) -> io::Result<ExitStatus> {
        // A refused write means the app is already gone, which is what is wanted.
        let _ = self.send("QUIT");
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.stdin = None;
                return match unrecognized_command(&fs::read_to_string(&self.log).unwrap_or_default()) {
                    Some(line) => Err(io::Error::other(line.to_string())),
                    None => Ok(status),
                };
            }
            if Instant::now() >= deadline {
                self.kill();
                return Err(timed_out(format!("instance {} did not exit after QUIT", self.child.id())));
            }
            sleep(POLL);
        }
    }

    /// Kills the instance (TerminateProcess) and reaps it. Its view hosts die with it through manifold's own
    /// kill-on-close job.
    pub fn kill(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_command_in_the_log_is_found() {
        let line = "Unrecognized debug command: \"SLEEPNODE t1\" -- try HELP";
        assert_eq!(unrecognized_command(&format!("started\n{line}\nquit\n")), Some(line));
        assert_eq!(unrecognized_command("started\nquit\n"), None);
    }

    #[test]
    fn launch_registers_its_log_once_and_take_empties_the_list() {
        // One test, not two: the registry is process-wide and tests run in parallel.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let missing_exe = dir.join("no-such-manifold.exe");
        let log = dir.join("x.log");
        assert!(Instance::launch(&missing_exe, &dir.join("profile"), &dir, "x.log").is_err(), "the spawn fails");
        let taken = take_logs();
        assert!(taken.contains(&log), "launch_env registers the log at File::create: {taken:?}");
        // A second launch truncates the same file: still one entry.
        assert!(Instance::launch(&missing_exe, &dir.join("profile"), &dir, "x.log").is_err());
        assert!(Instance::launch(&missing_exe, &dir.join("profile"), &dir, "x.log").is_err());
        let taken = take_logs();
        assert_eq!(taken.iter().filter(|p| **p == log).count(), 1, "deduplicated: {taken:?}");
        assert!(!take_logs().contains(&log), "take empties the list");
    }

    #[test]
    fn every_instance_gets_the_same_webview2_arguments() {
        let arguments = || {
            let env = offline().unwrap().env(&[]);
            env.into_iter().find(|(name, _)| *name == "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap().1.to_string()
        };
        assert_eq!(arguments(), arguments());
    }

    #[test]
    fn file_urls_escape_spaces_and_flip_slashes() {
        assert_eq!(file_url(Path::new(r"C:\Users\Winston Li\x y.txt")), "file:///C:/Users/Winston%20Li/x%20y.txt");
    }

    #[test]
    fn seeded_settings_skip_the_demo() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        seed_settings(&dir, "webview2", ", \"x\": 1").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["has_shown_first_launch_demo"], true);
        assert_eq!(v["default_starting_view_kind"], "webview2");
        assert_eq!(v["x"], 1);
        assert_eq!(v["download_directory"], download_dir(&dir).unwrap().to_str().unwrap());
        assert!(Path::new(v["download_directory"].as_str().unwrap()).is_absolute());
    }

    #[test]
    fn an_instance_downloads_into_its_own_profile_by_full_path() {
        let dir = download_dir(Path::new("rel-profile")).unwrap();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(dir.ends_with(Path::new("rel-profile").join("downloads")), "{}", dir.display());
    }
}
