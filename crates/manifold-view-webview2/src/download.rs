//! Download handling via WebView2's native `DownloadStarting`/`BytesReceivedChanged`/`StateChanged` events
//! (`ICoreWebView2_4`). This project never prompts for a save location. `SetHandled(true)` suppresses WebView2's own
//! download UI (a flyout a windowed Edge would show): this off-screen host has nowhere for it to appear, the same
//! caution `popup.rs`/`find.rs` take.
//!
//! The host names every download by the rule every host uses (`downloads::destination`: the leaf of Edge's proposed
//! `ResultFilePath`, made safe and numbered against existing files and this host's held downloads, in the folder the
//! app set) and sets it with `SetResultFilePath` before reporting it (run 2026-10-28-dldir-result: it works from this
//! handler, and Edge then writes nothing into its own folder). So `path` is final at `DownloadStarted` and is the key
//! of every later message about the download.
//!
//! Edge numbers its proposal against the profile's `DefaultDownloadFolderPath` (run 2026-10-28-dldir-profile-seed),
//! so `register` points that at an empty naming folder the host owns, where nothing is saved (`naming_folder`). The
//! profile is InPrivate and keeps no setting to the next run, so it is set at every start (2026-10-28-dldir-profile),
//! and only once the folder is made: Edge fires no `DownloadStarting` at all when it can't make that folder
//! (2026-10-28-dldir-profile-bad).
//!
//! A download the host can't place is cancelled and reported started and failed with its reason
//! (`downloads::refused`), so the app shows a failed card that says why: no folder from the app, or one that can't be
//! made (`downloads::destination` classifies both), or a path Edge refuses (`SetResultFilePath` fails:
//! `FolderUnusable`; its HRESULT's message can name the path, so it goes to this host's log, not the card). A
//! download Edge ends `INTERRUPTED` says why by its `InterruptReason` (`failure_kind`).
//!
//! A transfer that breaks is retried by the engine as a second `DownloadStarting`, whose operation starts with the
//! first one's `InterruptReason` rather than none (runs 2026-10-28-dldir-result2, -result-etag2). The retry takes the
//! broken download's path (`retry_path`): restarted from 0 it overwrites that file; resumed from the bytes the engine
//! kept (an HTTP/1.1 reply with an ETag and `Accept-Ranges`) it continues it (2026-10-28-dldir-result-etag), where
//! Edge's own naming proposed `slow (1).zip` (2026-10-28-dldir-profile-etag). One path keeps one card: the app restarts
//! the card of a download in flight under the same path (`fetched.rs` `insert_download_node`).
//!
//! The host holds each download's operation until it reports finished or failed. The retry reports its outcome only
//! while held: dropped, it never did (runs 2026-10-27-download-host-events, 2026-10-28-dl-failed-red); held, it does
//! (runs 2026-10-27-dl-probe-*). The first operation never reports again, so it is let go with its retry (`release`):
//! its name stays taken while the retry runs, and is free once a failed retry's file is gone.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use manifold_view_protocol::{downloads, DownloadFailure, FailureKind, ViewToHost};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2DownloadOperation, ICoreWebView2_13, ICoreWebView2_4,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON, COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
    COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
};
use webview2_com::{
    take_pwstr, BytesReceivedChangedEventHandler, DownloadStartingEventHandler, StateChangedEventHandler,
};
use windows::core::{Interface, HSTRING, PWSTR};

/// A queue, not a single slot -- two downloads progressing within the same
/// tick would otherwise silently clobber each other (each gets its own
/// `BytesReceivedChangedEventHandler`/`StateChangedEventHandler` closures
/// below, all writing into whatever this type is).
pub type PendingDownload = Rc<RefCell<Vec<ViewToHost>>>;

/// A download the host reported and has not seen finish or fail (see the module doc).
struct Held {
    operation: ICoreWebView2DownloadOperation,
    /// Where it saves: its key in every message.
    path: PathBuf,
    /// What it fetches, to match a retry to it.
    uri: String,
}

/// What `save_path` reads of a held download.
struct Seen<'a> {
    path: &'a Path,
    uri: &'a str,
    /// Its transfer broke (`InterruptReason` not none) while it still reads in progress: the engine is retrying it.
    broken: bool,
}

