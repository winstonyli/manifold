//! The five small probes that were bash scripts (a Git Bash fork costs seconds on this machine), as `gate` stages. Each
//! launches `manifold --headless` on an isolated profile seeded with one tab and checks what the app or the page saw:
//!
//! - `page-kind` (was scripts/page-kind-probe.sh): a tab at a local URL answering `application/json` or `image/png`
//!   turns into a Json or Image card; a plain HTML page stays a live tab
//! - `hscroll` (was hscroll-probe.sh): a horizontal wheel scroll moves a 6000 px wide page right, then back
//! - `key` (was key-probe.sh): the keys `KEYPAGE` sends arrive at the page with the code/key/keyCode `key_codes!` says
//! - `bad-url` (was bad-url-probe.sh): a view host given an initial URL it cannot open shows about:blank, no panic
//! - `failed-spawn` (was failed-spawn-probe.sh): a failed spawn leaves one failed tab card, kept by New and Retry
//!
//! `VIEWS="webview2 cef"` picks the view kinds (default `webview2`; CEF needs its host built). The local site is served
//! in process (was scripts/probe_server.py).

use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::check::{check, check_eq, exit_code, failures, note_failure, Section};
use crate::http::{self, response};
use crate::instance::{remove_dir_with_retries, seed_settings, Instance, STEP_TIMEOUT};
use crate::state::{State, JAR, TAB};
use crate::suggest::{query_param, url_query};
use crate::windows_main::{manifold_exe, work_dir};

const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

const PAGE: &str = "<!doctype html><title>keys</title><textarea id=t autofocus></textarea>\n<script>\n\
document.getElementById('t').focus();\n\
addEventListener('keydown', e => {\n  const q = new URLSearchParams({code: e.code, key: e.key, kc: e.keyCode});\n  new Image().src = '/k?' + q;\n});\n</script>";
const WIDE: &str = "<!doctype html><title>wide</title>\n\
<div style=\"width:6000px;height:400px;background:linear-gradient(90deg,#fff,#9cf)\"></div>\n<script>\nlet t;\n\
addEventListener('scroll', () => {\n  clearTimeout(t);\n  t = setTimeout(() => fetch('/s?e=hscroll&y=' + Math.round(scrollX)), 300);\n});\n</script>";

/// A page taller than any window, for the sleep probe: it reports its scroll position on load and after each scroll
/// settles, and `?dirty` makes its input differ from the default.
const TALL: &str = "<!doctype html><title>tall</title><input id=i>\n\
<div style=\"height:6000px;background:linear-gradient(#fff,#9cf)\"></div>\n<script>\n\
if (location.search.includes('dirty')) document.getElementById('i').value = 'draft';\n\
fetch('/s?e=load&y=' + Math.round(scrollY));\nlet t;\n\
addEventListener('scroll', () => {\n  clearTimeout(t);\n  t = setTimeout(() => fetch('/s?e=scroll&y=' + Math.round(scrollY)), 300);\n});\n</script>";

/// The local site: `/` reports keydowns (`/k`), `/wide` reports horizontal scrolls (`/s`), `/tall` vertical ones,
/// `/data.json` and `/pixel.png` answer with their real content types. Everything the page reports lands in `lines`.
pub(crate) struct Site {
    server: http::Server,
    lines: Arc<Mutex<Vec<String>>>,
    /// Every request target the server has answered, in order.
    requests: Arc<Mutex<Vec<String>>>,
}

