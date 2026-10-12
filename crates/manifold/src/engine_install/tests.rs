#[test]
fn unpacked_files_keep_exec_bits_but_never_setuid_or_group_write() {
    assert_eq!(unpacked_mode(0o100755), 0o755);
    assert_eq!(unpacked_mode(0o644), 0o644);
    assert_eq!(unpacked_mode(0o4755), 0o755, "setuid dropped: CEF runs with --no-sandbox");
    assert_eq!(unpacked_mode(0o777), 0o755);
}

use super::*;
use crate::store::testutil::temp_dir;
use std::io::BufRead;
use std::net::TcpListener;

const PIN: Pin = Pin { version: "1.0", url: "http://x/a", size: 10, sha1: "aa", sha256: "bb" };

/// A plan that is only CEF's runtime archive (no host files pinned), as the runtime tests install.
fn runtime_plan(pin: &Pin) -> Plan {
    Plan { host: &[], runtime: Some(pin.clone()) }
}

/// `runtime_plan` of the platform's real pin, in the form the state functions take.
fn cef_plan() -> Result<Plan, String> {
    Ok(runtime_plan(cef_pin()))
}

/// The disk of an engine whose host is there and whose runtime is `runtime`.
fn disk(runtime: RuntimeOnDisk) -> OnDisk {
    OnDisk { host: RuntimeOnDisk::Ready, runtime }
}

#[test]
fn engine_state_is_reused_within_a_frame_and_refreshed_when_an_install_changes() {
    let mut app = App::default();
    let kind = EngineKind::Cef;
    app.engine_ui.engine_state_cache.borrow_mut().insert(kind, EngineState::Failed("cached".into()));
    assert_eq!(app.engine_state(kind), EngineState::Failed("cached".into()), "reused");
    app.cancel_engine_install(kind);
    assert_ne!(app.engine_state(kind), EngineState::Failed("cached".into()), "refreshed");
    app.engine_ui.engine_state_cache.borrow_mut().insert(kind, EngineState::Failed("cached".into()));
    app.engine_ui.engine_state_cache.borrow_mut().clear(); // what each new frame does
    assert_ne!(app.engine_state(kind), EngineState::Failed("cached".into()));
}

#[test]
fn remove_deletes_the_runtime_and_its_leftovers_only_when_idle() {
    let dir = temp_dir("remove_runtime");
    std::fs::create_dir_all(dir.join("cef-runtime")).unwrap();
    std::fs::write(dir.join("cef-runtime").join(lib()), vec![0u8; 5]).unwrap();
    std::fs::write(dir.join("cef-runtime.part"), b"keep").unwrap();
    assert_eq!(dir_size(&dir.join("cef-runtime")), 5);
    assert!(remove_engine_files(&dir, EngineKind::Cef, &[], true, false).is_err());
    assert!(remove_engine_files(&dir, EngineKind::Cef, &[], false, true).is_err());
    assert!(dir.join("cef-runtime").exists());
    remove_engine_files(&dir, EngineKind::Cef, &[], false, false).unwrap();
    assert!(!dir.join("cef-runtime").exists());
    assert!(!dir.join("cef-runtime.part").exists(), "Remove also clears the leftovers");
    remove_engine_files(&dir, EngineKind::Cef, &[], false, false).unwrap(); // already gone is fine
}

#[test]
fn a_missing_directory_is_missing() {
    assert_eq!(runtime_state(&temp_dir("ei1").join("nope"), &PIN), RuntimeOnDisk::Missing);
}

#[test]
fn a_marker_with_the_pins_version_is_ready_and_another_is_stale() {
    let dir = temp_dir("ei2");
    std::fs::write(dir.join(MARKER), "1.0").unwrap();
    assert_eq!(runtime_state(&dir, &PIN), RuntimeOnDisk::Ready);
    std::fs::write(dir.join(MARKER), "0.9").unwrap();
    assert_eq!(runtime_state(&dir, &PIN), RuntimeOnDisk::Stale);
}

/// Versions in the forms the CDN index, `archive.json` and the cef crate use, around a 152.0.6 pin.
const V151: &str = "151.3.24+g2384915+chromium-151.0.7922.174";
const V152: &str = "152.0.6+g708dc14+chromium-152.0.7977.83";
const V154: &str = "154.0.33+ga03e714+chromium-154.0.8037.94";

#[test]
fn runtime_compat_compares_the_cef_major() {
    assert_eq!(runtime_compat(V152, V152), Compat::Current);
    assert_eq!(runtime_compat(V152, "152.0.4+gabc+chromium-152.0.1.2"), Compat::Current);
    assert_eq!(runtime_compat(V152, "152.1.0+152.0.6"), Compat::Current);
    assert_eq!(runtime_compat(V152, " 152.0.6\n"), Compat::Current, "a marker's newline");
    assert_eq!(runtime_compat(V152, "152"), Compat::Current);
    assert_eq!(runtime_compat(V152, V154), Compat::Newer);
    assert_eq!(runtime_compat(V152, V151), Compat::Older);
    assert_eq!(runtime_compat(V152, "99.0.1"), Compat::Older, "numbers, not text");
    assert_eq!(runtime_compat(V152, "1000.0.0"), Compat::Newer);
    for garbage in ["", "garbage", "v152.0.6", "152x.0", ".152", "-152.0"] {
        assert_eq!(runtime_compat(V152, garbage), Compat::Unknown, "{garbage:?}");
        assert_eq!(runtime_compat(garbage, V152), Compat::Unknown, "{garbage:?}");
    }
}

/// The downloaded runtime's marker: the pin or a later major is Installed, anything else Update needed.
#[test]
fn the_marker_is_installed_for_the_pin_or_a_later_major_and_update_needed_otherwise() {
    let pin = Pin { version: V152, ..PIN };
    let app = App::default();
    let dir = temp_dir("compat-marker");
    let state = |marker: &str| {
        std::fs::write(dir.join(MARKER), marker).unwrap();
        let on_disk = runtime_state(&dir, &pin);
        (on_disk, app.engine_state_at(EngineKind::Cef, disk(on_disk), &cef_plan()))
    };
    let update = (RuntimeOnDisk::Stale, EngineState::UpdateNeeded { size: cef_pin().size });
    let installed = (RuntimeOnDisk::Ready, EngineState::Installed);
    assert_eq!(state(V152), installed);
    assert_eq!(state(V154), installed, "installed by a newer app: the host runs on it");
    assert_eq!(state(V151), update);
    assert_eq!(state("152.0.4+gabc+chromium-152.0.1.2"), update, "a re-pin within a major still updates");
    assert_eq!(state("garbage"), update);
}

#[test]
fn a_custom_directory_of_the_pins_major_or_later_validates_and_an_earlier_one_does_not() {
    let pin = Pin { version: V152, ..PIN };
    let dir =
        |tag: &str, version: &str| custom_dir(tag, Some(&crate::cef_platform::PLATFORM.archive_name(version)), true);
    let newer = dir("compat-c154", V154);
    assert_eq!(validate_custom(&newer, &pin), Ok(()));
    assert_eq!(newer_than_tested(&newer, &pin).as_deref(), Some("CEF 154.0.33 is newer than the tested 152.0.6"));
    let same_major = dir("compat-c152", "152.0.4+gabc+chromium-152.0.1.2");
    assert_eq!(validate_custom(&same_major, &pin), Ok(()));
    assert_eq!(newer_than_tested(&same_major, &pin), None);
    let err = validate_custom(&dir("compat-c151", V151), &pin).unwrap_err();
    assert!(err.contains(V151) && err.contains(V152), "{err}");
    assert!(validate_custom(&dir("compat-cbad", "garbage"), &pin).is_err());
}

