---
status: done
---

# Sleep with page state Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Idle sleep skips a tab with a dirty form, and any sleep saves the scroll position and restores it on wake.

**Architecture:** A new request/reply pair, `HostToEngine::QueryPageState` / `EngineToHost::PageState`, runs one shared page-side script in each engine. Sleep becomes two-phase on the host: send the query, then sleep (or stay) when the reply arrives. The scroll position is kept in an in-memory `App` map and restored by `HostToEngine::RestoreScroll` after the woken tab's first load finishes.

**Tech Stack:** Rust workspace; `ipc-protocol` (postcard messages); engine hosts `manifold-engine-{webview2,cef,servo,webkit}`; host `manifold`. Probes are bash scripts driving headless `manifold.exe`.

**Spec:** `docs/superpowers/specs/2026-09-30-sleep-page-state-design.md`

## Global Constraints

- Work on `dev`, never `main`; commit per task; end each commit message with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Before each commit: `cargo fmt --all`; `cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings` must pass (CLAUDE.md). Never `--no-verify`.
- Tests must not touch `%APPDATA%\manifold` (CLAUDE.md): no new test reads `profile::dir()`.
- Live-tested on WebView2 and CEF only; Servo and WebKit get real handlers, compiled but not live-tested (spec, "Scope").
- No live-connection (websocket) detection (spec, "Scope").
- Build before probes: `cargo build -p manifold -p manifold-engine-webview2 -p manifold-engine-cef -j 8` (`scripts/fresh-exe.sh` refuses a stale exe). On this machine put `depot_tools` out of `PATH` for CEF builds if CMake fails on `ninja`.
- Probe scripts: use `mktemp -d`, `tail -n +1 -f`, `cygpath -w` for Windows tools, and `python -P file.py < /dev/null` (never `python -`).
- Deviations from the spec, decided while planning: (1) `PAGE_STATE_SCRIPT` returns plain text `"x y d"` (three numbers), not JSON, because `ipc-protocol` has no JSON dependency; (2) the saved scroll lives in `App::saved_scroll`, not in a new `UnloadedTabPayload` field, because that struct is built in about ten places and the spec keeps scroll in memory anyway.

## File structure

- `crates/ipc-protocol/src/messages.rs`, `lib.rs`: `PageState`, `PAGE_STATE_SCRIPT`, `restore_scroll_script`, the three new message variants.
- `crates/manifold-engine-webview2/src/main.rs`, `crates/manifold-engine-cef/src/{main,client,render_process_handler}.rs`, `crates/manifold-engine-servo/src/main.rs`, `crates/manifold-engine-webkit/src/main.rs`: engine handlers.
- `crates/manifold/src/sleep_state.rs` (new): pure types and decisions for two-phase sleep and scroll restore.
- `crates/manifold/src/{main,tabs,event_loop,node_menu,idle_sleep}.rs`: App fields, `request_sleep`, `finish_sleep`, reply and wake handling.
- `scripts/probe_server.py`, `scripts/sleep-probe.sh`, `README.md`, the lazy-tabs spec: probe cases and docs.

---

### Task 1: Protocol types, scripts, and no-op engine arms

**Files:**
- Modify: `crates/ipc-protocol/src/messages.rs`, `crates/ipc-protocol/src/lib.rs`
- Modify (no-op arms only): `crates/manifold-engine-webview2/src/main.rs:300`, `crates/manifold-engine-cef/src/main.rs:264`, `crates/manifold-engine-servo/src/main.rs:282`, `crates/manifold-engine-webkit/src/main.rs:316`

**Interfaces:**
- Produces: `ipc_protocol::PageState { scroll_x: f64, scroll_y: f64, dirty_form: bool }` (`Debug, Clone, PartialEq, Serialize, Deserialize`) with `PageState::parse(text: &str) -> Option<PageState>`; `ipc_protocol::PAGE_STATE_SCRIPT: &str`; `ipc_protocol::restore_scroll_script(x: f64, y: f64) -> String`; `HostToEngine::QueryPageState`, `HostToEngine::RestoreScroll { x: f64, y: f64 }`, `EngineToHost::PageState(PageState)`.

- [ ] **Step 1: Write the failing tests.** Append to `crates/ipc-protocol/src/messages.rs`, inside the existing `page_kind_tests` module's file as a new module:

