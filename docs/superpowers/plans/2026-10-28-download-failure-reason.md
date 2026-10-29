---
status: active
---

# Download Failure Reason Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every failed download card says why it failed, on its face, in the Downloads popup, in the toast and in its tooltip (which a screen reader hears too). The reason comes from one protocol type that each engine host fills in from its engine's own failure code.

**Architecture:**
- `ViewToHost::DownloadFailed` gains `failure: DownloadFailure { kind: FailureKind, detail: String }`. `FailureKind` is a closed, payload-free enum of seven kinds, sorted by what the user can do about the failure. This is protocol 8.
- The hosts' own refusals are classified once, in `downloads::destination`, which now returns a `Refusal` (path plus failure). `downloads::refused` carries it.
- Each host maps its engine's native code in one small pure `failure_kind` function that ends `_ => Other`.
- The app keeps one `DownloadState { InFlight, Completed, Failed(DownloadFailure) }` per download card, in place of two bools.
- All wording lives in one module, `crates/manifold/src/download_failure.rs`.
- A first probe task records what WebView2 actually reports for a download into `C:\`, a folder a normal user cannot write files into.

**Tech Stack:**
- Rust (nightly per `rust-toolchain.toml`, edition 2021) and egui 0.36.
- webview2-com 0.39.1 (`COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON`), cef 152.1.0+152.0.6 (`DownloadInterruptReason`), webkit2gtk 2.0.2 (`DownloadError`).
- postcard, plus the `gate` harness (chrome-shots, `~/.claude/runs/run.ps1`).

**Spec:** `C:\Users\WINSTO~1\AppData\Local\Temp\claude\C--Users-Winston-Li-Documents-GitHub-manifold\a2c355a1-5493-43d2-a2ce-bd6f49ba9245\scratchpad\item3-design.md`, plus its research digest and the verifier's corrections in the same scratchpad's `digest\`. The scratchpad is temporary, so the approved decisions are restated below; they are binding. The choices this plan makes on its own are listed under "Decisions for review" (P1-P16).

**Base commit:** d047184e. The plan was drafted at 36582023 with another agent's chrome-shots work uncommitted in the tree; that work landed as d047184e, and the anchors in the files it touched (`shots.rs`, `state.rs`, `debug_export.rs`, `PARKED.md`, `README.md`) were re-checked against d047184e (2026-10-28). All line numbers below match d047184e. Lines drift as tasks land, so find each edit by the code quoted with it, not by its line number. Task 1 Step 1 records the commit the work actually starts from, as B.

## Approved decisions (the spec, restated)

- **D5. Interface.**
  - The message becomes `ViewToHost::DownloadFailed { path, failure: DownloadFailure }`, with `DownloadFailure { kind: FailureKind, detail: String }` in `manifold-view-protocol`.
  - `kind` is a closed, payload-free enum, chosen by what the user can do.
  - `detail` is the engine's or OS's own words. It is shown only in the tooltip, is empty when there is nothing to add, and never contains a path.
  - The host's own refusals are classified once, in `downloads::destination`, which today discards the `io::Error` (`downloads.rs:157`). It returns the classified failure, and `downloads::refused` carries it, so the hosts add no code for those.
  - Each host maps its engine's native failure code in one small pure function ending `_ => Other`:
    - WebView2 maps its `InterruptReason`, sharing one reader with `interrupted()`. A `SetResultFilePath` failure maps to `FolderUnusable`, with the HRESULT's message as the detail.
    - CEF maps `interrupt_reason()` plus `is_canceled()`.
    - WebKit maps its three `DownloadError` arms.
  - All wording is in one app module.
- **D6. Kinds (7).**
  - `FolderUnreachable`: the host can't see the app's folder. In practice this is a WSL host given a folder it can't map (`HostRuntime::host_path` -> `None`), or a host started by hand with no folder or a relative one. The app always sends a folder, so it is never worded "no download folder".
  - `FolderUnusable`: the folder can't be made, isn't a folder, can't be written to, or the name is too long. It includes `FILE_ACCESS_DENIED`, `FILE_FAILED`, `FILE_NAME_TOO_LONG` and similar.
  - `DiskFull`.
  - `Blocked`: security software, a policy, or a malware verdict.
  - `Network`.
  - `Server`.
  - `Other`.
  - There is no `Cancelled` kind: nothing in the app can cancel a download today, and an engine's cancel maps to `Other`.
- **D7. Card.**
  - The kind's short phrase replaces "Download failed" on the card face; `Other` keeps "Download failed".
  - Each short phrase is at most about 18 characters, guarded by a table test. The face text draws at a fixed 13 screen points, unclipped, on cards down to about 116 pt wide. The 116 is `DOWNLOAD_NODE_SIZE` (= `GRAPH_NODE_SIZE`, 160 by 110, `card.rs:45,214`) at 80 pt tall, below which `tab_has_header` is false and `download_body.rs` skips the face (`card.rs:109,150-152`); the face is `push_centered_text` (`card.rs:162-173`, `download_body.rs:54-55`). This plan's test measures each phrase's width rather than counting characters (P17).
  - The tooltip and the card's accessible name carry the long text, the folder and the detail. The folder comes from the app's `download_folder()`, not the path's parent, because a `FolderUnreachable` path is a bare file name. P5 and P12 depart from this wording and wait for the owner's answer.
  - The popup row reads "Failed: <short>".
  - The toast keeps the path and appends " (<short>)".
- **D8. Node state.**
  - One `DownloadState { InFlight, Completed, Failed(DownloadFailure) }` replaces the `completed` and `failed` bools (`node.rs:402-410`).
  - The bools' doc chose a bool "without needing a separate enum". This reverses that choice; update the doc.
  - Constructors replace the 3 production literals (`fetched.rs:166`, `fetched.rs:526`, `session_sync/remote.rs:299`) and the 16 test fixtures that build the payload field by field.
- **D9. WebKit** sends the kind with an empty detail and writes glib's `error.message()` to its stderr log. GLib messages can embed a Linux path, which `in_app_terms` would not map.
- **D10. Probe first.**
  - Before the `FolderUnusable` mapping is fixed, one WebView2 run downloads into `C:\` and logs the real InterruptReason. No `FILE_*` reason has ever been observed (runs.md `2026-10-28-dldir-result2`, `-profile-bad`).
  - The instrumentation is temporary, in the style of earlier probes: `PROBE_*` env vars read by the host and the gate, run through `run.ps1 -Kind probe`, at a `-dirty` commit. It is reverted with `git restore --source=<commit>` and the revert is verified.
  - If Windows lets the write through, the one stray file in `C:\` is deleted by exact name.
  - The finding goes in `docs/runs.md`, and as prose wherever the mapping is documented.
- **D11. Protocol.** `PROTOCOL_VERSION` goes 7 -> 8. Re-record `RECORDED_FINGERPRINT`, and append every `FailureKind` value to the fingerprint bytes the way `MouseButton`'s values are.
- **Housekeeping (same work).**
  - PARKED's "A download refused for want of a folder shows a bare file name":
    - Delete it once the probe confirms that `C:\` is classified.
    - If WebView2 reports nothing usable for `C:\`, keep a narrowed entry instead.
  - Edit three PARKED entries:
    - the watchdog entry: the watchdog's failure now carries a kind;
    - "CEF and WebKit download paths unexercised": their mappings are unexercised too;
    - the Retry entry: only `Network`, `Server` and `Other` are worth retrying.
  - Update these docs:
    - the README download paragraph;
    - the `DownloadFailed` doc;
    - the protocol's version history;
    - the `refused` doc.
- **Out of scope.**
  - A write-access pre-check in `destination` (decided against: it would write into the user's folder).
  - Toast coalescing.
  - The click action on a failed card.
  - A CEF/WebKit runtime probe. That is item 5, a later plan, in which `gate page-kind` gains a good and a refused download case per engine.
- **Build constraints.**
  - CEF is not in the gate build line. It builds on Windows, warm, in `target/`.
  - WebKit builds only on Linux (`target-linux` via WSL).
  - The last task compile-checks both.
  - C: has about 5 GB free, so the CEF check must reuse the warm `cef-dll-sys` build (`9c45eb8d830cb15c`) and must never start a cold one.

## Decisions for review

These are choices this plan makes inside the approved design. Each lists what it recommends and the alternative.

**Owner sign-off before building.** Three of them depart from the approved design's wording, so they wait for the owner's answer, recorded on each one's `Answer:` line:
- P5 (D7: the reason goes in the card's accessible name; P5 extends the change to every card kind's tooltip);
- P7 (D5: the `SetResultFilePath` detail is the HRESULT's message; P7 keeps it only when Task 1 saw it name no path);
- P12 (D7: the tooltip's folder is `download_folder()`; P12 uses a full path's own folder).

Task 3 Step 4 needs P7's answer, and Task 8 needs P5's and P12's. An executor that reaches either step while its `Answer:` line still reads "pending" stops and asks; it never picks an option itself.

- **P1. `Refusal` struct.**
  - Recommended: `destination` returns `Result<PathBuf, Refusal>`, with `pub struct Refusal { pub path: PathBuf, pub failure: DownloadFailure }`, and `refused(refusal, url)` takes it whole. The path and its reason travel together.
  - Alternative: `Err((PathBuf, DownloadFailure))`, which names neither half.
- **P2. Placeholders in Task 2.**
  - Recommended: Task 2 changes the wire type. To keep every commit compiling, `refused` and the three hosts' engine-failure sites send `DownloadFailure::new(FailureKind::Other, "")` until Tasks 3, 5, 6 and 7 replace them. Task 10 greps for any that are left.
  - Alternative: one large task covering protocol, refusals and all three hosts.
- **P3. Wording module.**
  - Recommended: a new `crates/manifold/src/download_failure.rs`, since the card, the popup and the toast all use it. It is D5's "one app module".
  - Alternative: inside `card/download_body.rs`.
- **P4. `Other` has no short reason.**
  - The face reads "Download failed" (D7), the popup row "Failed", and the toast "Download failed: <path>", unchanged.
  - The D7-literal reading would produce "Failed: Download failed" and "Download failed: a.zip (Download failed)".
- **P5. Screen readers hear a card's tooltip, for every card kind.**
  - D7 names `GraphNode::accessible_detail` (`node.rs:451`). That is a `GraphNode` method: it can't see `App::download_folder()`.
  - Its result, `App::accessible_name`, is also shown as visible text: the card pane heading beside the Rename pencil (`graph_ui/card_pane.rs:83`), the node menu's disconnect rows (`node_menu.rs:839`) and the omnibar's rows (`omnibar.rs:132`). A long reason there would push the name aside in each of those places.
  - Recommended: the card's button label, built in `interact_card` (`card.rs:1234-1239`), becomes `card_label(accessible_name, tooltip)`, which appends each tooltip line as a sentence.
    - It applies to every card that has a tooltip, so these cards' AccessKit labels change too, each gaining its tooltip's sentences after its unchanged name:
      - an unloaded tab that failed (the engine and its error), or is sleeping or waking (`card/tab_body.rs:53-66`);
      - a live tab that is not responding (`card/tab_body.rs:410`);
      - a tab with a jar chip's tooltip (`card/tab_body.rs:69`, `:292`);
      - a group whose name has hidden characters (`card/group_body.rs:28`);
      - an image, JSON or PDF card with an error (`card/image_body.rs:67`, `card/json_body.rs:31`, `card/pdf_body.rs:43`).
    - A screen reader can't hover, so it never hears any of these today.
    - `interact_card` builds the label only through `card_label`, so a card's name part is unchanged for every kind. `a_cards_label_carries_its_tooltip` (Task 8) pins that for a non-download card: a not-responding tab's label is its old name plus the tooltip's sentence.
    - The gate clicks no card by name. At d047184e, `click_control` and `click_named` in `crates/gate/src` name only toolbar, menu and settings controls.
  - Alternative A: apply `card_label` to download cards only. In `interact_card`, pass `body.tooltip.as_deref().filter(|_| self.nodes[&id].as_download().is_some())` to `card_label`, and drop "Every kind's tooltip counts" from `card_label`'s doc. The test's tab case stays, since it tests `card_label` alone.
  - Alternative B (D7-literal): put the reason in `accessible_detail`. It would have no folder, and it would show in the pane heading and the omnibar. If the owner picks B, stop: Task 8 needs re-planning.
  - Answer: pending (owner).
- **P6. WebKit `filename_to_uri` failure.**
  - This is a third refusal feeder, outside `destination` (`main.rs:113-116`).
  - Recommended: refuse it as `Other` with an empty detail, writing GLib's words to the host's stderr.
- **P7. Details.**
  - WebView2 and CEF engine codes get an empty detail. The code's name, such as `NETWORK_FAILED`, is jargon, and the tooltip already has the long line.
  - WebView2's `SetResultFilePath` failure: D5 gives it the HRESULT's message as the detail, and also says a detail never holds a path. No `SetResultFilePath` failure has been seen yet, so nothing shows that its message keeps the second rule. Task 3 has two variants:
    - Recommended: the safe form, **A3-log**, by default. The detail is empty and the message goes to the host's stderr (`eprintln!`). **A3-msg**, the message as the detail, is used only when Task 1 branch (a) saw the message and it names no path. In branches (b) to (e) no message was seen, so they use A3-log.
    - Alternative (D5-literal): A3-msg unless Task 1 saw a message that names a path. Branches (b) to (e) then put a message nobody has seen into the tooltip, with nothing checking it for a path.
    - Answer: pending (owner).
  - WebKit's detail is always empty (D9).
  - `destination`'s refusals carry the `io::Error`'s `Display`, which never includes a path.
- **P8. Mapping choices beyond D6.**
  - `FILE_TOO_LARGE` -> `FolderUnusable`: the folder's file system can't hold the file.
  - Certificate problems -> `Server`.
  - CEF's `SERVER_UNREACHABLE` -> `Server`. It is Chromium's "unexpected server response"; WebView2 calls the same code `SERVER_UNEXPECTED_RESPONSE`.
  - `Other` for these, since none says anything the user can act on:
    - `FILE_TRANSIENT_ERROR`, `FILE_TOO_SHORT`, `FILE_HASH_MISMATCH` and `FILE_SAME_AS_SOURCE`;
    - the `USER_*` codes;
    - the crash codes;
    - WebKit's `CancelledByUser`.
- **P9. Debug export.**
  - Recommended: the export keeps `completed` and `failed` and adds `failure` (the kind's name, or `null`). The gate's other download views keep reading the bools unchanged.
  - The `download-failed` view also checks `failure == "Network"` (Task 5) and the card's accessible label (Task 8).
- **P10. `DownloadPayload::finished(path, bytes)` is pure.**
  - The caller reads the file's size, as today.
  - Completed test fixtures that had `total_bytes: None` get `Some(0)`.
  - In-flight fixtures with `bytes_received: 1` get 0.
  - No test reads either value; Task 4 stops if one does.
- **P11. Absolute test paths come from `temp_dir`.** A Windows path like `C:\dl` is not absolute on the Linux CI, so tests that need one build it from `temp_dir`.
- **P12. The tooltip's folder.**
  - Recommended: the failed path's own folder when the path is a full path, and the app's `download_folder()` only for a path without one (the bare name of a `FolderUnreachable` refusal).
  - Why: D7's reason for `download_folder()` was the bare-name case. For a full path, the parent is the folder that failed, and it stays right after the user changes the setting. With `download_folder()` alone, a card that failed in `C:\` would name the new folder.
  - Alternative (D7-literal): always `download_folder()`. If the owner picks it:
    - `folder_of` and its doc go, and `download_tooltip` passes `self.download_folder()`;
    - in `a_failed_cards_tooltip_names_its_folder_and_survives_a_rename`, the "a full path keeps its own folder" assert names `&b` instead, with the message "every card names the folder the app means now", and its doc and Review Focus 5 change to match;
    - Task 9's two PARKED texts that cite `download_failure::folder_of` cite `App::download_folder` instead.
  - Answer: pending (owner).
- **P13. CEF `is_canceled()` -> `Other`, checked before the reason.** A cancelled item's reason is `USER_CANCELED`, which is already `Other`. The flag guards against an engine that cancels with some other reason. This is D5's "plus `is_canceled()`".
- **P14. The probe reuses the gate's `download-real` view.**
  - It overrides the seeded folder through `PROBE_DL_DRIVE` in `instance::seed_settings`, rather than adding a gate view.
  - The override never touches `instance::download_dir`, which `download_views` deletes before it starts.
- **P15. `DownloadState` access.** The field `pub(crate) state`, plus three accessors (`in_flight`, `is_completed`, `failure`) for the readers that only ask one question.
- **P16. `-j 4`.** CLAUDE.md's lines say `-j 8`. The design says `-j 4` for this shared machine, and every cargo line here uses `-j 4`.
- **P17. The face test measures width, not characters.**
  - Recommended: lay each face out in the app's own font (`theme::apply_fonts`) at 13 points, the size the face draws at, and require it to fit 116 points less the card's 6-point margin on each side (`GRAPH_THUMB_MARGIN`), so 104 points. The test pins the 116 too, with `tab_has_header` on a `DOWNLOAD_NODE_SIZE` card: the face draws at 117 points wide and not at 116.
  - Why: the text is drawn unclipped (`identity::paint_centered`, `layout_no_wrap`), and a character count does not bound its width. Measured with fontTools from `FamiljenGrotesk-Regular.ttf` advance widths at 13 points, without kerning: "Can't reach folder" is about 98 points and fits, but "Folder unreachable", also 18 characters, is about 107 points and would not.
  - Alternative: the design's 18-character count. It is cheaper, but it is only a proxy for width.
  - The face that no gate view draws, `FolderUnreachable` (a WSL host or a host with no folder), is covered by this test, not by a shot.

## Global Constraints

- **Start condition.**
  - `git status --short -- crates README.md PARKED.md` must print nothing before Task 1 and before each commit, apart from the current task's own files.
  - This is the guard against another session's work. Tasks 4 and 8 stage with `git add crates/manifold/src`, which would sweep in any foreign change there. If anything else is listed, stop and report it. Never stage, revert or edit another agent's changes.
  - The other agent's chrome-shots work, which was uncommitted when this plan was drafted, is committed as d047184e, the base. When this was checked (2026-10-28, at d047184e), the tree was clean apart from this plan file.
- **One cargo per target dir.** Before any cargo command, run `Get-Process cargo -ErrorAction SilentlyContinue` (PowerShell). If one is running, wait for it to exit: it is another session's build in `target/`.
- **Commands.**
  - Every cargo line uses `-j 4` (P16).
  - The clippy line: `cargo clippy -j 4 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`.
  - The test lines are listed below. The app has no lib target, hence `--bin manifold`.
    - `cargo test -j 4 -p manifold --bin manifold`
    - `cargo test -j 4 -p manifold-view-protocol`
    - `cargo test -j 4 -p manifold-view-webview2`
    - `cargo test -j 4 -p gate`
  - The gate build line: `cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`.
- **Formatting and commits.**
  - Run `cargo fmt --all` before each commit, and check `git diff --stat` first.
  - The pre-commit hook runs fmt and clippy. Never bypass it with `--no-verify`. Never write an `#[allow]` without a `reason`.
  - Commit messages end with the `Co-Authored-By:` trailer that your own session's system reminder gives. It names your session's model, which differs between the planner, implementers and reviewers.
    - Each commit block below writes the trailer as the placeholder `<attribution>`. Put that line in its place.
    - A message ending in the literal placeholder is wrong. After each commit, check it with `git log -1 --format=%B | tail -n 1`. If it is wrong, fix it before going on with `git commit --amend -F -` and the corrected message in a heredoc. A bare `--amend` opens an editor, which this environment can't use.
  - Never push, tag or bump.
  - No CHANGELOG edit: entries are written at the bump.
  - No migration code and no compatibility shims.
