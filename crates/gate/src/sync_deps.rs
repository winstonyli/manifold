//! `gate sync-runtime-deps` (was scripts/sync-runtime-deps.sh): puts each engine host's runtime files beside the host
//! a dev tree runs (README "Runtime-file placement"). Cargo does not, and the fingerprint folder they are built into
//! can change on a rebuild that touched neither Cargo.toml nor Cargo.lock, silently staling an earlier copy.
//! - Servo's ANGLE DLLs go beside `target-servo/debug/manifold-view-servo.exe`, where manifold runs it, from the
//!   newest mozangle output in `target-servo/debug/build/` or `target/debug/build/`. With no `target-servo/debug/`
//!   they are skipped quietly; with the host there and no DLL to give it, that is an error.
//! - CEF's runtime and its host exe go into `target/debug/cef-runtime/`, the copy manifold runs.
//!
//! Safe to run any time (a no-op when everything is current); run it after every build of manifold-view-servo or
//! manifold-view-cef. Every step runs even when one fails: the failures are listed at the end and the exit code is 1,
//! and a file a running host holds open is named with "close manifold and run again". Not a build.rs: a build script
//! sees only its own crate's OUT_DIR.
//!
//! The source is the most recently *written* match, not the newest fingerprint directory, whose mtime cargo touches
//! on every build. Newer cargo nightlies put build-script output under `build/<crate>/<hash>/`, older ones under
//! `build/<crate>-<hash>/`, and a target dir built by both has each.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

/// Servo's whole runtime besides its exe (release.yml ships the same three files).
const SERVO_DLLS: [&str; 2] = ["libEGL.dll", "libGLESv2.dll"];
const SERVO_EXE: &str = "manifold-view-servo.exe";

fn say(msg: &str) {
    println!("sync-runtime-deps: {msg}");
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// `<build>/<krate>-*/out/<tail>` and `<build>/<krate>/*/out/<tail>` that exist, for each of `builds`, the newest by
/// mtime. A build folder that isn't there is skipped.
fn find_newest(builds: &[PathBuf], krate: &str, tail: &str) -> Option<PathBuf> {
    let mut found = Vec::new();
    for build in builds {
        for e in fs::read_dir(build).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("{krate}-")) {
                found.push(e.path().join("out").join(tail));
            } else if name == krate {
                for h in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                    found.push(h.path().join("out").join(tail));
                }
            }
        }
    }
    found.into_iter().filter_map(|p| Some((mtime(&p)?, p))).max().map(|(_, p)| p)
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dst)?;
        } else {
            fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

/// Copies `src` over `dst` unless their bytes already agree; true if it copied.
fn sync_file(src: Option<&Path>, dst: &Path, label: &str) -> io::Result<bool> {
    let Some(src) = src else {
        eprintln!("sync-runtime-deps: no candidate found for {label}, skipping");
        return Ok(false);
    };
    if dst.is_file() && fs::read(src)? == fs::read(dst)? {
        say(&format!("{label} already current"));
        return Ok(false);
    }
    fs::copy(src, dst)?;
    say(&format!("recopied {label}"));
    Ok(true)
}

/// What the steps did: whether any copied, and a line for each that failed.
#[derive(Default)]
struct Outcome {
    changed: bool,
    errors: Vec<String>,
}

impl Outcome {
    /// Records one step's result; `dst` names the file it was writing.
    fn step(&mut self, dst: &str, result: io::Result<bool>) {
        match result {
            Ok(changed) => self.changed |= changed,
            Err(e) => self.errors.push(step_error(dst, &e)),
        }
    }
}

/// The line for a step that failed. A file Windows holds open (a DLL loaded by a running host: os error 32, 33 or
/// 1224) says to close manifold, the usual holder.
fn step_error(dst: &str, e: &io::Error) -> String {
    if matches!(e.raw_os_error(), Some(32 | 33 | 1224)) {
        format!("{dst} is in use: close manifold and run again")
    } else {
        format!("{dst}: {e}")
    }
}

/// Servo's DLLs beside its host in `servo_debug` (`target-servo/debug/`), from the newest mozangle output in `builds`.
fn sync_servo(servo_debug: &Path, builds: &[PathBuf], outcome: &mut Outcome) {
    if !servo_debug.is_dir() {
        say("no target-servo/debug, skipping Servo's DLLs");
        return;
    }
    let exe_there = servo_debug.join(SERVO_EXE).is_file();
    for dll in SERVO_DLLS {
        let label = format!("target-servo/debug/{dll}");
        match find_newest(builds, "mozangle", dll) {
            Some(src) => outcome.step(&label, sync_file(Some(&src), &servo_debug.join(dll), &label)),
            None if exe_there => outcome.errors.push(format!(
                "no mozangle build output holds {dll}, which {SERVO_EXE} needs: build Servo first (README \"Building \
                 Servo on its own\")"
            )),
            None => say(&format!("no mozangle build output holds {dll} and no {SERVO_EXE} is built, skipping")),
        }
    }
}

/// CEF's runtime and its host exe into `target_debug/cef-runtime/`, the copy manifold runs. The host exe is rebuilt
/// whenever manifold-view-cef changes, unlike the runtime: it is always compared.
fn sync_cef(target_debug: &Path, outcome: &mut Outcome) {
    let runtime = target_debug.join("cef-runtime");
    match find_newest(&[target_debug.join("build")], "cef-dll-sys", "cef_windows_x86_64") {
        Some(out) => {
            if let Err(e) = copy_runtime(&out, &runtime, outcome) {
                outcome.errors.push(step_error("cef-runtime", &e));
            }
        }
        None => eprintln!("sync-runtime-deps: no cef-dll-sys output directory found, skipping CEF runtime"),
    }
    let exe = target_debug.join("manifold-view-cef.exe");
    let label = "cef-runtime/manifold-view-cef.exe";
    outcome
        .step(label, sync_file(exe.is_file().then_some(exe.as_path()), &runtime.join("manifold-view-cef.exe"), label));
}

/// Each file of CEF's distribution `out` into `runtime`, recopied when it differs, and each directory (locales/) only
/// when missing or empty, since those don't change between rebuilds. A file or directory that fails is listed and the
/// rest still copy; only a folder that can't be created or listed stops it.
fn copy_runtime(out: &Path, runtime: &Path, outcome: &mut Outcome) -> io::Result<()> {
    fs::create_dir_all(runtime)?;
    for e in fs::read_dir(out)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let label = format!("cef-runtime/{name}");
        let dst = runtime.join(&name);
        if e.file_type()?.is_dir() {
            if fs::read_dir(&dst).map_or(true, |mut d| d.next().is_none()) {
                match copy_dir(&e.path(), &dst) {
                    Ok(()) => {
                        say(&format!("copied {label}/"));
                        outcome.changed = true;
                    }
                    Err(err) => outcome.errors.push(step_error(&label, &err)),
                }
            }
        } else {
            outcome.step(&label, sync_file(Some(&e.path()), &dst, &label));
        }
    }
    Ok(())
}

