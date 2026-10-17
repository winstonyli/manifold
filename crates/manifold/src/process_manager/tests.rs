use super::*;
use crate::view_install::{no_download_reason, NO_PIN_REASON};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Page-controlled text (a URL typed, pasted or synced) must reach `wsl.exe` only as hex: whatever
/// the distro's login shell or bash do with quotes, `$`, backticks or newlines, they never see any.
#[test]
fn page_controlled_text_never_reaches_the_wsl_command_line() {
    let token = [7u8; transport::TOKEN_LEN];
    let build = |url: &str| webkit_shell_command(url, 4000, &token, "/mnt/c/repo/target-linux/debug/host").unwrap();
    let benign_url = "https://example.com/";
    let hostile_url = "https://x/'$(touch${IFS}/tmp/pwn)'\";`id`\n&|;<>\\ \u{e9}";
    let (benign, hostile) = (build(benign_url), build(hostile_url));
    assert!(!hostile.contains("pwn") && !hostile.contains('\n'), "{hostile}");
    // The two commands are the same text once each URL's hex is masked: nothing else depends on it.
    assert_eq!(
        benign.replace(&transport::to_hex(benign_url.as_bytes()), "H"),
        hostile.replace(&transport::to_hex(hostile_url.as_bytes()), "H"),
    );
}

#[test]
fn a_url_too_long_for_a_command_line_is_refused_by_name() {
    let token = [7u8; transport::TOKEN_LEN];
    let build = |len: usize| webkit_shell_command(&"a".repeat(len), 4000, &token, "/x");
    assert!(build(8192).is_ok(), "the longest URL that fits");
    assert_eq!(build(8193).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(build(20_000).unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
}

/// The host's reader of one engine connection; `feed` writes the engine's side of it, its version first.
fn reader_fed(
    feed: impl FnOnce(std::net::TcpStream) + Send + 'static,
) -> (mpsc::Receiver<ViewToHost>, view_io::FrameSlot, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || feed(std::net::TcpStream::connect(("127.0.0.1", port)).unwrap()));
    let (stream, _) = listener.accept().unwrap();
    let frame = view_io::FrameSlot::default();
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&wakes);
    let wake: view_io::Wake = Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    });
    (spawn_reader_thread(manifold_view_protocol::MessageChannel::new(stream), Arc::clone(&frame), wake), frame, wakes)
}

/// The host's reader of one engine connection, fed `version` and then a
/// well-formed message; returns the first event it delivers.
fn first_event_after_version(version: u32) -> ViewToHost {
    use std::io::Write as _;
    let (rx, ..) = reader_fed(move |mut stream| {
        stream.write_all(&version.to_le_bytes()).unwrap();
        // Best effort: on a mismatch the reader may already have hung up.
        let _ = manifold_view_protocol::MessageChannel::new(stream).send(&ViewToHost::Cookies("[]".into()));
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

fn frame(n: u32) -> ViewToHost {
    ViewToHost::FrameReady { generation: 1, width: n, height: 1, damage: None, pixels: None }
}

/// Frames never wait in the events channel (survey E3): a burst of them ahead of a message leaves the message first.
#[test]
fn frames_never_queue_on_the_events_channel() {
    let (rx, frame, wakes) = reader_fed(|stream| {
        let mut view_kind = manifold_view_protocol::MessageChannel::new(stream);
        view_kind.send_version().unwrap();
        for n in 0..100 {
            view_kind.send(&frame(n)).unwrap();
        }
        view_kind.send(&ViewToHost::Cookies("[]".into())).unwrap();
    });
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ViewToHost::Cookies(_)), "a frame came first");
    let newest =
        frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take().expect("the frames went to the slot");
    assert_eq!(newest.width, 99, "the newest frame wins");
    // The engine end closed after the Cookies: the reader reports it, waking once more.
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ViewToHost::Crashed { .. }));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while wakes.load(Ordering::SeqCst) < 3 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        3,
        "one wake per message (Cookies, Crashed) and one for the burst's first frame, none for the other 99"
    );
}

/// An idle loop only wakes for what it is told of: a frame into an empty slot wakes it (the page changed by itself),
/// frames piling up behind it do not, and the next frame after the app took the slot wakes it again.
#[test]
fn a_frame_wakes_the_loop_only_when_the_slot_was_empty() {
    let (go, gate) = mpsc::channel::<()>();
    let (_rx, slot, wakes) = reader_fed(move |stream| {
        let mut view_kind = manifold_view_protocol::MessageChannel::new(stream);
        view_kind.send_version().unwrap();
        view_kind.send(&frame(1)).unwrap();
        view_kind.send(&frame(2)).unwrap();
        gate.recv().unwrap();
        view_kind.send(&frame(3)).unwrap();
        let _ = gate.recv();
    });
    let until = |done: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !done() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(done(), "timed out");
    };
    let width = || slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().map(|f| f.width);
    until(&|| width() == Some(2));
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "the second frame found the first still in the slot");
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    go.send(()).unwrap();
    until(&|| width() == Some(3));
    assert_eq!(wakes.load(Ordering::SeqCst), 2, "a frame into the emptied slot wakes the loop again");
    let _ = go.send(());
}

fn damaged(width: u32, height: u32, first_row: u32, rows: u32) -> ViewToHost {
    ViewToHost::FrameReady { generation: 1, width, height, damage: Some((0, first_row, width, rows)), pixels: None }
}

/// Frames the app never took fold into one: same-file damage unions its rows, a size or generation change makes the
/// frame whole.
#[test]
fn piled_up_frames_union_their_rows() {
    for (frames, width, rows) in [
        (vec![damaged(4, 10, 1, 1), damaged(4, 10, 5, 1)], 4, Some(1..6)),
        (vec![damaged(4, 10, 1, 1), damaged(4, 10, 5, 1), damaged(6, 10, 2, 1)], 6, None),
        (
            vec![
                damaged(4, 10, 1, 1),
                ViewToHost::FrameReady { generation: 1, width: 4, height: 10, damage: None, pixels: None },
            ],
            4,
            None,
        ),
        (vec![damaged(4, 10, 2, 1)], 4, Some(2..3)),
        (
            // The same size in another generation is another file: whole.
            vec![
                damaged(4, 10, 1, 1),
                ViewToHost::FrameReady {
                    generation: 2,
                    width: 4,
                    height: 10,
                    damage: Some((0, 5, 4, 1)),
                    pixels: None,
                },
            ],
            4,
            None,
        ),
    ] {
        let (rx, frame_slot, _) = reader_fed(move |stream| {
            let mut view_kind = manifold_view_protocol::MessageChannel::new(stream);
            view_kind.send_version().unwrap();
            for f in &frames {
                view_kind.send(f).unwrap();
            }
            view_kind.send(&ViewToHost::Cookies("[]".into())).unwrap();
        });
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ViewToHost::Cookies(_)));
        let newest = frame_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take().unwrap();
        assert_eq!((newest.width, newest.rows), (width, rows));
    }
}