- **Tests and the user's files.**
  - No test reads or writes the real `%APPDATA%`. Tests that need folders use `crate::store::testutil::temp_dir` (app) or `std::env::temp_dir()` (protocol, WebView2).
  - No run writes into the user's Downloads folder. Each run step diffs it before and after.
- **Python.** Run Python only as `python -P f.py < /dev/null`, with the script written to a file first. No step here needs Python.
- **Detail rule.** `DownloadFailure::detail` never holds a path. Only `DownloadFailed::path` is mapped between a WSL host's terms and the app's (`process_manager::in_app_terms`).
- **Wording.** The app's words for a failure live only in `crates/manifold/src/download_failure.rs`.
- **CEF and WebKit.**
  - Neither is in the clippy line or the hook. CEF compiles only through the CEF guard below; WebKit only through WSL, in Tasks 6, 7 and 10.
  - Tasks 2 and 3 edit both hosts without compiling them. Their edits are mechanical, and Tasks 6 and 7 compile and test them.
- **Disk.** C: has about 5 GB free. Make no new target dir. The CEF guard refuses to start a cold `cef-dll-sys` build.
- **While a run is live.** Never edit sources while a probe or gate run started from this repo is live.

### Pre-run check (before every `run.ps1` launch)

1. In PowerShell, run `Get-Process cargo, gate, manifold -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, Path`.
   - Any `cargo` or `gate` belongs to another session: wait for it to exit.
   - A `manifold` whose `Path` is this repo's `target\debug\manifold.exe` holds the exe the build replaces: ask the user to close it.
   - Any other `manifold` is the user's own app: leave it.
2. Find unfinished runs.
   - `grep -nE "T[0-9:]+ \|  \|  \|" docs/runs.md | cut -c1-100` lists the rows with an empty `end`.
   - For each, find the run's `wrapper pid N` (in its notes cell, or `"notes"` in its `run.json`) and run `tasklist //FI "PID eq N"`.
   - A live wrapper is a run in progress: wait for it.
   - A dead one is a lost run: close it as RUNS.md "Finding and closing" says, and commit that on its own (`runs: close <n> lost rows`).
3. Save the user's Downloads folder listing: `ls -A "$USERPROFILE/Downloads" > "<scratchpad>/downloads-before-<run name>.txt"`. `<scratchpad>` is your session's scratchpad directory.

### Waiting for a run

Run this as a background Bash call (`run_in_background: true`), with `<id>` being the run id that run.ps1 printed. Use deadline 900 s for a probe and 4500 s for the full gate:

```bash
RUN="$LOCALAPPDATA/claude-runs/manifold/<id>"; deadline=$((SECONDS+900)); until grep -Eq '"end": +"' "$RUN/run.json" || [ $SECONDS -ge $deadline ]; do sleep 5; done; grep -E '"(exit|headline|commit)"' "$RUN/run.json"; tail -n 15 "$RUN/out.log"
```

Afterwards, `ls -A "$USERPROFILE/Downloads" | diff "<scratchpad>/downloads-before-<run name>.txt" -` must print nothing. Any difference is a stop-and-report: name each new file.

A run that fails keeps its work folder; `probes::finish` deletes it only on a pass. Remove it once its output has been read: `rm -rf -- "$RUN"/work/gate-chrome-shots-*`.

### CEF guard (create once per session)

Write this to `<scratchpad>\cef-check.ps1` with the Write tool:

```powershell
# Compile-checks (check) or tests (test) manifold-view-cef on the warm cef-dll-sys build only; never starts a cold one.
param([ValidateSet('check', 'test')][string]$Cmd = 'check', [Parameter(Mandatory)][string]$Logs)
$ErrorActionPreference = 'Stop'
$repo = 'C:\Users\Winston Li\Documents\GitHub\manifold'
$build = Join-Path $repo 'target\debug\build'
$warm = Join-Path $build 'cef-dll-sys\9c45eb8d830cb15c'
function Get-CefBuildDirs {
    @(Get-ChildItem -Directory (Join-Path $build 'cef-dll-sys') -ErrorAction SilentlyContinue | ForEach-Object FullName) +
    @(Get-ChildItem -Directory $build -Filter 'cef-dll-sys-*' -ErrorAction SilentlyContinue | ForEach-Object FullName)
}
if (-not (Test-Path (Join-Path $warm 'out\cef_windows_x86_64\libcef.dll'))) {
    'not checked: the warm cef-dll-sys build (9c45eb8d830cb15c) is gone'; exit 2
}
$before = Get-CefBuildDirs
Remove-Item Env:CEF_PATH, Env:FLATPAK -ErrorAction SilentlyContinue
$env:PATH = ($env:PATH -split ';' | Where-Object { $_ -notmatch 'depot_tools' }) -join ';'
$cargoArgs = if ($Cmd -eq 'check') { 'check -j 4 -p manifold-view-cef --all-targets' } else { 'test -j 4 -p manifold-view-cef --no-run' }
$out = Join-Path $Logs "cef-$Cmd.out.log"; $err = Join-Path $Logs "cef-$Cmd.err.log"
$p = Start-Process cargo -ArgumentList $cargoArgs -WorkingDirectory $repo -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput $out -RedirectStandardError $err
$null = $p.Handle
try { $p.PriorityClass = 'BelowNormal' } catch {}
$deadline = (Get-Date).AddMinutes(20)
while (-not $p.HasExited) {
    $new = @(Get-CefBuildDirs | Where-Object { $before -notcontains $_ })
    if ($new.Count -gt 0) {
        taskkill /T /F /PID $p.Id | Out-Null
        $p.WaitForExit()
        $new | ForEach-Object { Remove-Item -Recurse -Force -LiteralPath $_ }
        "not checked: cargo started a new cef-dll-sys build ($($new -join ', ')); killed it and deleted only those folders"
        exit 2
    }
    if ((Get-Date) -gt $deadline) { taskkill /T /F /PID $p.Id | Out-Null; 'not checked: over 20 minutes'; exit 2 }
    Start-Sleep -Milliseconds 500
}
$p.WaitForExit()
"cargo $Cmd exit $($p.ExitCode)"
Get-Content $err -Tail 12
if ($p.ExitCode -ne 0 -or $Cmd -eq 'check') { exit $p.ExitCode }
# The test exe imports libcef.dll: run it directly with the warm build's CEF folder on PATH (cargo never sees that PATH).
$exe = Select-String -Path $err -Pattern '\((target[\\/]debug[\\/]deps[\\/]manifold_view_cef-[0-9a-f]+\.exe)\)' |
    Select-Object -Last 1 | ForEach-Object { $_.Matches[0].Groups[1].Value }
if (-not $exe) { 'compiled, not run: cargo named no test executable'; exit 3 }
$env:PATH = (Join-Path $warm 'out\cef_windows_x86_64') + ';' + $env:PATH
$tout = Join-Path $Logs 'cef-tests.out.log'; $terr = Join-Path $Logs 'cef-tests.err.log'
$t = Start-Process (Join-Path $repo $exe) -WorkingDirectory $repo -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput $tout -RedirectStandardError $terr
$null = $t.Handle
if (-not $t.WaitForExit(300000)) { taskkill /T /F /PID $t.Id | Out-Null; 'not run to the end: over 5 minutes'; exit 2 }
Get-Content $tout -Tail 20
if ($t.ExitCode -eq -1073741515) { 'compiled, not run: STATUS_DLL_NOT_FOUND (0xc0000135)'; exit 3 }
exit $t.ExitCode
```

Run it from bash as `powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd check -Logs "<scratchpad>"` (or `-Cmd test`); the scratchpad path is in 8.3 form and has no space. The exit codes are:
- 0: passed.
- 1 or 101: a compile error or a test failure. Read the log.
- 2: not checked. Report it; never work around it.
- 3: compiled but not run. Report it as such.

### WebKit check

```bash
wsl.exe -d Ubuntu -- bash -lc 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo build -j 4 -p manifold-view-webkitgtk && CARGO_TARGET_DIR=target-linux cargo test -j 4 -p manifold-view-webkitgtk'
```

`target-linux` is warm (built 2026-10-28). Use a 600000 ms timeout.

## Review Focus

Each is an input or condition the spec implies but no single unit test of a mapping exercises, most likely first. The test that pins it is named after the line.