/// CEF 154's minimal archive has no `libEGL.dll`/`libGLESv2.dll` (ANGLE is inside libcef): a runtime
/// without them is complete, both as our install and as a bring-your-own directory.
#[test]
fn a_runtime_without_angle_dlls_counts_as_installed() {
    let pin = cef_pin();
    let dir = custom_dir("compat-noangle", None, true);
    std::fs::write(dir.join(MARKER), pin.version).unwrap();
    assert!(!dir.join("libEGL.dll").exists() && !dir.join("libGLESv2.dll").exists());
    assert_eq!(runtime_state(&dir, pin), RuntimeOnDisk::Ready);
    assert_eq!(
        App::default().engine_state_at(EngineKind::Cef, disk(runtime_state(&dir, pin)), &cef_plan()),
        EngineState::Installed
    );
    assert_eq!(validate_custom(&dir, pin), Ok(()));
}

/// The pin of the platform the tests run on (Windows or Linux).
fn cef_pin() -> &'static Pin {
    crate::cef_pin::pin_for_platform().expect("a pin on a platform with tests")
}

/// The runtime library's file name on this platform.
fn lib() -> &'static str {
    crate::cef_platform::PLATFORM.runtime_lib
}

/// A bring-your-own directory as CEF's own archive lays it out: the runtime library and `archive.json`.
fn custom_dir(tag: &str, archive_name: Option<&str>, dll: bool) -> PathBuf {
    let dir = temp_dir(tag);
    if dll {
        std::fs::write(dir.join(lib()), b"x").unwrap();
    }
    if let Some(name) = archive_name {
        std::fs::write(dir.join("archive.json"), format!(r#"{{"type":"minimal","name":"{name}","sha1":"ab"}}"#))
            .unwrap();
    }
    dir
}

#[test]
fn a_custom_directory_of_the_pinned_version_validates() {
    let pin = cef_pin();
    let name = crate::cef_platform::PLATFORM.archive_name(pin.version);
    assert_eq!(validate_custom(&custom_dir("vc1", Some(&name), true), pin), Ok(()));
    // The pin's own sample: a name without the prefix or suffix still reads.
    assert_eq!(validate_custom(&custom_dir("vc1b", Some(pin.version), true), pin), Ok(()));
}

#[test]
fn a_custom_directory_of_another_version_names_both() {
    let dir =
        custom_dir("vc2", Some(&crate::cef_platform::PLATFORM.archive_name("150.1.2+gabc+chromium-150.0.1")), true);
    let err = validate_custom(&dir, cef_pin()).unwrap_err();
    assert!(err.contains("150.1.2+gabc+chromium-150.0.1"), "{err}");
    assert!(err.contains(cef_pin().version), "{err}");
}

#[test]
fn a_custom_directory_without_archive_json_or_libcef_is_refused() {
    let pin = cef_pin();
    assert!(validate_custom(&custom_dir("vc3", None, true), pin).unwrap_err().contains("archive.json"));
    let name = crate::cef_platform::PLATFORM.archive_name(pin.version);
    assert!(validate_custom(&custom_dir("vc4", Some(&name), false), pin).unwrap_err().contains(lib()));
    assert!(validate_custom(&temp_dir("vc5").join("nope"), pin).is_err());
    let bad = custom_dir("vc6", None, true);
    std::fs::write(bad.join("archive.json"), "{}").unwrap();
    assert!(validate_custom(&bad, pin).is_err());
}

#[test]
fn a_runtime_we_installed_validates_by_its_marker() {
    // Our install leaves the runtime library and `MARKER` (the pin's version), no `archive.json`.
    let pin = cef_pin();
    let ours = custom_dir("vm1", None, true);
    std::fs::write(ours.join(MARKER), pin.version).unwrap();
    assert_eq!(validate_custom(&ours, pin), Ok(()));

    let stale = custom_dir("vm2", None, true);
    std::fs::write(stale.join(MARKER), "150.1.2+gabc+chromium-150.0.1\n").unwrap();
    let err = validate_custom(&stale, pin).unwrap_err();
    assert!(err.contains("150.1.2+gabc+chromium-150.0.1") && err.contains(pin.version), "{err}");

    // No marker and no archive.json: both sources are named.
    let none = validate_custom(&custom_dir("vm3", None, true), pin).unwrap_err();
    assert!(none.contains("archive.json") && none.contains(MARKER), "{none}");
}

#[test]
fn archive_json_outranks_the_marker() {
    let pin = cef_pin();
    let other = crate::cef_platform::PLATFORM.archive_name("150.1.2+gabc+chromium-150.0.1");
    let dir = custom_dir("vm4", Some(&other), true);
    std::fs::write(dir.join(MARKER), pin.version).unwrap();
    assert!(validate_custom(&dir, pin).unwrap_err().contains("150.1.2"));
    // An invalid archive.json is an error even with a good marker beside it.
    std::fs::write(dir.join("archive.json"), "{}").unwrap();
    assert!(validate_custom(&dir, pin).unwrap_err().contains("archive.json"));
}

#[test]
fn a_custom_directory_deleted_later_stops_being_custom() {
    let pin = cef_pin();
    let dir = custom_dir("vc9", Some(&crate::cef_platform::PLATFORM.archive_name(pin.version)), true);
    let mut app = App::default();
    app.set_cef_directory(dir.to_str().unwrap());
    assert!(app.cef_dir_verdict().unwrap().1.is_ok());
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(app.cef_dir_verdict().unwrap().1.is_ok(), "still inside the TTL: the cached verdict stands");
    // Age the cached verdict past the TTL, as waiting would.
    let aged = std::time::Instant::now().checked_sub(CEF_DIR_CHECK_TTL * 2).unwrap();
    app.engine_ui.cef_dir_check.borrow_mut().as_mut().unwrap().1 = aged;
    assert!(app.cef_dir_verdict().unwrap().1.is_err());
    assert_eq!(app.custom_cef_dir(), None);
}

#[test]
fn a_valid_custom_directory_makes_cef_custom_and_an_invalid_one_does_not() {
    let pin = cef_pin();
    let good = custom_dir("vc7", Some(&crate::cef_platform::PLATFORM.archive_name(pin.version)), true);
    let mut app = App::default();
    app.set_cef_directory(good.to_str().unwrap());
    assert_eq!(
        app.engine_state_at(EngineKind::Cef, disk(RuntimeOnDisk::Missing), &cef_plan()),
        EngineState::Custom(good.clone())
    );
    // The CEF folder is CEF's alone: Servo still follows its own host exe.
    assert_eq!(
        app.engine_state_at(
            EngineKind::Servo,
            OnDisk::both(RuntimeOnDisk::Ready),
            &Err(no_download_reason(EngineKind::Servo))
        ),
        EngineState::Installed
    );
    assert!(matches!(
        app.engine_state_at(
            EngineKind::Servo,
            OnDisk::both(RuntimeOnDisk::Missing),
            &Err(no_download_reason(EngineKind::Servo))
        ),
        EngineState::Unavailable(_)
    ));

    let bad = custom_dir("vc8", Some(&crate::cef_platform::PLATFORM.archive_name("1.0")), true);
    app.set_cef_directory(bad.to_str().unwrap());
    assert_eq!(app.custom_cef_dir(), None);
    assert!(app.cef_dir_verdict().unwrap().1.is_err(), "the page shows why");
    // No silent fallback and no Custom: the normal on-disk state.
    assert_eq!(
        app.engine_state_at(EngineKind::Cef, disk(RuntimeOnDisk::Missing), &cef_plan()),
        EngineState::NotInstalled { size: pin.size }
    );
    assert_eq!(app.engine_state_at(EngineKind::Cef, disk(RuntimeOnDisk::Ready), &cef_plan()), EngineState::Installed);

    app.set_cef_directory("  ");
    assert_eq!(app.settings.cef_directory, None);
    assert!(app.cef_dir_verdict().is_none());
}

#[test]
fn a_directory_without_a_marker_is_missing_not_ready() {
    let dir = temp_dir("ei3");
    std::fs::write(dir.join(lib()), b"x").unwrap();
    assert_eq!(runtime_state(&dir, &PIN), RuntimeOnDisk::Missing);
}

/// How the test server answers a request that carries `Range: bytes=N-`.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// 206 with a correct Content-Range.
    Honour,
    /// 200 with the whole body.
    Ignore,
    /// 206 whose Content-Range starts at 0 instead of N.
    WrongStart,
    /// 416.
    Refuse,
}

/// Serve `body` on a local port, answering ranged requests per `mode`. `cut_first` closes the first
/// connection after that many body bytes (the headers still promise the full length).
fn serve(body: Arc<Vec<u8>>, mode: Mode, cut_first: Option<usize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/f", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut cut = cut_first;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut start = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    start = v.trim().trim_end_matches('-').parse().unwrap_or(0);
                }
            }
            let len = body.len();
            let ranged = start > 0;
            if ranged && mode == Mode::Refuse {
                let _ = stream.write_all(
                    b"HTTP/1.1 416 Range Not Satisfiable
Content-Length: 0
Connection: close

",
                );
                continue;
            }
            // The first byte the Content-Range claims; a wrong-start server then sends the whole body
            // from byte 0 under it.
            let claimed = start;
            if ranged && matches!(mode, Mode::Ignore | Mode::WrongStart) {
                start = 0;
            }
            let head = if ranged && mode != Mode::Ignore {
                let shown = if mode == Mode::WrongStart { 0 } else { claimed };
                format!(
                    "HTTP/1.1 206 Partial Content
Content-Range: bytes {shown}-{}/{len}
Content-Length: {}
Connection: close

",
                    len - 1,
                    len - start
                )
            } else {
                format!(
                    "HTTP/1.1 200 OK
Content-Length: {len}
Connection: close

"
                )
            };
            let mut payload = &body[start..];
            if let Some(n) = cut.take() {
                payload = &payload[..n.min(payload.len())];
            }
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(payload);
        }
    });
    url
}

