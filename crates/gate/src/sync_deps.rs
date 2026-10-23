//! `gate sync-runtime-deps` (was scripts/sync-runtime-deps.sh): recopies Servo's ANGLE DLLs and CEF's runtime files
//! from their build.rs output directories into target/debug/, where manifold-view-servo.exe and
//! manifold-view-cef.exe (through its isolated `cef-runtime/` subdirectory) load them from. Cargo does not do this,
//! and the fingerprint hash directory they live under can change on a rebuild that touched neither Cargo.toml nor
//! Cargo.lock, silently staling an earlier copy (README "Manual runtime-file placement"). Safe to run any time (a
//! no-op when everything is current); run it after every `cargo build` that touches manifold-view-servo or
//! manifold-view-cef. Not a build.rs: a build script sees only its own crate's OUT_DIR.
//!
//! The source is the most recently *written* match, not the newest fingerprint directory, whose mtime cargo touches
//! on every build. Newer cargo nightlies put build-script output under `build/<crate>/<hash>/`, older ones under
//! `build/<crate>-<hash>/`, and a target dir built by both has each.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

fn say(msg: &str) {
    println!("sync-runtime-deps: {msg}");
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// `build/<krate>-*/out/<tail>` and `build/<krate>/*/out/<tail>` that exist, the newest by mtime.
fn find_newest(build: &Path, krate: &str, tail: &str) -> Option<PathBuf> {
    let mut found = Vec::new();
    for e in fs::read_dir(build).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{krate}-")) {
            found.push(e.path().join("out").join(tail));
        } else if name == krate {
            for h in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                found.push(h.path().join("out").join(tail));
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

fn run() -> io::Result<()> {
    let target_debug = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .canonicalize()
        .map_err(|_| io::Error::other("target/debug does not exist - build first"))?;
    let build = target_debug.join("build");
    let mut changed = false;
    for dll in ["libEGL.dll", "libGLESv2.dll"] {
        let src = find_newest(&build, "mozangle", dll);
        changed |= sync_file(src.as_deref(), &target_debug.join(dll), dll)?;
    }
    // CEF's full runtime, isolated in its own subdirectory (see process_manager.rs's engine_binary_path).
    let runtime = target_debug.join("cef-runtime");
    if let Some(out) = find_newest(&build, "cef-dll-sys", "cef_windows_x86_64") {
        fs::create_dir_all(&runtime)?;
        for e in fs::read_dir(&out)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let dst = runtime.join(&name);
            if e.file_type()?.is_dir() {
                // Directories (locales/) are part of the CEF distribution and don't change between rebuilds.
                if fs::read_dir(&dst).map_or(true, |mut d| d.next().is_none()) {
                    copy_dir(&e.path(), &dst)?;
                    say(&format!("copied cef-runtime/{name}/"));
                    changed = true;
                }
            } else {
                changed |= sync_file(Some(&e.path()), &dst, &format!("cef-runtime/{name}"))?;
            }
        }
    } else {
        eprintln!("sync-runtime-deps: no cef-dll-sys output directory found, skipping CEF runtime");
    }
    // The host exe is rebuilt whenever manifold-view-cef changes, unlike the DLLs: always resync it.
    let exe = target_debug.join("manifold-view-cef.exe");
    changed |= sync_file(
        exe.is_file().then_some(exe.as_path()),
        &runtime.join("manifold-view-cef.exe"),
        "cef-runtime/manifold-view-cef.exe",
    )?;
    if !changed {
        say("everything already current, nothing to do");
    }
    Ok(())
}

pub fn sync_runtime_deps() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sync-runtime-deps: {e}");
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

    #[test]
    fn the_newest_of_both_layouts_wins_and_sync_copies_only_on_change() {
        let dir = std::env::temp_dir().join(format!("gate-sync-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let build = dir.join("build");
        touch(&build.join("mozangle-aaa/out/x.dll"), 10, b"old");
        touch(&build.join("mozangle/bbb/out/x.dll"), 20, b"new");
        touch(&build.join("other-ccc/out/x.dll"), 30, b"other");
        let src = find_newest(&build, "mozangle", "x.dll").unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"new");
        let dst = dir.join("x.dll");
        assert!(sync_file(Some(&src), &dst, "x").unwrap(), "first copy");
        assert!(!sync_file(Some(&src), &dst, "x").unwrap(), "already current");
        assert!(find_newest(&build, "absent", "x.dll").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