```rust
#[cfg(test)]
mod page_state_tests {
    use super::*;

    #[test]
    fn page_state_parses_the_scripts_three_numbers() {
        assert_eq!(
            PageState::parse("12.5 340 1"),
            Some(PageState { scroll_x: 12.5, scroll_y: 340.0, dirty_form: true })
        );
        assert_eq!(PageState::parse("0 0 0"), Some(PageState { scroll_x: 0.0, scroll_y: 0.0, dirty_form: false }));
    }

    #[test]
    fn page_state_rejects_anything_else() {
        for bad in ["", "1 2", "a b c", "1 2 3 4", "1 2 2", "NaN 0 0", "inf 0 0"] {
            assert_eq!(PageState::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn restore_scroll_script_embeds_only_finite_numbers() {
        let s = restore_scroll_script(1.5, 2400.0);
        assert!(s.contains("1.5") && s.contains("2400"), "{s}");
        // A non-finite value must not reach the page as JS source.
        assert_eq!(restore_scroll_script(f64::NAN, 5.0), restore_scroll_script(0.0, 5.0));
        assert_eq!(restore_scroll_script(1.0, f64::INFINITY), restore_scroll_script(1.0, 0.0));
    }
}
```

- [ ] **Step 2: Run to verify they fail.** `cargo test -p ipc-protocol page_state` → compile error (`PageState` undefined).

- [ ] **Step 3: Implement.** In `messages.rs`, after `PageKindReply`'s `impl`, add:

```rust
/// Returns `"<scrollX> <scrollY> <1|0>"`: where the page is scrolled and
/// whether any form field differs from its default (the dirty-form check
/// of idle sleep). A text/number input, textarea or select counts when its
/// value differs from its default; a checkbox or radio when `checked` does;
/// a `contenteditable` element when it has any text. Button-like inputs are
/// ignored. Pages that keep state outside form elements are not detected.
pub const PAGE_STATE_SCRIPT: &str = "(function(){var d=false;var els=document.querySelectorAll('input,textarea,select,[contenteditable]');for(var i=0;i<els.length&&!d;i++){var e=els[i];var t=(e.type||'').toLowerCase();if(e.tagName==='INPUT'){if(t==='button'||t==='submit'||t==='reset'||t==='image'||t==='hidden'||t==='file'){continue;}if(t==='checkbox'||t==='radio'){d=e.checked!==e.defaultChecked;}else{d=e.value!==e.defaultValue;}}else if(e.tagName==='TEXTAREA'){d=e.value!==e.defaultValue;}else if(e.tagName==='SELECT'){for(var j=0;j<e.options.length;j++){if(e.options[j].selected!==e.options[j].defaultSelected){d=true;break;}}}else if(e.isContentEditable){d=(e.textContent||'')!=='';}}return (window.scrollX||0)+' '+(window.scrollY||0)+' '+(d?1:0);})();";

/// A tab's page state at the moment of `HostToEngine::QueryPageState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageState {
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub dirty_form: bool,
}

impl PageState {
    /// Parses `PAGE_STATE_SCRIPT`'s result; `None` for any other text.
    pub fn parse(text: &str) -> Option<PageState> {
        let mut parts = text.split_whitespace();
        let scroll_x: f64 = parts.next()?.parse().ok().filter(|v: &f64| v.is_finite())?;
        let scroll_y: f64 = parts.next()?.parse().ok().filter(|v: &f64| v.is_finite())?;
        let dirty_form = match parts.next()? {
            "1" => true,
            "0" => false,
            _ => return None,
        };
        parts.next().is_none().then_some(PageState { scroll_x, scroll_y, dirty_form })
    }
}