fn body(n: usize) -> Arc<Vec<u8>> {
    Arc::new((0..n).map(|i| (i % 251) as u8).collect())
}

#[test]
fn download_fetches_everything() {
    let b = body(300_000);
    let url = serve(b.clone(), Mode::Honour, None);
    let part = temp_dir("ei4").join("f.part");
    let mut last = 0;
    download(&url, &part, b.len() as u64, &mut |n| last = n, &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(&part).unwrap(), *b);
    assert_eq!(last, b.len() as u64);
}

#[test]
fn download_aborts_past_the_pinned_size() {
    let b = body(300_000);
    let url = serve(b.clone(), Mode::Honour, None);
    let part = temp_dir("ei-cap").join("f.part");
    let total = 100_000u64; // the pin says 100 000 bytes; the server sends 300 000
    let mut seen = 0;
    let err = download(&url, &part, total, &mut |n| seen = n, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(&err, InstallError::Network(m) if m.contains("more than")), "{err:?}");
    assert!(seen <= total, "progress never passes the pin: {seen}");
    assert!(!part.exists(), "a stream that overran the pin is not kept for a resume");
}

#[test]
fn download_resumes_after_a_cut_connection() {
    let b = body(300_000);
    let url = serve(b.clone(), Mode::Honour, Some(100_000));
    let part = temp_dir("ei5").join("f.part");
    let cancel = AtomicBool::new(false);
    let err = download(&url, &part, b.len() as u64, &mut |_| {}, &cancel).unwrap_err();
    assert!(matches!(err, InstallError::Network(_)), "{err:?}");
    let kept = std::fs::metadata(&part).unwrap().len();
    assert!(kept > 0 && kept < b.len() as u64, "kept {kept}");
    let mut first = 0;
    download(
        &url,
        &part,
        b.len() as u64,
        &mut |n| {
            if first == 0 {
                first = n
            }
        },
        &cancel,
    )
    .unwrap();
    assert!(first > kept, "progress counts resumed bytes: {first} vs {kept}");
    assert_eq!(std::fs::read(&part).unwrap(), *b);
}

#[test]
fn download_restarts_when_the_server_ignores_range() {
    let b = body(200_000);
    let url = serve(b.clone(), Mode::Ignore, None);
    let part = temp_dir("ei6").join("f.part");
    std::fs::write(&part, &b[..50_000]).unwrap();
    download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(&part).unwrap(), *b);
}

#[test]
fn download_stops_when_cancelled() {
    let b = body(300_000);
    let url = serve(b.clone(), Mode::Honour, None);
    let part = temp_dir("ei7").join("f.part");
    let cancel = AtomicBool::new(false);
    let err =
        download(&url, &part, b.len() as u64, &mut |_| cancel.store(true, Ordering::Relaxed), &cancel).unwrap_err();
    assert_eq!(err, InstallError::Cancelled);
    let kept = std::fs::metadata(&part).unwrap().len();
    assert!(kept > 0 && kept < b.len() as u64, "kept {kept}");
}

#[test]
fn download_fails_on_a_short_body_without_content_length() {
    let b = body(100_000);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/f", listener.local_addr().unwrap());
    let half = b[..40_000].to_vec();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK
Connection: close

",
        );
        let _ = stream.write_all(&half);
    });
    let part = temp_dir("ei9").join("f.part");
    let err = download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::Network(_)), "{err:?}");
    assert_eq!(std::fs::metadata(&part).unwrap().len(), 40_000);
}

/// The installer fetches on the shared agent (`net::agent`), so it sends manifold's User-Agent.
#[test]
fn download_sends_the_user_agent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/f", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap_or(0);
        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase());
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK
Content-Length: 3
Connection: close

abc",
        );
    });
    let part = temp_dir("ei-ua").join("f.part");
    download(&url, &part, 3, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let request = rx.recv().unwrap();
    assert!(request.contains("user-agent: manifold/"), "{request}");
}