/// `counter` once it has not moved for `quiet`, or at `deadline`. The `quiet` sleep proves an absence: nothing more
/// got through.
fn still_at(counter: &AtomicUsize, quiet: Duration, deadline: Duration) -> usize {
    let end = std::time::Instant::now() + deadline;
    let mut last = counter.load(Ordering::SeqCst);
    while std::time::Instant::now() < end {
        std::thread::sleep(quiet);
        let now = counter.load(Ordering::SeqCst);
        if now == last {
            return now;
        }
        last = now;
    }
    last
}

/// The events channel is bounded (survey E3): an app that stops draining it stops the reader, which stops the engine
/// on its pipe, and no message is lost.
#[test]
fn an_app_that_stops_draining_stops_the_reader_and_loses_nothing() {
    const N: usize = 1000;
    let written = Arc::new(AtomicUsize::new(0));
    let progress = Arc::clone(&written);
    let (rx, ..) = reader_fed(move |stream| {
        let mut view_kind = manifold_view_protocol::MessageChannel::new(stream);
        view_kind.send_version().unwrap();
        for i in 0..N {
            if view_kind.send(&ViewToHost::Cookies(format!("{}{i}", "0".repeat(65_530)))).is_err() {
                return;
            }
            progress.store(i + 1, Ordering::SeqCst);
        }
    });
    let stalled_at = still_at(&written, Duration::from_millis(300), Duration::from_secs(20));
    assert!(stalled_at < N, "the engine wrote all {N} while the app read none: the channel is unbounded");
    for i in 0..N {
        match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
            ViewToHost::Cookies(s) => assert_eq!(s.trim_start_matches('0').parse::<usize>().unwrap_or(0), i),
            _ => panic!("message {i} is not Cookies"),
        }
    }
}

#[test]
fn a_stale_view_host_is_a_protocol_mismatch_crash() {
    assert!(matches!(first_event_after_version(manifold_view_protocol::PROTOCOL_VERSION), ViewToHost::Cookies(_)));
    match first_event_after_version(manifold_view_protocol::PROTOCOL_VERSION + 1) {
        ViewToHost::Crashed { reason } => {
            assert!(reason.starts_with(manifold_view_protocol::PROTOCOL_MISMATCH), "{reason}")
        }
        other => panic!("expected a protocol-mismatch Crashed, got {other:?}"),
    }
}

/// A second spawn for the same tab id keeps the first one's log.
#[test]
fn per_tab_logs_append() {
    let tab_id = format!("test-log-append-{}", std::process::id());
    let path = per_tab_log_path(&tab_id);
    let _ = std::fs::remove_file(&path);
    drop(per_tab_log_files(&tab_id).unwrap());
    drop(per_tab_log_files(&tab_id).unwrap());
    let log = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(log.matches("spawning a host for").count(), 2, "{log}");
}
#[test]
fn a_log_over_the_cap_is_emptied_at_open() {
    let dir = crate::store::testutil::temp_dir("log-cap");
    let path = dir.join("manifold-t.log");
    std::fs::write(&path, [b'x'; 100]).unwrap();
    drop(open_capped_log(&path, 100).unwrap());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 100, "at the cap is kept");
    drop(open_capped_log(&path, 99).unwrap());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
}

#[test]
fn the_log_sweep_drops_old_tab_logs_and_all_but_the_newest_crash_reports() {
    let dir = crate::store::testutil::temp_dir("log-sweep");
    let now = std::time::SystemTime::now();
    let make = |name: &str, age_days: u64| {
        let f = std::fs::File::create(dir.join(name)).unwrap();
        f.set_modified(now - Duration::from_secs(age_days * 24 * 3600)).unwrap();
    };
    make("manifold-fresh.log", 1);
    make("manifold-stale.log", 20);
    make("manifold-crash-1.log", 3);
    make("manifold-crash-2.log", 2);
    make("manifold-crash-3.log", 1);
    make("other.log", 99);
    let removed = sweep_view_logs_in(&dir, Duration::from_secs(14 * 24 * 3600), 2);
    let mut left: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    left.sort();
    assert_eq!(left, ["manifold-crash-2.log", "manifold-crash-3.log", "manifold-fresh.log", "other.log"]);
    assert_eq!(removed, 2);
}

/// The host copy tests only need a file name; the platform's suffix is not what they check.
const HOST: &str = "manifold-engine-cef";

/// A shell command as a child whose stdout and stderr go to `log`, the
/// way `spawn_tab` starts a host.
fn fake_host(script_windows: &str, script_unix: &str, log: &Path) -> Child {
    fake_host_to(script_windows, script_unix, std::fs::File::create(log).unwrap())
}

/// `fake_host` writing to `out`, e.g. a log opened for appending as `per_tab_log_files` does.
fn fake_host_to(script_windows: &str, script_unix: &str, out: std::fs::File) -> Child {
    let err = out.try_clone().unwrap();
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.raw_arg(format!("/c {script_windows}"));
        let _ = script_unix;
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = Command::new("sh");
        c.args(["-c", script_unix]);
        let _ = script_windows;
        c
    };
    command.stdout(out).stderr(err).spawn().unwrap()
}

