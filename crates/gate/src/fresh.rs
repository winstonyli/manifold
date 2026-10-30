//! Was scripts/fresh-exe.sh: refuse to drive an exe older than the sources it was built from. `cargo check`, `test` and
//! `clippy` do not rebuild it, and `cargo build -p manifold` does not build the WebView2 host, so each exe is judged
//! by its own cargo dep-info file (`manifold.d` beside `manifold.exe`).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use manifold_hosts::{Host, Os, PDFIUM, WEBVIEW2};

/// The hosts the gate's build line builds beside manifold.exe: separate crates that `-p manifold` does not build, so
/// each is judged by its own dep-info, and the build hint names them (one list of these is PARKED "The `-p` crate
/// lists are repeated").
static DEP_INFO_HOSTS: [&Host; 2] = [&WEBVIEW2, &PDFIUM];

/// The sources a dep-info file lists: one make rule, `<exe>: <src> <src> ...`, with a space in a path escaped as
/// `\ ` (paths are Windows paths, so a backslash before anything else is no escape).
pub fn parse_dep_info(text: &str) -> Vec<PathBuf> {
    let first = text.lines().next().unwrap_or("");
    let Some(at) = first.rfind(".exe: ") else { return Vec::new() };
    let mut sources = Vec::new();
    let mut cur = String::new();
    let mut chars = first[at + ".exe: ".len()..].chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                chars.next();
                cur.push(' ');
            }
            ' ' => {
                if !cur.is_empty() {
                    sources.push(PathBuf::from(std::mem::take(&mut cur)));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        sources.push(PathBuf::from(cur));
    }
    sources
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The sources listed in `<exe minus .exe>.d` that are newer than `exe` (none if there is no `.d`; a source that no
/// longer exists is not newer).
fn sources_newer_than(exe: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(exe.with_extension("d")) else { return Vec::new() };
    let Some(built) = mtime(exe) else { return Vec::new() };
    parse_dep_info(&text).into_iter().filter(|src| mtime(src).is_some_and(|t| t > built)).collect()
}

/// `require_fresh_exe`: `Err` names the stale files. `ALLOW_STALE` set (non-empty) skips the check.
pub fn require_fresh_exe(exe: &Path) -> Result<(), String> {
    if std::env::var_os("ALLOW_STALE").is_some_and(|v| !v.is_empty()) {
        return Ok(());
    }
    let dir = exe.parent().unwrap_or(Path::new("."));
    // A host with a runtime (CEF) runs from an isolated copy in its runtime folder that only `gate sync-runtime-deps`
    // refreshes. The app itself refuses a copy whose protocol number differs from its own (Unavailable, naming which
    // side to rebuild); this catches a copy that is older but speaks the same protocol, which would run old code
    // silently.
    for host in manifold_hosts::ALL {
        let Some(runtime) = &host.runtime else { continue };
        let file = host.file_name(Os::here());
        if let (Some(built), Some(copy)) = (mtime(&dir.join(&file)), mtime(&host.beside(Os::here(), dir))) {
            if built > copy {
                return Err(format!(
                    "{}/{file} is older than the built one: run `target/debug/gate.exe sync-runtime-deps`, or set ALLOW_STALE=1.",
                    runtime.dir
                ));
            }
        }
    }
    // The hosts in `DEP_INFO_HOSTS`, each judged by its own dep-info; one that is absent is skipped.
    let hosts: Vec<PathBuf> = DEP_INFO_HOSTS.iter().map(|h| dir.join(h.file_name(Os::here()))).collect();
    let mut report = String::new();
    for e in std::iter::once(exe).chain(hosts.iter().map(PathBuf::as_path)) {
        if e != exe && !e.exists() {
            continue;
        }
        let newer = sources_newer_than(e);
        if !newer.is_empty() {
            report.push_str(&format!("{} is older than:\n", e.display()));
            for n in newer {
                report.push_str(&format!("  {}\n", n.display()));
            }
        }
    }
    if report.is_empty() {
        Ok(())
    } else {
        let crates: String = DEP_INFO_HOSTS.iter().map(|h| format!(" -p {}", h.exe)).collect();
        Err(format!("{report}Build first (cargo build -j 8 -p manifold{crates}), or set ALLOW_STALE=1."))
    }
}

#[cfg(test)]
mod tests {
    use manifold_hosts::CEF;

    use super::*;

    #[test]
    fn dep_info_unescapes_spaces() {
        let text = "C:\\Users\\Winston\\ Li\\t\\manifold.exe: C:\\Users\\Winston\\ Li\\a.rs C:\\b.rs\n\nC:\\b.rs:\n";
        assert_eq!(parse_dep_info(text), [PathBuf::from("C:\\Users\\Winston Li\\a.rs"), PathBuf::from("C:\\b.rs")]);
        assert!(parse_dep_info("garbage").is_empty());
    }

    /// A folder with manifold.exe and the hosts in `DEP_INFO_HOSTS`, each with a `.d` naming `src.rs`, and CEF's built
    /// exe with its copy in its runtime folder (`cef_exe_and_copy`); `src.rs` and every exe get the mtimes (seconds
    /// since the epoch) the test sets.
    struct Bin(PathBuf);

    impl Bin {
        fn new(name: &str) -> Bin {
            let dir = std::env::temp_dir().join(format!("gate-fresh-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let bin = Bin(dir);
            bin.touch("src.rs", 1);
            // `manifold` is the app's own exe, not a host row.
            let exes = std::iter::once(("manifold", "manifold.exe".to_owned()))
                .chain(DEP_INFO_HOSTS.iter().map(|h| (h.exe, h.file_name(Os::here()))));
            for (stem, file) in exes {
                let rule = format!("{}: {}", bin.0.join(&file).display(), bin.0.join("src.rs").display());
                std::fs::write(
                    bin.0.join(format!("{stem}.d")),
                    rule.replace(' ', "\\ ").replacen(".exe:\\ ", ".exe: ", 1),
                )
                .unwrap();
                bin.touch(&file, 2);
            }
            let (cef, copy) = cef_exe_and_copy();
            bin.touch(&cef, 2);
            bin.touch(&copy, 2);
            bin
        }

        /// Sets `file`'s mtime, creating the file and its folder if they are missing.
        fn touch(&self, file: &str, secs: u64) {
            let path = self.0.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let f = std::fs::OpenOptions::new().create(true).append(true).open(&path).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        }

        fn check(&self) -> Result<(), String> {
            require_fresh_exe(&self.0.join("manifold.exe"))
        }
    }

    impl Drop for Bin {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fresh_exes_pass_and_each_stale_one_is_named() {
        if std::env::var_os("ALLOW_STALE").is_some() {
            return;
        }
        let bin = Bin::new("each");
        assert_eq!(bin.check(), Ok(()), "all exes fresh");

        // The source changes after the WebView2 host was built, but manifold.exe and the PDF view are rebuilt.
        bin.touch("src.rs", 3);
        bin.touch("manifold.exe", 4);
        bin.touch("manifold-view-pdfium.exe", 4);
        let err = bin.check().unwrap_err();
        assert!(err.contains("manifold-view-webview2.exe is older"), "{err}");
        assert!(err.contains("-p manifold -p manifold-view-webview2"), "{err}");
        assert!(!err.contains("manifold-view-pdfium.exe is older"), "{err}");

        bin.touch("manifold-view-webview2.exe", 4);
        bin.touch("manifold-view-pdfium.exe", 2);
        let err = bin.check().unwrap_err();
        assert!(err.contains("manifold-view-pdfium.exe is older"), "{err}");

        // A host that is absent is skipped.
        std::fs::remove_file(bin.0.join("manifold-view-webview2.exe")).unwrap();
        std::fs::remove_file(bin.0.join("manifold-view-pdfium.exe")).unwrap();
        assert_eq!(bin.check(), Ok(()), "a missing host is skipped");

        bin.touch("manifold.exe", 2);
        assert!(bin.check().unwrap_err().contains("manifold.exe is older"), "only manifold.exe stale");
    }

    /// CEF's built exe and its copy in its runtime folder, relative to a `Bin`'s folder.
    fn cef_exe_and_copy() -> (String, String) {
        let file = CEF.file_name(Os::here());
        let runtime = CEF.runtime.as_ref().expect("CEF has a runtime");
        let copy = format!("{}/{file}", runtime.dir);
        (file, copy)
    }

    /// A host's copy in its runtime folder (CEF's `cef-runtime/`) older than the built exe is refused, naming the sync
    /// that refreshes it and `ALLOW_STALE`; with no copy there is nothing to compare.
    #[test]
    fn a_stale_runtime_copy_is_refused_and_an_absent_one_skipped() {
        if std::env::var_os("ALLOW_STALE").is_some() {
            return;
        }
        let bin = Bin::new("runtime");
        let (cef, copy) = cef_exe_and_copy();
        assert_eq!(bin.check(), Ok(()), "the copy is as new as the built exe");

        bin.touch(&cef, 3);
        let err = bin.check().unwrap_err();
        assert!(err.contains(&format!("{copy} is older than the built one")), "{err}");
        assert!(err.contains("sync-runtime-deps") && err.contains("ALLOW_STALE"), "{err}");

        std::fs::remove_file(bin.0.join(&copy)).unwrap();
        assert_eq!(bin.check(), Ok(()), "an absent copy is skipped");
    }
}