impl Site {
    pub(crate) fn start() -> io::Result<Site> {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let log = lines.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let mut png = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(4, 4, image::Rgb([0xd7, 0xa2, 0x4a]))
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(io::Error::other)?;
        let png = png.into_inner();
        let server = http::Server::start(Arc::new(move |request_line: &str| {
            let target = request_line.split(' ').nth(1).unwrap_or("");
            seen.lock().unwrap_or_else(PoisonError::into_inner).push(target.to_owned());
            let path = target.split_once('?').map_or(target, |(p, _)| p);
            let query = url_query(target).to_string();
            let param = |k: &str| query_param(&query, k, true).unwrap_or_default();
            let note = |line: String| log.lock().unwrap_or_else(PoisonError::into_inner).push(line);
            match path {
                "/k" => {
                    note(format!("code={} key={} kc={}", param("code"), param("key"), param("kc")));
                    response("204 No Content", &[], "text/plain", b"")
                }
                "/s" => {
                    note(format!("{} y={}", param("e"), param("y")));
                    response("204 No Content", &[], "text/plain", b"")
                }
                "/data.json" => response("200 OK", &[], "application/json", br#"{"probe": [1, 2, 3]}"#),
                "/pixel.png" => response("200 OK", &[], "image/png", &png),
                "/wide" => response("200 OK", &[], "text/html", WIDE.as_bytes()),
                "/tall" => response("200 OK", &[], "text/html", TALL.as_bytes()),
                _ => response("200 OK", &[], "text/html", PAGE.as_bytes()),
            }
        }))?;
        Ok(Site { server, lines, requests })
    }

    /// The requests so far for `target` (path and query, as the browser sent it).
    pub(crate) fn hits(&self, target: &str) -> usize {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).iter().filter(|t| *t == target).count()
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server.base())
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Waits until the page has reported at least `n` lines starting with `prefix`, or `limit` passes.
    pub(crate) fn wait_for(&self, prefix: &str, n: usize, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if self.lines().iter().filter(|l| l.starts_with(prefix)).count() >= n {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(100));
        }
    }
}

/// `manifold.exe` and a fresh work dir, or the exit code to stop with.
pub(crate) fn setup(parent: Option<PathBuf>, name: &str) -> Result<(PathBuf, PathBuf), ExitCode> {
    let exe = manifold_exe().map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(1)
    })?;
    let work = work_dir(parent, name).map_err(|e| {
        eprintln!("work dir: {e}");
        ExitCode::from(1)
    })?;
    Ok((exe, work))
}

pub(crate) fn finish(work: &Path) -> ExitCode {
    if failures() == 0 && std::env::var_os("KEEP").is_none() {
        remove_dir_with_retries(work);
    }
    exit_code()
}

/// The view kinds to run: `VIEWS` (space separated), default `webview2`.
pub(crate) fn views() -> Vec<String> {
    std::env::var("VIEWS").unwrap_or_else(|_| "webview2".into()).split_whitespace().map(str::to_owned).collect()
}

/// scripts' seed step: `PROFILE/settings.json` starting tabs on `view_kind`, and `PROFILE/session.mnf` built from
/// `nodes` with `--import-json`.
pub(crate) fn seed(exe: &Path, profile: &Path, view_kind: &str, nodes: &[Value]) -> io::Result<()> {
    seed_settings(profile, view_kind, "")?;
    import_session(exe, profile, nodes)
}

/// The profile's session becomes a fresh import of `nodes` (any earlier `session.mnf*` is removed first).
pub(crate) fn import_session(exe: &Path, profile: &Path, nodes: &[Value]) -> io::Result<()> {
    for entry in fs::read_dir(profile)?.flatten() {
        if entry.file_name().to_string_lossy().starts_with("session.mnf") {
            fs::remove_file(entry.path())?;
        }
    }
    let seed = profile.join("seed.json");
    fs::write(&seed, json!({ "nodes": nodes }).to_string())?;
    let status = Command::new(exe)
        .arg("--import-json")
        .arg(&seed)
        .arg("--out")
        .arg(profile.join("session.mnf"))
        .stdout(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("--import-json exited with {status}")))
    }
}