#[test]
fn a_host_that_exits_on_start_is_reported_with_its_code_and_log() {
    let dir = crate::store::testutil::temp_dir("host_exits_on_start");
    let log = dir.join("host.log");
    let listeners = transport::listen_local_pair(&format!("test-exit-{}", std::process::id())).unwrap();
    let mut child = fake_host(
        "echo starting & echo libfake.so: cannot open shared object file 1>&2 & exit /b 3",
        "echo starting; echo 'libfake.so: cannot open shared object file' >&2; exit 3",
        &log,
    );
    let started = std::time::Instant::now();
    let err = connect_host(&listeners, &mut child, &log, Duration::from_secs(20)).err().expect("no host connected");
    let message = err.to_string();
    assert!(started.elapsed() < Duration::from_secs(10), "gave up when the child exited, took {:?}", started.elapsed());
    assert!(message.starts_with("view-host exited with code 3 before connecting: "), "{message}");
    assert!(message.contains("starting"), "{message}");
    assert!(message.contains("libfake.so: cannot open shared object file"), "{message}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_host_that_never_connects_is_reported_as_still_running_and_killed() {
    let dir = crate::store::testutil::temp_dir("host_never_connects");
    let log = dir.join("host.log");
    let listeners = transport::listen_local_pair(&format!("test-silent-{}", std::process::id())).unwrap();
    let mut child = fake_host("ping -n 30 127.0.0.1", "sleep 30", &log);
    let err = connect_host(&listeners, &mut child, &log, Duration::from_millis(300)).err().expect("no host connected");
    let message = err.to_string();
    assert!(message.starts_with("view-host is still running but did not connect within 300ms"), "{message}");
    assert!(matches!(child.try_wait(), Ok(Some(_))), "the child was killed and reaped");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn log_tail_keeps_the_last_non_empty_lines() {
    let dir = crate::store::testutil::temp_dir("log_tail");
    let log = dir.join("a.log");
    std::fs::write(&log, "one\n\ntwo\r\nthree\n  \nfour\n").unwrap();
    assert_eq!(log_tail_lines(&log, 2), ["three", "four"]);
    assert_eq!(log_tail_lines(&log, 9), ["one", "two", "three", "four"]);
    assert!(log_tail_lines(&dir.join("missing.log"), 3).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_host_fatal_line_is_the_whole_reason() {
    let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let reason = "CEF runtime doesn't support CEF API version 15200 (the host was built for CEF 152)";
    let fatal = format!("{}{reason}", manifold_view_protocol::HOST_FATAL);
    let log = lines(&[
        &spawn_header("t"),
        "[...:ERROR:cef\\libcef_dll\\libcef_dll2.cc:105] Request for unsupported CEF API version 15200",
        &fatal,
    ]);
    assert_eq!(host_fatal(&log).as_deref(), Some(reason));
    assert_eq!(host_fatal(&lines(&["starting", "libfake.so: cannot open shared object file"])), None);
    // An earlier spawn's fatal line, above the latest header, is not this host's.
    let stale = lines(&[&spawn_header("t"), &fatal, "---- manifold pid 9 spawning a host for t ----", "noise"]);
    assert_eq!(host_fatal(&stale), None);
}

/// Two spawns in one appended log: the first host left a fatal line, the second died silently. The
/// second is reported by its exit code and log, not by the first's reason.
#[test]
fn an_earlier_spawns_fatal_line_is_not_the_reason_for_the_next() {
    let dir = crate::store::testutil::temp_dir("host_fatal_stale");
    let log = dir.join("host.log");
    let old = format!(
        "{}\n{}CEF runtime too old\n{}\n",
        spawn_header("t"),
        manifold_view_protocol::HOST_FATAL,
        spawn_header("t")
    );
    std::fs::write(&log, old).unwrap();
    let listeners = transport::listen_local_pair(&format!("test-stale-{}", std::process::id())).unwrap();
    let out = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    let mut child = fake_host_to("exit /b 4", "exit 4", out);
    let err = connect_host(&listeners, &mut child, &log, Duration::from_secs(20)).err().expect("no host connected");
    let message = err.to_string();
    assert!(message.starts_with("view-host exited with code 4 before connecting: "), "{message}");
    assert!(!message.contains("too old"), "only this spawn's lines: {message}");
    assert_eq!(message.matches("spawning a host for").count(), 1, "{message}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_host_that_exits_with_a_fatal_line_is_reported_by_that_line() {
    let dir = crate::store::testutil::temp_dir("host_fatal_on_start");
    let log = dir.join("host.log");
    let listeners = transport::listen_local_pair(&format!("test-fatal-{}", std::process::id())).unwrap();
    let mut child = fake_host(
        "echo noise 1>&2 & echo view-host fatal: CEF runtime too old 1>&2 & exit /b 1",
        "echo noise >&2; echo 'view-host fatal: CEF runtime too old' >&2; exit 1",
        &log,
    );
    let err = connect_host(&listeners, &mut child, &log, Duration::from_secs(20)).err().expect("no host connected");
    assert_eq!(err.to_string(), "CEF runtime too old");
    let _ = std::fs::remove_dir_all(dir);
}
const HASH: &str = "db3e0751979c3cb3068f732ed4f0ef12ade69d183d98ceee120ad08c8d00f74c";

/// Servo's pinned host: the exe and the two ANGLE DLLs it needs beside it (1 + 2 + 90 MB).
static SERVO_FILES: [crate::host_pin::Component; 3] = [
    crate::host_pin::Component { file: "libEGL.dll", url: "http://x/egl", size: 1_000_000, sha256: HASH },
    crate::host_pin::Component { file: "libGLESv2.dll", url: "http://x/gles", size: 2_000_000, sha256: HASH },
    crate::host_pin::Component {
        file: "manifold-engine-servo.exe",
        url: "http://x/servo",
        size: 90_000_000,
        sha256: HASH,
    },
];
const SERVO_SIZE: u64 = 93_000_000;

/// CEF's pinned host (0.7 MB) and the platform's runtime archive.
static CEF_FILES: [crate::host_pin::Component; 1] =
    [crate::host_pin::Component { file: "manifold-engine-cef.exe", url: "http://x/cef", size: 700_000, sha256: HASH }];

fn servo_plan() -> Result<Plan, String> {
    Ok(Plan { host: &SERVO_FILES, runtime: None })
}

fn cef_plan() -> Result<Plan, String> {
    let pin = crate::cef_pin::pin_for_platform().expect("a pin on a platform with tests");
    Ok(Plan { host: &CEF_FILES, runtime: Some(pin.clone()) })
}

/// CEF with its host and runtime in the given states.
fn cef_disk(host: RuntimeOnDisk, runtime: RuntimeOnDisk) -> OnDisk {
    OnDisk { host, runtime }
}

#[test]
fn view_kinds_that_are_not_downloaded_are_always_installed_where_the_os_can_run_them() {
    let none = Err("unused".to_string());
    let a = |os, kind, disk, installing, failed| availability_with(os, &none, kind, disk, installing, failed, None);
    let kind = ViewKind::Webview2;
    assert!(!downloads(kind));
    assert_eq!(a("windows", kind, OnDisk::both(RuntimeOnDisk::Missing), None, None), ViewStatus::Installed);
    assert_eq!(a("windows", kind, OnDisk::both(RuntimeOnDisk::Stale), Some((1, 2)), Some("x")), ViewStatus::Installed);
}

/// WebKit's host is the Linux ELF, pinned for linux-x64 whichever OS runs manifold (WSL runs it on Windows).
static WEBKIT_FILES: [crate::host_pin::Component; 1] = [crate::host_pin::Component {
    file: "manifold-engine-webkit",
    url: "http://x/webkit",
    size: 7_000_000,
    sha256: HASH,
}];

/// WebKit follows its host on Windows and Linux alike: Ready with the ELF there (a sibling or dev-tree host counts),
/// else offered for download when this build pins it, else Unavailable with the reason; macOS stays unsupported.
#[test]
fn webkit_is_installed_with_its_host_else_offered_or_unavailable() {
    assert!(downloads(ViewKind::Webkit));
    assert_eq!(host_platform(ViewKind::Webkit), "linux-x64");
    assert_eq!(host_platform(ViewKind::Servo), if cfg!(windows) { "windows-x64" } else { "linux-x64" });
    let reason = no_download_reason(ViewKind::Webkit);
    assert_eq!(reason, "this build has no WebKit download");
    let a = |os, plan: &Result<Plan, String>, disk| {
        availability_with(os, plan, ViewKind::Webkit, OnDisk::both(disk), None, None, None)
    };
    let (no_pin, pinned) = (Err(reason.clone()), Ok(Plan { host: &WEBKIT_FILES, runtime: None }));
    for os in ["windows", "linux"] {
        assert_eq!(a(os, &no_pin, RuntimeOnDisk::Ready), ViewStatus::Installed, "{os}");
        assert_eq!(a(os, &no_pin, RuntimeOnDisk::Missing), ViewStatus::Unavailable(reason.clone()), "{os}");
        assert_eq!(a(os, &pinned, RuntimeOnDisk::Missing), ViewStatus::NotInstalled { size: 7_000_000 }, "{os}");
        assert_eq!(a(os, &pinned, RuntimeOnDisk::Stale), ViewStatus::UpdateNeeded { size: 7_000_000 }, "{os}");
        assert_eq!(a(os, &pinned, RuntimeOnDisk::Ready), ViewStatus::Installed, "{os}");
    }
    assert!(matches!(
        check_spawnable(ViewKind::Webkit, &pinned, OnDisk::both(RuntimeOnDisk::Missing), None),
        Err(SpawnError::ViewMissing(ViewKind::Webkit))
    ));
    assert_eq!(SpawnError::ViewMissing(ViewKind::Webkit).to_string(), "WebKit is not installed");
}

/// Where WebKit's host is looked for, in order: beside the exe (a Linux package), a dev tree's
/// `target-linux/debug/` found by walking up from the exe's folder, then the downloaded copy in `engine-hosts/`.
#[test]
fn the_webkit_host_is_found_beside_the_exe_then_in_a_dev_tree_then_in_engine_hosts() {
    let root = crate::store::testutil::temp_dir("webkit-lookup");
    let exe_dir = root.join("repo").join("target").join("debug");
    std::fs::create_dir_all(&exe_dir).unwrap();
    let hosts = root.join("engine-hosts");
    let downloaded = hosts.join("manifold-engine-webkit");
    // Nothing built: the downloaded path, even though no file is there yet (the install puts it there).
    assert_eq!(pick_webkit_host(&exe_dir, Some(&hosts)), Some(downloaded.clone()));
    assert_eq!(pick_webkit_host(&exe_dir, None), None);
    assert!(built_webkit_host(&exe_dir).is_none());
    // A dev tree three folders up, as the old fixed lookup assumed.
    let dev = root.join("repo").join("target-linux").join("debug");
    std::fs::create_dir_all(&dev).unwrap();
    assert!(built_webkit_host(&exe_dir).is_none(), "the folder alone is not a host");
    std::fs::write(dev.join("manifold-engine-webkit"), b"elf").unwrap();
    assert_eq!(built_webkit_host(&exe_dir), Some(dev.join("manifold-engine-webkit")));
    assert_eq!(pick_webkit_host(&exe_dir, Some(&hosts)), Some(dev.join("manifold-engine-webkit")));
    // A host beside the exe wins over both.
    let flat = exe_dir.join("manifold-engine-webkit");
    std::fs::write(&flat, b"elf").unwrap();
    assert_eq!(pick_webkit_host(&exe_dir, Some(&hosts)), Some(flat));
}

/// Servo follows its host files on every OS: Installed when they are there (a sibling host counts, whatever the pin
/// table says), else offered for download when this build pins them, else Unavailable with the reason the lists show.
#[test]
fn servo_is_installed_with_its_host_else_offered_or_unavailable() {
    let reason = no_download_reason(ViewKind::Servo);
    assert_eq!(reason, "this build has no Servo download");
    let a = |os, plan: &Result<Plan, String>, disk, installing, failed| {
        availability_with(os, plan, ViewKind::Servo, OnDisk::both(disk), installing, failed, None)
    };
    let no_pin = Err(reason.clone());
    let missing = ViewStatus::Unavailable(reason.clone());
    for os in ["windows", "linux", "macos"] {
        for plan in [&no_pin, &servo_plan()] {
            assert_eq!(a(os, plan, RuntimeOnDisk::Ready, None, None), ViewStatus::Installed, "{os}");
            assert_eq!(a(os, plan, RuntimeOnDisk::Ready, None, Some("x")), ViewStatus::Installed, "{os}");
        }
        // Nothing pinned and no sibling: Unavailable, an install or failure changing nothing (it cannot exist).
        assert_eq!(a(os, &no_pin, RuntimeOnDisk::Missing, None, None), missing, "{os}");
        assert_eq!(a(os, &no_pin, RuntimeOnDisk::Stale, None, Some("x")), missing, "{os}");
        // Pinned: the same disk is a download.
        let plan = servo_plan();
        assert_eq!(a(os, &plan, RuntimeOnDisk::Missing, None, None), ViewStatus::NotInstalled { size: SERVO_SIZE });
        assert_eq!(a(os, &plan, RuntimeOnDisk::Stale, None, None), ViewStatus::UpdateNeeded { size: SERVO_SIZE });
        assert_eq!(
            a(os, &plan, RuntimeOnDisk::Missing, Some((5, 9)), None),
            ViewStatus::Installing { received: 5, total: 9 }
        );
        assert_eq!(a(os, &plan, RuntimeOnDisk::Missing, None, Some("boom")), ViewStatus::Failed("boom".into()));
    }
    assert_eq!(
        crate::view_info::pick_outcome(ViewKind::Servo, &missing),
        crate::view_info::PickOutcome::Unavailable(ViewKind::Servo, reason.clone())
    );
    assert!(crate::view_info::view_status_label(&missing).0.contains(&reason));
    assert_eq!(
        crate::view_info::view_status_label(&ViewStatus::NotInstalled { size: SERVO_SIZE }).0,
        "Not installed \u{b7} 93 MB"
    );
}

/// A missing Servo host with nothing pinned is refused by name before any launch, not by the os error a spawn would
/// hit; one that is pinned is refused as not installed.
#[test]
fn a_missing_servo_host_is_refused_with_the_reason() {
    let reason = no_download_reason(ViewKind::Servo);
    let none = Err(reason.clone());
    let missing = OnDisk::both(RuntimeOnDisk::Missing);
    let err = check_spawnable(ViewKind::Servo, &none, missing, None).unwrap_err();
    assert!(matches!(&err, SpawnError::ViewUnavailable(ViewKind::Servo, r) if *r == reason), "{err:?}");
    assert_eq!(err.to_string(), format!("Servo is unavailable: {reason}"));
    assert!(check_spawnable(ViewKind::Servo, &none, OnDisk::both(RuntimeOnDisk::Ready), None).is_ok());
    assert!(matches!(
        check_spawnable(ViewKind::Servo, &servo_plan(), missing, None),
        Err(SpawnError::ViewMissing(ViewKind::Servo))
    ));
    assert_eq!(SpawnError::ViewMissing(ViewKind::Servo).to_string(), "Servo is not installed");
    // A custom CEF folder is CEF's alone (spawn_tab drops it for other kinds; it changes nothing here either).
    assert!(check_spawnable(ViewKind::Servo, &none, missing, Some(Path::new("D:/cef"))).is_err());
}

/// The lists' answer for a downloaded engine's host matches the disk the spawn reads: a sibling host (the test exe has
/// none beside it unless one was copied there) is Ready, anything else Missing (the data dir is `None` under test).
#[test]
fn the_lists_read_the_same_host_the_spawn_looks_for() {
    for kind in [ViewKind::Servo, ViewKind::Cef, ViewKind::Webkit] {
        let want = if sibling_host(kind).is_some() { RuntimeOnDisk::Ready } else { RuntimeOnDisk::Missing };
        assert_eq!(view_on_disk(kind).host, want, "{kind:?}");
        assert_eq!(view_on_disk_fresh(kind).host, want, "{kind:?}");
    }
    assert_eq!(view_on_disk(ViewKind::Webview2), OnDisk::both(RuntimeOnDisk::Ready));
}

/// The lists' disk answers are reread only after the TTL or an install/remove (`forget_on_disk`).
#[test]
fn on_disk_answers_are_reused_until_the_ttl_or_forgotten() {
    let cache = OnDiskCache::default();
    let t0 = std::time::Instant::now();
    let reads = std::cell::Cell::new(0);
    let read = |answer| {
        reads.set(reads.get() + 1);
        OnDisk::both(answer)
    };
    let get = |kind, at, answer| on_disk_cached(&cache, kind, at, || read(answer));
    let (missing, ready) = (OnDisk::both(RuntimeOnDisk::Missing), OnDisk::both(RuntimeOnDisk::Ready));
    assert_eq!(get(ViewKind::Cef, t0, RuntimeOnDisk::Missing), missing);
    assert_eq!(get(ViewKind::Cef, t0 + ON_DISK_CHECK_TTL, RuntimeOnDisk::Ready), missing, "reused");
    assert_eq!(reads.get(), 1);
    assert_eq!(get(ViewKind::Servo, t0, RuntimeOnDisk::Ready), ready, "per engine");
    let later = t0 + ON_DISK_CHECK_TTL + Duration::from_millis(1);
    assert_eq!(get(ViewKind::Cef, later, RuntimeOnDisk::Ready), ready, "expired");
    cache.lock().unwrap().clear(); // what `forget_on_disk` does
    assert_eq!(get(ViewKind::Servo, later, RuntimeOnDisk::Missing), missing, "forgotten");
    assert_eq!(reads.get(), 4);
}

/// Every OS case, from any host: the reason text is also what the spawn refusal and the lists show.
#[test]
fn view_kinds_the_os_cannot_run_are_unavailable_with_a_reason() {
    let none = Err("unused".to_string());
    let unavailable =
        |os, kind| availability_with(os, &none, kind, OnDisk::both(RuntimeOnDisk::Ready), None, None, None);
    let reason = |text: &str| ViewStatus::Unavailable(text.to_string());
    for os in ["linux", "macos", "freebsd"] {
        assert_eq!(unavailable(os, ViewKind::Webview2), reason("needs Windows"), "{os}");
    }
    assert_eq!(unavailable("macos", ViewKind::Webkit), reason("needs Windows or Linux"));
    assert_eq!(unavailable("freebsd", ViewKind::Webkit), reason("needs Windows or Linux"));
    assert_eq!(unavailable("windows", ViewKind::Webview2), ViewStatus::Installed);
    assert_eq!(unavailable("windows", ViewKind::Webkit), ViewStatus::Installed);
    assert_eq!(unavailable("linux", ViewKind::Webkit), ViewStatus::Installed);
    // Servo runs everywhere (with its host on disk); CEF follows its pin, not the OS table.
    assert_eq!(unavailable("macos", ViewKind::Servo), ViewStatus::Installed);
    assert_eq!(unavailable("macos", ViewKind::Cef), ViewStatus::Installed);
    // The spawn refusal reads the same table.
    assert_eq!(unsupported_on("windows", ViewKind::Webview2), None);
    assert_eq!(unsupported_on("linux", ViewKind::Servo), None);
    assert_eq!(unsupported_on("linux", ViewKind::Webkit), None);
    let refusal = unsupported_on("macos", ViewKind::Webview2).unwrap();
    assert!(refusal.contains("WebView2") && refusal.contains("needs Windows"), "{refusal}");
    assert!(unsupported_on("macos", ViewKind::Webkit).unwrap().contains("needs Windows or Linux"));
}

#[test]
fn a_linux_webkit_spawn_names_the_missing_native_host() {
    // Available on Linux, but the only host is reached through wsl.exe: refused by name, not by a
    // raw "No such file or directory" from a wsl.exe that isn't there.
    assert!(webkit_host_missing("linux").unwrap().contains("native Linux WebKit host"));
    assert_eq!(webkit_host_missing("windows"), None);
}

#[test]
fn the_platform_default_is_the_first_view_the_os_can_run() {
    assert_eq!(platform_default_view_kind("windows"), ViewKind::Webview2);
    for os in ["linux", "macos", "freebsd"] {
        assert_eq!(platform_default_view_kind(os), ViewKind::Servo, "{os}");
    }
    for os in ["windows", "linux", "macos"] {
        assert!(platform_unavailable_reason(os, platform_default_view_kind(os)).is_none());
    }
}

#[test]
fn cef_state_follows_disk_then_overrides() {
    use RuntimeOnDisk::{Missing, Ready, Stale};
    let pin = crate::cef_pin::pin_for_platform().expect("a pin on a platform with tests");
    let host = CEF_FILES[0].size;
    let plan = cef_plan();
    let a =
        |disk, installing, failed, custom| view_availability(ViewKind::Cef, &plan, disk, installing, failed, custom);
    let ready = cef_disk(Ready, Ready);
    assert_eq!(a(ready, None, None, None), ViewStatus::Installed);
    // Installed needs the host and the runtime both; what is not there is what a download fetches.
    assert_eq!(a(cef_disk(Missing, Missing), None, None, None), ViewStatus::NotInstalled { size: host + pin.size });
    assert_eq!(a(cef_disk(Ready, Missing), None, None, None), ViewStatus::NotInstalled { size: pin.size });
    assert_eq!(a(cef_disk(Missing, Ready), None, None, None), ViewStatus::NotInstalled { size: host });
    assert_eq!(a(cef_disk(Ready, Stale), None, None, None), ViewStatus::UpdateNeeded { size: pin.size });
    assert_eq!(a(cef_disk(Stale, Ready), None, None, None), ViewStatus::UpdateNeeded { size: host });
    assert_eq!(a(cef_disk(Stale, Stale), None, None, None), ViewStatus::UpdateNeeded { size: host + pin.size });
    assert_eq!(a(cef_disk(Stale, Missing), None, None, None), ViewStatus::NotInstalled { size: host + pin.size });
    let missing = cef_disk(Missing, Missing);
    assert_eq!(a(missing, Some((5, 9)), Some("old"), None), ViewStatus::Installing { received: 5, total: 9 });
    assert_eq!(a(missing, None, Some("boom"), None), ViewStatus::Failed("boom".into()));
    assert_eq!(a(cef_disk(Ready, Stale), None, Some("boom"), None), ViewStatus::Failed("boom".into()));
    // Current parts outrank a recorded failure, so the list agrees with what a spawn would do.
    assert_eq!(a(ready, None, Some("boom"), None), ViewStatus::Installed);
    // A bring-your-own runtime replaces the runtime part only: with the host there it is Custom, whatever the runtime.
    let p = Path::new("C:/my/cef");
    assert_eq!(a(cef_disk(Ready, Missing), Some((1, 2)), Some("x"), Some(p)), ViewStatus::Custom(p.to_path_buf()));
    assert_eq!(a(cef_disk(Ready, Stale), None, None, Some(p)), ViewStatus::Custom(p.to_path_buf()));
    // ... and without the host it still needs the host: only the host is offered, never the 170 MB runtime.
    assert_eq!(a(cef_disk(Missing, Missing), None, None, Some(p)), ViewStatus::NotInstalled { size: host });
    assert_eq!(a(cef_disk(Stale, Missing), None, None, Some(p)), ViewStatus::UpdateNeeded { size: host });
    assert_eq!(
        a(cef_disk(Missing, Ready), Some((1, 2)), None, Some(p)),
        ViewStatus::Installing { received: 1, total: 2 }
    );
}

#[test]
fn a_platform_without_a_pin_offers_no_install() {
    let none = Err(NO_PIN_REASON.to_string());
    let a = |disk: OnDisk, installing, failed, custom| {
        availability_with("windows", &none, ViewKind::Cef, disk, installing, failed, custom)
    };
    let unavailable = ViewStatus::Unavailable(NO_PIN_REASON.to_string());
    for disk in [RuntimeOnDisk::Missing, RuntimeOnDisk::Stale] {
        assert_eq!(a(OnDisk::both(disk), None, None, None), unavailable);
        // A recorded failure cannot exist without a pin, and would not change the answer.
        assert_eq!(a(OnDisk::both(disk), None, Some("boom"), None), unavailable);
    }
    // What is already on disk (a dev tree) or configured still works, and other engines are untouched.
    assert_eq!(a(OnDisk::both(RuntimeOnDisk::Ready), None, None, None), ViewStatus::Installed);
    let p = Path::new("C:/my/cef");
    assert_eq!(
        a(cef_disk(RuntimeOnDisk::Ready, RuntimeOnDisk::Missing), None, None, Some(p)),
        ViewStatus::Custom(p.to_path_buf())
    );
    assert_eq!(
        availability_with("windows", &none, ViewKind::Servo, OnDisk::both(RuntimeOnDisk::Ready), None, None, None),
        ViewStatus::Installed
    );
    // The pick says so rather than asking to download, and the label gives the reason.
    assert_eq!(
        crate::view_info::pick_outcome(ViewKind::Cef, &unavailable),
        crate::view_info::PickOutcome::Unavailable(ViewKind::Cef, NO_PIN_REASON.to_string())
    );
    assert!(crate::view_info::view_status_label(&unavailable).0.contains(NO_PIN_REASON));
}

#[test]
fn a_stale_marker_is_update_needed_not_a_spawn() {
    let pin = crate::cef_pin::pin_for_platform().expect("a pin on a platform with tests");
    let plan = cef_plan();
    let stale = cef_disk(RuntimeOnDisk::Ready, RuntimeOnDisk::Stale);
    let state = view_availability(ViewKind::Cef, &plan, stale, None, None, None);
    assert_eq!(state, ViewStatus::UpdateNeeded { size: pin.size });
    assert!(matches!(check_spawnable(ViewKind::Cef, &plan, stale, None), Err(SpawnError::ViewOutdated(ViewKind::Cef))));
    assert_eq!(SpawnError::ViewOutdated(ViewKind::Cef).to_string(), "Blink (CEF) needs an update");
    assert!(matches!(
        check_spawnable(ViewKind::Cef, &plan, cef_disk(RuntimeOnDisk::Ready, RuntimeOnDisk::Missing), None),
        Err(SpawnError::ViewMissing(ViewKind::Cef))
    ));
    // A host that is not installed refuses the spawn too, whatever the runtime.
    assert!(matches!(
        check_spawnable(ViewKind::Cef, &plan, cef_disk(RuntimeOnDisk::Missing, RuntimeOnDisk::Ready), None),
        Err(SpawnError::ViewMissing(ViewKind::Cef))
    ));
    assert!(check_spawnable(ViewKind::Cef, &plan, OnDisk::both(RuntimeOnDisk::Ready), None).is_ok());
    assert!(check_spawnable(ViewKind::Servo, &servo_plan(), OnDisk::both(RuntimeOnDisk::Ready), None).is_ok());
}

#[test]
fn a_custom_directory_allows_a_spawn_the_disk_would_refuse() {
    let custom = Path::new("D:/cef");
    let plan = cef_plan();
    for runtime in [RuntimeOnDisk::Missing, RuntimeOnDisk::Stale] {
        let disk = cef_disk(RuntimeOnDisk::Ready, runtime);
        assert!(check_spawnable(ViewKind::Cef, &plan, disk, None).is_err());
        assert!(check_spawnable(ViewKind::Cef, &plan, disk, Some(custom)).is_ok());
    }
    // The host is still needed.
    let no_host = cef_disk(RuntimeOnDisk::Missing, RuntimeOnDisk::Ready);
    assert!(check_spawnable(ViewKind::Cef, &plan, no_host, Some(custom)).is_err());
}

#[test]
fn the_host_copy_is_made_when_missing_skipped_when_unchanged_and_replaced_when_the_source_changes() {
    use crate::store::testutil::temp_dir;
    let src_dir = temp_dir("host_src");
    let custom = temp_dir("host_custom");
    let host = src_dir.join(HOST);
    std::fs::write(&host, b"v1").unwrap();

    let dest = ensure_host_copy(&custom, &host).unwrap();
    assert_eq!(dest, custom_host_path(&custom, &host));
    assert_eq!(dest.parent().unwrap(), custom);
    assert_eq!(std::fs::read(&dest).unwrap(), b"v1", "copied when missing");

    // Unchanged: a marker written into the copy survives only if it was not recopied.
    let stamp = std::fs::metadata(&dest).unwrap().modified().unwrap();
    ensure_host_copy(&custom, &host).unwrap();
    assert_eq!(std::fs::metadata(&dest).unwrap().modified().unwrap(), stamp);

    std::fs::write(&host, b"version 2").unwrap();
    ensure_host_copy(&custom, &host).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"version 2", "replaced when the source changed");

    let err = ensure_host_copy(&custom, &src_dir.join("nope")).unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

#[test]
fn the_cef_host_is_copied_beside_a_runtime_that_lacks_one() {
    let (custom, runtime) = (Path::new("D:/mine"), PathBuf::from("D:/data/cef-runtime"));
    assert_eq!(host_home(Some(custom), true, Some(runtime.clone())), Some(custom.to_path_buf()));
    assert_eq!(host_home(None, true, Some(runtime.clone())), None, "a dev tree runs its own host");
    assert_eq!(host_home(None, false, Some(runtime.clone())), Some(runtime), "a downloaded runtime has none");
    assert_eq!(host_home(None, false, None), None);
}

#[test]
fn concurrent_spawns_copy_the_host_once_and_leave_no_temp_files() {
    use crate::store::testutil::temp_dir;
    let src_dir = temp_dir("host_race_src");
    let custom = temp_dir("host_race_dest");
    let host = src_dir.join(HOST);
    std::fs::write(&host, vec![7u8; 200_000]).unwrap();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (custom, host) = (custom.clone(), host.clone());
            std::thread::spawn(move || ensure_host_copy(&custom, &host))
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap().unwrap(), custom.join(HOST));
    }
    assert_eq!(std::fs::read(custom.join(HOST)).unwrap(), vec![7u8; 200_000]);
    let names: Vec<_> = std::fs::read_dir(&custom).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(names.len(), 1, "no temp copies left: {names:?}");
}

#[test]
fn a_matching_host_copy_is_current_even_when_it_is_locked() {
    use crate::store::testutil::temp_dir;
    let src_dir = temp_dir("host_lock_src");
    let custom = temp_dir("host_lock_dest");
    let host = src_dir.join(HOST);
    std::fs::write(&host, b"host").unwrap();
    let dest = ensure_host_copy(&custom, &host).unwrap();
    // Held open without share-delete, as a running host holds its exe: a rename over it would fail.
    let mut held = std::fs::OpenOptions::new();
    held.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        held.share_mode(1);
    }
    let _held = held.open(&dest).unwrap();
    assert_eq!(ensure_host_copy(&custom, &host).unwrap(), dest);
}

#[test]
fn a_missing_view_error_reads_plainly() {
    assert_eq!(SpawnError::ViewMissing(ViewKind::Cef).to_string(), "Blink (CEF) is not installed");
}

#[test]
fn only_cef_has_a_downloaded_runtime_dir() {
    // `view_data_dir()` is None under cfg(test), so no real profile is touched.
    assert_eq!(view_runtime_dir(ViewKind::Cef), None);
    assert_eq!(view_runtime_dir(ViewKind::Servo), None);
}

/// Engines that need nothing on disk read as Ready without touching it, cached or fresh; spawn and the engine lists
/// share one reader per engine (`on_disk_reader`).
#[test]
fn bundled_view_kinds_read_ready_without_a_disk_check() {
    assert_eq!(view_on_disk(ViewKind::Webview2), OnDisk::both(RuntimeOnDisk::Ready));
}

/// A dir whose files a live CEF holds (opened without share-delete, as
/// Chromium's SQLite does) survives the sweep untouched; a dead one and
/// one an earlier sweep renamed aside are deleted.
#[cfg(windows)]
#[test]
fn sweep_skips_dirs_still_in_use() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;

    let root = crate::store::testutil::temp_dir("cef-sweep");
    for name in ["1-0", "1-1", "1-2.old"] {
        std::fs::create_dir(root.join(name)).unwrap();
        std::fs::write(root.join(name).join("Cookies"), b"x").unwrap();
    }
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ_WRITE)
        .open(root.join("1-1").join("Cookies"))
        .unwrap();

    super::sweep_cache_dirs(&root, "", Duration::from_secs(3600));
    assert!(root.join("1-0").exists(), "a dir touched within min_age is kept");

    super::sweep_cache_dirs(&root, "", Duration::ZERO);
    let mut left: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
    left.sort();
    assert_eq!(left, ["1-1"]);
    assert!(root.join("1-1").join("Cookies").exists());
}

