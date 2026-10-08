# Wave 0: tooling, build and script hygiene Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pin the toolchain, add CI and the workspace manifest/lint hygiene, make the `EXPORT*` debug files atomic, stop the engine hosts panicking on a bad initial URL, and clean up the probe scripts (no stdin python, no leaked processes or dirs, one shared `scripts/lib.sh`).

**Architecture:** Twelve small tasks, each one commit on `main`. Tasks 1-5 touch manifests, the README and a new workflow; Task 6 the smoke test and one python sheet step; Task 7 three Rust writers and the probes' sleeps; Tasks 8-11 move one helper family each into `scripts/lib.sh`; Task 12 the two hosts plus a new probe. Two full gates: after Task 7 (covers 1-7) and after Task 12 (covers 8-12).

**Tech Stack:** Rust (rustup, cargo workspace tables, clippy), GitHub Actions, bash (Git Bash/MSYS on Windows), Python 3 (probe helpers), WebView2 and Servo engine hosts.

**Spec:** `docs/superpowers/specs/2026-10-07-wave-0-tooling-design.md`. Facts below were read from the files named per task at `c8b7485`; line numbers are as of that commit and shift as tasks land, so each edit also quotes the text it replaces.

## Global Constraints

- Line numbers are as of `c8b7485`; earlier tasks (and earlier steps of the same task) shift them, so find each edit by the text it quotes.
- Before Task 1, commit this plan and the spec as they stand (`git add docs/superpowers/specs/2026-10-07-wave-0-tooling-design.md docs/superpowers/plans/2026-10-07-wave-0-tooling.md`, message `Docs: wave 0 spec and plan`; add `.superpowers/survey/verified-wave-0.md` too only if the user wants the survey tracked), so every task's commit can cite them and the Outcome commit is a diff.
- Work on `main`, no worktree (CLAUDE.md "Branches"). One commit per task (Task 12 adds a second, for the Outcome), ending with `Co-Authored-By: Claude <Model> 5.5 <noreply@anthropic.com>`, `<Model>` being the implementer's (Haiku, Sonnet or Opus). Do not push except in Task 5, and only after the user says yes.
- Run `cargo fmt --all` before each commit. `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass. `.githooks/pre-commit` runs both, but **only when a `.rs` or `rustfmt.toml` file is staged**; a task that stages only manifests, scripts or docs runs them by hand before committing. The hook takes 2+ minutes: run `git commit` in the background. Never `--no-verify`. Don't silence a lint without a reason.
- `-j 8` on every cargo call; builds, tests and probes at BelowNormal. Before Task 9 only `link-probe.sh`, `focus-loss-probe.sh` and `jar-probe.sh` set their own instance BelowNormal, so every other probe run before then (Task 7's chrome-shots, Task 8's baseline sweep) is launched detached at BelowNormal (`cmd //c start "" //b //belownormal bash -c '...'`; children inherit the priority class); from Task 9 on, `launch_headless` does it. Check `df -h /c` before any build or toolchain download: C: is about 94% full (56-59 G free on 2026-10-07; the verified file's ~98% / 22 G was 2026-09-30). Stop and report if under 10 G.
- No test may touch the real `%APPDATA%\manifold`: `App::default()` is safe; files go under `store::testutil::temp_dir`.
- Python: only from a script file or `-c`, always `python -P ... </dev/null`. Never `python -`, never a heredoc into python, never a no-op python call. Subagents must not dispatch subagents; a subagent prompt that may edit files repeats this python rule.
- Edition 2021: no let-chains.
- `scripts/focus-loss-probe.sh` steals the OS foreground for a few seconds per case. Tell the user and get a yes before each run of it; if they decline, leave it out and say so in the Outcome.
- Gate (CLAUDE.md "Gate"): `cargo build -j 8 -p manifold` first, then `cmd //c start "" //b //belownormal bash scripts/gate.sh LOG full`, reporting the CPU load and Defender state it records. Each task names the gate level its change needs; the two full gates (Tasks 7 and 12) cover every task before them, so no other task runs a gate of its own (Tasks 3 and 4 are full by CLAUDE.md's wording and say they rely on Task 7's).
- Spec values: toolchain `nightly-2026-09-23`, no `rust-version`; `ipc-protocol = { path = "crates/ipc-protocol" }`; lint `allow_attributes_without_reason = "warn"` (never `allow_attributes`); `WAIT_FILE_SECS` default 60, `chrome-shots.sh` 30; fallback page `about:blank`.

## File structure

| File | Task | Responsibility |
|---|---|---|
| `rust-toolchain.toml` (new) | 1 | The pinned nightly |
| `Cargo.toml` | 2, 3, 4 | `[workspace.dependencies]`, `[workspace.lints.clippy]` |
| `crates/*/Cargo.toml` | 2, 3, 4 | `ipc-protocol.workspace = true`, optional `windows.workspace = true`, `[lints] workspace = true` |
| 12 Rust files with an `#[allow]` | 4 | `reason = "..."` on each |
| `.github/workflows/ci.yml` (new) | 5 | Two CI jobs |
| `scripts/smoke_eval.py` (new) | 6 | The smoke test's five python checks |
| `scripts/shots_sheet.py` (new) | 6 | chrome-shots' crop and contact-sheet step |
| `scripts/headless-smoke-test.sh` | 6 | Calls `smoke_eval.py`; `kill_tree` cleanup |
| `scripts/export_state.py` | 6 | Docstring names `smoke_eval.py` as a user |
| `crates/manifold/src/{debug_control,session_sync,main}.rs`, `session_sync/tests.rs` | 7 | Atomic exports and their tests |
| `scripts/chrome-shots.sh` | 6, 7, 8-10 | Sheet step via `shots_sheet.py`; `state()` sleep removed; lib.sh helpers |
| `scripts/{focus-loss-probe,link-probe}.sh` | 7, 8-11 | Sleeps removed; then lib.sh helpers |
| `scripts/lib.sh`, `scripts/lib-test.sh` (new) | 8-11 | Shared probe helpers and their check |
| 7 more probes (`failed-spawn`, `hscroll`, `jar`, `key`, `page-kind`, `scale-test`, `sleep`) | 8, 9 | lib.sh seed and launcher |
| `crates/manifold-engine-{servo,webview2}/src/main.rs` | 12 | `about:blank` fallback |
| `scripts/bad-url-probe.sh` (new) | 12 | BT-5's end-to-end check |
| `README.md` | 1, 2, 5, 7, 8-12 | Docs for each change |

Order and dependencies: 1 → 2 → 3 (optional, needs 2) → 4 → 5 (needs 1: CI installs the pinned toolchain) → 6 → 7 (its chrome-shots run needs 6's stdin-free sheet step) → 8 → 9 → 10 → 11 (each edits the same probes; 10 also needs 7's sleeps gone) → 12 (its probe uses lib.sh from 8-11).

---

### Task 1: Pin the toolchain

Model: **haiku**. Gate: **light** (no product code). Proof: `rustup show active-toolchain` names the pin, and the hook's clippy finds every unit `Fresh`.

**Files:**
- Create: `rust-toolchain.toml`
- Modify: `README.md:26` (Prerequisites), `README.md:53` (Building, WebKit paragraph)

**Interfaces:**
- Consumes: nothing.
- Produces: `rust-toolchain.toml` with `channel = "nightly-2026-09-23"`; Task 5's CI installs it with `rustup toolchain install`.

- [ ] **Step 1: Check space and the current toolchain**

Run: `df -h /c | tail -1; rustup show active-toolchain; rustc -vV`
Expected: at least 10 G free; `nightly-x86_64-pc-windows-msvc (default)`; `commit-hash: 6bb1652a0...`, `commit-date: 2026-09-22`. Save the `rustc -vV` output to compare in Step 4.
Run: `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` under the rolling nightly, so every unit is up to date before the pin and Step 4's `Fresh` count measures only the toolchain change.

- [ ] **Step 2: Write `rust-toolchain.toml`**

```toml
[toolchain]
channel = "nightly-2026-09-23" # the nightly in use since 2026-09-23; bump on purpose, never implicitly
components = ["rustfmt", "clippy"]
profile = "minimal"
```

Do not add `rust-version`. Do not uninstall the rolling `nightly`: other projects on this machine use it as their default.

- [ ] **Step 3: Install and confirm the override**

Run from the repo root: `rustup toolchain install` (no argument: it installs the toolchain `rust-toolchain.toml` names, a few hundred MB), then `rustup show active-toolchain`.
Expected: `nightly-2026-09-23-x86_64-pc-windows-msvc (overridden by '...\rust-toolchain.toml')`.

- [ ] **Step 4: Prove the compiler is the same one**

Run: `rustc -vV` and compare with Step 1: same `commit-hash` and `release`.
Run: `cargo clippy -v -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings 2>&1 | tee target/wave0-t1-clippy.log | grep -cE '^ +Fresh'` then `grep -cE '^ +(Checking|Compiling) ' target/wave0-t1-clippy.log`.
Expected: many `Fresh`, `0` Checking/Compiling, exit 0. If units rebuild, the dated install is not byte-identical to the rolling one: let it finish, record the count and both `rustc -vV` outputs in the Outcome. Then run `cargo fmt --all --check` (expected: no output).

- [ ] **Step 5: README**

In `README.md`, replace line 26 `- **Rust** (stable), via rustup.` with:

```markdown
- **Rust**: the toolchain `rust-toolchain.toml` pins (rustup installs it on first use). Nothing needs nightly features; it is pinned so a clippy update can't fail the `-D warnings` hook with no code change.
```

Append to the end of line 53 (`Adjust the path for wherever this repo actually lives on your machine (WSL2 sees your Windows drives under `/mnt/c/...`).`):

```markdown
 `rust-toolchain.toml` applies there too: the first build makes WSL's rustup install the Linux build of the same nightly, and `target-linux` rebuilds once if WSL was on another toolchain.
```

- [ ] **Step 6: Commit**

```bash
git add rust-toolchain.toml README.md
git commit -m "Toolchain: pin nightly-2026-09-23 in rust-toolchain.toml

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `ipc-protocol` in `[workspace.dependencies]`

Model: **haiku**. Gate: **light**. Proof: `cargo tree` identical before and after, `Cargo.lock` unchanged.

**Files:**
- Modify: `Cargo.toml` (after `description = ...`, line 15)
- Modify: `crates/manifold/Cargo.toml:30`, `crates/manifold-engine-cef/Cargo.toml:10`, `crates/manifold-engine-servo/Cargo.toml:10`, `crates/manifold-engine-webkit/Cargo.toml:9`, `crates/manifold-engine-webview2/Cargo.toml:8`
- Modify: `README.md:345`

**Interfaces:**
- Consumes: Task 1's toolchain.
- Produces: a `[workspace.dependencies]` table in the root `Cargo.toml` that Task 3 adds to.

- [ ] **Step 1: Record the tree before**

Run: `cargo tree --workspace -e normal,build --prefix none | sort > target/wave0-tree-before.txt`

- [ ] **Step 2: Root table**

In `Cargo.toml`, after the line `description = "A browser where each tab can run on a different rendering engine ..."` (end of `[workspace.package]`), insert:

```toml

