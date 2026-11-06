---
status: active
---

# Seed a host's cookies when it lands (ARC3)

## Why

A new host's first message is its cookie seed. Today a background spawn thread sends it, so the thread needs a copy of the jars. That one choice costs:

- `jar::Snapshots`, a second copy of `App::jars` behind `Arc<Mutex<..>>`, kept in step by hand at four sites (`publish_jar`, `forget_jar_memory`, `clear_jar_cookies`, `apply_pulled_cookies`). The pull clones a whole jar each time it changes one.
- A clear count per jar, because a seed read on the thread can be stale by the time the host lands. `TabSpawnOutcome` carries `seed_clears`, and `note_spawned_jar` compares counts to catch that.
- `RefCell` plumbing in `spawn_tab_async`, to get the seed back out of a closure that `process_manager::spawn_tab` calls.

Decision x1 (2026-11-05): the UI thread sends the seed when the spawn result lands, and `jars` moves into `CookieState` as a plain map. The reader of the jars and every writer are then on one thread, so the copy, the count and the race go away together.

## Design

**Seed.** `handle_tab_spawn_result`'s `Ok(tab)` arm already calls `note_spawned_jar` first. That function becomes:

```rust
/// A new host for tab `id` landed: forgets the old host's cookie state, then seeds a jar engine with `jar` as it is
/// now and records it. Sent before anything else, as the host waits for it (`ENV_AWAIT_COOKIES`).
pub(crate) fn seed_landed_host(&mut self, id: NodeId, jar: Option<NodeId>, tab: &ViewTab) {
    self.clear_cookie_state(id);
    let cookies = jar.and_then(|jar| self.cookies.jars.get(&jar)).map_or(&[][..], |jar| jar.cookies.as_slice());
    let seeded = if tab.kind.has(crate::caps::Tag::Jars) {
        tab.send(&HostToView::SetCookies(serde_json::to_string(cookies).unwrap_or_else(|_| "[]".to_owned())));
        crate::jar::TabCookies::seeded(cookies)
    } else {
        crate::jar::TabCookies::default()
    };
    if let Some(jar) = jar {
        self.cookies.jar_tabs.insert(id, (jar, seeded));
    }
}
```

Behaviours kept, each with a test (below):

- A jar engine with no jar (`jar: None`) still gets `SetCookies("[]")`, or it would sit out the 5 s wait. Today the closure sends that.
- A host without `Tag::Jars` gets no seed and an empty baseline, as today. Servo and WebKit ignore the message, but cookie values should not travel to an engine that has no use for them.
- A jar deleted during the spawn seeds empty, as a removed snapshot did.
- The seed is the host's first message. `spawn_tab` sends nothing else, the tab is not in `nodes` until this arm inserts it, and this call comes before the arm's `SetMuted`.
- An early `return` in `handle_tab_spawn_result` (stale session generation, tab closed while waking) drops the `ViewTab`, so no seed is needed.

**A clear during a spawn needs no handling.** The seed is read after the clear, so it is empty and the host never held what was cleared. `jar_reseed` stays for what it is really for: hosts already running at a clear.

**State.** `jars: BTreeMap<NodeId, Jar>` moves from `App` to `CookieState`, under its existing name. `CookieState::forget` excuses it by name, as it does `jar_dirty`, because `forget_jar_memory` saves before it removes (file I/O, so it stays an `App` method). The `jars` row leaves `EXCUSED` in `main/tests.rs`, and `APP_FIELDS_MAX` drops from 61 to 60. `CookieState` keeps its name: renaming it `JarState` would touch every `self.cookies` site and add nothing.

The seed does not need this move: it would work with `jars` on `App`. The move is here because x1 chose it. The jars and their bookkeeping become one group with one owner, which was ARC3's goal, and `App` loses a field. It is its own commit, so it can be dropped without touching the rest.

`publish_jar` has one line left, so it is inlined at both callers (`after_jar_ephemeral_changed`, `ensure_jar_loaded`).

## Timing change

The seed now waits for the UI thread to process `TabSpawned`, where before it left as soon as the host connected. A host's margin is its engine start-up plus `COOKIE_SEED_WAIT` (5 s): both hosts connect before they create their engine (webview2 `main.rs:151` then `:163`, cef `main.rs:152` then `:186`), and the 5 s clock starts only after that (`host_env.rs:66-71`). If the seed misses that margin, the first navigation goes out without cookies. The seed is still applied when it arrives (webview2 `main.rs:326`, cef `main.rs:289`), so the next load is logged in. The symptom is `jar: no seed received` in the host's stderr.

Two things can use up the margin:

- **One long stall** of the UI thread, a modal file dialog being the likely case.
- **Queued landings.** Each landing also runs `seed_landing`, `apply_pending_local` and `save_session()`, which commits a journal transaction (`spawn_result.rs:106,110,166`). The seed is sent at the top of its own landing, but a session restore lands many tabs and each later host waits for the landings ahead of it.

No `COOKIE_SEED_WAIT` change is proposed. If the symptom appears, the fix is a longer wait, since a seed that is merely late costs nothing else.

UI-thread cost: serialising a jar and building its baseline, once per landing, instead of on a spawn thread. That this is small is an inference. Step 3 measures it with an ignored, release-profile test in `session_sync/tests/costs.rs`: 50 `SpawnPurpose::Restore` spawns on an `app_on` app (a real journal, so `save_session` is real) with one 5,000-cookie jar, timed from the first to the last `handle_tab_spawn_result`, recorded here. Fake tabs only queue the message, so this is the UI thread's landing cost and not delivery. Over 2 s stops the work and comes back as a decision, because that would be most of the margin.

