//! `gate sync-runtime-deps` (was scripts/sync-runtime-deps.sh): puts each engine host's runtime files beside the host
//! a dev tree runs (README "Runtime-file placement"). Cargo does not, and the fingerprint folder they are built into
//! can change on a rebuild that touched neither Cargo.toml nor Cargo.lock, silently staling an earlier copy.
//!
//! One rule for every row of `manifold_hosts::ALL` (`sync_host`). A host's build folder is `<dev_dir>/debug/`, or
//! `target/debug/` for a host with no `dev_dir`. Its sidecars go beside its exe there (Servo's ANGLE DLLs beside
//! `target-servo/debug/manifold-view-servo.exe`), and its runtime and a copy of its exe go into its runtime folder
//! there (CEF's into `target/debug/cef-runtime/`, the copy manifold runs). Each part comes from the host's own
//! `build/` folder when that holds one, else from `target/debug/build/` (`find_first`). A part found in neither is
//! kept when it is already in place, an error when the host exe is built, and skipped otherwise. A host whose build
//! folder is missing is skipped; the run fails only when no host with something to sync has one.
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

use manifold_hosts::{BuildOutput, Host, Os};

use crate::cef_pin::repo;

fn say(msg: &str) {
    println!("sync-runtime-deps: {msg}");
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The newest match by mtime in the first of `roots` that has one. Each root is searched in both cargo layouts,
/// `<root>/<krate>-*/out/<tail>` and `<root>/<krate>/*/out/<tail>`; a root that isn't there is skipped.
pub(crate) fn find_first(roots: &[PathBuf], from: &BuildOutput) -> Option<PathBuf> {
    roots.iter().find_map(|root| {
        let mut found = Vec::new();
        for e in fs::read_dir(root).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("{}-", from.krate)) {
                found.push(e.path().join("out").join(from.tail));
            } else if name == from.krate {
                for h in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                    found.push(h.path().join("out").join(from.tail));
                }
            }
        }
        found.into_iter().filter_map(|p| Some((mtime(&p)?, p))).max().map(|(_, p)| p)
    })
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
        say(&format!("no candidate found for {label}, skipping"));
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

/// What `sync_host` found, so `run_in` and the tests read it instead of captured output.
#[derive(Debug, PartialEq, Eq)]
enum HostSync {
    /// No sidecar for this OS and no runtime: nothing is looked for or printed, and the run does not count it.
    NothingToSync,
    /// Its build folder is missing: skipped, with one line.
    NoBuildFolder,
    /// Its build folder is there, and each part was copied, kept, skipped or listed as a failure.
    Synced,
}

/// One host's parts in its build folder, `root/<dev_dir or target>/debug/`: each sidecar for this OS beside the exe,
/// then, for a host with a runtime, the runtime and a copy of the exe in `<runtime.dir>/`. A part comes from
/// `find_first` over the folder's own `build/`, then `target/debug/build/`. One found in neither is kept when it is
/// in place (the sidecar beside the exe; the runtime folder holding its marker file), an error when the exe is built,
/// and skipped otherwise. The exe copy is always compared (it is rebuilt with its crate), and is attempted only once
/// the runtime was copied or is in place: otherwise the line above already says why, and the copy would fail on a
/// missing folder as a second error.
fn sync_host(root: &Path, host: &Host, outcome: &mut Outcome) -> HostSync {
    let here = Os::here();
    if host.sidecars_for(here).next().is_none() && host.runtime.is_none() {
        return HostSync::NothingToSync;
    }
    let (dev, package) = (host.dev_dir.unwrap_or("target"), host.exe);
    let folder = root.join(dev).join("debug");
    if !folder.is_dir() {
        say(&format!("no {dev}/debug, skipping {package}"));
        return HostSync::NoBuildFolder;
    }
    let exe = host.file_name(here);
    let exe_there = folder.join(&exe).is_file();
    let mut roots = vec![folder.join("build")];
    if host.dev_dir.is_some() {
        roots.push(root.join("target").join("debug").join("build"));
    }
    for sidecar in host.sidecars_for(here) {
        let file = sidecar.file;
        let label = format!("{dev}/debug/{file}");
        let dst = folder.join(file);
        match find_first(&roots, &sidecar.from) {
            Some(src) => outcome.step(&label, sync_file(Some(&src), &dst, &label)),
            None if dst.is_file() => say(&format!("no build output holds {file}, keeping {label} in place")),
            None if exe_there => {
                outcome.errors.push(format!("no build output holds {file}, which {exe} needs: build {package} first"))
            }
            None => say(&format!("no build output holds {file} and no {exe} is built, skipping")),
        }
    }
    if let (Some(runtime), Some(lib)) = (&host.runtime, host.runtime_lib(here)) {
        let dir = runtime.dir;
        let dst = folder.join(dir);
        let runtime_ready = match find_first(&roots, &runtime.from) {
            Some(out) => match copy_runtime(&out, &folder, dir, outcome) {
                Ok(()) => true,
                Err(e) => {
                    outcome.errors.push(step_error(dir, &e));
                    false
                }
            },
            None if dst.join(lib).is_file() => {
                say(&format!("no build output holds {package}'s runtime, keeping {dir}/{lib} in place"));
                true
            }
            None if exe_there => {
                outcome.errors.push(format!(
                    "no build output holds the runtime {exe} needs ({lib}): a CEF_PATH build leaves it outside \
                     OUT_DIR, so copy it into {dev}/debug/{dir}/"
                ));
                false
            }
            None => {
                say(&format!("no build output holds {package}'s runtime and no {exe} is built, skipping"));
                false
            }
        };
        if runtime_ready {
            let label = format!("{dir}/{exe}");
            let built = folder.join(&exe);
            outcome.step(&label, sync_file(exe_there.then_some(built.as_path()), &dst.join(&exe), &label));
        }
    }
    HostSync::Synced
}