/// The id of the Default jar card in the session file `session`, read with `exe --dump`: its stdout is header lines,
/// then the pretty JSON document, which starts at the first line beginning with `{`.
pub(crate) fn default_jar_id(exe: &Path, session: &Path) -> io::Result<String> {
    let out = Command::new(exe).arg("--dump").arg(session).stdout(Stdio::piped()).stderr(Stdio::null()).output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let json = text.lines().skip_while(|l| !l.starts_with('{')).collect::<Vec<_>>().join(
        "
",
    );
    let doc: Value = serde_json::from_str(&json).map_err(io::Error::other)?;
    doc["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|n| n["type"] == JAR && n["is_default"] == true)
        .and_then(|n| n["id"].as_str().map(str::to_owned))
        .ok_or_else(|| io::Error::other(format!("no Default jar in {}", session.display())))
}

/// The Default jar card a seed carries so its id is known before the app runs (`default_jar_id`); a seed without one
/// gets a Default with an id of the app's choosing at startup.
pub(crate) fn default_jar_node() -> Value {
    json!({"id": "d", "type": JAR, "name": "Default", "is_default": true, "pos": [300.0, 0.0]})
}

pub(crate) fn tab_node(url: Option<&str>, view_kind: &str) -> Value {
    let mut node = json!({"id": "t1", "type": TAB, "view_kind": view_kind, "pos": [0.0, 0.0]});
    if let Some(url) = url {
        node["url"] = json!(url);
    }
    node
}

/// Quits the instance and counts a refusal to exit as a failure.
pub(crate) fn quit(inst: &mut Instance, label: &str) {
    if let Err(e) = inst.quit(QUIT_TIMEOUT) {
        println!("FAIL: {label}: {e}");
        note_failure();
    }
}

/// Runs `body`; an I/O error (a timeout, a dead instance) is a failure, not a crash.
pub(crate) fn guarded(label: &str, body: impl FnOnce() -> io::Result<()>) {
    if let Err(e) = body() {
        println!("FAIL: {label}: {e}");
        note_failure();
    }
}

pub(crate) fn page_kind(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "page-kind") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let site = match Site::start() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("site: {e}");
            return ExitCode::from(1);
        }
    };
    // path -> the card it must become (the type; `;loaded` on a live tab).
    let cases =
        [("/data.json", "application/json"), ("/pixel.png", "image/png"), ("/", "application/vnd.manifold.tab;loaded")];
    for view in views() {
        for (path, want) in cases {
            let name = format!("{view}{}", path.replace(['/', '.'], "-"));
            let _t = Section::start(&format!("page-kind-{name}"));
            guarded(&format!("{view} {path}"), || {
                let profile = work.join(format!("appdata-{name}"));
                seed(&exe, &profile, &view, &[tab_node(Some(&site.url(path)), &view)])?;
                let mut inst = Instance::launch(&exe, &profile, &work, &format!("{name}.log"))?;
                inst.send("WAITTABS")?;
                let labels = |s: &State| -> String {
                    s.cards()
                        .iter()
                        .filter_map(|c| c["id"].as_str())
                        .map(|id| s.label(id))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                // The detection runs when the load finishes, after the tab spawns: export until the card is what it
                // should become (the last export, if it never is, fails the check below).
                let state = inst.export_until(&format!("state-{name}"), |s| labels(s) == want)?;
                quit(&mut inst, &name);
                check_eq(&format!("{view} {path} becomes {want}"), want.to_string(), labels(&state));
                Ok(())
            });
        }
    }
    finish(&work)
}

pub(crate) fn hscroll(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "hscroll") {
        Ok(v) => v,
        Err(code) => return code,
    };
    for view in views() {
        let _t = Section::start(&format!("hscroll-{view}"));
        guarded(&format!("hscroll {view}"), || {
            let site = Site::start()?;
            let profile = work.join(format!("appdata-{view}"));
            seed(&exe, &profile, &view, &[tab_node(Some(&site.url("/wide")), &view)])?;
            let mut inst = Instance::launch(&exe, &profile, &work, &format!("{view}.log"))?;
            for line in ["WAITTABS", "WAITIDLE"] {
                inst.send(line)?;
            }
            inst.step(&["GRAPHDOWN", "GRAPHOPEN"], "the /wide page to load", || site.hits("/wide") > 0)?;
            // A wheel event sent before Tab view is drawn goes nowhere.
            inst.export_until(&format!("tabview-{view}"), State::tab_view)?;
            inst.send("SCROLL 300 200 240 0")?;
            // Each scroll reports once it settles; the second must not start before the first has.
            site.wait_for("hscroll", 1, Duration::from_secs(20));
            inst.send("SCROLL 300 200 -120 0")?;
            site.wait_for("hscroll", 2, Duration::from_secs(20));
            quit(&mut inst, &view);
            // The settled positions, in order: after the positive dx, after the negative one.
            let at: Vec<Option<i64>> = site
                .lines()
                .iter()
                .filter_map(|l| l.strip_prefix("hscroll y="))
                .map(|y| y.parse().ok())
                .chain([None, None])
                .take(2)
                .collect();
            let ok = matches!(at[..], [Some(a), Some(b)] if a > 0 && b < a);
            if !check(&format!("hscroll {view}: right then back (after +dx={:?} after -dx={:?})", at[0], at[1]), ok) {
                for l in site.lines() {
                    println!("    page: {l}");
                }
            }
            Ok(())
        });
    }
    finish(&work)
}

