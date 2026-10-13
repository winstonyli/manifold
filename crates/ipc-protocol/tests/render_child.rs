use ipc_protocol::render::{channel, run_child};
use std::process::Command;
use std::time::{Duration, Instant};

fn stub(mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_render_stub"));
    command.arg(mode);
    command
}

fn run(mode: &str, timeout: Duration) -> Result<ipc_protocol::render::Frame, String> {
    let (_cancel, inbox) = channel();
    run_child(stub(mode), "PDF renderer", timeout, inbox, |c| c.spawn())
}

const LONG: Duration = Duration::from_secs(30);

#[test]
fn a_good_frame_comes_back() {
    let frame = run("ok", LONG).unwrap();
    assert_eq!((frame.width, frame.height, frame.page_count, frame.rgba.len()), (2, 1, 3, 8));
}

#[test]
fn a_failure_reports_the_last_stderr_line() {
    assert_eq!(run("fail", LONG).unwrap_err(), "Couldn't read the file");
}

#[test]
fn a_crash_is_named() {
    assert_eq!(run("crash", LONG).unwrap_err(), "PDF renderer crashed");
}

#[test]
fn garbage_and_short_output_are_a_bad_frame() {
    for mode in ["garbage", "short"] {
        let err = run(mode, LONG).unwrap_err();
        assert!(err.starts_with("PDF renderer returned a bad frame"), "{mode}: {err}");
    }
}

#[test]
fn a_hang_times_out() {
    let start = Instant::now();
    assert_eq!(run("hang", Duration::from_millis(300)).unwrap_err(), "PDF renderer timed out");
    assert!(start.elapsed() < Duration::from_secs(10), "the hung child was killed, not waited for");
}

#[test]
fn a_valid_frame_wins_over_a_lingering_child() {
    let start = Instant::now();
    assert_eq!(run("linger", LONG).unwrap().page_count, 3);
    assert!(start.elapsed() < Duration::from_secs(10), "the child that kept running was killed, not waited for");
}

#[test]
fn a_cancel_kills_a_running_child() {
    let (cancel, inbox) = channel();
    let started = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let flag = started.clone();
    let worker = std::thread::spawn(move || {
        run_child(stub("hang"), "PDF renderer", LONG, inbox, move |c| {
            let child = c.spawn()?;
            *flag.0.lock().unwrap() = true;
            flag.1.notify_all();
            Ok(child)
        })
    });
    let (lock, cvar) = &*started;
    let guard = cvar.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |s| !*s).unwrap().0;
    assert!(*guard, "the child started");
    drop(guard);
    cancel.cancel();
    assert_eq!(worker.join().unwrap().unwrap_err(), "cancelled");
}

#[test]
fn a_missing_binary_is_not_installed() {
    let (_cancel, inbox) = channel();
    let err =
        run_child(Command::new("definitely-not-here-xyz"), "PDF renderer", LONG, inbox, |c| c.spawn()).unwrap_err();
    assert!(err.starts_with("PDF renderer is not installed"), "{err}");
}