/// Scrolls to `(x, y)`, retrying on animation frames for up to 3 s until the
/// page is tall (wide) enough to reach it, then to the nearest reachable
/// point. Gives up at once if the user scrolls first (wheel, key, touch).
/// A non-finite coordinate is replaced by 0, so no `NaN`/`inf` reaches the
/// page as source.
pub fn restore_scroll_script(x: f64, y: f64) -> String {
    let f = |v: f64| if v.is_finite() { v } else { 0.0 };
    format!(
        "(function(x,y){{var stop=false;var end=Date.now()+3000;['wheel','keydown','touchstart'].forEach(function(n){{addEventListener(n,function(){{stop=true;}},{{once:true,passive:true}});}});function go(){{if(stop){{return;}}var de=document.documentElement;var reach=de.scrollHeight-innerHeight>=y&&de.scrollWidth-innerWidth>=x;if(reach||Date.now()>=end){{scrollTo(x,y);return;}}scrollTo(x,y);requestAnimationFrame(go);}}go();}})({},{});",
        f(x),
        f(y)
    )
}
```
Add to `HostToEngine` (after `GetState`):

```rust
    /// Asks for the page's scroll position and dirty-form flag
    /// (`PAGE_STATE_SCRIPT`); the engine answers with `EngineToHost::PageState`.
    /// Sent before a tab sleeps. An engine that cannot answer stays silent
    /// and the host gives up after a few seconds.
    QueryPageState,
    /// Scrolls the page to `(x, y)` once, with retries (`restore_scroll_script`).
    /// Sent after a woken tab's first load.
    RestoreScroll { x: f64, y: f64 },
```
and to `EngineToHost` (after `StateSnapshot`): `PageState(PageState),`. In `lib.rs` add `PageState` and `restore_scroll_script` and `PAGE_STATE_SCRIPT` to the `pub use` of messages (next to `PageKindReply`).

For each engine file, in the `HostToEngine` match add `HostToEngine::QueryPageState | HostToEngine::RestoreScroll { .. } => {}` directly after the `GetState` arm (these are replaced in Tasks 2-3 and 6).

- [ ] **Step 4: Run.** `cargo test -p ipc-protocol` → pass; `cargo build -p manifold -p manifold-engine-webview2 -p manifold-engine-cef -j 8` → ok (Servo and WebKit are checked in Task 6).
- [ ] **Step 5: Commit.** `git add crates && git commit -m "ipc: PageState, QueryPageState, RestoreScroll and their scripts"`.

---

### Task 2: WebView2 handlers

**Files:** Modify `crates/manifold-engine-webview2/src/main.rs` (the `GetState` arm region near line 300, the drain section near line 360).

**Interfaces:** Consumes Task 1's `PAGE_STATE_SCRIPT`, `PageState::parse`, `restore_scroll_script`. Produces: replies `EngineToHost::PageState` to `QueryPageState`; runs the restore script on `RestoreScroll`.

- [ ] **Step 1: Implement.** Near the other shared cells (`pending_json_body`, line ~211) add `let pending_page_state: Rc<RefCell<Option<ipc_protocol::PageState>>> = Rc::new(RefCell::new(None));` (use the same `Rc`/`RefCell` imports the file already has; match how `nav_state::new_shared_json_body` is typed). Replace the no-op arm with:

```rust
                HostToEngine::QueryPageState => {
                    let pending = pending_page_state.clone();
                    let handler = ExecuteScriptCompletedHandler::create(Box::new(move |_error, result| {
                        // `ExecuteScript` returns the string JSON-encoded (quoted).
                        if let Ok(Some(text)) = serde_json::from_str::<Option<String>>(&result.to_string()) {
                            if let Some(state) = ipc_protocol::PageState::parse(&text) {
                                *pending.borrow_mut() = Some(state);
                            }
                        }
                        Ok(())
                    }));
                    let _ = unsafe { webview.ExecuteScript(&HSTRING::from(ipc_protocol::PAGE_STATE_SCRIPT), &handler) };
                }
                HostToEngine::RestoreScroll { x, y } => {
                    let handler = ExecuteScriptCompletedHandler::create(Box::new(|_error, _result| Ok(())));
                    let script = ipc_protocol::restore_scroll_script(x, y);
                    let _ = unsafe { webview.ExecuteScript(&HSTRING::from(script), &handler) };
                }
```
(import `ExecuteScriptCompletedHandler` and `HSTRING` as `nav_state.rs` does). In the drain section next to `pending_json_body` add:

```rust
        if let Some(state) = pending_page_state.borrow_mut().take() {
            let _ = control.send(&EngineToHost::PageState(state));
        }
