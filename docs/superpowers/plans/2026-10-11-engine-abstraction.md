---
status: done
---

# Engine abstraction (survey E4, E8) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline) for this plan: it has one required task and two optional ones, each small, so superpowers:subagent-driven-development is not warranted (WORKING_STYLE "Delegation"). Steps use checkbox (`- [ ]`) syntax for tracking. Nothing here starts without the user's go-ahead on the decisions at the end.

**Goal:** Make "which engines do X" a question the compiler asks, in the few places where today it is a grep:
- E4 (core): the "takes part in the cookie jar" fact is written five times as `matches!(kind, Webview2 | Cef)`. It becomes one `EngineDesc::jar` field. The three `_ =>` arms that decide a per-engine fact become exhaustive, so a fifth engine fails to compile until each is answered.
- E4 (optional caps): F12, Ctrl+F and their menu rows silently do nothing on engines that lack DevTools or find. Two fields, `devtools` and `find`, turn that into a toast.
- E8 (optional): the one subtle line pair WebView2 and CEF both copy (report `user_started`, then clear it once the load's end is reported) becomes one tested `NavLabel` method.

The plan also says plainly what is not worth doing: the survey's full table (transport, runtime, platforms, `graceful_exit`, presentation) and the full E8 (`NavReporter`, `FrameSink`, `seed_cookies`, `win_job`).

**Architecture:**
- **One mechanism.** `EngineDesc` (`crates/manifold/src/process_manager.rs:32-41`) is already the table that `desc()` builds by an exhaustive `match` (`:47-66`). It gains plain `bool` fields only. Every call site reads `kind.desc().<field>` and keeps no engine list of its own.
- **The rule for what goes in the table.** A fact goes in the table only when both hold:
  - code asks it through a non-exhaustive predicate (`matches!`, `==`, `_ =>`) at two or more sites;
  - or a new engine would silently get a wrong default from it.

  Facts that are already exhaustive `match`es are compiler-checked as they are. That covers `engine_info`'s colour, tier, pros and cons, `engine_binary_path` and `availability_with`; they stay where they are. Single-engine literals whose default is right for any new engine also stay. These are the CEF runtime, the WebView2 profile slot and the WebKit WSL relay (inventory D below).
- **E8 stays in `ipc-protocol::nav_label`**, the module the survey itself names as the pattern to follow (`engines.md:152`). It gets one pure method and its tests there. No new crate, and no `windows` dependency in `ipc-protocol`.

**Tech Stack:** Rust (edition 2021, nightly-2026-09-23), no new dependencies. Tests use the existing fakes:
- `EngineTab::fake` (`process_manager.rs:189-194`)
- `live_tab` and `sent` (`session_sync/tests.rs:4225-4246`)
- `app_on` (`:742`)

**Spec / source:**
- `.superpowers/survey/engines.md` E4 (`:67-94`) and E8 (`:143-163`), surveyed at 8182aa4.
- `docs/superpowers/specs/2026-09-30-main-split-and-engine-table-design.md:34-39`. That spec created `EngineDesc` and decided that "`engine_info`'s prose (pros and cons) stays where it is, keyed by `desc()`". This plan keeps that decision.
- `docs/superpowers/specs/2026-10-02-jars-design.md:46-47, 81-86`: Servo and WebKit "are neither seeded nor pulled"; their cookie APIs are mapped but not wired. `2026-10-03-per-tab-jars-design.md:121, 188` lists "Servo and WebKit cookie wiring" as out of scope. These are the known future the `jar` field pays for.
- README "Considered and rejected" (`README.md:479`) rejected one shared engine-side outbox. This plan does not revisit it.
- The user held E4 and E8 after wave 3b: "each needs a written, stress-tested plan and the user's go-ahead before code".

**Global constraints (every task):**
- Work on `main` (CLAUDE.md "Branches"). Commit per task. Run `cargo fmt --all` before each commit. The pre-commit hook runs fmt check and clippy; never `--no-verify`.
- Clippy: `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 -p gate --all-targets -- -D warnings`. An unused field fails it as dead code, so add no field without a reader in the same commit.
- Tests: `cargo test -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 -p gate`. Today's baseline is 1,124 tests (`CHANGELOG.md:17`; recount at the start, below). Each task's count must be the baseline plus exactly the tests it adds, all passing.
- No test may read or write the real `%APPDATA%\manifold` (CLAUDE.md "Tests and the user's profile"). The tests here use `App::default()`, `app_on(temp_dir(..))` and fakes only.
- Before any build, test or gate run, check that no timed run holds the machine:
  - `.superpowers/rep/out/done` exists, or that directory is gone;
  - no live exclusive GPU lease (LONG_RUNS.md);
  - `Get-Process python, manifold*, cargo -EA 0` shows no leftovers.

  Use `-j 8` for cargo (CLAUDE.md), which leaves the quarter of cores LONG_RUNS.md asks for.
- Python, if any is run: `python -P f.py < /dev/null`, never reading stdin.
- Implementers change nothing beyond their task and report every deviation (WORKING_STYLE "Delegation"). Use `model: "sonnet"` for an implementer and `model: "opus"` for the final whole-branch review. Small tasks under about 10 lines are done inline.

---

## Evidence: the survey's claims checked against HEAD (674d6af)

Line numbers in the survey are from 8182aa4. Since then wave 3a, wave 3b and the E16 file splits have moved them. Every row below was re-read at HEAD.

| Survey claim (engines.md) | At HEAD | Verdict |
|---|---|---|
| `EngineDesc {name, code, binary, label, bgra}` has no transport, runtime, jar, capability or platform fields (`:70`) | `process_manager.rs:32-41`, `desc()` `:47-66` | True |
| "Has a per-tab jar" `Webview2 \| Cef` at 5 sites (`:71`) | `process_manager.rs:720` (`ENGINE_AWAIT_COOKIES`), `:770` (seed `SetCookies`), `tabs.rs:752` (`will_switch`), `tabs/jar_pull.rs:133` (`pull_cookies`), `jar_defs.rs:242` (`engine_ignores`) | True: five sites, all non-exhaustive `matches!` |
| "Downloadable runtime" `== Cef` at 4 sites (`:72`) | `engine_install.rs:707` (custom dir), `:768` (install refusal), `settings_page.rs:308` (leftovers), `process_manager.rs:671` (custom dir filter), `:699-700` (host copy), `:707` (cache dir) | True, but every one of these is a CEF-specific mechanism (the pin, `cef-runtime/`, the custom dir that validates against the pin, Chromium's cache). A `Runtime::Download{..}` field would claim a generality the code does not have. Inventory D. |
| `engine_binary_path` with `unreachable!` for WebKit (`:74`) | `availability.rs:34-58`, `unreachable!` at `:44` | True. The `match` is exhaustive, and spawn_tab's early return (`process_manager.rs:665-670`) is the only route in. Kept (Options, rejected R3). |
| `engine_on_disk` (`:75`) | `availability.rs:146-153`, exhaustive. But spawn_tab re-dispatches the same readers with `_ => engine_on_disk(kind)` (`process_manager.rs:674-678`) | Half true: the hazard is the second dispatch's `_` arm. Task 1 step 5. |
| `platform_unavailable_reason` (`:76`) | `availability.rs:181-187`, `_ => None` at `:185` | True: a wildcard. Task 1 step 4. |
| `availability_with` (`:77`) | `availability.rs:212-218`, exhaustive | Already compiler-checked. No change. |
| WebKit special case in `Drop` and `reap_host` (`:78`) | `process_manager.rs:204`, `:236` | True. It is about the WSL relay (`webkit.rs`), not about "remote engines". A native Linux WebKit host would be local, so a `transport` field keyed by engine would be wrong there. Inventory D. |
| `engine_info.rs:116-227` separate matches (`:79`) | `engine_color` `:114-124`, `engine_tier` `:169-175`, `engine_pros` `:179-198`, `engine_cons` `:204-222`, all exhaustive | No hazard. The 2026-09-30 spec decided they stay. |
| Capability gaps are prose only; the UI offers actions that silently do nothing (`:80`) | Servo ignores `FindStart`/`FindNext`/`FindPrevious`/`FindStop`/`OpenDevTools`/`SetMuted` (`manifold-engine-servo/src/main.rs:318-329`). WebKit ignores `OpenDevTools` (`manifold-engine-webkit/src/main.rs:414-429`, Xvfb has no window manager). The host sends regardless (`shortcuts.rs:330-334`, `:408-424`, menu rows `popups.rs:1156-1157`, headless `debug_control.rs:1163, 1169`). | True, and wider than the survey says: WebKit DevTools is also a no-op. The `HostToEngine::OpenDevTools` doc (`ipc-protocol/src/messages.rs:272-280`) says WebKitGTK's inspector works after `developer-extras`. That is stale. |
| E8: seed-wait duplicated (`:146`) | `webview2/main.rs:253-260`, `cef/main.rs:208-217`, both already calling the shared `transport::wait_for_first_message` (`transport.rs:280`) | Mostly done: about 3 lines of glue each remain. |
| E8: `NavigationStateChanged` dirty-flag send duplicated (`:147`) | `webview2/main.rs:434-450`, `cef/main.rs:434-450`: identical 15-line blocks. `NavState` structs are near-identical: `webview2/src/nav_state.rs` (`dirty: false` initially) and `cef/src/nav_handler.rs:14-39` (`dirty: true`). | True for WebView2 and CEF. |
| E8: frame recreate duplicated (`:150`) | `webview2/main.rs:495-522`, `cef/main.rs:507-550` (CEF retries) | True. It belongs to E2 (frame path), which the user parked. |
| E8: Job Object duplicated (`:151`) | engine side `webview2/main.rs:541-561`, `cef/main.rs:46-66`. The host's lives in `process_manager/job.rs`. | True: 19 stable lines each. Sharing them would put `windows` into `ipc-protocol`, whose dependencies are `interprocess`, `memmap2`, `postcard` and `serde`. |
| E8 scenario: "the `finished` clear in NavLabel ... forgotten in Servo or WebKit" (`:153`) | Servo's `NavState` has no label (`servo/main.rs:36-43`) and sends `user_started: None` (`:334-348`). WebKit diffs polled state and sends `None` (`webkit/main.rs:439-490`). | Inaccurate: only WebView2 and CEF use `NavLabel`. The real duplication is two copies, not four. |
| "three of four can't be built on Windows" (`:162`; also `CLAUDE.md` "Formatting and lints") | README: only WebKit is Linux-only. CEF builds on Windows once its runtime is downloaded (a CEF debug exe from 2026-10-04 is in `target/debug/cef-runtime/`). Servo builds on Windows but takes about 33 minutes (there is no Servo exe in `target/debug`). | Conflict between CLAUDE.md and README. Left for the user (Decision 4). |
| E14 duplicate Local/Remote enums | `spawn_reader_thread` is generic (`process_manager.rs:591-633`), and `engine_io::Writer` takes either transport | Done in practice by wave 3b. Not in this plan. |
| E16 split "after E4" | `availability.rs`, `job.rs`, `profiles.rs`, `webkit.rs` and `engine_io.rs` already moved out; `process_manager.rs` is 806 lines | The order no longer matters: the table stays in `process_manager.rs`, and the split modules read it through `desc()`. |

### Inventory of every per-engine reference (non-test code at HEAD)

`grep -rn "EngineKind::\(Cef\|Webview2\|Servo\|Webkit\)" crates/manifold/src` lists about 70 lines. Each falls into one of four categories:

- **A. One fact asked through a non-exhaustive predicate at several sites: becomes a field.**
  - "Takes part in the cookie jar", ×5 (above). This is `jar`.
  - Optional, the same shape: "has DevTools" and "has find in page". There are no host checks today: the host sends to every engine, which is the bug. These are `devtools` and `find`.
- **B. A per-engine decision behind a `_ =>` arm: becomes exhaustive.**
  - `engine_runtime_dir` `_ => None` (`availability.rs:83`)
  - `platform_unavailable_reason` `_ => None` (`:185`)
  - spawn_tab's on-disk dispatch `_ => engine_on_disk(kind)` (`process_manager.rs:677`). This one is unified with `engine_on_disk`'s dispatch into one exhaustive `on_disk_reader`.
- **C. Already exhaustive: no change.**
  - `desc()`, `engine_binary_path`, `engine_on_disk` (after Task 1 it reads `on_disk_reader`), `availability_with`
  - `engine_color`, `engine_tier`, `engine_pros`, `engine_cons`
  - `ENGINE_PICKER_ORDER` (`engine_info.rs:226`) and `ALL`
- **D. Single-engine literals whose default is right for any new engine: stay, as named literals.**
  - CEF's custom dir, pin, install, leftovers, host copy and cache dir (`engine_install.rs:707, 768`, `settings_page.rs:308`, `process_manager.rs:361, 671, 699-700, 707`). A new engine downloads nothing until someone writes its installer, and then it needs its own pin and layout anyway.
  - WebView2's profile slot, sweep and demo (`process_manager.rs:727`, `profiles.rs:228`, `session_files.rs:143, 166`)
  - WebKit's WSL spawn, `Drop` and reap (`process_manager.rs:204, 236, 665`, `webkit.rs:151`). A new engine is local by default, which is the right default.
  - `platform_default_engine`'s preference list (`availability.rs:191-196`). This is a product choice, not an engine fact.

---

## Options

### E4

| # | Option | Cost | What it buys | Verdict |
|---|---|---|---|---|
| E4-1 | The survey's full table: `transport`, `has_jar`, `runtime: None\|Download{..}\|System`, `platforms`, `caps{find, devtools, mute, popups, downloads}`, `graceful_exit`, presentation data in the table | M, about 300-400 lines across 8 files | One place to read an engine | **Rejected.** Several fields would be dishonest or dead: `runtime: Download` would hide that everything about it is CEF's (inventory D); `transport` is about WSL, not the engine; `platforms` is already one exhaustive function; `graceful_exit` has no reader until E25, which waits on the parked E2; `popups`/`downloads` caps have no host consumer. Clippy `-D warnings` fails dead fields, so they would need fake readers. Moving `engine_info`'s prose reverses a recorded spec decision for no safety gain (those matches are exhaustive). |
| **E4-2** | **`jar` field (5 sites), the 3 wildcard arms made exhaustive, one `on_disk_reader`** | **S: about 25 lines of production change, about 50 of tests. One task. Gate `full` (about 5.5 min) plus `jar-probe.sh` (about 5 min)** | **Every non-exhaustive per-engine question in the host goes through the compiler or the table. Giving Servo or WebKit a jar (a known future, jars spec) becomes one `true` instead of a 5-site hunt where a miss seeds but never pulls, or the reverse.** | **Recommended** |
| E4-3 | Do less: `jar` field only | XS, about 10 lines plus tests | The one fact with 5 copies | Acceptable fallback. It leaves 3 wildcard arms, each of which hands a fifth engine a quiet default (`None` reason, no runtime dir, cached on-disk read at spawn). |
| E4-4 | Do nothing | 0 now | — | The cost is deferred, not zero. Today a grep for `Webview2 \| Cef` finds all 5 sites, because they are written identically, so a careful person would not miss one. The risk is when the first one drifts in spelling, or when Servo gets jars. Low cost of doing nothing; low cost of doing E4-2. |
| E4-caps | Optional add-on: `find` and `devtools` bools; the shortcut function (shared by the key, the menu row and headless) toasts "{label} has no DevTools" / "{label} has no find in page" instead of sending into the void | S: about 15 lines plus 2 tests | A visible fix: F12 on a Servo or WebKit tab, and Ctrl+F on Servo, now say why nothing happens | Recommended if the user wants the behaviour change. It is a user-visible change, so it is a separate decision. `mute` is rejected as a cap: it is persisted tab state that must survive a swap to an engine that has audio, so the toggle stays meaningful. |

### E8

| # | Option | Cost | What it buys | Verdict |
|---|---|---|---|---|
| E8-1 | The survey's full version: `NavReporter`, `FrameSink`, `seed_cookies()`, `win_job::kill_on_exit()` in `ipc-protocol` or a new `engine-common` crate | L | Shared engine-main logic | **Rejected now.** `FrameSink` is E2's home, and E2 is parked by the user. `win_job` adds `windows` to `ipc-protocol` (or a new crate) to share 19 lines that have not changed since they were written. `seed_cookies` would save about 3 lines a crate (the wait is already shared). Servo and WebKit are not candidates: neither uses `NavLabel` (Evidence), and neither can be verified on this machine (no Servo exe; WebKit needs WSL). |
| E8-2 | Move `NavState` itself (url, title, back/forward, loading, label, dirty) into `nav_label.rs` with `take_report() -> Option<EngineToHost>`; both engines use it | M: about 60 lines moved, 25 references re-pointed across two crates, saves about 25 lines | One struct | Not recommended: the two structs differ on purpose (the initial `dirty`; WebView2's `had_recent_input` used by `popup.rs`), and the change touches CEF, which CI does not build. Cost exceeds gain. |
| **E8-3** | **`NavLabel::take_for_report(&mut self) -> Option<bool>`: returns `user_started` and clears it once `finished` is taken. WebView2 and CEF each replace 3 lines with one call** | **XS: about 10 lines plus 2 tests in `ipc-protocol`, about 6 changed lines in the engines. Needs one local CEF build** | **The one subtle rule the survey's scenario is about ("the `finished` clear") lives once, tested, beside `label_new`** | Optional. Small and safe, but the gain is also small: two copies that sit 15 lines from each other. |
| E8-4 | Do nothing | 0 | — | **Recommended.** The duplicated block is 15 identical lines in two crates. The shared logic (`NavLabel`, `wait_for_first_message`) already moved in 55742e8. What remains is glue. |

---

## Task 0: baseline (inline, no code)

- [ ] **Step 1: Machine check.** Confirm the global constraints' machine check passes. Record CPU load (`(Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue`) and Defender state (`(Get-MpComputerStatus).RealTimeProtectionEnabled`).
- [ ] **Step 2: Baseline count.** Run `cargo test -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 -p gate 2>&1 | grep "^test result"`. Sum the `passed` counts and record the total as BASE in the commit message of Task 1. The expected value is 1,124 plus any tests added since `CHANGELOG.md:17`.

---

## Task 1: `EngineDesc::jar` and exhaustive per-engine arms (E4-2)

**Files:**
- Modify: `crates/manifold/src/process_manager.rs` (`EngineDesc` `:32-41`, `desc()` `:47-66`, spawn_tab `:674-678`, `:720`, `:770`)
- Modify: `crates/manifold/src/process_manager/availability.rs` (`engine_runtime_dir` `:80-85`, `engine_on_disk` `:143-153`, `platform_unavailable_reason` `:177-187`)
- Modify: `crates/manifold/src/tabs.rs` (`will_switch` `:747-753`)
- Modify: `crates/manifold/src/tabs/jar_pull.rs` (`pull_cookies` `:128-138`)
- Modify: `crates/manifold/src/jar_defs.rs` (`:242`)
- Test: `crates/manifold/src/session_sync/tests.rs` (after `sent`, `:4246`)
- Test: `crates/manifold/src/process_manager/tests.rs` (after `only_cef_has_a_downloaded_runtime_dir`, `:591`)

**Interfaces:**
- `EngineDesc` gains `pub(crate) jar: bool`.
- `availability.rs` gains `fn on_disk_reader(kind: EngineKind) -> Option<fn() -> RuntimeOnDisk>` (private) and `pub(super) fn engine_on_disk_fresh(kind: EngineKind) -> RuntimeOnDisk`.
- No other signature changes.

- [ ] **Step 1: Characterization tests, written against today's code (they must pass before any production change).**

Append to `crates/manifold/src/session_sync/tests.rs`, right after `fn sent` (`:4244-4246`):

```rust
/// The engines that take part in the cookie jar (spec 2026-10-02-jars-design): only their tabs are asked for
/// cookies, and only they count as "will switch" when their jar changes. The rest neither seed nor pull.
const JAR_ENGINES: [EngineKind; 2] = [EngineKind::Webview2, EngineKind::Cef];

#[test]
fn only_jar_engines_are_asked_for_cookies() {
    let mut app = app_on(&temp_dir("jar-engines-pull").join("session.mnf"), 1);
    for kind in EngineKind::ALL {
        let (id, mut peer) = live_tab(&mut app, kind, "https://a/");
        app.pull_cookies(&id);
        let asked = sent(&mut peer).iter().any(|m| matches!(m, HostToEngine::GetCookies));
        assert_eq!(asked, JAR_ENGINES.contains(&kind), "{kind:?}");
    }
}

#[test]
fn only_jar_engines_switch_when_their_jar_is_cleared() {
    let mut app = app_on(&temp_dir("jar-engines-switch").join("session.mnf"), 1);
    for kind in EngineKind::ALL {
        let (id, _peer) = live_tab(&mut app, kind, "https://a/");
        // Running on the default jar, which the user cleared: a jar engine must respawn into it.
        app.cookies.jar_tabs.insert(id.clone(), (crate::jar::JarKey::Default, crate::jar::TabCookies::default()));
        app.cookies.jar_reseed.insert(id.clone());
        assert_eq!(app.will_switch(&id), JAR_ENGINES.contains(&kind), "{kind:?}");
    }
}
```

Append to `crates/manifold/src/process_manager/tests.rs`, after `only_cef_has_a_downloaded_runtime_dir` (`:587-591`):

```rust
/// Engines that need nothing on disk read as Ready without touching it, cached or fresh; spawn and the engine lists
/// share one reader per engine (`on_disk_reader`).
#[test]
fn bundled_engines_read_ready_without_a_disk_check() {
    for kind in [EngineKind::Webview2, EngineKind::Webkit] {
        assert_eq!(engine_on_disk(kind), RuntimeOnDisk::Ready, "{kind:?}");
    }
}
```

Run: `cargo test -j 8 -p manifold -- only_jar_engines bundled_engines_read_ready`
Expected: 3 passed. If one fails, stop: the test, not the code, is wrong (no production code has changed yet). Fix the test and report it.

- [ ] **Step 2: Commit the characterization tests.**

```bash
cargo fmt --all
git add crates/manifold/src/session_sync/tests.rs crates/manifold/src/process_manager/tests.rs
git commit -m "test: pin which engines take part in the jar, and the bundled engines' on-disk answer (E4 characterization)"
```

- [ ] **Step 3: The `jar` field and its five readers.**

In `process_manager.rs`, add the field after `bgra` (`:40`):

```rust
    /// Frames arrive as BGRA (CEF, WebView2) rather than RGBA (Servo, WebKit).
    pub(crate) bgra: bool,
    /// Takes part in the cookie jar: waits for `SetCookies` before its first navigation (`ENGINE_AWAIT_COOKIES`), and
    /// answers `GetCookies` (spec 2026-10-02-jars-design). The rest are neither seeded nor pulled, and a jar choice
    /// on their tabs shows "This engine does not use jars yet".
    pub(crate) jar: bool,
```

In `desc()`, add the value to each row. Servo `jar: false`, Cef `jar: true`, Webkit `jar: false`, Webview2 `jar: true`. rustfmt will expand the one-line rows that grow past 120 columns; accept its layout.

Replace the five predicates. In each case the text before and after the predicate is unchanged:
- `process_manager.rs:719-720`: replace the comment and condition with:
  ```rust
      // Jar engines wait for the jar (`SetCookies`) before their first navigation.
      if kind.desc().jar {
  ```
- `process_manager.rs:770`: `if matches!(kind, EngineKind::Webview2 | EngineKind::Cef) {` becomes `if kind.desc().jar {`
- `tabs.rs:747-753`: in the doc, "A live WebView2 or CEF tab" becomes "A live tab on a jar engine (`EngineDesc::jar`)". In the body, `matches!(tab.tab.kind, EngineKind::Webview2 | EngineKind::Cef)` becomes `tab.tab.kind.desc().jar`.
- `tabs/jar_pull.rs:128`: in the doc, "Asks one WebView2 or CEF tab" becomes "Asks one jar engine's tab (`EngineDesc::jar`)". At `:133`, `matches!(node.tab.kind, EngineKind::Webview2 | EngineKind::Cef)` becomes `node.tab.kind.desc().jar`.
- `jar_defs.rs:242`: `let engine_ignores = !matches!(engine, EngineKind::Webview2 | EngineKind::Cef) && key != JarKey::Default;` becomes `let engine_ignores = !engine.desc().jar && key != JarKey::Default;`

Remove any `EngineKind` import that becomes unused (clippy reports it). Then check that no copy is left:

Run: `grep -rn "Webview2 | EngineKind::Cef\|Cef | EngineKind::Webview2" crates/manifold/src --include=*.rs`
Expected: only `engine_info.rs:171` (`engine_tier`, an exhaustive arm, inventory C) and test files.

- [ ] **Step 4: Exhaustive arms in `availability.rs`.**

Replace `engine_runtime_dir`'s body (`:81-84`):

```rust
    match kind {
        EngineKind::Cef => engine_hosts_dir().map(|dir| dir.join("cef-runtime")),
        EngineKind::Servo | EngineKind::Webkit | EngineKind::Webview2 => None,
    }
```

Replace `platform_unavailable_reason`'s body (`:182-186`):

```rust
    match kind {
        EngineKind::Webview2 => (os != "windows").then_some("needs Windows"),
        EngineKind::Webkit => (!matches!(os, "windows" | "linux")).then_some("needs Windows or Linux"),
        // Servo's host builds wherever manifold does; CEF's platforms are its pin's (`availability_with`).
        EngineKind::Servo | EngineKind::Cef => None,
    }
```

- [ ] **Step 5: One on-disk reader for the engine lists and for spawn.**

In `availability.rs`, replace `engine_on_disk` (`:143-153`, doc included) with:

```rust
/// The disk check `kind` needs before it can run: CEF's runtime (`cef_on_disk`) and Servo's host exe
/// (`servo_on_disk`). `None`: nothing to check (WebView2 is part of Windows; WebKit's host is found in WSL only when
/// a tab spawns). The one dispatch both `engine_on_disk` and `engine_on_disk_fresh` read.
fn on_disk_reader(kind: EngineKind) -> Option<fn() -> RuntimeOnDisk> {
    match kind {
        EngineKind::Cef => Some(cef_on_disk),
        EngineKind::Servo => Some(servo_on_disk),
        EngineKind::Webview2 | EngineKind::Webkit => None,
    }
}

/// The `on_disk` that `engine_availability` takes for `kind`, for the engine lists: `on_disk_reader`'s answer, reread
/// at most every `ON_DISK_CHECK_TTL`; Ready when there is nothing to check.
pub(crate) fn engine_on_disk(kind: EngineKind) -> RuntimeOnDisk {
    let Some(read) = on_disk_reader(kind) else { return RuntimeOnDisk::Ready };
    on_disk_cached(&ON_DISK, kind, std::time::Instant::now(), read)
}

/// `engine_on_disk` read now, not through the cache: a Servo host built or a CEF runtime copied a moment ago
/// should launch.
pub(super) fn engine_on_disk_fresh(kind: EngineKind) -> RuntimeOnDisk {
    on_disk_reader(kind).map_or(RuntimeOnDisk::Ready, |read| read())
}
```

In `process_manager.rs` spawn_tab, replace `:672-678` (the comment and the `match`) with:

```rust
    let on_disk = engine_on_disk_fresh(kind);
```

`servo_on_disk` and `cef_on_disk` keep their visibility. `cef_on_disk` is `pub(crate)` because `engine_install` uses it; check with `grep -rn "cef_on_disk\|servo_on_disk" crates/manifold/src`. If, after this step, `servo_on_disk` has no caller outside `availability.rs` and its tests, leave its `pub(super)` as is. Changing visibility is not part of this task.

- [ ] **Step 6: Run the checks.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 -p gate --all-targets -- -D warnings
cargo test -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 -p gate 2>&1 | grep "^test result"
```

Expected:
- clippy is clean;
- the test total is BASE + 3, with 0 failed;
- the three Step 1 tests still pass, unchanged. They are the parity proof for the five jar sites, two of which (`pull_cookies`, `will_switch`) they drive directly. The `engine_ignores` site is covered by the existing `the_chip_shows_only_non_default_or_warning_states` (`jar_defs/tests.rs:189-208`, a Servo tab). The two spawn sites are proven end to end in Step 8.

- [ ] **Step 7: Docs.**
  - `.superpowers/survey/engines.md:67`: append to the E4 heading "— DONE in part (`jar` field, exhaustive arms; plan 2026-10-11-engine-abstraction.md); the rest of the table rejected there". Under it, add one bullet with the commit hash and "survey line numbers were stale; see the plan's Evidence table".
  - README: no change. `README.md:334` and `:492` describe behaviour ("Servo and WebKit ... get no cookies"), which is unchanged.
  - `process_manager.rs` `EngineDesc` doc (`:29-31`): "the one place its name, store code, binary, label and pixel order live" becomes "the one place its name, store code, binary, label, pixel order and jar use live".

- [ ] **Step 8: Build and end-to-end proof.**

Build: `cargo build -j 8 -p manifold -p manifold-engine-webview2 -p gate`.

CEF's spawn site also changed, so the probe must run a current CEF host. `cargo build -j 8 -p manifold-engine-cef` needs the CEF download. A CEF debug build is present in `target/debug/cef-runtime/`, dated 2026-10-04. The host side of the jar is in `manifold.exe`, so the CEF exe from 2026-10-04 is still a valid peer: `ipc-protocol` has not changed in this task, and `PROTOCOL_VERSION` is unchanged. Rebuilding CEF is therefore not required. Confirm with `git diff HEAD~2 --stat -- crates/ipc-protocol` (expected empty).

Gate: `cmd //c start "" //b //belownormal bash scripts/gate.sh .superpowers/gate-e4.log full`. Expected `GATE done` and every stage `exit=0`, in about 5.5 minutes under 38-45% load (README `:399`).

Jar probe, after the gate finishes and not in parallel: `cmd //c start "" //b //belownormal bash scripts/jar-probe.sh > .superpowers/jar-probe-e4.log 2>&1`, about 5 minutes. Expected:
- the CEF and WebView2 first-request authentication checks pass;
- `perjar` and `jarswitch` pass.

This run proves `process_manager.rs:720` and `:770`: a host that stopped waiting for `SetCookies`, or stopped being seeded, fails the first-request check.

Report the CPU load and Defender state from each log's header.

- [ ] **Step 9: Commit.**

```bash
git add crates/manifold/src/process_manager.rs crates/manifold/src/process_manager/availability.rs crates/manifold/src/tabs.rs crates/manifold/src/tabs/jar_pull.rs crates/manifold/src/jar_defs.rs .superpowers/survey/engines.md
git diff --cached --stat
git commit -m "engines: EngineDesc::jar replaces five Webview2|Cef checks; per-engine wildcard arms made exhaustive; one on-disk reader (E4)

BASE=<n> tests, now <n+3>. Gate full: <result>, CPU <x>%, Defender <state>. jar-probe: <result>.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Fill each `<...>` from Step 0, Step 6 and Step 8 output before committing. These fields are run results, not placeholders in the plan.

---

## Task 2 (optional, Decision 1): `find` and `devtools` caps

Skip this task unless the user approves Decision 1.

**Files:**
- Modify: `crates/manifold/src/process_manager.rs` (`EngineDesc`, `desc()`)
- Modify: `crates/manifold/src/shortcuts.rs` (`open_devtools_shortcut` `:325-334`, `open_find_bar_shortcut` `:403-424`)
- Modify: `crates/ipc-protocol/src/messages.rs` (`OpenDevTools` doc `:272-280`, the stale WebKitGTK sentence)
- Modify: `README.md` (`:285`, `:310`), `CHANGELOG.md` (Unreleased, Features)
- Test: `crates/manifold/src/session_sync/tests.rs` (after Task 1's tests)

**Interfaces:**
- `EngineDesc` gains `pub(crate) find: bool` and `pub(crate) devtools: bool`.
- The behaviour of `open_devtools_shortcut` and `open_find_bar_shortcut` changes only for a focused live tab whose engine lacks the capability.

One mechanism covers all three entry points: the key (`shortcuts.rs:152, 164`), the tab menu rows (`popups.rs:1156-1157`) and headless `OPENDEVTOOLS`/`OPENFINDBAR` (`debug_control.rs:1163, 1169`) all call these two functions. It does not cover:
- greying out the menu rows (a second consumer; not proposed);
- a find bar already open when focus moves to a Servo tab (it stays open and gets no results, as today).

- [ ] **Step 1: Failing tests first.**

```rust
#[test]
fn devtools_on_an_engine_without_them_toasts_instead_of_sending() {
    let mut app = app_on(&temp_dir("caps-devtools").join("session.mnf"), 1);
    for (kind, has) in [
        (EngineKind::Cef, true),
        (EngineKind::Webview2, true),
        (EngineKind::Servo, false),
        (EngineKind::Webkit, false),
    ] {
        let (id, mut peer) = live_tab(&mut app, kind, "https://a/");
        app.focused = Some(id);
        app.toasts.save_toast_queue.clear();
        app.open_devtools_shortcut();
        let sent_it = sent(&mut peer).iter().any(|m| matches!(m, HostToEngine::OpenDevTools));
        assert_eq!(sent_it, has, "{kind:?}");
        let toast = format!("{} has no DevTools", kind.desc().label);
        assert_eq!(app.toasts.save_toast_queue.contains(&toast), !has, "{kind:?}");
    }
}

#[test]
fn find_on_an_engine_without_it_toasts_and_leaves_the_bar_shut() {
    let mut app = app_on(&temp_dir("caps-find").join("session.mnf"), 1);
    app.view_mode = ViewMode::Item;
    for (kind, has) in [
        (EngineKind::Cef, true),
        (EngineKind::Webview2, true),
        (EngineKind::Webkit, true),
        (EngineKind::Servo, false),
    ] {
        let (id, _peer) = live_tab(&mut app, kind, "https://a/");
        app.focused = Some(id);
        app.find.open = false;
        app.toasts.save_toast_queue.clear();
        app.open_find_bar_shortcut();
        assert_eq!(app.find.open, has, "{kind:?}");
        let toast = format!("{} has no find in page", kind.desc().label);
        assert_eq!(app.toasts.save_toast_queue.contains(&toast), !has, "{kind:?}");
    }
}
```

These call `kind.desc().label`, which exists today. They fail on the old code on their toast and `find.open` asserts, not on compilation. `ViewMode` is already in scope in `session_sync/tests.rs` (used at `:330`).

Run: `cargo test -j 8 -p manifold -- devtools_on_an_engine find_on_an_engine`
Expected: both FAIL. The Servo and WebKit rows fail on the toast assert, and Servo also fails on `find.open`.

- [ ] **Step 2: Fields.**

Add after `jar`:

```rust
    /// Opens its own DevTools window on `OpenDevTools` (CEF, WebView2). Servo has no inspector API; WebKit's
    /// inspector needs a window manager its Xvfb session doesn't have. Without it, F12 toasts instead.
    pub(crate) devtools: bool,
    /// Answers `FindStart` with `FindResult` (all but Servo, whose embedding API has no find). Without it, Ctrl+F
    /// toasts instead of opening a find bar that never gets a result.
    pub(crate) find: bool,
```

The values are:
- Servo: `devtools: false, find: false`
- Cef: `true, true`
- Webkit: `false, true`
- Webview2: `true, true`

- [ ] **Step 3: Readers.**

In `shortcuts.rs`, `open_devtools_shortcut` body:

```rust
        let Some(id) = self.focused.clone() else { return };
        let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return };
        let kind = node.tab.kind;
        if kind.desc().devtools {
            node.tab.send(&HostToEngine::OpenDevTools);
        } else {
            self.queue_toast(format!("{} has no DevTools", kind.desc().label));
        }
```

In `open_find_bar_shortcut`, insert after the `if !self.showing_tab_view() { return; }` block (`:414-416`):

```rust
        // A sleeping or starting tab opens the bar as before; its engine is checked when it is live.
        let live = self.focused.as_deref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab());
        if let Some(kind) = live.map(|t| t.tab.kind).filter(|k| !k.desc().find) {
            self.queue_toast(format!("{} has no find in page", kind.desc().label));
            return;
        }
```

Update both functions' doc comments. Add one sentence each: "An engine without DevTools (`EngineDesc::devtools`) gets a toast instead." and "An engine without find (`EngineDesc::find`) gets a toast instead."

In `ipc-protocol/src/messages.rs`, replace the `OpenDevTools` doc's WebKitGTK sentence. The new text says that WebKit's host ignores it, because the inspector window would wait forever under Xvfb with no window manager (`manifold-engine-webkit/src/main.rs:414-429`). This is a comment-only change in `messages.rs`. The E5 fingerprint test (`RECORDED_FINGERPRINT`) hashes serialized bytes, not docs, so check that it still passes.

- [ ] **Step 4: Checks.**

Run the clippy, fmt and test commands from Task 1 Step 6. Expected: clean, BASE + 5, 0 failed.

Gate tier: `scripts/gate-tier.sh` will advise `full`, because the change touches `process_manager.rs` and `ipc-protocol`. Run `full`, the same command as Task 1 Step 8, to log `.superpowers/gate-caps.log`. If Task 1 and Task 2 land together, one `full` run after both is enough.

Commit: `engines: F12 and Ctrl+F toast on engines without DevTools or find (EngineDesc::devtools, ::find)`, ending with the Co-Authored-By line.

Docs:
- Add a CHANGELOG "Unreleased" Features bullet: "F12 and Ctrl+F on an engine that can't (DevTools: Servo, WebKit; find: Servo) now say so in a toast instead of doing nothing."
- README `:285` (F12 paragraph: WebKit and Servo "no-op") and `:310` (Ctrl+F: "the bar still opens on a Servo tab ... but never shows a match count"): both now say the action shows a toast naming the engine instead.
- Update the survey E4 note.

---

## Task 3 (optional, Decision 2): `NavLabel::take_for_report` (E8-3)

Skip unless the user approves Decision 2 over the recommended E8-4 (do nothing).

**Files:**
- Modify: `crates/ipc-protocol/src/nav_label.rs` (impl `NavLabel`, after `label_new` `:61-66`; tests module)
- Modify: `crates/manifold-engine-webview2/src/main.rs` (`:434-450`)
- Modify: `crates/manifold-engine-cef/src/main.rs` (`:434-450`)

**Interfaces:**
- `pub fn take_for_report(&mut self) -> Option<bool>` on `NavLabel`.

- [ ] **Step 1: Failing tests** in `nav_label.rs`'s `mod tests`:

```rust
    #[test]
    fn a_finished_navigation_is_reported_once_with_its_label_then_unlabelled() {
        let mut l = NavLabel { user_started: Some(true), finished: true, ..NavLabel::default() };
        assert_eq!(l.take_for_report(), Some(true), "the load's end is sent with its label");
        assert_eq!((l.user_started, l.finished), (None, false));
        assert_eq!(l.take_for_report(), None, "a later change nothing started isn't labelled");
    }

    #[test]
    fn an_unfinished_navigation_keeps_its_label_across_reports() {
        let mut l = NavLabel { user_started: Some(false), ..NavLabel::default() };
        assert_eq!(l.take_for_report(), Some(false));
        assert_eq!(l.take_for_report(), Some(false), "redirects and title changes keep it");
    }
```

Run: `cargo test -j 8 -p ipc-protocol -- take_for_report a_finished an_unfinished`. Expected: compile error (no method), which is the failing state.

- [ ] **Step 2: Method**, after `label_new`:

```rust
    /// The label to send with a `NavigationStateChanged`. Once the load's end is being sent (`finished`), the label
    /// is cleared after this report, so a later change nothing started isn't labelled.
    pub fn take_for_report(&mut self) -> Option<bool> {
        let label = self.user_started;
        if std::mem::take(&mut self.finished) {
            self.user_started = None;
        }
        label
    }
```

Also update the `finished` field doc (`:43-45`): "the engine's main loop clears `user_started` after sending it" becomes "`take_for_report` clears `user_started` once it is sent".

- [ ] **Step 3: Engines.**

In both `webview2/main.rs:434-450` and `cef/main.rs:434-450`, set `user_started: state.label.take_for_report(),` (CEF: `nav.label`). Then delete the trailing `if std::mem::take(&mut ....label.finished) { ....label.user_started = None; }` block.

Order is preserved. The struct literal reads `user_started` before the clear, which is exactly the old send-then-clear. The send result is discarded with `let _` in both, so a failed send changed nothing before either.

- [ ] **Step 4: Checks.**
  - `ipc-protocol` and WebView2: clippy, fmt and test as in Task 1 Step 6. Expected BASE + 2 (on top of the earlier tasks), 0 failed.
  - CEF is not in CI or in CLAUDE.md's clippy set. Build it locally with `cargo build -j 8 -p manifold-engine-cef`. It needs the CEF download that produced the 2026-10-04 exe; if the build script cannot find it, stop and report. Do not commit an unbuilt CEF change. Then run `bash scripts/sync-runtime-deps.sh`, so probes run the new CEF (README `:409`).
  - Then run `cargo clippy -j 8 -p manifold-engine-cef -- -D warnings`.
  - Gate `full` (WebView2). Then run `CASES=history bash scripts/sleep-probe.sh`, which drives navigation labelling on WebView2 and CEF (README `:681`), detached at BelowNormal. Expected: pass on both.

Commit: `nav_label: take_for_report, one place for the report-then-unlabel rule WebView2 and CEF copied (E8)`, ending with the Co-Authored-By line.

---

## Final: review, docs, gate

- [ ] **Review.** Dispatch one whole-branch review with `model: "opus"` after the last task. The review checks:
  - the diff against this plan's scope;
  - each claim in the commit messages against the logs;
  - `git grep -n "EngineKind::Webview2 | EngineKind::Cef"` (expected: only `engine_info.rs:171`).

  Fix rounds go to `model: "sonnet"`.
- [ ] **Leftovers.** Check `Get-Process python, manifold*, cargo -EA 0`; nothing this work started may be left.
- [ ] **Docs, blast radius.**
  - The survey E4/E8 headings: E8 "PLANNED, DECLINED: see plan 2026-10-11", or "DONE in part" if Task 3 ran.
  - CHANGELOG Internal: one bullet for Task 1 ("`EngineDesc::jar` replaces five engine lists; per-engine wildcard arms are exhaustive").
  - README: none for Task 1 (`:334`, `:492` describe unchanged behaviour). Task 2 updates `:285` and `:310` in its own commit.

---

## Parity proofs, summarized

| Change | Proof |
|---|---|
| 5 jar sites | `only_jar_engines_are_asked_for_cookies` and `only_jar_engines_switch_when_their_jar_is_cleared` (written and passing before the change); `the_chip_shows_only_non_default_or_warning_states` (existing); `jar-probe.sh` for the two spawn sites (WebView2 and CEF hosts that wait for and receive the seed) |
| Exhaustive `engine_runtime_dir`, `platform_unavailable_reason` | Existing `only_cef_has_a_downloaded_runtime_dir` (`process_manager/tests.rs:587`), `engines_the_os_cannot_run_are_unavailable_with_a_reason` (`:391`), `the_platform_default_is_the_first_engine_the_os_can_run` (`:423`) |
| `on_disk_reader` | `bundled_engines_read_ready_without_a_disk_check` (new, passing before); existing `servo_is_installed_only_when_its_host_is_on_disk` (`:328`), `cef_state_follows_disk_then_overrides` (`:434`); gate `full` spawns WebView2 tabs through `engine_on_disk_fresh` |
| Whole crate | BASE tests, plus only the added ones; clippy `-D warnings`; fmt; Linux CI (`cargo test -p manifold -p ipc-protocol`) catches a `cfg(windows)` slip after push; `scripts/gate.sh LOG full` |
| Caps (Task 2) | Two new tests, red then green |
| `take_for_report` (Task 3) | Two new `ipc-protocol` tests; `CASES=history` sleep-probe on WebView2 and CEF |

`scripts/gate-tier.sh` puts `process_manager*`, `ipc-protocol` and the engine crates on `full`. Every task here touches one of them, so each gate run is `full`, never `light` or `ui`.

## Risks

- **`desc()` per call.** `kind.desc()` builds a five-to-eight-field struct by `match` each call. The sites run per pull, per chip draw and per spawn, not per frame per pixel. The compiler folds a `match` on a `Copy` enum returning constants. No measurement planned; if a profile ever shows it, make `desc()` return `&'static EngineDesc` from a `const` array (the 2026-09-30 spec's original shape).
- **rustfmt reflows `desc()`.** The one-line rows become multi-line as fields are added. This is cosmetic; `#[rustfmt::skip]` is not warranted (CLAUDE.md allows it only where hand layout carries meaning).
- **Task 1's test reaches `App` internals** (`cookies.jar_tabs`, `jar_reseed`, `focused`). These are the same fields `main/tests.rs:463-480` already uses, so the coupling is no new kind.
- **CEF unverifiable in CI.** Task 1 changes only the host side, and the 2026-10-04 CEF exe is a valid peer (protocol unchanged). Task 3 changes CEF source and is gated on a local CEF build (Step 4 stops if it can't build).
- **`sent` waits 200 ms per tab** (its read timeout). The new tests cost about 1.6 s of wall time in total. Acceptable; no fixed sleeps are added.

## Out of scope

- **E10** (`EngineEnv`, env-var literals across five crates): a separate survey item.
- **E2, E25.** E2 is parked by the user. E25 (`graceful_exit`) waits on it.
- **E16's remaining split, E24** (`current_exe().expect`), **E14** (done in practice).
- **The Linux-native WebKit host** (`webkit.rs:8`) and its `transport` question.
- Moving `engine_info`'s presentation into the table (rejected by spec 2026-09-30).
- Greying out menu rows for missing caps.
- **Mute as a cap** (rejected above).
- **`engine_binary_path`'s `unreachable!` for WebKit** (rejected R3). The `match` is exhaustive, the only route in is guarded at `process_manager.rs:665`, and returning `webkit_binary_path_in_repo()` instead would make a misrouted WebKit spawn try to exec a Linux ELF on Windows: a quieter failure than the panic, not a better one.
- Reconciling CLAUDE.md's "WebKit and Servo don't build on Windows" with the README (Decision 4).

## Decisions for the user

1. **Caps (Task 2): F12 and Ctrl+F toast on engines that lack them?** Recommend yes. It is a small, visible fix for a confirmed silent no-op (Servo: both; WebKit: DevTools), and it is the only part of E4 a user would notice.
2. **E8: do nothing (E8-4), or `NavLabel::take_for_report` (E8-3, Task 3)?** Recommend do nothing. The remaining duplication is 15 lines in two crates, the shared logic already moved in 55742e8, and Task 3 needs a local CEF build to verify. Choose E8-3 only if CEF will be rebuilt anyway.
3. **E4 scope: E4-2 (recommended), E4-3 (`jar` only), or E4-4 (nothing)?** Recommend E4-2. It is one small task, and the three wildcard arms are exactly the "silent default for a new engine" the survey worries about.
4. **Buildability wording.** CLAUDE.md says "WebKit and Servo don't build on Windows"; the README says only WebKit is Linux-only, and Servo builds but slowly. Recommend correcting CLAUDE.md's parenthesis to "WebKit is Linux-only; Servo builds on Windows but takes about 33 minutes". This is a shared-file change, so it is the user's call, not part of any task here.

## Stress test

Four adversarial passes over the draft, each re-reading the cited code. A fifth pass found nothing material.

- **Round 1: claims against code.**
  - The survey's E8 scenario (the `finished` clear forgotten in Servo or WebKit) is wrong: neither engine uses `NavLabel`. The draft had repeated it; the Evidence table now says so and E8's value is restated as two copies, not four.
  - The draft had proposed a `remote`/`transport` field for WebKit's `Drop` and `reap_host`. A native Linux WebKit host would be local, so the field would describe the WSL relay, not the engine. Moved to inventory D.
  - `engine_on_disk` is already exhaustive. The real hazard is spawn_tab's second dispatch with `_ =>` (`process_manager.rs:677`). Step 5 was retargeted at that.
  - WebKit's DevTools is a no-op too (`webkit/main.rs:429`), and the `messages.rs` doc saying otherwise is stale. Added to Task 2's values and its doc fix.
- **Round 2: will the tasks compile and prove what they claim?**
  - The draft test used `HostToEngine` equality; it derives only `Debug, Clone, Serialize, Deserialize` (`messages.rs:217`). Switched to `matches!`.
  - The draft `will_switch` test set a jar through `set_tab_jar`, which runs `sweep_jar_switches` and could put the tab to sleep, turning the assert into a test of the sweep. It now sets `jar_tabs` and `jar_reseed` directly, as `main/tests.rs:463-480` does.
  - The draft had `live_tab` called from `tabs/tests.rs`, but `live_tab` is a private fn in `session_sync/tests.rs`. The tests moved there.
  - The draft added `jar` without a reader in the same commit as a "table-only" first step, which clippy's dead-code lint would fail. Field and readers are now one step.
  - A Webkit-kind fake tab's `Drop` was checked: with `child: None` it returns before the WSL reaper (`process_manager.rs:201`).
  - Task 2's find check must sit after `showing_tab_view`, or Ctrl+F on a JSON level (`:410-413`) would toast for the focused tab's engine. It is placed after it.
- **Round 3: scope, proofs and the user's rules.**
  - The jar spawn sites (`:720`, `:770`) have no unit test reach (they spawn a process). The draft claimed "covered by the gate", but the gate runs WebView2 only and never checks a seed. `jar-probe.sh` (WebView2 plus CEF) was added as the proof, with the argument for why the 2026-10-04 CEF exe is a valid peer.
  - The build command was CLAUDE.md's `-p manifold -p manifold-engine-webview2`, but README `:399` adds `-p gate`, and `gate.sh` runs the `gate` binary. The plan uses the README's superset.
  - The draft's `Runtime::Download` "do less" variant contradicted the inventory D rule. It was removed rather than kept as a straw option, and the reason is recorded in the E4-1 row.
  - The draft's Task 2 toast lived only in the key handler. The menu rows and headless commands call the same two functions (`popups.rs:1156-1157`, `debug_control.rs:1163, 1169`), so the check moved into those functions: one mechanism, three entry points.
  - The gate tier was stated as `full` for every task, with the reason (`gate-tier.sh` paths).
- **Round 4: line numbers and docs blast radius.**
  - Three cited ranges were off by one: `will_switch`'s doc (`tabs.rs:747`), `EngineDesc`'s doc (`process_manager.rs:29-31`) and WebKit's DevTools comment (`webkit/main.rs:414`). All corrected.
  - The draft left Task 2's README update as "if any". README `:285` and `:310` both describe the silent no-ops Task 2 replaces, so they are now named in Task 2's docs step.
  - The draft told the implementer to check whether `ViewMode` is in scope; it is (`session_sync/tests.rs:330`), so the hedge was removed.
- **Round 5.** A re-read of every file:line in the Evidence table, the Files lists and the code blocks against HEAD 674d6af found nothing material.