fn run() -> Result<(), Vec<String>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target_debug = root
        .join("target/debug")
        .canonicalize()
        .map_err(|_| vec!["target/debug does not exist - build first".to_string()])?;
    let servo_debug = root.join("target-servo/debug");
    let mut outcome = Outcome::default();
    sync_servo(&servo_debug, &[servo_debug.join("build"), target_debug.join("build")], &mut outcome);
    sync_cef(&target_debug, &mut outcome);
    finish(outcome)
}

/// What the sync reports once every step ran: each failure (so the exit code is 1), else success, saying so when
/// nothing needed copying.
fn finish(outcome: Outcome) -> Result<(), Vec<String>> {
    if !outcome.errors.is_empty() {
        return Err(outcome.errors);
    }
    if !outcome.changed {
        say("everything already current, nothing to do");
    }
    Ok(())
}

pub fn sync_runtime_deps() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for e in errors {
                eprintln!("sync-runtime-deps: {e}");
            }
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path, secs: u64, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
        let f = fs::OpenOptions::new().append(true).open(p).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
    }

    /// A fresh, empty folder for one test, under the system temp folder.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gate-sync-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The newest written match wins across every build folder searched and both output layouts; a folder that
    /// isn't there is skipped.
    #[test]
    fn the_newest_in_either_build_folder_wins_and_sync_copies_only_on_change() {
        let dir = scratch("newest");
        let (main, servo) = (dir.join("build"), dir.join("servo-build"));
        touch(&main.join("mozangle-aaa/out/x.dll"), 10, b"old");
        touch(&servo.join("mozangle/bbb/out/x.dll"), 20, b"new");
        touch(&main.join("other-ccc/out/x.dll"), 30, b"other");
        let src = find_newest(&[servo, main.clone()], "mozangle", "x.dll").unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"new");
        let only_main = find_newest(std::slice::from_ref(&main), "mozangle", "x.dll").unwrap();
        assert_eq!(fs::read(only_main).unwrap(), b"old");
        let dst = dir.join("x.dll");
        assert!(sync_file(Some(&src), &dst, "x").unwrap(), "first copy");
        assert!(!sync_file(Some(&src), &dst, "x").unwrap(), "already current");
        assert!(find_newest(&[main], "absent", "x.dll").is_none());
        assert!(find_newest(&[dir.join("no-such-folder")], "mozangle", "x.dll").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Servo's DLLs go beside its host in `target-servo/debug/`. With no such folder, or the folder with neither a host
    /// nor a DLL in any build, nothing happens; a DLL that can't be copied is listed and the other still copies; a
    /// built host with no DLL to give it is an error for each.
    #[test]
    fn servo_dlls_go_beside_its_host_and_a_failed_one_does_not_stop_the_rest() {
        let dir = scratch("servo");
        let builds = [dir.join("build")];
        for dll in SERVO_DLLS {
            touch(&builds[0].join("mozangle-aaa/out").join(dll), 10, dll.as_bytes());
        }
        let servo_debug = dir.join("target-servo/debug");
        let mut outcome = Outcome::default();
        sync_servo(&servo_debug, &builds, &mut outcome);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no target-servo/debug: skipped, {:?}", outcome.errors);

        let empty = dir.join("empty/target-servo/debug");
        fs::create_dir_all(&empty).unwrap();
        let mut outcome = Outcome::default();
        sync_servo(&empty, &[dir.join("no-build")], &mut outcome);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no host and no DLL: skipped, {:?}", outcome.errors);

        fs::create_dir_all(servo_debug.join("libEGL.dll")).unwrap();
        let mut outcome = Outcome::default();
        sync_servo(&servo_debug, &builds, &mut outcome);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].starts_with("target-servo/debug/libEGL.dll: "), "{:?}", outcome.errors);
        assert!(outcome.changed, "libGLESv2.dll still copied");
        assert_eq!(fs::read(servo_debug.join("libGLESv2.dll")).unwrap(), b"libGLESv2.dll");

        let bare = dir.join("bare/target-servo/debug");
        fs::create_dir_all(&bare).unwrap();
        fs::write(bare.join(SERVO_EXE), b"host").unwrap();
        let mut outcome = Outcome::default();
        sync_servo(&bare, &[dir.join("empty-build")], &mut outcome);
        assert_eq!(outcome.errors.len(), 2, "{:?}", outcome.errors);
        assert!(outcome.errors.iter().all(|e| e.contains("build Servo first")), "{:?}", outcome.errors);
        let _ = fs::remove_dir_all(&dir);
    }

    /// CEF's runtime and its host go into `cef-runtime/`: a file that can't be copied is listed while every other
    /// file, the locales folder and the host exe still copy, and the sync then fails; once everything is current a
    /// rerun copies nothing and succeeds.
    #[test]
    fn a_cef_file_that_cant_be_copied_is_listed_and_the_rest_still_copy() {
        let td = scratch("cef");
        let out = td.join("build/cef-dll-sys-aaa/out/cef_windows_x86_64");
        for file in ["a.dll", "b.dll", "locales/en.pak"] {
            touch(&out.join(file), 10, file.as_bytes());
        }
        touch(&td.join("manifold-view-cef.exe"), 10, b"host");
        let runtime = td.join("cef-runtime");
        // A folder where a file should go: copying onto it fails on every run.
        fs::create_dir_all(runtime.join("a.dll")).unwrap();
        let mut outcome = Outcome::default();
        sync_cef(&td, &mut outcome);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].starts_with("cef-runtime/a.dll: "), "{:?}", outcome.errors);
        for file in ["b.dll", "locales/en.pak"] {
            assert_eq!(fs::read(runtime.join(file)).unwrap(), file.as_bytes(), "{file}");
        }
        assert_eq!(fs::read(runtime.join("manifold-view-cef.exe")).unwrap(), b"host");
        assert!(outcome.changed);
        let errors = outcome.errors.clone();
        assert_eq!(finish(outcome), Err(errors), "a failed step fails the sync");

        fs::remove_dir(runtime.join("a.dll")).unwrap();
        let mut outcome = Outcome::default();
        sync_cef(&td, &mut outcome);
        assert!(outcome.changed && outcome.errors.is_empty(), "a.dll now copies: {:?}", outcome.errors);
        let mut outcome = Outcome::default();
        sync_cef(&td, &mut outcome);
        assert!(!outcome.changed && outcome.errors.is_empty(), "all current: {:?}", outcome.errors);
        assert_eq!(finish(outcome), Ok(()));
        let _ = fs::remove_dir_all(&td);
    }

    /// A file Windows holds open (a DLL a running host loaded) is named, with what to do about it.
    #[test]
    fn a_held_file_says_to_close_manifold() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("held");
        let (src, dst) = (dir.join("src.dll"), dir.join("dst.dll"));
        fs::write(&src, b"new").unwrap();
        fs::write(&dst, b"old").unwrap();
        // FILE_SHARE_READ, as a loaded DLL allows: comparing reads it, and the copy over it is the step that fails.
        let held = fs::OpenOptions::new().read(true).share_mode(1).open(&dst).unwrap();
        let mut outcome = Outcome::default();
        outcome.step("target-servo/debug/dst.dll", sync_file(Some(&src), &dst, "dst"));
        assert_eq!(outcome.errors, ["target-servo/debug/dst.dll is in use: close manifold and run again"]);
        drop(held);
        let _ = fs::remove_dir_all(&dir);
    }
}