```
- [ ] **Step 2: Build.** `cargo build -p manifold-engine-webview2 -j 8` → ok.
- [ ] **Step 3: Commit.** `git commit -am "WebView2: answer QueryPageState, run RestoreScroll"`.

(Live check comes with the probe in Task 7; the reply is unobservable from the host until Task 4.)

---

### Task 3: CEF handlers

**Files:** Modify `crates/manifold-engine-cef/src/{render_process_handler,client,main}.rs`.

**Interfaces:** CEF has no script-result API; mirror the audio-playing round trip (`CHECK_AUDIO_PLAYING` / `AUDIO_PLAYING_RESULT`).

- [ ] **Step 1: Renderer side.** In `render_process_handler.rs` add constants `pub const CHECK_PAGE_STATE: &str = "check-page-state";` and `pub const PAGE_STATE_RESULT: &str = "page-state-result";`. In `on_process_message_received`, before the `CHECK_JSON_BODY` test, add:

```rust
            if name == CHECK_PAGE_STATE {
                // Only reply when the eval produced text: silence means "could not tell".
                let text = frame.v8_context().and_then(|context| {
                    context.enter();
                    let text = eval_string(&context, ipc_protocol::PAGE_STATE_SCRIPT);
                    context.exit();
                    text
                });
                if let Some(text) = text {
                    if let Some(mut reply) = process_message_create(Some(&PAGE_STATE_RESULT.into())) {
                        if let Some(args) = reply.argument_list() {
                            args.set_string(0, Some(&text.as_str().into()));
                        }
                        frame.send_process_message(ProcessId::BROWSER, Some(&mut reply));
                    }
                }
                return 1;
            }
```
- [ ] **Step 2: Browser side.** In `client.rs` add `pub type SharedPageState = Rc<RefCell<Option<String>>>;` and `new_shared_page_state()`, a `page_state: SharedPageState` field on `ClientBuilder` (and its constructor function at `client.rs:147`, next to `audio_playing`), and in `on_process_message_received`:

```rust
            if name == PAGE_STATE_RESULT {
                if let Some(args) = message.argument_list() {
                    *self.page_state.borrow_mut() = Some(CefString::from(&args.string(0)).to_string());
                }
                return 1;
            }
```
(import `PAGE_STATE_RESULT`). In `main.rs`: `let page_state = new_shared_page_state();` beside `audio_playing` (line ~157), pass `page_state.clone()` where `audio_playing.clone()` is passed (line ~184), and replace the no-op arm:

```rust
                HostToEngine::QueryPageState => {
                    if let Some(frame) = browser.main_frame() {
                        if let Some(mut message) = process_message_create(Some(&CHECK_PAGE_STATE.into())) {
                            frame.send_process_message(ProcessId::RENDERER, Some(&mut message));
                        }
                    }
                }
                HostToEngine::RestoreScroll { x, y } => {
                    if let Some(frame) = browser.main_frame() {
                        frame.execute_java_script(Some(&ipc_protocol::restore_scroll_script(x, y).as_str().into()), None, 0);
                    }
                }
```
and in the drain section after the `redirect_content_type` block:

```rust
        if let Some(text) = page_state.borrow_mut().take() {
            if let Some(state) = ipc_protocol::PageState::parse(&text) {
                let _ = control.send(&EngineToHost::PageState(state));
            }
        }