#[test]
fn the_webview2_sweep_touches_only_manifolds_profile_dirs() {
    let root = crate::store::testutil::temp_dir("wv2-sweep");
    for dir in ["WV2Profile_manifold_8", "WV2Profile_manifold_9.old", "WV2Profile_other", "Default"] {
        std::fs::create_dir(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("WV2Profile_manifold_file"), b"x").unwrap();
    super::sweep_cache_dirs(&root, super::WEBVIEW2_PROFILE_PREFIX, Duration::ZERO);
    let mut left: Vec<_> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
    left.sort();
    assert_eq!(left, ["Default", "WV2Profile_manifold_file", "WV2Profile_other"]);
}

#[test]
fn a_webview2_hosts_profile_dir_is_in_its_default_user_data_folder() {
    let dir = super::webview2_profile_dir(Path::new("bin/manifold-engine-webview2.exe"), 3);
    assert_eq!(dir, Path::new("bin/manifold-engine-webview2.exe.WebView2/EBWebView/WV2Profile_manifold_3"));
}

#[test]
fn an_extracted_webview2_host_keeps_its_user_data_folder_in_its_own_engine_hosts_folder() {
    let hosts = Path::new("data/engine-hosts");
    let host = crate::embedded_host::Embedded::Webview2.host_path(hosts);
    assert_eq!(host, hosts.join("webview2").join("manifold-engine-webview2.exe"));
    assert_eq!(
        super::webview2_profile_dir(&host, 0),
        hosts.join("webview2/manifold-engine-webview2.exe.WebView2/EBWebView/WV2Profile_manifold_0")
    );
}

#[test]
fn profile_slots_are_the_lowest_free_and_reused_once_freed() {
    let mut busy = std::collections::BTreeMap::new();
    let all = |_| Some(());
    assert_eq!([0, 1, 2].map(|_| super::claim_slot(&mut busy, all)), [0, 1, 2]);
    busy.remove(&1);
    assert_eq!(super::claim_slot(&mut busy, all), 1, "a freed slot is reused before a new one");
    assert_eq!(super::claim_slot(&mut busy, all), 3);
}

#[test]
fn a_slot_in_use_elsewhere_is_skipped() {
    let mut busy = std::collections::BTreeMap::new();
    // Slots 0 and 2 are held by another manifold instance (their lock files).
    let free_elsewhere = |n| (n != 0 && n != 2).then_some(());
    assert_eq!(super::claim_slot(&mut busy, free_elsewhere), 1);
    assert_eq!(super::claim_slot(&mut busy, free_elsewhere), 3);
}

#[test]
fn an_exhausted_pool_hands_out_a_slot_beyond_it() {
    let mut busy: std::collections::BTreeMap<usize, ()> = (0..super::WEBVIEW2_PROFILE_POOL).map(|n| (n, ())).collect();
    assert_eq!(super::claim_slot(&mut busy, |_| Some(())), super::WEBVIEW2_PROFILE_POOL);
    busy.remove(&5);
    assert_eq!(super::claim_slot(&mut busy, |_| Some(())), 5, "back inside the pool once one frees");
}

/// A slot whose lock another holder has open is not taken. A profile folder the shared browser still holds open
/// does not block it (its cookies were in memory); a free slot's leftover folder is deleted when it is taken.
#[cfg(windows)]
#[test]
fn taking_a_slot_respects_its_lock_and_reuses_a_profile_folder_in_use() {
    use std::os::windows::fs::OpenOptionsExt;
    let udf = crate::store::testutil::temp_dir("wv2-slots");
    let lock0 = super::take_webview2_slot(&udf, 0).expect("a fresh slot");
    assert!(lock0.is_some());
    assert!(super::take_webview2_slot(&udf, 0).is_none(), "another instance holds slot 0's lock");
    drop(lock0);
    assert!(super::take_webview2_slot(&udf, 0).is_some(), "free again once the lock is closed");

    let profile = udf.join("EBWebView").join("WV2Profile_manifold_1");
    std::fs::create_dir_all(profile.join("Network")).unwrap();
    std::fs::write(profile.join("Network").join("Cookies"), b"x").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(profile.join("Network").join("Cookies"))
        .unwrap();
    assert!(super::take_webview2_slot(&udf, 1).is_some(), "a folder the browser holds does not block the slot");
    assert!(profile.exists(), "a held folder stays");
    drop(held);
    assert!(super::take_webview2_slot(&udf, 1).is_some());
    assert!(!profile.exists(), "the leftover folder is deleted when the slot is taken");
}

/// An engine that stops reading must never block the app: 4096 sends of 64 KiB (far more than a pipe or a loopback
/// socket buffers) return at once (survey E1).
#[test]
fn sends_to_a_view_that_stopped_reading_never_block() {
    let (tab, _peer) = ViewTab::fake(ViewKind::Servo);
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let big = HostToView::SetCookies("x".repeat(64 * 1024));
        for _ in 0..4096 {
            tab.send(&big);
        }
        let _ = done_tx.send(tab);
    });
    let tab = done_rx.recv_timeout(Duration::from_secs(5)).expect("the sends returned although the engine read none");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !tab.writer.is_unresponsive(std::time::Instant::now()) {
        assert!(
            std::time::Instant::now() < deadline,
            "its queue filled and a write is stuck, so it reads as unresponsive"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let start = std::time::Instant::now();
    drop(tab);
    // `fake_fed` has no child process, so `Drop` returns before its stuck-tab rule: this only bounds the rest of `Drop`.
    assert!(start.elapsed() < Duration::from_secs(1), "dropping a stuck tab does not block");
}

/// A writer stuck on a local pipe its engine stopped reading ends when the engine's end closes, as killing the engine
/// closes it (`Drop` relies on this; measured on Windows at about 0.2 ms).
#[test]
fn a_writer_stuck_on_a_local_pipe_ends_when_the_view_end_closes() {
    let id = format!("test-stuck-writer-{}", std::process::id());
    let pair = transport::listen_local_pair(&id).unwrap();
    let view_kind = std::thread::spawn(move || transport::connect_local_pair(&id).unwrap());
    let (to_view, _from_view) = transport::accept_local_pair(&pair, Duration::from_secs(5)).unwrap();
    let view_end = view_kind.join().unwrap();
    let writer = view_io::Writer::spawn(to_view, "test writer");
    writer.wedge();
    writer.shutdown();
    assert!(!writer.wait_finished(Duration::from_millis(100)), "stuck while the engine end is open");
    drop(view_end);
    assert!(writer.wait_finished(Duration::from_secs(5)), "the blocked write failed once the engine end closed");
}