#[test]
fn download_restarts_when_content_range_starts_elsewhere() {
    let b = body(200_000);
    let url = serve(b.clone(), Mode::WrongStart, None);
    let part = temp_dir("ei10").join("f.part");
    std::fs::write(&part, &b[..50_000]).unwrap();
    download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(&part).unwrap(), *b);
}

#[test]
fn download_retries_from_zero_after_416_and_fails_if_that_fails_too() {
    let b = body(200_000);
    let url = serve(b.clone(), Mode::Refuse, None);
    let part = temp_dir("ei11").join("f.part");
    std::fs::write(&part, &b[..50_000]).unwrap();
    download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(&part).unwrap(), *b);
    // A server that answers every request 416 fails the retry too.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/f", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 416 Range Not Satisfiable
Content-Length: 0
Connection: close

",
            );
        }
    });
    let part = temp_dir("ei12").join("f.part");
    std::fs::write(&part, &b[..50_000]).unwrap();
    let err = download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::Network(_)), "{err:?}");
}

#[test]
fn download_cancelled_beforehand_opens_nothing() {
    let part = temp_dir("ei13").join("f.part");
    let err = download("http://127.0.0.1:1/f", &part, 10, &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
    assert_eq!(err, InstallError::Cancelled);
    assert!(!part.exists());
}

#[test]
fn verify_rejects_a_wrong_hash_and_accepts_the_right_one() {
    let path = temp_dir("ei8").join("f");
    std::fs::write(&path, b"hello").unwrap();
    let sha256: &'static str = Box::leak(to_hex(&Sha256::digest(b"hello")).into_boxed_str());
    let sha1: &'static str = Box::leak(to_hex(&Sha1::digest(b"hello")).into_boxed_str());
    let good = Pin { version: "1", url: "u", size: 5, sha1, sha256 };
    verify(&path, &good).unwrap();
    let bad256 = Pin { sha256: "00", ..good.clone() };
    assert!(matches!(verify(&path, &bad256), Err(InstallError::BadHash { .. })));
    let bad1 = Pin { sha1: "00", ..good };
    assert!(matches!(verify(&path, &bad1), Err(InstallError::BadHash { .. })));
}

/// A `tar.bz2` holding `entries` (path, bytes); a path is written raw, so `..` survives.
fn archive(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let mut tar = tar::Builder::new(Vec::new());
    for (path, data) in entries {
        let mut h = tar::Header::new_gnu();
        h.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path.as_bytes());
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append(&h, *data).unwrap();
    }
    bz(dir, "a.tar.bz2", &tar.into_inner().unwrap())
}

fn bz(dir: &Path, name: &str, raw: &[u8]) -> PathBuf {
    let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
    enc.write_all(raw).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, enc.finish().unwrap()).unwrap();
    path
}

const GOOD: [(&str, &[u8]); 4] = [
    ("top/Release/runtime.bin", b"dll"),
    ("top/Resources/resources.pak", b"pak"),
    ("top/include/x.h", b"h"),
    ("top/Release/libcef.lib", b"lib"),
];

/// A pin for the archive at `path`, served at `url`.
fn pin_for(path: &Path, url: String) -> Pin {
    let bytes = std::fs::read(path).unwrap();
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    Pin {
        version: "9.9",
        url: leak(url),
        size: bytes.len() as u64,
        sha1: leak(to_hex(&Sha1::digest(&bytes))),
        sha256: leak(to_hex(&Sha256::digest(&bytes))),
    }
}

#[test]
fn unpack_keeps_runtime_files_and_drops_leftovers() {
    let dir = temp_dir("ei20");
    let a = archive(&dir, &GOOD);
    let out = dir.join("out");
    unpack(&a, &out, &pin_for(&a, String::new()), &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(out.join("runtime.bin")).unwrap(), b"dll");
    assert_eq!(std::fs::read(out.join("resources.pak")).unwrap(), b"pak");
    assert!(!out.join("x.h").exists() && !out.join("include").exists());
    // An import library is a Windows build leftover; elsewhere `.lib` is not special.
    assert_eq!(!out.join("libcef.lib").exists(), !crate::cef_platform::PLATFORM.skip_suffixes.is_empty());
    assert_eq!(std::fs::read_to_string(out.join(MARKER)).unwrap(), "9.9");
}

#[test]
fn a_corrupt_tar_header_is_a_bad_archive_not_a_file_error() {
    let dir = temp_dir("ei22");
    // 512 bytes of junk: not a valid header, and its checksum cannot match.
    let a = bz(&dir, "junk.tar.bz2", &[b'x'; 1024]);
    let err = unpack(&a, &dir.join("out"), &pin_for(&a, String::new()), &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::Unpack(_)), "{err:?}");
}

#[test]
fn unpack_refuses_a_path_that_escapes() {
    let dir = temp_dir("ei21");
    let a = archive(&dir, &[("top/Release/runtime.bin", b"x"), ("top/Release/../../evil", b"x")]);
    // `into` is nested so that a naive join lands at a place inside `dir` that we can check.
    let nested = dir.join("nested");
    let out = nested.join("out");
    let err = unpack(&a, &out, &pin_for(&a, String::new()), &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::Unpack(_)), "{err:?}");
    assert!(!dir.join("evil").exists() && !out.join(MARKER).exists());
    for (name, lands) in [
        ("top/Release/../evil", nested.join("evil")),
        ("top/Release/..\\evil", nested.join("evil")),
        ("top\\Release\\..\\..\\evil", dir.join("evil")),
        ("../evil", dir.join("evil")),
        ("/top/Release/evil", PathBuf::from("/top/Release/evil")),
    ] {
        let a = archive(&dir, &[(name, b"x")]);
        let r = unpack(&a, &out, &pin_for(&a, String::new()), &AtomicBool::new(false));
        assert!(matches!(r, Err(InstallError::Unpack(_))), "{name}: {r:?}");
        assert!(!lands.exists(), "{name} wrote {lands:?}");
    }
}

#[test]
fn unpack_refuses_names_with_a_colon() {
    // `C:evil.dll` is drive-relative: `join` would let it replace the destination; `a:b` is a stream.
    let dir = temp_dir("ei40");
    let out = dir.join("out");
    for name in ["top/Release/C:evil.dll", "top/Release/a:b", "top/Release/sub/D:x", "top/Resources/c:"] {
        let a = archive(&dir, &[(name, b"x")]);
        let r = unpack(&a, &out, &pin_for(&a, String::new()), &AtomicBool::new(false));
        assert!(matches!(r, Err(InstallError::Unpack(_))), "{name}: {r:?}");
        assert!(!out.join(MARKER).exists(), "{name}");
    }
    assert!(target(Path::new("top/Release/C:evil.dll"), &out).is_err());
    assert!(target(Path::new("top/Release/runtime.bin"), &out).unwrap().is_some());
}

#[test]
fn unpack_stops_when_cancelled() {
    let dir = temp_dir("ei41");
    let a = archive(&dir, &GOOD);
    let r = unpack(&a, &dir.join("out"), &pin_for(&a, String::new()), &AtomicBool::new(true));
    assert_eq!(r, Err(InstallError::Cancelled));
    assert!(!dir.join("out").join(MARKER).exists());
}

