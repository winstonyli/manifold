//! `gate jar` (was scripts/jar-probe.sh and scripts/cookie_server.py): an end-to-end probe of the cookie jar. A WebView2
//! tab logs in, the 30 s pull writes the jar file, and a fresh run's CEF tab, then WebView2 tab, must be authenticated on
//! their first request. The host is a name (`jar.localhost`, which Chromium resolves to loopback; `JAR_PROBE_HOST=127.0.0.1`
//! uses an IP instead), not an IP, so a host-only cookie can be told apart from a domain cookie: after the CEF round trip
//! the jar's sid must still be host-only (no dotted copy).
//!
//! `RUNS` (space separated; default all) picks from:
//!
//! - `login` `fresh` `cef` `webview2`: the login run and the three that follow it (they share state, so a subset of them
//!   is not meaningful)
//! - `twotab`: whether two WebView2 InPrivate tabs (separate host processes) share cookies: tab B, seeded before tab A logs
//!   in, asks `/whoami?b` after A's login. Informational, not pass/fail.
//! - `reuse`, `ten`: WebView2 hosts take turns with a pool of profile names (manifold_0..7). `reuse`: a tab closed and
//!   replaced while another keeps the browser alive (the replacement may reuse its slot); the reused profile must start
//!   empty. `ten`: ten tabs opened and closed in turn; WebView2's Local State must gain no profile names outside the pool.
//! - `perjar`: per-tab jars (spec 2026-10-03-per-tab-jars-design.md): two tabs on the named jars Alice and Bob sign in as
//!   two probe users, and each login must land in its own jar's file with the default jar untouched
//! - `jarswitch`: restarts on perjar's profile (so it needs `perjar` first), moves Alice's tab to Bob with SETJAR and
//!   SWITCHJARNOW, and its next request must be Bob's
//! - `switchnow`: (then `switchclear` on its profile) the replacing host of Switch Jar Now and Clear cookies must not
//!   start with the old jar's sign-ins (they live in the shared browser process, by profile name), and no pull may write
//!   them into a jar. A keeper tab on the default jar, first to start, stays open so the browser outlives each replaced host.
//!
//! The cookie server prints and logs probe usernames and counts only, never cookie values. Uses a throwaway profile (never
//! the real one). Build first (`cargo build`); this runs the existing target/debug exes, the app at below-normal priority.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::check::{check, check_eq, check_is, note_failure, Section};
use crate::http::{self, response};
use crate::instance::{seed_settings, wait_file, Instance};
use crate::probes::{default_jar_id, default_jar_node, finish, guarded, import_session, setup};
use crate::state::{JAR, TAB};
use crate::suggest::{query_param, url_query};

/// A run's commands (stays of 38 s and more) are all processed before its QUIT.
const RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// One request the cookie server saw.
#[derive(Clone)]
struct Req {
    engine: String,
    path: String,
    auth: bool,
    user: Option<String>,
    names: String,
}

impl Req {
    fn user_field(&self) -> String {
        format!("user={}", self.user.as_deref().unwrap_or("-"))
    }
}

type Log = Arc<Mutex<Vec<Req>>>;

fn header<'a>(head: &'a str, name: &str) -> &'a str {
    head.lines()
        .skip(1)
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then_some(v.trim())
        })
        .unwrap_or("")
}

/// The probe user whose session cookie `cookie` carries (`PROBE-SESSION-<user>`, `default` for none).
fn user_of(cookie: &str) -> Option<String> {
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        let rest = value.strip_prefix("PROBE-SESSION")?;
        (name == "sid").then(|| rest.strip_prefix('-').filter(|r| !r.is_empty()).unwrap_or("default").to_owned())
    })
}