[workspace.dependencies]
ipc-protocol = { path = "crates/ipc-protocol" } # path only: nothing is published, so no version to bump
```

- [ ] **Step 3: The five dependents**

In each of the five crate manifests listed above, replace the line
`ipc-protocol = { version = "0.3.0", path = "../ipc-protocol" }`
with
`ipc-protocol.workspace = true`.
`crates/manifold/Cargo.toml` starts with a UTF-8 BOM (bytes `EF BB BF`); edit only line 30 and confirm with `head -c 3 crates/manifold/Cargo.toml | od -c` that the BOM is still there.

- [ ] **Step 4: Prove nothing changed**

Run: `cargo tree --workspace -e normal,build --prefix none | sort > target/wave0-tree-after.txt; diff target/wave0-tree-before.txt target/wave0-tree-after.txt && git diff --exit-code Cargo.lock && echo SAME`
Expected: `SAME`.
Run (the hook won't, no `.rs` is staged): `cargo fmt --all --check && cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings`
Expected: exit 0.

- [ ] **Step 5: README**

In `README.md:345`, replace `(workspace `version` and the `ipc-protocol` requirements in the crate manifests, plus `Cargo.lock`)` with `(workspace `version`, plus `Cargo.lock`)`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/*/Cargo.toml README.md
git commit -m "Workspace: ipc-protocol is a path-only [workspace.dependencies] entry

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

---

### Task 3 (optional): one shared `windows` entry

Model: **haiku**. Gate: **full** by CLAUDE.md's wording (it changes the WebView2 host's `windows` features), covered by Task 7's gate; no gate of its own. Proof: `windows`' feature list is the same for `-p manifold` as for the hook's set. **Ask the user first** ("Unify the `windows` crate's features across manifold, CEF and WebView2? Alternating `-p manifold` and the hook already stays `Fresh` once both are built (verified #9); this removes only the one extra recheck of `windows`, `wgpu-*`, `egui-wgpu` and `manifold` per selection after a dependency or toolchain change, and one copy on disk. It costs `manifold` compiling `windows` modules it doesn't use. Value: low."). If they decline, skip this task and say so in the Outcome.

**Files:**
- Modify: `Cargo.toml` (`[workspace.dependencies]` from Task 2)
- Modify: `crates/manifold/Cargo.toml:99`, `crates/manifold-engine-cef/Cargo.toml:16`, `crates/manifold-engine-webview2/Cargo.toml:21-52`

**Interfaces:**
- Consumes: Task 2's `[workspace.dependencies]` table.
- Produces: nothing later tasks use.

- [ ] **Step 1: Record the split**

Run: `cargo tree -p manifold -e normal --prefix none -f '{p}|{f}' | grep '^windows v0.62' | sort -u > target/wave0-win-manifold.txt; cargo tree -p manifold -p ipc-protocol -p manifold-engine-webview2 -e normal --prefix none -f '{p}|{f}' | grep '^windows v0.62' | sort -u > target/wave0-win-hook.txt; diff target/wave0-win-manifold.txt target/wave0-win-hook.txt`
Expected: they differ (36 against 53 features). The hook's 53 are the union of every requester in that graph, not only the three manifests: `webview2-com` and `webview2-com-sys` 0.39.1 also ask for `Win32_Globalization`, `Win32_System_Ole` and `Win32_System_Variant` (their `Cargo.toml` `[dependencies.windows]`), which is why the list below has them.

- [ ] **Step 2: The shared entry**

Append to `[workspace.dependencies]` in `Cargo.toml` (the comment above `webview2-com` in `crates/manifold-engine-webview2/Cargo.toml` stays where it is; it explains that pin too). The list is the three manifests' features plus webview2-com's three. Before saving, check it against Step 1's `target/wave0-win-hook.txt`: every feature named there must be in the list or implied by one in it (`Win32`, `Win32_System`, `Win32_Graphics`, `Win32_UI`, `Graphics`, `UI` and similar parents are implied); add any other one, and say so in the Outcome.

```toml
# Pinned to match webview2-com's own workspace pin (wravery/webview2-rs Cargo.toml) exactly -- COM/WinRT
# interface types from a mismatched `windows` version are not interchangeable with webview2-com's. One
# feature list for every crate (including the three webview2-com itself asks for), so `-p manifold` and the
# hook's `-p manifold -p ipc-protocol -p manifold-engine-webview2` build the same `windows`: one copy on
# disk, and one recheck instead of two after a dependency or toolchain change.
windows = { version = "0.62", features = [
    "Win32_Foundation",
    "Win32_Globalization", # webview2-com
    "Win32_System_Ole", # webview2-com
    "Win32_System_Variant", # webview2-com
    "Win32_System_Com",
    "Win32_System_LibraryLoader",
    "Win32_System_JobObjects",
    "Win32_System_Threading",
    "Win32_Security",
    "Win32_Security_Cryptography",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_HiDpi",
    "Win32_UI_Shell",
    "Win32_Graphics_Gdi",
    "Win32_System_WinRT",
    "Win32_System_WinRT_Direct3D11",
    "Win32_System_WinRT_Graphics",
    "Win32_System_WinRT_Graphics_Capture",
    "Win32_Graphics_Direct3D",
    "Win32_Graphics_Direct3D11",
    "Win32_Graphics_Dxgi",
    "Win32_Graphics_Dxgi_Common",
    "Foundation",
    "Graphics_Capture",
    "Graphics_DirectX",
    "Graphics_DirectX_Direct3D11",
    "UI_Composition",
    "UI_Composition_Desktop",
    "Win32_System_WinRT_Composition",
    "System",
    "Win32_UI_Input_KeyboardAndMouse",
] }
```

- [ ] **Step 3: The three users**

- `crates/manifold/Cargo.toml`: replace line 99 `windows = { version = "0.62", features = ["Win32_UI_HiDpi", "Win32_Security_Cryptography"] }` with `windows.workspace = true` (it stays under `[target.'cfg(windows)'.dependencies]`; keep its comment lines 97-98).
- `crates/manifold-engine-cef/Cargo.toml`: replace line 16 `windows = { version = "0.62", features = ["Win32_Foundation", "Win32_Security", "Win32_System_JobObjects", "Win32_System_Threading"] }` with `windows.workspace = true`.
- `crates/manifold-engine-webview2/Cargo.toml`: replace the whole `windows = { version = "0.62", features = [ ... ] }` entry (lines 21-52, from `windows = {` through `] }`, including its inner comment) with `windows.workspace = true`.

- [ ] **Step 4: Prove the split is gone**

Run Step 1's two `cargo tree` commands again, then `diff target/wave0-win-manifold.txt target/wave0-win-hook.txt && echo SAME`.
Expected: `SAME`, and the list (which includes implied parent features such as `Win32`) contains `Win32_Security_Cryptography`, `UI_Composition` and `Win32_System_Variant`. If not `SAME`, the diff names the missing features: add them to the shared list and rerun this step (do not commit a list that still splits).
Run: `cargo fmt --all --check && cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` (expected: exit 0; the first run rechecks `windows` and its users once).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/manifold/Cargo.toml crates/manifold-engine-cef/Cargo.toml crates/manifold-engine-webview2/Cargo.toml
git commit -m "Workspace: one windows feature list, so -p manifold and the hook share one build

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `allow_attributes_without_reason`

Model: **haiku**. Gate: **full** by CLAUDE.md's wording (it touches `mirror.rs` and `session_sync/tests.rs`, attributes only), covered by Task 7's gate; no gate of its own. Proof: with the lint on, clippy fails on the hook-set sites (12, or 11 if the `cfg_attr` one is not seen); after the reasons, it passes.

**Files:**
- Modify: `Cargo.toml`; all six `crates/*/Cargo.toml`
- Modify: `crates/ipc-protocol/src/messages.rs:587-588`, `crates/manifold/src/card.rs:653-654` and `:798-799`, `crates/manifold/src/gpu.rs:1028`, `crates/manifold/src/graph_ui.rs:1618`, `crates/manifold/src/json_file.rs:130-132`, `crates/manifold/src/json_level_ui.rs:262`, `crates/manifold/src/local_state.rs:87`, `crates/manifold/src/mirror.rs:270-271`, `crates/manifold/src/popups.rs:39`, `crates/manifold/src/session_sync/tests.rs:3251-3253`, `crates/manifold/src/engine_install.rs:436`, `crates/manifold-engine-cef/src/client.rs:1-3`

**Interfaces:**
- Consumes: Task 2's root `Cargo.toml` layout (the new table goes after `[workspace.dependencies]`).
- Produces: every later `#[allow]` must carry `reason = "..."`.

- [ ] **Step 1: Turn the lint on (the failing test)**

In the root `Cargo.toml`, insert directly after the `[workspace.dependencies]` table (before the `[profile...]` tables):

```toml

[workspace.lints.clippy]
# An #[allow] must say why. Not `allow_attributes` as well: it pushes toward #[expect], which on cfg-dependent
# code (the Windows-only read-only tests, `cfg_attr(not(unix), ...)`) goes unfulfilled on one target and fails
# -D warnings there.
allow_attributes_without_reason = "warn"
```

In each of the six crate manifests, after line 5 `description.workspace = true`, insert (line 6 is blank and line 7 is `[dependencies]` in all six, so `[package]` keeps all its keys):

```toml

[lints]
workspace = true
```

- [ ] **Step 2: Watch it fail**

Run: `cargo clippy --keep-going -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings 2>&1 | tee target/wave0-t4-before.log | grep -c 'without specifying a reason'` then `grep -A3 'without specifying a reason' target/wave0-t4-before.log | grep -oE 'crates[\\/][^ ]+\.rs:[0-9]+' | sort -u`
Expected: a nonzero count (sites in non-test code are reported once per target), and these 12 locations, or 11 without `engine_install.rs:436` if the lint does not fire on an allow that `cfg_attr` expands (unverified; record which in the Outcome, and give that site its `reason` either way): `messages.rs:588`, `card.rs:654`, `card.rs:799`, `gpu.rs:1028`, `graph_ui.rs:1618`, `json_file.rs:132`, `json_level_ui.rs:262`, `local_state.rs:87`, `mirror.rs:271`, `popups.rs:39`, `session_sync/tests.rs:3253` (or `session_sync\tests.rs`), `engine_install.rs:436`. The 13th site, `manifold-engine-cef/src/client.rs:3`, is outside the hook's crates.

- [ ] **Step 3: Give each site its reason**

Each existing `//` comment moves into `reason` (delete the comment line); doc comments (`///`) stay.

`crates/ipc-protocol/src/messages.rs:587-588`, replace
```rust
    // The macro also emits `ALL`, `dom_code` and `windows_vk`, which this one-key table never calls.
    #[allow(dead_code)]
```
with
```rust
    #[allow(dead_code, reason = "the macro also emits `ALL`, `dom_code` and `windows_vk`, which this one-key table never calls")]
```

`crates/manifold/src/card.rs:653-654` and again `:798-799`, replace each
```rust
// Free function: callers hold no CardPass, and a struct for one call site would only rename the arguments.
#[allow(clippy::too_many_arguments)]
```
with
```rust
#[allow(
    clippy::too_many_arguments,
    reason = "free function: callers hold no CardPass, and a struct for one call site would only rename the arguments"
)]
```

`crates/manifold/src/gpu.rs:1028`, replace
```rust
    #[allow(clippy::too_many_arguments)] // Disjoint borrows of `GpuState` fields: `self` can't be passed whole.
```
with
```rust
    #[allow(clippy::too_many_arguments, reason = "disjoint borrows of `GpuState` fields: `self` can't be passed whole")]
```

`crates/manifold/src/graph_ui.rs:1618` (`draw_graph_cards`; its one caller is `build_graph_ui`'s Scene closure at `:588`), replace
```rust
    #[allow(clippy::too_many_arguments)]
```
with
```rust
    #[allow(
        clippy::too_many_arguments,
        reason = "one call site, in `build_graph_ui`'s Scene closure: these are that frame's locals, and a struct would only rename them"
    )]
```

`crates/manifold/src/json_file.rs:130-132`, replace
```rust
            // The Windows read-only attribute makes the rename over it
            // fail; on Unix this is a temp file this test owns.
            #[allow(clippy::permissions_set_readonly_false)]
```
with
```rust
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "the Windows read-only attribute makes the rename over it fail; on Unix this is a temp file this test owns"
            )]
```

`crates/manifold/src/json_level_ui.rs:262`, replace
```rust
#[allow(clippy::too_many_arguments)] // one card's full description; a struct for it would only rename these
```
with
```rust
#[allow(clippy::too_many_arguments, reason = "one card's full description; a struct for it would only rename these")]
```

`crates/manifold/src/local_state.rs:87`, replace
```rust
#[allow(clippy::type_complexity)] // mirrors the field's own type
```
with
```rust
#[allow(clippy::type_complexity, reason = "mirrors the field's own type")]
```

`crates/manifold/src/mirror.rs:270-271`, replace
```rust
    // Named after the field it reads (`from_json`), as the other accessors are; it converts nothing.
    #[allow(clippy::wrong_self_convention)]
```
with
```rust
    #[allow(
        clippy::wrong_self_convention,
        reason = "named after the field it reads (`from_json`), as the other accessors are; it converts nothing"
    )]
```

`crates/manifold/src/popups.rs:39`, replace
```rust
#[allow(clippy::too_many_arguments)] // egui builder: each is a plain knob
```
with
```rust
#[allow(clippy::too_many_arguments, reason = "egui builder: each is a plain knob")]
```

`crates/manifold/src/session_sync/tests.rs:3251-3253`, replace
```rust
        // The Windows read-only attribute makes the append fail; on Unix this would make the file writable by
        // everyone, but it's a temp file this test owns.
        #[allow(clippy::permissions_set_readonly_false)]
```
with
```rust
        #[allow(
            clippy::permissions_set_readonly_false,
            reason = "the Windows read-only attribute makes the append fail; on Unix this would make the file writable by everyone, but it's a temp file this test owns"
        )]
```

`crates/manifold/src/engine_install.rs:436`, replace
```rust
#[cfg_attr(not(unix), allow(dead_code))] // only applied on Unix; tested everywhere
```
with
```rust
#[cfg_attr(not(unix), allow(dead_code, reason = "only applied on Unix; tested everywhere"))]
```

`crates/manifold-engine-cef/src/client.rs:1-3`, replace
```rust
// `wrap_client!`'s generated `new` and `ClientBuilder::build` take one argument per
// handler or shared handle; bundling them into a struct would only rename the list.
#![allow(clippy::too_many_arguments)]
```
with
```rust
#![allow(
    clippy::too_many_arguments,
    reason = "`wrap_client!`'s generated `new` and `ClientBuilder::build` take one argument per handler or shared handle; bundling them into a struct would only rename the list"
)]
```

- [ ] **Step 4: Watch it pass**

Run: `cargo fmt --all` then `cargo clippy --keep-going -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings 2>&1 | grep -c 'without specifying a reason'`
Expected: `0`, and clippy exits 0 (run it once more without the pipe to see the exit code).
CEF (outside the hook): `sed -n '1,4p' crates/manifold-engine-cef/src/client.rs` shows the `reason`. Only if `ls target/debug/build | grep -q '^cef-dll-sys'` (CEF already built here), also run `cargo clippy -j 8 -p manifold-engine-cef -- -D warnings`; stop it if it starts compiling `cef-dll-sys` from scratch, and note that in the Outcome.
Run: `cargo test -j 8 -p manifold -p ipc-protocol` (expected: all pass; nothing but attributes changed).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates
git commit -m "Lints: every #[allow] gives a reason (allow_attributes_without_reason)

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Minimal CI

Model: **haiku**. Gate: **light** (no product code). Proof: the first pushed run of both jobs (`gh run watch`). Needs Task 1.

**Files:**
- Create: `.github/workflows/ci.yml`
- Modify: `README.md:18`, plus a new paragraph after it

**Interfaces:**
- Consumes: `rust-toolchain.toml` (Task 1); `rustup toolchain install` with no argument installs it (rustup 1.28+).
- Produces: nothing later tasks use.

- [ ] **Step 1: The workflow**

```yaml
name: ci
on: [push, pull_request]
jobs:
  windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - run: rustup toolchain install # installs rust-toolchain.toml's toolchain (rustup 1.28+)
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings
      - run: cargo test -p manifold -p ipc-protocol -p manifold-engine-webview2
  linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: rustup toolchain install
      - uses: Swatinem/rust-cache@v2
      - run: cargo test -p manifold -p ipc-protocol # catches cfg(windows) leaks; no apt step: neither crate needs GTK
```

- [ ] **Step 2: README**

In `README.md:18`, replace the sentence `Linux needs GTK 3 development files for `rfd`.` with `` `manifold` and `ipc-protocol` need no GTK on Linux: `rfd` 0.15 resolves to the XDG portal there, and only `manifold-engine-webkit` pulls `gtk-sys`. ``
After line 18, insert a blank line and:

```markdown
**CI** (`.github/workflows/ci.yml`) runs on every push and pull request: on Windows, `cargo fmt --all --check`, the pre-commit hook's clippy, and `cargo test` for the hook's three crates; on Linux, `cargo test -p manifold -p ipc-protocol`. CEF (a download, CMake, ninja), Servo (a 33-minute build), WebKit (WSL) and the probes (a desktop and a GPU) stay local.
```

- [ ] **Step 3: Check and commit**

Run: `cargo fmt --all --check` (nothing staged is Rust; expected: no output).

```bash
git add .github/workflows/ci.yml README.md
git commit -m "CI: Windows fmt/clippy/test and Linux test on every push

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 4: First run (only with the user's yes)**

Ask the user: "Push `main` to origin so CI runs for the first time? It sends Tasks 1-5 before the first full gate (Task 7); the alternative is to push after that gate." Only on yes: `git push origin main`, then `gh run list --limit 1` and `gh run watch <id>` (one run holds both jobs).
Expected: Windows green (same commands as the hook, plus tests). A test that fails only on Linux, or only on the runner, is a finding: record it in the Outcome and ask the user how to proceed; do not skip or `#[ignore]` it. If the user declines the push, record "CI not yet run" in the Outcome.

---

### Task 6: Smoke-test hygiene (BT-4)

Model: **sonnet**. Gate: **light** (scripts only). Proof: no stdin python left in `scripts/`; `smoke_eval.py` gives the old answers on fixtures; a whole smoke run passes; an interrupted one leaves no `manifold.exe` and no profile dir, compared against the same interrupt with the old `cleanup()`.

**Files:**
- Create: `scripts/smoke_eval.py`, `scripts/shots_sheet.py`
- Modify: `scripts/headless-smoke-test.sh` (`:39-42`, `:121-137`, `:172-177`, `:252-275`, `:396-400`, `:414-427`, `:569-577`)
- Modify: `scripts/chrome-shots.sh:662-688`
- Modify: `scripts/export_state.py:1-3` (docstring)
- Delete: the ten `target/headless-test-appdata-*` dirs (untracked)

**Interfaces:**
- Consumes: `kill_tree <msys pid>` from `scripts/winpid.sh` (no-op if the pid is gone; else `taskkill //F //T` of its Windows pid).
- Produces: `python -P scripts/smoke_eval.py <state|query|nine|border|converted> ARGS... </dev/null`, same stdout and exit status as the heredoc it replaces.

- [ ] **Step 1: `scripts/smoke_eval.py`**

Each branch is the heredoc body it replaces, unchanged except for indentation:

```python
"""The headless smoke test's python checks (scripts/headless-smoke-test.sh), one subcommand each.

Usage: python -P scripts/smoke_eval.py <state|query|nine|border|converted> ARGS... </dev/null

Each branch is the heredoc it replaced, unchanged and at module level, so the expressions it `eval`s see the
same names. The subcommand is popped off sys.argv first, so sys.argv[1], [2]... are the heredoc's arguments.
`query` imports scripts/export_state.py through PYTHONPATH, which the smoke test exports.
"""
import json
import os
import sys
from urllib.parse import parse_qs, urlparse

cmd = sys.argv.pop(1)

if cmd == "state":
    # state NAME EXPR [url]: EXPR over the EXPORTSTATE file; prints it (or the focused URL), exit 0 if truthy.
    s = json.load(open(sys.argv[1], encoding="utf-8"))
    url = s["focused_url"] or ""
    q = parse_qs(urlparse(url).query).get("q", [""])[0]
    row = os.environ.get("ROW", "")
    v = eval(sys.argv[2])
    print(url if sys.argv[3] == "url" else v)
    sys.exit(0 if v else 1)
elif cmd == "query":
    # query FILE EXPR: prints EXPR over the cards (see `query` in the smoke test).
    from export_state import *

    s = json.load(open(sys.argv[1], encoding="utf-8"))
    cards = {c["id"]: c for c in s["cards"]}
    tabs = sorted(i for i, c in cards.items() if card_is(c, TAB))
    groups = sorted(i for i, c in cards.items() if card_is(c, GROUP))
    W, H, TOP = 816, 639, 40  # RESIZE below; the graph toolbar's height, rounded up

    def pt(i, x, y):
        if not (0 <= x < W and TOP <= y < H):
            sys.stderr.write(f"query: {x:.0f},{y:.0f} ({label(cards[i])} {i}) is off the graph canvas\n")
        return "%.0f %.0f" % (x, y)

    xy = lambda i: pt(i, *cards[i]["screen"])
    lt = lambda i: pt(i, cards[i]["rect"][0] + 3, cards[i]["rect"][1] + 3)
    r = [cards[i]["rect"] for i in tabs]
    bbox = "%.0f %.0f %.0f %.0f" % (max(0, min(x[0] for x in r) - 20), max(TOP, min(x[1] for x in r) - 20),
                                    min(W - 1, max(x[2] for x in r) + 20), min(H - 1, max(x[3] for x in r) + 20)) if r else ""
    env = os.environ
    print(eval(sys.argv[2]))
elif cmd == "nine":
    # nine FILE: writes a group "Nine" of nine tabs, for IMPORTGROUP.
    tab = lambda i: {"id": f"t{i}", "type": "application/vnd.manifold.tab", "url": "manifold://welcome", "engine": "webview2", "group": "g"}
    json.dump({"nodes": [{"id": "g", "type": "application/vnd.manifold.group", "name": "Nine"}] + [tab(i) for i in range(9)]}, open(sys.argv[1], "w"))
elif cmd == "border":
    # border FILE PNG ID RGB: exit 0 if a pixel on ID's left edge, at mid-height, is within 40 of RGB.
    from PIL import Image

    cards = {c["id"]: c for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"]}
    img = Image.open(sys.argv[2]).convert("RGB")
    left, top, _, bottom = cards[sys.argv[3]]["rect"]
    y = round((top + bottom) / 2)
    want = eval(sys.argv[4])
    near = min(sum((a - b) ** 2 for a, b in zip(img.getpixel((x, y)), want)) ** 0.5 for x in range(round(left) - 1, round(left) + 3))
    sys.exit(0 if near < 40 else 1)
elif cmd == "converted":
    # converted FILE NAME KIND: exit 0 if a KIND node's file:// url ends in /NAME and no tab is there.
    nodes = json.load(open(sys.argv[1], encoding="utf-8"))["nodes"]
    KINDS = {"json": "application/json", "pdf": "application/pdf"}
    at = lambda n: n.get("url", "").startswith("file://") and n.get("url", "").endswith("/" + sys.argv[2])
    sys.exit(0 if any(n["type"] == KINDS[sys.argv[3]] and at(n) for n in nodes) and not any(n["type"] == "application/vnd.manifold.tab" and at(n) for n in nodes) else 1)
else:
    sys.exit(f"smoke_eval: unknown subcommand {cmd!r}")
```

- [ ] **Step 2: Check it offline against fixtures**

Run in bash from the repo root:

```bash
d="$(mktemp -d)"; export PYTHONPATH="$(cygpath -w scripts)"; ev() { python -P scripts/smoke_eval.py "$@" </dev/null; }
printf '{"focused_url": "https://x/?q=hi", "tab_view": true}' >"$d/s.json"
ev state "$(cygpath -w "$d/s.json")" 'q == "hi"' url; echo "exit $?"                       # https://x/?q=hi / exit 0
ev state "$(cygpath -w "$d/s.json")" 'not s["tab_view"]' ''; echo "exit $?"                # False / exit 1
printf '{"cards": [{"id": "a", "type": "application/vnd.manifold.tab", "loaded": true, "json_card": false, "screen": [100, 100], "rect": [50, 60, 150, 140]}]}' >"$d/q.json"
ev query "$(cygpath -w "$d/q.json")" 'len(tabs), xy("a"), bbox'                            # (1, '100 100', '30 40 170 160')
ev nine "$(cygpath -w "$d/nine.json")"; python -P -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["nodes"]))' "$(cygpath -w "$d/nine.json")" </dev/null   # 10
printf '{"nodes": [{"id": "n", "type": "application/json", "url": "file:///C:/x/conv.json"}]}' >"$d/c.json"
ev converted "$(cygpath -w "$d/c.json")" conv.json json; echo "exit $?"                    # exit 0
printf '{"nodes": [{"id": "n", "type": "application/vnd.manifold.tab", "url": "file:///C:/x/conv.json"}]}' >"$d/c2.json"
ev converted "$(cygpath -w "$d/c2.json")" conv.json json; echo "exit $?"                   # exit 1
if python -P -c "import PIL" </dev/null 2>/dev/null; then
  python -P -c 'import sys; from PIL import Image; im = Image.new("RGB", (20, 20)); [im.putpixel((5, y), (196, 87, 58)) for y in range(20)]; im.save(sys.argv[1])' "$(cygpath -w "$d/b.png")" </dev/null
  printf '{"cards": [{"id": "g", "rect": [5, 0, 15, 20]}]}' >"$d/b.json"
  ev border "$(cygpath -w "$d/b.json")" "$(cygpath -w "$d/b.png")" g "(196, 87, 58)"; echo "exit $?"    # exit 0
  ev border "$(cygpath -w "$d/b.json")" "$(cygpath -w "$d/b.png")" g "(215, 162, 74)"; echo "exit $?"   # exit 1
fi
ev bogus; echo "exit $?"                                                                     # smoke_eval: unknown subcommand 'bogus' / exit 1
rm -rf "$d"
```
Expected: each line's output as its comment says.

- [ ] **Step 3: Point the smoke test at it**

In `scripts/headless-smoke-test.sh` (line numbers before item 1's insertion; find each by its quoted text):

1. After line 39 `. "$repo_root/scripts/fresh-exe.sh"`, insert:
```bash
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
```
2. Replace line 41 `# The inline python below reads EXPORTSTATE cards through scripts/export_state.py.` with `# scripts/smoke_eval.py's checks read EXPORTSTATE cards through scripts/export_state.py.`
3. In `state()`, replace the heredoc call (lines 126-136, `python - "$path" "$2" "${3:-}" <<'EOF'` through `EOF`) with:
```bash
    python -P "$repo_root/scripts/smoke_eval.py" state "$path" "$2" "${3:-}" </dev/null
```
4. In `query()`, replace lines 253-274 (`python - "$out_dir/$1.json" "$2" <<'EOF'` through `EOF`) with:
```bash
    python -P "$repo_root/scripts/smoke_eval.py" query "$out_dir/$1.json" "$2" </dev/null
```
5. Replace lines 396-400 (`python - "$out_dir/nine.group.json" <<'EOF'` through `EOF`) with:
```bash
python -P "$repo_root/scripts/smoke_eval.py" nine "$out_dir/nine.group.json" </dev/null
```
6. In `border()`, replace lines 415-425 with:
```bash
    python -P "$repo_root/scripts/smoke_eval.py" border "$out_dir/$1.json" "$out_dir/borders.png" "$2" "$3" </dev/null
```
7. Replace line 427 `if ! python -c "import PIL" 2>/dev/null; then` with `if ! python -P -c "import PIL" </dev/null 2>/dev/null; then`.
8. In `converted()`, replace lines 570-576 with:
```bash
    python -P "$repo_root/scripts/smoke_eval.py" converted "$out_dir/$1.json" "$2" "$3" </dev/null
```
9. Replace `cleanup()` (lines 172-176) with:
```bash
# The coproc's pid is the MSYS proxy whose Windows tree holds manifold.exe (two-instance-test.sh says how), so
# kill_tree, not kill: a kill of that pid stops only the proxy, and manifold.exe could outlive an interrupted run
# with its profile locked (ten such profiles were left in target/). The profile's session.mnf.lock goes only once
# manifold.exe is gone, so the rm retries for up to 5 s.
cleanup() {
    kill_tree "$browser_pid"
    kill_tree "$probe_pid"
    wait "$browser_pid" 2>/dev/null
    for _ in 1 2 3 4 5; do
        rm -rf "$profile" 2>/dev/null
        [ -e "$profile" ] || break
        sleep 1
    done
}
```
The `pair.group.json` heredoc at lines 338-343 is `cat`, not python: it stays.

- [ ] **Step 4: The same for chrome-shots' sheet**

Create `scripts/shots_sheet.py` holding the body of the heredoc at `scripts/chrome-shots.sh:663-687` unchanged, under this header:

```python
"""chrome-shots.sh's last step: crops each view's PNG to its top CROP_PX rows and tiles them into sheet.png.

Usage: python -P scripts/shots_sheet.py OUT CROP_PX "VIEW..." </dev/null
"""
```
(then the 25 lines from `import os, sys` to `    print(f"{len(imgs)} views: {out}")`, as they are). Replace lines 662-688 of `chrome-shots.sh` (`python -P - "$OUT" "$CROP_PX" "$VIEWS" </dev/null <<'EOF' 2>&1 | tail -2` through `EOF`) with:

```bash
python -P "$repo_root/scripts/shots_sheet.py" "$OUT" "$CROP_PX" "$VIEWS" </dev/null 2>&1 | tail -2
```

In `scripts/export_state.py` line 1, change `(headless-smoke-test.sh,` to `(smoke_eval.py for headless-smoke-test.sh,` (the docstring lists its users).

- [ ] **Step 5: Nothing feeds python from stdin any more**

Run: `grep -nE 'python3? +(-P +)?- ' scripts/*.sh; grep -n 'python' scripts/*.sh | grep '<<'; bash -n scripts/headless-smoke-test.sh && bash -n scripts/chrome-shots.sh && echo SYNTAX-OK`
Expected: only `SYNTAX-OK` (the `cat ... <<'PY'` lines in `chrome-shots.sh` and `jar-probe.sh` write script files and contain no `python` before `<<`).

- [ ] **Step 6: Remove the leftover profile dirs**

Run: `ls -d target/headless-test-appdata-* 2>/dev/null | wc -l` (expected 10 at `c8b7485`, maybe more), then `find target -maxdepth 1 -name 'headless-test-appdata-*' -mmin +120 -exec rm -rf {} +` and list again.
Expected: none left older than two hours (a smoke test another session is running keeps its profile).

- [ ] **Step 7: A whole run**

Run: `cargo build -j 8 -p manifold` (the smoke test refuses a stale exe), then `cmd //c start "" //b //belownormal bash -c 'bash scripts/headless-smoke-test.sh > target/wave0-smoke.log 2>&1'` and wait for it (about 5 min; watch the log's tail).
Expected: `All checks passed.` (the `SKIP: group border colors` line only if Pillow is missing), and `ls -d target/headless-test-appdata-*` lists nothing new.

- [ ] **Step 8: An interrupted run, with the old cleanup and the new**

The SIGINT also reaches the coproc and closes `manifold.exe`'s stdin, and the app quits on stdin EOF, so a clean result after the change alone would not show that `kill_tree` is what fixed it. Run the same interrupt on a copy with the old `cleanup()` and on the real script, and compare.

Write `target/wave0-interrupt.sh`:

```bash
#!/usr/bin/env bash
# Usage: bash target/wave0-interrupt.sh SCRIPT
# Interrupts the smoke test SCRIPT the way Ctrl-C does (SIGINT to its whole process group) once it is mid-run
# (its first PASS line, after WAITTABS), then lists what it left. Every wait is a condition with a deadline.
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
pids() { tasklist //FI "IMAGENAME eq manifold.exe" //FO CSV //NH | tr -d '\r' | cut -d, -f2 | sort; }
dirs() { ls -d target/headless-test-appdata-* 2>/dev/null | sort; }
new_pids() { pids | comm -13 "$d/before" -; }
new_dirs() { dirs | comm -13 "$d/dirs-before" -; }
pids >"$d/before"
dirs >"$d/dirs-before"
set -m # the smoke test gets its own process group
bash "$1" >"$d/smoke.log" 2>&1 &
p=$!
deadline=$((SECONDS + 240))
until grep -q '^PASS:' "$d/smoke.log" 2>/dev/null; do
    kill -0 "$p" 2>/dev/null || { echo "smoke ended before its first PASS"; break; }
    [ "$SECONDS" -lt "$deadline" ] || { echo "no PASS within 240 s"; break; }
    sleep 0.5
done
kill -INT -- "-$p" 2>/dev/null
wait "$p"
echo "smoke exit $?"
# What the run leaves may take a moment to go (the app quitting on EOF, the rm retry): poll up to 15 s.
deadline=$((SECONDS + 15))
until [ -z "$(new_pids)$(new_dirs)" ] || [ "$SECONDS" -ge "$deadline" ]; do sleep 0.5; done
echo "manifold.exe left by the run:"; new_pids
echo "profile dirs left by the run:"; new_dirs
echo "last lines:"; tail -3 "$d/smoke.log"
```

Before: make `scripts/wave0-smoke-oldcleanup.sh` (untracked; it must sit in `scripts/` because the smoke test finds the repo from its own path), a copy of the edited `scripts/headless-smoke-test.sh` whose `cleanup()` is put back to the old one (`git show c8b7485:scripts/headless-smoke-test.sh | sed -n '172,176p'`: `kill`, `wait`, one `rm -rf`). It keeps the `smoke_eval.py` calls, so no stdin python runs.
Run: `bash target/wave0-interrupt.sh scripts/wave0-smoke-oldcleanup.sh > target/wave0-interrupt-before.log 2>&1` (in the background; up to about 5 min).
Expected: verified #1's race. A profile dir (with `session.mnf.lock`) and/or a `manifold.exe` is left. Record what was left. Kill a leftover `manifold.exe` by its pid (`taskkill //F //T //PID <pid>`, only one this run started; check `CreationDate` as below) and remove its dir. If nothing was left, the race did not show this time: run it twice more, and if it never shows, record that the before/after difference was not reproduced (the change still stands on verified #1's finding). Delete `scripts/wave0-smoke-oldcleanup.sh`.

After: Run: `bash target/wave0-interrupt.sh scripts/headless-smoke-test.sh > target/wave0-interrupt-after.log 2>&1` (in the background).
Expected: a nonzero `smoke exit`, nothing under either "left by the run" heading, and a log that stops mid-run. Another session may start a `manifold.exe` meanwhile: before calling a listed pid a leak, check its start time and command line (`Get-CimInstance Win32_Process -Filter "ProcessId=<pid>" | select CreationDate, CommandLine`). If a `manifold.exe` is left, `kill_tree` on the coproc pid missed it: resolve the instance at start the way `two-instance-test.sh` does (`browser_winpid="$(winpid_of_child "$browser_pid")" || true` after line 168, and `[ -n "$browser_winpid" ] && taskkill //F //T //PID "$browser_winpid" >/dev/null 2>&1` first in `cleanup`), rerun the After run, and record it in the Outcome. If the smoke test did not stop at all, bash's wait-and-cooperative-exit kept it running: record that, and rely on Step 7's run for the cleanup proof.

- [ ] **Step 9: Commit**

```bash
git add scripts/smoke_eval.py scripts/shots_sheet.py scripts/headless-smoke-test.sh scripts/chrome-shots.sh scripts/export_state.py
git commit -m "Smoke test: python from script files, kill_tree cleanup; chrome-shots sheet likewise

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Atomic exports, then no post-appearance sleeps (BT-9)

Model: **sonnet**. Gate: **full** (`session_sync.rs`), the first full gate: it covers Tasks 1-7. Proof: three unit tests fail before and pass after; `link-probe.sh` and `focus-loss-probe.sh` green three times each without the sleeps; a chrome-shots subset green; the gate.

**Files:**
- Modify: `crates/manifold/src/debug_control.rs:1129-1133` (arm), `:1466` (new method before the impl's closing brace), `:256` (doc), test module (ends `:1898`)
- Modify: `crates/manifold/src/session_sync.rs:1946`
- Modify: `crates/manifold/src/main.rs:251`, test module (ends `:3647`)
- Test: `crates/manifold/src/session_sync/tests.rs` (after `export_session_drops_links_with_a_dead_end`, `:5359`)
- Modify: `scripts/focus-loss-probe.sh:116,126,135,147,152,160,173,189,193`, `scripts/link-probe.sh:64,73,77`, `scripts/chrome-shots.sh:96-101`
- Modify: `README.md:362`

**Interfaces:**
- Consumes: `crate::store::atomic::replace(path: &Path, bytes: &[u8]) -> io::Result<()>`; `#[cfg(test)] crate::store::atomic::block_for_test(path: &Path, block: bool)` (makes `replace` on that exact path fail; `std::fs::write` ignores it); `crate::json_file::load<T: DeserializeOwned>(path: &Path, what: &str) -> Result<T, String>`; `session_sync::tests::{app_on, succeed_spawn}`; `store::testutil::temp_dir(name) -> PathBuf`.
- Produces: `App::write_export_state(&self, path: &std::path::Path)` (`pub(crate)`, in `debug_control.rs`); the three `EXPORT*` files appear whole.

- [ ] **Step 1: The three failing tests**

`crates/manifold/src/debug_control.rs`, at the end of `mod tests` (before its closing `}` at line 1898):

```rust
    /// EXPORTSTATE writes through `store::atomic::replace`, so a probe can read the file as soon as it exists:
    /// a blocked target stays unwritten (a plain write would ignore the block).
    #[test]
    fn export_state_is_written_through_the_atomic_replace() {
        let path = crate::store::testutil::temp_dir("export-state-atomic").join("state.json");
        let app = App::default();
        crate::store::atomic::block_for_test(&path, true);
        app.write_export_state(&path);
        assert!(!path.exists(), "the write went around store::atomic::replace");
        crate::store::atomic::block_for_test(&path, false);
        app.write_export_state(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), app.export_state().to_string());
    }
```

`crates/manifold/src/main.rs`, at the end of `mod tests` (after `save_as_a_device_name_says_why`, before the module's closing `}` at line 3647):

```rust
    /// Export Group writes through `store::atomic::replace`: a blocked target stays unwritten.
    #[test]
    fn a_group_export_is_written_through_the_atomic_replace() {
        let path = store::testutil::temp_dir("group-export-atomic").join("g.group.json");
        let json = store::json::JsonDoc::default();
        store::atomic::block_for_test(&path, true);
        write_json_export(&json, &path, "group");
        assert!(!path.exists(), "the write went around store::atomic::replace");
        store::atomic::block_for_test(&path, false);
        write_json_export(&json, &path, "group");
        assert_eq!(json_file::load::<store::json::JsonDoc>(&path, "group"), Ok(json));
    }
```

`crates/manifold/src/session_sync/tests.rs`, after `export_session_drops_links_with_a_dead_end` (line 5359):

```rust
/// Export Session writes through `store::atomic::replace`: a blocked target fails the export and stays unwritten.
#[test]
fn export_session_is_written_through_the_atomic_replace() {
    let dir = temp_dir("export-session-atomic");
    let mut app = app_on(&dir.join("session.mnf"), 1);
    succeed_spawn(&mut app, "https://a/");
    let path = dir.join("export.json");
    crate::store::atomic::block_for_test(&path, true);
    assert!(!app.export_session_to(&path));
    assert!(!path.exists(), "the write went around store::atomic::replace");
    crate::store::atomic::block_for_test(&path, false);
    assert!(app.export_session_to(&path));
    let written: JsonDoc = crate::json_file::load(&path, "session").unwrap();
    assert_eq!(written.nodes, app.export_session_json().unwrap().nodes);
}
```

(`JsonDoc` reaches the tests through `use super::*;`: `session_sync.rs` names it in `export_session_json`'s signature. If the compiler says otherwise, write `crate::store::json::JsonDoc`.)

- [ ] **Step 2: Watch them fail**

Run: `cargo test -j 8 -p manifold -- export_state_is_written_through_the_atomic_replace a_group_export_is_written_through_the_atomic_replace export_session_is_written_through_the_atomic_replace`
Expected: `export_state_...` fails to compile (`no method named write_export_state`). Add Step 3's method with its body still `std::fs::write` (below) to see the runtime failure first:

```rust
    pub(crate) fn write_export_state(&self, path: &std::path::Path) {
        if let Err(err) = std::fs::write(path, self.export_state().to_string()) {
            eprintln!("EXPORTSTATE: couldn't write {}: {err}", path.display());
        }
    }
```
Run again. Expected: all three FAIL. The state and group tests fail at `the write went around store::atomic::replace`; the session one fails earlier, at `assert!(!app.export_session_to(&path))`, because a plain write ignores the block and the export succeeds.

- [ ] **Step 3: Write through `replace`**

`debug_control.rs`, the method (in the `impl App` that holds `export_state`, just before its closing `}` at line 1467):

```rust
    /// `EXPORTSTATE`: writes `export_state` to `path` through `store::atomic::replace`, so a probe never reads
    /// a half-written file. A failure is logged.
    pub(crate) fn write_export_state(&self, path: &std::path::Path) {
        if let Err(err) = crate::store::atomic::replace(path, self.export_state().to_string().as_bytes()) {
            eprintln!("EXPORTSTATE: couldn't write {}: {err}", path.display());
        }
    }
```

and the arm (lines 1129-1133):

```rust
            DebugCommand::ExportState(path) => self.write_export_state(std::path::Path::new(&path)),
```

`session_sync.rs:1946`, replace `std::fs::write(path, text).map_err(|e| e.to_string())` with
`crate::store::atomic::replace(path, text.as_bytes()).map_err(|e| e.to_string())`.

`main.rs:251`, replace `std::fs::write(path, text).map_err(|e| e.to_string())` with
`store::atomic::replace(path, text.as_bytes()).map_err(|e| e.to_string())`.

Docs: in `debug_control.rs`, after line 256 (`/// a test can poll this until `suggestions` fills.`, the end of `ExportState`'s doc), insert:

```rust
    /// The file is replaced atomically (`store::atomic::replace`), so a test may read it as soon as it exists.
```

`README.md:362`, append to the paragraph: ` `EXPORTSTATE`, `EXPORTSESSION` and `EXPORTGROUP` replace their file atomically (written beside it, then renamed over it), so a script may read one as soon as it exists; `SCREENSHOT` writes in place, so wait for its size to settle.`

- [ ] **Step 4: Watch them pass, then the suite**

Run: the Step 2 command. Expected: 3 passed.
Run: `grep -n 'std::fs::write' crates/manifold/src/debug_control.rs crates/manifold/src/session_sync.rs crates/manifold/src/main.rs`
Expected: test lines only (`debug_control.rs` near 1677, `main.rs` from about 3268 on).
Run: `cargo test -j 8 -p manifold -p ipc-protocol` (expected: all pass).

- [ ] **Step 5: Drop the sleeps**

- `scripts/focus-loss-probe.sh:116`: `wait_file "$dir/$1_$i.json" >&2 && sleep 0.1` becomes `wait_file "$dir/$1_$i.json" >&2`. Line 118's `sleep 0.25` (the poll interval) stays.
- `scripts/focus-loss-probe.sh:126,135,147,152,160,173,189,193`: each `wait_file "$dir/<name>.json" && sleep 0.2` becomes `wait_file "$dir/<name>.json"` (names `s0 s1 s3 s4 c0 c1 c3 c4`).
- `scripts/link-probe.sh:64,73,77`: `wait_file "$out/s0.json" && sleep 0.2`, `... s5 ...`, `... s6 ...` become `wait_file "$out/s0.json"`, `wait_file "$out/s5.json"`, `wait_file "$out/s6.json"`.
- `scripts/chrome-shots.sh:100`: delete the line `    sleep 0.2` in `state()`.

Run: `grep -nE 'wait_file .*&& sleep' scripts/*.sh; sed -n '96,100p' scripts/chrome-shots.sh`
Expected: no `wait_file ... && sleep` lines; `state()` ends at `wait_file "$work/state.json"`.

- [ ] **Step 6: Build and run the probes**

Run: `cargo build -j 8 -p manifold`.
Run: `for i in 1 2 3; do bash scripts/link-probe.sh > target/wave0-link-$i.log 2>&1; echo "link $i exit $?"; done` (about 45 s each; BelowNormal by itself).
Expected: three `exit 0`, every line in the logs `PASS:`.
Tell the user focus-loss-probe will take the foreground for a few seconds per case; on their yes:
Run: `for i in 1 2 3; do bash scripts/focus-loss-probe.sh > target/wave0-focus-$i.log 2>&1; echo "focus $i exit $?"; done` (about 80 s each).
Expected: three `exit 0` (a `SKIP` for a refused steal is not a failure; record it).
Run (detached at BelowNormal: chrome-shots does not lower its own instance until Task 9): `cmd //c start "" //b //belownormal bash -c 'VIEWS="graph graph-link graph-link-handles graph-link-drag tabview settings" OUT="$PWD/target/wave0-shots" bash scripts/chrome-shots.sh > target/wave0-shots.log 2>&1; echo "exit $?" >> target/wave0-shots.log'`, wait until the log's last line is `exit N`, then `for v in graph graph-link graph-link-handles graph-link-drag tabview settings; do [ -s target/wave0-shots/$v.png ] || echo "MISSING $v"; done`.
Expected: `exit 0` and no `MISSING`.

- [ ] **Step 7: Commit**

```bash
git add crates/manifold/src/debug_control.rs crates/manifold/src/session_sync.rs crates/manifold/src/session_sync/tests.rs crates/manifold/src/main.rs scripts/focus-loss-probe.sh scripts/link-probe.sh scripts/chrome-shots.sh README.md
git commit -m "Exports: EXPORTSTATE, Export Session and Export Group replace atomically; probes drop their sleeps

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 8: Full gate (covers Tasks 1-7)**

Run: `cargo build -j 8 -p manifold`, then `cmd //c start "" //b //belownormal bash scripts/gate.sh target/gate-wave0-1.log full`; wait for `GATE done` in the log (about 17 min quiet, up to 28 under load).
Expected: three `two-instance run N exit=0` lines and `smoke exit=0 ... fail=0`. Record the `cpu=` and `defender_rt=` values from the log in the Outcome. A failure goes to the user before Task 8 starts.

---

### Task 8: `scripts/lib.sh` and the settings seed

Model: **haiku**. Gate: **light** (scripts). Proof: `scripts/lib-test.sh` passes; every edited probe passes `bash -n`; the baseline sweep is recorded (it is compared in Task 11).

**Files:**
- Create: `scripts/lib.sh`, `scripts/lib-test.sh`, `target/wave0-sweep/sweep.sh` (untracked)
- Modify: `scripts/chrome-shots.sh:64-65` and `:606-607`, `scripts/failed-spawn-probe.sh:38-39`, `scripts/focus-loss-probe.sh:68-69`, `scripts/hscroll-probe.sh:41`, `scripts/jar-probe.sh:37-39`, `scripts/key-probe.sh:71`, `scripts/link-probe.sh:36-37`, `scripts/page-kind-probe.sh:46-47`, `scripts/scale-test.sh:62-63`, `scripts/sleep-probe.sh:79`, plus one sourcing line in each of these ten
- Modify: `README.md` (one paragraph before "**Building Servo on its own:**")

**Interfaces:**
- Consumes: nothing from earlier tasks except Task 7's probes (no sleeps).
- Produces: `scripts/lib.sh` (sourced after `winpid.sh` in the ten probes) with `seed_settings PROFILE [ENGINE] [MORE]`; `scripts/lib-test.sh` ending in `exit $fails`, which Tasks 9-11 extend by inserting before that line.

- [ ] **Step 1: Baseline sweep (before any probe changes)**

Write `target/wave0-sweep/sweep.sh`:

```bash
#!/usr/bin/env bash
# Usage: bash target/wave0-sweep/sweep.sh before|after
# Runs each probe Tasks 8-11 move onto scripts/lib.sh, one at a time, keeping its exit code and verdict lines.
cd "$(dirname "$0")/../.." || exit 1
dir="target/wave0-sweep/$1"
mkdir -p "$dir"
pids() { tasklist //FI "IMAGENAME eq manifold.exe" //FO CSV //NH | tr -d '\r' | cut -d, -f2 | sort; }
pids >"$dir/pids-before"
run() { # name env-and-command...
    local name="$1"
    shift
    echo "$(date +%T) $name" >&2
    env "$@" >"$dir/$name.log" 2>&1
    echo "exit=$?" >"$dir/$name.verdicts"
    grep -E '^(PASS|FAIL|SKIP)' "$dir/$name.log" | tr -d '\r' >>"$dir/$name.verdicts"
}
run failed-spawn bash scripts/failed-spawn-probe.sh
run link bash scripts/link-probe.sh
[ -n "${FOCUS_OK:-}" ] && run focus-loss bash scripts/focus-loss-probe.sh
run hscroll ENGINES=webview2 bash scripts/hscroll-probe.sh
run key ENGINES=webview2 bash scripts/key-probe.sh
run page-kind ENGINES=webview2 bash scripts/page-kind-probe.sh
run sleep ENGINES=webview2 bash scripts/sleep-probe.sh
run scale bash scripts/scale-test.sh 3
[ -d target/debug/cef-runtime ] && run jar bash scripts/jar-probe.sh
run shots VIEWS="graph graph-link tabview settings engine-states-picker" OUT="$PWD/target/wave0-sweep/shots-$1" bash scripts/chrome-shots.sh
# No probe may leave a manifold.exe behind (verified #8's check). A quitting app can take a moment: poll up to 15 s.
deadline=$((SECONDS + 15))
until [ -z "$(pids | comm -13 "$dir/pids-before" -)" ] || [ "$SECONDS" -ge "$deadline" ]; do sleep 0.5; done
echo "manifold.exe left: $(pids | comm -13 "$dir/pids-before" - | tr '\n' ' ')" >"$dir/leftovers"
echo "$(date +%T) done" >&2
```

Run: `cargo build -j 8 -p manifold`, then, detached at BelowNormal because most of these probes don't yet lower their own instance, `cmd //c start "" //b //belownormal bash -c 'bash target/wave0-sweep/sweep.sh before 2> target/wave0-sweep/before.progress'` (about 15 min; prefix `FOCUS_OK=1` inside the quotes only after the user agreed to the foreground steal). Wait until `before.progress` ends with `done`. `engine-states-picker` exercises chrome-shots' second launcher (the `HTTPS_PROXY` one).
Expected: `target/wave0-sweep/before/*.verdicts`, and `before/leftovers` reading `manifold.exe left: ` with no pid (a pid listed may be another session's: check its `CreationDate` and command line before calling it a leak). Note any `exit=` other than 0 now: those are pre-existing and are compared, not fixed, in Task 11.

- [ ] **Step 2: `scripts/lib-test.sh` (the failing test)**

```bash
#!/usr/bin/env bash
# Usage: bash scripts/lib-test.sh
# Checks scripts/lib.sh's helpers without the app, in a few seconds. Exits 0 if every check passed.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
fails=0
expect() { # name want got
    if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "NOT OK: $1: got [$3], wanted [$2]"; fails=1; fi
}

seed_settings "$t/p1"
expect "seed_settings: webview2 by default" '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2"}' "$(cat "$t/p1/manifold/settings.json")"
expect "seed_settings: ends in a newline" 1 "$(tail -c1 "$t/p1/manifold/settings.json" | wc -l)"
seed_settings "$t/p2" cef
expect "seed_settings: the engine" '{"has_shown_first_launch_demo": true, "default_starting_engine": "cef"}' "$(cat "$t/p2/manifold/settings.json")"
seed_settings "$t/p3" webview2 ', "home_page": "http://x/"'
expect "seed_settings: extra members" '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2", "home_page": "http://x/"}' "$(cat "$t/p3/manifold/settings.json")"

exit $fails
```

Run: `bash scripts/lib-test.sh` (no `lib.sh` yet). Expected: `lib.sh: No such file or directory`, `seed_settings: command not found` and `NOT OK` lines, exit 1.

- [ ] **Step 3: `scripts/lib.sh` with the seed helper**

```bash
# Helpers the probe scripts share. Source after winpid.sh:
#   # shellcheck source=lib.sh
#   . "$repo_root/scripts/lib.sh"
# `bash scripts/lib-test.sh` checks every helper without the app.

# seed_settings PROFILE [ENGINE] [MORE]: PROFILE/manifold/settings.json that skips the first-launch demo and
# starts tabs on ENGINE (default webview2). MORE is extra JSON members, starting with ", ".
seed_settings() {
    mkdir -p "$1/manifold"
    echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"${2:-webview2}\"${3:-}}" >"$1/manifold/settings.json"
}
```

Run: `bash scripts/lib-test.sh`. Expected: four `ok:` lines, exit 0.

- [ ] **Step 4: Source lib.sh in the ten probes**

After each probe's `winpid.sh` source line, insert:

```bash
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
```

Lines: `chrome-shots.sh:39`, `failed-spawn-probe.sh:21`, `focus-loss-probe.sh:36`, `hscroll-probe.sh:18`, `key-probe.sh:22`, `link-probe.sh:24`, `page-kind-probe.sh:18`, `scale-test.sh:20`, `sleep-probe.sh:33`. `jar-probe.sh` has no `$repo_root`: after its line 25 `. "$sp/winpid.sh"`, insert `# shellcheck source=lib.sh` and `. "$sp/lib.sh"`.

- [ ] **Step 5: Replace the seeds**

Line numbers are from before Step 4's insertions (and, for `chrome-shots.sh:606`, before Task 7 deleted line 100); find each by its text.

| File:lines | Old | New |
|---|---|---|
| `chrome-shots.sh:64-65` | `mkdir -p "$profile/manifold"` + the `echo '{...webview2"}' >...settings.json` line | `seed_settings "$profile"` |
| `chrome-shots.sh:606-607` | `profile="$work/eng-$1"; mkdir -p "$profile/manifold"` + `echo "{...\"webview2\"${2:-}}" >...` | `profile="$work/eng-$1"` then `seed_settings "$profile" webview2 "${2:-}"` |
| `failed-spawn-probe.sh:38-39` | `mkdir -p ...` + `echo '{...}' >...` | `seed_settings "$profile"` |
| `focus-loss-probe.sh:68-69` (indent 4) | same pair | `    seed_settings "$profile"` |
| `link-probe.sh:36-37` | same pair | `seed_settings "$profile"` |
| `hscroll-probe.sh:41`, `key-probe.sh:71`, `sleep-probe.sh:79` (indent 4) | the `echo "{...\"$engine\"}" >...` line only (their `mkdir` is elsewhere and stays) | `    seed_settings "$profile" "$engine"` |
| `page-kind-probe.sh:46-47` (indent 8), `scale-test.sh:62-63` (indent 4) | `mkdir -p "$profile/manifold"` + `echo "{...\"$engine\"}" >...` | `seed_settings "$profile" "$engine"` at the same indent |
| `jar-probe.sh:37,39` | line 37 `profile="$out/appdata"; mkdir -p "$profile/manifold"`, line 39 the `echo` with `home_page` and `cef_directory` | line 37 `profile="$out/appdata"`; line 39 `seed_settings "$profile" webview2 ", \"home_page\": \"$base/whoami?b\", \"cef_directory\": \"$(cygpath -m "$repo/target/debug/cef-runtime")\""` (line 38's comment stays) |

Run: `grep -n 'has_shown_first_launch_demo' scripts/*.sh`
Expected: only `headless-smoke-test.sh` (comment and its own seed), `json-pan-profile.sh`, `package.sh`, `two-instance-test.sh` (out of scope), `lib.sh` and `lib-test.sh` (its `expect` strings).
Run: `for f in chrome-shots failed-spawn-probe focus-loss-probe hscroll-probe jar-probe key-probe link-probe page-kind-probe scale-test sleep-probe; do bash -n scripts/$f.sh || echo "SYNTAX $f"; done; bash scripts/lib-test.sh`
Expected: no `SYNTAX`, lib-test exit 0.
Spot-check one probe end to end: `bash scripts/link-probe.sh` (expected: all PASS; it lowers its own instance).

- [ ] **Step 6: README**

Insert before the paragraph starting `**Building Servo on its own:**` (it names only what exists; Tasks 9-11 each add their helpers to it):

```markdown
`scripts/lib.sh` holds the helpers the probes share: `seed_settings` (a profile's `settings.json`). `bash scripts/lib-test.sh` checks them in seconds, without the app. `headless-smoke-test.sh` and `two-instance-test.sh` drive coprocs instead and don't use it.
```

- [ ] **Step 7: Commit**

```bash
git add scripts/lib.sh scripts/lib-test.sh scripts/*.sh README.md
git commit -m "Probes: scripts/lib.sh, starting with seed_settings

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

---

### Task 9: `launch_headless` and `send`

Model: **haiku**. Gate: **light**. Proof: lib-test's fake-exe check (the instance's `$!` and Windows pid, `APPDATA`, extra environment and first command all arrive); `bash -n`; `link-probe.sh` and a chrome-shots engine view still pass.

**Files:**
- Modify: `scripts/lib.sh`, `scripts/lib-test.sh`, `README.md` (the lib.sh paragraph from Task 8)
- Modify: the 12 launch sites: `chrome-shots.sh:90` and `:617-618`, `failed-spawn-probe.sh:45-46`, `focus-loss-probe.sh:79-80`, `hscroll-probe.sh:47-48`, `jar-probe.sh:58`, `key-probe.sh:77`, `link-probe.sh:51-52`, `page-kind-probe.sh:53-54`, `scale-test.sh:79-80`, `sleep-probe.sh:85-86` and `:198`
- Modify: the `send()` definitions at `chrome-shots.sh:93`, `focus-loss-probe.sh:51`, `link-probe.sh:47` (deleted)

**Interfaces:**
- Consumes: `scripts/lib.sh` and `scripts/lib-test.sh` from Task 8.
- Produces: `launch_headless EXE PROFILE CMDFILE LOG` (backgrounds the instance and sets it BelowNormal; sets `launched_winpid`; the caller reads `$!` right after; extra environment as a prefix assignment on the call; the caller empties CMDFILE first; needs `winpid.sh` sourced first); `send LINE...` (appends each argument as a line to `$cmd`).

- [ ] **Step 1: The failing test**

Insert before `exit $fails` in `scripts/lib-test.sh`:

```bash
# A fake instance: prints its first argument, APPDATA, HTTPS_PROXY and the first command line, then waits for an
# END line, so it is still running while launch_headless looks up its Windows pid.
cat >"$t/fake-exe" <<'EOF'
#!/usr/bin/env bash
read -r first
printf '%s\n' "$1" "$APPDATA" "${HTTPS_PROXY:-none}" "$first"
while read -r line; do [ "$line" = END ] && break; done
EOF
chmod +x "$t/fake-exe"
cmd="$t/cmd"
: >"$cmd"
send "FIRST LINE" SECOND
expect "send: one line per argument" "FIRST LINE
SECOND" "$(cat "$cmd")"
HTTPS_PROXY=http://proxy.test launch_headless "$t/fake-exe" "$t/prof" "$cmd" "$t/fake.log"
pid=$!
expect "launch_headless: \$! is still the instance after the priority lookup" yes "$(kill -0 "$pid" 2>/dev/null && echo yes)"
expect "launch_headless: launched_winpid is set" yes "$([ -n "$launched_winpid" ] && echo yes)"
send END
wait "$pid"
expect "launch_headless: \$! is the instance, and it exits 0" 0 "$?"
expect "launch_headless: --headless" --headless "$(sed -n 1p "$t/fake.log")"
expect "launch_headless: APPDATA is the profile, Windows-style" "$(cygpath -w "$t/prof")" "$(sed -n 2p "$t/fake.log")"
expect "launch_headless: a prefix assignment reaches the instance" http://proxy.test "$(sed -n 3p "$t/fake.log")"
expect "launch_headless: commands from the file's first line" "FIRST LINE" "$(sed -n 4p "$t/fake.log")"
```

Run: `bash scripts/lib-test.sh`. Expected: `send: command not found` / `launch_headless: command not found`, a `NOT OK` line, and a nonzero exit (under `set -u` it may stop at `pid=$!` with `!: unbound variable`, since nothing was backgrounded).

- [ ] **Step 2: The helpers**

Append to `scripts/lib.sh`:

```bash
# launch_headless EXE PROFILE CMDFILE LOG: one `EXE --headless` instance in the background, APPDATA set to
# PROFILE, reading commands as they are appended to CMDFILE, output to LOG, set BelowNormal (winpid.sh's
# below_normal; a failure warns and the run goes on). Sets `launched_winpid` to its Windows pid ("" if the lookup
# failed). The caller empties CMDFILE first (some write commands before launching) and reads `$!` right after:
# the lookup runs in the foreground, so `$!` is still the instance. More environment goes on the call:
# `HTTPS_PROXY=... launch_headless ...`.
launch_headless() {
    # `-n +1`: plain `tail -f` starts with only the last 10 lines.
    APPDATA="$(cygpath -w "$2")" "$1" --headless < <(exec tail -n +1 -f --pid=$$ "$3" 2>/dev/null) >"$4" 2>&1 &
    launched_winpid="$(below_normal "$!")" || echo "WARNING: $1 (pid $!) runs at normal priority" >&2
}

# send LINE...: appends each argument as one command line to "$cmd".
send() { printf '%s\n' "$@" >>"$cmd"; }
```

Run: `bash scripts/lib-test.sh`. Expected: every line `ok:`, exit 0.

- [ ] **Step 3: Use them**

At each launch site, delete the comment line directly above the launch if it is `# \`-n +1\`: plain \`tail -f\` starts with only the last 10 lines.` (failed-spawn, focus-loss, hscroll, link, page-kind, sleep:85) or `# Commands come from a tail that exits with this script.` (scale-test:79), and replace the launch line, keeping its indent and the `inst=$!`/`app=$!` line after it:

| File:line | New launch line |
|---|---|
| `chrome-shots.sh:90` | `launch_headless "$exe" "$profile" "$cmd" "$work/log"` |
| `chrome-shots.sh:617-618` (two lines with `\`) | `HTTPS_PROXY="http://127.0.0.1:$(cat "$work/proxy-port")" launch_headless "$eng/manifold.exe" "$profile" "$cmd" "$work/log-eng-$1"` |
| `failed-spawn-probe.sh:46` | `launch_headless "$exe" "$profile" "$cmd" "$profile/log"` |
| `focus-loss-probe.sh:80` | `launch_headless "$exe" "$profile" "$cmd" "$dir/log"` |
| `hscroll-probe.sh:48`, `key-probe.sh:77`, `page-kind-probe.sh:54`, `scale-test.sh:80`, `sleep-probe.sh:86` | `launch_headless "$exe" "$profile" "$cmd" "$profile/log"` |
| `sleep-probe.sh:198` | `launch_headless "$exe" "$profile" "$cmd" "$profile/log2"` |
| `jar-probe.sh:58` | `launch_headless "$exe" "$profile" "$cmd" "$profile/log-$name"` (its `cmd` is a `local` of `start`; the function sees it) |
| `link-probe.sh:52` | `launch_headless "$exe" "$profile" "$cmd" "$out/$1"` |

Delete the `send() { printf '%s\n' "$@" >>"$cmd"; }` lines at `chrome-shots.sh:93`, `focus-loss-probe.sh:51` and `link-probe.sh:47`.

The three probes that already set their instance BelowNormal now get it from `launch_headless`:
- `link-probe.sh:54`: delete `below_normal "$inst" >/dev/null || echo "WARNING: $1: the instance runs at normal priority" >&2`.
- `focus-loss-probe.sh:82-83`: replace the comment and `winpid="$(below_normal "$inst")"` with `    winpid="$launched_winpid" # the steal needs the Windows pid; launch_headless looked it up`. Line 85's empty-pid FAIL stays.
- `jar-probe.sh:60`: replace `local winpid; winpid="$(below_normal "$app")" || echo "WARNING: $name: the app runs at normal priority" >&2` with `  local winpid="$launched_winpid"`.

The other seven (chrome-shots, failed-spawn, hscroll, key, page-kind, scale-test, sleep) ran the app at Normal priority until now; they are BelowNormal from this task on.

Run: `grep -n 'tail -n +1 -f' scripts/*.sh; grep -n '^ *send() {' scripts/*.sh; grep -n 'below_normal' scripts/*.sh`
Expected: `tail` only in `lib.sh` and `package.sh`; `send()` only in `lib.sh` and `headless-smoke-test.sh` (its coproc `send` is different and stays); `below_normal` only in `winpid.sh` and `lib.sh`.
Run: `bash -n` on the ten probes (Task 8 Step 5's loop) and `bash scripts/lib-test.sh`.
Run: `bash scripts/link-probe.sh` and `VIEWS="graph engine-states-picker" OUT="$PWD/target/wave0-shots" bash scripts/chrome-shots.sh > target/wave0-shots-t9.log 2>&1; echo "exit $?"`.
Expected: link all PASS; chrome-shots exit 0 with `graph.png` and the engine-states PNGs in `target/wave0-shots`.
README: in the lib.sh paragraph, replace `` `seed_settings` (a profile's `settings.json`). `` with `` `seed_settings` (a profile's `settings.json`), `launch_headless` (one `--headless` instance reading commands from a file, set BelowNormal) and `send`. ``

- [ ] **Step 4: Commit**

```bash
git add scripts/lib.sh scripts/lib-test.sh scripts/*.sh README.md
git commit -m "Probes: launch_headless (BelowNormal) and send from scripts/lib.sh

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

---

### Task 10: `wait_file`

Model: **haiku**. Gate: **light**. Proof: lib-test's wait_file checks (a late file, the timeout and its message, an empty file); `link-probe.sh` passes.

**Files:**
- Modify: `scripts/lib.sh`, `scripts/lib-test.sh`, `README.md` (the lib.sh paragraph)
- Modify: `scripts/chrome-shots.sh:94` (and a `WAIT_FILE_SECS=30` line), `scripts/focus-loss-probe.sh:53`, `scripts/link-probe.sh:59` (line numbers before Task 9's deletions; find each by its text)

**Interfaces:**
- Consumes: `scripts/lib.sh`, `scripts/lib-test.sh` from Tasks 8-9.
- Produces: `wait_file PATH`: returns 0 once PATH is non-empty, polling every 0.25 s for `${WAIT_FILE_SECS:-60}` seconds; otherwise prints `timed out waiting for PATH` on stderr and returns 1.

- [ ] **Step 1: The failing test**

Insert before `exit $fails` in `scripts/lib-test.sh`:

```bash
(sleep 1; echo x >"$t/late") &
wait_file "$t/late"
expect "wait_file: a file that turns up later" 0 "$?"
WAIT_FILE_SECS=1 wait_file "$t/never" 2>"$t/wait.err"
expect "wait_file: gives up after WAIT_FILE_SECS" 1 "$?"
expect "wait_file: says what it waited for" "timed out waiting for $t/never" "$(cat "$t/wait.err")"
: >"$t/empty"
WAIT_FILE_SECS=1 wait_file "$t/empty" 2>/dev/null
expect "wait_file: an empty file is not there yet" 1 "$?"
```

Run: `bash scripts/lib-test.sh`. Expected: `wait_file: command not found`, `NOT OK`, exit 1.

- [ ] **Step 2: The helper**

Append to `scripts/lib.sh`:

```bash
# wait_file PATH: until PATH exists and is non-empty, for WAIT_FILE_SECS seconds (default 60). The EXPORT*
# files are replaced atomically, so a non-empty one is whole.
wait_file() {
    local _
    for _ in $(seq $((${WAIT_FILE_SECS:-60} * 4))); do
        [ -s "$1" ] && return 0
        sleep 0.25
    done
    echo "timed out waiting for $1" >&2
    return 1
}
```

Run: `bash scripts/lib-test.sh`. Expected: all `ok:`, exit 0.

- [ ] **Step 3: Use it**

Delete the `wait_file() { for _ in $(seq ...); do [ -s "$1" ] && return 0; sleep 0.25; done; echo "timed out waiting for $1" >&2; return 1; }` lines in `chrome-shots.sh` (120 tries), `focus-loss-probe.sh` and `link-probe.sh` (240 tries). In `chrome-shots.sh`, right after its new `. "$repo_root/scripts/lib.sh"` line, insert:

```bash
WAIT_FILE_SECS=30 # chrome-shots' own wait, from before lib.sh
```

Run: `grep -n 'wait_file() {' scripts/*.sh`
Expected: `lib.sh` and `two-instance-test.sh` (its own `wait_file` exits the script on a timeout and stays).
Run: `bash -n` on the three edited probes, `bash scripts/lib-test.sh`, then `bash scripts/link-probe.sh`.
Expected: no syntax errors, lib-test exit 0, link all PASS.
README: in the lib.sh paragraph, replace `` and `send`. `` with `` , `send` and `wait_file` (`WAIT_FILE_SECS`, default 60). ``

- [ ] **Step 4: Commit**

```bash
git add scripts/lib.sh scripts/lib-test.sh scripts/chrome-shots.sh scripts/focus-loss-probe.sh scripts/link-probe.sh README.md
git commit -m "Probes: wait_file from scripts/lib.sh (WAIT_FILE_SECS, default 60)

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

---

### Task 11: `check`, and the after sweep

Model: **haiku**. Gate: **light**. Proof: lib-test's check checks; the after sweep's verdicts match the baseline from Task 8.

**Files:**
- Modify: `scripts/lib.sh`, `scripts/lib-test.sh`, `README.md` (the lib.sh paragraph)
- Modify: `scripts/failed-spawn-probe.sh:71-73`, `scripts/focus-loss-probe.sh:58-60`, `scripts/link-probe.sh:93-95` (find each by its text)

**Interfaces:**
- Consumes: `scripts/lib.sh`, `scripts/lib-test.sh` from Tasks 8-10; `target/wave0-sweep/sweep.sh` and `target/wave0-sweep/before/` from Task 8.
- Produces: `check NAME WANT GOT`: prints `PASS: NAME`, or `FAIL: NAME: got [GOT], wanted [WANT]` and sets `status=1` (the caller sets `status=0` first).

- [ ] **Step 1: The failing test**

Insert before `exit $fails` in `scripts/lib-test.sh`:

```bash
expect "check: a match passes and leaves status" "PASS: same
status=0" "$(status=0; check same a a; echo "status=$status")"
expect "check: a mismatch says got and wanted, and sets status" "FAIL: differs: got [b], wanted [a]
status=1" "$(status=0; check differs a b; echo "status=$status")"
```

Run: `bash scripts/lib-test.sh`. Expected: `check: command not found`, `NOT OK`, exit 1.

- [ ] **Step 2: The helper**

Append to `scripts/lib.sh`:

```bash
# check NAME WANT GOT: a PASS or FAIL line; a FAIL sets status=1 (the caller starts with status=0).
check() {
    if [ "$2" = "$3" ]; then echo "PASS: $1"; else echo "FAIL: $1: got [$3], wanted [$2]"; status=1; fi
}
```

Run: `bash scripts/lib-test.sh`. Expected: all `ok:`, exit 0.

- [ ] **Step 3: Use it**

In `failed-spawn-probe.sh`, `focus-loss-probe.sh` and `link-probe.sh`, delete the three lines

```bash
check() { # name want got
    if [ "$2" = "$3" ]; then echo "PASS: $1"; else echo "FAIL: $1: got [$3], wanted [$2]"; status=1; fi
}
```

and keep each file's `status=0`. `chrome-shots.sh`'s own `check` (inside a function, other argument order, no status) stays; it overrides lib's only where it is defined.

Run: `grep -n 'check() {' scripts/*.sh`
Expected: `chrome-shots.sh` (its own), `lib.sh`, `package.sh` (its `--check` mode).
Run: `bash -n` on the three probes and `bash scripts/lib-test.sh`.
README: in the lib.sh paragraph, replace `` , `send` and `wait_file` `` with `` , `send`, `wait_file` `` and `` (`WAIT_FILE_SECS`, default 60). `` with `` (`WAIT_FILE_SECS`, default 60) and `check` (a PASS/FAIL line that sets `status`). ``

- [ ] **Step 4: After sweep**

Run: `cargo build -j 8 -p manifold`, then `cmd //c start "" //b //belownormal bash -c 'bash target/wave0-sweep/sweep.sh after 2> target/wave0-sweep/after.progress'` (with `FOCUS_OK=1` only if the baseline had it; detached as the baseline was, so both sides run alike), and wait for `done` in `after.progress`. Expected: `after/leftovers` lists no pid.
Run: `for f in target/wave0-sweep/before/*.verdicts; do n=$(basename "$f"); diff <(sed -E 's/[0-9]+/N/g' "$f") <(sed -E 's/[0-9]+/N/g' "target/wave0-sweep/after/$n") >/dev/null || echo "DIFFERS $n"; done`
Expected: no `DIFFERS` (numbers are masked: link ids, timings and ports change per run). A difference that is a timing flake reruns that one probe twice; one that persists means a migration changed behavior: find which task's edit, fix it there, and record it in the Outcome.

- [ ] **Step 5: Commit**

```bash
git add scripts/lib.sh scripts/lib-test.sh scripts/failed-spawn-probe.sh scripts/focus-loss-probe.sh scripts/link-probe.sh README.md
git commit -m "Probes: check from scripts/lib.sh

Co-Authored-By: Claude Haiku 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Hosts open `about:blank` for a bad initial URL (BT-5)

Model: **opus**. Gate: **full** (engine change), the second full gate: it covers Tasks 8-12. Proof: `scripts/bad-url-probe.sh` fails before the host change and passes after; the Servo host compiles where `target-servo` already has a build; the gate.

**Files:**
- Create: `scripts/bad-url-probe.sh`
- Modify: `crates/manifold-engine-webview2/src/main.rs:262-264`
- Modify: `crates/manifold-engine-servo/src/main.rs:200-201`
- Modify: `README.md` (a paragraph after the `scripts/focus-loss-probe.sh` one, `:410`), the spec's Status line, this plan's Outcome

**Interfaces:**
- Consumes: `seed_settings`, `launch_headless`, `send`, `wait_file`, `check` (Tasks 8-11); `kill_tree` (`winpid.sh`; `launch_headless` uses its `below_normal`); `require_fresh_exe` (`fresh-exe.sh`). Host facts: the app passes a tab's stored url as `ENGINE_INITIAL_URL` (`process_manager.rs:1254`), `""` when the tab has none; a host's log is `std::env::temp_dir()\manifold-<tab id>.log`, appended (`process_manager.rs:1005-1023`); an imported tab sleeps until opened (`GRAPHDOWN` focuses the first card when nothing is focused, `GRAPHOPEN` opens it, as `hscroll-probe.sh` does).
- Produces: nothing later tasks use.

- [ ] **Step 1: The probe (the failing test)**

```bash
#!/usr/bin/env bash
# Usage: [ENGINES="webview2"] scripts/bad-url-probe.sh
#
# Checks (spec 2026-10-07-wave-0-tooling, BT-5) that an engine host given an initial URL it can't open shows
# about:blank instead of panicking, for two tabs:
#   garbage: stored url "not a url";
#   missing: no url (the host gets ENGINE_INITIAL_URL="").
# Each is one headless instance on a fresh profile seeded with that one tab. Imported tabs sleep, so GRAPHDOWN
# (focus the first card) and GRAPHOPEN wake it. EXPORTSTATE is polled until the tab is loaded (or 30 tries); then a
# host log must exist and hold no "panicked", and the tab must be loaded. Hosts log to %TEMP%\manifold-<tab>.log,
# so TEMP and TMP point into the case's own directory. ENGINES defaults to webview2 (servo needs manifold-engine-servo.exe beside the exe). Runs
# BelowNormal; kills only its own instances. Build first: cargo build -p manifold.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
require_fresh_exe "$exe" || exit 1
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT
status=0
cmd=""

# tab_loaded STATE: "<tab card count> <all loaded>" from one EXPORTSTATE file, e.g. "1 True".
tab_loaded() {
    python -P -c 'import json,sys; c=[c for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"] if c["type"]=="application/vnd.manifold.tab"]; print(len(c), all(x["loaded"] for x in c))' "$(cygpath -w "$1")" </dev/null 2>&1 | tr -d '\r'
}

# run_case ENGINE NAME URL-MEMBER: URL-MEMBER is the seed tab's `, "url": ...` member, or empty for none.
run_case() {
    local dir="$out/$1-$2" profile="$out/$1-$2/appdata" tmp
    seed_settings "$profile" "$1"
    mkdir -p "$dir/tmp"
    tmp="$(cygpath -w "$dir/tmp")"
    echo "{\"nodes\": [{\"id\": \"t1\", \"type\": \"application/vnd.manifold.tab\"$3, \"engine\": \"$1\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null ||
        { check "$1 $2: the seed imports" 0 1; return; }
    cmd="$dir/cmd"
    : >"$cmd"
    TEMP="$tmp" TMP="$tmp" launch_headless "$exe" "$profile" "$cmd" "$dir/log"
    inst=$! # launch_headless set it BelowNormal
    # `SLEEP 4` before GRAPHDOWN is hscroll-probe's settle for the graph's first layout, kept as is: EXPORTSTATE has
    # no field that says the graph will take GRAPHDOWN yet. Every wait after it is a condition with a deadline.
    send WAITTABS "SLEEP 4" GRAPHDOWN GRAPHOPEN
    # Poll until the tab is loaded, up to 30 exports. A host that panics never gets there: CRASH_LIMIT respawns it
    # up to 3 times, then the tab fails. The first export waits behind WAITTABS (up to 60 s), hence 120 s per file.
    local n=0 got="" state=""
    while [ "$n" -lt 30 ]; do
        n=$((n + 1))
        state="$dir/s$n.json"
        send "EXPORTSTATE $(cygpath -w "$state")"
        WAIT_FILE_SECS=120 wait_file "$state" || break
        got="$(tab_loaded "$state")"
        [ "$got" = "1 True" ] && break
        sleep 2 # the poll interval
    done
    send QUIT
    wait "$inst" 2>/dev/null
    inst=""
    local logs
    logs="$(ls "$dir"/tmp/manifold-*.log 2>/dev/null | wc -l)"
    check "$1 $2: the host wrote a log" yes "$([ "$logs" -gt 0 ] && echo yes || echo no)"
    check "$1 $2: the host did not panic" 0 "$(cat "$dir"/tmp/manifold-*.log 2>/dev/null | grep -c panicked)"
    check "$1 $2: the tab ends loaded (after $n exports)" "1 True" "$got"
    [ -s "$state" ] && echo "INFO: $1 $2: focused_url $(python -P -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["focused_url"])' "$(cygpath -w "$state")" </dev/null 2>&1 | tr -d '\r')"
    [ "$status" -eq 0 ] || { echo "--- $1 $2 host log"; tail -n 15 "$dir"/tmp/manifold-*.log 2>/dev/null; }
}

for engine in ${ENGINES:-webview2}; do
    run_case "$engine" garbage ', "url": "not a url"'
    run_case "$engine" missing ''
done
exit $status
```

Run: `bash -n scripts/bad-url-probe.sh && cargo build -j 8 -p manifold && cargo build -j 8 -p manifold-engine-webview2` (two commands, so `manifold` keeps its own `windows` feature set), then `bash scripts/bad-url-probe.sh`.
Expected: exit 1 with `FAIL: webview2 garbage: the host did not panic` (the log shows `initial navigate` and `panicked`) and `FAIL: webview2 garbage: the tab ends loaded (after 30 exports)` (about 90 s for that case: the poll runs to its limit). If the garbage case passes before any host change, the app rewrites the url upstream: stop and report to the user. If the missing case already passes, record it in the Outcome: the app substitutes a URL for an empty one, and the case stays as a guard.

- [ ] **Step 2: WebView2 host**

`crates/manifold-engine-webview2/src/main.rs:262-264`, replace

```rust
    unsafe {
        webview.Navigate(&HSTRING::from(&initial_url)).expect("initial navigate");
    }
```

with

```rust
    // A tab's stored URL can be anything (an old build's, an import's): one WebView2 rejects opens about:blank,
    // where the user can type another, instead of panicking the host into the crash-respawn loop.
    unsafe {
        if let Err(err) = webview.Navigate(&HSTRING::from(&initial_url)) {
            eprintln!("initial navigate to {initial_url:?} failed ({err}); opening about:blank");
            let _ = webview.Navigate(&HSTRING::from("about:blank"));
        }
    }
```

- [ ] **Step 3: Servo host**

`crates/manifold-engine-servo/src/main.rs:200-201`, replace

```rust
    let webview = WebViewBuilder::new(&servo, rendering_context.clone() as Rc<dyn RenderingContext>)
        .url(Url::parse(&initial_url).expect("parse initial url"))
```

with

```rust
    // As the WebView2 host: an initial URL that doesn't parse opens about:blank instead of panicking.
    let start = Url::parse(&initial_url).unwrap_or_else(|err| {
        eprintln!("initial url {initial_url:?} doesn't parse ({err}); opening about:blank");
        Url::parse("about:blank").expect("about:blank parses")
    });
    let webview = WebViewBuilder::new(&servo, rendering_context.clone() as Rc<dyn RenderingContext>)
        .url(start)
```

- [ ] **Step 4: Watch the probe pass**

Run: `cargo build -j 8 -p manifold-engine-webview2`, then `bash scripts/bad-url-probe.sh`.
Expected: exit 0; every `check` line `PASS`; `INFO: ... focused_url about:blank` (or the substituted URL for `missing`). The host log for `garbage` holds the `initial navigate to "not a url" failed` line.
Run: `cargo fmt --all --check && cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` (expected: exit 0).

- [ ] **Step 5: Servo compiles (only where it is already built)**

Only if `target-servo/debug/manifold-engine-servo.exe` exists and Task 1 found every unit `Fresh` (else skip, and say so in the Outcome):
Run: `cmd //c start "" //b //belownormal bash -c 'CARGO_TARGET_DIR=target-servo cargo build -j 8 -p manifold-engine-servo > target/wave0-servo.log 2>&1'`, and watch the log.
Expected: only `manifold-engine-servo` (and maybe `ipc-protocol`) compiles, `Finished`. If it starts compiling `mozjs_sys` or other Servo dependencies, stop it (it is a 33-minute, 3.9 GB rebuild), and record it in the Outcome.
`cargo clippy -j 8 -p manifold-engine-servo` is not required (the hook leaves Servo out).

- [ ] **Step 6: Docs**

`README.md`, after the `scripts/focus-loss-probe.sh` paragraph (`:410`), insert a blank line and:

```markdown
`scripts/bad-url-probe.sh` checks that an engine host given an initial URL it can't open shows `about:blank` instead of panicking (under a minute when it passes, about 3 when it fails; build first; `ENGINES` defaults to `webview2`). For a tab stored with the url `not a url`, and one with no url, a headless instance opens the tab (`GRAPHDOWN`, `GRAPHOPEN`) and polls `EXPORTSTATE`; the tab must end loaded, and its host log must hold no `panicked`. Hosts log to `%TEMP%\manifold-<tab>.log`, so the probe points `TEMP`/`TMP` into its own directory.
```

In `docs/superpowers/specs/2026-10-07-wave-0-tooling-design.md`, change the first line after the title from `Status: Agreed 2026-10-07, not started.` to `Status: Done <today's date>; outcome in the plan.` (the rest of that paragraph stays).

- [ ] **Step 7: Commit**

```bash
git add scripts/bad-url-probe.sh crates/manifold-engine-webview2/src/main.rs crates/manifold-engine-servo/src/main.rs README.md docs/superpowers/specs/2026-10-07-wave-0-tooling-design.md
git commit -m "Hosts: a bad initial URL opens about:blank instead of panicking (WebView2, Servo)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 8: Full gate (covers Tasks 8-12)**

Run: `cargo build -j 8 -p manifold`, then `cmd //c start "" //b //belownormal bash scripts/gate.sh target/gate-wave0-2.log full`; wait for `GATE done`.
Expected: three `exit=0` two-instance runs and `smoke exit=0 ... fail=0`. Record `cpu=` and `defender_rt=`.
Then `Get-Process python, manifold -ErrorAction SilentlyContinue | select Id, StartTime, Path` (PowerShell): none of them is one this plan started (other sessions' are theirs; leave them).

- [ ] **Step 9: Outcome**

Fill this plan's `## Outcome`: per task, what landed and its commit; Task 1's Fresh/rebuild result; Task 3 done or declined (and any feature added beyond the listed union); Task 4's site count (11 or 12); Task 5's CI result or "not pushed"; Task 6's interrupt results, old `cleanup()` against new; the probes' run counts and any SKIP; the sweep comparison and its leftovers; Task 12's before/after probe results and the Servo build result; both gates with CPU and Defender; anything deferred. Commit it:

```bash
git add docs/superpowers/plans/2026-10-07-wave-0-tooling.md
git commit -m "Wave 0: outcome

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

## Outcome