/// Each file of a runtime's build output `out` into `folder/<dir>/`, recopied when it differs, and each directory
/// (CEF's locales/) only when missing or empty, since those don't change between rebuilds. A file or directory that
/// fails is listed and the rest still copy; only a folder that can't be created or listed stops it.
fn copy_runtime(out: &Path, folder: &Path, dir: &str, outcome: &mut Outcome) -> io::Result<()> {
    let runtime = folder.join(dir);
    fs::create_dir_all(&runtime)?;
    for e in fs::read_dir(out)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let label = format!("{dir}/{name}");
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

/// Syncs every host into the dev tree at `root`. Fails with each failed step, or when no host with something to
/// sync has its build folder.
fn run_in(root: &Path) -> Result<(), Vec<String>> {
    let mut outcome = Outcome::default();
    let mut synced = false;
    for host in manifold_hosts::ALL {
        synced |= sync_host(root, host, &mut outcome) == HostSync::Synced;
    }
    if !synced {
        return Err(vec!["no build folder for any host - build first".to_string()]);
    }
    finish(outcome)
}

fn run() -> Result<(), Vec<String>> {
    run_in(&repo())
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
    use manifold_hosts::{Os, CEF, PDFIUM, SERVO, WEBKIT, WEBVIEW2};

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

    /// The names in `folder`, sorted.
    fn names_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(folder).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    }

    /// A part comes from the host's own build folder when that holds a match, the newest written there across both
    /// cargo layouts, even though `target/`'s is newer still; a part only `target/` holds comes from there; another
    /// crate's output is never taken.
    #[test]
    fn each_part_comes_from_the_hosts_own_build_folder_first_and_the_newest_there() {
        let root = scratch("first");
        let (own, main) = (root.join("target-servo/debug/build"), root.join("target/debug/build"));
        touch(&own.join("mozangle-aaa/out/libEGL.dll"), 10, b"own, older");
        touch(&own.join("mozangle/bbb/out/libEGL.dll"), 20, b"own, newest");
        touch(&main.join("mozangle-ccc/out/libEGL.dll"), 30, b"target, newer still");
        touch(&own.join("other-ddd/out/libEGL.dll"), 40, b"another crate");
        touch(&main.join("mozangle/eee/out/libGLESv2.dll"), 10, b"target only");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &SERVO, &mut outcome), HostSync::Synced);
        assert!(outcome.changed && outcome.errors.is_empty(), "{:?}", outcome.errors);
        let debug = root.join("target-servo/debug");
        assert_eq!(fs::read(debug.join("libEGL.dll")).unwrap(), b"own, newest");
        assert_eq!(fs::read(debug.join("libGLESv2.dll")).unwrap(), b"target only");
        let _ = fs::remove_dir_all(&root);
    }

    /// A file is copied when its bytes differ from the copy's and not again once they agree; with no source it is
    /// skipped.
    #[test]
    fn sync_file_copies_only_on_change() {
        let dir = scratch("copy");
        let (src, dst) = (dir.join("src.dll"), dir.join("dst.dll"));
        fs::write(&src, b"one").unwrap();
        assert!(sync_file(Some(&src), &dst, "x").unwrap(), "first copy");
        assert!(!sync_file(Some(&src), &dst, "x").unwrap(), "already current");
        fs::write(&src, b"two").unwrap();
        assert!(sync_file(Some(&src), &dst, "x").unwrap(), "changed bytes copy again");
        assert_eq!(fs::read(&dst).unwrap(), b"two");
        assert!(!sync_file(None, &dst, "x").unwrap(), "no source: skipped");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Servo's DLLs go beside its host in `target-servo/debug/`. With no such folder the host is skipped; with the
    /// folder and neither a host nor a DLL in any build, nothing happens; a DLL that can't be copied is listed and the
    /// other still copies; a built host with no DLL to give it is an error for each.
    #[test]
    fn servo_dlls_go_beside_its_host_and_a_failed_one_does_not_stop_the_rest() {
        let dir = scratch("servo");
        for dll in ["libEGL.dll", "libGLESv2.dll"] {
            touch(&dir.join("target/debug/build/mozangle-aaa/out").join(dll), 10, dll.as_bytes());
        }
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&dir, &SERVO, &mut outcome), HostSync::NoBuildFolder);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no target-servo/debug: skipped, {:?}", outcome.errors);

        let empty = dir.join("empty");
        fs::create_dir_all(empty.join("target-servo/debug")).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&empty, &SERVO, &mut outcome), HostSync::Synced);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no host and no DLL: skipped, {:?}", outcome.errors);

        let servo_debug = dir.join("target-servo/debug");
        fs::create_dir_all(servo_debug.join("libEGL.dll")).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&dir, &SERVO, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].starts_with("target-servo/debug/libEGL.dll: "), "{:?}", outcome.errors);
        assert!(outcome.changed, "libGLESv2.dll still copied");
        assert_eq!(fs::read(servo_debug.join("libGLESv2.dll")).unwrap(), b"libGLESv2.dll");

        let bare = dir.join("bare");
        touch(&bare.join("target-servo/debug").join(SERVO.file_name(Os::here())), 10, b"host");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&bare, &SERVO, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 2, "{:?}", outcome.errors);
        assert!(outcome.errors.iter().all(|e| e.contains("build manifold-view-servo first")), "{:?}", outcome.errors);
        let _ = fs::remove_dir_all(&dir);
    }

    /// CEF's runtime and its host go into `cef-runtime/`: a file that can't be copied is listed while every other
    /// file, the locales folder and the host exe still copy (a failed file does not stop the exe), and the sync then
    /// fails; once everything is current a rerun copies nothing and succeeds.
    #[test]
    fn a_cef_file_that_cant_be_copied_is_listed_and_the_rest_still_copy() {
        let root = scratch("cef");
        let td = root.join("target/debug");
        let out = td.join("build/cef-dll-sys-aaa/out/cef_windows_x86_64");
        for file in ["a.dll", "b.dll", "locales/en.pak"] {
            touch(&out.join(file), 10, file.as_bytes());
        }
        let exe = CEF.file_name(Os::here());
        touch(&td.join(&exe), 10, b"host");
        let runtime = td.join("cef-runtime");
        // A folder where a file should go: copying onto it fails on every run.
        fs::create_dir_all(runtime.join("a.dll")).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].starts_with("cef-runtime/a.dll: "), "{:?}", outcome.errors);
        for file in ["b.dll", "locales/en.pak"] {
            assert_eq!(fs::read(runtime.join(file)).unwrap(), file.as_bytes(), "{file}");
        }
        assert_eq!(fs::read(runtime.join(&exe)).unwrap(), b"host");
        assert!(outcome.changed);
        let errors = outcome.errors.clone();
        assert_eq!(finish(outcome), Err(errors), "a failed step fails the sync");

        fs::remove_dir(runtime.join("a.dll")).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert!(outcome.changed && outcome.errors.is_empty(), "a.dll now copies: {:?}", outcome.errors);
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert!(!outcome.changed && outcome.errors.is_empty(), "all current: {:?}", outcome.errors);
        assert_eq!(finish(outcome), Ok(()));
        let _ = fs::remove_dir_all(&root);
    }

    /// A part no build output holds is kept when it is in place: a sidecar beside the exe, or a runtime folder
    /// holding its marker file. Neither is an error or copied over, and a runtime in place still gets the built exe.
    /// A built exe whose part is neither found nor in place is an error. A host with no runtime gets no copy of its
    /// exe.
    #[test]
    fn a_part_in_place_is_kept_and_the_exe_follows_a_runtime_in_place() {
        let root = scratch("kept");
        let servo_debug = root.join("target-servo/debug");
        let servo_exe = SERVO.file_name(Os::here());
        touch(&servo_debug.join(&servo_exe), 10, b"servo host");
        touch(&servo_debug.join("libEGL.dll"), 10, b"kept");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &SERVO, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 1, "only libGLESv2.dll is missing: {:?}", outcome.errors);
        assert!(outcome.errors[0].contains("libGLESv2.dll"), "{:?}", outcome.errors);
        assert!(!outcome.changed);
        assert_eq!(fs::read(servo_debug.join("libEGL.dll")).unwrap(), b"kept");
        assert_eq!(names_in(&servo_debug), ["libEGL.dll", servo_exe.as_str()], "no copy of a host without a runtime");

        let td = root.join("target/debug");
        let cef_exe = CEF.file_name(Os::here());
        touch(&td.join(&cef_exe), 10, b"cef host");
        touch(&td.join("cef-runtime/libcef.dll"), 10, b"runtime in place");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert!(outcome.changed && outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(fs::read(td.join("cef-runtime/libcef.dll")).unwrap(), b"runtime in place");
        assert_eq!(fs::read(td.join("cef-runtime").join(&cef_exe)).unwrap(), b"cef host", "the exe follows");
        let _ = fs::remove_dir_all(&root);
    }

    /// A CEF build with no runtime in any build output and none in place (a `CEF_PATH` build) is exactly one error,
    /// naming `CEF_PATH` and the folder to copy the runtime into; the exe copy is not attempted, so no folder is
    /// created. With no exe built either, it is skipped.
    #[test]
    fn a_cef_build_with_no_runtime_is_one_error_naming_cef_path_and_creates_no_folder() {
        let root = scratch("cef-path");
        let td = root.join("target/debug");
        fs::create_dir_all(&td).unwrap();
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no exe and no runtime: skipped, {:?}", outcome.errors);
        assert!(!td.join("cef-runtime").exists());

        touch(&td.join(CEF.file_name(Os::here())), 10, b"cef host");
        let mut outcome = Outcome::default();
        assert_eq!(sync_host(&root, &CEF, &mut outcome), HostSync::Synced);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].contains("CEF_PATH"), "{:?}", outcome.errors);
        assert!(outcome.errors[0].contains("target/debug/cef-runtime/"), "{:?}", outcome.errors);
        assert!(!outcome.changed);
        assert!(!td.join("cef-runtime").exists(), "no folder created");
        let _ = fs::remove_dir_all(&root);
    }

    /// Servo and CEF have parts to sync, so a tree without their build folders skips each. WebKit, WebView2 and
    /// PDFium have no sidecar on this OS and no runtime: nothing is looked for, even with build folders there.
    #[test]
    fn a_host_with_nothing_to_sync_is_not_looked_for() {
        let root = scratch("nothing");
        for host in [&SERVO, &CEF] {
            let mut outcome = Outcome::default();
            assert_eq!(sync_host(&root, host, &mut outcome), HostSync::NoBuildFolder, "{}", host.id);
            assert!(!outcome.changed && outcome.errors.is_empty(), "{}: {:?}", host.id, outcome.errors);
        }
        for dev in ["target", "target-linux"] {
            fs::create_dir_all(root.join(dev).join("debug")).unwrap();
        }
        for host in [&WEBKIT, &WEBVIEW2, &PDFIUM] {
            let mut outcome = Outcome::default();
            assert_eq!(sync_host(&root, host, &mut outcome), HostSync::NothingToSync, "{}", host.id);
            assert!(!outcome.changed && outcome.errors.is_empty(), "{}: {:?}", host.id, outcome.errors);
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// The run fails only when no host with something to sync has its build folder: either host's folder is enough,
    /// and WebKit's, whose host has nothing to sync, does not count.
    #[test]
    fn the_run_fails_only_when_no_host_has_its_build_folder() {
        let none = Err(vec!["no build folder for any host - build first".to_string()]);
        let root = scratch("run");
        assert_eq!(run_in(&root), none, "an empty tree");
        fs::create_dir_all(root.join("target-linux/debug")).unwrap();
        assert_eq!(run_in(&root), none, "WebKit's folder alone");
        fs::create_dir_all(root.join("target-servo/debug")).unwrap();
        assert_eq!(run_in(&root), Ok(()), "Servo's folder: CEF's missing target/debug is skipped");
        let other = scratch("run-target");
        fs::create_dir_all(other.join("target/debug")).unwrap();
        assert_eq!(run_in(&other), Ok(()), "target/debug alone: Servo's missing folder is skipped");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&other);
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