/// `/login` sets an HttpOnly session cookie, a SameSite=Strict one and a `__Host-` one (Secure; browsers accept it over
/// http only on localhost names, if at all). `/login?u=X` signs in probe user X (`sid=PROBE-SESSION-X`); `/whoami` answers
/// `AUTH <user>` or `ANON` by whether a session cookie arrived.
fn cookie_server(log: Log) -> io::Result<http::Server> {
    http::Server::start_head(Arc::new(move |line: &str, head: &str| {
        let target = line.split(' ').nth(1).unwrap_or("");
        let path = target.split_once('?').map_or(target, |(p, _)| p);
        let ua = header(head, "user-agent");
        let engine = if ua.contains("Chrome") && !ua.contains("Edg/") { "cef" } else { "webview2" };
        let cookie = header(head, "cookie");
        let user = user_of(cookie);
        let mut names: Vec<&str> = cookie.split(';').filter_map(|p| p.split_once('=')).map(|(k, _)| k.trim()).collect();
        names.sort_unstable();
        let names = if names.is_empty() { "-".to_owned() } else { names.join(",") };
        log.lock().unwrap_or_else(PoisonError::into_inner).push(Req {
            engine: engine.to_owned(),
            path: target.to_owned(),
            auth: user.is_some(),
            user: user.clone(),
            names,
        });
        let mut headers: Vec<(&str, String)> = vec![("Cache-Control", "no-store".to_owned())];
        let body = if path.starts_with("/login") {
            let sid = match query_param(url_query(target), "u", true).filter(|u| !u.is_empty()) {
                Some(u) => format!("sid=PROBE-SESSION-{u}"),
                None => "sid=PROBE-SESSION".to_owned(),
            };
            headers.push(("Set-Cookie", format!("{sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600")));
            headers.push(("Set-Cookie", "pref=dark; Path=/; SameSite=Strict; Max-Age=3600".to_owned()));
            headers
                .push(("Set-Cookie", "__Host-hp=1; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=3600".to_owned()));
            "logged in".to_owned()
        } else if path.starts_with("/whoami") {
            user.map_or("ANON".to_owned(), |u| format!("AUTH {u}"))
        } else {
            String::new()
        };
        let extra: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        response("200 OK", &extra, "text/html", body.as_bytes())
    }))
}

/// DPAPI `CryptUnprotectData` (current user), as the app's jar file uses it.
fn dpapi_unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr().cast_mut() };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    #[allow(unsafe_code, reason = "DPAPI has no safe wrapper here; the blobs outlive the call and the output is freed")]
    // SAFETY: `input` points at `data`, which outlives the call; `out` is filled by the call and freed with `LocalFree`.
    unsafe {
        if CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        ) == 0
        {
            return Err(io::Error::other("DPAPI unprotect failed"));
        }
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData.cast());
        Ok(bytes)
    }
}

/// The jar file's cookies; an error when it is missing, unreadable or not MJR1-protected.
fn load_jar(path: &Path) -> io::Result<Vec<Value>> {
    let raw = fs::read(path)?;
    let blob = raw
        .strip_prefix(b"MJR1")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "NOT PROTECTED (no MJR1 header)"))?;
    let doc: Value = serde_json::from_slice(&dpapi_unprotect(blob)?).map_err(io::Error::other)?;
    Ok(match doc {
        Value::Array(a) => a,
        other => other["cookies"].as_array().cloned().unwrap_or_default(),
    })
}

/// The probe user whose sid the jar holds (`none` without one); never a cookie value.
fn jar_user(path: &Path) -> String {
    let cookies = match load_jar(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return "none(no file)".to_owned(),
        Err(_) => return "unreadable".to_owned(),
    };
    let sids: Vec<&str> = cookies.iter().filter(|c| c["name"] == "sid").filter_map(|c| c["value"].as_str()).collect();
    let users: Vec<String> = sids
        .iter()
        .filter_map(|v| v.strip_prefix("PROBE-SESSION"))
        .map(|r| r.strip_prefix('-').filter(|r| !r.is_empty()).unwrap_or("default").to_owned())
        .collect();
    if !users.is_empty() {
        users.join(",")
    } else if sids.is_empty() {
        "none".to_owned()
    } else {
        "other".to_owned()
    }
}

/// The `jar:` lines the engine hosts logged to `%TEMP%/manifold-<tab id>.log` since `since` (process_manager::
/// per_tab_log_path), as (count, line) sorted by line.
fn host_jar_lines(since: SystemTime) -> Vec<(usize, String)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let fresh = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= since);
        if name.starts_with("manifold-") && name.ends_with(".log") && fresh {
            let text = String::from_utf8_lossy(&fs::read(entry.path()).unwrap_or_default()).into_owned();
            for line in text.lines().filter(|l| l.starts_with("jar:")) {
                *counts.entry(line.chars().take(200).collect()).or_default() += 1;
            }
        }
    }
    counts.into_iter().map(|(l, n)| (n, l)).collect()
}

/// The `manifold_N` profile names in WebView2's Local State, sorted; `None` if unreadable.
fn profile_names(local_state: &Path) -> Option<Vec<String>> {
    let doc: Value = serde_json::from_slice(&fs::read(local_state).ok()?).ok()?;
    let mut names: Vec<String> = doc["profile"]["info_cache"]
        .as_object()?
        .keys()
        .map(|k| k.strip_prefix("WV2Profile_").unwrap_or(k))
        .filter(|k| k.strip_prefix("manifold_").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
        .map(str::to_owned)
        .collect();
    names.sort();
    Some(names)
}

fn strs(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| (*s).to_owned()).collect()
}