/// The empty folder Edge proposes names in (see the module doc). Nothing is saved there.
fn naming_folder() -> PathBuf {
    std::env::temp_dir().join("manifold-webview2-names")
}

/// Points the profile's `DefaultDownloadFolderPath` at `naming_folder`, made first.
fn set_naming_folder(webview: &ICoreWebView2) -> Result<(), String> {
    let folder = naming_folder();
    std::fs::create_dir_all(&folder).map_err(|e| format!("{} not made: {e}", folder.display()))?;
    let set = || -> windows::core::Result<()> {
        let webview13: ICoreWebView2_13 = webview.cast()?;
        unsafe { webview13.Profile()?.SetDefaultDownloadFolderPath(&HSTRING::from(folder.as_path())) }
    };
    set().map_err(|e| e.to_string())
}

/// Why `operation`'s transfer broke, `NONE` while it hasn't (or when Edge can't say).
fn interrupt_reason(operation: &ICoreWebView2DownloadOperation) -> COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON {
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
    match unsafe { operation.InterruptReason(&mut reason) } {
        Ok(()) => reason,
        Err(_) => COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
    }
}

/// Whether `operation`'s transfer broke: a retry starts with the broken one's reason (see the module doc).
fn interrupted(operation: &ICoreWebView2DownloadOperation) -> bool {
    interrupt_reason(operation) != COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE
}

/// What the user can do about a download Edge ended `INTERRUPTED` for `reason` (webview2-com-sys 0.39.1 names 0-29;
/// any other is `Other`). Most `FILE_*` codes are an unusable folder, except `FILE_NO_SPACE` (a full disk) and the
/// malware and policy codes (blocked); `FILE_TRANSIENT_ERROR`, `FILE_TOO_SHORT`, `FILE_HASH_MISMATCH` (a resumed file that doesn't match), the `USER_*`
/// codes and a crash say nothing the user can act on. A server that dies mid-transfer ends the retry
/// `NETWORK_FAILED` (run 2026-10-27-dl-probe-hold).
/// A download into `C:\`, where a normal user can't create files, ends `FILE_ACCESS_DENIED` (2, run
/// 2026-10-28-dl-fail-croot).
fn failure_kind(reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON) -> FailureKind {
    match reason.0 {
        // FILE_FAILED, FILE_ACCESS_DENIED, FILE_NAME_TOO_LONG, FILE_TOO_LARGE
        1 | 2 | 4 | 5 => FailureKind::FolderUnusable,
        // FILE_NO_SPACE
        3 => FailureKind::DiskFull,
        // FILE_MALICIOUS, FILE_BLOCKED_BY_POLICY, FILE_SECURITY_CHECK_FAILED
        6 | 8 | 9 => FailureKind::Blocked,
        // NETWORK_FAILED, _TIMEOUT, _DISCONNECTED, _SERVER_DOWN. NETWORK_INVALID_REQUEST (16) is Other: Chromium
        // uses it for an unsupported scheme, a bad URL or one blocked by policy, not a connection that failed.
        12..=15 => FailureKind::Network,
        // SERVER_FAILED through SERVER_CROSS_ORIGIN_REDIRECT
        17..=25 => FailureKind::Server,
        _ => FailureKind::Other,
    }
}

/// The path of the download a retry of `uri` retries: the latest held one of that URL whose transfer broke.
fn retry_path<'a>(held: &[Seen<'a>], uri: &str) -> Option<&'a Path> {
    held.iter().rev().find(|h| h.broken && h.uri == uri).map(|h| h.path)
}

