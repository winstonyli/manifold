# Parked minors (failed-spawn cards follow-ups) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Close five small follow-ups left by the failed-spawn-cards work (items a-e below).

**Architecture:** Small edits to the failure-toast coalescer, the spawn-failure paths in `tabs.rs`, and the empty-canvas hint. No new modules.

**Tech Stack:** Rust, egui.

**Spec:** `docs/superpowers/specs/2026-10-06-failed-spawn-cards-design.md` (decisions 1, 4, 6). Parked list: Outcome section of `docs/superpowers/plans/2026-10-06-failed-spawn-cards.md`.

## Global Constraints

- Work on `main` (CLAUDE.md "Branches"); no worktree.
- `cargo fmt --all`; `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass; the pre-commit hook runs both (commit in the background, it can take 2+ minutes). Never `--no-verify`.
- Use `-j 8` on cargo. No test may touch the real `%APPDATA%\manifold` (use `temp_dir`, see CLAUDE.md).
- Python only via a script file and `python -P f.py < /dev/null`; never `python -`, heredoc python. Prefer the Edit tool.
- Do not dispatch subagents.

---

### Task 1: Items a-e

**Files:**
- Modify: `crates/manifold/src/failure_toasts.rs`, `crates/manifold/src/main.rs` (`queue_failure_toast`, ~2733), `crates/manifold/src/tabs.rs` (~568-577 failure toast; `failed_twin` ~630; `leave_crashed_card` ~935), `crates/manifold/src/graph_ui.rs` (~446 hint), tests in `session_sync/tests.rs` and `failure_toasts.rs`.
- Docs: spec decision 6 / README line about failed spawns only if their wording is now wrong.

Each item: write the failing test first, see it fail, fix, see it pass.

- [ ] **a. Coalescer key and count.** Today the merge key is the error text alone and the count is failures. Spec decision 6 says cause = engine + error text, and the count should be distinct failed *intents* (url, engine, group), so one intent failing N times reads as one failure (its own message), not "N tabs". Change `FailureNotices::push` to take the engine, the error text and an intent key; hold a set of intents per cause; a burst of one distinct intent flushes as its own message, N>1 as `"{N} tabs couldn't start: {error text}"`. Update `queue_failure_toast` and its caller (tabs.rs ~577; intent = url + engine + group). Unit tests in `failure_toasts.rs`: same intent x3 -> its message; same text on two engines -> two messages; 3 distinct intents -> "3 tabs ...".
- [ ] **b. Route `leave_crashed_card`'s cannot_start toast** (`"Couldn't start {engine} for {url}: {reason}"`, tabs.rs ~955) through `queue_failure_toast` with the same key scheme. The "Tab keeps crashing, left closed" toast stays `queue_toast`. Test: two cannot_start crashes with one reason within the window read as one "2 tabs ..." message (use the pure `failure_notices` field and `flush`, as existing toast tests do).
- [ ] **c. Repaint.** A failure queued in a frame after `build_save_toast` ran, with nothing else repainting, flushes late. `App` has `egui_ctx`; call `self.egui_ctx.request_repaint_after(failure_toasts::WINDOW)` in `queue_failure_toast`. (A test is not practical; say so in the report.)
- [ ] **d. Hint per level.** `graph_ui.rs` ~446 paints `EMPTY_HINT` when `self.nodes.is_empty()` (whole graph), so an empty group shows no hint. Show it whenever the current level (the nodes drawn at this level) has none. Find the existing "nodes at this level" value used by the rest of `build_graph_ui` rather than recomputing. Check the minimap early return and the test near graph_ui.rs:2182-2195 still make sense. Test: an app with a node in a group, viewing the empty root-or-sibling level / the empty group level, paints the hint; a level with a node does not.
- [ ] **e. M5: a failed card whose retry is in flight.** Retrying a failed card (`wake_tab`) sets it `UnloadedState::Waking`, so `failed_twin` no longer sees it; a Root/New failure of the same intent meanwhile makes a second failed card, and when the wake fails too there are two cards for one intent (spec decision 4: one). First write a test reproducing it (card fails, retry puts it in Waking, a second Root/New failure of the same url/engine/group lands, then the wake result fails: expect exactly one failed card). Fix at the shared place: when a failure lands for an intent that already has a failed card (including a Respawn/Wake failure whose card now has a twin), keep one and refresh its error. Do not set a Waking card to Failed early. Choose which card survives so the retried card's id stays valid; state the ruling in the report. If the fix would need more than ~40 lines or a new field, stop and report instead.
- [ ] Run `cargo test -j 8 -p manifold` (BelowNormal), `cargo fmt --all`, clippy as in Global Constraints, then commit (one commit, message "Failed-spawn follow-ups: toast count by intent, crash toast coalesced, per-level hint, retry twin").

## Outcome (2026-10-06)

Commit 2374736 plus a review fix. Review: spec PASS, quality APPROVE, 7 minors, no Critical/Important.
- **c dropped:** `request_repaint_after` called between frames is discarded (egui resets the delay when the next frame starts; no repaint callback is registered), so the call did nothing. Removed. The existing request in `build_save_toast` still flushes the burst. A real fix needs the event loop to schedule a wake; not done.
- **e ruling:** the retried (Wake/Respawn) card survives; the Root/New twin is recorded in Recently Closed and removed.
- **Parked:** `leave_crashed_card` can still leave two failed cards for one intent (no `failed_twin` check; predates this batch); Restore does not collapse persisted duplicate failed cards (`find_map` drops one twin); same-text failures on two engines read identically in the "N tabs" toast; the e test does not assert the refreshed error text or the Recently Closed entry; M4 (item f) still a design question.