struct Probe {
    exe: PathBuf,
    work: PathBuf,
    profile: PathBuf,
    base: String,
    host: String,
    log: Log,
    runs: Vec<String>,
}

impl Probe {
    fn wanted(&self, run: &str) -> bool {
        self.runs.is_empty() || self.runs.iter().any(|r| r == run)
    }

    fn seen(&self) -> usize {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// The requests after the first `n0`.
    fn since(&self, n0: usize) -> Vec<Req> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).iter().skip(n0).cloned().collect()
    }

    fn jars_dir(&self) -> PathBuf {
        self.profile.join("jars")
    }

    fn rm_jars(&self) {
        let _ = fs::remove_dir_all(self.jars_dir());
    }

    /// One headless app run on the profile's session as it stands; the commands run in order, then QUIT. Returns the
    /// hosts' `jar:` log lines, printed as it goes.
    fn start(&self, name: &str, cmds: &[String]) -> io::Result<Vec<(usize, String)>> {
        let mark = SystemTime::now();
        let mut inst = Instance::launch(&self.exe, &self.profile, &self.work, &format!("log-{name}.log"))?;
        println!("    app pid {}, log {}", inst.pid(), inst.log.display());
        for c in cmds {
            inst.send(c)?;
        }
        inst.quit(RUN_TIMEOUT)?;
        let lines = host_jar_lines(mark);
        lines.iter().for_each(|(n, l)| println!("    {n:>5} {l}"));
        Ok(lines)
    }

    /// One headless app run with a single tab; the extra commands run after the tab opens, before the stay.
    fn run(
        &self,
        name: &str,
        engine: &str,
        path: &str,
        stay: u32,
        extra: &[String],
    ) -> io::Result<Vec<(usize, String)>> {
        let tab = json!({"id": "t", "type": TAB, "url": format!("{}{path}", self.base), "view_kind": engine, "pos": [0.0, 0.0]});
        self.import(&[default_jar_node(), tab])?;
        println!("--- run {name} ({engine} {path})");
        let mut cmds = strs(&["WAITTABS", "GRAPHRIGHT", "GRAPHOPEN"]);
        cmds.extend_from_slice(extra);
        cmds.push(format!("SLEEP {stay}"));
        self.start(name, &cmds)
    }

    fn export_session(&self, inst: &mut Instance, name: &str) -> io::Result<Value> {
        let path = self.work.join(format!("{name}.json"));
        let _ = fs::remove_file(&path);
        inst.send(&format!("EXPORTSESSION {}", path.display()))?;
        wait_file(&path, RUN_TIMEOUT)?;
        serde_json::from_slice(&fs::read(&path)?).map_err(io::Error::other)
    }

    /// `start` whose last command exports the session as `export`; returns the exported document.
    fn start_export(&self, name: &str, cmds: &[String], export: &str) -> io::Result<(Vec<(usize, String)>, Value)> {
        let mark = SystemTime::now();
        let mut inst = Instance::launch(&self.exe, &self.profile, &self.work, &format!("log-{name}.log"))?;
        println!("    app pid {}, log {}", inst.pid(), inst.log.display());
        for c in cmds {
            inst.send(c)?;
        }
        let doc = self.export_session(&mut inst, export)?;
        inst.quit(RUN_TIMEOUT)?;
        let lines = host_jar_lines(mark);
        lines.iter().for_each(|(n, l)| println!("    {n:>5} {l}"));
        Ok((lines, doc))
    }

    /// Prints the jar's summary and checks it holds one HttpOnly sid, host-only on the probe host.
    fn jar_summary(&self, path: &Path) {
        let cookies = match load_jar(path) {
            Ok(c) => c,
            Err(e) => {
                println!("jar file: missing or unreadable: {e}");
                check("jar lacks one HttpOnly sid", false);
                return;
            }
        };
        let sid: Vec<&Value> = cookies.iter().filter(|c| c["name"] == "sid").collect();
        let hp: Vec<&Value> = cookies.iter().filter(|c| c["name"] == "__Host-hp").collect();
        let domains =
            |v: &[&Value]| v.iter().map(|c| c["domain"].as_str().unwrap_or("?").to_owned()).collect::<Vec<_>>();
        println!(
            "jar file: protected (MJR1); {} cookies; sid present={} sid domains={:?} __Host-hp domains={:?}",
            cookies.len(),
            !sid.is_empty(),
            domains(&sid),
            domains(&hp)
        );
        check("jar holds one HttpOnly sid", sid.len() == 1 && sid[0]["http_only"].as_bool() == Some(true));
        check_is("sid is exactly one host-only cookie on the probe host", vec![self.host.clone()], domains(&sid));
    }

    /// The Default jar's cookie file in this profile: `<id>.json`, the id read from the session (`default_jar_id`).
    fn default_jar(&self) -> io::Result<PathBuf> {
        let id = default_jar_id(&self.exe, &self.profile.join("session.mnf"))?;
        Ok(self.jars_dir().join(format!("{id}.json")))
    }

    /// `import_session` of `nodes`, which must carry the Default card (`default_jar_node`). Each import makes a new
    /// Default id, so the login a run left in the previous Default's file is carried over: if the new Default has no
    /// file yet, the newest `<node id>.json` in the jars dir is copied to it. (A node id's name has a `-`, so an old
    /// `default.json` is never picked, and asides and temp files do not end in `.json`.)
    fn import(&self, nodes: &[Value]) -> io::Result<()> {
        import_session(&self.exe, &self.profile, nodes)?;
        let file = self.default_jar()?;
        if file.exists() {
            return Ok(());
        }
        let newest = fs::read_dir(self.jars_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".json").is_some_and(|stem| stem.contains('-') && !stem.contains('.'))
            })
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .max();
        if let Some((_, from)) = newest {
            fs::copy(from, file)?;
        }
        Ok(())
    }

    /// login, fresh, cef, webview2: the sign-in reaches the jar file and the next runs start authenticated.
    fn login_runs(&self) -> io::Result<()> {
        self.run("login", "webview2", "/login", 38, &[])?;
        self.jar_summary(&self.default_jar()?);
        // Fresh jar, same user-data folder: this run's tab reuses the login run's profile name and must not be signed in.
        let kept = self.profile.join("jars.kept");
        fs::rename(self.jars_dir(), &kept)?;
        let n0 = self.seen();
        self.run("fresh", "webview2", "/whoami?fresh", 6, &[])?;
        let f = self.since(n0).into_iter().find(|r| r.engine == "webview2" && r.path == "/whoami?fresh");
        println!(
            "fresh jar, reused profile: /whoami?fresh {}",
            f.as_ref().map_or("no request".to_owned(), |r| format!("auth={}", r.auth))
        );
        check_is("a reused profile name carried no cookies across runs", Some(false), f.map(|r| r.auth));
        self.rm_jars();
        fs::rename(&kept, self.jars_dir())?;
        self.run("cef", "cef", "/whoami", 12, &[])?;
        self.jar_summary(&self.default_jar()?);
        self.run("webview2", "webview2", "/whoami", 12, &[])?;
        println!("=== requests (engine path auth user cookie-names)");
        for r in self.since(0) {
            println!("{} {} auth={} {} names={}", r.engine, r.path, r.auth, r.user_field(), r.names);
        }
        for engine in ["cef", "webview2"] {
            let first = self.since(0).into_iter().find(|r| r.engine == engine && r.path == "/whoami");
            println!("{engine}: {}", first.as_ref().map_or("no request".to_owned(), |r| format!("auth={}", r.auth)));
            check_is(&format!("{engine}'s first /whoami was authenticated"), Some(true), first.map(|r| r.auth));
        }
        self.jar_summary(&self.default_jar()?);
        Ok(())
    }

    /// Two WebView2 tabs in one run, from an empty jar: A logs in, then B (NEWTAB opens the home page, in a second host
    /// process seeded with the still-empty jar) asks /whoami?b. Nothing pulls in between (the first 30 s pull ran before A
    /// existed), so B is authenticated only if the two InPrivate tabs share one cookie store.
    fn twotab(&self) -> io::Result<()> {
        self.rm_jars();
        let n0 = self.seen();
        self.run("twotab", "webview2", "/login", 6, &strs(&["SLEEP 4", "NEWTAB", "WAITTABS"]))?;
        println!("=== two-tab requests");
        for r in self.since(n0) {
            println!("{} {} auth={} {} names={}", r.engine, r.path, r.auth, r.user_field(), r.names);
        }
        let b = self.since(0).into_iter().find(|r| r.engine == "webview2" && r.path == "/whoami?b");
        match b.map(|r| r.auth) {
            Some(true) => println!("two-tab: WebView2 InPrivate tabs SHARE cookies across host processes"),
            Some(false) => println!("two-tab: WebView2 InPrivate tabs do NOT share cookies"),
            None => println!("two-tab: inconclusive (tab B made no request)"),
        }
        Ok(())
    }

    /// Slot reuse while the browser lives: tab C (the first host, whose job holds the browser process; started alone first)
    /// stays open; tab A logs in and closes; tab B then takes the lowest free profile name, A's if its folder could be
    /// deleted. A closes well within the first 30 s, so no pull carries A's login into the jar. NEWTAB leaves focus on C,
    /// so NEXTTAB (tabs cycle in creation order) moves it to the new tab before ADDRESSBAR and CLOSETAB. A's slot is freed
    /// only after `remove_cache_dir` gives up on its folder, which C's live browser holds: 50 tries, measured 5.1 to 9.6 s
    /// (2026-10-04), so B waits 14 s.
    fn reuse(&self) -> io::Result<()> {
        self.rm_jars();
        let n0 = self.seen();
        let base = &self.base;
        let extra = [
            "SLEEP 4".to_owned(),
            "NEWTAB".into(),
            "WAITTABS".into(),
            "NEXTTAB".into(),
            format!("ADDRESSBAR {base}/login"),
            "SLEEP 1".into(),
            "ENTER".into(),
            "SLEEP 3".into(),
            format!("ADDRESSBAR {base}/whoami?a"),
            "SLEEP 1".into(),
            "ENTER".into(),
            "SLEEP 3".into(),
            "CLOSETAB".into(),
            "SLEEP 14".into(),
            "NEWTAB".into(),
            "WAITTABS".into(),
            "SLEEP 3".into(),
        ];
        let jar_log = self.run("reuse", "webview2", "/whoami?c", 4, &extra)?;
        let log = self.since(n0);
        let on =
            |p: &str| log.iter().filter(|r| r.engine == "webview2" && r.path == p).map(|r| r.auth).collect::<Vec<_>>();
        let a = on("/whoami?a").first().copied();
        let r = on("/whoami?b").get(1).copied();
        println!("reuse: tab A after its login: {a:?}; tab B's /whoami?b: {r:?}");
        check_is("tab A logged in, so the reuse check proves something", Some(true), a);
        check_is("tab B carried no cookies (a reused profile was empty)", Some(false), r);
        // A and B must share a profile name (C keeps manifold_0, so both get manifold_1), or B's check proves nothing.
        check(
            "tab B reused tab A's profile name manifold_1",
            jar_log.iter().any(|(n, l)| *n == 2 && l == "jar: InPrivate profile manifold_1"),
        );
        Ok(())
    }

    /// Ten WebView2 tabs opened and closed in turn: Local State may gain no profile names outside the pool.
    fn ten(&self) -> io::Result<()> {
        let local_state = self.exe.with_file_name("manifold-engine-webview2.exe.WebView2/EBWebView/Local State");
        let before = profile_names(&local_state).unwrap_or_default();
        // Focus is on C at first and on nothing after a close; NEXTTAB from nothing lands on C (the first tab), so later
        // rounds need two to reach the new tab.
        let mut cycle = strs(&["SLEEP 4"]);
        for i in 1..=10 {
            cycle.extend(strs(&["NEWTAB", "WAITTABS", "NEXTTAB"]));
            if i != 1 {
                cycle.push("NEXTTAB".into());
            }
            cycle.extend([format!("ADDRESSBAR {}/whoami?t{i}", self.base)]);
            cycle.extend(strs(&["SLEEP 1", "ENTER", "SLEEP 2", "CLOSETAB", "SLEEP 6"]));
        }
        self.rm_jars();
        let jar_log = self.run("ten", "webview2", "/whoami?c", 2, &cycle)?;
        let after = profile_names(&local_state).unwrap_or_default();
        let added: Vec<&String> = after.iter().filter(|n| !before.contains(n)).collect();
        let pool =
            after.iter().filter(|n| n.strip_prefix("manifold_").is_some_and(|d| d.len() == 1 && d < "8")).count();
        println!("ten tabs: Local State profile names added this run: {added:?}; pool names present: {pool} of 8");
        let names = jar_log.iter().filter(|(_, l)| l.contains("InPrivate profile")).count();
        let tn = self
            .since(0)
            .iter()
            .filter(|r| {
                r.engine == "webview2"
                    && r.path
                        .strip_prefix("/whoami?t")
                        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
            })
            .count();
        println!("ten tabs: {names} distinct profile names used; {tn} of 10 tabs navigated");
        check_eq("every tab was opened and closed as meant (10 navigated)", 10, tn);
        for p in added {
            check(
                &format!("Local State gained {p} inside the pool"),
                p.strip_prefix("manifold_").is_some_and(|d| d.len() == 1 && d < "8"),
            );
        }
        Ok(())
    }

    fn jar_ids(doc: &Value) -> BTreeMap<String, String> {
        doc["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|n| n["type"] == JAR)
            .filter_map(|n| Some((n["name"].as_str()?.to_owned(), n["id"].as_str()?.to_owned())))
            .collect()
    }

    /// "perjar: alice jar=<..> bob jar=<..> default=<..>" from the exported session and the jar files.
    fn perjar_line(&self, doc: &Value) -> io::Result<String> {
        let ids = Self::jar_ids(doc);
        let user = |name: &str| {
            ids.get(name).map_or("no-jar-node".to_owned(), |id| jar_user(&self.jars_dir().join(format!("{id}.json"))))
        };
        Ok(format!(
            "perjar: alice jar={} bob jar={} default={}",
            user("Alice"),
            user("Bob"),
            jar_user(&self.default_jar()?)
        ))
    }

    fn jar_node(id: &str, name: &str) -> Value {
        json!({"id": id, "type": JAR, "name": name})
    }

    fn jar_tab(&self, id: &str, path: &str, jar: Option<&str>, x: f64) -> Value {
        let mut n = json!({"id": id, "type": TAB, "url": format!("{}{path}", self.base), "view_kind": "webview2", "pos": [x, 0.0]});
        if let Some(j) = jar {
            n["jar"] = j.into();
        }
        n
    }

    /// Per-tab jars: named jars Alice and Bob, a WebView2 tab on each signing in as its own probe user. One 30 s pull (the
    /// 38 s stay) and the quit pull carry each login into its tab's jar file; the default jar must get none. GRAPHOPEN
    /// opens the first tab and NEXTTAB the second, so both are live and both log in.
    fn perjar(&self) -> io::Result<bool> {
        self.rm_jars();
        self.import(&[
            default_jar_node(),
            Self::jar_node("ja", "Alice"),
            Self::jar_node("jb", "Bob"),
            self.jar_tab("a", "/login?u=alice", Some("ja"), 0.0),
            self.jar_tab("b", "/login?u=bob", Some("jb"), 400.0),
        ])?;
        println!("--- run perjar (two WebView2 tabs on jars Alice and Bob)");
        let (_, doc) =
            self.start_export("perjar", &strs(&["WAITTABS", "GRAPHRIGHT", "GRAPHOPEN", "NEXTTAB", "SLEEP 38"]), "s")?;
        for r in self.since(0).iter().filter(|r| r.path.starts_with("/login?u=")) {
            println!("    {} {} auth={} {}", r.engine, r.path, r.auth, r.user_field());
        }
        let line = self.perjar_line(&doc)?;
        println!("{line}");
        let ok = line == "perjar: alice jar=alice bob jar=bob default=none"
            || line == "perjar: alice jar=alice bob jar=bob default=none(no file)";
        check("each login landed in its own tab's jar, and none in the default jar", ok);
        Ok(ok)
    }

    /// Jar switch on a live tab: restart on perjar's profile (both tabs restored, each seeded from its own jar), focus
    /// Alice's tab explicitly (the quick switcher, by its url; restored focus is not relied on), ask /whoami?switch, then
    /// SETJAR Bob and SWITCHJARNOW: the host is replaced by one seeded from Bob's jar, so /whoami?switch2 must be Bob's.
    fn jarswitch(&self) -> io::Result<()> {
        let n0 = self.seen();
        println!("--- run jarswitch (Alice's tab moves to jar Bob)");
        let base = &self.base;
        let cmds = [
            "WAITTABS".to_owned(),
            "QUICKSWITCHOPEN".into(),
            "QUICKSWITCHTEXT u=alice".into(),
            "QUICKSWITCHENTER".into(),
            "WAITTABS".into(),
            "SLEEP 3".into(),
            format!("ADDRESSBAR {base}/whoami?switch"),
            "SLEEP 1".into(),
            "ENTER".into(),
            "SLEEP 3".into(),
            "SETJAR Bob".into(),
            "SWITCHJARNOW".into(),
            "WAITTABS".into(),
            "SLEEP 8".into(),
            format!("ADDRESSBAR {base}/whoami?switch2"),
            "SLEEP 1".into(),
            "ENTER".into(),
            "SLEEP 3".into(),
        ];
        let (_, doc) = self.start_export("jarswitch", &cmds, "s2")?;
        let log = self.since(n0);
        println!("=== jarswitch requests (engine path auth user)");
        log.iter().for_each(|r| println!("    {} {} auth={} {}", r.engine, r.path, r.auth, r.user_field()));
        let users = |p: &str| log.iter().filter(|r| r.path == p).map(Req::user_field).collect::<Vec<_>>();
        let (u1, u2) = (users("/whoami?switch").first().cloned(), users("/whoami?switch2").first().cloned());
        // The switched host reloads /whoami?switch, now as Bob: the tab that asked it as Alice is the one that switched.
        let reload = users("/whoami?switch").get(1).cloned();
        // And the session agrees: Alice's tab (the one not on /login?u=bob) now names jar Bob; Bob's tab still does too.
        let names: BTreeMap<String, String> = Self::jar_ids(&doc).into_iter().map(|(n, i)| (i, n)).collect();
        let tabs: Vec<&Value> = doc["nodes"].as_array().into_iter().flatten().filter(|n| n["type"] == TAB).collect();
        let jar_of = |bob: bool| {
            let v: Vec<String> = tabs
                .iter()
                .filter(|n| n["url"].as_str().unwrap_or("").contains("u=bob") == bob)
                .map(|n| n["jar"].as_str().and_then(|j| names.get(j)).cloned().unwrap_or_else(|| "Default".to_owned()))
                .collect();
            if v.is_empty() {
                "no-tab".to_owned()
            } else {
                v.join(",")
            }
        };
        let jars = format!("alice-tab jar={} bob-tab jar={}", jar_of(false), jar_of(true));
        println!("jarswitch: reload after the switch: {reload:?}; session: {jars}");
        check_is("the switched tab's reload was Bob's", Some("user=bob".to_owned()), reload);
        check_eq("Alice's tab names jar Bob in the session", "alice-tab jar=Bob bob-tab jar=Bob".to_owned(), jars);
        check_is("whoami went alice -> bob", (Some("user=alice".to_owned()), Some("user=bob".to_owned())), (u1, u2));
        Ok(())
    }

    /// The user on the n-th (1-based) request for `path` in `log`, "no request" if there is none.
    fn nth_user(log: &[Req], path: &str, n: usize) -> String {
        log.iter().filter(|r| r.path == path).nth(n - 1).map_or("no request".to_owned(), Req::user_field)
    }

    fn address(&self, path: &str) -> [String; 4] {
        [format!("ADDRESSBAR {}{path}", self.base), "SLEEP 1".into(), "ENTER".into(), "SLEEP 3".into()]
    }

    /// Switch Jar Now and Clear cookies on a live WebView2 tab: the replacing host must not start with the old jar's
    /// sign-ins, and no pull may write them into a jar. switchnow: a tab on jar Alice signs in as alice and moves to the
    /// empty jar Bob with SWITCHJARNOW; its reload (at once) and its next /whoami must be signed out. It then signs in as
    /// bob and stays past a 30 s pull. switchclear restarts on that profile (the tab seeded from Bob's jar, so signed in as
    /// bob), clears Bob (CLEARJAR) and switches now: its reload and next /whoami must be signed out, and after another pull
    /// and the quit pull Bob's jar must hold no sid, Alice's still hers.
    fn switchnow(&self) -> io::Result<()> {
        self.rm_jars();
        let keeper = self.jar_tab("k", "/whoami?keep", None, 0.0);
        self.import(&[
            default_jar_node(),
            Self::jar_node("ja", "Alice"),
            Self::jar_node("jb", "Bob"),
            keeper,
            self.jar_tab("a", "/login?u=alice", Some("ja"), 400.0),
        ])?;
        let n0 = self.seen();
        println!("--- run switchnow (alice's tab moves to the empty jar Bob)");
        let mut cmds = strs(&[
            "WAITTABS",
            "QUICKSWITCHOPEN",
            "QUICKSWITCHTEXT u=alice",
            "QUICKSWITCHENTER",
            "WAITTABS",
            "SLEEP 3",
        ]);
        cmds.extend(self.address("/whoami?sn1"));
        cmds.extend(strs(&["SETJAR Bob", "SWITCHJARNOW", "WAITTABS", "SLEEP 4"]));
        cmds.extend(self.address("/whoami?sn2"));
        cmds.extend(self.address("/login?u=bob"));
        cmds.extend(self.address("/whoami?sn3"));
        cmds.push("SLEEP 30".into());
        let (_, doc) = self.start_export("switchnow", &cmds, "sn")?;
        let log = self.since(n0);
        let (before, reload, next, bob) = (
            Self::nth_user(&log, "/whoami?sn1", 1),
            Self::nth_user(&log, "/whoami?sn1", 2),
            Self::nth_user(&log, "/whoami?sn2", 1),
            Self::nth_user(&log, "/whoami?sn3", 1),
        );
        let jars = self.perjar_line(&doc)?;
        println!("switchnow: before {before}; reload after the switch {reload}; next {next}; after bob's login {bob}; {jars}");
        check_eq("signed in as alice, so the switch proves something", "user=alice".to_owned(), before);
        check_is(
            "the tab moved to the empty jar Bob had no sign-in (reload, next)",
            ("user=-".to_owned(), "user=-".to_owned()),
            (reload, next),
        );
        check_eq("the tab signed in as bob", "user=bob".to_owned(), bob);
        check(
            "each jar holds only its own user's login",
            jars.starts_with("perjar: alice jar=alice bob jar=bob default=none"),
        );

        let n0 = self.seen();
        println!("--- run switchclear (bob's tab, its jar cleared, switches now)");
        let mut cmds =
            strs(&["WAITTABS", "QUICKSWITCHOPEN", "QUICKSWITCHTEXT sn3", "QUICKSWITCHENTER", "WAITTABS", "SLEEP 3"]);
        cmds.extend(self.address("/whoami?sc1"));
        cmds.extend(strs(&["CLEARJAR Bob", "SWITCHJARNOW", "WAITTABS", "SLEEP 4"]));
        cmds.extend(self.address("/whoami?sc2"));
        cmds.push("SLEEP 30".into());
        let (_, doc) = self.start_export("switchclear", &cmds, "sc")?;
        let log = self.since(n0);
        let (before, reload, next) = (
            Self::nth_user(&log, "/whoami?sc1", 1),
            Self::nth_user(&log, "/whoami?sc1", 2),
            Self::nth_user(&log, "/whoami?sc2", 1),
        );
        let jars = self.perjar_line(&doc)?;
        println!("switchclear: before {before}; reload after the clear {reload}; next {next}; {jars}");
        check_eq("signed in as bob, so the clear proves something", "user=bob".to_owned(), before);
        check_is(
            "the tab whose jar was cleared lost its sign-in (reload, next)",
            ("user=-".to_owned(), "user=-".to_owned()),
            (reload, next),
        );
        check(
            "Bob's cleared jar got no sign-in back, and Alice's is unchanged",
            jars.starts_with("perjar: alice jar=alice bob jar=none"),
        );
        Ok(())
    }
}