#[test]
fn a_cancel_before_unpack_keeps_the_part_and_makes_no_runtime() {
    let dir = temp_dir("ei42");
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    // The part is already complete, so the download returns at once; the first unpack entry sees the flag.
    std::fs::copy(&a, data.join("cef-runtime.part")).unwrap();
    let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
    assert_eq!(err, InstallError::Cancelled);
    assert!(data.join("cef-runtime.part").exists(), "the part is kept");
    assert!(!data.join("cef-runtime").exists() && !data.join("cef-runtime.tmp").exists());
}

#[test]
fn a_cancel_once_the_download_has_started_changes_nothing() {
    let dir = temp_dir("ei43a");
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
    std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
    // Cancelled from the progress callback: the download loop itself sees it (not the checks after it).
    let cancel = AtomicBool::new(false);
    let err = install(&pin, &data, &mut |_| cancel.store(true, Ordering::Relaxed), &cancel).unwrap_err();
    assert_eq!(err, InstallError::Cancelled);
    assert!(data.join("cef-runtime.part").exists());
    assert!(!data.join("cef-runtime.tmp").exists());
    assert!(data.join("cef-runtime").join("old").exists(), "no swap happened");
}

/// An install whose cancel flag flips at the `n`th unpack entry (counting from 1); `n` past the last
/// entry never flips it. Returns the result and the data dir, which holds a previous install.
fn install_cancelled_at_entry(name: &str, n: usize) -> (Result<PathBuf, InstallError>, PathBuf, usize) {
    let dir = temp_dir(name);
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
    std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
    let cancel = AtomicBool::new(false);
    let mut seen = 0;
    let r = install_with(&pin, &data, &mut |_| {}, &cancel, &mut || {
        seen += 1;
        if seen == n {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    (r, data, seen)
}

#[test]
fn a_cancel_during_unpack_removes_tmp_and_keeps_the_part() {
    let (r, data, seen) = install_cancelled_at_entry("ei43", 2);
    assert_eq!(r, Err(InstallError::Cancelled));
    assert_eq!(seen, 2, "the unpack stopped at the next entry");
    assert!(data.join("cef-runtime.part").exists());
    assert!(!data.join("cef-runtime.tmp").exists(), "install removed the half-filled tmp");
    assert!(data.join("cef-runtime").join("old").exists());
}

#[test]
fn a_cancel_after_the_last_entry_stops_before_the_swap() {
    // A dry run counts the entries; flipping at the last one lets the whole unpack finish.
    let (r, _, entries) = install_cancelled_at_entry("ei43b", usize::MAX);
    assert!(r.is_ok());
    let (r, data, _) = install_cancelled_at_entry("ei43c", entries);
    assert_eq!(r, Err(InstallError::Cancelled));
    assert!(data.join("cef-runtime.part").exists());
    assert!(!data.join("cef-runtime.tmp").exists());
    assert!(!data.join("cef-runtime.old").exists());
    assert!(data.join("cef-runtime").join("old").exists(), "the previous install is untouched");
    assert!(!data.join("cef-runtime").join(MARKER).exists(), "nothing new was swapped in");
}

#[test]
fn a_stalled_download_times_out_keeps_the_part_and_resumes() {
    let b = body(300_000);
    let (url, release) = serve_held(b.clone());
    let part = temp_dir("ei44").join("f.part");
    let started = std::time::Instant::now();
    let err = download_with_idle(
        &url,
        &part,
        b.len() as u64,
        &mut |_| {},
        &AtomicBool::new(false),
        Duration::from_millis(400),
    )
    .unwrap_err();
    assert!(matches!(err, InstallError::Network(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let kept = std::fs::metadata(&part).unwrap().len();
    assert!(kept > 0 && kept < b.len() as u64, "kept {kept}");
    release.send(()).unwrap();
    // A resume from the kept part finishes the file.
    let url = serve(b.clone(), Mode::Honour, None);
    download(&url, &part, b.len() as u64, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(&part).unwrap(), *b);
}

#[test]
fn a_stalled_download_is_cancelled_promptly() {
    let b = body(300_000);
    let (url, release) = serve_held(b.clone());
    let part = temp_dir("ei45").join("f.part");
    let cancel = AtomicBool::new(false);
    let err = download_with_idle(
        &url,
        &part,
        b.len() as u64,
        &mut |_| cancel.store(true, Ordering::Relaxed),
        &cancel,
        Duration::from_secs(60),
    )
    .unwrap_err();
    assert_eq!(err, InstallError::Cancelled);
    release.send(()).unwrap();
}

#[test]
fn a_failed_swap_removes_tmp_and_keeps_the_part() {
    let dir = temp_dir("ei46");
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    // A file where `cef-runtime.old` should be a directory: the swap cannot clear it.
    std::fs::write(data.join("cef-runtime.old"), b"x").unwrap();
    std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
    std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
    let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::Io(_)), "{err:?}");
    assert!(!data.join("cef-runtime.tmp").exists(), "tmp removed");
    assert!(data.join("cef-runtime.part").exists(), "a file error keeps the part");
    assert!(data.join("cef-runtime").join("old").exists());
}

#[test]
fn remove_deletes_the_marker_first_and_the_leftovers() {
    let dir = temp_dir("ei47");
    let live = dir.join("cef-runtime");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::write(live.join(MARKER), "v").unwrap();
    std::fs::write(live.join(lib()), b"x").unwrap();
    std::fs::write(dir.join("cef-runtime.part"), b"x").unwrap();
    std::fs::create_dir_all(dir.join("cef-runtime.tmp")).unwrap();
    std::fs::create_dir_all(dir.join("cef-runtime.old")).unwrap();
    assert!(has_leftovers(&dir, EngineKind::Cef, &[]));
    remove_engine_files(&dir, EngineKind::Cef, &[], false, false).unwrap();
    assert!(!live.exists() && !has_leftovers(&dir, EngineKind::Cef, &[]));
    // Leftovers with no runtime: still deletable.
    std::fs::write(dir.join("cef-runtime.part"), b"x").unwrap();
    remove_engine_files(&dir, EngineKind::Cef, &[], false, false).unwrap();
    assert!(!has_leftovers(&dir, EngineKind::Cef, &[]));
}

#[test]
fn unpack_refuses_a_symlink() {
    let dir = temp_dir("ei22");
    let mut tar = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_path("top/Release/link").unwrap();
    h.set_link_name("/etc/passwd").unwrap();
    h.set_size(0);
    h.set_cksum();
    tar.append(&h, std::io::empty()).unwrap();
    let a = bz(&dir, "s.tar.bz2", &tar.into_inner().unwrap());
    let err = unpack(&a, &dir.join("out"), &pin_for(&a, String::new()), &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::Unpack(_)), "{err:?}");
}

/// Serve the archive at `a`.
fn serve_file(a: &Path) -> String {
    serve(Arc::new(std::fs::read(a).unwrap()), Mode::Honour, None)
}

#[test]
fn install_end_to_end_against_the_local_server() {
    let dir = temp_dir("ei23");
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    let mut last = 0;
    let out = install(&pin, &data, &mut |n| last = n, &AtomicBool::new(false)).unwrap();
    assert_eq!(out, data.join("cef-runtime"));
    assert_eq!(runtime_state(&out, &pin), RuntimeOnDisk::Ready);
    assert_eq!(last, pin.size);
    assert!(!data.join("cef-runtime.part").exists() && !data.join("cef-runtime.tmp").exists());
    // A second install replaces the first.
    std::fs::write(out.join("stale.txt"), b"x").unwrap();
    install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert!(!out.join("stale.txt").exists() && out.join("runtime.bin").exists());
}

#[test]
fn install_clears_a_stale_tmp_and_old() {
    let dir = temp_dir("ei27");
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("cef-runtime.tmp")).unwrap();
    std::fs::write(data.join("cef-runtime.tmp").join("junk"), b"x").unwrap();
    std::fs::create_dir_all(data.join("cef-runtime.old")).unwrap();
    std::fs::write(data.join("cef-runtime.old").join("junk"), b"x").unwrap();
    let out = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(runtime_state(&out, &pin), RuntimeOnDisk::Ready);
    assert!(!out.join("junk").exists());
    assert!(!data.join("cef-runtime.tmp").exists() && !data.join("cef-runtime.old").exists());
}

#[test]
fn a_failed_swap_restores_the_previous_install() {
    let dir = temp_dir("ei28");
    let live = dir.join("cef-runtime");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::write(live.join(MARKER), "old").unwrap();
    // The new directory does not exist, so the second rename fails.
    let err = swap(&dir.join("cef-runtime.tmp"), &live).unwrap_err();
    assert!(matches!(err, InstallError::Io(_)), "{err:?}");
    assert_eq!(std::fs::read_to_string(live.join(MARKER)).unwrap(), "old");
    assert!(!dir.join("cef-runtime.old").exists());
}

#[test]
fn a_bad_hash_leaves_no_runtime_directory_and_no_part_file() {
    let dir = temp_dir("ei24");
    let a = archive(&dir, &GOOD);
    let pin = Pin { sha256: "00", ..pin_for(&a, serve_file(&a)) };
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("cef-runtime.tmp")).unwrap();
    let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::BadHash { .. }), "{err:?}");
    assert!(!data.join("cef-runtime").exists() && !data.join("cef-runtime.tmp").exists());
    assert!(!data.join("cef-runtime.part").exists());
}

