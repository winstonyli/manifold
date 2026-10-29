//! `gate package [EXE]` (was scripts/package.sh): the check for the bare-exe release `release.yml` builds. EXE
//! defaults to the newest `dist/manifold-v*-windows-x64.exe`. It is copied ALONE into a work folder, so nothing but
//! manifold.exe is beside it, and asserted on:
//!
//! - it imports no vcruntime/msvcp (`llvm-objdump -p`; the check fails without the tool);
//! - it runs headless with APPDATA redirected into the work folder (never the real one);
//! - `VIEWSTATE cef` reads "Not installed · N MB" when the release's pin manifest (`<exe stem>.host-pins.json`, beside
//!   the exe) pins a CEF windows-x64 host, else "Unavailable", and likewise for Servo;
//! - a WebView2 tab loads a local page, so the host was extracted from the embed (no host is in place);
//! - a PDF node renders the fixture (1000x500, 1 page) through the embedded PDF view, extracted beside the WebView2
//!   host (the first run downloads PDFium, so it needs the network once and waits up to 180 s);
//! - the folder still holds only manifold.exe, and the hosts were extracted under the redirected APPDATA.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::check::{check, Section};
use crate::instance::{log_tail, Instance};
use crate::probes::{finish, guarded, quit, seed, tab_node, Site};
use crate::windows_main::work_dir;

const FIXTURE: &str = "crates/manifold-view-pdfium/tests/fixture.pdf";
const PDF_WAIT: Duration = Duration::from_secs(180);
const INSTALL_WAIT: Duration = Duration::from_secs(600);

/// The newest `dist/manifold-v*-windows-x64.exe` under `repo`.
fn newest_release(repo: &Path) -> Option<PathBuf> {
    let mut found: Vec<_> = fs::read_dir(repo.join("dist"))
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("manifold-v") && n.ends_with("-windows-x64.exe")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    found.sort();
    found.pop().map(|(_, p)| p)
}

/// Whether the pin manifest has a `windows-x64` entry with files for `kind` (an engine's store name such as `cef`).
fn pins(manifest: &str, kind: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(manifest)
        .is_ok_and(|m| m[kind]["windows-x64"].as_array().is_some_and(|files| !files.is_empty()))
}