```
(import `CHECK_PAGE_STATE`, `new_shared_page_state`).
- [ ] **Step 3: Build.** `cargo build -p manifold-engine-cef -j 8` → ok.
- [ ] **Step 4: Commit.** `git commit -am "CEF: answer QueryPageState over a renderer round trip, run RestoreScroll"`.

---

### Task 4: Host: pure two-phase sleep and scroll-restore logic

**Files:** Create `crates/manifold/src/sleep_state.rs`; modify `crates/manifold/src/main.rs` (`mod sleep_state;` after `mod settings;` in alphabetical order, App fields, `Default`/constructor at the `idle_clock:` line), `crates/manifold/src/idle_sleep.rs`.

**Interfaces:**
- Produces (`sleep_state`): `enum SleepWhy { Manual, Idle }` (`Clone, Copy, PartialEq, Eq, Debug`); `struct PendingSleep { pub why: SleepWhy, pub sent: Instant }`; `const PENDING_TIMEOUT: Duration` (5 s); `enum Verdict { Sleep, Stay }`; `fn verdict(why: SleepWhy, state: &PageState) -> Verdict`; `fn expired(p: &PendingSleep, now: Instant) -> bool`; `struct SavedScroll { pub x: f64, pub y: f64, seen_loading: bool }` with `SavedScroll::new(x, y)` and `fn on_loading(&mut self, loading: bool) -> bool` (true when the restore is due: a load started and has now finished).
- Produces (`IdleClock`): `fn restart(&mut self, id: &str, now: Instant)` setting that tab's last-seen time to `now`.
- Produces (`App` fields): `pending_sleeps: HashMap<String, PendingSleep>`, `saved_scroll: HashMap<String, SavedScroll>`.

- [ ] **Step 1: Failing tests** in the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ipc_protocol::PageState;

    fn state(dirty: bool) -> PageState {
        PageState { scroll_x: 0.0, scroll_y: 10.0, dirty_form: dirty }
    }

    #[test]
    fn only_idle_sleep_stays_awake_for_a_dirty_form() {
        assert_eq!(verdict(SleepWhy::Idle, &state(true)), Verdict::Stay);
        assert_eq!(verdict(SleepWhy::Idle, &state(false)), Verdict::Sleep);
        assert_eq!(verdict(SleepWhy::Manual, &state(true)), Verdict::Sleep);
    }

    #[test]
    fn a_request_expires_after_the_timeout() {
        let t0 = Instant::now();
        let p = PendingSleep { why: SleepWhy::Manual, sent: t0 };
        assert!(!expired(&p, t0 + PENDING_TIMEOUT - Duration::from_millis(1)));
        assert!(expired(&p, t0 + PENDING_TIMEOUT));
    }

    #[test]
    fn restore_is_due_when_a_load_has_started_and_finished() {
        let mut s = SavedScroll::new(0.0, 300.0);
        assert!(!s.on_loading(false), "the engine's initial idle state is not the load");
        assert!(!s.on_loading(true));
        assert!(s.on_loading(false));
    }
}
```
- [ ] **Step 2: Run:** `cargo test -p manifold sleep_state` → compile error.
- [ ] **Step 3: Implement** `sleep_state.rs`:

```rust
//! Pure parts of two-phase sleep (`App::request_sleep` / `finish_sleep`) and
//! of scroll restore after a wake (`App::saved_scroll`).

use ipc_protocol::PageState;
use std::time::{Duration, Instant};

/// Why a tab is being put to sleep: the user asked, or the idle sweep did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SleepWhy {
    Manual,
    Idle,
}

/// A `QueryPageState` sent and not yet answered.
pub(crate) struct PendingSleep {
    pub(crate) why: SleepWhy,
    pub(crate) sent: Instant,
}

/// How long a query may go unanswered (engine busy, dead, or unable to answer).
pub(crate) const PENDING_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Sleep,
    Stay,
}

/// Idle sleep never discards a dirty form; a manual Sleep is an explicit act.
pub(crate) fn verdict(why: SleepWhy, state: &PageState) -> Verdict {
    if why == SleepWhy::Idle && state.dirty_form {
        Verdict::Stay
    } else {
        Verdict::Sleep
    }
}

pub(crate) fn expired(p: &PendingSleep, now: Instant) -> bool {
    now.duration_since(p.sent) >= PENDING_TIMEOUT
}

/// A sleeping tab's scroll position, waiting for its woken page's first load.
pub(crate) struct SavedScroll {
    pub(crate) x: f64,
    pub(crate) y: f64,
    seen_loading: bool,
}

impl SavedScroll {
    pub(crate) fn new(x: f64, y: f64) -> SavedScroll {
        SavedScroll { x, y, seen_loading: false }
    }

    /// Feed each `NavigationStateChanged`'s `loading`; true once a load has
    /// started and finished (a not-yet-started engine reports `false` too).
    pub(crate) fn on_loading(&mut self, loading: bool) -> bool {
        if loading {
            self.seen_loading = true;
            false
        } else {
            self.seen_loading
        }
    }
}
```
In `idle_sleep.rs` add to `impl IdleClock`:

```rust
    /// Restarts `id`'s clock at `now` (a tab that was kept awake, so it is
    /// not asked again every sweep).
    pub(crate) fn restart(&mut self, id: &str, now: Instant) {
        self.seen.insert(id.to_string(), now);
    }
```
with a test `restart_resets_the_clock` (`sweep` at t0, `restart("a", t0+50 s)`, then a sweep at `t0+100 s` with limit 60 s returns empty and one at `t0+110 s` returns `["a"]`). In `main.rs` add the two fields next to `idle_clock` (with a one-line doc each) and initialise them with `HashMap::new()` beside `idle_clock: idle_sleep::IdleClock::default(),`.
- [ ] **Step 4: Run** `cargo test -p manifold -j 8 sleep_state idle_sleep` → pass. **Step 5: Commit** `git add -A crates && git commit -m "sleep_state: pure verdict, timeout and scroll-restore logic"`.