#[test]
fn a_failed_unpack_deletes_tmp_and_part_and_keeps_the_previous_install() {
    let dir = temp_dir("ei25");
    let a = archive(&dir, &[("../evil", b"x")]);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
    std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
    let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::Unpack(_)), "{err:?}");
    assert!(data.join("cef-runtime").join("old").exists());
    assert!(!data.join("cef-runtime.tmp").exists() && !data.join("cef-runtime.part").exists());
}

#[test]
fn a_network_failure_keeps_the_part_file_and_the_previous_install() {
    let dir = temp_dir("ei26");
    let a = archive(&dir, &GOOD);
    let bytes = std::fs::read(&a).unwrap();
    let pin = pin_for(&a, serve(Arc::new(bytes), Mode::Honour, Some(50)));
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("cef-runtime")).unwrap();
    std::fs::write(data.join("cef-runtime").join("old"), b"x").unwrap();
    let err = install(&pin, &data, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, InstallError::Network(_)), "{err:?}");
    assert_eq!(std::fs::metadata(data.join("cef-runtime.part")).unwrap().len(), 50);
    assert!(data.join("cef-runtime").join("old").exists());
}

// ---- the install job on the App ----

use crate::App;

/// The state the engine list would show for CEF with `data` as the data directory.
fn cef_state(app: &App, data: &Path, pin: &Pin) -> EngineState {
    app.engine_state_at(EngineKind::Cef, disk(runtime_state(&data.join("cef-runtime"), pin)), &Ok(runtime_plan(pin)))
}

/// Poll until no job is left (the thread has finished and been collected).
fn settle(app: &mut App) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !app.engine_ui.engine_installs.is_empty() {
        app.poll_engine_installs();
        assert!(std::time::Instant::now() < deadline, "the job never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn toasts(app: &App) -> Vec<String> {
    app.toasts.save_toast_queue.iter().cloned().collect()
}

#[test]
fn a_job_installs_and_toasts() {
    let dir = temp_dir("ei30");
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    let mut app = App::default();
    assert!(matches!(cef_state(&app, &data, &pin), EngineState::NotInstalled { .. }));
    assert!(app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&pin)));
    assert!(matches!(cef_state(&app, &data, &pin), EngineState::Installing { total, .. } if total == pin.size));
    settle(&mut app);
    assert_eq!(cef_state(&app, &data, &pin), EngineState::Installed);
    assert_eq!(toasts(&app), ["Blink (CEF) installed"]);
    assert!(app.engine_ui.engine_failures.is_empty());
}

#[test]
fn an_install_waits_while_a_tab_is_still_starting() {
    let dir = temp_dir("ei30b");
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let data = dir.join("data");
    let mut app = App::default();
    let pending =
        crate::session_sync::Snapshot { kind: crate::store::model::Kind::Tab, group: None, fields: Default::default() };
    app.spawns.spawning.insert("t1".into(), pending);
    assert!(!app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&pin)));
    assert!(app.engine_ui.engine_installs.is_empty());
    assert_eq!(toasts(&app), ["A tab is still starting; try installing Blink (CEF) again in a moment"]);
    app.spawns.spawning.clear();
    assert!(app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&pin)));
    settle(&mut app);
}

#[test]
fn a_failed_job_records_the_reason_and_a_retry_clears_it() {
    let dir = temp_dir("ei31");
    let a = archive(&dir, &GOOD);
    let good = pin_for(&a, serve_file(&a));
    let bad = Pin { sha256: "0000", ..good.clone() };
    let data = dir.join("data");
    let mut app = App::default();
    assert!(app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&bad)));
    settle(&mut app);
    let EngineState::Failed(message) = cef_state(&app, &data, &bad) else { panic!("not failed") };
    assert!(message.contains("checksum"), "{message}");
    assert_eq!(toasts(&app), [format!("Blink (CEF) install failed: {message}")]);
    assert!(app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&good)));
    assert!(matches!(cef_state(&app, &data, &good), EngineState::Installing { .. }), "failure cleared");
    settle(&mut app);
    assert_eq!(cef_state(&app, &data, &good), EngineState::Installed);
}

/// A server that sends half of `body`, then holds the connection until `release` is sent.
fn serve_held(body: Arc<Vec<u8>>) -> (String, std::sync::mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/f", listener.local_addr().unwrap());
    let (release, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
        }
        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(&body[..body.len() / 2]);
        let _ = stream.flush();
        let _ = wait.recv();
    });
    (url, release)
}