/// Whether `log`'s `view-state ENGINE:` line says what a build with (`pinned`) or without a download reports.
fn engine_state_ok(log: &str, engine: &str, pinned: bool) -> (bool, String) {
    let full = log.lines().find_map(|l| l.strip_prefix(&format!("view-state {engine}: "))).unwrap_or("");
    // The state may carry a trailing `[host missing, ..]` note.
    let line = full.split(" [").next().unwrap_or("").to_string();
    let ok = if pinned {
        line.strip_prefix("Not installed \u{b7} ")
            .and_then(|r| r.strip_suffix(" MB"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    } else {
        line.starts_with("Unavailable: this build has no ") && line.ends_with(" download")
    };
    (ok, line)
}

/// The DLLs `exe` imports, from `llvm-objdump -p`; None if the tool is missing.
fn imports(exe: &Path) -> Option<Vec<String>> {
    let out = Command::new("llvm-objdump").arg("-p").arg(exe).stdin(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    Some(text.lines().filter_map(|l| l.trim().strip_prefix("DLL Name: ")).map(str::to_owned).collect())
}

/// Polls the instance's log until a line starts with `pdf: rendered ` or `pdf: couldn` (or the instance dies).
fn wait_pdf(inst: &mut Instance) {
    let deadline = Instant::now() + PDF_WAIT;
    while Instant::now() < deadline && inst.is_running() {
        let log = fs::read_to_string(&inst.log).unwrap_or_default();
        if log.lines().any(|l| l.starts_with("pdf: rendered ") || l.starts_with("pdf: couldn")) {
            return;
        }
        sleep(Duration::from_millis(200));
    }
}

pub(crate) fn package(args: &[String]) -> ExitCode {
    // crates/gate/../..: the repo this gate was built from.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Some(exe_src) = args.first().map(PathBuf::from).or_else(|| newest_release(&repo)).filter(|p| p.is_file())
    else {
        eprintln!("no release exe: pass one, or download a draft release's exe");
        return ExitCode::from(1);
    };
    let work = match work_dir(None, "package") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let _t = Section::start("package");
    guarded("package", || run(&repo, &exe_src, &work));
    finish(&work)
}

/// `gate install-check ENGINE EXE`: the exe alone in a temp folder with APPDATA redirected, asked to install the
/// engine (`cef` or `servo`) from the network, as a user's Download click does. For a published release only: a draft's asset URLs are not public. Waits up
/// to 10 minutes for `VIEWSTATE ENGINE` to read Installed or Failed.
pub(crate) fn install_check(args: &[String]) -> ExitCode {
    let [engine, exe_src] = args else {
        eprintln!("usage: gate install-check cef|servo EXE");
        return ExitCode::from(2);
    };
    let Some(files) = installed_files(engine) else {
        eprintln!("install-check knows cef and servo, not {engine:?}");
        return ExitCode::from(2);
    };
    let work = match work_dir(None, "install-check") {
        Ok(w) => w,
        Err(e) => {
            eprintln!("work dir: {e}");
            return ExitCode::from(1);
        }
    };
    let _t = Section::start("install-check");
    guarded("install-check", || install(engine, &files, Path::new(exe_src), &work));
    finish(&work)
}

/// The files, relative to `engine-hosts/`, an installed `engine` leaves on disk; None for an engine that is not
/// downloaded. The Servo list is the manifest's three files and is unconfirmed until a published release carries them.
fn installed_files(engine: &str) -> Option<Vec<&'static str>> {
    match engine {
        "cef" => Some(vec!["manifold-view-cef.exe", "cef-runtime/libcef.dll"]),
        "servo" => Some(vec!["manifold-view-servo.exe", "libEGL.dll", "libGLESv2.dll"]),
        _ => None,
    }
}

fn install(engine: &str, files: &[&str], exe_src: &Path, work: &Path) -> io::Result<()> {
    let bin = work.join("bin");
    fs::create_dir_all(&bin)?;
    let exe = bin.join("manifold.exe");
    fs::copy(exe_src, &exe)?;
    let appdata = work.join("appdata");
    let profile = appdata.join("manifold");
    seed(&exe, &profile, "webview2", &[tab_node(None, "webview2")])?;
    let appdata_env = appdata.display().to_string();
    let mut inst = Instance::launch_env(&exe, &profile, work, "log", &[("APPDATA", &appdata_env)])?;
    inst.send("WAITTABS")?;
    inst.send(&format!("INSTALLVIEW {engine}"))?;
    let deadline = Instant::now() + INSTALL_WAIT;
    let prefix = format!("view-state {engine}: ");
    let mut last = String::new();
    while Instant::now() < deadline && inst.is_running() {
        inst.send(&format!("VIEWSTATE {engine}"))?;
        sleep(Duration::from_secs(2));
        let log = fs::read_to_string(&inst.log).unwrap_or_default();
        last = log.lines().rev().find(|l| l.starts_with(&prefix)).unwrap_or_default().to_owned();
        if last.starts_with(&format!("{prefix}Installed")) || last.starts_with(&format!("{prefix}Failed")) {
            break;
        }
    }
    quit(&mut inst, "headless manifold exits after QUIT");
    check(
        &format!("{engine} installed from the network (last state {last:?})"),
        last.starts_with(&format!("{prefix}Installed")),
    );
    let hosts = profile.join("manifold").join("data").join("engine-hosts");
    for file in files {
        check(&format!("{file} is on disk"), hosts.join(file).is_file());
    }
    Ok(())
}

fn run(repo: &Path, exe_src: &Path, work: &Path) -> io::Result<()> {
    let bin = work.join("bin");
    fs::create_dir_all(&bin)?;
    let exe = bin.join("manifold.exe");
    fs::copy(exe_src, &exe)?;
    let fixture = work.join("fixture.pdf");
    fs::copy(repo.join(FIXTURE), &fixture)?;
    println!("    {}: {} bytes", exe_src.file_name().unwrap_or_default().to_string_lossy(), fs::metadata(&exe)?.len());

    // The CRT is linked statically, so the exe needs no Visual C++ Redistributable (vcruntime140, msvcp140).
    match imports(&exe) {
        Some(dlls) => {
            println!("    imports: {}", dlls.join(" "));
            let needs = dlls.iter().any(|d| {
                let d = d.to_ascii_lowercase();
                d.contains("vcruntime") || d.contains("msvcp")
            });
            check("the exe needs no VC++ Redistributable", !needs);
        }
        None => {
            check("llvm-objdump found, to verify no vcruntime/msvcp imports (scoop install llvm)", false);
        }
    }

    let site = Site::start()?;
    let appdata = work.join("appdata");
    let profile = appdata.join("manifold");
    seed(&exe, &profile, "webview2", &[tab_node(Some(&site.url("/tall")), "webview2")])?;
    let appdata_env = appdata.display().to_string();
    let mut inst = Instance::launch_env(&exe, &profile, work, "log", &[("APPDATA", &appdata_env)])?;
    inst.send("WAITTABS")?;
    // The tab's page reports its load once the WebView2 host is extracted from the embed and running.
    let loaded = site.wait_for("load ", 1, Duration::from_secs(60));
    for line in ["VIEWSTATE cef", "VIEWSTATE servo"] {
        inst.send(line)?;
    }
    inst.send(&format!("ADDFILE {} 600 500", fixture.display()))?;
    inst.send("WAITTABS")?;
    // The first render downloads PDFium: wait for either outcome, then quit.
    wait_pdf(&mut inst);
    quit(&mut inst, "headless manifold exits after QUIT");

    let log = fs::read_to_string(&inst.log).unwrap_or_default();
    let manifest = fs::read_to_string(exe_src.with_extension("host-pins.json")).unwrap_or_default();
    for engine in ["cef", "servo"] {
        let (ok, line) = engine_state_ok(&log, engine, pins(&manifest, engine));
        if !check(&format!("{engine} state line is right (got {line:?})"), ok) {
            println!("    log tail:\n{}", log_tail(&inst.log, 5));
        }
    }
    check("a WebView2 tab loaded the local page (host extracted from the embed)", loaded);
    let hosts = profile.join("manifold").join("data").join("engine-hosts");
    check(
        "the WebView2 host was extracted under the redirected APPDATA",
        hosts.join("webview2").join("manifold-view-webview2.exe").is_file(),
    );
    let rendered = log.lines().any(|l| l.starts_with("pdf: rendered ") && l.contains("fixture.pdf: 1000x500, 1 page"));
    if !check("the PDF view rendered the fixture (1000x500, 1 page)", rendered) {
        for l in log.lines().filter(|l| l.starts_with("pdf: ")) {
            println!("    log: {l}");
        }
    }
    check(
        "the PDF view was extracted under the redirected APPDATA",
        hosts.join("pdfium").join("manifold-view-pdfium.exe").is_file(),
    );
    let beside: Vec<String> =
        fs::read_dir(&bin)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    check(&format!("nothing but manifold.exe sits beside the exe (found {beside:?})"), beside == ["manifold.exe"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_state_lines() {
        let log = "x\nview-state cef: Not installed \u{b7} 172 MB\nview-state servo: Unavailable: this build has no Servo download\n";
        assert!(engine_state_ok(log, "cef", true).0);
        assert!(!engine_state_ok(log, "cef", false).0);
        assert!(engine_state_ok(log, "servo", false).0);
        assert!(!engine_state_ok(log, "servo", true).0);
        assert!(!engine_state_ok("", "cef", true).0);
    }

    #[test]
    fn the_manifest_pins_by_engine_and_platform() {
        let manifest = r#"{"cef": {"windows-x64": [{"file": "a.exe", "size": 1, "sha256": "00"}], "linux-x64": []}}"#;
        assert!(pins(manifest, "cef"));
        assert!(!pins(manifest, "servo"));
        assert!(!pins(r#"{"cef": {"linux-x64": [{}]}}"#, "cef"));
        assert!(!pins("", "cef"));
    }
}