## Searched

The deletion pass tried:

- **Removing `TabSpawnOutcome::jar`, resolving at landing.** Rejected. The expression could run again at landing, but `jar_tabs` must record the jar the host was **spawned with**: that is how `sweep_jar_switches` notices a jar edit made while the spawn was in flight. The jar loaded at spawn start is also the one to seed from.
- **Removing `ensure_jar_loaded` from `spawn_tab_async`, loading at landing.** Rejected: it puts a file read between connect and seed.
- **Leaving `jars` on `App`.** Works, and is smaller; see Design for why x1's move stays, as a separable commit.
- **Removing the `Tag::Jars` test by sending to every host.** Rejected: it sends cookie values to engines that discard them, one of them over WSL TCP.
- **Removing `jar_reseed`.** Not possible: running hosts still hold cleared cookies.
- **Removing `TabCookies::seeded`.** Not possible: `merge` needs the baseline.

Removed: `Snapshots` and everything listed under End-state fit.

## Tests

Written first, all with `ViewTab::fake` and `FakePeer::sent`:

- `a_host_lands_seeded_with_its_jar_as_it_is_then`: a pull changes the jar after the spawn started; the host's first message holds the new cookie and `jar_tabs` has it as baseline.
- `a_clear_during_a_spawn_seeds_the_host_empty`: replaces `a_clear_during_a_spawn_marks_the_host_when_it_lands`. Empty seed, no `jar_reseed` mark, and a cookie the site sets again afterwards reaches the jar through a pull, as any new cookie does. A fresh host has a fresh private profile, so it cannot hold the cleared cookie otherwise.
- `a_jar_engine_with_no_jar_is_seeded_empty`: `SetCookies("[]")`, no `jar_tabs` entry.
- `an_engine_without_jars_is_not_seeded`: nothing sent, `jar_tabs` entry with an empty baseline.
- `a_jar_deleted_during_a_spawn_seeds_empty`.
- `the_seed_is_the_first_message_a_muted_tab_gets`: through `handle_tab_spawn_result`, seed before `SetMuted`.

Adjusted: the seed assert at `main/tests.rs:300` goes (the line above it now asserts the same map); `app.jars` becomes `app.cookies.jars` (`main/tests.rs`, 8 lines); the 5 `TabSpawnOutcome` literals in `session_sync/tests*` lose `seeded` and `seed_clears`. Fake tabs of a jar kind now receive `SetCookies("[]")` first at landing; the assertions read so far use `.iter().any(..)`, and the `session_sync` run confirms none expects an exact sequence.

Guard: `jar_snapshots`, `Snapshots` and `seed_clears` join the needles in `deleted_names.rs` (step 2). `APP_FIELDS_MAX` is lowered in step 1.

End to end: the gate's per-tab jar probe (`crates/gate/src/jar.rs:565`) proves a first load stays logged in, so a `full` gate run is the check on real hosts. It runs once at the end, not per step.

## Order of work

1. Move `jars` into `CookieState`, with the `EXCUSED` row and `APP_FIELDS_MAX` in the same commit (`cargo unit` fails otherwise). Mechanical. Own commit.
2. Add `seed_landed_host` as a stub that keeps today's behaviour, write the tests against it, and watch their assertions fail. Then make it real, remove `seeded` and `seed_clears` from the outcome, and delete everything below. Own commit.
3. Measure the landing, record it in "Timing change", run lint, `cargo unit`, then the gate at tier `full` (it changes session and engine paths).

## End-state fit

| Change | Lands in | Replaces | Deletes |
|---|---|---|---|
| `seed_landed_host` | `tabs/jar_pull.rs`, where `note_spawned_jar` is | `note_spawned_jar`; the seed step of `spawn_tab` | the `stale` logic |
| `jars` | `jar::CookieState` | `App::jars` | its `EXCUSED` row, one `App` field |
| Spawn thread | `tabs.rs::spawn_tab_async` | the seed closure | `snapshots`, `RefCell`, `seeded`, `seed_clears` |
| `TabSpawnOutcome` | `tabs.rs` | | fields `seeded`, `seed_clears` |
| `spawn_tab` | `process_manager.rs` | | parameter `jar_json`, its last `Tag::Jars` block |
| `jar.rs` | | | `Snapshots` (6 methods), `CookieState::jar_snapshots` |
| Sync sites | `lifecycle.rs`, `jar_pull.rs` | | four `lock()` blocks, the pull's clone, `publish_jar` |
| Startup | `main.rs` | | the `jars` field and local, `Snapshots::new`, the `Arc`; `cookies` becomes `CookieState::default()` |

Doc comments that name the old shape are fixed in the same commit: `jar.rs` (`CookieState`, `jar_dirty`, `forget`), `node.rs:22`, `tabs.rs` (`TabSpawnOutcome`), `lifecycle.rs`, `manifold-view-protocol/src/messages.rs:254` ("right after connect") and `host_env.rs:63`. The `deleted_names` needles scan comments too, so a survivor fails the test. `PARKED.md:191` (a combined ARC3, ARC7 and ARC9 entry) drops only its ARC3 clause, and the restructuring plan's ARC3 item takes **Own plan** with a link here. Finished records stay as written: `docs/superpowers/specs/2026-10-07-wave-2-performance-design.md:175` and the compile-checked-id-forgetting spec and plan.