#[test]
fn cancel_is_immediate_keeps_the_part_and_records_no_failure() {
    let dir = temp_dir("ei32");
    let a = archive(&dir, &GOOD);
    let b = Arc::new(std::fs::read(&a).unwrap());
    let (url, release) = serve_held(b.clone());
    let pin = pin_for(&a, url);
    let data = dir.join("data");
    let mut app = App::default();
    assert!(app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&pin)));
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !matches!(cef_state(&app, &data, &pin), EngineState::Installing { received, .. } if received > 0) {
        assert!(std::time::Instant::now() < deadline, "no progress");
        std::thread::sleep(Duration::from_millis(10));
    }
    app.cancel_engine_install(EngineKind::Cef);
    assert!(matches!(cef_state(&app, &data, &pin), EngineState::NotInstalled { .. }), "at once");
    // The helper thread is still blocked on its read, but the job sees the cancel within ~100 ms.
    release.send(()).unwrap();
    settle(&mut app);
    assert!(matches!(cef_state(&app, &data, &pin), EngineState::NotInstalled { .. }));
    assert!(app.engine_ui.engine_failures.is_empty() && toasts(&app).is_empty(), "a cancel is not a failure");
    assert!(std::fs::metadata(data.join("cef-runtime.part")).unwrap().len() > 0, "the part is kept");
    // Once the thread is gone a new install starts (and resumes the part).
    assert!(app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&Pin { url: pin.url, ..pin.clone() })));
    app.cancel_engine_install(EngineKind::Cef);
}

#[test]
fn one_install_per_engine_and_only_cef_downloads() {
    let dir = temp_dir("ei33");
    let a = archive(&dir, &GOOD);
    let (url, release) = serve_held(Arc::new(std::fs::read(&a).unwrap()));
    let pin = pin_for(&a, url);
    let data = dir.join("data");
    let mut app = App::default();
    assert!(app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&pin)));
    assert!(!app.start_engine_install_at(EngineKind::Cef, &data, &runtime_plan(&pin)), "already installing");
    assert_eq!(app.engine_ui.engine_installs.len(), 1);
    app.cancel_engine_install(EngineKind::Cef);
    release.send(()).unwrap();
    settle(&mut app);
    assert!(
        !app.start_engine_install_at(EngineKind::Webkit, &data, &Plan { host: &[], runtime: None }),
        "only CEF downloads anything"
    );
}

#[test]
fn the_pin_matches_the_cef_crates_version() {
    let lock = include_str!("../../../../Cargo.lock");
    let after = lock.split("name = \"cef\"").nth(1).expect("Cargo.lock has a cef package");
    let line = after.lines().nth(1).expect("a version line follows the name");
    let version = line.strip_prefix("version = \"").and_then(|v| v.strip_suffix('"')).expect("a version line");
    // `152.1.0+152.0.6`: the crate's own version, then the CEF build it wraps.
    let build = version.split_once('+').expect("the cef crate version names its CEF build").1;
    let pinned = cef_pin().version;
    assert_eq!(pinned.split('+').next(), Some(build), "the pin is not the CEF build the crate wraps: {pinned}");
}

/// Verifies and unpacks a real downloaded archive for this platform's pin, timing both:
/// `MANIFOLD_CEF_ARCHIVE=<archive> MANIFOLD_CEF_UNPACK_TO=<new dir> cargo test -p manifold
/// unpack_a_real_archive -- --ignored --nocapture`. Ignored: it needs the 172 MB (Windows) or 321 MB
/// (Linux) download, which no test makes.
#[test]
#[ignore]
fn unpack_a_real_archive() {
    let archive = PathBuf::from(std::env::var_os("MANIFOLD_CEF_ARCHIVE").expect("MANIFOLD_CEF_ARCHIVE"));
    let into = PathBuf::from(std::env::var_os("MANIFOLD_CEF_UNPACK_TO").expect("MANIFOLD_CEF_UNPACK_TO"));
    let pin = cef_pin();
    let start = std::time::Instant::now();
    verify(&archive, pin).unwrap();
    println!("verify: {:.1} s", start.elapsed().as_secs_f64());
    let start = std::time::Instant::now();
    unpack(&archive, &into, pin, &AtomicBool::new(false)).unwrap();
    println!("unpack: {:.1} s", start.elapsed().as_secs_f64());
    assert_eq!(runtime_state(&into, pin), RuntimeOnDisk::Ready);
}

// ---- host components ----

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn sha_of(bytes: &[u8]) -> &'static str {
    leak(to_hex(&Sha256::digest(bytes)))
}

/// A pinned file `name` of `bytes`, served at `url`.
fn component(name: &'static str, bytes: &[u8], url: String) -> HostFile {
    HostFile { file: name, url: leak(url), size: bytes.len() as u64, sha256: sha_of(bytes) }
}

fn files(list: Vec<HostFile>) -> &'static [HostFile] {
    Box::leak(list.into_boxed_slice())
}

fn serve_bytes(bytes: &Arc<Vec<u8>>) -> String {
    serve(bytes.clone(), Mode::Honour, None)
}

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

/// A server that answers every request with a 302 to `target`.
fn serve_redirect(target: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/r", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });
    url
}

/// An address nothing answers on, for "the network must not be touched".
fn dead_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}/gone", listener.local_addr().unwrap())
}

#[test]
fn a_one_file_host_resumes_a_part_verifies_and_writes_the_marker_last() {
    let dir = temp_dir("host-a");
    let bytes = body(200_000);
    let f = files(vec![component("host.exe", &bytes, serve_bytes(&bytes))]);
    // The partial is named by the pin's hash prefix, and the install resumes it.
    let part = dir.join(format!("host.exe.{}.part", &f[0].sha256[..12]));
    std::fs::write(&part, &bytes[..70_000]).unwrap();
    let mut last = 0;
    install_host(&dir, EngineKind::Servo, f, &mut |n| last = n, &never()).unwrap();
    assert_eq!(std::fs::read(dir.join("host.exe")).unwrap(), *bytes);
    assert_eq!(last, f[0].size);
    assert!(!part.exists());
    assert_eq!(std::fs::read_to_string(dir.join("servo.host")).unwrap(), format!("{} host.exe\n", f[0].sha256));
    assert!(!dir.join("servo.host.tmp").exists());
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Ready);
}

#[test]
fn a_wrong_hash_deletes_the_part_and_leaves_no_file_and_no_marker() {
    let dir = temp_dir("host-b");
    let bytes = body(50_000);
    let mut c = component("host.exe", &bytes, serve_bytes(&bytes));
    c.sha256 = sha_of(b"something else");
    let f = files(vec![c]);
    let result = install_host(&dir, EngineKind::Servo, f, &mut |_| {}, &never());
    assert!(matches!(result, Err(InstallError::BadHash { .. })));
    assert!(!dir.join("host.exe").exists() && !dir.join("servo.host").exists());
    assert!(host_leftovers(&dir, EngineKind::Servo, f).is_empty(), "no .part left");
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Missing);
}

#[test]
fn a_two_file_host_with_a_bad_second_file_gets_no_marker_and_a_retry_skips_the_first() {
    let dir = temp_dir("host-c");
    let (one, two) = (body(30_000), Arc::new(vec![9u8; 20_000]));
    let first = component("a.dll", &one, serve_bytes(&one));
    let mut second = component("b.exe", &two, serve_bytes(&two));
    second.sha256 = sha_of(b"other");
    let bad = files(vec![HostFile { ..first }, second]);
    assert!(install_host(&dir, EngineKind::Servo, bad, &mut |_| {}, &never()).is_err());
    assert!(!dir.join("servo.host").exists(), "no Ready marker");
    assert_eq!(host_state(&dir, EngineKind::Servo, bad), RuntimeOnDisk::Missing);
    assert!(dir.join("a.dll").exists() && !dir.join("b.exe").exists());
    // The retry (a corrected pin) does not fetch the finished first file again.
    let good = files(vec![HostFile { url: leak(dead_url()), ..first }, component("b.exe", &two, serve_bytes(&two))]);
    install_host(&dir, EngineKind::Servo, good, &mut |_| {}, &never()).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, good), RuntimeOnDisk::Ready);
}