/// What the page should log for each key sent, per view kind. CEF sends no DOM `code`, and its modifier keyCode is the
/// side-specific VK_LSHIFT (160) where WebView2 sends the generic VK_SHIFT (16).
fn expected_keys(view: &str) -> Vec<&'static str> {
    if view == "cef" {
        vec![
            "key=Enter kc=13",
            "key=ArrowLeft kc=37",
            "key=Shift kc=160",
            "key=7 kc=55",
            "key=PageDown kc=34",
            "key=a kc=65",
        ]
    } else {
        vec![
            "code=Enter key=Enter kc=13",
            "code=ArrowLeft key=ArrowLeft kc=37",
            "code=ShiftLeft key=Shift kc=16",
            "code=Digit7 key=7 kc=55",
            "code=PageDown key=PageDown kc=34",
            "code=KeyA key=a kc=65",
        ]
    }
}

pub(crate) fn key(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "key") {
        Ok(v) => v,
        Err(code) => return code,
    };
    for view in views() {
        let _t = Section::start(&format!("key-{view}"));
        guarded(&format!("key {view}"), || {
            let site = Site::start()?;
            let profile = work.join(format!("appdata-{view}"));
            seed(&exe, &profile, &view, &[tab_node(Some(&site.url("/")), &view)])?;
            let mut inst = Instance::launch(&exe, &profile, &work, &format!("{view}.log"))?;
            for line in ["WAITTABS", "WAITIDLE"] {
                inst.send(line)?;
            }
            inst.step(&["GRAPHDOWN", "GRAPHOPEN"], "the page to load", || site.hits("/") > 0)?;
            // Each command draws a frame, and the view change needs one.
            inst.export_state(&format!("state-{view}"))?;
            inst.send("CLICK 300 200")?;
            inst.send("WAITIDLE")?;
            for k in ["Enter", "ArrowLeft", "ShiftLeft", "Digit7", "PageDown", "KeyA"] {
                inst.send(&format!("KEYPAGE {k}"))?;
            }
            site.wait_for("", 6, Duration::from_secs(30));
            quit(&mut inst, &view);
            // CEF sends no `code`: drop it from what the page logged.
            let got: Vec<String> =
                site.lines().into_iter().map(|l| if view == "cef" { l.replacen("code= ", "", 1) } else { l }).collect();
            let want = expected_keys(&view);
            if !check(&format!("{view}: {} keys as expected", got.len()), got == want) {
                println!("  the page logged:");
                for l in &got {
                    println!("    {l}");
                }
                println!("  expected:");
                for l in &want {
                    println!("    {l}");
                }
            }
            Ok(())
        });
    }
    finish(&work)
}

/// The host logs (`manifold-<tab>.log`) the case's own TEMP holds.
fn host_logs(tmp: &Path) -> Vec<String> {
    fs::read_dir(tmp)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("manifold-") && e.path().extension().is_some_and(|x| x == "log")
        })
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .collect()
}

/// "<tab card count> <all loaded> <focused_url>" of one export.
fn tab_state(s: &State) -> (usize, bool, String) {
    let tabs: Vec<&Value> = s.cards().into_iter().filter(|c| c["type"] == TAB).collect();
    (tabs.len(), tabs.iter().all(|c| c["loaded"].as_bool() == Some(true)), s.focused_url().to_string())
}