pub(crate) fn jar(parent: Option<PathBuf>) -> ExitCode {
    let (exe, work) = match setup(parent, "jar") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let log: Log = Arc::default();
    let server = match cookie_server(log.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cookie server: {e}");
            return ExitCode::from(1);
        }
    };
    let host = std::env::var("JAR_PROBE_HOST").unwrap_or_else(|_| "jar.localhost".into());
    let base = format!("http://{host}:{}", server.port());
    let profile = work.join("appdata");
    let cef = exe.with_file_name("cef-runtime").display().to_string().replace('\\', "/");
    // The home page is what NEWTAB opens: tab B of the two-tab run.
    let more = format!(", \"home_page\": \"{base}/whoami?b\", \"cef_directory\": \"{cef}\"");
    if let Err(e) = seed_settings(&profile, "webview2", &more) {
        eprintln!("settings: {e}");
        return ExitCode::from(1);
    }
    let runs = std::env::var("RUNS").unwrap_or_default().split_whitespace().map(str::to_owned).collect();
    let p = Probe { exe, work: work.clone(), profile, base, host, log, runs };
    let section = |name: &str| Section::start(name);
    if p.wanted("login") || p.wanted("fresh") || p.wanted("cef") || p.wanted("webview2") {
        let _t = section("jar-login");
        guarded("login runs", || p.login_runs());
    }
    if p.wanted("twotab") {
        let _t = section("jar-twotab");
        guarded("twotab", || p.twotab());
    }
    if p.wanted("reuse") {
        let _t = section("jar-reuse");
        guarded("reuse", || p.reuse());
    }
    if p.wanted("ten") {
        let _t = section("jar-ten");
        guarded("ten", || p.ten());
    }
    let mut perjar_ok = false;
    if p.wanted("perjar") {
        let _t = section("jar-perjar");
        guarded("perjar", || {
            perjar_ok = p.perjar()?;
            Ok(())
        });
    }
    if p.wanted("jarswitch") {
        let _t = section("jar-jarswitch");
        if perjar_ok {
            guarded("jarswitch", || p.jarswitch());
        } else {
            println!("FAIL: jarswitch needs a passing perjar run on this profile (RUNS=\"perjar jarswitch\")");
            note_failure();
        }
    }
    if p.wanted("switchnow") {
        let _t = section("jar-switchnow");
        guarded("switchnow", || p.switchnow());
    }
    finish(&work)
}