1. **A WebKit host under WSL that can't see the app's folder.** It refuses with a bare file name (`FolderUnreachable`), and the tooltip must still name the folder the app meant, not an empty one. Pinned in Task 8, `a_failed_cards_tooltip_names_its_folder_and_survives_a_rename`.
2. **The detail is never a path, and never on the face, popup or toast.** Held three ways:
   - Task 3's `a_destination_is_made_in_the_folder_or_refused` (the OS's words name no folder);
   - the `SetResultFilePath` message becomes the detail only when Task 1 saw it name no path (Task 3's A3-msg); otherwise it goes to the host's log (A3-log, P7);
   - by signature: `card_face`, `popup_row` and `toast` take a `FailureKind`, never the `DownloadFailure`.
3. **A renamed failed card still says why.** The rename replaces the name a screen reader hears first, not the reason after it, and the face is drawn from the state. Pinned in Task 8, same test as 1.
4. **The same file downloaded again after its card failed.** It gets a card of its own, and the failed card stays failed. Pinned in Task 4, `a_redownload_after_a_failure_gets_its_own_card`.
5. **The Download folder setting changed after a failure.** A card that failed at a full path keeps naming that path's folder; a bare-name card names the folder the app means now (P12). Pinned in Task 8, same test as 1.

## File Structure

- `crates/manifold-view-protocol/src/messages.rs`: `FailureKind`, `DownloadFailure`, the `DownloadFailed` field, and the fingerprint test.
- `crates/manifold-view-protocol/src/lib.rs`: the re-exports, `PROTOCOL_VERSION` 8 and its history line.
- `crates/manifold-view-protocol/src/downloads.rs`: `Refusal`, `destination` and `unmade_folder` (the classification), and `refused`.
- `crates/manifold-view-webview2/src/download.rs`: `interrupt_reason`, `failure_kind`, and the `SetResultFilePath` refusal.
- `crates/manifold-view-cef/src/download_handler.rs`: `failure_kind` and its first test module.
- `crates/manifold-view-webkitgtk/src/main.rs`: `failure_kind`, `connect_failed`, the `filename_to_uri` refusal, and the crate's first test module.
- `crates/manifold/src/node.rs`: `DownloadState`, plus `DownloadPayload`'s constructors and accessors.
- `crates/manifold/src/download_failure.rs` (new): all wording, the tooltip, and `App::download_tooltip`.
- `crates/manifold/src/card.rs`: `card_label` and the card's accessible label.
- `crates/manifold/src/card/download_body.rs`: the face and the tooltip.
- `crates/manifold/src/popups/dropdowns.rs`, `event_loop/view_message.rs`, `debug_export.rs`, `fetched.rs`, `session_sync/{remote,writes}.rs`, `tabs/sleep.rs` and `process_manager.rs`: readers and the message path.
- 16 test fixtures: `bench.rs`, `card/tests.rs`, `card_kind.rs`, `level.rs`, `links.rs`, `link_gestures/tests.rs`, `node.rs`, `session_sync/tests/{links,writes}.rs` and `trail_strip.rs`.
- `crates/gate/src/shots.rs` (the `download-failed` check) and `crates/gate/src/state.rs` (the `downloads_from` doc), plus `crates/gate/src/instance.rs` and the WebView2 `download.rs` (Task 1's throwaway probe only).
- `README.md`, `PARKED.md`, `docs/runs.md`: the docs.

---

### Task 1: Probe what WebView2 reports for a download into `C:\`

**Files:**
- Throwaway edits (reverted in this task): `crates/gate/src/instance.rs` (`seed_settings`) and `crates/manifold-view-webview2/src/download.rs`.
- Modify: `docs/runs.md`, written by the wrapper, plus your headline.
- Modify: this plan, its "Probe finding (Task 1)" section.

**Interfaces:**
- Consumes: nothing.
- Produces: the probe finding below, one of branches (a) to (e). Task 3 picks its WebView2 `SetResultFilePath` variant from it; Task 5 and Task 9 quote it.

- [ ] **Step 1: Record the base, and check the start condition**

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git status --short
git rev-parse --short HEAD
```
Expected:
- `git status --short` lists nothing under `crates/`, and neither `README.md` nor `PARKED.md`. Only this plan file may appear, as untracked or modified. If anything else is listed, stop and report it (Global Constraints, "Start condition").
- Write the printed hash into this plan's "Probe finding (Task 1)" section as `Base (B):`.
- Set this plan's front matter to `status: active`.
- Commit the plan file alone, so the run's `-dirty` names only the probe's edits:

```bash
git add docs/superpowers/plans/2026-10-28-download-failure-reason.md
git commit -F - <<'EOF'
plans: download failure reason (active)

<attribution>
EOF
C=$(git rev-parse HEAD); echo "$C"
```
Expected: one file committed; the hook passes (no Rust files are staged); a commit id is printed. Call it C.

- [ ] **Step 2: Pre-run checks, plus the `C:\` root**

Run the Pre-run check (Global Constraints) with run name `dl-fail-croot`. Then:

```bash
ls -A /c/ > "<scratchpad>/c-root-before.txt"
ls -A /c/ | grep -i "real" ; echo "exit $?"
```
Expected: `exit 1`, meaning nothing in `C:\` is named like `real*`. Otherwise stop: the probe would name its file `real (1).zip`, and the cleanup below must never touch a file the user owns.

- [ ] **Step 3: The throwaway instrumentation (do not commit)**

In `crates/gate/src/instance.rs` `seed_settings`, replace

```rust
    let downloads = serde_json::Value::from(download_dir(profile)?.to_string_lossy().into_owned());
```

with

```rust
    let downloads = match std::env::var("PROBE_DL_DRIVE") {
        // PROBE (download-failure plan Task 1, reverted): the root of drive PROBE_DL_DRIVE, not the instance's folder.
        Ok(drive) => serde_json::Value::from(format!("{drive}:\\")),
        Err(_) => serde_json::Value::from(download_dir(profile)?.to_string_lossy().into_owned()),
    };
```

Do not change `download_dir`: `download_views` deletes that folder before it starts (`fs::remove_dir_all(&folder)`).

In `crates/manifold-view-webview2/src/download.rs`, make five edits.

1. After the `use windows::core::{Interface, HSTRING, PWSTR};` line, add:

```rust
/// PROBE (download-failure plan Task 1, reverted): appends `line` to the file `PROBE_DL_OUT` names.
fn probe_log(line: &str) {
    use std::io::Write;
    let Ok(path) = std::env::var("PROBE_DL_OUT") else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}
```

2. In `register`, replace

```rust
    if let Err(e) = set_naming_folder(webview) {
```

with

```rust
    let named = set_naming_folder(webview);
    probe_log(&format!("naming folder: {named:?}"));
    if let Err(e) = named {
```

3. Right after the `let decided = { ... };` block (the one ending in `save_path(&seen, &uri, broken_at_start, &proposed, downloads::folder().as_deref())` and `};`), add:

```rust
        probe_log(&format!("starting uri {uri} proposed {proposed} decided {decided:?}"));
```

4. In `placed`, replace `            Err(_) => Err(path),` with:

```rust
            Err(e) => {
                probe_log(&format!(
                    "SetResultFilePath {} failed: hresult {:#010x} message {:?}",
                    path.display(),
                    e.code().0,
                    e.message()
                ));
                Err(path)
            }
```

5. In the `StateChanged` handler, right after `if unsafe { sender.State(&mut state) }.is_ok() {`, add:

```rust
                    let mut probe_reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
                    let _ = unsafe { sender.InterruptReason(&mut probe_reason) };
                    probe_log(&format!("state {} reason {} path {path_text}", state.0, probe_reason.0));
```

- [ ] **Step 4: Build and launch**

```bash
cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate 2>&1 | tail -n 2
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name dl-fail-croot -Detach -Command 'set "PROBE_DL_DRIVE=C"& set "PROBE_DL_OUT=%RUN_DIR%\probe.txt"& set "VIEWS=download-real"& set "OUT=%RUN_DIR%\shots"& target\debug\gate.exe chrome-shots "%RUN_DIR%\work"'
```
Expected:
- The build prints `Finished` with no warning.
- run.ps1 prints the run id (`<date>-dl-fail-croot`).
- The run takes about 2-3 minutes: the view waits up to `STEP_TIMEOUT` (90 s, `instance.rs:144`) for a completed card in the instance's own folder, which never comes.

- [ ] **Step 5: Wait, then read the finding**

Wait as in "Waiting for a run" (deadline 900 s). Expected:
- `"exit"` is 1.
- `out.log` has the view's failure line (`shots.rs` `step` prints it; the sheet line comes after it, so it is not the last line): `FAIL: downloads: the download of http://127.0.0.1:<port>/real.zip is not one completed card saving to …\real.zip: [...]`. The bracket is the card list in `serde_json` Debug form, and the card's path shows as `"path": String("C:\\real.zip")` (Debug doubles the backslash). `grep -n "^FAIL: downloads" "$RUN/out.log"` finds it.
- `"commit"` ends `-dirty`.

If the card list is empty or its path is not `C:\real.zip`, note it; that is branch (e) or (d) below. Any other error (the instance failed to start, the server failed) is a stop-and-report with the run id.

```bash
RUN="$LOCALAPPDATA/claude-runs/manifold/<id>"; cat "$RUN/probe.txt"
ls -A /c/ | diff "<scratchpad>/c-root-before.txt" -
```

`probe.txt` holds:
- a `naming folder: Ok(())` line;
- a `starting uri http://127.0.0.1:<port>/real.zip proposed … decided Ok("C:\\real.zip")` line;
- then exactly one of the branches below.

Name the branch:
- **(a)** A line `SetResultFilePath C:\real.zip failed: hresult 0x… message "…"`, and no `state` line: Edge refused the path up front.
  - Check whether the message names a path: it contains `C:\`, `real` or `.zip`.
  - Task 3 uses variant **A3-msg** if it doesn't, and **A3-log** if it does.
- **(b)** A line `state 1 reason R path C:\real.zip`, where 1 is `INTERRUPTED`.
  - R in {1, 2, 4, 5} (`FILE_FAILED`, `FILE_ACCESS_DENIED`, `FILE_NAME_TOO_LONG`, `FILE_TOO_LARGE`) confirms Task 5's `FolderUnusable` mapping.
  - Any other R is a stop-and-report: the mapping in D6 does not hold for `C:\`, and the user decides.
  - Task 3 uses **A3-log**: no `SetResultFilePath` failure was seen, so nothing shows that its message names no path (P7).
- **(c)** A line `state 2 reason 0 path C:\real.zip`, where 2 is `COMPLETED`: Windows let the write through.
  - The `C:\` diff shows `real.zip`. Delete it by exact name: `rm -- /c/real.zip`.
  - Task 3 uses **A3-log** (P7).
- **(d)** A `starting` line, but no `SetResultFilePath` and no `state 1`/`state 2` line: the download never ended within the wait. Task 3 uses **A3-log** (P7).
- **(e)** No `starting` line at all: Edge fired no `DownloadStarting`, the same family as run `2026-10-28-dldir-profile-bad`. Task 3 uses **A3-log** (P7).

These variant choices follow P7's recommendation. If the owner's answer to P7 is the D5-literal alternative, (b) to (e) use A3-msg instead.

The `C:\` diff may show new entries. Only `real.zip`, `real.zip.crdownload` or `Unconfirmed *.crdownload` may be deleted, each by exact name (`rm -- "/c/<name>"`). Any other new entry is a stop-and-report: leave it alone. The Downloads diff must print nothing.

Remove the kept work folder: `rm -rf -- "$RUN"/work/gate-chrome-shots-*`.

- [ ] **Step 6: Revert to C and prove it**

```bash
C=<the commit id from Step 1>
git restore --source="$C" -- crates/gate/src/instance.rs crates/manifold-view-webview2/src/download.rs
git status --short -- crates
git diff "$C" --stat -- crates
```
Expected: both commands print nothing. (`docs/runs.md` has the run's row; it is committed below.)

- [ ] **Step 7: Record the finding**

- In `docs/runs.md`, set the `2026-…-dl-fail-croot` row's headline cell, and `"headline"` in its `run.json`, to one line. Use the template for your branch, with the real values filled in:
  - (a): `C:\ root: SetResultFilePath fails <hresult> "<message>"; refused before any reason`
  - (b): `C:\ root: INTERRUPTED reason <R> <NAME>`
  - (c): `C:\ root: COMPLETED (Windows let the write through); stray C:\real.zip deleted`
  - (d): `C:\ root: no final state within 90 s`
  - (e): `C:\ root: no DownloadStarting`
- Fill in this plan's "Probe finding (Task 1)" section:
  - the run id;
  - the branch letter;
  - the `probe.txt` lines, verbatim;
  - the Task 3 variant;
  - for (a), whether the message names a path.

```bash
git diff --stat
git add docs/runs.md docs/superpowers/plans/2026-10-28-download-failure-reason.md
git commit -F - <<'EOF'
runs: dl-fail-croot probe (WebView2 download into C:\)

<attribution>
EOF
```
Expected: two files.

---

### Task 2: `DownloadFailure` on the wire (protocol 8)

**Files:**
- Modify: `crates/manifold-view-protocol/src/messages.rs`:
  - after the `ViewToHost` enum, the new types;
  - `DownloadFailed`, lines 553-555;
  - the `wire_shape_tests` module, lines 718-965.
- Modify: `crates/manifold-view-protocol/src/lib.rs:14-18` (re-exports) and `:21-32` (history and version).
- Modify: `crates/manifold-view-protocol/src/downloads.rs`:
  - `:8` (import);
  - `:163-165` (`refused`, placeholder);
  - `:238` (test pattern).
- Modify: `crates/manifold-view-webview2/src/download.rs`:
  - `:37` (import);
  - `:227-228` (placeholder).
- Modify: `crates/manifold-view-cef/src/download_handler.rs:7` (import) and `:132-136` (placeholder).
- Modify: `crates/manifold-view-webkitgtk/src/main.rs:10` (import) and `:153-158` (placeholder).
- Modify: `crates/manifold/src/process_manager.rs:945` (`in_app_terms`).
- Test: `crates/manifold/src/process_manager/tests.rs`, after `a_wsl_hosts_download_paths_arrive_in_windows_terms` (line 72).
- Modify: `crates/manifold/src/event_loop/view_message.rs`:
  - `:234` (pattern);
  - tests `:257-327`.

**Interfaces:**
- Consumes: nothing.
- Produces (all `pub`, re-exported at `manifold_view_protocol::`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureKind { FolderUnreachable, FolderUnusable, DiskFull, Blocked, Network, Server, Other }
impl FailureKind { pub const ALL: [FailureKind; 7]; }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadFailure { pub kind: FailureKind, pub detail: String }
impl DownloadFailure { pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self; }
// ViewToHost::DownloadFailed { path: String, failure: DownloadFailure }
pub const PROTOCOL_VERSION: u32 = 8;
```
- Placeholders that later tasks remove (P2): every `DownloadFailure::new(FailureKind::Other, "")` this task adds.

- [ ] **Step 1: Write the failing fingerprint test**

In `messages.rs` `wire_shape_tests`, make five edits.

1. In the module doc, replace "including every `InputEvent`, `MouseButton` and `KeyCode`." with "including every `InputEvent`, `MouseButton`, `KeyCode` and `FailureKind`."
2. After `const MOUSE_BUTTON_VARIANTS: usize = 3;`, add `    const FAILURE_KIND_VARIANTS: usize = 7;`.
3. After the `button_slot` fn, add:

```rust
    /// As `host_slot`, for `FailureKind`, which `ViewToHost::DownloadFailed` carries.
    fn failure_kind_slot(k: &FailureKind) -> usize {
        match k {
            FailureKind::FolderUnreachable => 0,
            FailureKind::FolderUnusable => 1,
            FailureKind::DiskFull => 2,
            FailureKind::Blocked => 3,
            FailureKind::Network => 4,
            FailureKind::Server => 5,
            FailureKind::Other => 6,
        }
    }
```

4. In `view_samples`, replace `            ViewToHost::DownloadFailed { path: "a.bin".into() },` with:

```rust
            ViewToHost::DownloadFailed {
                path: "a.bin".into(),
                failure: DownloadFailure::new(FailureKind::Network, "connection reset"),
            },
```

5. In `wire_shape_matches_the_recorded_fingerprint`, first add this after the `buttons` slot loop (the one ending `"buttons is out of step with button_slot"); }`):

```rust
        assert_eq!(FailureKind::ALL.len(), FAILURE_KIND_VARIANTS, "FailureKind has a new variant: add it to ALL and its index");
        for (i, k) in FailureKind::ALL.iter().enumerate() {
            assert_eq!(failure_kind_slot(k), i, "FailureKind::ALL is out of step with failure_kind_slot");
        }
```

Then, after the `KeyCode::ALL` `bytes.extend(...)` statement and before `bytes.extend(frames(&host));`, add:

```rust
        // Every `FailureKind`, as `DownloadFailed` carries it.
        let failed = |kind| ViewToHost::DownloadFailed { path: String::new(), failure: DownloadFailure::new(kind, "") };
        bytes.extend(frames(&FailureKind::ALL.map(failed)));
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -j 4 -p manifold-view-protocol wire_shape`
Expected: FAIL to compile, with errors naming `FailureKind` and `DownloadFailure` as undeclared (``use of undeclared type `FailureKind` ``, ``cannot find function, tuple struct or tuple variant `DownloadFailure` `` or similar).

- [ ] **Step 3: The types and the field**

In `messages.rs`, right after the `ViewToHost` enum's closing `}` (the line after `    PageText { text: String },`) and before `#[cfg(test)] mod key_code_tests`, add:

```rust

/// Why a download failed, by what the user can do about it (`DownloadFailure::kind`). Closed and payload-free: each
/// host maps its engine's own codes onto these in one function ending `_ => Other` (`failure_kind` in each host).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureKind {
    /// The host can't see the app's download folder: a WSL host given a folder it can't map, or a host started by
    /// hand with no folder or a relative one. The failed path is then the bare file name.
    FolderUnreachable,
    /// The folder can't be made, isn't a folder, can't be written to, or can't hold this file (a name too long, a
    /// file too large for its file system).
    FolderUnusable,
    /// The disk the folder is on is full.
    DiskFull,
    /// Security software, a policy, or a malware verdict blocked it.
    Blocked,
    /// The connection failed, timed out or dropped.
    Network,
    /// The server refused it or answered with an error.
    Server,
    /// Anything else, an engine's own cancel included: nothing the user can act on.
    Other,
}

impl FailureKind {
    /// Every kind, in declaration (wire) order.
    pub const ALL: [FailureKind; 7] = [
        FailureKind::FolderUnreachable,
        FailureKind::FolderUnusable,
        FailureKind::DiskFull,
        FailureKind::Blocked,
        FailureKind::Network,
        FailureKind::Server,
        FailureKind::Other,
    ];
}

/// Why `ViewToHost::DownloadFailed`'s download failed: the `kind` the app words, and `detail`, the engine's or OS's
/// own words for the tooltip, empty when there is nothing to add. `detail` never holds a path: only
/// `DownloadFailed::path` is mapped between a WSL host's terms and the app's (`process_manager::in_app_terms`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadFailure {
    pub kind: FailureKind,
    pub detail: String,
}

impl DownloadFailure {
    pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self { kind, detail: detail.into() }
    }
}
```

`ViewToHost` is `#[rustfmt::skip]`: keep its lines at most 120 wide by hand. Replace

```rust
    /// A download ended without finishing (cancelled, interrupted, network error) -- `path` is the one its
    /// `DownloadStarted` reported. `manifold` marks the card failed and stops treating it as in flight.
    DownloadFailed { path: String },
```

with

```rust
    /// A download ended without finishing -- `path` is the one its `DownloadStarted` reported, and `failure` says
    /// why: a refusal the host classified itself (`crate::downloads::destination`), or its engine's own failure code,
    /// mapped by the host's `failure_kind`. `manifold` marks the card failed, words the reason on it
    /// (`download_failure.rs`), and stops treating it as in flight.
    DownloadFailed { path: String, failure: DownloadFailure },
```

In `lib.rs`, add `DownloadFailure, FailureKind,` to the `pub use messages::{…}` list, before `HostSettings`.

- [ ] **Step 4: Every site compiles (P2 placeholders)**

`crates/manifold-view-protocol/src/downloads.rs`:
- Replace `use crate::ViewToHost;` with `use crate::{DownloadFailure, FailureKind, ViewToHost};`.
- In `refused`, replace `ViewToHost::DownloadFailed { path }]` with `ViewToHost::DownloadFailed { path, failure: DownloadFailure::new(FailureKind::Other, "") }]`. Task 3 replaces this placeholder.
- In `a_refused_download_starts_and_fails_at_once`, replace `ViewToHost::DownloadFailed { path } if` with `ViewToHost::DownloadFailed { path, .. } if`.

`crates/manifold-view-webview2/src/download.rs`:
- Replace `use manifold_view_protocol::{downloads, ViewToHost};` with `use manifold_view_protocol::{downloads, DownloadFailure, FailureKind, ViewToHost};`.
- Replace `Some(ViewToHost::DownloadFailed { path: path_text.clone() })` with `Some(ViewToHost::DownloadFailed { path: path_text.clone(), failure: DownloadFailure::new(FailureKind::Other, "") })`. Task 5 replaces this.

`crates/manifold-view-cef/src/download_handler.rs`:
- Replace `use manifold_view_protocol::ViewToHost;` with `use manifold_view_protocol::{DownloadFailure, FailureKind, ViewToHost};`.
- Replace `ViewToHost::DownloadFailed { path }` (in `on_download_updated`) with `ViewToHost::DownloadFailed { path, failure: DownloadFailure::new(FailureKind::Other, "") }`. Task 6 replaces this.

`crates/manifold-view-webkitgtk/src/main.rs`:
- Replace `use manifold_view_protocol::{transport, HostToView, MessageChannel, TabState, ViewToHost};` with `use manifold_view_protocol::{transport, DownloadFailure, FailureKind, HostToView, MessageChannel, TabState, ViewToHost};`.
- In `connect_failed`, replace `pending_for_failed.borrow_mut().push(ViewToHost::DownloadFailed { path });` with `pending_for_failed.borrow_mut().push(ViewToHost::DownloadFailed { path, failure: DownloadFailure::new(FailureKind::Other, "") });`. Task 7 replaces this.

`crates/manifold/src/process_manager.rs` `in_app_terms` (not a placeholder: this is final): replace

```rust
        ViewToHost::DownloadFailed { path } => ViewToHost::DownloadFailed { path: runtime.app_path(path) },
```

with

```rust
        ViewToHost::DownloadFailed { path, failure } => ViewToHost::DownloadFailed { path: runtime.app_path(path), failure },
```

`crates/manifold/src/event_loop/view_message.rs`:
- Replace `            ViewToHost::DownloadFailed { path } => {` with `            ViewToHost::DownloadFailed { path, .. } => {`. Task 4 uses the failure.
- In `mod tests`, after `use crate::store::model::{ActorId, NodeId};`, add `    use manifold_view_protocol::{DownloadFailure, FailureKind};`.
- In `a_failed_download_is_marked_and_no_longer_in_flight` and in `same_named_downloads_from_two_tabs_stay_apart`, replace each `ViewToHost::DownloadFailed { path: "f.zip".into() }` with `ViewToHost::DownloadFailed { path: "f.zip".into(), failure: DownloadFailure::new(FailureKind::Network, "") }`.

- [ ] **Step 5: Write the WSL failure test**

In `crates/manifold/src/process_manager/tests.rs`, after `a_wsl_hosts_download_paths_arrive_in_windows_terms`, add:

```rust
/// A WSL host's failed download arrives with its path in Windows terms and its failure as sent (`detail` holds no
/// path to map); a bare file name, a refusal for want of a folder, stays as it is.
#[test]
fn a_wsl_hosts_failed_download_keeps_its_failure() {
    use manifold_view_protocol::{DownloadFailure, FailureKind};
    let failure = DownloadFailure::new(FailureKind::FolderUnreachable, "");
    for (sent, arrives) in [("/mnt/d/dl/a.zip", r"D:\dl\a.zip"), ("a.zip", "a.zip")] {
        let msg = in_app_terms(HostRuntime::Wsl, ViewToHost::DownloadFailed { path: sent.into(), failure: failure.clone() });
        assert!(
            matches!(&msg, ViewToHost::DownloadFailed { path, failure: f } if path == arrives && *f == failure),
            "{msg:?}"
        );
    }
}
```

- [ ] **Step 6: Re-record the fingerprint**

Run: `cargo test -j 4 -p manifold-view-protocol wire_shape`
Expected: FAIL with "the HostToView/ViewToHost wire shape changed (fingerprint 0x…, recorded 0xe3d8fc72424dba86)".

Then make three edits:
- In `wire_shape_tests`, set `RECORDED_FOR_VERSION` to `8`.
- Set `RECORDED_FINGERPRINT` to the printed value, written in 4-digit groups like the old one (`0x1234_5678_9abc_def0`).
- In `lib.rs`, add the history line after the `/// 7:` line, and bump the version:

```rust
/// 8: `ViewToHost::DownloadFailed` carries a `DownloadFailure` (why: a `FailureKind` and the engine's or OS's words).
pub const PROTOCOL_VERSION: u32 = 8;
```

Run: `cargo test -j 4 -p manifold-view-protocol`
Expected: PASS, `test result: ok.` for both test binaries, `wire_shape_matches_the_recorded_fingerprint ... ok` among them.

- [ ] **Step 7: The rest passes**

```bash
cargo test -j 4 -p manifold --bin manifold download
cargo test -j 4 -p manifold-view-webview2
cargo clippy -j 4 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
```
Expected:
- `test result: ok.`, with `a_wsl_hosts_failed_download_keeps_its_failure ... ok` and the view_message download tests among them;
- WebView2's tests pass;
- clippy prints no warning.

CEF and WebKit are not compiled here (Global Constraints).

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold-view-protocol crates/manifold-view-webview2/src/download.rs crates/manifold-view-cef/src/download_handler.rs crates/manifold-view-webkitgtk/src/main.rs crates/manifold/src/process_manager.rs crates/manifold/src/process_manager/tests.rs crates/manifold/src/event_loop/view_message.rs
git commit -F - <<'EOF'
protocol 8: DownloadFailed carries a DownloadFailure

FailureKind (seven kinds, by what the user can do) and the engine's or
OS's words. Hosts send Other for now; the classification follows.

<attribution>
EOF
```
Expected: nine files; the hook passes.

---

### Task 3: Classified refusals (`Refusal`, `destination`, `refused`)

**Files:**
- Modify: `crates/manifold-view-protocol/src/downloads.rs:149-165`; tests `:209-239`.
- Modify: `crates/manifold-view-webview2/src/download.rs`:
  - module doc `:18-19`;
  - `save_path` `:96-114`;
  - the handler's `placed`, `:162-175`;
  - test `:336-340`.
- Modify: `crates/manifold-view-cef/src/download_handler.rs`: comment `:63-69` and the `Err` arm `:98-103`.
- Modify: `crates/manifold-view-webkitgtk/src/main.rs`: comment `:72-82` and `:113-128`.

**Interfaces:**
- Consumes: `DownloadFailure`, `FailureKind` (Task 2).
- Produces:

```rust
// manifold_view_protocol::downloads
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal { pub path: PathBuf, pub failure: DownloadFailure }
pub fn destination(dir: Option<&Path>, suggested: &str, taken: impl Fn(&Path) -> bool) -> Result<PathBuf, Refusal>;
pub fn refused(refusal: Refusal, url: Option<String>) -> [ViewToHost; 2];
fn unmade_folder(e: &std::io::Error) -> DownloadFailure; // private
// manifold-view-webview2 download.rs
fn save_path(held: &[Seen<'_>], uri: &str, broken_at_start: bool, proposed: &str, folder: Option<&Path>) -> Result<PathBuf, downloads::Refusal>;
```

- [ ] **Step 1: Write the failing tests**

In `downloads.rs` `mod tests`, replace the test `a_destination_is_made_in_the_folder_or_refused` and the test `a_refused_download_starts_and_fails_at_once` with:

```rust
    /// Review Focus 3 (download-folder plan) and 2 (this plan): a folder that can't be had refuses the download, and
    /// nothing is saved anywhere else; the refusal says why, in the OS's words, which name no path.
    #[test]
    fn a_destination_is_made_in_the_folder_or_refused() {
        let root = std::env::temp_dir().join(format!("manifold-destination-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("Zoë Q dl");
        let none = |_: &Path| false;
        assert_eq!(destination(Some(dir.as_path()), "../a.zip", none), Ok(dir.join("a.zip")));
        assert!(dir.is_dir(), "the folder is made at the first download");
        std::fs::write(dir.join("a.zip"), b"x").unwrap();
        assert_eq!(destination(Some(dir.as_path()), "a.zip", |p| p.exists()), Ok(dir.join("a (1).zip")));
        let unreachable = |path: PathBuf| -> Result<PathBuf, Refusal> {
            Err(Refusal { path, failure: DownloadFailure::new(FailureKind::FolderUnreachable, "") })
        };
        assert_eq!(destination(None, "b.zip", none), unreachable(PathBuf::from("b.zip")), "no folder");
        let rel = Path::new("manifold-relative-dl");
        assert_eq!(destination(Some(rel), "b.zip", none), unreachable(rel.join("b.zip")), "not a full path");
        assert!(!rel.exists(), "a relative folder is never made");
        let file = root.join("a file");
        std::fs::write(&file, b"x").unwrap();
        let Err(refusal) = destination(Some(file.as_path()), "c.zip", none) else {
            panic!("a file in the folder's place is refused")
        };
        assert_eq!(refusal.path, file.join("c.zip"));
        assert_eq!(refusal.failure.kind, FailureKind::FolderUnusable, "a file in its place");
        assert!(!refusal.failure.detail.is_empty(), "the OS's words");
        assert!(!refusal.failure.detail.contains("manifold-destination-test"), "no path: {}", refusal.failure.detail);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder that can't be made on a full disk is `DiskFull`; any other reason is an unusable folder.
    #[test]
    fn a_full_disk_is_told_apart_from_an_unusable_folder() {
        assert_eq!(unmade_folder(&std::io::Error::from(std::io::ErrorKind::StorageFull)).kind, FailureKind::DiskFull);
        #[cfg(windows)]
        assert_eq!(unmade_folder(&std::io::Error::from_raw_os_error(112)).kind, FailureKind::DiskFull, "ERROR_DISK_FULL");
        #[cfg(unix)]
        assert_eq!(unmade_folder(&std::io::Error::from_raw_os_error(28)).kind, FailureKind::DiskFull, "ENOSPC");
        let other = unmade_folder(&std::io::Error::from_raw_os_error(5));
        assert_eq!(other.kind, FailureKind::FolderUnusable);
        assert!(other.detail.contains("os error 5"), "{}", other.detail);
    }

    #[test]
    fn a_refused_download_starts_and_fails_at_once() {
        let failure = DownloadFailure::new(FailureKind::FolderUnusable, "Access is denied. (os error 5)");
        let refusal = Refusal { path: PathBuf::from(r"D:\dl\a.zip"), failure: failure.clone() };
        let [started, failed] = refused(refusal, Some("http://h/a".into()));
        assert!(
            matches!(&started, ViewToHost::DownloadStarted { path, total_bytes: None, url }
                if path == r"D:\dl\a.zip" && url.as_deref() == Some("http://h/a")),
            "{started:?}"
        );
        assert!(
            matches!(&failed, ViewToHost::DownloadFailed { path, failure: f } if path == r"D:\dl\a.zip" && *f == failure),
            "{failed:?}"
        );
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 4 -p manifold-view-protocol downloads`
Expected: FAIL to compile, with ``cannot find struct, variant or union type `Refusal` `` and ``cannot find function `unmade_folder` ``.

- [ ] **Step 3: The classification**

In `downloads.rs`, replace `destination` and `refused` and their doc comments (lines 149-165) with:

```rust
/// A download the host refuses (`destination`'s `Err`): the path it would have had, which the failed card shows, and
/// why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub path: PathBuf,
    pub failure: DownloadFailure,
}

/// Where a download the server suggested as `suggested` is saved in `dir`: `unique_path` of its `safe_name`, the
/// folder made first if missing. `Err` is a refusal, classified here once for every host: no folder, or one that is
/// not absolute, is `FolderUnreachable` (the path is then the bare name, or the name in that folder); a folder that
/// can't be made is `unmade_folder`'s kind. The host then refuses the download (`refused`) rather than saving it
/// anywhere else.
pub fn destination(dir: Option<&Path>, suggested: &str, taken: impl Fn(&Path) -> bool) -> Result<PathBuf, Refusal> {
    let name = safe_name(suggested);
    let Some(dir) = dir.filter(|dir| dir.is_absolute()) else {
        let path = dir.map_or_else(|| PathBuf::from(&name), |dir| dir.join(&name));
        return Err(Refusal { path, failure: DownloadFailure::new(FailureKind::FolderUnreachable, "") });
    };
    std::fs::create_dir_all(dir).map_err(|e| Refusal { path: dir.join(&name), failure: unmade_folder(&e) })?;
    Ok(unique_path(dir, &name, taken))
}

/// Why a download folder couldn't be made, from `create_dir_all`'s error: a full disk, or a folder that is unusable
/// (no access, a file in its place, an offline share). The OS's words are the detail; an `io::Error` names no path.
fn unmade_folder(e: &std::io::Error) -> DownloadFailure {
    let kind =
        if e.kind() == std::io::ErrorKind::StorageFull { FailureKind::DiskFull } else { FailureKind::FolderUnusable };
    DownloadFailure::new(kind, e.to_string())
}

/// The messages for a download the host refused (`destination`'s `Err`, or a path its engine won't take): it starts
/// and fails at once with the refusal's reason, so the app shows a failed card that says why.
pub fn refused(refusal: Refusal, url: Option<String>) -> [ViewToHost; 2] {
    let path = refusal.path.to_string_lossy().into_owned();
    [
        ViewToHost::DownloadStarted { path: path.clone(), total_bytes: None, url },
        ViewToHost::DownloadFailed { path, failure: refusal.failure },
    ]
}
```

Run: `cargo test -j 4 -p manifold-view-protocol downloads`
Expected: PASS, all three tests above `... ok`.

- [ ] **Step 4: The WebView2 host passes refusals through**

Before this step, P7's `Answer:` line must hold the owner's answer ("Decisions for review"). If it still reads "pending", stop and ask.

In `crates/manifold-view-webview2/src/download.rs`, make five edits.

1. Replace the module doc paragraph

```rust
//! A download the host can't place (no folder from the app, one that can't be made, a path Edge refuses) is
//! cancelled and reported started and failed (`downloads::refused`), so the app shows a failed card.
```

with the version for your Task 1 variant.

**A3-msg** (branch a when the message names no path; under P7's alternative, also branches b to e):

```rust
//! A download the host can't place is cancelled and reported started and failed with its reason
//! (`downloads::refused`), so the app shows a failed card that says why: no folder from the app, or one that can't be
//! made (`downloads::destination` classifies both), or a path Edge refuses (`SetResultFilePath` fails:
//! `FolderUnusable`, with the HRESULT's message as the detail). A download Edge ends `INTERRUPTED` says why by its
//! `InterruptReason` (`failure_kind`).
```

**A3-log** (branch a when the message names a path, and, under P7's recommendation, branches b to e, where no message was seen):

```rust
//! A download the host can't place is cancelled and reported started and failed with its reason
//! (`downloads::refused`), so the app shows a failed card that says why: no folder from the app, or one that can't be
//! made (`downloads::destination` classifies both), or a path Edge refuses (`SetResultFilePath` fails:
//! `FolderUnusable`; its HRESULT's message can name the path, so it goes to this host's log, not the card). A
//! download Edge ends `INTERRUPTED` says why by its `InterruptReason` (`failure_kind`).
```

2. In the doc of `save_path`, replace "`Err` is a download to refuse." with "`Err` is a download to refuse, and why."
3. In `save_path`'s signature, replace `) -> Result<PathBuf, PathBuf> {` with `) -> Result<PathBuf, downloads::Refusal> {`.
4. Replace

```rust
        // A path Edge won't take (`SetResultFilePath` fails) is refused like a folder that can't be made.
        let placed = decided.and_then(|path| match unsafe { args.SetResultFilePath(&HSTRING::from(path.as_path())) } {
            Ok(()) => Ok(path),
            Err(_) => Err(path),
        });
        let path = match placed {
            Ok(path) => path,
            Err(path) => {
                unsafe {
                    let _ = args.SetCancel(true);
                }
                pending_for_handler.borrow_mut().extend(downloads::refused(path.to_string_lossy().into_owned(), url));
                return Ok(());
            }
        };
```

with the version for your variant.

**A3-msg**:

```rust
        // A path Edge won't take (`SetResultFilePath` fails) is an unusable folder, in the HRESULT's words.
        let placed = decided.and_then(|path| match unsafe { args.SetResultFilePath(&HSTRING::from(path.as_path())) } {
            Ok(()) => Ok(path),
            Err(e) => {
                Err(downloads::Refusal { path, failure: DownloadFailure::new(FailureKind::FolderUnusable, e.message()) })
            }
        });
        let path = match placed {
            Ok(path) => path,
            Err(refusal) => {
                unsafe {
                    let _ = args.SetCancel(true);
                }
                pending_for_handler.borrow_mut().extend(downloads::refused(refusal, url));
                return Ok(());
            }
        };
```

**A3-log**: the same, but with this `Err(e)` arm:

```rust
            Err(e) => {
                // Its words could name the path, which a detail never holds: to this host's log, not the card (run
                // <Task 1 run id>).
                eprintln!("download: SetResultFilePath refused: {}", e.message());
                Err(downloads::Refusal { path, failure: DownloadFailure::new(FailureKind::FolderUnusable, "") })
            }
```

5. In `a_second_download_of_one_name_is_numbered_and_a_retry_is_not`, replace

```rust
        assert_eq!(
            save_path(&[], "http://h/a", false, r"C:\names\a.zip", None),
            Err(PathBuf::from("a.zip")),
            "no folder: refused"
        );
```

with

```rust
        assert_eq!(
            save_path(&[], "http://h/a", false, r"C:\names\a.zip", None).map_err(|r| r.path),
            Err(PathBuf::from("a.zip")),
            "no folder: refused"
        );
```

Run: `cargo test -j 4 -p manifold-view-webview2`
Expected: PASS.

- [ ] **Step 5: CEF and WebKit pass refusals through (compiled in Tasks 6 and 7)**

In `crates/manifold-view-cef/src/download_handler.rs`, replace the last three lines of the comment above `on_before_download`

```rust
        // placed is reported started and failed (`downloads::refused`) and handed back to CEF's default handling
        // (return 0), which cancels it for a windowless browser (Alloy style); `on_download_updated` reports nothing
        // of it, since its id was never assigned.
```

with

```rust
        // placed is reported started and failed with `destination`'s reason (`downloads::refused`) and handed back to
        // CEF's default handling (return 0), which cancels it for a windowless browser (Alloy style);
        // `on_download_updated` reports nothing of it, since its id was never assigned.
```

Then replace

```rust
                Err(path) => {
                    let url = Some(CefString::from(&item.url()).to_string()).filter(|u| !u.is_empty());
                    let messages = manifold_view_protocol::downloads::refused(path.to_string_lossy().into_owned(), url);
```

with

```rust
                Err(refusal) => {
                    let url = Some(CefString::from(&item.url()).to_string()).filter(|u| !u.is_empty());
                    let messages = manifold_view_protocol::downloads::refused(refusal, url);
```

In `crates/manifold-view-webkitgtk/src/main.rs`, replace these lines of the downloads comment

```rust
    // taken (WebKit doesn't overwrite, so a second download of one name would otherwise fail). With no folder, or one
    // that can't be made, the download is reported started and failed (`downloads::refused`) and cancelled. `reserved`
    // holds names handed out and not yet finished, so concurrent same-name downloads get distinct names before either
    // file exists. The destination is set as a properly escaped `file://` URI (`glib::filename_to_uri`), so a `#` or
    // space in a name works.
```

with

```rust
    // taken (WebKit doesn't overwrite, so a second download of one name would otherwise fail). With no folder, or one
    // that can't be made, the download is reported started and failed with `destination`'s reason
    // (`downloads::refused`) and cancelled; a destination GLib can't make a URI of is refused as `Other`, GLib's
    // words going to this host's log (they can hold a Linux path). `reserved` holds names handed out and not yet
    // finished, so concurrent same-name downloads get distinct names before either file exists. The destination is
    // set as a properly escaped `file://` URI (`glib::filename_to_uri`), so a `#` or space in a name works.
```

Then replace

```rust
                .and_then(|dest| match gtk::glib::filename_to_uri(&dest, None) {
                    Ok(uri) => Ok((dest, uri)),
                    Err(_) => Err(dest),
                });
                let (dest, uri) = match decided {
                    Ok(pair) => pair,
                    Err(dest) => {
                        // Refused: reported started and failed, then cancelled, since WebKit's own default would save
                        // it into ~/Downloads. `dest_for_events` stays empty, so its later signals report nothing.
                        pending_for_destination.borrow_mut().extend(manifold_view_protocol::downloads::refused(
                            dest.to_string_lossy().into_owned(),
                            url,
                        ));
```

with

```rust
                .and_then(|dest| match gtk::glib::filename_to_uri(&dest, None) {
                    Ok(uri) => Ok((dest, uri)),
                    Err(e) => {
                        eprintln!("download: no file URI for the destination: {}", e.message());
                        let failure = DownloadFailure::new(FailureKind::Other, "");
                        Err(manifold_view_protocol::downloads::Refusal { path: dest, failure })
                    }
                });
                let (dest, uri) = match decided {
                    Ok(pair) => pair,
                    Err(refusal) => {
                        // Refused: reported started and failed, then cancelled, since WebKit's own default would save
                        // it into ~/Downloads. `dest_for_events` stays empty, so its later signals report nothing.
                        pending_for_destination
                            .borrow_mut()
                            .extend(manifold_view_protocol::downloads::refused(refusal, url));
```

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy -j 4 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo fmt --all
git diff --stat
git add crates/manifold-view-protocol/src/downloads.rs crates/manifold-view-webview2/src/download.rs crates/manifold-view-cef/src/download_handler.rs crates/manifold-view-webkitgtk/src/main.rs
git commit -F - <<'EOF'
downloads: destination classifies a refusal; refused carries it

No folder is FolderUnreachable; a folder create_dir_all can't make is
DiskFull or FolderUnusable with the OS's words. WebView2's refused
SetResultFilePath is FolderUnusable (probe dl-fail-croot).

<attribution>
EOF
```
Expected: clippy prints no warning; four files are committed; the hook passes.

---

### Task 4: One `DownloadState` per download card

**Files:**
- Modify: `crates/manifold/src/node.rs:384-411` (`DownloadPayload`), plus the new `DownloadState`.
- Modify: `crates/manifold/src/fetched.rs:166-172`, `:196`, `:526-532`.
- Modify: `crates/manifold/src/session_sync/remote.rs:299-305`.
- Modify: `crates/manifold/src/session_sync/writes.rs:398`, `:624`.
- Modify: `crates/manifold/src/tabs/sleep.rs:121-125`.
- Modify: `crates/manifold/src/popups/dropdowns.rs:574-576`.
- Modify: `crates/manifold/src/debug_export.rs:176-183`.
- Modify: `crates/manifold/src/card/download_body.rs`.
- Modify: `crates/manifold/src/event_loop/view_message.rs`: handlers `:234-250`, tests.
- Modify: `crates/manifold/src/fetched/tests.rs:31`.
- Test fixtures: `bench.rs:107`, `card/tests.rs:627` and `:683`, `card_kind.rs:167`, `level.rs:335`, `links.rs:508`, `link_gestures/tests.rs:561`, `node.rs:626`, `session_sync/tests/links.rs:119`, and `session_sync/tests/writes.rs:223`, `:234`, `:363`, `:382`, `:400`, `:1116`; `trail_strip.rs:872`.

**Interfaces:**
- Consumes: `manifold_view_protocol::DownloadFailure` (Task 2).
- Produces (in `crate::node`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DownloadState { InFlight, Completed, Failed(manifold_view_protocol::DownloadFailure) }
// DownloadPayload { path, bytes_received, total_bytes, pub(crate) state: DownloadState }
impl DownloadPayload {
    pub(crate) fn started(path: String, total_bytes: Option<u64>) -> Self;
    pub(crate) fn finished(path: String, bytes: u64) -> Self;
    pub(crate) fn in_flight(&self) -> bool;
    pub(crate) fn is_completed(&self) -> bool;
    pub(crate) fn failure(&self) -> Option<&manifold_view_protocol::DownloadFailure>;
}
```
- Debug export: each download carries `"failure"`, the kind's `Debug` name (`"Network"`) or `null`.

- [ ] **Step 1: Write the failing tests**

In `view_message.rs` `mod tests`, replace `a_failed_download_is_marked_and_no_longer_in_flight` with:

```rust
    #[test]
    fn a_failed_download_is_marked_and_no_longer_in_flight() {
        let mut app = App::default();
        app.insert_note_at(N, None);
        app.insert_download_node(N, "f.zip".into(), Some(10), None);
        let in_flight = |app: &App| app.nodes.values().any(|n| n.as_download().is_some_and(|d| d.in_flight()));
        assert!(in_flight(&app));
        let failure = DownloadFailure::new(FailureKind::Network, "");
        app.handle_view_message(N, ViewToHost::DownloadFailed { path: "f.zip".into(), failure: failure.clone() });
        let download = app.nodes.values().find_map(|n| n.as_download()).unwrap();
        assert_eq!(download.failure(), Some(&failure));
        assert!(!download.is_completed());
        assert!(!in_flight(&app));
    }

    /// Review Focus 4: the same file started again after its card failed gets a card of its own; the failed one stays
    /// failed.
    #[test]
    fn a_redownload_after_a_failure_gets_its_own_card() {
        let mut app = App::default();
        app.insert_note_at(N, None);
        let started = || ViewToHost::DownloadStarted { path: "f.zip".into(), total_bytes: Some(10), url: None };
        app.handle_view_message(N, started());
        let failure = DownloadFailure::new(FailureKind::Server, "");
        app.handle_view_message(N, ViewToHost::DownloadFailed { path: "f.zip".into(), failure: failure.clone() });
        app.handle_view_message(N, started());
        app.handle_view_message(N, ViewToHost::DownloadProgress { path: "f.zip".into(), bytes_received: 4 });
        let downloads: Vec<_> = app.node_order.iter().filter_map(|id| app.nodes[id].as_download()).collect();
        assert_eq!(downloads.len(), 2);
        assert_eq!(downloads[0].failure(), Some(&failure), "the failed card stays failed");
        assert!(downloads[1].in_flight() && downloads[1].bytes_received == 4);
    }
```

In `a_redownload_updates_the_new_node_not_the_finished_one`, replace `assert!(downloads[0].completed && downloads[0].bytes_received == 0, "the old node is untouched");` with `assert!(downloads[0].is_completed() && downloads[0].bytes_received == 0, "the old node is untouched");`.

In `same_named_downloads_from_two_tabs_stay_apart`, replace

```rust
                .and_then(|(_, n)| n.as_download())
                .unwrap()
                .failed
```

with

```rust
                .and_then(|(_, n)| n.as_download())
                .unwrap()
                .failure()
                .is_some()
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 4 -p manifold --bin manifold view_message`
Expected: FAIL to compile, with ``no method named `in_flight` found`` (and the same for `failure` and `is_completed`).

- [ ] **Step 3: The state, its constructors and accessors**

In `node.rs`, replace the fields from `    /// Set once `DownloadCompleted` arrives for this node's own `path`.` through `    pub(crate) failed: bool,` with:

```rust
    /// Where the download stands: one state, so it is never both completed and failed, and a failure carries why
    /// (`DownloadFailure`).
    pub(crate) state: DownloadState,
```

After the `DownloadPayload` struct's closing `}`, add:

```rust

/// Where a download stands (`DownloadPayload::state`). Not stored: the store keeps only completed downloads, so a
/// failed one is gone after a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DownloadState {
    /// Started and still arriving: its card shows a progress bar while the server gave a size.
    InFlight,
    /// `DownloadCompleted` arrived for this node's own `path`, or the file was already on disk.
    Completed,
    /// `DownloadFailed` arrived: why, as the host said (`crate::download_failure` words it).
    Failed(manifold_view_protocol::DownloadFailure),
}

impl DownloadPayload {
    /// A download its engine just started: nothing received yet, `total_bytes` as the server said.
    pub(crate) fn started(path: String, total_bytes: Option<u64>) -> Self {
        Self { path, bytes_received: 0, total_bytes, state: DownloadState::InFlight }
    }

    /// A file already on disk, `bytes` long: a restored download, or a dropped file.
    pub(crate) fn finished(path: String, bytes: u64) -> Self {
        Self { path, bytes_received: bytes, total_bytes: Some(bytes), state: DownloadState::Completed }
    }

    pub(crate) fn in_flight(&self) -> bool {
        self.state == DownloadState::InFlight
    }

    pub(crate) fn is_completed(&self) -> bool {
        self.state == DownloadState::Completed
    }

    /// Why it failed, `None` unless it did.
    pub(crate) fn failure(&self) -> Option<&manifold_view_protocol::DownloadFailure> {
        match &self.state {
            DownloadState::Failed(failure) => Some(failure),
            _ => None,
        }
    }
}
```

- [ ] **Step 4: The handlers**

In `view_message.rs`, replace

```rust
            ViewToHost::DownloadFailed { path, .. } => {
                self.queue_toast(format!("Download failed: {path}"));
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.failed = true;
                    }
                }
            }
```

with

```rust
            ViewToHost::DownloadFailed { path, failure } => {
                self.queue_toast(format!("Download failed: {path}"));
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.state = crate::node::DownloadState::Failed(failure);
                    }
                }
            }
```

and `dl.completed = true;` with `dl.state = crate::node::DownloadState::Completed;`.

- [ ] **Step 5: The production literals and readers**

`fetched.rs`, in `insert_download_node`: replace

```rust
        let payload = Runtime::Download(DownloadPayload {
            path,
            bytes_received: 0,
            total_bytes,
            completed: false,
            failed: false,
        });
```

with `        let payload = Runtime::Download(DownloadPayload::started(path, total_bytes));`.

`fetched.rs`, `in_flight_download`: replace `.is_some_and(|d| d.path == path && !d.completed && !d.failed)` with `.is_some_and(|d| d.path == path && d.in_flight())`.

`fetched.rs`, the dropped file (`K::Other`): replace

```rust
                        let payload = DownloadPayload {
                            path: path_str,
                            bytes_received: bytes,
                            total_bytes: Some(bytes),
                            completed: true,
                            failed: false,
                        };
```

with `                        let payload = DownloadPayload::finished(path_str, bytes);`.

`session_sync/remote.rs`: replace

```rust
                let payload = DownloadPayload {
                    path,
                    bytes_received: bytes,
                    total_bytes: Some(bytes),
                    completed: true,
                    failed: false,
                };
```

with `                let payload = DownloadPayload::finished(path, bytes);`.

`session_sync/writes.rs`: in `is_running_download`, replace `.is_some_and(|dl| !dl.completed)` with `.is_some_and(|dl| !dl.is_completed())`. In `check_graph_matches_document`, replace `node.as_download().is_some_and(|dl| !dl.completed)` with `node.as_download().is_some_and(|dl| !dl.is_completed())`.

`tabs/sleep.rs`: replace the comment `// A download card's parent is the tab it came from; `completed` flips when the engine finishes it.` with `// A download card's parent is the tab it came from; it is in flight until the engine finishes or fails it.`, and `.filter(|(_, n)| n.as_download().is_some_and(|d| !d.completed && !d.failed))` with `.filter(|(_, n)| n.as_download().is_some_and(|d| d.in_flight()))`.

`popups/dropdowns.rs`: replace

```rust
                        if node.failed {
                            ui.colored_label(danger(ui.visuals()), "Failed");
                        } else if !node.completed {
```

with

```rust
                        if node.failure().is_some() {
                            ui.colored_label(danger(ui.visuals()), "Failed");
                        } else if !node.is_completed() {
```

`debug_export.rs` (P9): replace

```rust
                        "completed": d.completed,
                        "failed": d.failed,
```

with

```rust
                        "completed": d.is_completed(),
                        "failed": d.failure().is_some(),
                        // Why it failed: the kind's name, as the gate checks it.
                        "failure": d.failure().map(|f| format!("{:?}", f.kind)),
```

`card/download_body.rs`:
- After `use crate::store::model::NodeId;`, add `use crate::node::DownloadState;`.
- Replace

```rust
        let Some((download_path, completed, failed, total_bytes, bytes_received)) = self
            .nodes
            .get(&id)
            .and_then(|n| n.as_download())
            .map(|d| (d.path.clone(), d.completed, d.failed, d.total_bytes, d.bytes_received))
```

with

```rust
        let Some((download_path, state, total_bytes, bytes_received)) = self
            .nodes
            .get(&id)
            .and_then(|n| n.as_download())
            .map(|d| (d.path.clone(), d.state.clone(), d.total_bytes, d.bytes_received))
```

- Replace

```rust
                let size = match (completed, total_bytes) {
                    _ if failed => "Download failed".to_owned(),
                    (true, total) => crate::human_bytes(total.unwrap_or(bytes_received)),
                    (false, Some(total)) => {
                        format!("{} of {}", crate::human_bytes(bytes_received), crate::human_bytes(total))
                    }
                    (false, None) => crate::human_bytes(bytes_received),
                };
                let middle = card_body_rect(rect, true, inv_zoom).center();
                let color = if failed { danger(ui.visuals()) } else { ui.visuals().weak_text_color() };
```

with

```rust
                let size = match (&state, total_bytes) {
                    (DownloadState::Failed(_), _) => "Download failed".to_owned(),
                    (DownloadState::Completed, total) => crate::human_bytes(total.unwrap_or(bytes_received)),
                    (DownloadState::InFlight, Some(total)) => {
                        format!("{} of {}", crate::human_bytes(bytes_received), crate::human_bytes(total))
                    }
                    (DownloadState::InFlight, None) => crate::human_bytes(bytes_received),
                };
                let middle = card_body_rect(rect, true, inv_zoom).center();
                let failed = matches!(state, DownloadState::Failed(_));
                let color = if failed { danger(ui.visuals()) } else { ui.visuals().weak_text_color() };
```

- Replace `        if !completed && !failed {` with `        if state == DownloadState::InFlight {`.

`fetched/tests.rs`: replace `p.completed)` with `p.is_completed())` in `Runtime::Download(p) => format!("download {} done={}", p.bytes_received, p.completed),`.

- [ ] **Step 6: The 16 test fixtures (P10)**

Replace each literal with a constructor, keeping the binding name.

| Fixture | New code |
| --- | --- |
| `bench.rs:107` | `let download = DownloadPayload::finished("f.bin".into(), 0);` |
| `card/tests.rs:627` | `let download = Runtime::Download(DownloadPayload::started("f.bin".into(), None));` |
| `card/tests.rs:683` | `let download = Runtime::Download(DownloadPayload::finished("C:/dl/f.bin".into(), 0));` |
| `card_kind.rs:167` | `let download = Runtime::Download(DownloadPayload::started("f.bin".into(), None));` |
| `level.rs:335` | `Runtime::Download(DownloadPayload::finished(path.to_string(), 0))` |
| `link_gestures/tests.rs:561` | `let download = Runtime::Download(crate::node::DownloadPayload::started("f.bin".into(), None));` |
| `node.rs:626` (in `the_accessible_name_starts_with_the_registry_word`) | `Runtime::Download(DownloadPayload::started("dir/f.bin".into(), None)),` |
| `session_sync/tests/writes.rs:223` | `let payload = DownloadPayload::started("f.bin".into(), Some(2));` |
| `session_sync/tests/writes.rs:234` | `app.nodes.get_mut(&dl).unwrap().runtime = Runtime::Download(DownloadPayload::finished("f.bin".into(), 2));` |
| `session_sync/tests/writes.rs:363` and `:382` | `let payload = DownloadPayload::finished("f.bin".into(), 0);` |
| `session_sync/tests/writes.rs:400` | `let download = DownloadPayload::finished("f.bin".into(), 0);` |
| `session_sync/tests/writes.rs:1116` | `let dl = DownloadPayload::finished("f.bin".into(), 0);` |
| `trail_strip.rs:872` | `let download = crate::Runtime::Download(crate::node::DownloadPayload::finished(format!("{title}.bin"), 0));` |

`links.rs:508` and `session_sync/tests/links.rs:119` each have the closure `let download = |completed| { Runtime::Download(DownloadPayload { … completed, failed: false, }) };`. Replace both with:

```rust
        let download = |completed: bool| {
            Runtime::Download(if completed {
                DownloadPayload::finished("f.bin".into(), 2)
            } else {
                DownloadPayload::started("f.bin".into(), Some(2))
            })
        };
```

Then check that nothing still names the old fields:

```bash
grep -rnE "completed: (true|false|completed)|failed: false|\b(d|dl|p|node|download)\.(completed|failed)\b|downloads\[[0-9]\]\.(completed|failed)" crates/manifold/src
```
Expected: exactly two lines, neither about downloads: `crates/manifold/src/node_menu/tests.rs:57: failed: false,` (a `NodeMenuView`) and `crates/manifold/src/store/journal.rs:779: compaction_failed: false,`. Any other line is a download site left to convert.

- [ ] **Step 7: Run the tests**

```bash
cargo test -j 4 -p manifold --bin manifold
cargo clippy -j 4 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
```
Expected:
- `test result: ok.`, including `a_failed_download_is_marked_and_no_longer_in_flight` and `a_redownload_after_a_failure_gets_its_own_card`;
- clippy prints no warning.

If a test fails on a fixture's bytes or total (P10), stop and report it; don't change the test.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold/src
git commit -F - <<'EOF'
app: one DownloadState per download card

InFlight, Completed or Failed(DownloadFailure) replaces the completed and
failed bools; constructors replace the payload literals. The debug export
adds the failure's kind.

<attribution>
EOF
```
Expected: about 20 files, all under `crates/manifold/src`; the hook passes.

---

### Task 5: WebView2 maps its `InterruptReason`

**Files:**
- Modify: `crates/manifold-view-webview2/src/download.rs`:
  - imports `:37-42`;
  - `interrupted` `:87-91`;
  - `StateChanged` `:226-228`;
  - tests.
- Modify: `crates/gate/src/shots.rs`, the `download-failed` closure in `download_views`. Find it by `let failed = |s: &State| {`.
- Modify: `crates/gate/src/state.rs:347-348`, the `downloads_from` doc, which lists the export's `download` keys.
- Modify: `docs/runs.md` (the run rows).

**Interfaces:**
- Consumes: `DownloadFailure`, `FailureKind` (Task 2); the debug export's `"failure"` (Task 4).
- Produces:

```rust
fn interrupt_reason(operation: &ICoreWebView2DownloadOperation) -> COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON;
fn failure_kind(reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON) -> FailureKind;
```

- [ ] **Step 1: Write the failing unit test**

At the end of `download.rs`'s `mod tests` (before its closing `}`), add:

```rust
    /// Every reason webview2-com-sys 0.39.1 names (0-29) has its kind; one it doesn't name is `Other`.
    #[test]
    fn every_interrupt_reason_has_its_kind() {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        use FailureKind as K;
        let table = [
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_FAILED, K::FolderUnusable),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_ACCESS_DENIED, K::FolderUnusable),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NO_SPACE, K::DiskFull),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NAME_TOO_LONG, K::FolderUnusable),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_LARGE, K::FolderUnusable),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS, K::Blocked),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TRANSIENT_ERROR, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY, K::Blocked),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED, K::Blocked),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_SHORT, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_HASH_MISMATCH, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_FAILED, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_SERVER_DOWN, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_INVALID_REQUEST, K::Network),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FAILED, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_BAD_CONTENT, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CERTIFICATE_PROBLEM, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNEXPECTED_RESPONSE, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CROSS_ORIGIN_REDIRECT, K::Server),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_SHUTDOWN, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED, K::Other),
            (COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_DOWNLOAD_PROCESS_CRASHED, K::Other),
        ];
        for (reason, kind) in table {
            assert_eq!(failure_kind(reason), kind, "{reason:?}");
        }
        assert_eq!(failure_kind(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON(30)), K::Other, "a reason this build doesn't name");
    }
```

Run: `cargo test -j 4 -p manifold-view-webview2 every_interrupt_reason`
Expected: FAIL to compile, with ``cannot find function `failure_kind` ``.

- [ ] **Step 2: Make the gate's `download-failed` view check the kind (runtime RED)**

In `crates/gate/src/shots.rs`, in the `download-failed` closure, replace

```rust
                    matches!(s.downloads_from(&url)[..], [d] if d["failed"] == true && d["path"] == slow.as_str())
                        && s.toast().is_none()
```

with

```rust
                    matches!(s.downloads_from(&url)[..], [d]
                        if d["failed"] == true && d["path"] == slow.as_str() && d["failure"] == "Network")
                        && s.toast().is_none()
```

Also in the view's comment (the one starting `// The server dies mid-transfer.`), replace "the card shows it failed, and the shot waits for its toast to go." with "the card shows it failed for the network (`NETWORK_FAILED`, run 2026-10-27-dl-probe-hold), and the shot waits for its toast to go."

In `crates/gate/src/state.rs`, the doc of `downloads_from` lists the `download` object's keys. Replace

```rust
    /// The `download` object (url, path, bytes, total, completed, failed) of each download card fetched from `url`, in
    /// draw order.
```

with

```rust
    /// The `download` object (url, path, bytes, total, completed, failed, failure) of each download card fetched from
    /// `url`, in draw order.
```

Run the Pre-run check (run name `dl-reason-red`). Then:

```bash
cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate 2>&1 | tail -n 2
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name dl-reason-red -Detach -Command 'set "VIEWS=download-progress download-failed"& set "OUT=%RUN_DIR%\shots"& target\debug\gate.exe chrome-shots "%RUN_DIR%\work"'
```

The build must finish without the Step 1 test: `cargo build` doesn't compile tests. Wait as in "Waiting for a run" (deadline 900 s). Expected:
- `"exit"` is 1, and `"commit"` ends `-dirty`.
- `grep -n "^FAIL: downloads" "$RUN/out.log"` prints exactly one line: `FAIL: downloads: the one card downloading http://127.0.0.1:<port>/slow to …\slow.zip did not show the download failed (toast None): [...]`. The bracket holds one card in `serde_json` Debug form, with `"failed": Bool(true)` and `"failure": String("Other")`. The line is printed by `shots.rs` `step` and is not the last line of the log.

Any other error, or a different `failure`, is a stop-and-report with the run id. Then remove the work folder (`rm -rf -- "$RUN"/work/gate-chrome-shots-*`) and check the Downloads diff.

- [ ] **Step 3: The mapping**

In `download.rs`, add `COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON` to the `webview2_com::Microsoft::Web::WebView2::Win32::{…}` import. Then replace

```rust
/// Whether `operation`'s transfer broke: a retry starts with the broken one's reason (see the module doc).
fn interrupted(operation: &ICoreWebView2DownloadOperation) -> bool {
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
    unsafe { operation.InterruptReason(&mut reason) }.is_ok() && reason != COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE
}
```

with the code below. The doc's last sentence differs by Task 1's branch; use exactly one of the listed sentences, with the run id and values filled in.

```rust
/// Why `operation`'s transfer broke, `NONE` while it hasn't (or when Edge can't say).
fn interrupt_reason(operation: &ICoreWebView2DownloadOperation) -> COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON {
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
    match unsafe { operation.InterruptReason(&mut reason) } {
        Ok(()) => reason,
        Err(_) => COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE,
    }
}

/// Whether `operation`'s transfer broke: a retry starts with the broken one's reason (see the module doc).
fn interrupted(operation: &ICoreWebView2DownloadOperation) -> bool {
    interrupt_reason(operation) != COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE
}

/// What the user can do about a download Edge ended `INTERRUPTED` for `reason` (webview2-com-sys 0.39.1 names 0-29;
/// any other is `Other`). The `FILE_*` codes for a file Windows won't write are an unusable folder;
/// `FILE_TRANSIENT_ERROR`, `FILE_TOO_SHORT`, `FILE_HASH_MISMATCH` (a resumed file that doesn't match), the `USER_*`
/// codes and a crash say nothing the user can act on. A server that dies mid-transfer ends the retry
/// `NETWORK_FAILED` (run 2026-10-27-dl-probe-hold).
/// <ONE SENTENCE FROM THE LIST BELOW>
fn failure_kind(reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON) -> FailureKind {
    match reason.0 {
        // FILE_FAILED, FILE_ACCESS_DENIED, FILE_NAME_TOO_LONG, FILE_TOO_LARGE
        1 | 2 | 4 | 5 => FailureKind::FolderUnusable,
        // FILE_NO_SPACE
        3 => FailureKind::DiskFull,
        // FILE_MALICIOUS, FILE_BLOCKED_BY_POLICY, FILE_SECURITY_CHECK_FAILED
        6 | 8 | 9 => FailureKind::Blocked,
        // NETWORK_FAILED, _TIMEOUT, _DISCONNECTED, _SERVER_DOWN, _INVALID_REQUEST
        12..=16 => FailureKind::Network,
        // SERVER_FAILED through SERVER_CROSS_ORIGIN_REDIRECT
        17..=25 => FailureKind::Server,
        _ => FailureKind::Other,
    }
}
```

The `<ONE SENTENCE FROM THE LIST BELOW>` line by branch:
- (a): `/// A download into `C:\`, where a normal user can't create files, never gets here: Edge refuses its path up front (`SetResultFilePath` <hresult>, run <id>).`
- (b): `/// A download into `C:\`, where a normal user can't create files, ends `<NAME>` (<R>, run <id>).`
- (c): `/// No `FILE_*` code has been seen: Windows let a download into `C:\` through (run <id>; PARKED).`
- (d): `/// No `FILE_*` code has been seen: a download into `C:\` never ended within 90 s (run <id>; PARKED).`
- (e): `/// No `FILE_*` code has been seen: Edge started no download into `C:\` (run <id>; PARKED).`

In `StateChanged`, replace

```rust
                        Some(ViewToHost::DownloadFailed {
                            path: path_text.clone(),
                            failure: DownloadFailure::new(FailureKind::Other, ""),
                        })
```

(Task 2's placeholder, as `cargo fmt` laid it out; it may be on one line) with

```rust
                        // Edge's code says why; it has no words of its own to add.
                        let failure = DownloadFailure::new(failure_kind(interrupt_reason(sender)), "");
                        Some(ViewToHost::DownloadFailed { path: path_text.clone(), failure })
```

Here `sender` is the `&ICoreWebView2DownloadOperation` bound by `if let Some(sender) = &sender`.

Run: `cargo test -j 4 -p manifold-view-webview2`
Expected: PASS, including `every_interrupt_reason_has_its_kind ... ok`.

- [ ] **Step 4: Commit, then the runtime GREEN**

```bash
cargo test -j 4 -p gate
cargo clippy -j 4 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo fmt --all
git diff --stat
git add crates/manifold-view-webview2/src/download.rs crates/gate/src/shots.rs crates/gate/src/state.rs
git commit -F - <<'EOF'
webview2: a failed download says why, by its InterruptReason

failure_kind maps the 30 reasons; the gate's download-failed view checks
the network failure it causes.

<attribution>
EOF
```
Expected: the gate's tests pass; clippy prints no warning; three files are committed.

Run the Pre-run check (run name `dl-reason-green`). Then:

```bash
cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate 2>&1 | tail -n 2
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name dl-reason-green -Detach -Command 'set "VIEWS=download-progress download-failed download-real download-resume"& set "OUT=%RUN_DIR%\shots"& target\debug\gate.exe chrome-shots "%RUN_DIR%\work"'
```

Wait (deadline 900 s). Expected:
- `"exit"` is 0.
- `"commit"` is `git rev-parse --short HEAD`. It may end `-dirty` only for this plan file's ticked boxes; run `git status --short` to name the dirty files. Any change under `crates/` means the run did not test the commit: stop.
- `$RUN/shots/` holds `download-progress.png`, `download-failed.png`, `download-real-tab.png`, `download-real.png` and `download-resume.png`.
- The Downloads diff prints nothing.

If `"exit"` is 1, stop and report the error line with the run id. Don't loosen the check.

- [ ] **Step 5: Commit the run rows**

Set each row's headline in `docs/runs.md` if the wrapper's last-line headline isn't the verdict:
- RED: `download-failed: failure Other (host placeholder), check wants Network`.
- GREEN: `download views pass; download-failed card failure Network`.

```bash
git diff --stat
git add docs/runs.md
git commit -F - <<'EOF'
runs: dl-reason RED (placeholder Other) and GREEN (Network) rows

<attribution>
EOF
```

---

### Task 6: CEF maps `interrupt_reason()` and `is_canceled()`

**Files:**
- Modify: `crates/manifold-view-cef/src/download_handler.rs`:
  - `on_download_updated` `:132-136`;
  - a new module-level `failure_kind`;
  - a new `mod tests` at the end of the file.

**Interfaces:**
- Consumes: `DownloadFailure`, `FailureKind` (Task 2); the CEF guard (Global Constraints).
- Produces: `fn failure_kind(canceled: bool, reason: DownloadInterruptReason) -> FailureKind`.

- [ ] **Step 1: Write the failing test**

At the end of `download_handler.rs`, add:

```rust

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reason CEF 152 names has its kind, as the WebView2 host maps the same Chromium codes; an engine cancel is
    /// `Other` whatever its reason.
    #[test]
    fn every_interrupt_reason_has_its_kind() {
        use DownloadInterruptReason as R;
        use FailureKind as K;
        let table = [
            (R::NONE, K::Other),
            (R::FILE_FAILED, K::FolderUnusable),
            (R::FILE_ACCESS_DENIED, K::FolderUnusable),
            (R::FILE_NO_SPACE, K::DiskFull),
            (R::FILE_NAME_TOO_LONG, K::FolderUnusable),
            (R::FILE_TOO_LARGE, K::FolderUnusable),
            (R::FILE_VIRUS_INFECTED, K::Blocked),
            (R::FILE_TRANSIENT_ERROR, K::Other),
            (R::FILE_BLOCKED, K::Blocked),
            (R::FILE_SECURITY_CHECK_FAILED, K::Blocked),
            (R::FILE_TOO_SHORT, K::Other),
            (R::FILE_HASH_MISMATCH, K::Other),
            (R::FILE_SAME_AS_SOURCE, K::Other),
            (R::NETWORK_FAILED, K::Network),
            (R::NETWORK_TIMEOUT, K::Network),
            (R::NETWORK_DISCONNECTED, K::Network),
            (R::NETWORK_SERVER_DOWN, K::Network),
            (R::NETWORK_INVALID_REQUEST, K::Network),
            (R::SERVER_FAILED, K::Server),
            (R::SERVER_NO_RANGE, K::Server),
            (R::SERVER_BAD_CONTENT, K::Server),
            (R::SERVER_UNAUTHORIZED, K::Server),
            (R::SERVER_CERT_PROBLEM, K::Server),
            (R::SERVER_FORBIDDEN, K::Server),
            (R::SERVER_UNREACHABLE, K::Server),
            (R::SERVER_CONTENT_LENGTH_MISMATCH, K::Server),
            (R::SERVER_CROSS_ORIGIN_REDIRECT, K::Server),
            (R::USER_CANCELED, K::Other),
            (R::USER_SHUTDOWN, K::Other),
            (R::CRASH, K::Other),
        ];
        for (reason, kind) in table {
            assert_eq!(failure_kind(false, reason), kind, "{reason:?}");
        }
        assert_eq!(failure_kind(true, R::NETWORK_FAILED), K::Other, "an engine cancel");
    }
}
```

Create the CEF guard script (Global Constraints), then run it:

```bash
powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd check -Logs "<scratchpad>"
```
Expected:
- exit 101 (or 1), with `cef-check.err.log` showing ``cannot find function `failure_kind` `` in `download_handler.rs`;
- no other error: the earlier tasks' CEF edits compile.

If an error names another line, fix that line, since it is a Task 2 or 3 edit. Exit 2 means the check could not run on the warm build: stop and report it.

- [ ] **Step 2: The mapping**

Before `impl DownloadHandlerBuilder {` (after the `wrap_download_handler!` block's closing `}`), add:

```rust
/// What the user can do about a download CEF ended without completing: `canceled` (`is_canceled`) is the engine's own
/// cancel, nothing the user asked for (the app has no cancel); otherwise `reason` (`interrupt_reason`), Chromium's
/// codes, mapped as the WebView2 host maps Edge's (`SERVER_UNREACHABLE` is Chromium's "unexpected server response").
fn failure_kind(canceled: bool, reason: DownloadInterruptReason) -> FailureKind {
    if canceled {
        return FailureKind::Other;
    }
    match reason {
        DownloadInterruptReason::FILE_FAILED
        | DownloadInterruptReason::FILE_ACCESS_DENIED
        | DownloadInterruptReason::FILE_NAME_TOO_LONG
        | DownloadInterruptReason::FILE_TOO_LARGE => FailureKind::FolderUnusable,
        DownloadInterruptReason::FILE_NO_SPACE => FailureKind::DiskFull,
        DownloadInterruptReason::FILE_VIRUS_INFECTED
        | DownloadInterruptReason::FILE_BLOCKED
        | DownloadInterruptReason::FILE_SECURITY_CHECK_FAILED => FailureKind::Blocked,
        DownloadInterruptReason::NETWORK_FAILED
        | DownloadInterruptReason::NETWORK_TIMEOUT
        | DownloadInterruptReason::NETWORK_DISCONNECTED
        | DownloadInterruptReason::NETWORK_SERVER_DOWN
        | DownloadInterruptReason::NETWORK_INVALID_REQUEST => FailureKind::Network,
        DownloadInterruptReason::SERVER_FAILED
        | DownloadInterruptReason::SERVER_NO_RANGE
        | DownloadInterruptReason::SERVER_BAD_CONTENT
        | DownloadInterruptReason::SERVER_UNAUTHORIZED
        | DownloadInterruptReason::SERVER_CERT_PROBLEM
        | DownloadInterruptReason::SERVER_FORBIDDEN
        | DownloadInterruptReason::SERVER_UNREACHABLE
        | DownloadInterruptReason::SERVER_CONTENT_LENGTH_MISMATCH
        | DownloadInterruptReason::SERVER_CROSS_ORIGIN_REDIRECT => FailureKind::Server,
        _ => FailureKind::Other,
    }
}
```

The consts can be used as patterns because `DownloadInterruptReason` derives `PartialEq, Eq` (cef 152.1.0 bindings). If the compiler still rejects them ("constant of non-structural type"), stop and report it rather than rewriting the match as an if-chain.

In `on_download_updated`, replace

```rust
                    let event = if complete {
                        ViewToHost::DownloadCompleted { path }
                    } else {
                        ViewToHost::DownloadFailed { path, failure: DownloadFailure::new(FailureKind::Other, "") }
                    };
```

(Task 2's placeholder, as fmt laid it out) with

```rust
                    let event = if complete {
                        ViewToHost::DownloadCompleted { path }
                    } else {
                        // CEF's code says why; it has no words of its own to add.
                        let kind = failure_kind(item.is_canceled() != 0, item.interrupt_reason());
                        ViewToHost::DownloadFailed { path, failure: DownloadFailure::new(kind, "") }
                    };
```

- [ ] **Step 3: Check and test through the guard**

```bash
powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd check -Logs "<scratchpad>"
powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd test -Logs "<scratchpad>"
```
Expected:
- `check` exits 0 with no warning in `cef-check.err.log` (`grep -c "^warning" "<scratchpad>/cef-check.err.log"` prints 0).
- `test` exits 0, and `cef-tests.out.log` shows `download_handler::tests::every_interrupt_reason_has_its_kind ... ok` and `test result: ok.`.
- If `test` exits 3 ("compiled, not run"), record that in the commit message and the Task 10 report; the check still counts as compiled.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold-view-cef/src/download_handler.rs
git commit -F - <<'EOF'
cef: a failed download says why, by interrupt_reason and is_canceled

Checked and tested on the warm cef-dll-sys build (cef-check.ps1).

<attribution>
EOF
```

---

### Task 7: WebKit maps its `DownloadError`

**Files:**
- Modify: `crates/manifold-view-webkitgtk/src/main.rs`:
  - `connect_failed` `:149-158`;
  - a new module-level `failure_kind`;
  - a new `mod tests` at the end of the file (the crate's first).

**Interfaces:**
- Consumes: `DownloadFailure`, `FailureKind` (Task 2).
- Produces: `fn failure_kind(error: &gtk::glib::Error) -> FailureKind`.

- [ ] **Step 1: Write the failing test**

At the end of `main.rs`, add:

```rust

#[cfg(test)]
mod tests {
    use super::*;

    /// WebKit's three download error codes, as real `glib::Error`s so the `kind` lookup runs too; a cancel and an error
    /// of another domain say nothing the user can act on.
    #[test]
    fn webkits_download_errors_map_to_failure_kinds() {
        use gtk::glib::{Error, FileError};
        use webkit2gtk::DownloadError as E;
        assert_eq!(failure_kind(&Error::new(E::Network, "x")), FailureKind::Network);
        assert_eq!(failure_kind(&Error::new(E::Destination, "x")), FailureKind::FolderUnusable);
        assert_eq!(failure_kind(&Error::new(E::CancelledByUser, "x")), FailureKind::Other);
        assert_eq!(failure_kind(&Error::new(FileError::Nospc, "x")), FailureKind::Other, "another error domain");
    }
}
```

Run the WebKit check (Global Constraints).
Expected: the build succeeds; that also compiles Tasks 2 and 3's WebKit edits, so any error there must be fixed first. The test build fails with ``cannot find function `failure_kind` ``.

- [ ] **Step 2: The mapping**

Before `fn main()` (or right after the `use` block), add:

```rust
/// What the user can do about a download WebKit reports `failed` with `error`: its three `DownloadError` codes, a
/// cancel and an error of any other domain being `Other`.
fn failure_kind(error: &gtk::glib::Error) -> FailureKind {
    match error.kind::<webkit2gtk::DownloadError>() {
        Some(webkit2gtk::DownloadError::Network) => FailureKind::Network,
        Some(webkit2gtk::DownloadError::Destination) => FailureKind::FolderUnusable,
        _ => FailureKind::Other,
    }
}
```

Replace

```rust
            download.connect_failed(move |_download, _error| {
                failed_for_handler.set(true);
                if let Some(path) = dest_for_failed.borrow().clone() {
                    pending_for_failed
                        .borrow_mut()
                        .push(ViewToHost::DownloadFailed { path, failure: DownloadFailure::new(FailureKind::Other, "") });
                }
            });
```

(Task 2's placeholder, as fmt laid it out) with

```rust
            download.connect_failed(move |_download, error| {
                failed_for_handler.set(true);
                // GLib's words can hold a Linux path, and only `path` is mapped into the app's terms: they go to this
                // host's log, and the card gets the kind alone.
                eprintln!("download failed: {}", error.message());
                if let Some(path) = dest_for_failed.borrow().clone() {
                    let failure = DownloadFailure::new(failure_kind(error), "");
                    pending_for_failed.borrow_mut().push(ViewToHost::DownloadFailed { path, failure });
                }
            });
```

- [ ] **Step 3: Build and test in WSL**

Run the WebKit check.
Expected:
- `Finished` for the build, with no warning;
- `test tests::webkits_download_errors_map_to_failure_kinds ... ok` and `test result: ok. 1 passed`.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold-view-webkitgtk/src/main.rs
git commit -F - <<'EOF'
webkit: a failed download says why, by its DownloadError

GLib's message goes to the host's log: it can hold a Linux path. Built
and tested in WSL (target-linux).

<attribution>
EOF
```

---

### Task 8: The words, on every surface

**Files:**
- Create: `crates/manifold/src/download_failure.rs`.
- Modify: `crates/manifold/src/main.rs:22-23`: add `mod download_failure;` between `mod doc;` and `mod duplicate;`.
- Modify: `crates/manifold/src/card/download_body.rs` (tooltip, face).
- Modify: `crates/manifold/src/popups/dropdowns.rs` (popup row).
- Modify: `crates/manifold/src/event_loop/view_message.rs` (toast and its test).
- Modify: `crates/manifold/src/card.rs`: `card_label` after `chain_tooltip` (`:176`), and `interact_card` `:1223-1240`.
- Test: `crates/manifold/src/card/tests.rs`.
- Modify: `crates/gate/src/shots.rs`, the `download-failed` closure and its error message.
- Modify: `docs/runs.md`.

**Interfaces:**
- Consumes: `DownloadFailure`, `FailureKind`, `FailureKind::ALL` (Task 2); `DownloadPayload::failure`, `DownloadState` (Task 4); `App::download_folder() -> Option<PathBuf>` (`settings_page.rs:305`); `App::accessible_name(NodeId) -> Option<String>` (`node.rs:483`); `card::DOWNLOAD_NODE_SIZE`, `card::GRAPH_THUMB_MARGIN`, `card::tab_has_header` and `theme::apply_fonts` (the face-width test).
- Produces:

```rust
// crate::download_failure
pub(crate) fn card_face(kind: FailureKind) -> &'static str;
pub(crate) fn popup_row(kind: FailureKind) -> String;
pub(crate) fn toast(path: &str, kind: FailureKind) -> String;
impl App { pub(crate) fn download_tooltip(&self, id: NodeId) -> Option<String>; }
// crate::card
pub(crate) fn card_label(name: String, tooltip: Option<&str>) -> String;
```

Before Step 1, P5's and P12's `Answer:` lines must hold the owner's answers ("Decisions for review"). If either still reads "pending", stop and ask. The steps below follow both recommendations; P5 and P12 say what changes under each alternative.

- [ ] **Step 1: Write the failing tests**

Create `crates/manifold/src/download_failure.rs` with only the tests for now, so they fail:

```rust
//! What the app says about a failed download (`manifold_view_protocol::DownloadFailure`).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{field, FieldValue};
    use manifold_view_protocol::ViewToHost;

    /// The face draws unclipped at a fixed 13 points (`download_body.rs`, `push_centered_text`) on every download card
    /// tall enough for a header (`card::tab_has_header`): `DOWNLOAD_NODE_SIZE` is 160 by 110 and the header needs 80
    /// points of height, so down to about 116 points of width. Laid out in the app's own font, each short reason fits
    /// inside the card's margins there (a character count would not bound its width); each kind reads differently,
    /// and only `Other` falls back to "Download failed".
    #[test]
    fn every_face_fits_the_smallest_card_and_only_other_is_plain() {
        let faces: std::collections::HashSet<&str> = FailureKind::ALL.iter().map(|&k| card_face(k)).collect();
        assert_eq!(faces.len(), FailureKind::ALL.len(), "two kinds read the same");
        for kind in FailureKind::ALL {
            assert_eq!(card_face(kind) == "Download failed", kind == FailureKind::Other, "{kind:?}");
        }
        // The narrowest card that draws a face is between 116 and 117 points wide on screen.
        let card = egui::Rect::from_min_size(egui::Pos2::ZERO, crate::card::DOWNLOAD_NODE_SIZE);
        let drawn_at = |width: f32| crate::card::tab_has_header(card, card.width() / width);
        assert!(drawn_at(117.0) && !drawn_at(116.0), "the narrowest card with a face moved: update `room`");
        let room = 116.0 - 2.0 * crate::card::GRAPH_THUMB_MARGIN;
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut widths: Vec<(FailureKind, f32)> = Vec::new();
        // `set_fonts` takes effect at the start of the next pass, so this one lays out in the app's font.
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::FontId::proportional(13.0);
            widths = FailureKind::ALL
                .iter()
                .map(|&kind| {
                    let face = card_face(kind).to_owned();
                    (kind, ui.ctx().fonts_mut(|f| f.layout_no_wrap(face, font.clone(), egui::Color32::WHITE)).size().x)
                })
                .collect();
        });
        // The font atlas upload is never applied here; dropping it unapplied is a debug assertion.
        output.textures_delta.clear();
        assert_eq!(widths.len(), FailureKind::ALL.len());
        for (kind, width) in widths {
            assert!(width <= room, "{kind:?}: {:?} is {width} points wide; the card has {room}", card_face(kind));
        }
    }

    #[test]
    fn the_popup_and_the_toast_name_the_short_reason() {
        assert_eq!(popup_row(FailureKind::DiskFull), "Failed: Disk full");
        assert_eq!(popup_row(FailureKind::Other), "Failed");
        assert_eq!(toast(r"D:\dl\a.zip", FailureKind::Network), r"Download failed: D:\dl\a.zip (Network error)");
        assert_eq!(toast("a.zip", FailureKind::Other), "Download failed: a.zip");
    }

    /// The tooltip says what happened, where, and the engine's or OS's words, a line each; a blank detail or no folder
    /// adds no line.
    #[test]
    fn the_tooltip_says_why_where_and_the_engines_words() {
        let folder = std::path::Path::new("dl");
        let unusable = DownloadFailure::new(FailureKind::FolderUnusable, " Access is denied. (os error 5)\n");
        assert_eq!(
            tooltip(&unusable, Some(folder)),
            "The download folder can't be made or written to, or can't hold this file\nFolder: dl\n\
             Access is denied. (os error 5)"
        );
        let network = DownloadFailure::new(FailureKind::Network, " ");
        assert_eq!(tooltip(&network, None), "The connection failed or timed out");
    }

    /// Review Focus 1, 3 and 5. A refusal for want of a folder (a bare file name) names the folder the app means now;
    /// a failure at a full path names that path's folder, even after the setting moves (P12); a renamed card still
    /// says why, after the name a screen reader hears first.
    #[test]
    fn a_failed_cards_tooltip_names_its_folder_and_survives_a_rename() {
        let mut app = App::default();
        let tab = app.alloc_node_id();
        app.insert_note_at(tab, None);
        let root = crate::store::testutil::temp_dir("download-tooltip");
        let (a, b) = (root.join("a"), root.join("b"));
        app.set_download_directory(a.to_str().unwrap());
        let full = a.join("full.zip").to_string_lossy().into_owned();
        for path in ["bare.zip", full.as_str(), "running.zip"] {
            app.insert_download_node(tab, path.to_owned(), None, None);
        }
        let unreachable = DownloadFailure::new(FailureKind::FolderUnreachable, "");
        app.handle_view_message(tab, ViewToHost::DownloadFailed { path: "bare.zip".into(), failure: unreachable });
        let unusable = DownloadFailure::new(FailureKind::FolderUnusable, "Access is denied. (os error 5)");
        app.handle_view_message(tab, ViewToHost::DownloadFailed { path: full.clone(), failure: unusable });
        let id_of = |app: &App, path: &str| {
            let found = app.node_order.iter().copied().find(|id| app.nodes[id].as_download().is_some_and(|d| d.path == path));
            found.unwrap()
        };
        let (bare, at_full, running) = (id_of(&app, "bare.zip"), id_of(&app, &full), id_of(&app, "running.zip"));
        let names_folder = |tip: Option<String>, dir: &std::path::Path| {
            tip.is_some_and(|t| t.lines().any(|l| l == format!("Folder: {}", dir.display())))
        };
        assert!(names_folder(app.download_tooltip(bare), &a), "a bare name: the app's folder");
        assert!(names_folder(app.download_tooltip(at_full), &a));
        app.set_download_directory(b.to_str().unwrap());
        assert!(names_folder(app.download_tooltip(bare), &b), "a bare name follows the setting");
        assert!(names_folder(app.download_tooltip(at_full), &a), "a full path keeps its own folder");
        assert_eq!(app.download_tooltip(running), None, "in flight: no tooltip");
        assert_eq!(app.download_tooltip(tab), None, "not a download");
        app.doc.mirror.set(at_full, field::NAME, FieldValue::Str("Mine".into()));
        let label =
            crate::card::card_label(app.accessible_name(at_full).unwrap(), app.download_tooltip(at_full).as_deref());
        assert!(label.starts_with("Download: Mine. The download folder can't be made or written to"), "{label}");
        assert!(label.ends_with("Access is denied. (os error 5)"), "{label}");
    }
}
```

In `main.rs`, add `mod download_failure;` between `mod doc;` and `mod duplicate;`.

In `card/tests.rs`, add at the end:

```rust
/// Screen readers can't hover: a card's tooltip lines follow its name, a sentence each; no tooltip, no change.
#[test]
fn a_cards_label_carries_its_tooltip() {
    assert_eq!(card_label("Note: Hi".into(), None), "Note: Hi");
    assert_eq!(card_label("Note: Hi.".into(), Some(" \n")), "Note: Hi.", "a blank tooltip adds nothing");
    assert_eq!(
        card_label("Download: a.zip".into(), Some("The disk the download folder is on is full\nFolder: D:\\dl\n")),
        "Download: a.zip. The disk the download folder is on is full. Folder: D:\\dl"
    );
    assert_eq!(
        card_label("Tab: x".into(), Some("Not responding. Right-click for Restart View or End View.")),
        "Tab: x. Not responding. Right-click for Restart View or End View"
    );
}
```

In `view_message.rs` `mod tests`, add:

```rust
    /// The toast keeps the path and adds the short reason (`download_failure::toast`).
    #[test]
    fn a_failed_downloads_toast_says_why() {
        let mut app = App::default();
        app.insert_note_at(N, None);
        app.insert_download_node(N, "f.zip".into(), Some(10), None);
        let failure = DownloadFailure::new(FailureKind::Network, "");
        app.handle_view_message(N, ViewToHost::DownloadFailed { path: "f.zip".into(), failure });
        assert_eq!(app.toasts.save_toast_queue.back().map(String::as_str), Some("Download failed: f.zip (Network error)"));
    }
```

Run: `cargo test -j 4 -p manifold --bin manifold download_failure`
Expected: FAIL to compile. The errors name only what Step 2 and Step 3 add: ``cannot find function `card_face` `` (and `popup_row`, `toast`, `tooltip`, `download_tooltip`, `card_label`), plus `FailureKind`, `DownloadFailure` and `App`, which the module doesn't import yet. An error naming anything else (`card::DOWNLOAD_NODE_SIZE`, `theme::apply_fonts`, `run_ui`, `fonts_mut`) is a mistake in this plan: stop and report it.

- [ ] **Step 2: The wording module**

Above the `#[cfg(test)]` line of `download_failure.rs`, replace the one-line module doc with:

```rust
//! What the app says about a failed download (`manifold_view_protocol::DownloadFailure`): the card's face, the
//! Downloads popup row, the toast and the tooltip, worded here and nowhere else. The face draws unclipped at a fixed
//! 13 points on download cards down to about 116 points wide (`card::DOWNLOAD_NODE_SIZE` is 160 by 110, and
//! `card::tab_has_header` draws it from 80 points tall), so each short reason must fit that width in the app's font
//! (`tests::every_face_fits_the_smallest_card_and_only_other_is_plain` measures it). The engine's or OS's own words
//! (`detail`) go in the tooltip only: the face, the popup and the toast take the kind alone.

use std::path::{Path, PathBuf};

use manifold_view_protocol::{DownloadFailure, FailureKind};

use crate::store::model::NodeId;
use crate::App;

/// The few words for `kind` on the card, the popup row and the toast; `None` for `Other`, which says no more than
/// that it failed.
fn reason(kind: FailureKind) -> Option<&'static str> {
    match kind {
        FailureKind::FolderUnreachable => Some("Can't reach folder"),
        FailureKind::FolderUnusable => Some("Can't save here"),
        FailureKind::DiskFull => Some("Disk full"),
        FailureKind::Blocked => Some("Blocked"),
        FailureKind::Network => Some("Network error"),
        FailureKind::Server => Some("Server error"),
        FailureKind::Other => None,
    }
}

/// The tooltip's first line: what happened, as a sentence.
fn long(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::FolderUnreachable => "This engine can't reach the download folder",
        FailureKind::FolderUnusable => "The download folder can't be made or written to, or can't hold this file",
        FailureKind::DiskFull => "The disk the download folder is on is full",
        FailureKind::Blocked => "Security software or a policy blocked it",
        FailureKind::Network => "The connection failed or timed out",
        FailureKind::Server => "The server refused it or sent an error",
        FailureKind::Other => "The download stopped before it finished",
    }
}

/// A failed download card's face.
pub(crate) fn card_face(kind: FailureKind) -> &'static str {
    reason(kind).unwrap_or("Download failed")
}

/// The Downloads popup's status row for a failed download.
pub(crate) fn popup_row(kind: FailureKind) -> String {
    match reason(kind) {
        Some(reason) => format!("Failed: {reason}"),
        None => "Failed".to_owned(),
    }
}

/// The toast for a download that failed at `path`.
pub(crate) fn toast(path: &str, kind: FailureKind) -> String {
    match reason(kind) {
        Some(reason) => format!("Download failed: {path} ({reason})"),
        None => format!("Download failed: {path}"),
    }
}

/// The hover text: what happened, the folder the download was meant for, and the engine's or OS's own words, a line
/// each; a blank detail or no folder adds no line.
fn tooltip(failure: &DownloadFailure, folder: Option<&Path>) -> String {
    let folder = folder.map(|f| format!("Folder: {}", f.display()));
    let detail = Some(failure.detail.trim()).filter(|d| !d.is_empty()).map(str::to_owned);
    std::iter::once(long(failure.kind).to_owned()).chain(folder).chain(detail).collect::<Vec<_>>().join("\n")
}

/// The folder a download that failed at `path` was meant for: the path's own folder when it is a full path, else
/// `app_folder`, the app's download folder (a refusal for want of a folder reports the bare file name).
fn folder_of(path: &str, app_folder: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    let path = Path::new(path);
    match path.parent() {
        Some(parent) if path.is_absolute() => Some(parent.to_path_buf()),
        _ => app_folder(),
    }
}

impl App {
    /// Download card `id`'s hover text when it failed (`tooltip`), else `None`. A screen reader hears it after the
    /// card's name (`card::card_label`).
    pub(crate) fn download_tooltip(&self, id: NodeId) -> Option<String> {
        let download = self.nodes.get(&id)?.as_download()?;
        let failure = download.failure()?;
        let folder = folder_of(&download.path, || self.download_folder());
        Some(tooltip(failure, folder.as_deref()))
    }
}
```

- [ ] **Step 3: The card label, and every surface**

In `card.rs`, after `chain_tooltip`, add:

```rust

/// What a screen reader hears for a card: its `name` (`App::accessible_name`), then each line of its hover `tooltip`,
/// which it can't hover to read, a sentence each: "Download: a.zip. The disk the download folder is on is full.
/// Folder: D:\dl". Every kind's tooltip counts: a failure's reason, an engine's error, a jar chip, hidden characters.
pub(crate) fn card_label(name: String, tooltip: Option<&str>) -> String {
    let lines: Vec<&str> = tooltip
        .into_iter()
        .flat_map(str::lines)
        .map(|line| line.trim().trim_end_matches('.'))
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return name;
    }
    format!("{}. {}", name.trim_end_matches('.'), lines.join(". "))
}
```

In `interact_card`, replace

```rust
        let response = match body.tooltip {
            Some(tooltip) if self.menus.node_menu.open.is_none() && self.menus.trail_menu.is_none() => {
                response.on_hover_text(tooltip)
            }
            _ => response,
        };
        // Screen readers: the card is a button named by its kind and title (egui announces it on focus).
        if let Some(id) = card.node().filter(|id| self.nodes.contains_key(id)) {
            response.widget_info(|| {
                let name = self.accessible_name(id).unwrap_or_default();
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name)
            });
        }
```

with

```rust
        let response = match &body.tooltip {
            Some(tooltip) if self.menus.node_menu.open.is_none() && self.menus.trail_menu.is_none() => {
                response.on_hover_text(tooltip.as_str())
            }
            _ => response,
        };
        // Screen readers: the card is a button named by its kind and title (egui announces it on focus), then its
        // tooltip, which they can't hover to read (`card_label`).
        if let Some(id) = card.node().filter(|id| self.nodes.contains_key(id)) {
            response.widget_info(|| {
                let name = card_label(self.accessible_name(id).unwrap_or_default(), body.tooltip.as_deref());
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), name)
            });
        }
```

In `card/download_body.rs`:
- Replace `        let body = CardBody { interact_rect: rect, tooltip: None };` with `        let mut body = CardBody { interact_rect: rect, tooltip: None };`.
- After the `let Some((download_path, state, total_bytes, bytes_received)) = … else { return body; };` statement, add:

```rust
        // Why it failed, on hover and after the card's name for a screen reader (`card_label`).
        body.tooltip = self.download_tooltip(id);
```

- Replace `                    (DownloadState::Failed(_), _) => "Download failed".to_owned(),` with `                    (DownloadState::Failed(failure), _) => crate::download_failure::card_face(failure.kind).to_owned(),`.

In `popups/dropdowns.rs`, replace

```rust
                        if node.failure().is_some() {
                            ui.colored_label(danger(ui.visuals()), "Failed");
```

with

```rust
                        if let Some(failure) = node.failure() {
                            ui.colored_label(danger(ui.visuals()), crate::download_failure::popup_row(failure.kind));
```

In `view_message.rs`, in the `DownloadFailed` arm, replace `                self.queue_toast(format!("Download failed: {path}"));` with `                self.queue_toast(crate::download_failure::toast(&path, failure.kind));`.

- [ ] **Step 4: Run the tests**

```bash
cargo test -j 4 -p manifold --bin manifold
cargo clippy -j 4 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
```
Expected:
- `test result: ok.`, including:
  - `every_face_fits_the_smallest_card_and_only_other_is_plain`
  - `the_popup_and_the_toast_name_the_short_reason`
  - `the_tooltip_says_why_where_and_the_engines_words`
  - `a_failed_cards_tooltip_names_its_folder_and_survives_a_rename`
  - `a_cards_label_carries_its_tooltip`
  - `a_failed_downloads_toast_says_why`
  - `graph_ui.rs`'s `card_widget_info_reaches_the_accesskit_tree`, unchanged: it builds its own label rather than calling `interact_card`.
- clippy prints no warning.

If `every_face_fits_the_smallest_card_and_only_other_is_plain` fails on a width, stop and report the face and its width. Don't raise `room` and don't reword the face: the words are the owner's call. P17 expects "Can't reach folder", the widest face, at about 98 of 104 points.

- [ ] **Step 5: Commit the app side**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold/src
git commit -F - <<'EOF'
app: a failed download card says why

download_failure words each kind: the card's face, the Downloads popup
row, the toast, and a tooltip with the folder and the engine's words. A
screen reader hears every card's tooltip after its name (card_label).

<attribution>
EOF
```
Expected: seven files (`download_failure.rs`, `main.rs`, `card.rs`, `card/tests.rs`, `card/download_body.rs`, `popups/dropdowns.rs`, `event_loop/view_message.rs`).

- [ ] **Step 6: The gate checks what a screen reader hears**

In `crates/gate/src/shots.rs`'s `download-failed` block, replace

```rust
                let failed = |s: &State| {
                    matches!(s.downloads_from(&url)[..], [d]
                        if d["failed"] == true && d["path"] == slow.as_str() && d["failure"] == "Network")
                        && s.toast().is_none()
                };
                let state = self.inst.export_until("download-failed", failed)?;
                if !failed(&state) {
                    return Err(io::Error::other(format!(
                        "the one card downloading {url} to {slow} did not show the download failed (toast {:?}): {:?}",
                        state.toast(),
                        state.downloads_from(&url)
                    )));
                }
```

(as fmt laid it out in Task 5) with

```rust
                // The download cards' names as a screen reader hears them (`controls`): kind, title, then the tooltip.
                let card_names = |s: &State| -> Vec<String> {
                    s.controls().into_iter().map(|c| c.name).filter(|n| n.starts_with("Download:")).collect()
                };
                let failed = |s: &State| {
                    matches!(s.downloads_from(&url)[..], [d]
                        if d["failed"] == true && d["path"] == slow.as_str() && d["failure"] == "Network")
                        && s.toast().is_none()
                        && card_names(s).iter().any(|n| {
                            n.starts_with("Download: slow.zip. ") && n.contains("The connection failed or timed out")
                        })
                };
                let state = self.inst.export_until("download-failed", failed)?;
                if !failed(&state) {
                    return Err(io::Error::other(format!(
                        "the one card downloading {url} to {slow} did not show why the download failed (toast {:?}, \
                         card names {:?}): {:?}",
                        state.toast(),
                        card_names(&state),
                        state.downloads_from(&url)
                    )));
                }
```

The check reads the names through `State::controls` (`state.rs:223`, since d047184e). Every `controls` entry of the export has a name and a rect (`debug_export.rs:342`), so it drops no card.

Run: `cargo test -j 4 -p gate`
Expected: PASS.

Prove the check can fail before trusting a pass (RED):
- Make this throwaway edit, which is never committed: in `interact_card`, replace `card_label(self.accessible_name(id).unwrap_or_default(), body.tooltip.as_deref())` with `self.accessible_name(id).unwrap_or_default()`.
- Run the Pre-run check (run name `dl-label-red`). Then:

```bash
cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate 2>&1 | tail -n 2
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name dl-label-red -Detach -Command 'set "VIEWS=download-progress download-failed"& set "OUT=%RUN_DIR%\shots"& target\debug\gate.exe chrome-shots "%RUN_DIR%\work"'
```

Wait (deadline 900 s). Expected:
- `"exit"` is 1, and `out.log` has a `FAIL:` line with "did not show why the download failed".
- The card names in that line include `"Download: slow.zip"` with nothing after it.
- The card's `"failure"` is `String("Network")`.

Any other error is a stop-and-report. Then remove the work folder and check the Downloads diff. Revert the throwaway edit and prove it:

```bash
git restore -- crates/manifold/src/card.rs
git status --short -- crates/manifold
```
Expected: nothing printed.

- [ ] **Step 7: GREEN, and look at the shot**

Commit the gate change first, so the run is at a clean commit:

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/shots.rs
git commit -F - <<'EOF'
gate: download-failed checks the card's reason as a screen reader hears it

<attribution>
EOF
```

Run the Pre-run check (run name `dl-label-green`). Then:

```bash
cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate 2>&1 | tail -n 2
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name dl-label-green -Detach -Command 'set "VIEWS=download-progress download-failed download-real download-resume"& set "OUT=%RUN_DIR%\shots"& target\debug\gate.exe chrome-shots "%RUN_DIR%\work"'
```

Wait (deadline 900 s). Expected:
- `"exit"` is 0, and `"commit"` is HEAD (`-dirty` only for this plan's ticks).
- The Downloads diff is empty.

The Downloads popup's "Failed: <reason>" row has no gate view: `popup-downloads` shoots the popup before any download fails (it seeds a completed one, `ADDDOWNLOAD`). Its words are pinned by `the_popup_and_the_toast_name_the_short_reason`, and the full gate (Task 10) still draws the popup.

Open `$RUN/shots/download-failed.png` with the Read tool:
- the failed card's middle reads "Network error" in the danger colour;
- the text fits inside the card.

If it reads "Download failed", or the text is clipped, stop and report it.

Set the rows' headlines:
- RED: `download-failed: label lacks the reason (throwaway revert)`.
- GREEN: `download views pass; failed card face "Network error", label carries the reason`.

Commit:

```bash
git add docs/runs.md
git commit -F - <<'EOF'
runs: dl-label RED (no tooltip in the label) and GREEN rows

<attribution>
EOF
```

---

### Task 9: Docs and PARKED

**Files:**
- Modify: `README.md:329`, the download paragraph.
- Modify: `PARKED.md:126-139` ("## Downloads (2026-10-28)").

**Interfaces:**
- Consumes: Task 1's branch and run id; the words of Task 8.
- Produces: nothing in code.

- [ ] **Step 1: README**

In `README.md`, in the paragraph starting "Clicking a download link saves the file", find the sentence `With no folder, or one that can't be made, the download shows a failed card rather than saving anywhere else — the same "pick the obvious default, don't build a save-as dialog" posture this project already takes elsewhere.` After it, insert:

```markdown
Since 2026-10-28 every failed card says why (`ViewToHost::DownloadFailed` carries a `DownloadFailure`, protocol 8). Its face shows a short reason ("Can't reach folder", "Can't save here", "Disk full", "Blocked", "Network error", "Server error", or "Download failed" when the engine says nothing more). The Downloads popup row says "Failed: <reason>", and the toast appends it. The tooltip, which a screen reader hears after the card's name, adds a sentence, the folder and the engine's or OS's own words. The host classifies its own refusals once (`downloads::destination`), and each host maps its engine's code in one function (`failure_kind`: WebView2's `InterruptReason`, CEF's `interrupt_reason()`, WebKit's `DownloadError`); all wording is in `download_failure.rs`. <C:\ SENTENCE>
```

Replace `<C:\ SENTENCE>` with the sentence for Task 1's branch:
- (a): `A folder a normal user can't create files in, such as `C:\`, fails at once as "Can't save here": Edge refuses the path (`SetResultFilePath`, run `<id>`).`
- (b): `A folder a normal user can't create files in, such as `C:\`, fails as "Can't save here": Edge ends the download `<NAME>` (run `<id>`).`
- (c), (d), (e): `No folder without write access has been seen to fail with a reason: a WebView2 download into `C:\` <was let through | never ended | never started> (run `<id>`; PARKED "A folder without write access is not known to fail with a reason").`

- [ ] **Step 2: PARKED**

In `PARKED.md` under "## Downloads (2026-10-28)", make six changes.

1. The watchdog entry ("A WebView2 download that stalls without breaking never reports"): replace `*Fix:* a host watchdog: no `BytesReceivedChanged` for a set time reports the download failed.` with `*Fix:* a host watchdog: no `BytesReceivedChanged` for a set time reports the download failed, as `FailureKind::Network` with the watchdog's words as the detail ("no data for 60 s").`
2. "CEF and WebKit download paths unexercised at runtime":
   - after "WebKit's download into a `/mnt/<drive>/...` folder under WSL.", add: ` Their failure mappings (`failure_kind` in each host, 2026-10-28) are unit-tested only: no engine has reported a code through them.`;
   - replace its `*Fix:*` with `*Fix:* that probe stage (a good folder and a refused one, checking each card's `failure` and face).`;
   - change "verified 2026-10-28" to the day of this edit.
3. "A download refused for want of a folder shows a bare file name":
   - branches (a) and (b): delete the whole entry. Its two cases are closed: a bare name's card names the folder the app means (P12), and `C:\` fails as "Can't save here".
   - branches (c), (d) and (e): replace it with

```markdown
- **A folder without write access is not known to fail with a reason.** *What:* the `FILE_*` interrupt reasons map to "Can't save here" (`failure_kind` in `crates/manifold-view-webview2/src/download.rs`, and CEF's), but no engine has reported one: a WebView2 download into `C:\` <was let through | never ended | never started> (run `<id>`). A bare file name is no longer a case: its card names the folder the app means (`download_failure::folder_of`). *Why parked:* the probe could not produce a `FILE_*` reason on this machine. *Trigger:* a failed card reading "Download failed" for a folder the user can't write to, or a WebView2 upgrade. *Fix:* reproduce with a test folder whose ACL denies the user write access, and map the reason it shows. *Evidence:* run `<id>`, verified <today>. *Status:* parked.
```

4. "Retry for a failed download": after "CEF and WebKit would need their own.", add ` Only `Network`, `Server` and `Other` failures are worth a Retry button: the folder and disk kinds fail again until the user changes something, and `Blocked` is a policy.`
5. Add at the end of the section:

```markdown
- **A click on a failed download card reveals nothing useful.** *What:* a plain click on any download card runs Reveal on its path (`reveal_node`, `crates/manifold/src/reveal.rs:65`), but a failed download's file is gone or was never made, and a refusal for want of a folder has a bare file name with no folder. *Why parked:* by decision (2026-10-28, download-failure plan: out of scope). *Trigger:* wanting the click on a failed card to do something (open its folder, retry, or open the Download folder setting). *Fix:* a failed card's click opens the folder its tooltip names (`download_failure::folder_of`), or retries (see "Retry for a failed download"). *Status:* parked.
- **Failure toasts are not coalesced.** *What:* each failed download queues its own toast (`crates/manifold/src/event_loop/view_message.rs`, the `DownloadFailed` arm), so several failing at once show one after another. *Status:* decided against (2026-10-28): rare, and each names its own file and reason.
- **A write-access check in `destination`.** *What:* checking that the download folder can be written before handing a path to the engine. *Status:* decided against (2026-10-28): it would write a test file into the user's folder; the failure's reason now says why instead.
```

6. Remove nothing else.

- [ ] **Step 3: Check the docs for old wording**

```bash
grep -rn 'DownloadFailed { path }\|pub(crate) completed: bool\|shows a bare file name\|queue_toast(format!("Download failed\|(url, path, bytes, total, completed, failed)' README.md PARKED.md ROADMAP.md CLAUDE.md docs crates 2>/dev/null | grep -v "^docs/superpowers/" | cut -c1-160
```
Expected: nothing.
- Each pattern is old text that a task above replaces: the old message shape (Task 2), the old field (Task 4), the old PARKED entry (this task), the old toast call (Task 8), and the gate's old list of export keys (Task 5).
- At d047184e the grep prints 10 lines, which shows it can find them. None of the patterns matches text this plan adds. In particular, `download_failure::toast` formats "Download failed: …" without `queue_toast`, and the new `state.rs` doc ends its list with `failure`.
- `docs/superpowers/` is left out on purpose: its plans and `mockups/README.md` (whose 2026-10-02 entry names `EngineToHost::DownloadFailed { path }`) are dated history and stay. `docs/runs.md` rows are history too.

- [ ] **Step 4: Commit**

```bash
git diff --stat
git add README.md PARKED.md
git commit -F - <<'EOF'
docs: a failed download says why; PARKED downloads updated

<attribution>
EOF
```
Expected: two files.

---

### Task 10: Final verification

**Files:**
- Modify: `docs/runs.md` (the gate row).
- Modify: this plan (`status: done`).

**Interfaces:**
- Consumes: everything above.
- Produces: the report.

- [ ] **Step 1: Format, lints, tests**

```bash
cargo fmt --all --check
cargo clippy -j 4 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo test -j 4 -p manifold --bin manifold
cargo test -j 4 -p manifold-view-protocol
cargo test -j 4 -p manifold-view-webview2
cargo test -j 4 -p gate
```
Expected: no fmt diff; no warning; every `test result: ok.`

- [ ] **Step 2: CEF and WebKit, and no placeholder left**

```bash
powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd check -Logs "<scratchpad>"
powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd test -Logs "<scratchpad>"
grep -rn 'DownloadFailure::new(FailureKind::Other, "")' crates
```
Then run the WebKit check.

Expected:
- Both CEF runs exit 0. Exit 3 ("compiled, not run") is reported as such; exit 2 is reported as "not checked".
- WebKit builds and its one test passes.
- The grep prints exactly one line: WebKit's `filename_to_uri` refusal (P6). Any other line is a leftover placeholder: stop and fix it in its owning task's style.

- [ ] **Step 3: The gate**

- Run `target/debug/gate.exe tier <B>`, where `<B>` is the base recorded in "Probe finding (Task 1)". It advises a tier. Run `full` regardless: the protocol and every engine host changed (CLAUDE.md "Gate").
- Run the Pre-run check (run name `gate-download-failure`). The full gate runs the download views, and several stages launch instances with no `seed_settings`.

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name gate-download-failure -Detach -Command 'cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'
```

Wait (deadline 4500 s). Expected:
- `"exit"` is 0;
- the Downloads diff is empty.

Set the row's headline from the gate log's last lines (`tail -n 5 "$RUN/gate-log.txt"`). Report the run's CPU load and Defender state from its `conditions` cell. If the gate fails, stop: report the failing stage and its `LOG.<stage>.log` lines with the run id.

- [ ] **Step 4: Close the plan**

- Set this plan's front matter to `status: done`. If anything was left undone, add it to `PARKED.md` first.

```bash
git diff --stat
git add docs/runs.md docs/superpowers/plans/2026-10-28-download-failure-reason.md PARKED.md
git commit -F - <<'EOF'
runs: gate-download-failure; plans: download failure reason done

<attribution>
EOF
```

- In PowerShell, check for leftovers: `Get-Process python, manifold, gate, msedgewebview2, cargo -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, StartTime, Path`. A process this work started is stopped. Anything else is named in the report, not stopped.

- [ ] **Step 5: Report**

The report states:
- what is local, committed or uncommitted (nothing is pushed or tagged);
- the probe's branch and run id;
- the CEF result: passed, compiled-not-run, or not checked;
- the WebKit result;
- the gate's run id, with its CPU load and Defender state.

It also tells the user that a dev's CEF and Servo hosts built at protocol 7 now fail the version check until rebuilt. Servo's build takes about 33 minutes (README "Building Servo on its own"); CEF's is the warm build above, if they use `target/`. Releases rebuild every host.

## Probe finding (Task 1)

Filled in by Task 1 Step 7.

- Base (B): d047184e. The plan was committed as C = 878e75f9 (the run's `-dirty` base).
- Run id: `2026-10-28-dl-fail-croot` (exit 1 as expected; `C:\` and the real Downloads folder unchanged).
- Branch: **(b)**. `SetResultFilePath` accepted `C:\real.zip`; the download then ended `INTERRUPTED` with reason 2, `FILE_ACCESS_DENIED`, which confirms Task 5's `FolderUnusable` mapping. The card's path was `C:\real.zip`, 0 of 4 bytes, `failed: true`.
- `probe.txt`:

  ```text
  naming folder: Ok(())
  starting uri http://127.0.0.1:47311/real.zip proposed C:\Users\WINSTO~1\AppData\Local\Temp\manifold-webview2-names\real.zip decided Ok("C:\\real.zip")
  state 1 reason 2 path C:\real.zip
  ```

- Task 3 variant, by the branch rules and P7's answer (A3-msg / A3-log): A3-log under P7's recommendation (no `SetResultFilePath` failure was seen); A3-msg under the D5-literal alternative. Set when P7 is answered.
- For (a), the message names a path (yes/no): not applicable (branch b).