---

### Task 5: Host: two-phase sleep and wake restore wired up

**Files:** Modify `crates/manifold/src/tabs.rs` (`sleep_tab` at ~492, `sweep_idle_tabs`, `sleep_group`, `remove_tab`), `event_loop.rs` (`handle_engine_message`, the `SleepNode` and `IdleSweep` handlers), `node_menu.rs` (~442, 445).

**Interfaces:**
- Consumes Task 4's `SleepWhy`, `PendingSleep`, `verdict`, `expired`, `SavedScroll`, `IdleClock::restart`; Task 1's messages.
- Produces: `App::request_sleep(&mut self, id: &str, why: SleepWhy) -> bool` (true if a query was sent); `App::finish_sleep(&mut self, id: &str, state: PageState)`; `sleep_group` now returns how many were requested; the old `sleep_tab` becomes a private `sleep_now(&mut self, id: &str, scroll: Option<(f64, f64)>) -> bool`.

- [ ] **Step 1: Implement.** In `tabs.rs`:
  - Rename `sleep_tab` to `sleep_now(&mut self, id: &str, scroll: Option<(f64, f64)>)`. Keep every existing check (open tab, split tab, live tab). After the payload swap and texture frees, add `if let Some((x, y)) = scroll { if x != 0.0 || y != 0.0 { self.saved_scroll.insert(id.to_string(), SavedScroll::new(x, y)); } }`.
  - Add:

```rust
    /// First phase of every sleep: validates as `sleep_now` does, asks the
    /// engine for its page state, and notes the request. `finish_sleep`
    /// completes it when `EngineToHost::PageState` arrives; `sweep_idle_tabs`
    /// drops requests nobody answered. Returns whether a query was sent.
    pub(crate) fn request_sleep(&mut self, id: &str, why: SleepWhy) -> bool {
        let open = self.showing_tab_view() && self.focused.as_deref() == Some(id);
        if open || self.split_tab.as_deref() == Some(id) || self.pending_sleeps.contains_key(id) {
            return false;
        }
        let Some(tab) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) else { return false };
        tab.tab.send(&HostToEngine::QueryPageState);
        self.pending_sleeps.insert(id.to_string(), PendingSleep { why, sent: std::time::Instant::now() });
        true
    }

    /// Second phase: the engine's answer. Idle sleep stays awake for a dirty
    /// form (and restarts that tab's idle clock); otherwise the tab sleeps,
    /// keeping its scroll position for the wake.
    pub(crate) fn finish_sleep(&mut self, id: &str, state: PageState) {
        let Some(pending) = self.pending_sleeps.remove(id) else { return };
        match sleep_state::verdict(pending.why, &state) {
            Verdict::Stay => self.idle_clock.restart(id, std::time::Instant::now()),
            Verdict::Sleep => {
                self.sleep_now(id, Some((state.scroll_x, state.scroll_y)));
            }
        }
    }
```
  - `sleep_group`: `members.iter().filter(|m| self.request_sleep(m, SleepWhy::Manual)).count()`.
  - `sweep_idle_tabs`: at its start `let now = Instant::now(); self.pending_sleeps.retain(|_, p| !sleep_state::expired(p, now));`, and `for id in idle { self.request_sleep(&id, SleepWhy::Idle); }`.
  - `remove_tab`: also `self.pending_sleeps.remove(id); self.saved_scroll.remove(id);`.
  - `node_menu.rs` Sleep: `self.request_sleep(id, SleepWhy::Manual);`; `event_loop.rs` `SleepNode` non-group branch: `self.request_sleep(&id, SleepWhy::Manual);`.
  - In `handle_engine_message` add an arm `EngineToHost::PageState(state) => self.finish_sleep(tab_id, state),` and, in the `NavigationStateChanged` arm right after `node.loading = loading;` and before `return url_changed;`:

```rust
                let restore = self.saved_scroll.get_mut(tab_id).is_some_and(|s| s.on_loading(loading));
                if restore {
                    if let Some(s) = self.saved_scroll.remove(tab_id) {
                        if let Some(tab) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
                            tab.tab.send(&HostToEngine::RestoreScroll { x: s.x, y: s.y });
                        }
                    }
                }
```
  (`node` is last used on the line above, so the borrow ends; if the borrow checker objects, move `node.loading = loading` into a scoped block.) Update every doc comment that says `sleep_tab` sleeps at once (`node_menu.rs:22`, `idle_sleep.rs:2`, `settings.rs:120`, `debug_control.rs:307`) to name `request_sleep`.
