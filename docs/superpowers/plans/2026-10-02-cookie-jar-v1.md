# Cookie Jar v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One persistent default cookie jar that manifold owns; WebView2 and CEF tabs start from it and write back to it, so a login carries between engines.

**Architecture:** `jar.rs` (pure) holds the neutral cookie model, merge and file store under `<profile>\jars`. Manifold writes nothing to a host at start: it passes `ENGINE_JAR_FILE` (the jar's path) and the host reads it and seeds the engine before its first navigation (WebView2: InPrivate controller + CDP `Network.setCookies`; CEF: `set_cookie`). Manifold pulls cookies back with `GetCookies` / `Cookies(json)` on a timer and before a tab sleeps, merges, and saves.

**Tech Stack:** Rust, serde_json, ipc-protocol messages, WebView2 (`webview2-com` 0.39), CEF (`cef` 152).

**Spec:** `docs/superpowers/specs/2026-10-02-jars-design.md`. Evidence: branch `spike/cookie-bridge` (code there is throwaway; lift the ideas, not the commits).

## Global Constraints

- Work on `dev`; never `main`. Run `cargo fmt --all` before each commit; `cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings` must pass; never `--no-verify`.
- No test may touch the real `%APPDATA%\manifold`: paths start from `profile::dir()` (None under `cfg(test)`); tests use `store::testutil::temp_dir` and pass the path in. `profile::tests` fails if another source file mentions `"APPDATA"` or `etcetera`.
- Cookies only (spec: "Cookies only (Recommended)"). No localStorage, sessionStorage, IndexedDB.
- Never log cookie values.
- The default jar starts empty; the old WebView2 folder is not imported (spec "Decided").
- Neutral cookie shape: `name, value, domain, path, expires: Option<f64 unix>, secure, http_only, same_site: Strict | Lax | None | Unspecified`.
- Only WebView2 and CEF in v1. The Servo and WebKit hosts only gain no-op match arms.
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

**Scope note (deviation from the spec's Model section):** the per-tab `jar: Option<JarId>` field is not in this plan. With one jar there is nothing to choose, and the field touches about ten files (`pinned` appears in 19 source files, all to be mirrored). It becomes its own plan when a second jar exists. Encryption at rest (DPAPI, spec "Security") is Task 6, last, so the rest can be used and reviewed first.

---

### Task 1: The jar model, merge and file store

**Files:**
- Create: `crates/manifold/src/jar.rs`
- Modify: `crates/manifold/src/main.rs` (add `mod jar;` beside the other `mod` lines)
- Modify: `crates/manifold/src/profile.rs` (add `jars_dir()`)
- Test: inline `#[cfg(test)] mod tests` in `jar.rs`

**Interfaces:**
- Produces: `jar::Cookie`, `jar::SameSite`, `jar::Jar`, `Jar::load(dir: &Path) -> Jar`, `Jar::save(&self, dir: &Path) -> std::io::Result<()>`, `Jar::path(dir: &Path) -> PathBuf`, `Jar::merge(&mut self, pulled: Vec<Cookie>, known: &HashSet<Key>) -> bool` (returns whether anything changed), `Jar::keys(&self) -> HashSet<Key>`, `type Key = (String, String, String)` (name, domain, path), `jar::from_json(&str) -> Option<Vec<Cookie>>`, `profile::jars_dir() -> Option<PathBuf>`.
- Merge rule (spec "Flow" 3): last write wins per `(name, domain, path)`; a cookie in the jar but absent from the pull is deleted only if its key is in `known` (it was present at the previous sync, so its absence is a logout).

- [ ] **Step 1: Write the failing tests** (create `jar.rs` with only the tests and `todo!()`-free stubs omitted; the module does not compile until Step 3)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;

    fn c(name: &str, value: &str) -> Cookie {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: "example.com".into(),
            path: "/".into(),
            expires: None,
            secure: false,
            http_only: true,
            same_site: SameSite::Lax,
        }
    }

    #[test]
    fn a_pulled_cookie_replaces_the_stored_one_with_the_same_key() {
        let mut jar = Jar { cookies: vec![c("sid", "old")] };
        assert!(jar.merge(vec![c("sid", "new")], &jar.keys()));
        assert_eq!(jar.cookies, vec![c("sid", "new")]);
    }

    #[test]
    fn a_cookie_missing_from_the_pull_is_deleted_only_if_it_was_known() {
        let mut jar = Jar { cookies: vec![c("sid", "1"), c("fresh", "2")] };
        let known: HashSet<Key> = [("sid".into(), "example.com".into(), "/".into())].into();
        assert!(jar.merge(vec![], &known));
        assert_eq!(jar.cookies, vec![c("fresh", "2")], "sid was a logout; fresh was never seen by the engine");
    }

    #[test]
    fn merging_the_same_cookies_changes_nothing() {
        let mut jar = Jar { cookies: vec![c("sid", "1")] };
        assert!(!jar.merge(vec![c("sid", "1")], &jar.keys()));
    }

    #[test]
    fn an_expired_cookie_is_dropped_on_merge() {
        let mut gone = c("old", "x");
        gone.expires = Some(1.0);
        let mut jar = Jar::default();
        assert!(!jar.merge(vec![gone], &HashSet::new()), "nothing was added, so nothing changed");
        assert!(jar.cookies.is_empty());
    }

    #[test]
    fn the_jar_round_trips_through_its_file() {
        let dir = temp_dir("jar-roundtrip");
        let jar = Jar { cookies: vec![c("sid", "1")] };
        jar.save(&dir).unwrap();
        assert_eq!(Jar::load(&dir), jar);
    }

    #[test]
    fn a_missing_or_corrupt_file_loads_as_an_empty_jar() {
        let dir = temp_dir("jar-corrupt");
        assert_eq!(Jar::load(&dir), Jar::default());
        std::fs::write(Jar::path(&dir), "not json").unwrap();
        assert_eq!(Jar::load(&dir), Jar::default());
    }

    #[test]
    fn engine_json_parses_into_cookies() {
        let json = r#"[{"name":"a","value":"b","domain":"x.com","path":"/","expires":null,"secure":true,"http_only":false,"same_site":"Strict"}]"#;
        let cookies = from_json(json).unwrap();
        assert_eq!(cookies[0].same_site, SameSite::Strict);
        assert!(from_json("nope").is_none());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p manifold jar:: 2>&1 | tail -5`
Expected: compile error (`Jar`, `Cookie` undefined).

- [ ] **Step 3: Implement** (`jar.rs`, above the tests)

```rust
//! The cookie jar manifold owns. Engines are seeded from it at tab start and pulled back into it, so a login
//! carries between engines (spec docs/superpowers/specs/2026-10-02-jars-design.md). Pure: no egui, no `App`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameSite {
    Strict,
    Lax,
    None,
    Unspecified,
}

/// The neutral cookie every engine's cookies convert to and from. `expires` is Unix seconds, `None` for a session cookie.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

/// `(name, domain, path)`: what identifies a cookie.
pub type Key = (String, String, String);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Jar {
    pub cookies: Vec<Cookie>,
}

impl Cookie {
    fn key(&self) -> Key {
        (self.name.clone(), self.domain.clone(), self.path.clone())
    }

    fn expired(&self, now: f64) -> bool {
        self.expires.is_some_and(|e| e <= now)
    }
}

/// An engine's cookie dump, as its host serialised it. `None` if it is not that shape.
pub fn from_json(json: &str) -> Option<Vec<Cookie>> {
    serde_json::from_str(json).ok()
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl Jar {
    /// `<dir>\default.json`: v1 has one jar.
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("default.json")
    }

    /// The saved jar, or an empty one when the file is missing or unreadable (a corrupt jar must not stop the app).
    pub fn load(dir: &Path) -> Jar {
        std::fs::read_to_string(Self::path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Writes beside the target and renames, so a crash mid-write leaves the old jar.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("default.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, Self::path(dir))
    }

    pub fn keys(&self) -> HashSet<Key> {
        self.cookies.iter().map(Cookie::key).collect()
    }

    /// Folds an engine's dump into the jar (rule in the plan's Task 1). Returns whether the jar changed.
    pub fn merge(&mut self, pulled: Vec<Cookie>, known: &HashSet<Key>) -> bool {
        let before = self.cookies.clone();
        let t = now();
        let pulled_keys: HashSet<Key> = pulled.iter().map(Cookie::key).collect();
        self.cookies.retain(|c| pulled_keys.contains(&c.key()) || !known.contains(&c.key()));
        for p in pulled.into_iter().filter(|p| !p.expired(t)) {
            match self.cookies.iter_mut().find(|c| c.key() == p.key()) {
                Some(slot) => *slot = p,
                None => self.cookies.push(p),
            }
        }
        self.cookies.retain(|c| !c.expired(t));
        self.cookies != before
    }
}
```

In `profile.rs`, after `content_dir`:

```rust
/// Where cookie jars live (`jar::Jar`). `None` like `dir()`.
pub fn jars_dir() -> Option<PathBuf> {
    Some(dir()?.join("jars"))
}
```

In `main.rs` add `mod jar;` in alphabetical position among the `mod` lines.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p manifold jar:: 2>&1 | tail -5`
Expected: 7 passed.

- [ ] **Step 5: Gates and commit**

```bash
cargo fmt --all && cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings 2>&1 | tail -3
git add crates/manifold/src/jar.rs crates/manifold/src/main.rs crates/manifold/src/profile.rs
git commit -m "Jar: neutral cookie model, merge and file store" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: IPC messages

**Files:**
- Modify: `crates/ipc-protocol/src/messages.rs` (after `QueryPageState`, line ~233; and after `PageState(PageState)`, line ~387)
- Modify: `crates/manifold-engine-servo/src/main.rs` (match at ~293), `crates/manifold-engine-webkit/src/main.rs` (~339): add no-op arms. These hosts are not built here; the arms are checked by reading only.
- Test: `crates/ipc-protocol` existing round-trip tests (find with `grep -n "QueryPageState" crates/ipc-protocol/src/*.rs`); add one for the new variants.

**Interfaces:**
- Produces: `HostToEngine::GetCookies` (answer: `EngineToHost::Cookies(String)`, a JSON array of `jar::Cookie` shape). Engines that cannot answer stay silent.

- [ ] **Step 1: Write the failing test** next to the existing message round-trip tests

```rust
#[test]
fn the_cookie_messages_round_trip() {
    let bytes = serde_json::to_vec(&HostToEngine::GetCookies).unwrap();
    assert!(matches!(serde_json::from_slice::<HostToEngine>(&bytes).unwrap(), HostToEngine::GetCookies));
    let bytes = serde_json::to_vec(&EngineToHost::Cookies("[]".into())).unwrap();
    assert!(matches!(serde_json::from_slice::<EngineToHost>(&bytes).unwrap(), EngineToHost::Cookies(s) if s == "[]"));
}
```

If the existing tests use the crate's own `transport` encoding rather than serde_json, mirror that encoding instead (read the neighbouring test first).

- [ ] **Step 2: Run to verify it fails**: `cargo test -p ipc-protocol 2>&1 | tail -5`; expected: compile error.

- [ ] **Step 3: Implement.** In `HostToEngine` after `QueryPageState,`:

```rust
    /// Asks for every cookie in the engine's jar; the engine answers with `EngineToHost::Cookies`.
    /// An engine that cannot answer stays silent.
    GetCookies,
```

In `EngineToHost` after `PageState(PageState),`:

```rust
    /// The engine's whole cookie jar as a JSON array of `jar::Cookie` (name, value, domain, path, expires,
    /// secure, http_only, same_site), the answer to `HostToEngine::GetCookies`.
    Cookies(String),
```

Add `HostToEngine::GetCookies => {}` to the Servo and WebKit hosts' match arms.

- [ ] **Step 4: Run to verify it passes and the workspace builds:** `cargo test -p ipc-protocol 2>&1 | tail -3 && cargo build -p manifold -p manifold-engine-webview2 -p manifold-engine-cef 2>&1 | grep -E "^error|Finished"`. Expected: tests pass; the build fails only in webview2/cef non-exhaustive matches (fixed in Tasks 3 and 4). To keep each commit green, add `HostToEngine::GetCookies => {}` to the webview2 and CEF hosts here too, and replace them in Tasks 3 and 4.

- [ ] **Step 5: Commit**: `git add -A crates && git commit -m "IPC: GetCookies and Cookies messages" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"`

---

### Task 3: WebView2 host: InPrivate, seed, pull

**Files:**
- Modify: `crates/manifold-engine-webview2/src/main.rs`
- Test: manual probe (below); there is no unit seam in the host. Keep the pure conversion in a testable function.
- Create: `crates/manifold-engine-webview2/src/jar_cdp.rs` (pure conversions, unit-tested)

**Interfaces:**
- Consumes: `ENGINE_JAR_FILE` env (path of the jar file, may not exist); `HostToEngine::GetCookies`; `EngineToHost::Cookies(String)`.
- Produces: `jar_cdp::cdp_cookies_from_jar(json: &str) -> serde_json::Value` (params for `Network.setCookies`), `jar_cdp::jar_from_cdp(result: &serde_json::Value) -> String` (neutral JSON from `Network.getAllCookies`). Both `jar.rs`'s shape; the host does not depend on the manifold crate, so the shape is duplicated in these two functions and their tests.

- [ ] **Step 1: Write the failing tests** in `jar_cdp.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_jar_becomes_cdp_set_cookies_params() {
        let jar = r#"{"cookies":[{"name":"sid","value":"1","domain":"a.com","path":"/","expires":4102444800.0,"secure":true,"http_only":true,"same_site":"Lax"},
                                  {"name":"s","value":"2","domain":"a.com","path":"/","expires":null,"secure":false,"http_only":false,"same_site":"Unspecified"}]}"#;
        let p = cdp_cookies_from_jar(jar);
        assert_eq!(p["cookies"][0]["httpOnly"], true);
        assert_eq!(p["cookies"][0]["sameSite"], "Lax");
        assert_eq!(p["cookies"][0]["expires"], 4102444800.0);
        assert!(p["cookies"][1].get("expires").is_none(), "a session cookie has no expiry");
        assert!(p["cookies"][1].get("sameSite").is_none(), "Unspecified is left to the engine");
    }

    #[test]
    fn cdp_cookies_become_neutral_json() {
        let r = json!({"cookies":[{"name":"sid","value":"1","domain":".a.com","path":"/","session":false,"expires":99.0,
                                   "secure":true,"httpOnly":true,"sameSite":"Strict"},
                                  {"name":"t","value":"2","domain":"b.com","path":"/","session":true,"expires":-1,"secure":false,"httpOnly":false}]});
        let out: serde_json::Value = serde_json::from_str(&jar_from_cdp(&r)).unwrap();
        assert_eq!(out[0]["http_only"], true);
        assert_eq!(out[0]["same_site"], "Strict");
        assert_eq!(out[1]["expires"], serde_json::Value::Null);
        assert_eq!(out[1]["same_site"], "Unspecified");
    }

    #[test]
    fn an_unreadable_jar_seeds_nothing() {
        assert_eq!(cdp_cookies_from_jar("garbage")["cookies"], json!([]));
    }
}
```

- [ ] **Step 2: Run to verify it fails**: `cargo test -p manifold-engine-webview2 jar_cdp 2>&1 | tail -4`; expected: compile error.

- [ ] **Step 3: Implement.** `jar_cdp.rs`:

```rust
//! Conversions between manifold's jar file and the DevTools `Network` cookie shapes.

use serde_json::{json, Value};

/// `Network.setCookies` params from the jar file's contents (`{"cookies":[...]}`). Unreadable input seeds nothing.
pub fn cdp_cookies_from_jar(jar_json: &str) -> Value {
    let list = serde_json::from_str::<Value>(jar_json).ok().and_then(|v| v["cookies"].as_array().cloned()).unwrap_or_default();
    let cookies: Vec<Value> = list
        .iter()
        .map(|c| {
            let mut o = json!({
                "name": c["name"], "value": c["value"], "domain": c["domain"], "path": c["path"],
                "secure": c["secure"], "httpOnly": c["http_only"],
            });
            match c["same_site"].as_str() {
                Some(s @ ("Strict" | "Lax" | "None")) => o["sameSite"] = s.into(),
                _ => {}
            }
            if !c["expires"].is_null() {
                o["expires"] = c["expires"].clone();
            }
            o
        })
        .collect();
    json!({ "cookies": cookies })
}

/// The neutral JSON array (`jar::Cookie` shape) from a `Network.getAllCookies` result.
pub fn jar_from_cdp(result: &Value) -> String {
    let list = result["cookies"].as_array().cloned().unwrap_or_default();
    let neutral: Vec<Value> = list
        .iter()
        .map(|c| {
            json!({
                "name": c["name"], "value": c["value"], "domain": c["domain"], "path": c["path"],
                "expires": if c["session"].as_bool() == Some(true) { Value::Null } else { c["expires"].clone() },
                "secure": c["secure"], "http_only": c["httpOnly"],
                "same_site": c["sameSite"].as_str().unwrap_or("Unspecified"),
            })
        })
        .collect();
    Value::Array(neutral).to_string()
}
```

In `main.rs` (`mod jar_cdp;` with the other `mod` lines), port these pieces from `spike/cookie-bridge` (`git show spike/cookie-bridge:crates/manifold-engine-webview2/src/main.rs`):
1. In `create_composition_controller`: the `in_private` branch, but unconditional, and when the `cast::<ICoreWebView2Environment10>()` fails fall back to `environment3.CreateCoreWebView2CompositionController` and `eprintln!("jar: InPrivate unavailable, using the shared profile")` (spec: "fall back"). Seeding still happens in the fallback, and its pulled cookies then include the shared profile's; Task 5 notes this.
2. Before `webview.Navigate(initial_url)`: if `ENGINE_JAR_FILE` is set and readable, `Network.setCookies` with `jar_cdp::cdp_cookies_from_jar(&contents)`, awaited with `CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation` (the spike's `spike_seed_cookies`, minus the env var and the neutral conversion).
3. `HostToEngine::GetCookies`: the spike's `SpikeGetCookies` arm and `pending_cookies` slot, with the handler calling `jar_from_cdp(&parsed_result)` (no domain filter, no logging of values) and the poll loop sending `EngineToHost::Cookies(json)`.

- [ ] **Step 4: Run tests and probe**

Run: `cargo test -p manifold-engine-webview2 jar_cdp 2>&1 | tail -3` (3 passed), then `cargo build -p manifold-engine-webview2`.
Manual probe: adapt `scripts/spike/inprivate-seed.sh` from the spike branch (`git show spike/cookie-bridge:scripts/spike/inprivate-seed.sh`) to write a jar file, set `ENGINE_JAR_FILE`, and expect the server log to show the first request carrying the cookie. Record the result in the commit message.

- [ ] **Step 5: Commit**: `cargo fmt --all && git add -A crates && git commit -m "WebView2 host: InPrivate tab seeded from the jar, GetCookies" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"`

---

### Task 4: CEF host: seed and pull

**Files:**
- Modify: `crates/manifold-engine-cef/src/main.rs`, `crates/manifold-engine-cef/Cargo.toml` (`serde_json = "1"`)
- Create: `crates/manifold-engine-cef/src/jar_cookies.rs`

**Interfaces:**
- Consumes: `ENGINE_JAR_FILE`; `HostToEngine::GetCookies`.
- Produces: `jar_cookies::seed(jar_json: &str)` (writes cookies via the global manager), `jar_cookies::JarPull` (shared result slot), `jar_cookies::start_pull() -> JarPull`, `JarPull::take(&self) -> Option<String>`, `cookie_to_json(&Cookie) -> serde_json::Value` and `cookie_from_json(&serde_json::Value) -> Option<(String, Cookie)>` (url, cookie), the two pure conversions, unit-tested.

- [ ] **Step 1: Write the failing tests** for the pure conversions (Basetime math and the SameSite mapping)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unix_seconds_round_trip_through_cef_basetime() {
        let t = to_basetime(1_700_000_000.0);
        assert!((from_basetime(&t) - 1_700_000_000.0).abs() < 0.001);
    }

    #[test]
    fn a_neutral_cookie_becomes_a_cef_cookie_and_back() {
        let j = json!({"name":"sid","value":"1","domain":"a.com","path":"/","expires":4102444800.0,"secure":true,"http_only":true,"same_site":"Strict"});
        let (url, cookie) = cookie_from_json(&j).unwrap();
        assert_eq!(url, "https://a.com/");
        assert_eq!(cookie.httponly, 1);
        let back = cookie_to_json(&cookie);
        assert_eq!(back["same_site"], "Strict");
        assert_eq!(back["http_only"], true);
        assert!((back["expires"].as_f64().unwrap() - 4102444800.0).abs() < 0.01);
    }

    #[test]
    fn a_session_cookie_has_no_expiry() {
        let j = json!({"name":"s","value":"1","domain":".a.com","path":"/","expires":null,"secure":false,"http_only":false,"same_site":"Unspecified"});
        let (url, cookie) = cookie_from_json(&j).unwrap();
        assert_eq!(url, "http://a.com/");
        assert_eq!(cookie_to_json(&cookie)["expires"], serde_json::Value::Null);
    }
}
```

- [ ] **Step 2: Run to verify it fails**: `cargo test -p manifold-engine-cef jar_cookies 2>&1 | tail -4`. The CEF crate's test binary may need the CEF runtime DLLs next to it; if it will not start, run the pure functions' tests with `cargo test -p manifold-engine-cef --lib` or move the conversions behind `#[cfg(test)]`-free code in a crate that links no CEF (record which in the commit). Expected now: compile error.

- [ ] **Step 3: Implement.** Port from `spike/cookie-bridge` (`git show spike/cookie-bridge:crates/manifold-engine-cef/src/main.rs`, the `SpikeSetCookies` arm) into `cookie_from_json` (Basetime = microseconds since 1601; `url` = `http[s]://<domain without leading dot>/`; SameSite mapping; `..Default::default()`), and write the inverse:

```rust
fn to_basetime(unix: f64) -> Basetime {
    Basetime { val: ((unix + 11_644_473_600.0) * 1_000_000.0) as i64 }
}

fn from_basetime(t: &Basetime) -> f64 {
    t.val as f64 / 1_000_000.0 - 11_644_473_600.0
}
```

`cookie_to_json` reads `name/value/domain/path` via `CefString::to_string()` (check the exact accessor in `cef-152.1.0/src/string.rs`), `secure`/`httponly` as `!= 0`, `has_expires` to choose `expires`, and `same_site` by comparing with the `CookieSameSite::*` constants (the spike used `==`-free `match`; if the type is not `PartialEq`, compare `.0`... verify against the bindings before choosing).

`seed(jar_json)`: for each cookie in `{"cookies":[..]}` call `cookie_manager_get_global_manager(None)?.set_cookie(Some(&url.as_str().into()), Some(&cookie), None)`. Call it in `main` after CEF initialises and before `browser_host_create_browser_sync` (main.rs:180), reading `ENGINE_JAR_FILE`.

`start_pull`: `visit_all_cookies` takes a `CookieVisitor` made with `wrap_cookie_visitor!` (same pattern as `find_handler.rs`). The visitor pushes `cookie_to_json(cookie)` into a shared `Rc<RefCell<Vec<Value>>>` and, when `count + 1 == total`, sets the finished slot to the JSON array string. **Known gap:** with zero cookies CEF never calls the visitor, so a pull that returns no callback within 500 ms answers `"[]"`. `JarPull` records the start `Instant` for that.

In the message loop, replace the Task 2 no-op: `HostToEngine::GetCookies => { pull = Some(jar_cookies::start_pull()); }`, and each tick: `if let Some(p) = &pull { if let Some(json) = p.take() { control.send(&EngineToHost::Cookies(json)); pull = None; } }`.

- [ ] **Step 4: Run tests and probe**: pure tests pass; build with `cargo build -p manifold-engine-cef`. Probe with the spike's `run_spike2.sh` shape: the CEF tab's first poll after startup carries the seeded cookie because the jar file was written beforehand; then `GetCookies` returns it back.

- [ ] **Step 5: Commit**: `cargo fmt --all && git add -A crates && git commit -m "CEF host: seed from the jar file, GetCookies" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"`

---

### Task 5: Manifold wiring: seed path, pull timer, merge

**Files:**
- Modify: `crates/manifold/src/process_manager.rs` (`spawn_tab`, ~line 950: `.env("ENGINE_JAR_FILE", path)` for `WebView2` and `Cef`)
- Modify: `crates/manifold/src/main.rs` (`App`: `jar: jar::Jar`, `jar_known: HashSet<jar::Key>`, `jar_last_pull: Option<Instant>`; `App::default` loads the jar from `profile::jars_dir()`)
- Modify: `crates/manifold/src/event_loop.rs` (`handle_engine_message`: `EngineToHost::Cookies(json)` arm; `about_to_wait`: the 30 s pull)
- Modify: `crates/manifold/src/tabs.rs` (before `QueryPageState` at ~553: pull first)
- Test: a unit test on `App` using `App::default()`; `jars_dir()` is `None` under test so saving is skipped (assert on `app.jar`).

**Interfaces:**
- Consumes: Task 1 `Jar`, `from_json`; Task 2 messages.
- Produces: `App::jar_pull_all(&mut self)` (sends `GetCookies` to every live WebView2/CEF tab), `App::apply_pulled_cookies(&mut self, json: &str) -> bool`.

- [ ] **Step 1: Write the failing test** (in `main.rs`'s or `event_loop.rs`'s existing `tests` module; mirror an existing `App::default()` test)

```rust
#[test]
fn pulled_cookies_reach_the_jar_and_a_logout_removes_them() {
    let mut app = App::default();
    let sid = r#"[{"name":"sid","value":"1","domain":"a.com","path":"/","expires":null,"secure":false,"http_only":true,"same_site":"Lax"}]"#;
    assert!(app.apply_pulled_cookies(sid));
    assert_eq!(app.jar.cookies.len(), 1);
    assert!(app.apply_pulled_cookies("[]"), "sid was known, so its absence is a logout");
    assert!(app.jar.cookies.is_empty());
    assert!(!app.apply_pulled_cookies("not json"), "a garbled pull changes nothing");
}
```

- [ ] **Step 2: Run to verify it fails**: `cargo test -p manifold pulled_cookies 2>&1 | tail -4`; expected: compile error.

- [ ] **Step 3: Implement**

```rust
impl App {
    /// Folds one engine's cookie dump into the jar and saves it when it changed. `false` for a dump that does not parse.
    pub(crate) fn apply_pulled_cookies(&mut self, json: &str) -> bool {
        let Some(pulled) = crate::jar::from_json(json) else { return false };
        let changed = self.jar.merge(pulled, &self.jar_known);
        self.jar_known = self.jar.keys();
        if changed {
            if let Some(dir) = crate::profile::jars_dir() {
                if let Err(e) = self.jar.save(&dir) {
                    eprintln!("jar: could not save: {e}");
                }
            }
        }
        changed
    }

    /// Asks every live WebView2 and CEF tab for its cookies; answers arrive as `EngineToHost::Cookies`.
    pub(crate) fn jar_pull_all(&mut self) {
        for node in self.nodes.values_mut().filter_map(|n| n.as_tab_mut()) {
            if matches!(node.tab.kind, EngineKind::WebView2 | EngineKind::Cef) {
                node.tab.send(&HostToEngine::GetCookies);
            }
        }
    }
}
```

`handle_engine_message`: `EngineToHost::Cookies(json) => { self.apply_pulled_cookies(&json); }`. In `about_to_wait`, after the existing per-frame work: `if self.jar_last_pull.map_or(true, |t| t.elapsed() >= JAR_PULL_EVERY) { self.jar_pull_all(); self.jar_last_pull = Some(Instant::now()); }` with `const JAR_PULL_EVERY: Duration = Duration::from_secs(30);`. In `tabs.rs` immediately before `tab.tab.send(&HostToEngine::QueryPageState)`: `self.jar_pull_all();`-equivalent for that one tab (`tab.tab.send(&HostToEngine::GetCookies)`) if its kind is WebView2 or Cef.

`spawn_tab` has no jar argument. Add a free function `fn jar_env(kind: EngineKind) -> Option<PathBuf>` in `process_manager.rs`: `matches!(kind, WebView2 | Cef)` then `profile::jars_dir().map(|d| jar::Jar::path(&d))`; and `if let Some(p) = jar_env(kind) { command.env("ENGINE_JAR_FILE", p); }` after the `ENGINE_INITIAL_URL` env. `App` must have saved the jar to disk before a tab spawns; it does, because Task 5's save runs on every change and `App::default` loads it.

**Known limits, recorded in the spec's Open questions at the end of this task:** a tab closed within 30 s of its last change loses that change (no pull after a host is gone); a WebView2 host that fell back to the shared profile reports its 20-odd real cookies, which this task's merge would then copy into the jar. Guard that: in fallback mode the host prints `jar: InPrivate unavailable` and does not answer `GetCookies` at all (an empty answer would read as a logout for every known cookie), so the jar never absorbs the shared profile.

- [ ] **Step 4: Run the tests**: `cargo test -p manifold 2>&1 | grep -E "^test result|FAILED"` (all pass); `cargo build -p manifold -p manifold-engine-webview2 -p manifold-engine-cef`.

- [ ] **Step 5: End-to-end probe and commit.** Use the spike's `run_spike2.sh` shape with real engines: log in on a WebView2 tab at the local cookie server, wait 35 s, check `<profile>\jars\default.json` holds `sid` with `http_only: true`; quit; start with a CEF tab at `/whoami`; expect its first request to be authenticated. Add the script as `scripts/jar-probe.sh` (derived from `scripts/spike/cookie-bridge.sh`, no spike env vars). Commit: `git add -A && git commit -m "Jar: seed from the jar file, pull cookies back every 30 s and before sleep" -m "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"`.

---

### Task 6: Protect the jar at rest

**Files:**
- Modify: `crates/manifold/src/jar.rs` (`save`/`load` through a `protect`/`unprotect` pair), `crates/manifold/Cargo.toml` (`windows` feature `Win32_Security_Cryptography`, or `windows-sys` equivalent already present: check `Cargo.toml` lines 87-110 first).
- Modify: hosts read the jar file at start, so they cannot read an encrypted file. **Design decision needed before starting this task:** either (a) manifold decrypts and passes the seed over the existing IPC (a new `SetCookies` message sent before the host's first `Navigate`, which means the host waits for it), or (b) manifold writes a short-lived plaintext copy that it deletes after the host connects. Neither is in the spec; the spec says only "protect at rest with the OS store". Stop here and ask the user which.

- [ ] **Step 1:** Ask the user: (a) IPC seeding, or (b) a short-lived plaintext file.
- [ ] **Step 2 onward:** written once the choice is made (test: a saved jar is not readable as JSON, and round-trips through `load`; Windows DPAPI `CryptProtectData`/`CryptUnprotectData` behind `cfg(windows)`, a plaintext no-op elsewhere with a stated comment).

---

## Self-review

- **Spec coverage:** model and store (T1); flow steps 1 and 2 (T3-T5); merge rule (T1); engine mapping for WebView2 and CEF (T3, T4); WebView2 InPrivate and fallback (T3); security at rest (T6, pending the decision); default jar empty (T1 `load` of a missing file); out of scope items untouched. Not covered by design: the per-tab `jar` field and jar nodes (Scope note), Servo and WebKit (Global Constraints), sync on engine swap (the 30 s timer and sleep pull cover swap's data except for the last 30 s; add an explicit pull in the swap path when that code is next touched).
- **Placeholder scan:** Task 3 step 3 and Task 4 step 3 point at the spike branch for ported code instead of repeating all of it, and Task 4's CEF visitor body and `cookie_to_json` are described, not written out. These are the plan's weakest steps; the executor must read the named spike file and `find_handler.rs` first. Task 6 is intentionally a stub pending a user decision.
- **Type consistency:** `Key`, `Jar::merge(pulled, &known)`, `from_json`, `GetCookies`, `Cookies(String)`, `ENGINE_JAR_FILE`, `jar_cdp::{cdp_cookies_from_jar, jar_from_cdp}`, `jar_cookies::{seed, start_pull, JarPull}` are used consistently across tasks.