pub(crate) fn bad_url(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "bad-url") {
        Ok(v) => v,
        Err(code) => return code,
    };
    for view in views() {
        // garbage: a stored url that is not one; missing: none (the host gets VIEW_INITIAL_URL="").
        for (name, url) in [("garbage", Some("not a url")), ("missing", None)] {
            let _t = Section::start(&format!("bad-url-{view}-{name}"));
            let label = format!("{view} {name}");
            guarded(&label, || {
                let dir = work.join(format!("{view}-{name}"));
                let tmp = dir.join("tmp");
                fs::create_dir_all(&tmp)?;
                let profile = dir.join("appdata");
                seed(&exe, &profile, &view, &[tab_node(url, &view)])?;
                let tmp_s = tmp.display().to_string();
                let mut inst = Instance::launch_env(&exe, &profile, &dir, "log", &[("TEMP", &tmp_s), ("TMP", &tmp_s)])?;
                // Imported tabs sleep, so GRAPHDOWN (focus the first card) and GRAPHOPEN wake it, once the graph's
                // first layout is done (WAITIDLE).
                for line in ["WAITTABS", "WAITIDLE", "GRAPHDOWN", "GRAPHOPEN"] {
                    inst.send(line)?;
                }
                // Poll until the tab is loaded on about:blank, within STEP_TIMEOUT. A host that panics never gets
                // there: CRASH_LIMIT respawns it up to 3 times, then the tab fails.
                let mut got = (0, false, String::new());
                let mut exports = 0;
                let deadline = Instant::now() + STEP_TIMEOUT;
                while Instant::now() < deadline {
                    exports += 1;
                    match inst.export_state(&format!("s{exports}")) {
                        Ok(s) => got = tab_state(&s),
                        Err(e) => {
                            println!("  export {exports}: {e}");
                            break;
                        }
                    }
                    if got == (1, true, "about:blank".to_string()) {
                        break;
                    }
                    sleep(Duration::from_millis(250));
                }
                quit(&mut inst, &label);
                let logs = host_logs(&tmp);
                check(&format!("{label}: the host wrote a log"), !logs.is_empty());
                check(
                    &format!("{label}: the host fell back to about:blank"),
                    logs.iter().any(|l| l.contains("opening about:blank")),
                );
                check_eq(
                    &format!("{label}: the host did not panic"),
                    0,
                    logs.iter().map(|l| l.matches("panicked").count()).sum(),
                );
                check(&format!("{label}: the tab ends loaded (after {exports} exports)"), got.0 == 1 && got.1);
                check_eq(&format!("{label}: the tab ends on about:blank"), "about:blank".to_string(), got.2.clone());
                if failures() > 0 {
                    println!("--- {label} host log");
                    for l in &logs {
                        println!(
                            "{}",
                            l.lines()
                                .rev()
                                .take(15)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join("\n")
                        );
                    }
                }
                Ok(())
            });
        }
    }
    finish(&work)
}

pub(crate) fn failed_spawn(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "failed-spawn") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let _t = Section::start("failed-spawn");
    guarded("failed-spawn", || {
        // The failure is forced by running a copy of the exe with a view host beside it that is not a program: a
        // sibling host is used as it stands (`view_binary_path`), so the spawn fails. With none beside it the app
        // falls back to the host it extracted into `engine-hosts/`, which `--profile-dir` does not redirect (it is the
        // user's real data dir, `profile::view_data_dir`) and which exists on any machine that has run the app, so the
        // spawn succeeded and the stage proved nothing.
        let bin = work.join("bin");
        fs::create_dir_all(&bin)?;
        let lone = bin.join("manifold.exe");
        fs::copy(&exe, &lone)?;
        fs::write(bin.join("manifold-view-webview2.exe"), "not a program")?;
        let profile = work.join("appdata");
        seed(&lone, &profile, "webview2", &[])?;
        let mut inst = Instance::launch(&lone, &profile, &work, "log")?;
        let snap = |inst: &mut Instance, name: &str| -> io::Result<(usize, String, String)> {
            let s = inst.export_state(name)?;
            let first = s.cards().first().and_then(|c| c["id"].as_str()).map(str::to_owned);
            Ok((
                s.cards().len(),
                first.clone().unwrap_or_else(|| "-".into()),
                first.map_or("-".into(), |id| s.label(&id)),
            ))
        };
        for line in ["NEWTAB", "WAITTABS", "SLEEP 2"] {
            inst.send(line)?;
        }
        let (n1, id1, label1) = snap(&mut inst, "s1")?;
        for line in ["NEWTAB", "WAITTABS", "SLEEP 2"] {
            inst.send(line)?;
        }
        let (n2, id2, _) = snap(&mut inst, "s2")?;
        for line in ["GRAPHRIGHT", "GRAPHOPEN", "SLEEP 2"] {
            inst.send(line)?;
        }
        let (n3, id3, _) = snap(&mut inst, "s3")?;
        quit(&mut inst, "failed-spawn");
        check_eq("a failed startup Root spawn leaves one card", 1, n1);
        check_eq("it is a tab that is not loaded", TAB.to_string(), label1);
        check_eq("a New spawn failing for the same intent stays one card", 1, n2);
        check_eq("and it is the same card", id1.clone(), id2);
        check_eq("Retry keeps the card count", 1, n3);
        check_eq("Retry keeps the card id", id1, id3);
        Ok(())
    });
    finish(&work)
}