- [ ] **Step 2: Test the flow.** Add to `tabs.rs`'s existing test module (or `sleep_state.rs` if `App` tests live elsewhere; follow how `session_sync.rs` tests build an `App`: `App::default()` with `NodePayload::Unloaded`/tab fixtures) a test that `finish_sleep` with no pending request does nothing, and that `request_sleep` on an id that is not a live tab returns `false`. The full flow needs a live engine; the probe covers it.
- [ ] **Step 3: Run.** `cargo fmt --all && cargo clippy -p manifold -p ipc-protocol --all-targets -j 8 -- -D warnings && cargo test -p manifold -j 8` → pass.
- [ ] **Step 4: Commit** `git add -A crates && git commit -m "Sleep asks the engine for its page state first; idle sleep keeps dirty forms; a woken tab gets its scroll back"`.

---

### Task 6: Servo and WebKit handlers (compiled, not live-tested)

**Files:** Modify `crates/manifold-engine-servo/src/main.rs` (arm near 282), `crates/manifold-engine-webkit/src/main.rs` (arm near 316).

- [ ] **Step 1: Servo.** Add `let pending_page_state: Rc<RefCell<Option<ipc_protocol::PageState>>> = Rc::new(RefCell::new(None));` near the other shared cells (as `json_body` is made), and replace the no-op arm with (the `webview` binding is the one `notify_load_status_changed` receives; use the one in scope in the command loop, as `HostToEngine::Navigate`'s arm does):

```rust
                HostToEngine::QueryPageState => {
                    let pending = pending_page_state.clone();
                    webview.evaluate_javascript(ipc_protocol::PAGE_STATE_SCRIPT, move |result| {
                        if let Ok(JSValue::String(text)) = result {
                            if let Some(state) = ipc_protocol::PageState::parse(&text) {
                                *pending.borrow_mut() = Some(state);
                            }
                        }
                    });
                }
                HostToEngine::RestoreScroll { x, y } => {
                    webview.evaluate_javascript(ipc_protocol::restore_scroll_script(x, y), |_| {});
                }
```
and next to where `NavigationStateChanged` is sent: `if let Some(state) = pending_page_state.borrow_mut().take() { let _ = control.send(&EngineToHost::PageState(state)); }`. If `evaluate_javascript` takes `impl Into<String>`/`&str`, adapt the `restore_scroll_script` argument the way `PAGE_KIND_SCRIPT` is passed.
- [ ] **Step 2: WebKit.** Same cell near `pending_json_body` (line ~204); arms:

```rust
                HostToEngine::QueryPageState => {
                    let pending = pending_page_state.clone();
                    webview.run_javascript(ipc_protocol::PAGE_STATE_SCRIPT, gtk::gio::Cancellable::NONE, move |result| {
                        if let Ok(js_result) = result {
                            if let Some(value) = js_result.js_value() {
                                if value.is_string() {
                                    if let Some(state) = ipc_protocol::PageState::parse(&value.to_str()) {
                                        *pending.borrow_mut() = Some(state);
                                    }
                                }
                            }
                        }
                    });
                }
                HostToEngine::RestoreScroll { x, y } => {
                    webview.run_javascript(&ipc_protocol::restore_scroll_script(x, y), gtk::gio::Cancellable::NONE, |_| {});
                }
```
and drain next to `pending_json_body` (line ~430): `if let Some(state) = pending_page_state.borrow_mut().take() { let _ = to_host.send(&EngineToHost::PageState(state)); }`.
- [ ] **Step 3: Compile.** Servo: `CARGO_TARGET_DIR=target-servo cargo check -p manifold-engine-servo -j 4` (the target dir already exists; `nice` and `-j 4` per LONG_RUNS). WebKit: in WSL, `CARGO_TARGET_DIR=target-linux nice -n 19 cargo check -p manifold-engine-webkit -j 4`. Both must compile; neither is run.
- [ ] **Step 4: Commit** `git commit -am "Servo, WebKit: QueryPageState and RestoreScroll handlers (compiled, not live-tested)"`.

---

### Task 7: Probe cases, docs, final gate

**Files:** Modify `scripts/probe_server.py`, `scripts/sleep-probe.sh`, `README.md`, `docs/superpowers/specs/2026-09-27-lazy-tabs-design.md`, `docs/superpowers/specs/2026-09-30-sleep-page-state-design.md` (status line, deviations).

- [ ] **Step 1: Probe page.** In `probe_server.py` add a `/tall` page: a `div` 6000 px tall, a text `<input>`, and a script that (a) `fetch('/s?e=load&y='+scrollY)` on load, (b) on `scroll` (debounced 300 ms) `fetch('/s?e=scroll&y='+Math.round(scrollY))`, (c) if the query string has `dirty`, sets `input.value='draft'` on load. Handle `/s` in `do_GET` like `/k`: append `"<e> y=<y>\n"` to the log and answer 204. (Page bytes as a module constant like `PAGE`; route by `url.path == "/tall"`.)
- [ ] **Step 2: Probe cases.** In `sleep-probe.sh` add `CASES` `scroll` and `dirty` (default list `manual idle scroll dirty`), each its own seeded tab and instance as the existing cases.
  - `scroll`: seed url `/tall`; commands `WAITTABS`, `SLEEP 3`, `GRAPHDOWN`, `GRAPHOPEN`, `SLEEP 6`, `CLICK 300 200`, `KEYPAGE PageDown` ×3, `SLEEP 1`, `ESCAPE`, `SLEEP 2`, `SLEEPNODE id`, `SLEEP 2`, `GRAPHOPEN`, `WAITTABS`, `SLEEP 6`, `QUIT`. Pass when the page log's last `scroll y=` before its second `load` line (A) is above 0 and the last `scroll y=` overall (B) satisfies `|A-B| <= 2`.
  - `dirty`: seed url `/tall?dirty=1`; from graph view `IDLESWEEP 9999`, `SLEEP 3`, `IDLESWEEP 2`, `SLEEP 2`, `EXPORTSTATE stay`, `SLEEPNODE id`, `SLEEP 2`, `EXPORTSTATE slept`, `QUIT`. Pass when the cards read `id:Tab` then `id:Unloaded` (the idle sweep left the dirty tab awake; the manual sleep did not).
  Both need the page log passed to `probe_server.py` per case (as `key-probe.sh` does) and read after `wait`.
- [ ] **Step 3: Run.** `cargo build -p manifold -p manifold-engine-webview2 -p manifold-engine-cef -j 8`, then `bash scripts/sleep-probe.sh` → all four cases PASS on WebView2 and CEF. If `scroll` fails with A≠B, check first whether the reply reached the host (`manifold` log) before touching the script.
- [ ] **Step 4: Docs.** README's Sleep paragraph: sleeping keeps the scroll position (lost on app restart), idle sleep skips tabs with a form changed from its default (forms only), waking restores scroll once; list the new probe cases. Lazy-tabs spec: remove the "dirty-form" and "scroll" mentions from the Deferred bullet and describe them under "Sleep". Page-state spec: change "Status" to implemented, note the two deviations from this plan's Global Constraints.
- [ ] **Step 5: Gate.** `cargo fmt --all --check`, clippy (command above), `cargo test -p manifold -p ipc-protocol -j 8`, `bash scripts/headless-smoke-test.sh` (note CPU load with the result).
- [ ] **Step 6: Commit** `git add -A README.md docs scripts && git commit -m "sleep-probe: scroll and dirty-form cases; docs"`.

---

## Self-review

**Spec coverage:** protocol (Task 1); engines WebView2, CEF, Servo, WebKit (Tasks 2, 3, 6); two-phase sleep, 5 s timeout, drop when opened (Tasks 4, 5: `request_sleep` refuses an open tab; `sleep_now` re-checks at `finish_sleep`); manual ignores dirty, idle skips and restarts the clock (Tasks 4, 5); scroll saved and restored once after first load, user-scroll cancels (script in Task 1, edge in Tasks 4, 5); `sleep_group` counts requests (Task 5); tests and probes (Tasks 1, 4, 7); docs (Task 7). Deviations are listed under Global Constraints.

**Placeholders:** none; Servo and WebKit adaptations name the existing call they copy.

**Type consistency:** `PageState`, `SleepWhy`, `PendingSleep`, `SavedScroll`, `request_sleep`, `finish_sleep`, `sleep_now`, `IdleClock::restart` are used with the signatures defined in Tasks 1 and 4.