#[test]
fn a_host_marker_of_another_release_is_stale_and_no_marker_is_missing() {
    let dir = temp_dir("host-d");
    let bytes = body(1000);
    let f = files(vec![component("host.exe", &bytes, dead_url())]);
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Missing);
    std::fs::write(dir.join("host.exe"), &*bytes).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Missing, "a file without its marker");
    std::fs::write(dir.join("servo.host"), format!("{} host.exe\n", sha_of(b"old release"))).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Stale);
    std::fs::write(dir.join("servo.host"), marker_text(f)).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Ready);
    std::fs::write(dir.join("host.exe"), b"short").unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Missing, "a truncated file");
}

#[test]
fn not_installed_is_the_sum_of_the_missing_components() {
    let app = App::default();
    let f = files(vec![
        component("a.dll", &[0; 1000], dead_url()),
        component("b.dll", &[1; 2000], dead_url()),
        component("c.exe", &[2; 4000], dead_url()),
    ]);
    let plan = Ok(Plan { host: f, runtime: None });
    let state = |host| app.engine_state_at(EngineKind::Servo, OnDisk::both(host), &plan);
    assert_eq!(state(RuntimeOnDisk::Missing), EngineState::NotInstalled { size: 7000 });
    assert_eq!(state(RuntimeOnDisk::Ready), EngineState::Installed);
}

#[test]
fn a_host_poll_hashes_nothing_but_the_spawn_check_does() {
    let dir = temp_dir("host-g");
    let bytes = body(100_000);
    let f = files(vec![component("host.exe", &bytes, serve_bytes(&bytes))]);
    install_host(&dir, EngineKind::Servo, f, &mut |_| {}, &never()).unwrap();
    let before = HASHED.with(|n| n.get());
    for _ in 0..3 {
        assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Ready);
    }
    assert_eq!(HASHED.with(|n| n.get()), before, "the on-disk poll must not hash");
    verify_installed_host(&dir, f).unwrap();
    assert_eq!(HASHED.with(|n| n.get()), before + 1);
}

#[test]
fn an_install_skips_a_component_that_is_already_there_with_its_hash() {
    let dir = temp_dir("host-h");
    let bytes = body(10_000);
    // The URL is dead: only an untouched network lets this succeed.
    let f = files(vec![component("host.exe", &bytes, dead_url())]);
    std::fs::write(dir.join("host.exe"), &*bytes).unwrap();
    install_host(&dir, EngineKind::Servo, f, &mut |_| {}, &never()).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Ready);
}

#[test]
fn a_host_file_changed_after_install_fails_the_spawn_check() {
    let dir = temp_dir("host-i");
    let bytes = body(10_000);
    let f = files(vec![component("host.exe", &bytes, serve_bytes(&bytes))]);
    install_host(&dir, EngineKind::Servo, f, &mut |_| {}, &never()).unwrap();
    verify_installed_host(&dir, f).unwrap();
    let mut changed = (*bytes).clone();
    changed[5000] ^= 1;
    std::fs::write(dir.join("host.exe"), changed).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Ready, "same size: the poll cannot tell");
    assert!(matches!(verify_installed_host(&dir, f), Err(InstallError::BadHash { .. })));
}

#[test]
fn a_download_follows_a_redirect() {
    let dir = temp_dir("host-l");
    let bytes = body(40_000);
    let f = files(vec![component("host.exe", &bytes, serve_redirect(serve_bytes(&bytes)))]);
    install_host(&dir, EngineKind::Servo, f, &mut |_| {}, &never()).unwrap();
    assert_eq!(std::fs::read(dir.join("host.exe")).unwrap(), *bytes);
}

/// CEF is one job: the host, then the runtime archive, with progress across both; it is Installed only when both
/// are in.
#[test]
fn a_cef_install_fetches_the_host_then_the_runtime_in_one_progress() {
    let dir = temp_dir("host-f");
    let a = archive(&dir, &GOOD);
    let pin = pin_for(&a, serve_file(&a));
    let bytes = body(20_000);
    let f = files(vec![component("cef-host.exe", &bytes, serve_bytes(&bytes))]);
    let plan = Plan { host: f, runtime: Some(pin.clone()) };
    let data = dir.join("data");
    let (mut last, mut monotonic) = (0, true);
    let mut progress = |n| {
        monotonic &= n >= last;
        last = n;
    };
    install_plan(EngineKind::Cef, &plan, &data, &mut progress, &never()).unwrap();
    assert!(monotonic);
    assert_eq!(last, plan.size());
    let app = App::default();
    let runtime = runtime_state(&data.join("cef-runtime"), &pin);
    let both = OnDisk { host: host_state(&data, EngineKind::Cef, f), runtime };
    assert_eq!(app.engine_state_at(EngineKind::Cef, both, &Ok(plan.clone())), EngineState::Installed);
    std::fs::remove_file(data.join("cef.host")).unwrap();
    let no_host = OnDisk { host: host_state(&data, EngineKind::Cef, f), runtime };
    assert_eq!(
        app.engine_state_at(EngineKind::Cef, no_host, &Ok(plan.clone())),
        EngineState::NotInstalled { size: f[0].size }
    );
}

/// Servo's three files land flat beside each other (the ANGLE DLLs must sit next to the exe); the exe alone, without
/// a DLL or the marker, is Missing, and Ready only when the marker (written last) names all three.
#[test]
fn servo_installs_its_exe_and_both_dlls_flat_and_the_exe_alone_is_missing() {
    let dir = temp_dir("servo-flat");
    let (egl, gles, exe) = (body(1000), body(2000), body(4000));
    let f = files(vec![
        component("libEGL.dll", &egl, serve_bytes(&egl)),
        component("libGLESv2.dll", &gles, serve_bytes(&gles)),
        component("manifold-engine-servo.exe", &exe, serve_bytes(&exe)),
    ]);
    std::fs::write(dir.join("manifold-engine-servo.exe"), &*exe).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Missing, "exe without DLLs or marker");
    std::fs::write(dir.join("servo.host"), marker_text(f)).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Missing, "marker but no DLL");
    std::fs::remove_file(dir.join("servo.host")).unwrap();
    install_host(&dir, EngineKind::Servo, f, &mut |_| {}, &never()).unwrap();
    assert_eq!(host_state(&dir, EngineKind::Servo, f), RuntimeOnDisk::Ready);
    for (name, bytes) in [("libEGL.dll", &egl), ("libGLESv2.dll", &gles), ("manifold-engine-servo.exe", &exe)] {
        assert_eq!(std::fs::read(dir.join(name)).unwrap(), **bytes, "{name} sits directly in the folder");
    }
    verify_installed_host(&dir, f).unwrap();
}