/// Where a download of `uri` that Edge proposed as `proposed` is saved: a retry (`broken_at_start`) of a broken held
/// download takes that download's path; anything else is named by `downloads::destination` in `folder`, numbered
/// against existing files and every held download. `Err` is a download to refuse, and why.
fn save_path(
    held: &[Seen<'_>],
    uri: &str,
    broken_at_start: bool,
    proposed: &str,
    folder: Option<&Path>,
) -> Result<PathBuf, downloads::Refusal> {
    match retry_path(held, uri) {
        Some(path) if broken_at_start => Ok(path.to_path_buf()),
        _ => downloads::destination(folder, proposed, |p| {
            p.exists() || held.iter().any(|h| downloads::same_download_path(h.path, p))
        }),
    }
}

/// Lets go of what a download that reported finished or failed at `path` leaves held: itself, and the broken
/// download its retry took the path of, which never reports again (see the module doc). Two held downloads share a
/// path only as a retry and what it retried, since `save_path` numbers every other one.
fn release<T>(held: &mut Vec<T>, path: &Path, path_of: impl Fn(&T) -> &Path) {
    held.retain(|h| !downloads::same_download_path(path_of(h), path));
}

/// Best-effort like this project's other version-gated WebView2 APIs -- an
/// `ICoreWebView2_4`-less Runtime just means downloads on this tab proceed
/// with WebView2's own default UI/behavior instead of being silently
/// tracked, rather than failing to download at all.
pub fn register(webview: &ICoreWebView2) -> windows::core::Result<PendingDownload> {
    let webview4: ICoreWebView2_4 = webview.cast()?;
    if let Err(e) = set_naming_folder(webview) {
        eprintln!("download: naming folder not set ({e}); Edge numbers its proposals against its own folder");
    }

    let pending: PendingDownload = Rc::new(RefCell::new(Vec::new()));
    let mut token = Default::default();

    let pending_for_handler = pending.clone();
    // Every download that hasn't finished or failed yet, and each broken one whose retry hasn't (see the module doc).
    let held_for_handler: Rc<RefCell<Vec<Held>>> = Rc::default();
    let handler = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let _ = args.SetHandled(true);
        }
        let Ok(operation) = (unsafe { args.DownloadOperation() }) else { return Ok(()) };
        let mut proposed_raw = PWSTR::null();
        let proposed = if unsafe { args.ResultFilePath(&mut proposed_raw) }.is_ok() {
            take_pwstr(proposed_raw)
        } else {
            String::new()
        };
        let mut uri_raw = PWSTR::null();
        let uri = if unsafe { operation.Uri(&mut uri_raw) }.is_ok() { take_pwstr(uri_raw) } else { String::new() };
        let broken_at_start = interrupted(&operation);
        let decided = {
            let held = held_for_handler.borrow();
            let seen: Vec<Seen> = held
                .iter()
                .map(|h| Seen { path: h.path.as_path(), uri: h.uri.as_str(), broken: interrupted(&h.operation) })
                .collect();
            save_path(&seen, &uri, broken_at_start, &proposed, downloads::folder().as_deref())
        };
        let url = Some(uri.clone()).filter(|u| !u.is_empty());
        // A path Edge won't take (`SetResultFilePath` fails) is an unusable folder. Its message could name the path,
        // which a detail never holds, so it goes to this host's log, not the card (unverified: no run has seen it).
        let placed = decided.and_then(|path| match unsafe { args.SetResultFilePath(&HSTRING::from(path.as_path())) } {
            Ok(()) => Ok(path),
            Err(e) => {
                eprintln!("download: SetResultFilePath refused: {}", e.message());
                Err(downloads::Refusal { path, failure: DownloadFailure::new(FailureKind::FolderUnusable, "") })
            }
        });
        let path = match placed {
            Ok(path) => path,
            Err(refusal) => {
                unsafe {
                    let _ = args.SetCancel(true);
                }
                pending_for_handler.borrow_mut().extend(downloads::refused(refusal, url));
                return Ok(());
            }
        };
        let path_text = path.to_string_lossy().into_owned();
        let mut total_bytes_raw: i64 = -1;
        let total_bytes = unsafe { operation.TotalBytesToReceive(&mut total_bytes_raw) }
            .is_ok()
            .then_some(total_bytes_raw)
            .filter(|n| *n >= 0)
            .map(|n| n as u64);
        pending_for_handler.borrow_mut().push(ViewToHost::DownloadStarted {
            path: path_text.clone(),
            total_bytes,
            url,
        });
        // A retry that resumed starts with the bytes the engine kept.
        let mut bytes_received: i64 = 0;
        if unsafe { operation.BytesReceived(&mut bytes_received) }.is_ok() && bytes_received > 0 {
            pending_for_handler
                .borrow_mut()
                .push(ViewToHost::DownloadProgress { path: path_text.clone(), bytes_received: bytes_received as u64 });
        }
        held_for_handler.borrow_mut().push(Held { operation: operation.clone(), path, uri });

        let pending_for_bytes = pending_for_handler.clone();
        let path_for_bytes = path_text.clone();
        let mut bytes_token = Default::default();
        let bytes_handler = BytesReceivedChangedEventHandler::create(Box::new(move |sender, _args| {
            if let Some(sender) = &sender {
                let mut bytes_received: i64 = 0;
                if unsafe { sender.BytesReceived(&mut bytes_received) }.is_ok() && bytes_received >= 0 {
                    pending_for_bytes.borrow_mut().push(ViewToHost::DownloadProgress {
                        path: path_for_bytes.clone(),
                        bytes_received: bytes_received as u64,
                    });
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = operation.add_BytesReceivedChanged(&bytes_handler, &mut bytes_token);
        }

        let pending_for_state = pending_for_handler.clone();
        let held_for_state = held_for_handler.clone();
        let mut state_token = Default::default();
        let state_handler = StateChangedEventHandler::create(Box::new(move |sender, _args| {
            if let Some(sender) = &sender {
                let mut state = Default::default();
                if unsafe { sender.State(&mut state) }.is_ok() {
                    let message = if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                        Some(ViewToHost::DownloadCompleted { path: path_text.clone() })
                    } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                        // Edge's code says why; it has no words of its own to add.
                        let failure = DownloadFailure::new(failure_kind(interrupt_reason(sender)), "");
                        Some(ViewToHost::DownloadFailed { path: path_text.clone(), failure })
                    } else {
                        None
                    };
                    if let Some(message) = message {
                        pending_for_state.borrow_mut().push(message);
                        // Over: nothing more will come from it, nor from a broken download it retried.
                        release(&mut *held_for_state.borrow_mut(), Path::new(&path_text), |h| h.path.as_path());
                    }
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = operation.add_StateChanged(&state_handler, &mut state_token);
        }
        Ok(())
    }));
    unsafe {
        let _ = webview4.add_DownloadStarting(&handler, &mut token);
    }

    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen<'a>(path: &'a Path, uri: &'a str, broken: bool) -> Seen<'a> {
        Seen { path, uri, broken }
    }

    /// A fresh, empty folder of this test's own.
    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manifold-wv2-{name}-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_retry_takes_the_path_of_the_broken_download_it_retries() {
        let dir = temp_folder("retry");
        let slow = dir.join("slow.zip");
        let broken = [seen(&slow, "http://h/slow", true)];
        let proposed = r"C:\names\slow (1).zip";
        let folder = Some(dir.as_path());
        assert_eq!(save_path(&broken, "http://h/slow", true, proposed, folder), Ok(slow.clone()));
        assert_eq!(save_path(&broken, "http://h/slow", true, proposed, None), Ok(slow.clone()), "needs no folder");
        assert_eq!(
            save_path(&broken, "http://h/other", true, r"C:\names\other.zip", folder),
            Ok(dir.join("other.zip")),
            "another URL is not its retry"
        );
        let healthy = [seen(&slow, "http://h/slow", false)];
        assert_eq!(
            save_path(&healthy, "http://h/slow", true, proposed, folder),
            Ok(dir.join("slow (1).zip")),
            "only a broken download is retried"
        );
        assert_eq!(
            save_path(&broken, "http://h/slow", false, r"C:\names\slow.zip", folder),
            Ok(dir.join("slow (1).zip")),
            "a fresh download of the same URL is a new one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_latest_broken_download_of_the_url_is_the_one_retried() {
        let (a, b, c) = (Path::new(r"C:\d\a.zip"), Path::new(r"C:\d\b.zip"), Path::new(r"C:\d\c.zip"));
        let held = [seen(a, "u", true), seen(b, "u", true), seen(c, "u", false)];
        assert_eq!(retry_path(&held, "u"), Some(b));
        assert_eq!(retry_path(&held, "v"), None);
        assert_eq!(retry_path(&held[2..], "u"), None, "a healthy one is not retried");
        assert_eq!(retry_path(&[], "u"), None);
    }

    /// Review Focus 4: a second download of one name while the first is arriving is numbered; a retry is not.
    #[test]
    fn a_second_download_of_one_name_is_numbered_and_a_retry_is_not() {
        let dir = temp_folder("second");
        let folder = Some(dir.as_path());
        let a = dir.join("a.zip");
        let healthy = [seen(&a, "http://h/a", false)];
        assert_eq!(
            save_path(&healthy, "http://h/a", false, r"C:\names\a.zip", folder),
            Ok(dir.join("a (1).zip")),
            "the first is still arriving"
        );
        let upper = dir.join("A.ZIP");
        let other_case = [seen(&upper, "http://h/A", false)];
        assert_eq!(
            save_path(&other_case, "http://h/a", false, r"C:\names\a.zip", folder),
            Ok(dir.join("a (1).zip")),
            "Windows names ignore case"
        );
        std::fs::write(dir.join("b.zip"), b"old").unwrap();
        assert_eq!(
            save_path(&[], "http://h/b", false, r"C:\names\b.zip", folder),
            Ok(dir.join("b (1).zip")),
            "an existing file keeps its name"
        );
        assert_eq!(
            save_path(&[], "http://h/c", false, r"C:\names\..\c:x.zip", folder),
            Ok(dir.join("c_x.zip")),
            "only the leaf, made safe"
        );
        assert_eq!(
            save_path(&[], "http://h/a", false, r"C:\names\a.zip", None).map_err(|r| r.path),
            Err(PathBuf::from("a.zip")),
            "no folder: refused"
        );
        let broken = [seen(&a, "http://h/a", true)];
        assert_eq!(
            save_path(&broken, "http://h/a", true, r"C:\names\a (1).zip", folder),
            Ok(a.clone()),
            "the retry keeps the name"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review Focus 4: a retry that ends takes the broken download it retried with it. That one never reports
    /// again; held, it would keep the name taken after the retry failed and its file went.
    #[test]
    fn a_retry_that_ends_frees_the_name_of_the_download_it_retried() {
        let dir = temp_folder("release");
        let folder = Some(dir.as_path());
        let (slow, other) = (dir.join("slow.zip"), dir.join("other.zip"));
        let mut held = vec![
            seen(&slow, "http://h/slow", true),
            seen(&other, "http://h/other", false),
            seen(&slow, "http://h/slow", false),
        ];
        release(&mut held, &slow, |h| h.path);
        assert_eq!(held.len(), 1, "the retry and the download it retried are both over");
        assert_eq!(held[0].path, other.as_path());
        assert_eq!(
            save_path(&held, "http://h/slow", false, r"C:\names\slow.zip", folder),
            Ok(slow.clone()),
            "the failed retry's file is gone, so the name is free again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every reason webview2-com-sys 0.39.1 names (0-29) has its kind; one it doesn't name is `Other`.
    #[test]
    fn every_interrupt_reason_has_its_kind() {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        use FailureKind as K;
        let table = [
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_FAILED, K::FolderUnusable),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_ACCESS_DENIED, K::FolderUnusable),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NO_SPACE, K::DiskFull),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NAME_TOO_LONG, K::FolderUnusable),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_LARGE, K::FolderUnusable),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS, K::Blocked),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TRANSIENT_ERROR, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY, K::Blocked),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED, K::Blocked),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_SHORT, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_HASH_MISMATCH, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_FAILED, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_SERVER_DOWN, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_INVALID_REQUEST, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FAILED, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_BAD_CONTENT, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CERTIFICATE_PROBLEM, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNEXPECTED_RESPONSE, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CROSS_ORIGIN_REDIRECT, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_SHUTDOWN, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_DOWNLOAD_PROCESS_CRASHED, K::Other),
        ];
        for (reason, kind) in table {
            assert_eq!(failure_kind(reason), kind, "{reason:?}");
        }
        assert_eq!(
            failure_kind(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON(30)),
            K::Other,
            "a reason this build doesn't name"
        );
    }
}
