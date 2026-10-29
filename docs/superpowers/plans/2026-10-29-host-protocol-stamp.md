---
status: active
---

# Host Protocol Stamp Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every engine host exe carries its protocol number on disk, and the app refuses an outdated, unstamped or too-new host before spawning it, everywhere it already shows and enforces availability, with one reason text that says which side to rebuild.

**Architecture:**
- `manifold-view-protocol` puts a 20-byte record `{ tag, version, check }` in its own linker section `.mfproto`. `send_version` reads the version out of that record, so every exe that can speak the protocol carries it, and the number on disk is the number on the wire.
- `host_protocol(path)` reads that record through the PE or ELF64 section table: a few small reads, no memory map.
- `Skew { host: Option<u32>, app: u32 }`, built only by `Skew::between`, is the one mismatch type. Its `Display` is the one reason text, and it replaces `check_version` and `PROTOCOL_MISMATCH`.
- The app checks the exe a spawn would actually run (`run_in_place`: the dev build folder, then beside `manifold.exe`; for CEF only its `cef-runtime/` copy), never anything under `engine-hosts/`.
  - The result rides in `OnDisk::skew`, cached by (path, length, modified time).
  - `availability_with` turns it into `ViewStatus::Unavailable(skew)` right after the platform check, so the engine lists, Settings, the pick, the spawn and the failed card all say it.
- The handshake stays as a backstop. A mismatch found there becomes `Crashed { HOST_FATAL + skew }`, so the tab is a "can't start" card with the same reason.
- `ViewDesc` gains each engine's dev build folder (`target-servo`, `target-linux`). One resolver replaces WebKit's one-off lookup, so the dev Servo host runs where it is built, and `gate sync-runtime-deps` puts its DLLs there.
- All four hosts are rebuilt at the end and checked by the app's own verdict.

**Tech Stack:**
- Rust (nightly per `rust-toolchain.toml`, edition 2021), egui 0.36.
- `#[link_section]` with PE (MSVC link.exe) and ELF64 (GNU ld/lld with `--gc-sections`) section tables, read with `std::io::{Read, Seek}`.
- The `gate` harness (`failed-spawn`, `sync-runtime-deps`, `page-text`, `all`), `~/.claude/runs/run.ps1`, WSL Ubuntu for WebKit.

**Spec:** `C:\Users\WINSTO~1\AppData\Local\Temp\claude\C--Users-Winston-Li-Documents-GitHub-manifold\a2c355a1-5493-43d2-a2ce-bd6f49ba9245\scratchpad\item6-design.md` (owner-approved 2026-10-28), with its evidence file `item6-verify.txt` beside it. The scratchpad is temporary, so the approved decisions are restated below; they are binding. The choices this plan makes on its own are listed under "Decisions for review" (P1-P44).

**Base commit:** bfacedc2 ("docs: PARKED: one list under Chrome shots"). The tree was clean there when this plan was drafted (2026-10-29), and every line number below was read at bfacedc2. Lines drift as tasks land, and another session may commit in this tree, so find each edit by the code quoted with it, not by its line number. Task 1 Step 1 records the commit the work actually starts from, as B.
Started at 8f3f0727 (2026-10-29).

**Dates.** Dates in new text are the day of the edit. The plan was drafted on 2026-10-29, and its examples use that date. The one older fact kept with its own date is "os error 193 ... checked 2026-10-28".

## Approved decisions (the spec, restated)

- **D1. Own linker section, read through the section table.**
  - `manifold-view-protocol` defines a `#[repr(C)]` record `{ tag: [u8; 12], version: u32, check: u32 }` with `check = !version`, in `#[link_section = ".mfproto"]` (8 characters, the PE limit), initialised from `PROTOCOL_VERSION`.
  - `send_version` (channel.rs:52-56, the only non-test writer of the version on the wire) reads the version through `std::hint::black_box(&STAMP).version`. So every exe that can send the version carries the section, and the number on disk cannot differ from the number on the wire.
  - `host_protocol(path: &Path) -> Option<u32>` reads the PE or ELF64 section table, finds `.mfproto`, and returns the version when the tag and the check field agree, else `None`.
    - It makes a few small reads and never maps the file: a mapped exe can make a rebuild's linker fail to truncate it.
    - Any other format is `None`; there is no macOS target.
  - The spike (2026-10-28, throwaway): the section survives in release on MSVC and on Linux ELF (`--gc-sections`) when `send_version` reads it. An unreferenced one is dropped on ELF and kept on MSVC.
    - Reading the section table took 0.2 ms (PE) and about 3 ms (ELF, WSL /mnt/c).
    - A whole-file scan took 886 ms (memchr at opt-level 0) and 2.7 s (naive) on the 202 MB debug Servo exe, over the owner's 100 ms limit. That is why the section table is read.
  - No `gate release-verify` assertion: release hosts are embedded or hash-pinned and never stamp-read at runtime. A vanished stamp in any tree that does read it fails loudly as "unstamped" (D3).
  - Runnable checks left behind:
    - a protocol-crate test that `host_protocol(current_exe) == Some(PROTOCOL_VERSION)` for its own test exe, which links `send_version`;
    - parser tests on small synthetic PE and ELF64 images;
    - a garbage or truncated file returns `None` and never panics.
    - CI runs the protocol crate on Linux, so ELF is covered there and PE on Windows.
- **D2. The PDF renderer's mismatch survives to the user; no pdfium stamp yet.**
  - In `run_child` (render.rs:153-166), test for the `render protocol mismatch` reason before the exit-code branches, kill the child, and append "; rebuild manifold-view-pdfium". P22 departs from this wording and waits for the owner.
  - Today pdfium exits 1 on the broken pipe first ("couldn't write the frame"), so the mismatch text never reaches the card.
  - Add a `render_stub` mismatch mode whose frame body is larger than a pipe buffer, so the test exercises the real ordering.
  - Park the pdfium stamp. Trigger: the first `RENDER_PROTOCOL_VERSION` bump (it has been 1 since inception, render.rs:11).
- **D3. An unstamped host reads as outdated**, with its own text (there is no number to print).
  - Every text names which side to rebuild: a host older than this build needs the engine host rebuilt, a newer one needs manifold rebuilt.
  - gate `fresh.rs` test `fresh_exes_pass_and_each_stale_one_is_named` builds hosts as touched empty files. It matters only if gate reads stamps, which it does not (D7). Re-check it still passes unchanged.
  - The `probes.rs` failed-spawn stage writes a 13-byte "not a program" file where the host goes. It would now be refused as unstamped before the OS spawn it exists to test. Keep its intent; the plan decides how (P15).
  - `process_manager/tests.rs:629-639` reads the real disk, and `target-linux/debug` is reachable from cargo test's `current_exe`. In tests the skew must be an injected `OnDisk` value, never a real disk read.
  - The design counted "about 20 `OnDisk` literals across 5 files". There are 6 in 4 files at bfacedc2 (P24).
- **D4. One rule for which exe is checked, carried in `OnDisk`.**
  - Check the exe a spawn would actually run. Split the first half of `view_binary_path` (the cef-runtime copy for CEF, the sibling otherwise, if it exists) into `run_in_place(kind) -> Option<PathBuf>`, used by both `view_binary_path` and the check. WebKit (TCP link) uses the same lookup through `pick_webkit_host`.
  - Do NOT check `sibling_host`: it returns CEF's flat exe first while a tab runs the cef-runtime copy. The design says to leave `sibling_host` as it is, since `shipped_cef_host` and `ensure_host_copy` want flat-first. P10 departs from that for the non-CEF kinds and waits for the owner.
  - Check every kind: today `read_on_disk` and `view_on_disk` return early for engines that are not downloaded, and `availability_with` returns Installed for them. The one exception is an exe under `view_hosts_dir()`: those were hash-checked by the installer or are the embedded WebView2 host. This rule also avoids a release-upgrade deadlock (`check_spawnable` runs before the embedded host is extracted).
  - Cache the read by (path, length, mtime). `availability_with` returns `Unavailable(skew.to_string())` after the platform check and before the `!downloads` return.
  - Fix every place that assumes Unavailable means "this platform can't run it", or that offers a download for it:
    - `missing_view_phrases` and the failed card's "not installed (click to download)";
    - the node menu's "Download Engine";
    - the doc comments on `ViewStatus::Unavailable`, `PickOutcome::Unavailable`, the engine row's dim comment and the Settings default line;
    - `on_disk_text`, which would print "host ready" beside "Unavailable".
  - Known gap, stated and not closed: a custom bring-your-own CEF directory gets a copy of `shipped_cef_host()` in a folder outside `view_hosts_dir()` (`prepare_cef`, `ensure_host_copy`), which is not stamp-checked. The handshake backstop catches it (PARKED, Task 11).
- **D5. The handshake mismatch is a "can't start" with a reason; the special case is deleted.**
  - A `Skew` type with named sides replaces `check_version`: `Skew { host: Option<u32>, app: u32 }`, built only as `Skew::between(host, app)`, so no caller can reverse the labels. The host-side caller (transport.rs:48-58) reverses them today. Its `Display` is the only reason text (D3's three cases).
  - The app's reader (process_manager.rs:881-893, the only app-side caller) reports `Crashed { reason: HOST_FATAL + skew }`. So `after_crash` returns `CannotStart`, and the failed card's toast reads "Couldn't start <engine> for <url>: <reason>".
  - Delete `PROTOCOL_MISMATCH`, its `GiveUp` branch in `after_crash`, `check_version`, and the assertions on them.
  - Widen `HOST_FATAL`'s doc, which today says "exits before connecting". Fix the docs on `AfterCrash` and `leave_failed_card`. Close plan 2026-10-11's item 9 (its line 39).
- **D6. `Skew` and `host_protocol` live in `manifold-view-protocol`; gate takes no dependency on it.** GATE-CEF-DIR (PARKED) stays parked: this does not subsume it.
- **D7. gate's freshness check is unchanged**, apart from fixing its wrong comment (fresh.rs:54-55 says a stale copy "speaks an old message numbering and its host crashes"; only `gate sync-runtime-deps` refreshes it).
  - The app's own refusal, now with its reason, appears in a stage's log ("spawn tab failed for <id>: ...").
  - A gate-side readiness helper (reading `view_kinds[engine].state` from `export_state`) is added only if that proves hard to read. It is parked with that trigger.
- **D8. The dev Servo host is found where it is built.**
  - Each engine's `ViewDesc` gains its dev build folder (`target-servo` for Servo, `target-linux` for WebKit).
  - One resolver replaces `built_webkit_host`'s WebKit-only walk: beside `manifold.exe` (a package), or the dev folder found by walking up from `manifold.exe`'s folder as WebKit does now. The order when both exist is the plan's to decide (P8: the dev folder first, as recommended); README says so.
  - `gate sync-runtime-deps` puts the two ANGLE DLLs (libEGL.dll, libGLESv2.dll, the Servo host's whole runtime per release.yml and package.rs) beside the Servo exe in `target-servo/debug`. It searches both build folders' `mozangle` output; both copies are byte-identical today.
    - A missing DLL is an error when a Servo exe is there; a missing `target-servo` is skipped quietly.
    - A locked file must not mask the other syncs (today `run()` aborts on the first error). Its error names the file and says to close manifold.
  - Delete the stale copy `target/debug/manifold-view-servo.exe` (and its PDB) once nothing looks there. There is no PDB beside it (checked 2026-10-29; P39).
  - Docs: README build list and command, "Runtime-file placement", the trail and engine-setup lines, "Building Servo on its own"; the `sync_deps.rs` module doc; the `all.rs` pointer; CLAUDE.md only if it names the copy (it does not).
- **D9. Rebuild all four hosts once, after the code lands**, each verified stamped by the app's own verdict (no Unavailable for it) or by `host_protocol`:
  - WebView2: `cargo build -j 4 -p manifold-view-webview2` (part of the normal build line).
  - CEF: through the CEF guard (warm `cef-dll-sys` build `1350bf2d9e1a185f`; a new cef-dll-sys folder means stop), as `cargo build -j 4 -p manifold-view-cef`, then `target/debug/gate.exe sync-runtime-deps` so the cef-runtime copy is fresh.
  - Servo: `CARGO_TARGET_DIR=target-servo cargo build -j 4 -p manifold-view-servo` (about 15 s warm, docs/runs.md row `2026-10-28-servo-build-settings`), through run.ps1 so it has a record; then `sync-runtime-deps` for its DLLs.
  - WebKit: in WSL, `CARGO_TARGET_DIR=target-linux cargo build -j 4 -p manifold-view-webkitgtk`.
- **D10, D11.** Done 2026-10-28: the cold `cef-dll-sys` build (the guard points at `1350bf2d9e1a185f`), and the `manifold-engine-*` leftovers deleted.
- **Subsumed:**
  - the CEF flat-versus-copy resolver divergence;
  - the sibling-only exemption, the three `!downloads` early-outs, and WebView2's early-return hole;
  - the mismatch special case in `after_crash`, and the reversed host-side label;
  - WebKit's one-off dev lookup, and the manual Servo copy and its drift.
- **Not covered:**
  - render protocol staleness (D2, parked stamp);
  - staleness that does not change the protocol number (dep-info and the CEF copy age check stay);
  - the custom CEF directory gap (backstop only);
  - GATE-CEF-DIR.
- **Resist adding:** a per-kind CEF branch in the stamp check, a gate-side engine list, a new `ViewStatus` variant, or a second `SpawnError` for skew beside `ViewUnavailable`.
- **Also to record:**
  - four new PARKED entries: the pdfium stamp; the render child's `child.wait()` with no deadline; the gate readiness helper; the custom CEF directory gap. Fix GATE-CEF-DIR's stale line numbers.
  - No `PROTOCOL_VERSION` bump: the stamp adds a section, and the wire format is unchanged.
  - First run after landing: a fresh profile can still start on an outdated engine (`platform_default_view_kind` uses only the platform reason); its failed root card then explains why. Recommended, and done (P21): no change, since the rebuild step makes it moot in this tree.

## Decisions for review

These are choices this plan makes inside the approved design. Each lists what it recommends and the alternative.

**Owner sign-off before building.** Two of them depart from the approved design's wording, so they wait for the owner's answer, recorded on each one's `Answer:` line:
- P10 (D4: "leave `sibling_host` as it is"; P10 routes the non-CEF kinds through `run_in_place`, so a dev Servo host in `target-servo` reads Installed);
- P22 (D2: append "; rebuild manifold-view-pdfium"; P22 names the side to rebuild by direction, like D3).

Task 3 needs P22's answer, and Task 4 (and so every task after it) needs P10's. An executor that reaches either task while its `Answer:` line still reads "pending" stops and asks; it never picks an option itself. Tasks 1 and 2 need neither.

- **P1. Where the pieces live.**
  - Recommended: `Skew` in `lib.rs`, beside `PROTOCOL_VERSION` and `HOST_FATAL`; the record and its reader in a private `stamp.rs`, which re-exports only `host_protocol`. `send_version` reaches the record as `crate::stamp::STAMP`.
  - Alternative: one public `stamp` module with both. `Skew` is about the protocol, not the file format.
- **P2. The tag is `*b"manifold-ver"`** (12 bytes). It is readable in a hex dump, and the check field guards against a chance match. Alternative: random bytes.
- **P3. Formats: PE and ELF64 little-endian only.** Anything else, a 32-bit ELF included, reads `None`. Alternative: Mach-O now; there is no macOS target (YAGNI), and the module doc says what Mach-O would need.
- **P4. The reason texts (`Skew`'s `Display`).**
  - Unstamped: "the engine host carries no protocol number (built before hosts were stamped); rebuild the engine host".
  - Numbered: "the engine host speaks protocol {host}, manifold speaks {app}; rebuild the engine host" when the host is older, "...; rebuild manifold" when it is newer.
  - Each text is written from the app's point of view whichever side detects it. The host's own log line uses the same text.
  - Alternative: keep a "protocol mismatch:" prefix. After D5 nothing reads a prefix, and the text follows "<engine> is unavailable: " in every place it is shown.
- **P5. `on_disk_text` replaces the host word when skewed:** "host protocol 7, manifold 8" or "host unstamped", then ", runtime ..." for CEF as before. Alternative: append the skew after "host ready", which reads as a contradiction beside "Unavailable".
- **P6. One function per surface for what a failed tab's click does.**
  - `missing_view_phrases` gains an `Unavailable` arm and carries its own click hint: "(click to download)" or "(click for why)".
  - `PickOutcome::retry_label` names the menu's first row from the same `pick_outcome` that `retry_missing_file` acts on, so the label cannot drift from the action.
  - Alternative (the design's critic): derive the card text from `view_status_label` plus an "offers install" predicate. `view_status_label` gives a state line ("Unavailable: <whole reason>"), too long for a card body and a menu row.
- **P7. The menu row for an Unavailable engine reads "Why Unavailable".**
  - Clicking it toasts the reason through `retry_missing_file`'s existing `Unavailable` arm.
  - `AlreadyInstalling` keeps today's "Download Engine" label; its click says it is already installing.
  - Alternative: "Show Reason".
- **P8. Lookup order: the dev folder first, then beside `manifold.exe`.**
  - The dev folder is the one supported dev route. A stray copy left beside the exe (the very drift D8 removes) must not shadow a fresh build.
  - A package has no dev folder, so it is unaffected.
  - Alternative: beside the exe first, as `built_webkit_host` did.
- **P9. `host_file_name(kind)` is the one file-name rule.** A host built for another OS than this one (WebKit's ELF on Windows) keeps its bare name; every other host gets `PLATFORM.host_exe`. It is driven by the existing `ViewDesc::host_os`, not a WebKit branch. Alternative: a WebKit special case, as `built_webkit_host` and `pick_webkit_host` have today.
- **P10. `sibling_host` (needs the owner's answer).**
  - Recommended:
    - For CEF, `sibling_host` is unchanged: its flat exe first, then its `cef-runtime/` copy.
    - Every other kind returns `run_in_place(kind)`.
    - Why: `host_on_disk`, `prepare_webview2` and `spawn_tab`'s re-hash all use `sibling_host` to mean "a host ships in place". With D8's dev folder, a Servo host in `target-servo` must read Ready there. Otherwise Servo reads Unavailable ("this build has no Servo download") in a dev tree once the flat copy is gone, and the spawn refuses the host it would run.
  - Alternative (D4's wording): leave `sibling_host` alone and add the dev folder to `run_in_place` only. The dev Servo host is then never usable in a dev tree.
  - Answer: recommended (owner, 2026-10-29)
- **P11. The `engine-hosts/` exemption is structural.** `run_in_place` looks only beside `manifold.exe` and in dev folders, so it never yields a path under `view_hosts_dir()`, and no path-prefix test is needed. Alternative: a `starts_with(view_hosts_dir)` filter, which would need canonical paths on both sides.
- **P12. `run_in_place` returns `None` under `cfg(test)`**, like `profile::dir()`, so no unit test reads a real host's stamp (D3). The lookup itself is tested through `run_in_place_from(kind, exe_dir)` on a temp folder, and the stamp read through `read_on_disk_at(kind, in_place, cache)` on the test's own exe and a temp file (Task 5, P41). Alternative: thread an exe folder through every caller.
- **P13. The stamp cache.** `stamp_cached(cache, path, read)` takes its reader as a parameter, keys each entry by (path, length, modified time), and never caches a path it cannot stat. It sits under the 5 s `ON_DISK` cache, so a stat runs at most every 5 s per engine. Alternative: key by path with a TTL, which would miss a rebuild inside the TTL.
- **P14. The parser.**
  - It is generic over `Read + Seek`, so the tests feed it a `Cursor`.
  - Every allocation is capped: at most 1024 sections, at most 64 KiB of section names.
  - An ELF section header must be 64 bytes, and the record must sit at the start of the section.
  - Alternative: a memory map, rejected by D1.
- **P15. The failed-spawn stage keeps its intent with a genuine host whose PE optional-header magic is zeroed.**
  - The copy still carries its stamp, so the app's check passes, and Windows refuses to start it (os error 193, checked 2026-10-28).
  - A new check asserts the log shows an OS error and no "is unavailable", so the stage cannot silently turn into a pre-spawn refusal again.
  - Alternative: a hand-made stamped non-program, which would need a PE writer in gate.
- **P16. D7 as written:** only fresh.rs's comment changes; the readiness helper is parked.
- **P17. `sync-runtime-deps` stops copying Servo's DLLs into `target/debug/`.** After Task 4 nothing runs a Servo host from there once Task 8 deletes the stale copy. Alternative: copy to both places, which keeps the drift D8 removes.
  - A dev Servo host is supported only when built in `target-servo`. One built into `target/` by hand (`cargo build -p manifold-view-servo` without `CARGO_TARGET_DIR`) still runs, from beside `manifold.exe` when `target-servo` has none, but nothing puts its DLLs there: its tab fails to start, and the sync says nothing about it. README "Building Servo on its own" says so (Task 10). Alternative: `sync_servo` reports such an exe as an error, a check for one wrong layout that the docs already rule out.
- **P18. What counts as a file in use.**
  - os error 32 (sharing violation), 33 (lock violation) or 1224 (a user-mapped section open) names the file and says to close manifold.
  - Errors are collected, so every step runs. `finish(outcome)` turns them into the `Err` that `sync_runtime_deps` exits 1 on; it is split out of `run`, whose paths are fixed to the repo, so a test pins that mapping (review round 2).
  - Alternative: stop at the first error (today's behaviour, the masking D8 forbids).
- **P19. `find_newest` takes `&[PathBuf]`** (every build folder to search) in place of one folder; the CEF caller passes one.
- **P20. A dev tree's flat CEF exe without its `cef-runtime/` copy is not stamp-checked.** It is folded into the same PARKED entry as the custom-directory gap; the handshake backstop catches both.
- **P21. First run unchanged, no `PROTOCOL_VERSION` bump** (the design's recommendation).
- **P22. The render mismatch names the side to rebuild by direction (needs the owner's answer).**
  - Recommended: `read_frame` ends its reason "; rebuild manifold-view-pdfium" when the renderer is older and "; rebuild manifold" when it is newer, and `run_child` passes it through unchanged.
    - This matches D3's rule that every text names which side to rebuild. It keeps the wording in the one function that has both numbers.
  - Alternative (D2's wording): `run_child` appends "; rebuild manifold-view-pdfium" always, which is wrong for a newer renderer.
  - Answer: recommended (owner, 2026-10-29)
- **P23. The CEF guard gains a `build` mode** (D9 builds `manifold-view-cef` under the same guard as the compile check).
- **P24. `OnDisk` literals: 6 in 4 files** (availability.rs:184, process_manager/tests.rs:492, view_info.rs:408, view_install/tests.rs:28, :1349, :1352), not about 20 in 5. Each gains `skew: None`, or is rewritten by its task.
- **P25. The `!downloads` early-outs affect only WebView2.** Servo, CEF and WebKit all download.
- **P26. How D9 is verified.** A headless `VIEWSTATE` run per engine, with `APPDATA` redirected into the run folder so the real `engine-hosts/` is never read, then one live `page-text` on Servo and CEF. Alternative: `host_protocol` through a throwaway exe, which proves the stamp but not the app's verdict.
- **P27. The final gate is `full` without `WITH_ENGINES`**, as CLAUDE.md "Gate" says. Servo and CEF are exercised live by Task 9's `page-text` run.
- **P28. A skew outranks a custom runtime, an install in flight and a recorded failure:** none of them changes which exe is stamp-checked. Only the platform reason comes before it.
  - The one case where the checked exe is not the one that starts (review round 2): CEF with a custom runtime. `prepare_cef` then starts a copy of `shipped_cef_host()` (`sibling_host(Cef)`: in a dev tree the flat `target/debug/manifold-view-cef.exe` first), while the check reads the `cef-runtime/` copy (`run_in_place`). So a stale `cef-runtime/` copy beside a current flat exe reads Unavailable although the tab would start (a false refusal, cleared by `gate sync-runtime-deps`), and a stale flat exe beside a current copy starts and is caught by the handshake. Both directions are in the PARKED gap entry (Task 11) and README's paragraph (Task 10).
  - Alternative: read `shipped_cef_host()` when a custom runtime is set, the per-kind CEF branch in the stamp check the design says to resist.
- **P29. The stub's mismatch mode mimics pdfium:** on a write error it prints "couldn't write the frame: ..." and exits 1, as pdfium's main.rs:23 does.
- **P30. B is HEAD when Task 1 starts.** If it is not bfacedc2, the anchors in the files that moved are re-found first.
- **P31. `#[allow(dead_code, reason = "...")]` on the record's `tag` and `check` fields:** the code reads them only from the file, never through the struct.
- **P32. Test housekeeping.**
  - `bundled_view_kinds_read_ready_without_a_disk_check` is deleted: its "without a disk check" is no longer true, and `the_lists_read_the_same_host_the_spawn_looks_for` keeps its one assertion.
  - `view_kinds_that_are_not_downloaded_are_always_installed_where_the_os_can_run_them` drops "always".
- **P33. `cef_sibling_exe` is defined through `beside_exe`,** so the cef-runtime layout is written once.
- **P34. The walk up from the exe stays at 5 folders** (`built_webkit_host`'s) and now serves Servo too. `ancestors()` starts with the exe's own folder, so that is the exe's folder and its four parents; `the_dev_folder_is_searched_four_parents_up_and_no_further` (Task 4) pins both edges (round 3).
- **P35. README "Failed spawns and the empty graph" (:188) stays as it is:** it is still true. The `gate sleep` line (:462) is true except for the old script name (`scripts/sync-runtime-deps.sh`), which Task 10 renames because its grep finds it. The 2026-10 disk-full entry (:702) is a dated incident record and stays.
  - Finished specs and decisions are dated records and stay too; Task 10's grep searches them so a live claim there would show. At bfacedc2 it finds `docs/superpowers/specs/2026-10-02-engine-install-design.md` :60, :76, :120 (`engine_binary_path`, the old script name), `2026-10-12-file-renderers-design.md:78` (the render mismatch's old text), and `2026-10-14-trail-design.md:258` (a measurement on a copied Servo host), all in `status: done` specs.
  - `docs/decisions/2026-10-18-hosts-in-the-release.md:15` ("its engines read Unavailable unless a sibling host exists") stays as well: it is still true of the no-pin case, and a skewed host is a second cause that README's one paragraph states. Alternative: a dated note in the decision, which would restate that paragraph.
  - The grep leaves out `docs/superpowers/plans`. Plans are step-by-step history, and this plan's own repo copy quotes every old string, so a grep there would always match. The one finished plan this work touches, `2026-10-11-bare-exe-host-downloads.md`, gets a closing note on item 9 (Task 10). Its item 6 ("Dev builds and Linux CI have an empty table and keep sibling lookup.", :36 at bfacedc2) is history and stays.
- **P36. The WebKit host is rebuilt in Task 2** (its check builds it, stamped) and checked again in Task 9.
- **P37. Task 5's steps are split** (field, then the skew branch, then the cache), so each RED is a single cause.
- **P38. The Servo rebuild is watched for a cold start.** A new folder under `target-servo/debug/build/` means cargo began rebuilding the dependency tree (about 33 minutes): the run is stopped, reported, and nothing is deleted.
- **P39. Only three files are deleted from `target/debug/`:** `manifold-view-servo.exe`, `libEGL.dll` and `libGLESv2.dll`. There is no Servo PDB there. Nothing else in `target/debug` loads ANGLE: manifold.exe draws with wgpu.
- **P40. `sibling_binary` keeps its two remaining users** (the PDF renderer and CEF's flat exe), and its doc says so.
- **P41. `read_on_disk_at(kind, in_place, cache)` is the tested seam of the stamp check** (review rounds 1 and 2). `read_on_disk(kind)` is `read_on_disk_at(kind, run_in_place(kind).as_deref(), &STAMPS)`, and `read_on_disk_at` is tested with WebView2 (not downloaded, so no other disk read) on the test's own exe (stamped, since it links `send_version`), on a temp file with no stamp, and with no host in place. That pins that the disk check fills `skew` from the host it is given, which reader runs and which number is the host's. Only the one line passing `run_in_place(kind)` is pinned live (Task 9 Step 2), since `run_in_place` is `None` under test (P12). Round 1's `skew_of` and `host_skew` are folded into it: with the host passed in, neither has a second caller. Alternative: thread an exe folder into `read_on_disk` so a unit test reaches `run_in_place` too, the change P12 rejected.
- **P42. `missing_view_text(kind, state)` builds a failed card's whole body text** (review round 1), so the test pins the string the card draws, label and hint included, and `tab_body.rs` adds no words of its own. Alternative: a paint test of the tab body that reads the drawn text, as the group card's test does; heavier, for one string.
- **P43. The render stub's `mismatch-linger` mode pins `run_child`'s kill on a mismatch** (review round 1). It writes the header alone, then keeps running for at most 20 s, so a `run_child` that waits for it fails the test in 20 s instead of hanging it (the `linger` mode parks forever, which is fine there only because its kill already exists). Alternative: drop the kill and rely on pdfium exiting on the broken pipe, which leaves a hanging mismatched renderer holding a worker.
- **P44. `unloadable(image)` is the pure half of `unloadable_copy`** (review round 1), so a unit test checks on a synthetic PE image that only the magic changes and that a non-PE file is refused. Alternative: test `unloadable_copy` through a temp file, which adds file handling to a byte check.

## Global Constraints

- **Start condition.**
  - `git status --short -- crates README.md PARKED.md docs` must print nothing before Task 1 and before each commit, apart from the current task's own files.
  - This is the guard against another session's work. If anything else is listed, stop and report it. Never stage, revert or edit another agent's changes.
  - When this was checked (2026-10-29, at bfacedc2), the tree was clean.
- **One cargo per target dir.** Before any cargo command, run `Get-Process cargo -ErrorAction SilentlyContinue` (PowerShell). If one is running, wait for it to exit: it is another session's build in `target/`.
- **Commands.**
  - Every cargo line uses `-j 4`.
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
  - No test reads or writes the real `%APPDATA%`. Tests that need folders use `crate::store::testutil::temp_dir` (app) or `std::env::temp_dir()` (protocol, gate).
  - No unit test reads a real host's stamp: `run_in_place` is `None` under `cfg(test)` (P12), and every skew an availability test uses is an `OnDisk` value the test builds. The one real file a test reads a stamp from is its own exe (`std::env::current_exe()`, in the protocol crate's and in `read_on_disk_at`'s test).
  - No run writes into the user's Downloads folder. Each run step diffs it before and after.
- **Python.** Run Python only as `python -P f.py < /dev/null`, with the script written to a file first. No step here needs Python.
- **Wording.** The mismatch text lives only in `Skew`'s `Display` (and, for the PDF renderer, in `read_frame`). No caller adds its own words around it beyond the existing "<engine> is unavailable: " and "Couldn't start <engine> for <url>: ".
- **CEF and WebKit.**
  - Neither is in the clippy line or the hook. CEF compiles only through the CEF guard below; WebKit only through WSL.
  - No host crate's source changes in this plan. Each host changes only through the protocol crate it links (`send_version`, `spawn_host_reader`). Task 2 compiles both after the host-side reader changes, and Task 9 rebuilds all four.
- **Disk.** C: has about 15 GB free. Make no new target dir. The CEF guard refuses to start a cold `cef-dll-sys` build, and Task 9 stops a cold Servo build (P38).
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

`RUN` in the steps below is the run's folder, `$LOCALAPPDATA/claude-runs/manifold/<YYYY-MM-DD>-<Name>`: the launch day and the run's `-Name`, with `-2`, `-3` appended if the name was already used that day. A run started without `-Detach` returns when it ends and prints no run id, so find its folder by name: `RUN=$(ls -dt "$LOCALAPPDATA"/claude-runs/manifold/????-??-??-<Name>* | head -1)` (newest first; check `echo "$RUN"` printed a folder). A detached run prints `run <id> started: ...`, and `<id>` below is that id. For a detached run, run this as a background Bash call (`run_in_background: true`). Use deadline 900 s for a probe and 4500 s for the full gate:

```bash
RUN="$LOCALAPPDATA/claude-runs/manifold/<id>"; deadline=$((SECONDS+900)); until grep -Eq '"end": +"' "$RUN/run.json" || [ $SECONDS -ge $deadline ]; do sleep 5; done; grep -E '"(exit|headline|commit)"' "$RUN/run.json"; tail -n 15 "$RUN/out.log"
```

`run.ps1` writes `run.json` with Windows PowerShell's `ConvertTo-Json`, which puts two spaces after each colon (`"exit":  0,`), so read each field's value, never its spacing; a pattern that matches one uses ` +`, as the loop's `"end": +"` does. "`run.json`'s `"exit"` is `0`" below means that value.

Afterwards, `ls -A "$USERPROFILE/Downloads" | diff "<scratchpad>/downloads-before-<run name>.txt" -` must print nothing. Any difference is a stop-and-report: name each new file.

A gate stage that fails keeps its work folder (`<run folder>/gate-<stage>-<pid>`); `probes::finish` deletes it only on a pass. Remove it once its output has been read: `rm -rf -- "$RUN"/gate-<stage>-*`.

### CEF guard (create once per session)

Copy the script under "### CEF guard (create once per session)" in `docs/superpowers/plans/2026-10-28-download-failure-reason.md` (the body of its PowerShell block, lines 245-296 at bfacedc2, from `# Compile-checks (check) or tests (test)` to `exit $t.ExitCode`) to `<scratchpad>\cef-check.ps1` with the Write tool, with these four edits so it can also build the host (P23):

1. First line: `# Compile-checks (check), tests (test) or builds (build) manifold-view-cef on the warm cef-dll-sys build only; never starts a cold one.`
2. The `param` line becomes `param([ValidateSet('check', 'test', 'build')][string]$Cmd = 'check', [Parameter(Mandatory)][string]$Logs)`.
3. The `$cargoArgs = if ...` line becomes:

```powershell
$cargoArgs = switch ($Cmd) {
    'check' { 'check -j 4 -p manifold-view-cef --all-targets' }
    'test' { 'test -j 4 -p manifold-view-cef --no-run' }
    'build' { 'build -j 4 -p manifold-view-cef' }
}
```

4. `if ($p.ExitCode -ne 0 -or $Cmd -eq 'check') { exit $p.ExitCode }` becomes `if ($p.ExitCode -ne 0 -or $Cmd -ne 'test') { exit $p.ExitCode }`.

Everything else is copied as it stands. Run it from bash as `powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd check -Logs "<scratchpad>"` (or `-Cmd test`, `-Cmd build`). The scratchpad path is in 8.3 form and has no space. The exit codes are:
- 0: passed (for `build`: the host is built at `target/debug/manifold-view-cef.exe`).
- 1 or 101: a compile error or a test failure. Read the log.
- 2: not checked. Report it; never work around it.
- 3: compiled but not run. Report it as such.

### WebKit check

```bash
wsl.exe -d Ubuntu -- bash -lc 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo build -j 4 -p manifold-view-webkitgtk && CARGO_TARGET_DIR=target-linux cargo test -j 4 -p manifold-view-webkitgtk'
```

`target-linux` is warm (built 2026-10-28). Use a 600000 ms timeout. It also leaves `target-linux/debug/manifold-view-webkitgtk` built from the current tree, which is the host the app runs in place on this machine.

## Review Focus

Each is an input or condition the spec implies that is most likely to bite a person using this, most likely first. The test that pins it is named after the line.

1. **A host rebuilt in place, at the same path, while manifold runs.** The next read must see the new number, not a cached one: a rebuild changes the file's length or modified time. Pinned in Task 5, `a_rebuilt_host_is_read_again`.
2. **A host file that is truncated, garbage, not an executable, or a folder.** The reader must never panic or allocate without bound, and the host reads as unstamped, so the user is told to rebuild it. Pinned in Task 1, `truncated_or_garbage_files_read_none` and `a_missing_file_or_a_folder_reads_none`.
3. **A host newer than manifold** (a host rebuilt after pulling, manifold not). Both the pre-spawn refusal and the handshake backstop must say "rebuild manifold", not "rebuild the engine host". Pinned in Task 2, `a_host_speaking_another_protocol_cannot_start_and_says_which_side_to_rebuild` and `the_host_names_which_side_to_rebuild`, and in Task 5, `a_host_speaking_another_protocol_is_unavailable_with_its_reason`.
4. **A failed tab whose engine is Unavailable.** Neither the card nor its menu may offer a download that cannot help; both say a click explains why. Pinned in Task 6, `a_failed_card_says_what_a_click_on_it_does` and `a_failed_tabs_first_row_follows_its_engines_state`.
5. **CEF with a fresh flat exe and a stale `cef-runtime/` copy** (built, not synced). The check must read the copy a tab runs, not the flat exe. Pinned in Task 4, `each_host_is_found_in_its_dev_folder_then_beside_the_exe` (a flat CEF exe alone is not in place).

## File Structure

- `crates/manifold-view-protocol/src/stamp.rs` (new): the record, `host_protocol` and its PE/ELF readers, and their tests.
- `crates/manifold-view-protocol/src/lib.rs`: `mod stamp`, the `host_protocol` re-export, `Skew`, the `HOST_FATAL` doc; `PROTOCOL_MISMATCH` and `check_version` deleted.
- `crates/manifold-view-protocol/src/channel.rs`: `send_version` reads the record.
- `crates/manifold-view-protocol/src/transport.rs`: `skew_with_app` and the host-side reader.
- `crates/manifold-view-protocol/src/render.rs`, `src/bin/render_stub.rs`, `tests/render_child.rs`: the render mismatch (Task 3).
- `crates/manifold/src/process_manager.rs`: `ViewDesc::dev_dir`, the app's reader, the test imports, the `HostSource` docs (Task 10).
- `crates/manifold/src/process_manager/availability.rs`: the one resolver (`host_file_name`, `beside_exe`, `run_in_place`), the stamp cache, `read_on_disk_at`, and the skew branch in `availability_with`; the module doc and `host_on_disk`'s doc (Task 10).
- `crates/manifold/src/process_manager/tests.rs`: the lookup, skew and cache tests; two comments of the Servo availability test (Task 10).
- `crates/manifold/src/view_install.rs` (`OnDisk::skew`, docs) and `crates/manifold/src/view_install/tests.rs` (literals).
- `crates/manifold/src/tabs.rs`, `tabs/tests.rs`, `event_loop/view_message.rs`: the handshake mismatch as a "can't start".
- `crates/manifold/src/view_info.rs` (`on_disk_text`, `PickOutcome::retry_label`), `card.rs`, `card/tab_body.rs`, `card/tests.rs`, `node_menu.rs`, `node_menu/tests.rs`, `settings_page.rs`, `fetched.rs` (a comment): the wording.
- `crates/gate/src/probes.rs` (failed-spawn), `fresh.rs` (comment), `sync_deps.rs` (rewrite), `all.rs` and `package.rs` (docs).
- `README.md`, `PARKED.md`, `docs/runs.md`, `docs/superpowers/plans/2026-10-11-bare-exe-host-downloads.md`, and this plan's repo copy `docs/superpowers/plans/2026-10-29-host-protocol-stamp.md`.

---

### Task 1: Every exe carries its protocol number (`.mfproto`, `host_protocol`)

**Files:**
- Create: `docs/superpowers/plans/2026-10-29-host-protocol-stamp.md` (this plan, `status: active`).
- Create: `crates/manifold-view-protocol/src/stamp.rs`.
- Modify: `crates/manifold-view-protocol/src/lib.rs`:
  - the module list, lines 1-9 (`mod stamp;` after `pub mod render;`);
  - the re-exports, line 20 (`pub use stamp::host_protocol;` before `pub use transport::{LocalTransport, RemoteTransport};`).
- Modify: `crates/manifold-view-protocol/src/channel.rs:52-56` (`send_version`).

**Interfaces:**
- Consumes: `crate::PROTOCOL_VERSION`, `crate::MessageChannel`.
- Produces:

```rust
// manifold_view_protocol::host_protocol (re-exported from the private `stamp` module)
pub fn host_protocol(path: &std::path::Path) -> Option<u32>;
// inside the crate
pub(crate) struct Stamp { tag: [u8; 12], pub(crate) version: u32, check: u32 }
pub(crate) static STAMP: Stamp; // in #[link_section = ".mfproto"]
```

- [ ] **Step 1: Start**

- Run the start check: `git status --short -- crates README.md PARKED.md docs` prints nothing, and `Get-Process cargo -ErrorAction SilentlyContinue` (PowerShell) prints nothing.
- Run `git rev-parse --short HEAD`. That commit is B. If it is not bfacedc2, run `git diff --stat bfacedc2 B -- crates README.md PARKED.md docs` and re-find every anchor this plan quotes in the files it lists before going on.
- Copy this plan to `docs/superpowers/plans/2026-10-29-host-protocol-stamp.md`.
  - Set its front matter to `status: active`.
  - Under **Base commit:**, add the line `Started at <B> (<today's date>).`.
  - Copy the owner's answers to P10 and P22 into their `Answer:` lines if the owner gave them.

```bash
git add docs/superpowers/plans/2026-10-29-host-protocol-stamp.md
git commit -F - <<'EOF'
plans: host protocol stamp

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

- [ ] **Step 2: Write the failing tests**

Create `crates/manifold-view-protocol/src/stamp.rs` with the module doc and the test module only (Step 4 adds the code between them):

```rust
//! The protocol number every exe that speaks the protocol carries on disk, and the reader that finds it without
//! running the exe. The record sits in a linker section of its own (`.mfproto`), and `MessageChannel::send_version`
//! reads the number out of it, so any exe that can send the version keeps the section (a linker drops an
//! unreferenced one on ELF) and the number on disk is the number on the wire.
//!
//! `host_protocol` reads the section table with a few small reads and never maps the file: a mapped exe can make a
//! rebuild's linker fail to truncate it. PE and ELF64 (little-endian) only. A Mach-O host would need its section
//! named `"__DATA,__mfproto"`; there is no macOS target.

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read(bytes: &[u8]) -> Option<u32> {
        stamp_in(&mut Cursor::new(bytes))
    }

    fn stamp_bytes(version: u32, check: u32) -> Vec<u8> {
        [TAG.as_slice(), &version.to_le_bytes(), &check.to_le_bytes()].concat()
    }

    /// A minimal PE image: a DOS header pointing at a COFF header with no optional header, then one 40-byte section
    /// header per section, then the sections' bytes in order.
    fn pe(sections: &[(&[u8; 8], &[u8])]) -> Vec<u8> {
        let mut image = vec![0u8; 0x40];
        image[..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        image.extend_from_slice(b"PE\0\0");
        image.extend_from_slice(&0x8664u16.to_le_bytes()); // Machine: x64
        image.extend_from_slice(&(sections.len() as u16).to_le_bytes());
        image.extend_from_slice(&[0; 12]); // TimeDateStamp, PointerToSymbolTable, NumberOfSymbols
        image.extend_from_slice(&0u16.to_le_bytes()); // SizeOfOptionalHeader
        image.extend_from_slice(&0u16.to_le_bytes()); // Characteristics
        let mut data_at = image.len() + 40 * sections.len();
        for (name, data) in sections {
            let mut header = [0u8; 40];
            header[..8].copy_from_slice(*name);
            header[16..20].copy_from_slice(&(data.len() as u32).to_le_bytes()); // SizeOfRawData
            header[20..24].copy_from_slice(&(data_at as u32).to_le_bytes()); // PointerToRawData
            image.extend_from_slice(&header);
            data_at += data.len();
        }
        for (_, data) in sections {
            image.extend_from_slice(data);
        }
        image
    }

    /// A minimal ELF64 little-endian image: the 64-byte header, the sections' bytes, the section-name table, then the
    /// section header table (a null header, one per section, then `.shstrtab`'s).
    fn elf(sections: &[(&str, &[u8])]) -> Vec<u8> {
        fn section_header(name: u32, offset: usize, size: usize) -> [u8; 64] {
            let mut header = [0u8; 64];
            header[..4].copy_from_slice(&name.to_le_bytes());
            header[24..32].copy_from_slice(&(offset as u64).to_le_bytes());
            header[32..40].copy_from_slice(&(size as u64).to_le_bytes());
            header
        }
        let mut image = vec![0u8; 64];
        image[..6].copy_from_slice(b"\x7fELF\x02\x01");
        let mut headers = vec![[0u8; 64]];
        let mut names = vec![0u8];
        for (name, data) in sections {
            headers.push(section_header(names.len() as u32, image.len(), data.len()));
            names.extend_from_slice(name.as_bytes());
            names.push(0);
            image.extend_from_slice(data);
        }
        let names_at = names.len() as u32;
        names.extend_from_slice(b".shstrtab\0");
        headers.push(section_header(names_at, image.len(), names.len()));
        image.extend_from_slice(&names);
        let table_at = image.len() as u64;
        image[0x28..0x30].copy_from_slice(&table_at.to_le_bytes()); // e_shoff
        image[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
        image[0x3C..0x3E].copy_from_slice(&(headers.len() as u16).to_le_bytes()); // e_shnum
        image[0x3E..0x40].copy_from_slice(&(headers.len() as u16 - 1).to_le_bytes()); // e_shstrndx
        for header in &headers {
            image.extend_from_slice(header);
        }
        image
    }

    fn images(record: &[u8]) -> [Vec<u8>; 2] {
        [
            pe(&[(b".text\0\0\0", &b"code"[..]), (&SECTION, record)]),
            elf(&[(".text", &b"code"[..]), (".mfproto", record)]),
        ]
    }

    /// The test exe links `send_version`, so it carries the section as every host does, with this build's number, and
    /// the number `send_version` puts on the wire is the one on disk. On ELF this also proves the section is kept by
    /// `send_version`'s read: a linker drops an unreferenced one there (CI's Linux job and Task 1 Step 5's WSL run).
    #[test]
    fn this_test_exe_carries_its_protocol_number() {
        let mut channel = crate::MessageChannel::new(Cursor::new(Vec::<u8>::new()));
        channel.send_version().unwrap();
        let wire = channel.into_inner().into_inner();
        assert_eq!(wire, crate::PROTOCOL_VERSION.to_le_bytes(), "the wire carries this build's number");
        assert_eq!(host_protocol(&std::env::current_exe().unwrap()), Some(crate::PROTOCOL_VERSION), "so does the exe");
    }

    #[test]
    fn a_stamped_pe_or_elf_image_reads_its_number() {
        for image in images(&stamp_bytes(7, !7)) {
            assert_eq!(read(&image), Some(7));
        }
    }

    #[test]
    fn a_bad_record_or_no_section_reads_none() {
        for image in images(&stamp_bytes(7, 7)) {
            assert_eq!(read(&image), None, "the check field disagrees");
        }
        let mut wrong_tag = stamp_bytes(7, !7);
        wrong_tag[0] ^= 1;
        for image in images(&wrong_tag) {
            assert_eq!(read(&image), None, "the tag disagrees");
        }
        let record = stamp_bytes(7, !7);
        assert_eq!(read(&pe(&[(b".data\0\0\0", record.as_slice())])), None, "no .mfproto section");
        assert_eq!(read(&elf(&[(".mfprotox", record.as_slice())])), None, "a longer name is another section");
        assert_eq!(read(&elf(&[(".mfprot", record.as_slice())])), None, "a shorter name is another section");
    }

    /// Every cut of a stamped image, every single byte set to 0xFF, and random bytes after each format's magic read as
    /// no number or the right one, never a panic, a huge allocation or another number.
    #[test]
    fn truncated_or_garbage_files_read_none() {
        for image in images(&stamp_bytes(7, !7)) {
            for len in 0..image.len() {
                assert_eq!(read(&image[..len]), None, "cut at {len}");
            }
            for at in 0..image.len() {
                let mut bad = image.clone();
                bad[at] = 0xFF;
                assert!(matches!(read(&bad), None | Some(7)), "0xFF at {at}");
            }
        }
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        for head in [&b"MZ"[..], &b"\x7fELF\x02\x01"[..]] {
            for _ in 0..200 {
                let mut bytes = head.to_vec();
                bytes.extend((0..512).map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                }));
                assert_eq!(read(&bytes), None);
            }
        }
    }

    #[test]
    fn a_missing_file_or_a_folder_reads_none() {
        assert_eq!(host_protocol(&std::env::temp_dir()), None, "a folder");
        let gone = std::env::temp_dir().join(format!("no-such-host-{}.exe", std::process::id()));
        assert_eq!(host_protocol(&gone), None, "no such file");
    }
}
```

In `lib.rs`, after `pub mod render;`, add `mod stamp;`.

- [ ] **Step 3: Run the tests to see them fail**

Run: `cargo test -j 4 -p manifold-view-protocol stamp`
Expected: a compile error naming `stamp_in`, `host_protocol`, `TAG` and `SECTION` as not found.

- [ ] **Step 4: Write the record and the reader**

In `stamp.rs`, between the module doc and `#[cfg(test)]`, add:

```rust
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Starts the record, so a section of the right name holding something else is not misread.
const TAG: [u8; 12] = *b"manifold-ver";
/// The section's name: 8 bytes, the most a PE section name holds.
const SECTION: [u8; 8] = *b".mfproto";
/// The record on disk: the tag, the version and its check (u32 LE each).
const STAMP_LEN: usize = 20;
/// Bounds on what a corrupt header can make the reader allocate: section headers, and bytes of ELF section names.
const MAX_SECTIONS: u64 = 1024;
const MAX_NAMES: u64 = 64 * 1024;

/// What an exe carries: `TAG`, then `version`, then `!version`, so a torn or stray record reads as none.
#[repr(C)]
pub(crate) struct Stamp {
    #[allow(dead_code, reason = "read from the file by `host_protocol`, never through the struct")]
    tag: [u8; 12],
    pub(crate) version: u32,
    #[allow(dead_code, reason = "read from the file by `host_protocol`, never through the struct")]
    check: u32,
}

const _: () = assert!(std::mem::size_of::<Stamp>() == STAMP_LEN);

/// This exe's record. `MessageChannel::send_version` reads it, which keeps it in the exe.
#[link_section = ".mfproto"]
pub(crate) static STAMP: Stamp = Stamp { tag: TAG, version: crate::PROTOCOL_VERSION, check: !crate::PROTOCOL_VERSION };

/// The protocol number the exe at `path` carries, or `None` when it carries none: no such file, a folder, not a PE
/// or ELF64 image, no `.mfproto` section, or a record whose tag or check disagrees. Never panics; reads a few small
/// pieces of the file.
pub fn host_protocol(path: &Path) -> Option<u32> {
    stamp_in(&mut File::open(path).ok()?)
}

fn stamp_in(image: &mut (impl Read + Seek)) -> Option<u32> {
    let head = read_at(image, 0, 64)?;
    let at = if head.starts_with(b"MZ") {
        pe_section(image, &head)?
    } else if head.starts_with(b"\x7fELF") {
        elf_section(image, &head)?
    } else {
        return None;
    };
    parse_stamp(&read_at(image, at, STAMP_LEN as u64)?)
}

fn read_at(image: &mut (impl Read + Seek), offset: u64, len: u64) -> Option<Vec<u8>> {
    let mut bytes = vec![0; usize::try_from(len).ok()?];
    image.seek(SeekFrom::Start(offset)).ok()?;
    image.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..)?.get(..2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..)?.get(..4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..)?.get(..8)?.try_into().ok()?))
}

/// The file offset of the `.mfproto` section of a PE image whose DOS header is `head`. The section table follows the
/// COFF header and the optional header, whose size the COFF header gives; its magic is never read.
fn pe_section(image: &mut (impl Read + Seek), head: &[u8]) -> Option<u64> {
    let pe = u64::from(u32_at(head, 0x3C)?);
    let coff = read_at(image, pe, 24)?;
    if coff.get(..4)? != b"PE\0\0".as_slice() {
        return None;
    }
    let count = u64::from(u16_at(&coff, 6)?);
    let optional = u64::from(u16_at(&coff, 20)?);
    if count > MAX_SECTIONS {
        return None;
    }
    let table = read_at(image, pe + 24 + optional, count * 40)?;
    let header = table.chunks_exact(40).find(|h| h[..8] == SECTION)?;
    u32_at(header, 20).map(u64::from)
}

/// The file offset of the `.mfproto` section of a 64-bit little-endian ELF image whose header is `head`, found by
/// name through the section-name table.
fn elf_section(image: &mut (impl Read + Seek), head: &[u8]) -> Option<u64> {
    if head.get(4) != Some(&2) || head.get(5) != Some(&1) {
        return None;
    }
    let table_at = u64_at(head, 0x28)?;
    if u16_at(head, 0x3A)? != 64 {
        return None;
    }
    let count = u64::from(u16_at(head, 0x3C)?);
    let names_index = u64::from(u16_at(head, 0x3E)?);
    if count > MAX_SECTIONS || names_index >= count {
        return None;
    }
    let table = read_at(image, table_at, count * 64)?;
    let names_header = table.chunks_exact(64).nth(usize::try_from(names_index).ok()?)?;
    let names_size = u64_at(names_header, 32)?;
    if names_size > MAX_NAMES {
        return None;
    }
    let names = read_at(image, u64_at(names_header, 24)?, names_size)?;
    let header = table.chunks_exact(64).find(|h| {
        u32_at(h, 0)
            .and_then(|at| names.get(usize::try_from(at).ok()?..))
            .is_some_and(|name| name.starts_with(&SECTION) && name.get(8) == Some(&0))
    })?;
    u64_at(header, 24)
}

/// The version in a record whose tag and check agree.
fn parse_stamp(record: &[u8]) -> Option<u32> {
    let version = u32_at(record, 12)?;
    (record.get(..12)? == TAG.as_slice() && u32_at(record, 16)? == !version).then_some(version)
}
```

In `lib.rs`, before `pub use transport::{LocalTransport, RemoteTransport};`, add `pub use stamp::host_protocol;`.

In `channel.rs`, replace:

```rust
    /// Writes `PROTOCOL_VERSION`; the first thing each side sends.
    pub fn send_version(&mut self) -> io::Result<()> {
        self.stream.write_all(&crate::PROTOCOL_VERSION.to_le_bytes())?;
        self.stream.flush()
    }
```

with:

```rust
    /// Writes this exe's protocol number, read from the record it carries on disk (`stamp`), so the number on the wire
    /// is the one `host_protocol` reads; the first thing each side sends.
    pub fn send_version(&mut self) -> io::Result<()> {
        let version = std::hint::black_box(&crate::stamp::STAMP).version;
        self.stream.write_all(&version.to_le_bytes())?;
        self.stream.flush()
    }
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `cargo test -j 4 -p manifold-view-protocol`
Expected: `test result: ok.` for every test binary, with the five `stamp::tests::` tests among them.

If `this_test_exe_carries_its_protocol_number` fails while the four synthetic tests pass, the linker kept the section but put something before the record, or dropped the section. Stop and report it with the test's output; do not loosen the parser.

On Windows this test cannot prove that `send_version`'s read is what keeps the section: MSVC keeps an unreferenced one too (D1's spike). The ELF linker drops it, so run the same tests on Linux, in WSL (`target-linux` is warm; use a 600000 ms timeout):

```bash
wsl.exe -d Ubuntu -- bash -lc 'cd "/mnt/c/Users/Winston Li/Documents/GitHub/manifold" && CARGO_TARGET_DIR=target-linux cargo test -j 4 -p manifold-view-protocol stamp'
```

Expected: the five `stamp::tests::` tests pass. CI's Linux job runs them again on every push, which keeps proving it.

Then run the clippy line. Expected: no warning.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold-view-protocol/src/stamp.rs crates/manifold-view-protocol/src/lib.rs crates/manifold-view-protocol/src/channel.rs
git commit -F - <<'EOF'
protocol: every exe carries its protocol number in a .mfproto section

send_version reads the number out of the record, so an exe that can speak
the protocol keeps it, and host_protocol reads it through the PE or ELF64
section table without running or mapping the exe.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 2: `Skew`, and a handshake mismatch is a "can't start"

**Files:**
- Modify: `crates/manifold-view-protocol/src/lib.rs`:
  - delete `PROTOCOL_MISMATCH` (:36-38) and `check_version` (:47-56);
  - widen the `HOST_FATAL` doc (:40-44);
  - add `Skew` after `HOST_FATAL`;
  - replace the test `a_version_mismatch_names_both_versions` (:111-117).
- Modify: `crates/manifold-view-protocol/src/transport.rs`:
  - add `skew_with_app` after `pub const VIEW_INBOX: usize = 256;` (:30);
  - the doc line `/// Reads the host's `PROTOCOL_VERSION` first; a mismatch is a `Shutdown`.` (:41);
  - the version match in `spawn_host_reader` (:48-58);
  - a test after `a_wrong_version_from_the_host_becomes_shutdown` (:430-443).
- Modify: `crates/manifold/src/process_manager.rs`: the `spawn_reader_thread` doc (:863-865) and its version match (:881-893).
- Modify: `crates/manifold/src/process_manager/tests.rs:274-283` (`a_stale_view_host_is_a_protocol_mismatch_crash`).
- Modify: `crates/manifold/src/tabs.rs`: the `AfterCrash` docs (:292-296), `after_crash` (:309), the `leave_failed_card` doc (:663-666).
- Modify: `crates/manifold/src/tabs/tests.rs:23-24`.
- Modify: `crates/manifold/src/event_loop/view_message.rs:96-99` (comment).

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces (re-exported at `manifold_view_protocol::`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skew { host: Option<u32>, app: u32 }
impl Skew {
    pub fn between(host: Option<u32>, app: u32) -> Option<Skew>; // None when host == Some(app)
    pub fn host(self) -> Option<u32>;
    pub fn app(self) -> u32;
}
impl std::fmt::Display for Skew; // the one reason text (P4)
```
- Deletes: `manifold_view_protocol::PROTOCOL_MISMATCH`, `manifold_view_protocol::check_version`.

- [ ] **Step 1: Write the failing protocol tests**

In `lib.rs`'s test module, replace the whole test `a_version_mismatch_names_both_versions` with:

```rust
    #[test]
    fn a_skew_names_which_side_to_rebuild() {
        assert_eq!(Skew::between(Some(5), 5), None);
        let older = Skew::between(Some(4), 5).unwrap();
        assert_eq!((older.host(), older.app()), (Some(4), 5));
        assert_eq!(older.to_string(), "the engine host speaks protocol 4, manifold speaks 5; rebuild the engine host");
        assert_eq!(
            Skew::between(Some(6), 5).unwrap().to_string(),
            "the engine host speaks protocol 6, manifold speaks 5; rebuild manifold"
        );
        assert_eq!(
            Skew::between(None, 5).unwrap().to_string(),
            "the engine host carries no protocol number (built before hosts were stamped); rebuild the engine host"
        );
    }
```

In `transport.rs`'s first test module, after the test `a_wrong_version_from_the_host_becomes_shutdown`, add:

```rust
    /// The host speaks `PROTOCOL_VERSION`; manifold's number decides which side the host's log line says to rebuild.
    #[test]
    fn the_host_names_which_side_to_rebuild() {
        let v = crate::PROTOCOL_VERSION;
        assert_eq!(skew_with_app(v), None);
        assert!(skew_with_app(v + 1).unwrap().to_string().ends_with("; rebuild the engine host"));
        assert!(skew_with_app(v - 1).unwrap().to_string().ends_with("; rebuild manifold"));
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 4 -p manifold-view-protocol`
Expected: a compile error: cannot find type `Skew`, cannot find function `skew_with_app`.

- [ ] **Step 3: Write `Skew` and use it on the host side**

In `lib.rs`, extend the `HOST_FATAL` doc. After its last line (`/// reason, and manifold leaves a failed card naming the rest instead of respawning (`tabs::after_crash`).`), add:

```rust
/// The app's reader also makes one, with the `Skew` as the rest, for a host that speaks another protocol
/// (`process_manager::spawn_reader_thread`).
```

After `pub const HOST_FATAL: &str = "view-host fatal: ";`, add:

```rust
/// Which protocol a host speaks against the one manifold speaks, when they differ. Built only by `between`, which names
/// both sides, so no caller can swap them. Its `Display` is the one reason text, and it names the side to rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skew {
    host: Option<u32>,
    app: u32,
}

impl Skew {
    /// `None` when the host speaks `app`. `host` is `None` for a host that carries no number (`host_protocol`).
    pub fn between(host: Option<u32>, app: u32) -> Option<Skew> {
        (host != Some(app)).then_some(Skew { host, app })
    }

    pub fn host(self) -> Option<u32> {
        self.host
    }

    pub fn app(self) -> u32 {
        self.app
    }
}

impl std::fmt::Display for Skew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.host {
            None => f.write_str(
                "the engine host carries no protocol number (built before hosts were stamped); rebuild the engine host",
            ),
            Some(host) => {
                let rebuild = if host < self.app { "the engine host" } else { "manifold" };
                write!(f, "the engine host speaks protocol {host}, manifold speaks {}; rebuild {rebuild}", self.app)
            }
        }
    }
}
```

In `transport.rs`, after `pub const VIEW_INBOX: usize = 256;`, add:

```rust

/// What this engine host (speaking `PROTOCOL_VERSION`) makes of manifold's number `app`: `None` when they agree,
/// else the `Skew` naming which side to rebuild.
fn skew_with_app(app: u32) -> Option<crate::Skew> {
    crate::Skew::between(Some(crate::PROTOCOL_VERSION), app)
}
```

In `spawn_host_reader`'s doc, replace `/// Reads the host's `PROTOCOL_VERSION` first; a mismatch is a `Shutdown`.` with `/// Reads manifold's `PROTOCOL_VERSION` first; a mismatch is logged naming which side to rebuild (`Skew`) and is a `Shutdown`.`

In its body, replace:

```rust
        match from_host.recv_version().map(crate::check_version) {
            Ok(Ok(())) => {}
            Ok(Err(mismatch)) => {
                eprintln!("{mismatch}");
                let _ = tx.send(HostToView::Shutdown);
                return;
            }
```

with:

```rust
        match from_host.recv_version().map(skew_with_app) {
            Ok(None) => {}
            Ok(Some(skew)) => {
                eprintln!("{skew}");
                let _ = tx.send(HostToView::Shutdown);
                return;
            }
```

(The `Err(_) => { ... }` arm after it stays.) `check_version` stays until Step 7, which rewrites the app's reader (its last caller) and deletes it.

- [ ] **Step 4: Run the protocol tests to see them pass**

Run: `cargo test -j 4 -p manifold-view-protocol`
Expected: `test result: ok.` for every binary.

- [ ] **Step 5: Write the failing app tests**

In `crates/manifold/src/process_manager/tests.rs`, replace the whole test `a_stale_view_host_is_a_protocol_mismatch_crash` with:

```rust
/// A host speaking another protocol, older or newer, becomes a "can't start" (`HOST_FATAL`) naming which side to
/// rebuild, so the tab is a failed card with that reason and is never respawned.
#[test]
fn a_host_speaking_another_protocol_cannot_start_and_says_which_side_to_rebuild() {
    use manifold_view_protocol::{Skew, HOST_FATAL, PROTOCOL_VERSION as V};
    assert!(matches!(first_event_after_version(V), ViewToHost::Cookies(_)));
    for host in [V - 1, V + 1] {
        let skew = Skew::between(Some(host), V).unwrap();
        match first_event_after_version(host) {
            ViewToHost::Crashed { reason } => assert_eq!(reason, format!("{HOST_FATAL}{skew}")),
            other => panic!("expected a Crashed naming the skew, got {other:?}"),
        }
    }
    assert!(Skew::between(Some(V + 1), V).unwrap().to_string().ends_with("; rebuild manifold"));
}
```

In `crates/manifold/src/tabs/tests.rs`, in `a_host_that_cannot_start_is_not_restarted_and_says_why`, replace:

```rust
    let mismatch = format!("{}: host 2, engine 3", manifold_view_protocol::PROTOCOL_MISMATCH);
    assert_eq!(after_crash(&mismatch, &mut times, t0), AfterCrash::GiveUp);
```

with:

```rust
    let skew = manifold_view_protocol::Skew::between(
        Some(manifold_view_protocol::PROTOCOL_VERSION + 1),
        manifold_view_protocol::PROTOCOL_VERSION,
    )
    .unwrap()
    .to_string();
    let mismatch = format!("{}{skew}", manifold_view_protocol::HOST_FATAL);
    assert_eq!(after_crash(&mismatch, &mut times, t0), AfterCrash::CannotStart(skew.as_str()));
    assert!(times.is_empty(), "a mismatch is not counted as a crash");
```

(This one already passes: a `HOST_FATAL` reason is `CannotStart` today. It pins the reason the reader will send.)

- [ ] **Step 6: Run them to see the first fail**

Run: `cargo test -j 4 -p manifold --bin manifold a_host_`
Expected: `a_host_speaking_another_protocol_cannot_start_and_says_which_side_to_rebuild` FAILS on its `assert_eq!`: the left side starts "protocol mismatch: the engine host speaks protocol", the right side "view-host fatal: the engine host speaks protocol". `a_host_that_cannot_start_is_not_restarted_and_says_why` passes.

- [ ] **Step 7: Make the app's reader send a "can't start", and delete the special case**

In `process_manager.rs`, in `spawn_reader_thread`'s doc, replace:

```rust
/// Reads the engine's `PROTOCOL_VERSION` first. A stale host (built from an
/// older `messages.rs`) gets a `Crashed` whose reason starts with
/// `PROTOCOL_MISMATCH`, so no message of it is ever misread.
```

with:

```rust
/// Reads the engine's `PROTOCOL_VERSION` first. A host speaking another protocol gets a `Crashed` whose reason is
/// `HOST_FATAL` plus the `Skew`, so no message of it is ever misread and the tab becomes a failed card saying which
/// side to rebuild (`tabs::after_crash`). The app refuses such a host before spawning it when it can read its stamp
/// (`OnDisk::skew`); this is the backstop for one it cannot.
```

In its body, replace:

```rust
        match from_view.recv_version().map(manifold_view_protocol::check_version) {
            Ok(Ok(())) => {}
            Ok(Err(mismatch)) => {
                eprintln!("{mismatch}");
                let _ = tx.send(ViewToHost::Crashed { reason: mismatch });
                wake();
                return;
            }
            Err(_) => {
                let _ = tx.send(lost());
                wake();
                return;
            }
        }
```

with:

```rust
        let skew = match from_view.recv_version() {
            Ok(version) => {
                manifold_view_protocol::Skew::between(Some(version), manifold_view_protocol::PROTOCOL_VERSION)
            }
            Err(_) => {
                let _ = tx.send(lost());
                wake();
                return;
            }
        };
        if let Some(skew) = skew {
            eprintln!("{skew}");
            let reason = format!("{}{skew}", manifold_view_protocol::HOST_FATAL);
            let _ = tx.send(ViewToHost::Crashed { reason });
            wake();
            return;
        }
```

(`OnDisk::skew` is added in Task 5; until then the doc names a field that does not exist yet, which rustdoc does not check in a private item.)

In `tabs.rs`:
- Replace the `GiveUp` doc `/// Leave a failed card: the host speaks another protocol version, or keeps crashing (`crashes_too_often`).` with `/// Leave a failed card: the host keeps crashing (`crashes_too_often`).`
- Replace the `CannotStart` doc:

```rust
    /// Leave a failed card naming why: the host could not start at all (its reason began with
    /// `manifold_view_protocol::HOST_FATAL`), so a respawn would fail the same way.
```

with:

```rust
    /// Leave a failed card naming why: the host could not start at all, or speaks another protocol (its reason began
    /// with `manifold_view_protocol::HOST_FATAL`), so a respawn would fail the same way.
```

- In `after_crash`, replace `    if reason.starts_with(manifold_view_protocol::PROTOCOL_MISMATCH) || crashes_too_often(times, now) {` with `    if crashes_too_often(times, now) {`.
- Replace the `leave_failed_card` doc's first two lines:

```rust
    /// A tab whose engine keeps crashing (`crashes_too_often`), or speaks
    /// another protocol version, becomes a failed card: the reason is its
```

with:

```rust
    /// A tab whose engine keeps crashing (`crashes_too_often`), or whose host
    /// cannot start (it speaks another protocol, say), becomes a failed card: the reason is its
```

In `event_loop/view_message.rs`, replace the comment:

```rust
                // A stale engine binary (protocol mismatch) or a host that cannot
                // start would fail the same way again, and a page or install that
                // crashes on every start would loop forever; each is left as a
                // failed card instead.
```

with:

```rust
                // A host that cannot start (or speaks another protocol) would fail
                // the same way again, and a page or install that crashes on every
                // start would loop forever; each is left as a failed card instead.
```

In `lib.rs`, delete `PROTOCOL_MISMATCH` with its two doc lines (`/// Starts the `ViewToHost::Crashed` reason for a version mismatch, so the` / `/// host shows a failed card instead of respawning the same stale binary.`), and delete `check_version` with its doc line (`/// `Ok` when the peer speaks `PROTOCOL_VERSION`.`).

- [ ] **Step 8: Run everything**

Run, in order:
- `cargo test -j 4 -p manifold-view-protocol`
- `cargo test -j 4 -p manifold --bin manifold`
- `cargo test -j 4 -p manifold-view-webview2`
- the clippy line
- `grep -rn "PROTOCOL_MISMATCH\|check_version" crates`

Expected: every `test result: ok.`; no warning; the grep prints nothing.

- [ ] **Step 9: CEF and WebKit compile the new host-side reader**

- Create the CEF guard (see "CEF guard"), then run `powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd check -Logs "<scratchpad>"`. Expected: `cargo check exit 0`, exit 0.
- Run the WebKit check. Expected: it builds, and its tests pass.
- Report an exit 2 or 3 from the guard as such and go on; a compile error is fixed in this task.

- [ ] **Step 10: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold-view-protocol/src/lib.rs crates/manifold-view-protocol/src/transport.rs crates/manifold/src/process_manager.rs crates/manifold/src/process_manager/tests.rs crates/manifold/src/tabs.rs crates/manifold/src/tabs/tests.rs crates/manifold/src/event_loop/view_message.rs
git commit -F - <<'EOF'
protocol: Skew names which side to rebuild; a mismatch is a can't-start

Skew::between(host, app) replaces check_version and PROTOCOL_MISMATCH. The
app's reader turns a mismatch into Crashed { HOST_FATAL + skew }, so the tab
is a failed card saying what to rebuild, and after_crash loses its special
case. The host's log line no longer swaps the two numbers.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 3: The PDF renderer's mismatch reaches the card

Needs P22's answer. If its `Answer:` line reads "pending", stop and ask. If the owner chose D2's wording, use this task with one change: `read_frame` keeps today's text, and the `if mismatch` arm in `run_child` returns `Err(format!("{reason}; rebuild manifold-view-pdfium"))`; both expected strings in Step 1 then end in "; rebuild manifold-view-pdfium".

**Files:**
- Modify: `crates/manifold-view-protocol/src/render.rs`:
  - a `RENDER_MISMATCH` constant after `const MAGIC: [u8; 4] = *b"MRND";` (:12);
  - `read_frame`'s version check (:46-51);
  - `run_child`'s doc (:108-114) and its kill and result match (:141-171);
  - a test in its test module.
- Modify: `crates/manifold-view-protocol/src/bin/render_stub.rs` (the `mismatch` and `mismatch-linger` modes).
- Test: `crates/manifold-view-protocol/tests/render_child.rs`.

**Interfaces:**
- Consumes: nothing.
- Produces: `read_frame`'s mismatch reason, `"render protocol mismatch: the view speaks {v}, manifold speaks {RENDER_PROTOCOL_VERSION}; rebuild {manifold-view-pdfium|manifold}"`, which `run_child` returns whatever the child's exit says.

- [ ] **Step 1: Write the failing tests**

In `render.rs`'s test module, after `malformed_frames_are_named`, add:

```rust
    /// A frame of another render protocol says which side to rebuild: the renderer when it is older, manifold when it
    /// is newer.
    #[test]
    fn a_render_mismatch_names_which_side_to_rebuild() {
        let v = RENDER_PROTOCOL_VERSION;
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame()).unwrap();
        let with_version = |version: u32| {
            let mut other = bytes.clone();
            other[4..8].copy_from_slice(&version.to_le_bytes());
            read_frame(&mut other.as_slice()).unwrap_err()
        };
        assert_eq!(
            with_version(v + 1),
            format!("render protocol mismatch: the view speaks {}, manifold speaks {v}; rebuild manifold", v + 1)
        );
        assert_eq!(
            with_version(v - 1),
            format!(
                "render protocol mismatch: the view speaks {}, manifold speaks {v}; rebuild manifold-view-pdfium",
                v - 1
            )
        );
    }
```

In `render_stub.rs`, replace `use std::io::Write;` with:

```rust
use manifold_view_protocol::render::{HEADER_LEN, RENDER_PROTOCOL_VERSION};
use std::io::Write;
```

and add this arm before `other => panic!("unknown mode {other}"),`:

```rust
        // A frame of an older render protocol. "mismatch" makes it larger than a pipe buffer: the app stops reading
        // after the header, so the rest can't be written, and this exits 1 the way pdfium does ("couldn't write the
        // frame"). "mismatch-linger" writes the header alone and keeps running, so only a kill ends it soon; it gives
        // up after 20 s, so a run_child that waits for it fails a test instead of hanging it.
        "mismatch" | "mismatch-linger" => {
            let mut bytes = b"MRND".to_vec();
            for field in [RENDER_PROTOCOL_VERSION - 1, 1024, 1024, 1] {
                bytes.extend_from_slice(&field.to_le_bytes());
            }
            if mode == "mismatch" {
                bytes.resize(HEADER_LEN + 4 * 1024 * 1024, 0x7f);
            }
            let mut out = std::io::stdout().lock();
            if let Err(e) = out.write_all(&bytes).and_then(|()| out.flush()) {
                eprintln!("couldn't write the frame: {e}");
                std::process::exit(1);
            }
            if mode == "mismatch-linger" {
                std::thread::sleep(std::time::Duration::from_secs(20));
            }
        }
```

In `tests/render_child.rs`, after `garbage_and_short_output_are_a_bad_frame`, add:

```rust
/// A renderer of another render protocol is named with what to rebuild, even though it exits 1 on the broken pipe
/// once the app stops reading.
#[test]
fn a_render_protocol_mismatch_is_named_with_what_to_rebuild() {
    let v = manifold_view_protocol::render::RENDER_PROTOCOL_VERSION;
    assert_eq!(
        run("mismatch", LONG).unwrap_err(),
        format!("render protocol mismatch: the view speaks {}, manifold speaks {v}; rebuild manifold-view-pdfium", v - 1)
    );
}

/// A renderer of another render protocol that keeps running after its header is killed, not waited for: the
/// mismatch is the reason, whatever it would do next.
#[test]
fn a_mismatched_renderer_that_lingers_is_killed() {
    let v = manifold_view_protocol::render::RENDER_PROTOCOL_VERSION;
    let start = Instant::now();
    assert_eq!(
        run("mismatch-linger", LONG).unwrap_err(),
        format!("render protocol mismatch: the view speaks {}, manifold speaks {v}; rebuild manifold-view-pdfium", v - 1)
    );
    assert!(start.elapsed() < Duration::from_secs(10), "the child that kept running was killed, not waited for");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 4 -p manifold-view-protocol mismatch`
Expected:
- `a_render_mismatch_names_which_side_to_rebuild` FAILS: the left side ends "manifold speaks 1", with no "; rebuild".
- `a_render_protocol_mismatch_is_named_with_what_to_rebuild` FAILS: the left side is "couldn't write the frame: ..." (the stub's exit 1 wins today).
- `a_mismatched_renderer_that_lingers_is_killed` FAILS after about 20 s (today `run_child` waits for a child that wrote a bad frame): the left side ends "manifold speaks 1", with no "; rebuild".

- [ ] **Step 3: Name the side in `read_frame`, and let the mismatch win in `run_child`**

In `render.rs`, after `const MAGIC: [u8; 4] = *b"MRND";`, add:

```rust
/// Starts `read_frame`'s reason for a frame of another render protocol; `run_child` returns that reason whatever the
/// child's exit says.
const RENDER_MISMATCH: &str = "render protocol mismatch";
```

In `read_frame`, replace:

```rust
    if field(0) != RENDER_PROTOCOL_VERSION {
        return Err(format!(
            "render protocol mismatch: the view speaks {}, manifold speaks {RENDER_PROTOCOL_VERSION}",
            field(0)
        ));
    }
```

with:

```rust
    let version = field(0);
    if version != RENDER_PROTOCOL_VERSION {
        let rebuild = if version < RENDER_PROTOCOL_VERSION { "manifold-view-pdfium" } else { "manifold" };
        return Err(format!(
            "{RENDER_MISMATCH}: the view speaks {version}, manifold speaks {RENDER_PROTOCOL_VERSION}; rebuild {rebuild}"
        ));
    }
```

In `run_child`'s doc, replace `/// text's last line, any other failed exit is "crashed", and output that is not a valid frame is "returned a bad` and the next line `/// frame". A complete valid frame is the result whatever the child does next: it is killed rather than waited for.` with:

```rust
/// text's last line, any other failed exit is "crashed", a frame of another render protocol kills it and returns the
/// mismatch naming which side to rebuild (`read_frame`), and other output that is not a valid frame is "returned a
/// bad frame". A complete valid frame is the result whatever the child does next: it is killed rather than waited for.
```

Replace:

```rust
    // Only a bad frame leaves the child to finish by itself, so its exit status can say why; a timeout, a cancel and a
    // good frame all end it now.
    if !matches!(event, Ok(Event::Frame(Err(_)))) {
        let _ = child.kill();
    }
```

with:

```rust
    // A bad frame leaves the child to finish by itself, so its exit status can say why; a timeout, a cancel, a good
    // frame and a frame of another render protocol (the mismatch is the reason, whatever the exit says) end it now.
    let mismatch = matches!(&event, Ok(Event::Frame(Err(reason))) if reason.starts_with(RENDER_MISMATCH));
    if mismatch || !matches!(event, Ok(Event::Frame(Err(_)))) {
        let _ = child.kill();
    }
```

In the result match, after `        Ok(Event::Frame(Ok(frame))) => Ok(frame),`, add:

```rust
        // Killing it gave a failed exit, which says nothing: the mismatch is the reason.
        Ok(Event::Frame(Err(reason))) if mismatch => Err(reason),
```

and replace the last expression of the `Ok(Event::Frame(Err(reason)))` arm:

```rust
            Err(if reason.starts_with("render protocol mismatch") {
                reason
            } else {
                format!("{label} returned a bad frame ({reason})")
            })
```

with:

```rust
            Err(format!("{label} returned a bad frame ({reason})"))
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -j 4 -p manifold-view-protocol`
Expected: `test result: ok.` for the unit tests, the `render_child` tests (now ten) and the doc tests. `a_mismatched_renderer_that_lingers_is_killed` pins the kill: without it `run_child` waits out the stub's 20 s.

Then run the clippy line. Expected: no warning.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold-view-protocol/src/render.rs crates/manifold-view-protocol/src/bin/render_stub.rs crates/manifold-view-protocol/tests/render_child.rs
git commit -F - <<'EOF'
render: a renderer of another render protocol is named with what to rebuild

run_child kills it and returns read_frame's mismatch before the exit-code
branches, so pdfium's exit 1 on the broken pipe no longer hides the reason.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 4: One lookup finds each host in its dev folder, then beside `manifold.exe`

Needs P10's answer. If its `Answer:` line reads "pending", stop and ask. If the owner chose D4's wording (leave `sibling_host` alone), stop as well: Tasks 4 to 12 then need re-planning, because a dev Servo host in `target-servo` would never read Ready (P10).

**Files:**
- Modify: `crates/manifold/src/process_manager.rs`:
  - `ViewDesc`: a `dev_dir` field after `runtime_dir` (:70-73);
  - one `dev_dir` value in each of the four rows of `desc`;
  - the `#[cfg(test)] use availability::{...}` import (:1167-1169).
- Modify: `crates/manifold/src/process_manager/availability.rs`:
  - the `sibling_binary` doc (:6-8);
  - everything from `/// Where to find each view-host's executable.` (:15) to the end of `pick_webkit_host` (:109), keeping `view_hosts_dir` and `current_exe_dir` as they are.
- Test: `crates/manifold/src/process_manager/tests.rs`: replace `the_webkit_host_is_found_beside_the_exe_then_in_a_dev_tree_then_in_engine_hosts` (:543-567), and add `the_dev_folder_is_searched_four_parents_up_and_no_further` after it.

**Interfaces:**
- Consumes: nothing from Tasks 1-3.
- Produces (all in `availability.rs`):

```rust
pub(crate) dev_dir: Option<&'static str>,                                     // ViewDesc
pub(super) fn host_file_name(kind: ViewKind) -> String;
fn beside_exe(kind: ViewKind, exe_dir: &Path) -> PathBuf;
fn in_place_paths(kind: ViewKind, exe_dir: &Path) -> Vec<PathBuf>;
fn run_in_place(kind: ViewKind) -> Option<PathBuf>;                           // None under cfg(test)
pub(super) fn run_in_place_from(kind: ViewKind, exe_dir: &Path) -> Option<PathBuf>;
```
- Deletes: `built_webkit_host`.
- Unchanged signatures: `view_binary_path`, `cef_sibling_exe`, `sibling_host`, `pick_webkit_host`, `webkit_host_path`.

- [ ] **Step 1: Write the failing test**

In `process_manager/tests.rs`, replace the whole test `the_webkit_host_is_found_beside_the_exe_then_in_a_dev_tree_then_in_engine_hosts`, with its doc comment, by:

```rust
/// Where each host runs in place from, given `manifold.exe`'s folder (`run_in_place_from`): its dev build folder found
/// by walking up (`ViewDesc::dev_dir`), then beside the exe; CEF only from its `cef-runtime/` copy, the one a tab runs.
/// WebKit falls back to its downloaded copy in `engine-hosts/`, whether or not it is there yet.
#[test]
fn each_host_is_found_in_its_dev_folder_then_beside_the_exe() {
    let root = crate::store::testutil::temp_dir("host-lookup");
    let repo = root.join("repo");
    let exe_dir = repo.join("target").join("debug");
    std::fs::create_dir_all(&exe_dir).unwrap();
    for kind in ViewKind::ALL {
        assert_eq!(run_in_place_from(kind, &exe_dir), None, "{kind:?}: nothing built");
    }
    let hosts = root.join("engine-hosts");
    assert_eq!(pick_webkit_host(&exe_dir, Some(&hosts)), Some(hosts.join("manifold-view-webkitgtk")));
    assert_eq!(pick_webkit_host(&exe_dir, None), None);
    for (kind, dev) in [(ViewKind::Servo, "target-servo"), (ViewKind::Webkit, "target-linux")] {
        assert_eq!(kind.desc().dev_dir, Some(dev));
        let dev_dir = repo.join(dev).join("debug");
        std::fs::create_dir_all(&dev_dir).unwrap();
        assert_eq!(run_in_place_from(kind, &exe_dir), None, "{kind:?}: the folder alone is not a host");
        let flat = exe_dir.join(host_file_name(kind));
        std::fs::write(&flat, b"host").unwrap();
        assert_eq!(run_in_place_from(kind, &exe_dir), Some(flat), "{kind:?}: a packaged host beside the exe");
        let built = dev_dir.join(host_file_name(kind));
        std::fs::write(&built, b"host").unwrap();
        assert_eq!(run_in_place_from(kind, &exe_dir), Some(built), "{kind:?}: the dev build wins");
    }
    assert_eq!(
        pick_webkit_host(&exe_dir, Some(&hosts)),
        Some(repo.join("target-linux").join("debug").join("manifold-view-webkitgtk"))
    );
    // CEF runs only from its runtime folder: a flat exe alone is not in place, so its stamp is never read for a copy
    // the tab would not run.
    assert_eq!(ViewKind::Cef.desc().dev_dir, None);
    std::fs::write(exe_dir.join(host_file_name(ViewKind::Cef)), b"host").unwrap();
    assert_eq!(run_in_place_from(ViewKind::Cef, &exe_dir), None, "a flat CEF exe alone");
    let copy = exe_dir.join(CEF_RUNTIME_DIR).join(host_file_name(ViewKind::Cef));
    std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
    std::fs::write(&copy, b"host").unwrap();
    assert_eq!(run_in_place_from(ViewKind::Cef, &exe_dir), Some(copy));
    assert_eq!(ViewKind::Webview2.desc().dev_dir, None);
    let webview2 = exe_dir.join(host_file_name(ViewKind::Webview2));
    std::fs::write(&webview2, b"host").unwrap();
    assert_eq!(run_in_place_from(ViewKind::Webview2, &exe_dir), Some(webview2));
}

/// The dev folder is looked for in the exe's folder and its four parents (as WebKit's lookup walked): a host in
/// the fourth parent's dev folder is found, one in the fifth's is not.
#[test]
fn the_dev_folder_is_searched_four_parents_up_and_no_further() {
    let root = crate::store::testutil::temp_dir("host-walk");
    let exe_dir = root.join("p5").join("p4").join("p3").join("p2").join("p1").join("exe");
    std::fs::create_dir_all(&exe_dir).unwrap();
    let put = |parent: &Path| {
        let built = parent.join("target-servo").join("debug").join(host_file_name(ViewKind::Servo));
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"host").unwrap();
        built
    };
    put(&root.join("p5"));
    assert_eq!(run_in_place_from(ViewKind::Servo, &exe_dir), None, "five parents up is out of reach");
    let reached = put(&root.join("p5").join("p4"));
    assert_eq!(run_in_place_from(ViewKind::Servo, &exe_dir), Some(reached), "four parents up is reached");
}
```

In `process_manager.rs`, replace the test import:

```rust
#[cfg(test)]
use availability::{
    availability_with, built_webkit_host, on_disk_cached, pick_webkit_host, OnDiskCache, ON_DISK_CHECK_TTL,
};
```

with:

```rust
#[cfg(test)]
use availability::{
    availability_with, host_file_name, on_disk_cached, pick_webkit_host, run_in_place_from, OnDiskCache,
    ON_DISK_CHECK_TTL,
};
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -j 4 -p manifold --bin manifold each_host_is_found`
Expected: a compile error: no `host_file_name` or `run_in_place_from` in `availability`, and no field `dev_dir` on `ViewDesc`.

- [ ] **Step 3: Give each engine its dev folder**

In `ViewDesc`, after the `runtime_dir` field (its doc ends `/// than single pinned files.`), add:

```rust
    /// The folder a dev tree builds this host in, beside `target/` (`<dev_dir>/debug/<host>`), when that is not
    /// `target/` itself: Servo's `target-servo` (README "Building Servo on its own") and WebKit's `target-linux` (built
    /// inside WSL). manifold runs a host found there in place (`availability::run_in_place`).
    pub(crate) dev_dir: Option<&'static str>,
```

In `desc`, add one line after each row's `runtime_dir` line. Find each row by its own lines:
- Servo (`runtime_dir: None,` followed by `starting_default: true,` and `missing_system_libs: crate::view_install::missing_vc_runtime,`): `dev_dir: Some("target-servo"),`
- CEF (`runtime_dir: Some(CEF_RUNTIME_DIR),`): `dev_dir: None,`
- WebKit (`runtime_dir: None,` followed by `starting_default: false,` and `missing_system_libs: Vec::new,`): `dev_dir: Some("target-linux"),`
- WebView2 (`runtime_dir: None,` followed by `starting_default: true,` and `missing_system_libs: Vec::new,`): `dev_dir: None,`

- [ ] **Step 4: Write the one lookup**

In `availability.rs`, replace the `sibling_binary` doc:

```rust
/// The view-host binaries are sibling executables in the same
/// target/{debug,release} directory as manifold itself — standard
/// Cargo workspace layout, no separate install/discovery step needed.
```

with:

```rust
/// A file beside `manifold.exe` named as this platform names an exe: the PDF renderer (`pdf_view_exe`), and CEF's
/// flat host (`sibling_host`).
```

Then replace everything from `/// Where to find each view-host's executable.` through the closing `}` of `pick_webkit_host` with the code below. It keeps `view_hosts_dir` and `current_exe_dir` as they are, deletes `built_webkit_host`, and leaves `webkit_host_path` (after `pick_webkit_host`) untouched.

```rust
/// The file name of `kind`'s host: as this platform names an exe (`PLATFORM.host_exe`), except a host built for another
/// OS than this one, which keeps its bare name (WebKit's ELF, run in WSL from Windows).
pub(super) fn host_file_name(kind: ViewKind) -> String {
    let desc = kind.desc();
    if desc.host_os.is_some_and(|os| os != Os::here()) {
        desc.binary.to_owned()
    } else {
        crate::cef_platform::PLATFORM.host_exe(desc.binary)
    }
}

/// Where `kind`'s host goes beside `manifold.exe` in `exe_dir`, whether or not it is there: flat, except CEF's, which
/// lives in its runtime folder (`cef-runtime/`). CEF ships its own `libEGL.dll`/`libGLESv2.dll` (ANGLE), and Windows
/// resolves an exe's implicit DLL dependencies from the exe's own folder first, so a CEF host in one folder with
/// another ANGLE user would load whichever copy was put there last. The other hosts are a lone exe beside their own
/// files (WebView2's loader is statically linked by the `webview2-com` crate), so they need no folder of their own.
fn beside_exe(kind: ViewKind, exe_dir: &Path) -> PathBuf {
    let file = host_file_name(kind);
    match kind.desc().runtime_dir {
        Some(runtime) => exe_dir.join(runtime).join(file),
        None => exe_dir.join(file),
    }
}

/// Every place `kind`'s host can run in place from, given `manifold.exe`'s folder `exe_dir`, in the order they are
/// tried: its dev build folder (`ViewDesc::dev_dir`, then `debug/`), found by walking up at most as far as the repo
/// root of a worktree under `.claude/`, then beside `manifold.exe` (`beside_exe`). The dev folder comes first: it is
/// where a dev tree builds the host, and a stray copy left beside the exe must not shadow a fresh build.
fn in_place_paths(kind: ViewKind, exe_dir: &Path) -> Vec<PathBuf> {
    let file = host_file_name(kind);
    let mut paths: Vec<PathBuf> = match kind.desc().dev_dir {
        Some(dev) => exe_dir.ancestors().take(5).map(|dir| dir.join(dev).join("debug").join(&file)).collect(),
        None => Vec::new(),
    };
    paths.push(beside_exe(kind, exe_dir));
    paths
}

/// The host of `kind` that a spawn runs as it stands, when one is there (`in_place_paths`). It is never anything under
/// `view_hosts_dir()` (a download, hash-checked by its install, or the embedded WebView2 host), and for CEF only its
/// `cef-runtime/` copy, the one a tab runs. This is the exe whose protocol number `read_on_disk` checks. `None` under
/// `cfg(test)`, so no unit test reads a real host from disk (a dev tree's hosts are reachable from the test exe).
fn run_in_place(kind: ViewKind) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    run_in_place_from(kind, &current_exe_dir())
}

/// `run_in_place` for a `manifold.exe` in `exe_dir`.
pub(super) fn run_in_place_from(kind: ViewKind, exe_dir: &Path) -> Option<PathBuf> {
    in_place_paths(kind, exe_dir).into_iter().find(|path| path.is_file())
}

/// Where to find each view-host's executable: the host that runs in place (`run_in_place`), else the copy
/// `view_install` puts in `view_hosts_dir()` (under `profile::view_data_dir()`), whether or not it is there yet: flat
/// for a downloaded host, in a folder of its own for the embedded one. (A downloaded CEF host is never run from there:
/// `prepare_cef` copies it beside the runtime's `libcef.dll`.) Without a data directory, the place beside
/// `manifold.exe` (`beside_exe`).
///
/// TODO: a dev tree's runtime files are put beside its hosts by `gate sync-runtime-deps` (README "Runtime-file
/// placement"), not by cargo; a real install or packaging step should do that.
pub(super) fn view_binary_path(kind: ViewKind) -> PathBuf {
    let desc = kind.desc();
    assert!(desc.link == HostLink::PipesShm, "{} is reached over TCP, not by a local path", desc.label);
    if let Some(exe) = run_in_place(kind) {
        return exe;
    }
    let Some(mut fallback) = view_hosts_dir() else { return beside_exe(kind, &current_exe_dir()) };
    if let HostSource::Embedded(host) = desc.source {
        // Where `spawn_tab` extracts the embedded host (`Embedded::ensure`), in a folder of its own.
        return host.host_path(&fallback);
    }
    fallback.push(host_file_name(kind));
    fallback
}

pub(crate) fn view_hosts_dir() -> Option<PathBuf> {
    crate::profile::view_data_dir().map(|mut dir| {
        dir.push("engine-hosts");
        dir
    })
}

/// `cef-runtime/<exe>` next to `manifold.exe`: where `gate sync-runtime-deps` puts it for dev builds.
pub(super) fn cef_sibling_exe() -> PathBuf {
    beside_exe(ViewKind::Cef, &current_exe_dir())
}

/// The host exe of `kind` that ships in place (a dev tree's, or a packaged one's), when it is there: the one that runs
/// in place (`run_in_place`), except CEF's, which is its flat exe beside `manifold.exe` first, then its `cef-runtime/`
/// copy (`shipped_cef_host` copies the flat one into a runtime that has no host). A host in place makes the host part
/// Ready whatever the pin table says.
pub(super) fn sibling_host(kind: ViewKind) -> Option<PathBuf> {
    if kind.desc().runtime_dir.is_none() {
        return run_in_place(kind);
    }
    let flat = Some(sibling_binary(kind.desc().binary)).filter(|path| path.exists());
    flat.or_else(|| Some(cef_sibling_exe()).filter(|path| path.exists()))
}

fn current_exe_dir() -> PathBuf {
    let mut dir = std::env::current_exe().expect("current_exe");
    dir.pop();
    dir
}

/// Where WebKit's host is: the one that runs in place (`run_in_place_from`), else the copy `view_install` puts in
/// `hosts_dir` (`engine-hosts/`), whether or not it is there yet. `None` without a data directory.
pub(super) fn pick_webkit_host(exe_dir: &Path, hosts_dir: Option<&Path>) -> Option<PathBuf> {
    run_in_place_from(ViewKind::Webkit, exe_dir)
        .or_else(|| hosts_dir.map(|dir| dir.join(host_file_name(ViewKind::Webkit))))
}
```

What this changes, so the reviewer can check it:
- CEF's `sibling_host` is byte-for-byte today's: flat first, then the `cef-runtime/` copy (P10).
- Servo and WebView2 add only the dev folder in front of today's flat lookup; WebView2 has none, so it is unchanged.
- WebKit's order flips to the dev folder first (P8), and its walk is shared (P34).
- `view_binary_path` for CEF is today's: `run_in_place(Cef)` is exactly `cef_sibling_exe()` when that file is there.

- [ ] **Step 5: Run the tests to see them pass**

Run: `cargo test -j 4 -p manifold --bin manifold`
Expected: `test result: ok.`, with `each_host_is_found_in_its_dev_folder_then_beside_the_exe` and `the_dev_folder_is_searched_four_parents_up_and_no_further` among the passed tests. `the_lists_read_the_same_host_the_spawn_looks_for` still passes: under `cfg(test)`, `sibling_host` is `None` for every kind but CEF, and the data dir is `None`.

Then run the clippy line and `grep -rn "built_webkit_host" crates`. Expected: no warning; the grep prints nothing.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold/src/process_manager.rs crates/manifold/src/process_manager/availability.rs crates/manifold/src/process_manager/tests.rs
git commit -F - <<'EOF'
app: one lookup finds each host in its dev folder, then beside manifold.exe

ViewDesc::dev_dir names Servo's target-servo and WebKit's target-linux.
run_in_place tries the dev folder first, then the place beside the exe
(CEF only in cef-runtime/), and replaces WebKit's one-off walk. The dev
Servo host now runs where it is built.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 5: A host speaking another protocol is Unavailable, before any spawn

**Files:**
- Modify: `crates/manifold/src/view_install.rs`: `OnDisk` (:69-81) and the `ViewStatus::Unavailable` doc (:47-48).
- Modify: `crates/manifold/src/process_manager/availability.rs`: `read_on_disk`, `view_on_disk` and their docs (:179-197), the docs of `view_availability` (:205-209) and `availability_with` (:237-239), and its body (:249-254); add the stamp cache and `read_on_disk_at`.
- Modify: `crates/manifold/src/process_manager.rs`: the `check_spawnable` doc (:652-654) and the test import.
- Modify (literals gain `skew: None`): `crates/manifold/src/process_manager/tests.rs:492` (`cef_disk`), `crates/manifold/src/view_info.rs:408`, `crates/manifold/src/view_install/tests.rs:28`, `:1349`, `:1352`.
- Test: `crates/manifold/src/process_manager/tests.rs`.

**Interfaces:**
- Consumes: `manifold_view_protocol::{host_protocol, Skew, PROTOCOL_VERSION}` (Tasks 1, 2); `run_in_place` (Task 4).
- Produces:

```rust
// view_install.rs
pub(crate) struct OnDisk { pub(crate) host: RuntimeOnDisk, pub(crate) runtime: RuntimeOnDisk, pub(crate) skew: Option<manifold_view_protocol::Skew> }
// availability.rs
pub(super) type StampCache = std::sync::Mutex<Vec<(PathBuf, u64, Option<std::time::SystemTime>, Option<u32>)>>;
pub(super) fn stamp_cached(cache: &StampCache, path: &Path, read: impl FnOnce(&Path) -> Option<u32>) -> Option<u32>;
pub(super) fn read_on_disk_at(kind: ViewKind, in_place: Option<&Path>, cache: &StampCache) -> OnDisk;
fn read_on_disk(kind: ViewKind) -> OnDisk; // read_on_disk_at(kind, run_in_place(kind).as_deref(), &STAMPS)
```

- [ ] **Step 1: Give `OnDisk` its skew**

In `view_install.rs`, replace:

```rust
/// What the disk holds of an engine's two downloadable parts, each judged by a marker (never by hashing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OnDisk {
```

with:

```rust
/// What the disk holds of an engine's two downloadable parts, each judged by a marker (never by hashing), and whether
/// the host that would run speaks this build's protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OnDisk {
```

After the `runtime` field (`pub(crate) runtime: RuntimeOnDisk,`), add:

```rust
    /// The host that would run in place speaks another protocol than this build (`availability::read_on_disk_at`);
    /// `None` when it speaks this one or none runs in place.
    pub(crate) skew: Option<manifold_view_protocol::Skew>,
```

In `both`, replace `        Self { host: state, runtime: state }` with `        Self { host: state, runtime: state, skew: None }`.

Replace the `ViewStatus::Unavailable` doc:

```rust
    /// Nothing to download and nothing there: this platform has no CEF build (`NO_PIN_REASON`), or this build
    /// pins no host for the engine (`no_download_reason`). No offer made. Holds the reason to show.
```

with:

```rust
    /// It can't run here: this OS can't run it (`platform_unavailable_reason`), the host that would run speaks another
    /// protocol than this build (`OnDisk::skew`), or there is nothing to download and nothing there (no CEF build for
    /// this platform, `NO_PIN_REASON`; no host pinned for the engine, `no_download_reason`). No offer made. Holds the
    /// reason to show.
```

Add `skew: None` to each other literal:
- `availability.rs`, in `read_on_disk`: `OnDisk { host: host_on_disk(kind), runtime: runtime_on_disk(kind), skew: None }` (Step 6 rewrites this function).
- `process_manager/tests.rs`, `cef_disk`: `OnDisk { host, runtime, skew: None }`.
- `view_info.rs`, in `the_disk_text_names_the_parts_the_view_has`: `OnDisk { host: RuntimeOnDisk::Ready, runtime: RuntimeOnDisk::Stale, skew: None }`.
- `view_install/tests.rs`: in `disk`, `OnDisk { host: RuntimeOnDisk::Ready, runtime, skew: None }`; and the two literals `let both = OnDisk { host: host_state(&data, ViewKind::Cef, f), runtime };` and `let no_host = OnDisk { host: host_state(&data, ViewKind::Cef, f), runtime };`, each ending `runtime, skew: None };`.

Run: `cargo test -j 4 -p manifold --bin manifold`
Expected: `test result: ok.` (no behaviour has changed yet). If it does not compile, a literal was missed: `grep -rn "OnDisk {" crates/manifold/src` lists them.

- [ ] **Step 2: Write the failing availability test**

In `process_manager/tests.rs`, after `a_missing_servo_host_is_refused_with_the_reason`, add:

```rust
/// A host that would start speaking another protocol (older, newer, or carrying no number) is Unavailable with the
/// `Skew`'s reason whatever else holds, except on a platform that can't run the engine at all; the spawn is refused
/// with the same reason.
#[test]
fn a_host_speaking_another_protocol_is_unavailable_with_its_reason() {
    use manifold_view_protocol::{Skew, PROTOCOL_VERSION as V};
    let none = Err("unused".to_string());
    for host in [Some(V - 1), Some(V + 1), None] {
        let skew = Skew::between(host, V).unwrap();
        let disk = OnDisk { skew: Some(skew), ..OnDisk::both(RuntimeOnDisk::Ready) };
        for os in Os::ALL {
            for kind in ViewKind::picker_order() {
                let reason = platform_unavailable_reason(os, kind).unwrap_or_else(|| skew.to_string());
                let got =
                    availability_with(os, &none, kind, disk, Some((1, 2)), Some("x"), Some(Path::new("D:/cef")));
                assert_eq!(got, ViewStatus::Unavailable(reason), "{os:?} {kind:?} {host:?}");
            }
        }
    }
    let unstamped = Skew::between(None, V).unwrap();
    let disk = OnDisk { skew: Some(unstamped), ..OnDisk::both(RuntimeOnDisk::Ready) };
    let err = check_spawnable(ViewKind::Servo, &servo_plan(), disk, None).unwrap_err();
    assert_eq!(err.to_string(), format!("Servo is unavailable: {unstamped}"));
}
```

- [ ] **Step 3: Run it to see it fail**

Run: `cargo test -j 4 -p manifold --bin manifold a_host_speaking_another_protocol_is_unavailable`
Expected: FAIL on the first `assert_eq!` that meets an engine this OS can run: the left side is `Installed` (WebView2), `Custom(...)` (CEF) or `Installing { received: 1, total: 2 }` (Servo, WebKit), the right side `Unavailable("the engine host speaks protocol ...")`.

- [ ] **Step 4: Put the skew right after the platform check**

In `availability_with`, after:

```rust
    if let Some(reason) = platform_unavailable_reason(os, kind) {
        return ViewStatus::Unavailable(reason);
    }
```

add:

```rust
    // A host that would start speaking another protocol can't run whatever else holds: a custom runtime, a running
    // install or a recorded failure doesn't change which exe starts. (CEF with a custom runtime starts a copy of
    // `shipped_cef_host()`, which can differ from the `cef-runtime/` copy read here: PARKED "Host protocol stamp".)
    if let Some(skew) = on_disk.skew {
        return ViewStatus::Unavailable(skew.to_string());
    }
```

Replace the doc of `availability_with`:

```rust
/// `view_availability` for a given OS. A plan that is an `Err` means there is nothing to download: `Unavailable`
/// with its reason, unless the parts are already there (a dev tree's siblings) or, for CEF, a custom runtime is
/// set and the host is there.
```

with:

```rust
/// `view_availability` for a given OS. After the platform's reason, a host speaking another protocol (`OnDisk::skew`)
/// is `Unavailable` with the `Skew`'s reason. A plan that is an `Err` means there is nothing to download:
/// `Unavailable` with its reason, unless the parts are already there (a dev tree's hosts) or, for CEF, a custom
/// runtime is set and the host is there.
```

In the doc of `view_availability`, replace `/// `on_disk` what the engine has on disk (`view_on_disk`). An engine that is not downloaded (embedded, or a sibling` and its next line `/// only) is Installed. A downloaded one follows its host and, for CEF, its runtime: a custom path (and a running` with:

```rust
/// `on_disk` what the engine has on disk (`view_on_disk`). A host that would run speaking another protocol is
/// Unavailable, naming which side to rebuild. Otherwise an engine that is not downloaded (embedded, or in place only)
/// is Installed, and a downloaded one follows its host and, for CEF, its runtime: a custom path (and a running
```

(the doc's last two lines, `/// install) override; ...` and `/// download and nothing there it is Unavailable with the plan's reason.`, stay).

In `process_manager.rs`, replace the `check_spawnable` doc:

```rust
/// Refuses to launch an engine whose state (from disk alone) isn't Installed or Custom: by its reason when
/// Unavailable (no host and nothing pinned to download), as needing an update when Update needed, else as not
/// installed.
```

with:

```rust
/// Refuses to launch an engine whose state (from disk alone) isn't Installed or Custom: by its reason when
/// Unavailable (no host and nothing pinned to download, or a host speaking another protocol), as needing an update
/// when Update needed, else as not installed.
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `cargo test -j 4 -p manifold --bin manifold`
Expected: `test result: ok.`

- [ ] **Step 6: Write the failing cache test**

In `process_manager/tests.rs`, after `on_disk_answers_are_reused_until_the_ttl_or_forgotten`, add:

```rust
/// A host's stamp is read again when it is rebuilt in place (its length or modified time changes), and not before;
/// a path that can't be stat'ed is never remembered.
#[test]
fn a_rebuilt_host_is_read_again() {
    let dir = crate::store::testutil::temp_dir("stamp-cache");
    let exe = dir.join("host.exe");
    std::fs::write(&exe, b"one").unwrap();
    let cache = StampCache::default();
    let reads = std::cell::Cell::new(0);
    let read = |_: &Path| {
        reads.set(reads.get() + 1);
        Some(reads.get())
    };
    assert_eq!(stamp_cached(&cache, &exe, read), Some(1));
    assert_eq!(stamp_cached(&cache, &exe, read), Some(1), "unchanged: remembered");
    std::fs::write(&exe, b"three").unwrap();
    assert_eq!(stamp_cached(&cache, &exe, read), Some(2), "a new length");
    std::fs::File::options()
        .write(true)
        .open(&exe)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000))
        .unwrap();
    assert_eq!(stamp_cached(&cache, &exe, read), Some(3), "a new modified time, same length");
    assert_eq!(stamp_cached(&cache, &exe, read), Some(3));
    let gone = dir.join("gone.exe");
    assert_eq!(stamp_cached(&cache, &gone, read), Some(4));
    assert_eq!(stamp_cached(&cache, &gone, read), Some(5), "never remembered");
}

/// The disk check fills `skew` from the host that would run in place (`run_in_place`, which is `None` under test, so
/// the test passes it in), judged by the number it carries (`host_protocol`), which side is which fixed by
/// `Skew::between`: the test exe links `send_version`, so it speaks this build's protocol; a file with no stamp differs
/// as unstamped, and its reason says to rebuild the host; with no host in place there is no skew. WebView2 is not
/// downloaded, so nothing else on disk is read. The test exe is the one real file a stamp is read from here; no host's
/// is read (every skew the availability tests use is an `OnDisk` they build).
#[test]
fn the_disk_check_judges_the_host_in_place_by_the_number_it_carries() {
    let cache = StampCache::default();
    let check = |in_place: Option<&Path>| read_on_disk_at(ViewKind::Webview2, in_place, &cache);
    let me = std::env::current_exe().unwrap();
    assert_eq!(check(Some(me.as_path())), OnDisk::both(RuntimeOnDisk::Ready), "the test exe speaks this protocol");
    let garbage = crate::store::testutil::temp_dir("disk-skew").join("host.exe");
    std::fs::write(&garbage, b"MZ not a program").unwrap();
    let skew = check(Some(garbage.as_path())).skew.expect("an unstamped file differs");
    assert_eq!((skew.host(), skew.app()), (None, manifold_view_protocol::PROTOCOL_VERSION));
    assert!(skew.to_string().ends_with("; rebuild the engine host"), "{skew}");
    assert_eq!(check(None), OnDisk::both(RuntimeOnDisk::Ready), "no host in place: no skew");
}
```

In `process_manager.rs`, add `read_on_disk_at, stamp_cached, StampCache` to the `#[cfg(test)] use availability::{...}` list (`cargo fmt` puts them in its order).

Run: `cargo test -j 4 -p manifold --bin manifold -- a_rebuilt_host the_disk_check_judges`
Expected: a compile error: no `read_on_disk_at`, `stamp_cached` or `StampCache` in `availability`.

- [ ] **Step 7: Read the stamp of the host that would run, cached by its file**

In `availability.rs`, replace `read_on_disk` and `view_on_disk` with their docs:

```rust
/// The disk check `kind` needs before it can run, read now: a downloaded engine's host (a sibling, or the marker
/// of the pinned files) and CEF's runtime. Only markers and file sizes are read, never file contents. Nothing to
/// check for the rest (an embedded host; WebKit's is found in WSL only when a tab spawns), which read Ready.
fn read_on_disk(kind: ViewKind) -> OnDisk {
    if downloads(kind) {
        OnDisk { host: host_on_disk(kind), runtime: runtime_on_disk(kind), skew: None }
    } else {
        OnDisk::both(RuntimeOnDisk::Ready)
    }
}

/// The `on_disk` that `view_availability` takes for `kind`, for the engine lists: `read_on_disk`'s answer, reread
/// at most every `ON_DISK_CHECK_TTL`; Ready when there is nothing to check.
pub(crate) fn view_on_disk(kind: ViewKind) -> OnDisk {
    if !downloads(kind) {
        return OnDisk::both(RuntimeOnDisk::Ready);
    }
    on_disk_cached(&ON_DISK, kind, std::time::Instant::now(), || read_on_disk(kind))
}
```

with:

```rust
/// The stamp last read per host path, keyed by the file's length and modified time, so a host rebuilt in place is
/// read again.
pub(super) type StampCache = std::sync::Mutex<Vec<(PathBuf, u64, Option<std::time::SystemTime>, Option<u32>)>>;
static STAMPS: StampCache = std::sync::Mutex::new(Vec::new());

/// The protocol number `read` finds in `path`, remembered in `cache` until the file's length or modified time changes.
/// A path that can't be stat'ed is read every time and never remembered.
pub(super) fn stamp_cached(cache: &StampCache, path: &Path, read: impl FnOnce(&Path) -> Option<u32>) -> Option<u32> {
    let Ok(meta) = std::fs::metadata(path) else { return read(path) };
    let (len, modified) = (meta.len(), meta.modified().ok());
    let mut entries = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(&(_, _, _, stamp)) = entries.iter().find(|e| e.0 == path && e.1 == len && e.2 == modified) {
        return stamp;
    }
    let stamp = read(path);
    entries.retain(|e| e.0 != path);
    entries.push((path.to_path_buf(), len, modified, stamp));
    stamp
}

/// The disk check `kind` needs before it can run, read now, for every engine: `read_on_disk_at` with the host that
/// would run in place (`run_in_place`, never a host under `view_hosts_dir()`) and this process's stamp cache.
fn read_on_disk(kind: ViewKind) -> OnDisk {
    read_on_disk_at(kind, run_in_place(kind).as_deref(), &STAMPS)
}

/// `read_on_disk` given the host `in_place` that would run in place, its number read through `cache`
/// (`host_protocol`): the skew when it speaks another protocol than this build or carries none (built before hosts
/// were stamped, or not a host at all), none when it speaks this one or no host runs in place; and for a downloaded
/// engine its host (in place, or the marker of the pinned files) and CEF's runtime. Only markers, file sizes and a
/// host's section table are read (the table cached by the file's length and time), never whole files. An engine that
/// is not downloaded (an embedded host) has no parts to check, which read Ready.
pub(super) fn read_on_disk_at(kind: ViewKind, in_place: Option<&Path>, cache: &StampCache) -> OnDisk {
    use manifold_view_protocol::{host_protocol, Skew, PROTOCOL_VERSION};
    let parts = if downloads(kind) {
        OnDisk { host: host_on_disk(kind), runtime: runtime_on_disk(kind), skew: None }
    } else {
        OnDisk::both(RuntimeOnDisk::Ready)
    };
    let skew = in_place.and_then(|exe| Skew::between(stamp_cached(cache, exe, host_protocol), PROTOCOL_VERSION));
    OnDisk { skew, ..parts }
}

/// The `on_disk` that `view_availability` takes for `kind`, for the engine lists: `read_on_disk`'s answer, reread
/// at most every `ON_DISK_CHECK_TTL`.
pub(crate) fn view_on_disk(kind: ViewKind) -> OnDisk {
    on_disk_cached(&ON_DISK, kind, std::time::Instant::now(), || read_on_disk(kind))
}
```

(The early `!downloads` return of `view_on_disk` goes: an embedded WebView2 host beside the exe is now checked too, D4. The early `!downloads` return in `availability_with` stays: it comes after the skew branch.)

What pins what: `the_disk_check_judges_the_host_in_place_by_the_number_it_carries` pins `read_on_disk_at` (that it fills `skew` from `in_place`, the reader it passes, and which number is the host's). `read_on_disk` is one line over `run_in_place`, which is `None` under test (P12); Task 9 Step 2 pins that line live, where the unstamped Servo and CEF hosts must read Unavailable.

- [ ] **Step 8: Tidy the tests the change made untrue**

In `process_manager/tests.rs`:
- Rename `view_kinds_that_are_not_downloaded_are_always_installed_where_the_os_can_run_them` to `view_kinds_that_are_not_downloaded_are_installed_where_the_os_can_run_them`, and give it the doc `/// Unless its host speaks another protocol (`a_host_speaking_another_protocol_is_unavailable_with_its_reason`).`
- Delete `bundled_view_kinds_read_ready_without_a_disk_check` with its two doc lines (`/// Engines that need nothing on disk read as Ready without touching it, cached or fresh; spawn and the engine lists` / `/// share one reader per engine (`on_disk_reader`).`). Its one assertion is also the last line of `the_lists_read_the_same_host_the_spawn_looks_for`.
- Replace the doc of `the_lists_read_the_same_host_the_spawn_looks_for`:

```rust
/// The lists' answer for a downloaded engine's host matches the disk the spawn reads: a sibling host (the test exe has
/// none beside it unless one was copied there) is Ready, anything else Missing (the data dir is `None` under test).
```

with:

```rust
/// The lists' answer for a downloaded engine's host matches the disk the spawn reads: a host in place is Ready,
/// anything else Missing. Under test no host runs in place (`run_in_place`), the data dir is `None`, and only CEF's
/// flat or `cef-runtime/` copy beside the test exe could count, so no skew is read either.
```

- [ ] **Step 9: Run everything and commit**

Run: `cargo test -j 4 -p manifold --bin manifold`, then the clippy line.
Expected: `test result: ok.`; no warning.

```bash
cargo fmt --all
git diff --stat
git add crates/manifold/src/view_install.rs crates/manifold/src/view_install/tests.rs crates/manifold/src/view_info.rs crates/manifold/src/process_manager.rs crates/manifold/src/process_manager/availability.rs crates/manifold/src/process_manager/tests.rs
git commit -F - <<'EOF'
app: a host speaking another protocol is Unavailable, saying which side to rebuild

read_on_disk reads the stamp of the host that would run in place, for
every engine, cached by the file's length and time, into OnDisk::skew;
availability_with makes it Unavailable right after the platform check, so
the lists, Settings, the pick and the spawn all refuse it by one reason.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 6: An Unavailable engine's failed card and menu explain, and never offer a download

**Files:**
- Modify: `crates/manifold/src/view_info.rs`:
  - `on_disk_text` and its doc (:24-38);
  - the `PickOutcome::Unavailable` doc (:88);
  - `PickOutcome::retry_label`, in `impl PickOutcome` (:92-97);
  - the engine row's dim comment (:197);
  - tests in its test module.
- Modify: `crates/manifold/src/card.rs`: `missing_view_phrases` and its doc (:993-1000), and a new `missing_view_text` after it.
- Modify: `crates/manifold/src/card/tab_body.rs`: the comment at :50 and the body text at :110-111 (now `missing_view_text`).
- Modify: `crates/manifold/src/card/tests.rs:720-727` (`a_failed_card_names_an_outdated_view_as_needing_an_update`).
- Modify: `crates/manifold/src/node_menu.rs`:
  - the `view_missing` field of `NodeMenuView` (:637-638);
  - the tab menu's first row (:569-574);
  - `node_menu_view`'s `view_missing:` value (:798-801).
- Modify: `crates/manifold/src/node_menu/tests.rs`: `view_of` (:58), the missing-engine case (:179-180), a new app test.
- Modify: `crates/manifold/src/settings_page.rs:215` (comment).
- Modify: `crates/manifold/src/fetched.rs:296-297` (the comment above `retry_missing_file`'s `match`).

**Interfaces:**
- Consumes: `OnDisk::skew`, `Skew::{host, app}` (Tasks 2, 5); `pick_outcome`, `App::view_status`.
- Produces:

```rust
impl PickOutcome { pub(crate) fn retry_label(&self) -> &'static str; } // "Retry" | "Download Engine" | "Why Unavailable"
// NodeMenuView: `view_missing: bool` is replaced by
retry_label: &'static str,
```

- [ ] **Step 1: Write the failing tests**

In `view_info.rs`'s test module, at the end of `the_disk_text_names_the_parts_the_view_has`, add:

```rust
        let older = manifold_view_protocol::Skew::between(Some(7), 8);
        assert_eq!(on_disk_text(ViewKind::Servo, OnDisk { skew: older, ..disk }), "host protocol 7, manifold 8");
        let unstamped = manifold_view_protocol::Skew::between(None, 8);
        assert_eq!(
            on_disk_text(ViewKind::Cef, OnDisk { skew: unstamped, ..OnDisk::both(RuntimeOnDisk::Ready) }),
            "host unstamped, runtime ready"
        );
```

and after that test, add:

```rust
    /// A failed tab's first menu row names what a click on it does, for every engine state.
    #[test]
    fn a_failed_tabs_retry_is_named_for_what_it_does() {
        let label = |state: ViewStatus| pick_outcome(ViewKind::Servo, &state).retry_label();
        assert_eq!(label(ViewStatus::Installed), "Retry");
        assert_eq!(label(ViewStatus::Custom("D:/cef".into())), "Retry");
        assert_eq!(label(ViewStatus::NotInstalled { size: 1 }), "Download Engine");
        assert_eq!(label(ViewStatus::UpdateNeeded { size: 1 }), "Download Engine");
        assert_eq!(label(ViewStatus::Failed("x".into())), "Download Engine");
        assert_eq!(label(ViewStatus::Installing { received: 1, total: 2 }), "Download Engine");
        assert_eq!(label(ViewStatus::Unavailable("x".into())), "Why Unavailable");
    }
```

In `card/tests.rs`, replace the whole test `a_failed_card_names_an_outdated_view_as_needing_an_update`, with its doc line, by:

```rust
/// A failed card whose engine can't run names why and what a click on it does: a download for one that is missing or
/// out of date, the reason for one that is unavailable (a download could not help).
#[test]
fn a_failed_card_says_what_a_click_on_it_does() {
    use crate::view_install::ViewStatus;
    assert_eq!(
        missing_view_phrases(&ViewStatus::UpdateNeeded { size: 1 }),
        ("needs an update", "needs an update (click to download)")
    );
    assert_eq!(
        missing_view_phrases(&ViewStatus::NotInstalled { size: 1 }),
        ("is not installed", "not installed (click to download)")
    );
    assert_eq!(
        missing_view_phrases(&ViewStatus::Failed("x".into())),
        ("is not installed", "not installed (click to download)")
    );
    assert_eq!(
        missing_view_phrases(&ViewStatus::Unavailable("x".into())),
        ("is unavailable", "unavailable (click for why)")
    );
    // The card body is the engine's label and that phrase, so the click hint is said once.
    assert_eq!(
        missing_view_text(ViewKind::Servo, &ViewStatus::Unavailable("x".into())),
        "Servo unavailable (click for why)"
    );
    assert_eq!(
        missing_view_text(ViewKind::Cef, &ViewStatus::NotInstalled { size: 1 }),
        "Blink (CEF) not installed (click to download)"
    );
}
```

In `node_menu/tests.rs`:
- In `view_of`, replace `        view_missing: false,` with `        retry_label: "Retry",`.
- Replace:

```rust
    let missing = NodeMenuView { failed: true, view_missing: true, ..view_of(sleeping_tab()) };
    assert_eq!(menu_of(&missing, false).2, "Download Engine | Copy URL | --- | Rename | Pin | --- | Close");
```

with:

```rust
    let missing = NodeMenuView { failed: true, retry_label: "Download Engine", ..view_of(sleeping_tab()) };
    assert_eq!(menu_of(&missing, false).2, "Download Engine | Copy URL | --- | Rename | Pin | --- | Close");
    let unavailable = NodeMenuView { failed: true, retry_label: "Why Unavailable", ..view_of(sleeping_tab()) };
    assert_eq!(menu_of(&unavailable, false).2, "Why Unavailable | Copy URL | --- | Rename | Pin | --- | Close");
```

- After `duplicate_from_an_unloaded_tabs_menu_spawns_a_copy`, add:

```rust
/// A failed tab's first row follows its engine's state: Retry when it can run, the download when it is missing, and
/// why when it can't run at all, where a download could not help.
#[test]
fn a_failed_tabs_first_row_follows_its_engines_state() {
    use crate::view_install::ViewStatus;
    let mut app = App::default();
    let failed = crate::node::Runtime::Unloaded(crate::node::UnloadedTabPayload {
        url: "https://x.test/".into(),
        title: "X".into(),
        muted: false,
        zoom_factor: 1.0,
        state: crate::node::UnloadedState::Failed("x".into()),
    });
    let tab = crate::session_sync::tests::add(&mut app, crate::store::model::Kind::Tab, failed);
    let kind = app.doc.mirror.view_kind(tab);
    let unstamped = manifold_view_protocol::Skew::between(None, manifold_view_protocol::PROTOCOL_VERSION).unwrap();
    for (state, row) in [
        (ViewStatus::Installed, "Retry"),
        (ViewStatus::NotInstalled { size: 1 }, "Download Engine"),
        (ViewStatus::Unavailable(unstamped.to_string()), "Why Unavailable"),
    ] {
        app.view_ui.view_status_cache.borrow_mut().insert(kind, state);
        assert_eq!(app.node_menu_view(tab).unwrap().retry_label, row);
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 4 -p manifold --bin manifold failed`
Expected: a compile error: no method `retry_label` on `PickOutcome`, `NodeMenuView` has no field `retry_label`, and no function `missing_view_text`.

- [ ] **Step 3: Name each state's click once, and use it on the card and the menu**

In `view_info.rs`, replace `on_disk_text`'s doc and its `let mut text = format!("host {}", word(disk.host));` line. The doc:

```rust
/// What the disk holds of `kind`'s downloadable parts, for the headless `VIEWSTATE` and `EXPORTSTATE`:
/// "host ready", and for CEF also "runtime missing" (Ready, Missing or Stale each).
```

becomes:

```rust
/// What the disk holds of `kind`'s downloadable parts, for the headless `VIEWSTATE` and `EXPORTSTATE`:
/// "host ready", and for CEF also "runtime missing" (Ready, Missing or Stale each). A host speaking another protocol
/// is named by that instead of "host ready": "host protocol 7, manifold 8", or "host unstamped".
```

and the line becomes:

```rust
    let mut text = match disk.skew {
        Some(skew) => match skew.host() {
            Some(host) => format!("host protocol {host}, manifold {}", skew.app()),
            None => "host unstamped".to_string(),
        },
        None => format!("host {}", word(disk.host)),
    };
```

Replace the `PickOutcome::Unavailable` doc `    /// This platform cannot run it (no pin, wrong OS): the click only explains, with the reason.` with `    /// It can't run here (no pin, wrong OS, or a host speaking another protocol): the click only explains why.`

In `impl PickOutcome`, after `unavailable_toast`, add:

```rust

    /// The first row of a failed tab's menu, naming what a click on it does (`App::retry_missing_file` acts on the same
    /// outcome): try again, offer the download, or say why the engine can't run.
    pub(crate) fn retry_label(&self) -> &'static str {
        match self {
            PickOutcome::Switch(_) => "Retry",
            PickOutcome::AskInstall(_) | PickOutcome::AlreadyInstalling(_) => "Download Engine",
            PickOutcome::Unavailable(..) => "Why Unavailable",
        }
    }
```

Replace the engine row comment `    // An engine this platform cannot run is dimmed to weak text and only explains itself when clicked.` with these two lines:

```rust
    // An engine that can't run here (this platform, or a host speaking another protocol) is dimmed to weak text and
    // only explains itself when clicked.
```

In `card.rs`, replace `missing_view_phrases` with its doc:

```rust
/// How a failed card whose engine can't run names why: (tooltip phrase, card-body phrase), after the
/// engine's label. A runtime that is there but out of date needs an update; anything else is not installed.
fn missing_view_phrases(state: &crate::view_install::ViewStatus) -> (&'static str, &'static str) {
    match state {
        crate::view_install::ViewStatus::UpdateNeeded { .. } => ("needs an update", "needs an update"),
        _ => ("is not installed", "not installed"),
    }
}
```

with:

```rust
/// How a failed card whose engine can't run names why: (tooltip phrase, card-body phrase), after the engine's label.
/// The body phrase ends with what a click on the card does (`App::retry_missing_file`): a download for a runtime that
/// is missing or out of date, the reason for an engine that is unavailable, where a download could not help. (The
/// tooltip of an unavailable engine is its toast, so its tooltip phrase only keeps the match whole.)
fn missing_view_phrases(state: &crate::view_install::ViewStatus) -> (&'static str, &'static str) {
    use crate::view_install::ViewStatus;
    match state {
        ViewStatus::Unavailable(_) => ("is unavailable", "unavailable (click for why)"),
        ViewStatus::UpdateNeeded { .. } => ("needs an update", "needs an update (click to download)"),
        _ => ("is not installed", "not installed (click to download)"),
    }
}

/// The body text of a failed card whose engine can't run: the engine's label, then its body phrase
/// (`missing_view_phrases`), which already says what a click does.
fn missing_view_text(kind: ViewKind, state: &crate::view_install::ViewStatus) -> String {
    format!("{} {}", view_label(kind), missing_view_phrases(state).1)
}
```

In `card/tab_body.rs`:
- Replace `        // A failed card whose engine isn't installed offers its Download, not a retry that can't work.` with `        // A failed card whose engine can't run offers what a click on it does: its Download, or why it is unavailable.`
- Replace the two lines

```rust
                let phrase = missing_view_phrases(&self.view_status(kind)).1;
                let text = format!("{} {phrase} (click to download)", view_label(kind));
```

  with `                let text = missing_view_text(kind, &self.view_status(kind));`.

In `node_menu.rs`:
- Replace the field:

```rust
    /// Unloaded only: it failed because its engine isn't installed, so the menu offers the download.
    view_missing: bool,
```

with:

```rust
    /// Unloaded only, when it failed: its first row, naming what a click does for its engine's state
    /// (`PickOutcome::retry_label`).
    retry_label: &'static str,
```

- Replace the tab menu's first row:

```rust
            // Retrying a tab whose engine isn't installed asks to install it (`retry_missing_file`).
            let first = match (view.failed, view.view_missing) {
                (true, true) => Button("Download Engine", A::RetryMissingFile),
                (true, false) => Button("Retry", A::RetryMissingFile),
                (false, _) => Button("Open", A::Open),
            };
```

with:

```rust
            // A failed tab's first row says what retrying does for its engine (`retry_missing_file`): Retry, a
            // download, or why the engine can't run.
            let first = if view.failed { Button(view.retry_label, A::RetryMissingFile) } else { Button("Open", A::Open) };
```

- In `node_menu_view`, replace:

```rust
            view_missing: node.tab_failed() && {
                let kind = self.doc.mirror.view_kind(id);
                !matches!(pick_outcome(kind, &self.view_status(kind)), PickOutcome::Switch(_))
            },
```

with:

```rust
            retry_label: if node.tab_failed() {
                let kind = self.doc.mirror.view_kind(id);
                pick_outcome(kind, &self.view_status(kind)).retry_label()
            } else {
                "Retry"
            },
```

In `settings_page.rs`, replace `        // A saved default this platform cannot run is kept, not replaced; say why new tabs will fail.` with these two lines:

```rust
        // A saved default that can't run (wrong platform, nothing pinned, or a host speaking another protocol) is kept,
        // not replaced; say why new tabs will fail.
```

In `fetched.rs`, in `retry_missing_file`, replace the comment above the `match pick_outcome(...)`:

```rust
            // A tab that failed for want of its engine's runtime can't succeed by retrying: ask to
            // install the engine instead (the card's Download).
```

with:

```rust
            // A tab that failed for want of its engine can't succeed by retrying. A click does what the card and the
            // menu's first row say (`PickOutcome::retry_label`): offer the engine's download, say it is already
            // installing, say why it can't run here, or wake the tab when the engine is fine.
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -j 4 -p manifold --bin manifold`, then the clippy line, then `grep -rn "view_missing" crates` and `grep -n "click to download\|missing_view_phrases" crates/manifold/src/card/tab_body.rs`.
Expected: `test result: ok.`; no warning; the first grep prints nothing; the second prints only the tooltip's `missing_view_phrases(&state).0,` line (its "Click to download it" is capitalised), so the body's hint lives only in `missing_view_phrases`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/manifold/src/view_info.rs crates/manifold/src/card.rs crates/manifold/src/card/tab_body.rs crates/manifold/src/card/tests.rs crates/manifold/src/node_menu.rs crates/manifold/src/node_menu/tests.rs crates/manifold/src/settings_page.rs crates/manifold/src/fetched.rs
git commit -F - <<'EOF'
app: an Unavailable engine's failed card and menu explain, never offer a download

missing_view_phrases gains an Unavailable arm ending "(click for why)", and
the menu's first row is PickOutcome::retry_label, read from the same
outcome retry_missing_file acts on. on_disk_text names a skewed host by its
protocol instead of "host ready".

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 7: The failed-spawn stage still reaches the OS

**Files:**
- Modify: `crates/gate/src/probes.rs`:
  - the module doc's `failed-spawn` line (:9);
  - `failed_spawn`'s comment (:547-551), its host write (:556), and a check after `quit(&mut inst, "failed-spawn");` (:581);
  - new helpers `unloadable_copy` and `unloadable` after `failed_spawn` (the end of the file), then a test module.
- Modify: `crates/gate/src/fresh.rs:54-55` (comment).
- Modify: `docs/runs.md` (rows run.ps1 writes).

**Interfaces:**
- Consumes: `host_protocol`'s rule that the section table is found by `SizeOfOptionalHeader`, never by the optional header's magic (Task 1); the app's pre-spawn refusal (Task 5).
- Produces (private to `probes.rs`): `fn unloadable_copy(host: &Path) -> io::Result<Vec<u8>>`, and the pure `fn unloadable(image: Vec<u8>) -> Option<Vec<u8>>` it calls.

- [ ] **Step 1: Write the failing check**

In `failed_spawn`, after `        quit(&mut inst, "failed-spawn");`, add:

```rust
        let log = fs::read_to_string(&inst.log).unwrap_or_default();
        check(
            "the spawn reached the OS and failed there, not refused before it",
            log.contains("spawn tab failed for") && log.contains("(os error ") && !log.contains("is unavailable"),
        );
```

Run: `cargo test -j 4 -p gate`, then the clippy line. Expected: `test result: ok.`; no warning.

- [ ] **Step 2: Run the stage to see it fail**

Do the pre-run check, then from bash:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name failed-spawn-red -Detach -Command 'cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe failed-spawn "%RUN_DIR%"'
```

Wait for it (see "Waiting for a run", deadline 900 s). Then, with `RUN` set to its folder:

```bash
grep -E "^(PASS|FAIL)" "$RUN/out.log"; grep -h "spawn tab failed\|is unavailable" "$RUN"/gate-failed-spawn-*/log
```

Expected:
- the six existing checks PASS, and `FAIL: the spawn reached the OS and failed there, not refused before it`;
- the log has one `spawn tab failed for <id>: ...` line per failed spawn (the startup Root, each New, the Retry), and each reads `spawn tab failed for <id>: WebView2 is unavailable: the engine host carries no protocol number (built before hosts were stamped); rebuild the engine host`. The 13-byte "not a program" file is now refused before the spawn, which is what this task fixes.

Then delete the failed stage's work folder: `rm -rf -- "$RUN"/gate-failed-spawn-*`.

If the six existing checks do not all PASS, or any of those log lines differs, stop and report it with both outputs.

- [ ] **Step 3: Use a real host that Windows refuses to start**

First the helper's test. At the end of `probes.rs` (after `failed_spawn`, the last item at bfacedc2), add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The copy differs from the image only in the PE optional header's magic, and a file that is not a PE image is
    /// refused, so the stage fails loudly instead of running a host that still starts.
    #[test]
    fn an_unloadable_copy_zeroes_only_the_pe_magic() {
        let mut image = vec![0u8; 0x40];
        image[..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        image.extend_from_slice(b"PE\0\0");
        image.extend_from_slice(&[0x11; 20]); // the COFF header
        image.extend_from_slice(&0x20Bu16.to_le_bytes()); // the optional header's magic (PE32+)
        image.extend_from_slice(&[0x22; 30]);
        let copy = unloadable(image.clone()).expect("a PE image");
        assert_eq!(copy.len(), image.len());
        let changed: Vec<usize> = (0..image.len()).filter(|&at| copy[at] != image[at]).collect();
        assert_eq!(changed, [0x40 + 24, 0x40 + 25]);
        assert_eq!(copy[0x58..0x5A], [0, 0]);
        assert_eq!(unloadable(b"not a program".to_vec()), None);
        let mut no_signature = image.clone();
        no_signature[0x40] = b'X';
        assert_eq!(unloadable(no_signature), None, "no PE signature");
        assert_eq!(unloadable(image[..0x40 + 25].to_vec()), None, "cut inside the magic");
    }
}
```

Run: `cargo test -j 4 -p gate unloadable`
Expected: a compile error: cannot find function `unloadable`.

Then, in `failed_spawn`, replace the comment:

```rust
        // The failure is forced by running a copy of the exe with a view host beside it that is not a program: a
        // sibling host is used as it stands (`view_binary_path`), so the spawn fails. With none beside it the app
        // falls back to the host it extracted into `engine-hosts/`, which `--profile-dir` does not redirect (it is the
        // user's real data dir, `profile::view_data_dir`) and which exists on any machine that has run the app, so the
        // spawn succeeded and the stage proved nothing.
```

with:

```rust
        // The failure is forced by running a copy of the exe with a view host beside it that Windows refuses to
        // start: a host beside the exe runs as it stands (`view_binary_path`), so the spawn reaches the OS and fails
        // there. The host is the real one with its PE magic zeroed (`unloadable_copy`), so it still carries this
        // build's protocol number; a file that is not a host at all carries none, and the app would refuse it before
        // spawning (Unavailable), which is another path. With no host beside the exe the app falls back to the host it
        // extracted into `engine-hosts/`, which `--profile-dir` does not redirect (it is the user's real data dir,
        // `profile::view_data_dir`) and which exists on any machine that has run the app, so the spawn succeeded and
        // the stage proved nothing.
```

Replace `        fs::write(bin.join("manifold-view-webview2.exe"), "not a program")?;` with:

```rust
        fs::write(
            bin.join("manifold-view-webview2.exe"),
            unloadable_copy(&exe.with_file_name("manifold-view-webview2.exe"))?,
        )?;
```

After `failed_spawn` (after its closing `}`, before the test module), add:

```rust
/// `host`'s bytes with its PE optional header's magic zeroed (`unloadable`), or an error naming a host that is not a
/// PE image.
fn unloadable_copy(host: &Path) -> io::Result<Vec<u8>> {
    unloadable(fs::read(host)?).ok_or_else(|| io::Error::other(format!("{} is not a PE image", host.display())))
}

/// `image` with its PE optional header's magic zeroed, or `None` when it is not a PE image. Windows refuses to start
/// it ("%1 is not a valid Win32 application", os error 193; checked 2026-10-28), while the app still reads its
/// protocol number: `host_protocol` finds the section table by the optional header's size, never by its magic.
fn unloadable(mut image: Vec<u8>) -> Option<Vec<u8>> {
    let pe = usize::try_from(u32::from_le_bytes(image.get(0x3C..0x40)?.try_into().ok()?)).ok()?;
    if image.get(pe..pe.checked_add(4)?)? != b"PE\0\0".as_slice() {
        return None;
    }
    image.get_mut(pe + 24..pe + 26)?.fill(0);
    Some(image)
}
```

In the module doc, replace `//! - `failed-spawn` (was failed-spawn-probe.sh): a failed spawn leaves one failed tab card, kept by New and Retry` with `//! - `failed-spawn` (was failed-spawn-probe.sh): a spawn the OS refuses leaves one failed tab card, kept by New and Retry`.

In `fresh.rs`, replace:

```rust
    // CEF runs from an isolated copy that only `gate sync-runtime-deps` refreshes; a stale one speaks an old message
    // numbering and its host crashes.
```

with:

```rust
    // CEF runs from an isolated copy that only `gate sync-runtime-deps` refreshes. The app itself refuses a copy whose
    // protocol number differs from its own (Unavailable, naming which side to rebuild); this catches a copy that is
    // older but speaks the same protocol, which would run old code silently.
```

Run: `cargo test -j 4 -p gate`, then the clippy line. Expected: `test result: ok.` (`an_unloadable_copy_zeroes_only_the_pe_magic` among them, and `fresh_exes_pass_and_each_stale_one_is_named` unchanged, D3); no warning. Whether Windows then refuses the copy (os error 193) is proved by the stage itself in Step 4, whose new check needs an `(os error ` line.

- [ ] **Step 4: Run the stage to see it pass**

Do the pre-run check, then:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name failed-spawn-green -Detach -Command 'cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe failed-spawn "%RUN_DIR%"'
```

Wait for it, then run `grep -E "^(PASS|FAIL)" "$RUN/out.log"`.
Expected: seven `PASS` lines, no `FAIL`, and `run.json`'s `"exit"` is `0`. The stage deletes its own work folder on a pass. Diff the Downloads folder as "Waiting for a run" says.

If the new check fails and the log shows `is unavailable`, the copy's stamp did not read: stop and report the log line. Do not loosen the check.

- [ ] **Step 5: Commit**

Set each run's headline if run.ps1 left it empty (RUNS.md "Finding and closing"), then:

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/probes.rs crates/gate/src/fresh.rs docs/runs.md
git commit -F - <<'EOF'
gate: failed-spawn uses a real host Windows refuses, so the spawn still reaches the OS

A 13-byte non-program is now refused as unstamped before any spawn. The
stage copies the WebView2 host with its PE magic zeroed (still stamped,
os error 193) and checks the log names an OS error, not a refusal.
Runs failed-spawn-red and failed-spawn-green.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 8: `sync-runtime-deps` puts Servo's DLLs beside its host, and runs every step

**Files:**
- Modify (rewrite): `crates/gate/src/sync_deps.rs` (the whole file, 155 lines at bfacedc2).
- Delete from disk (build output, not tracked): `target/debug/manifold-view-servo.exe`, `target/debug/libEGL.dll`, `target/debug/libGLESv2.dll` (P39).

**Interfaces:**
- Consumes: Task 4's lookup, under which a dev tree runs Servo from `target-servo/debug/` and nothing runs a Servo host from `target/debug/` (P17).
- Produces: `gate sync-runtime-deps` output lines `recopied target-servo/debug/libEGL.dll` (or `target-servo/debug/libEGL.dll already current`), the CEF lines unchanged (`recopied cef-runtime/manifold-view-cef.exe`), and on failure one `sync-runtime-deps: <file> ...` line per failed step and exit 1. `main.rs`'s stage row (`crate::sync_deps::sync_runtime_deps`, a `fn() -> ExitCode`) is unchanged.

- [ ] **Step 1: Write the failing tests**

In `sync_deps.rs`'s test module, keep `touch`, replace the test `the_newest_of_both_layouts_wins_and_sync_copies_only_on_change` and add the helper and three tests, so the module reads (the module, tests included, compiles only on Windows: `main.rs` declares `#[cfg(windows)] mod sync_deps;`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path, secs: u64, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
        let f = fs::OpenOptions::new().append(true).open(p).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
    }

    /// A fresh, empty folder for one test, under the system temp folder.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gate-sync-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The newest written match wins across every build folder searched and both output layouts; a folder that
    /// isn't there is skipped.
    #[test]
    fn the_newest_in_either_build_folder_wins_and_sync_copies_only_on_change() {
        let dir = scratch("newest");
        let (main, servo) = (dir.join("build"), dir.join("servo-build"));
        touch(&main.join("mozangle-aaa/out/x.dll"), 10, b"old");
        touch(&servo.join("mozangle/bbb/out/x.dll"), 20, b"new");
        touch(&main.join("other-ccc/out/x.dll"), 30, b"other");
        let src = find_newest(&[servo, main.clone()], "mozangle", "x.dll").unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"new");
        let only_main = find_newest(&[main.clone()], "mozangle", "x.dll").unwrap();
        assert_eq!(fs::read(only_main).unwrap(), b"old");
        let dst = dir.join("x.dll");
        assert!(sync_file(Some(&src), &dst, "x").unwrap(), "first copy");
        assert!(!sync_file(Some(&src), &dst, "x").unwrap(), "already current");
        assert!(find_newest(&[main], "absent", "x.dll").is_none());
        assert!(find_newest(&[dir.join("no-such-folder")], "mozangle", "x.dll").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Servo's DLLs go beside its host in `target-servo/debug/`. With no such folder, or the folder with neither a host
    /// nor a DLL in any build, nothing happens; a DLL that can't be copied is listed and the other still copies; a
    /// built host with no DLL to give it is an error for each.
    #[test]
    fn servo_dlls_go_beside_its_host_and_a_failed_one_does_not_stop_the_rest() {
        let dir = scratch("servo");
        let builds = [dir.join("build")];
        for dll in SERVO_DLLS {
            touch(&builds[0].join("mozangle-aaa/out").join(dll), 10, dll.as_bytes());
        }
        let servo_debug = dir.join("target-servo/debug");
        let mut outcome = Outcome::default();
        sync_servo(&servo_debug, &builds, &mut outcome);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no target-servo/debug: skipped, {:?}", outcome.errors);

        let empty = dir.join("empty/target-servo/debug");
        fs::create_dir_all(&empty).unwrap();
        let mut outcome = Outcome::default();
        sync_servo(&empty, &[dir.join("no-build")], &mut outcome);
        assert!(!outcome.changed && outcome.errors.is_empty(), "no host and no DLL: skipped, {:?}", outcome.errors);

        fs::create_dir_all(servo_debug.join("libEGL.dll")).unwrap();
        let mut outcome = Outcome::default();
        sync_servo(&servo_debug, &builds, &mut outcome);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].starts_with("target-servo/debug/libEGL.dll: "), "{:?}", outcome.errors);
        assert!(outcome.changed, "libGLESv2.dll still copied");
        assert_eq!(fs::read(servo_debug.join("libGLESv2.dll")).unwrap(), b"libGLESv2.dll");

        let bare = dir.join("bare/target-servo/debug");
        fs::create_dir_all(&bare).unwrap();
        fs::write(bare.join(SERVO_EXE), b"host").unwrap();
        let mut outcome = Outcome::default();
        sync_servo(&bare, &[dir.join("empty-build")], &mut outcome);
        assert_eq!(outcome.errors.len(), 2, "{:?}", outcome.errors);
        assert!(outcome.errors.iter().all(|e| e.contains("build Servo first")), "{:?}", outcome.errors);
        let _ = fs::remove_dir_all(&dir);
    }

    /// CEF's runtime and its host go into `cef-runtime/`: a file that can't be copied is listed while every other
    /// file, the locales folder and the host exe still copy, and the sync then fails; once everything is current a
    /// rerun copies nothing and succeeds.
    #[test]
    fn a_cef_file_that_cant_be_copied_is_listed_and_the_rest_still_copy() {
        let td = scratch("cef");
        let out = td.join("build/cef-dll-sys-aaa/out/cef_windows_x86_64");
        for file in ["a.dll", "b.dll", "locales/en.pak"] {
            touch(&out.join(file), 10, file.as_bytes());
        }
        touch(&td.join("manifold-view-cef.exe"), 10, b"host");
        let runtime = td.join("cef-runtime");
        // A folder where a file should go: copying onto it fails on every run.
        fs::create_dir_all(runtime.join("a.dll")).unwrap();
        let mut outcome = Outcome::default();
        sync_cef(&td, &mut outcome);
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(outcome.errors[0].starts_with("cef-runtime/a.dll: "), "{:?}", outcome.errors);
        for file in ["b.dll", "locales/en.pak"] {
            assert_eq!(fs::read(runtime.join(file)).unwrap(), file.as_bytes(), "{file}");
        }
        assert_eq!(fs::read(runtime.join("manifold-view-cef.exe")).unwrap(), b"host");
        assert!(outcome.changed);
        let errors = outcome.errors.clone();
        assert_eq!(finish(outcome), Err(errors), "a failed step fails the sync");

        fs::remove_dir(runtime.join("a.dll")).unwrap();
        let mut outcome = Outcome::default();
        sync_cef(&td, &mut outcome);
        assert!(outcome.changed && outcome.errors.is_empty(), "a.dll now copies: {:?}", outcome.errors);
        let mut outcome = Outcome::default();
        sync_cef(&td, &mut outcome);
        assert!(!outcome.changed && outcome.errors.is_empty(), "all current: {:?}", outcome.errors);
        assert_eq!(finish(outcome), Ok(()));
        let _ = fs::remove_dir_all(&td);
    }

    /// A file Windows holds open (a DLL a running host loaded) is named, with what to do about it.
    #[test]
    fn a_held_file_says_to_close_manifold() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("held");
        let (src, dst) = (dir.join("src.dll"), dir.join("dst.dll"));
        fs::write(&src, b"new").unwrap();
        fs::write(&dst, b"old").unwrap();
        // FILE_SHARE_READ, as a loaded DLL allows: comparing reads it, and the copy over it is the step that fails.
        let held = fs::OpenOptions::new().read(true).share_mode(1).open(&dst).unwrap();
        let mut outcome = Outcome::default();
        outcome.step("target-servo/debug/dst.dll", sync_file(Some(&src), &dst, "dst"));
        assert_eq!(outcome.errors, ["target-servo/debug/dst.dll is in use: close manifold and run again"]);
        drop(held);
        let _ = fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 4 -p gate sync_deps`
Expected: a compile error: `Outcome`, `sync_servo`, `sync_cef`, `finish`, `SERVO_DLLS` and `SERVO_EXE` are not found, and `find_newest` expects `&Path`, found an array.

- [ ] **Step 3: Write the new sync**

Replace everything in `sync_deps.rs` above `#[cfg(test)]` (the module doc through `sync_runtime_deps`) with:

```rust
//! `gate sync-runtime-deps` (was scripts/sync-runtime-deps.sh): puts each engine host's runtime files beside the host
//! a dev tree runs (README "Runtime-file placement"). Cargo does not, and the fingerprint folder they are built into
//! can change on a rebuild that touched neither Cargo.toml nor Cargo.lock, silently staling an earlier copy.
//! - Servo's ANGLE DLLs go beside `target-servo/debug/manifold-view-servo.exe`, where manifold runs it, from the
//!   newest mozangle output in `target-servo/debug/build/` or `target/debug/build/`. With no `target-servo/debug/`
//!   they are skipped quietly; with the host there and no DLL to give it, that is an error.
//! - CEF's runtime and its host exe go into `target/debug/cef-runtime/`, the copy manifold runs.
//!
//! Safe to run any time (a no-op when everything is current); run it after every build of manifold-view-servo or
//! manifold-view-cef. Every step runs even when one fails: the failures are listed at the end and the exit code is 1,
//! and a file a running host holds open is named with "close manifold and run again". Not a build.rs: a build script
//! sees only its own crate's OUT_DIR.
//!
//! The source is the most recently *written* match, not the newest fingerprint directory, whose mtime cargo touches
//! on every build. Newer cargo nightlies put build-script output under `build/<crate>/<hash>/`, older ones under
//! `build/<crate>-<hash>/`, and a target dir built by both has each.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

/// Servo's whole runtime besides its exe (release.yml ships the same three files).
const SERVO_DLLS: [&str; 2] = ["libEGL.dll", "libGLESv2.dll"];
const SERVO_EXE: &str = "manifold-view-servo.exe";

fn say(msg: &str) {
    println!("sync-runtime-deps: {msg}");
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// `<build>/<krate>-*/out/<tail>` and `<build>/<krate>/*/out/<tail>` that exist, for each of `builds`, the newest by
/// mtime. A build folder that isn't there is skipped.
fn find_newest(builds: &[PathBuf], krate: &str, tail: &str) -> Option<PathBuf> {
    let mut found = Vec::new();
    for build in builds {
        for e in fs::read_dir(build).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("{krate}-")) {
                found.push(e.path().join("out").join(tail));
            } else if name == krate {
                for h in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                    found.push(h.path().join("out").join(tail));
                }
            }
        }
    }
    found.into_iter().filter_map(|p| Some((mtime(&p)?, p))).max().map(|(_, p)| p)
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dst)?;
        } else {
            fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}

/// Copies `src` over `dst` unless their bytes already agree; true if it copied.
fn sync_file(src: Option<&Path>, dst: &Path, label: &str) -> io::Result<bool> {
    let Some(src) = src else {
        eprintln!("sync-runtime-deps: no candidate found for {label}, skipping");
        return Ok(false);
    };
    if dst.is_file() && fs::read(src)? == fs::read(dst)? {
        say(&format!("{label} already current"));
        return Ok(false);
    }
    fs::copy(src, dst)?;
    say(&format!("recopied {label}"));
    Ok(true)
}

/// What the steps did: whether any copied, and a line for each that failed.
#[derive(Default)]
struct Outcome {
    changed: bool,
    errors: Vec<String>,
}

impl Outcome {
    /// Records one step's result; `dst` names the file it was writing.
    fn step(&mut self, dst: &str, result: io::Result<bool>) {
        match result {
            Ok(changed) => self.changed |= changed,
            Err(e) => self.errors.push(step_error(dst, &e)),
        }
    }
}

/// The line for a step that failed. A file Windows holds open (a DLL loaded by a running host: os error 32, 33 or
/// 1224) says to close manifold, the usual holder.
fn step_error(dst: &str, e: &io::Error) -> String {
    if matches!(e.raw_os_error(), Some(32 | 33 | 1224)) {
        format!("{dst} is in use: close manifold and run again")
    } else {
        format!("{dst}: {e}")
    }
}

/// Servo's DLLs beside its host in `servo_debug` (`target-servo/debug/`), from the newest mozangle output in `builds`.
fn sync_servo(servo_debug: &Path, builds: &[PathBuf], outcome: &mut Outcome) {
    if !servo_debug.is_dir() {
        say("no target-servo/debug, skipping Servo's DLLs");
        return;
    }
    let exe_there = servo_debug.join(SERVO_EXE).is_file();
    for dll in SERVO_DLLS {
        let label = format!("target-servo/debug/{dll}");
        match find_newest(builds, "mozangle", dll) {
            Some(src) => outcome.step(&label, sync_file(Some(&src), &servo_debug.join(dll), &label)),
            None if exe_there => outcome.errors.push(format!(
                "no mozangle build output holds {dll}, which {SERVO_EXE} needs: build Servo first (README \"Building \
                 Servo on its own\")"
            )),
            None => say(&format!("no mozangle build output holds {dll} and no {SERVO_EXE} is built, skipping")),
        }
    }
}

/// CEF's runtime and its host exe into `target_debug/cef-runtime/`, the copy manifold runs. The host exe is rebuilt
/// whenever manifold-view-cef changes, unlike the runtime: it is always compared.
fn sync_cef(target_debug: &Path, outcome: &mut Outcome) {
    let runtime = target_debug.join("cef-runtime");
    match find_newest(&[target_debug.join("build")], "cef-dll-sys", "cef_windows_x86_64") {
        Some(out) => {
            if let Err(e) = copy_runtime(&out, &runtime, outcome) {
                outcome.errors.push(step_error("cef-runtime", &e));
            }
        }
        None => eprintln!("sync-runtime-deps: no cef-dll-sys output directory found, skipping CEF runtime"),
    }
    let exe = target_debug.join("manifold-view-cef.exe");
    let label = "cef-runtime/manifold-view-cef.exe";
    outcome.step(label, sync_file(exe.is_file().then_some(exe.as_path()), &runtime.join("manifold-view-cef.exe"), label));
}

/// Each file of CEF's distribution `out` into `runtime`, recopied when it differs, and each directory (locales/) only
/// when missing or empty, since those don't change between rebuilds. A file or directory that fails is listed and the
/// rest still copy; only a folder that can't be created or listed stops it.
fn copy_runtime(out: &Path, runtime: &Path, outcome: &mut Outcome) -> io::Result<()> {
    fs::create_dir_all(runtime)?;
    for e in fs::read_dir(out)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let label = format!("cef-runtime/{name}");
        let dst = runtime.join(&name);
        if e.file_type()?.is_dir() {
            if fs::read_dir(&dst).map_or(true, |mut d| d.next().is_none()) {
                match copy_dir(&e.path(), &dst) {
                    Ok(()) => {
                        say(&format!("copied {label}/"));
                        outcome.changed = true;
                    }
                    Err(err) => outcome.errors.push(step_error(&label, &err)),
                }
            }
        } else {
            outcome.step(&label, sync_file(Some(&e.path()), &dst, &label));
        }
    }
    Ok(())
}

fn run() -> Result<(), Vec<String>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target_debug = root
        .join("target/debug")
        .canonicalize()
        .map_err(|_| vec!["target/debug does not exist - build first".to_string()])?;
    let servo_debug = root.join("target-servo/debug");
    let mut outcome = Outcome::default();
    sync_servo(&servo_debug, &[servo_debug.join("build"), target_debug.join("build")], &mut outcome);
    sync_cef(&target_debug, &mut outcome);
    finish(outcome)
}

/// What the sync reports once every step ran: each failure (so the exit code is 1), else success, saying so when
/// nothing needed copying.
fn finish(outcome: Outcome) -> Result<(), Vec<String>> {
    if !outcome.errors.is_empty() {
        return Err(outcome.errors);
    }
    if !outcome.changed {
        say("everything already current, nothing to do");
    }
    Ok(())
}

pub fn sync_runtime_deps() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for e in errors {
                eprintln!("sync-runtime-deps: {e}");
            }
            ExitCode::from(1)
        }
    }
}
```

rustfmt may rewrap the long `outcome.step(label, sync_file(...))` line in `sync_cef`; that is fine.

- [ ] **Step 4: Run the tests to see them pass**

Run: `cargo test -j 4 -p gate`, then the clippy line.
Expected: `test result: ok.` with the four `sync_deps` tests among them; no warning.

- [ ] **Step 5: Run the sync, then delete the copies nothing runs**

Do the start check ("One cargo per target dir"), then run `cargo build -j 4 -p gate` and `target/debug/gate.exe sync-runtime-deps`.
Expected:
- `sync-runtime-deps: recopied target-servo/debug/libEGL.dll` and `... recopied target-servo/debug/libGLESv2.dll` (the first run puts them there; `target-servo/debug/` holds no DLL at bfacedc2);
- the CEF lines as before (`cef-runtime/<file> already current`, and `recopied cef-runtime/manifold-view-cef.exe` if the flat exe differs from the copy);
- exit code 0.

Any `sync-runtime-deps: <file> ...` error line: stop and report it.

Then, in PowerShell, run `Get-Process manifold-view-servo -ErrorAction SilentlyContinue | Select-Object Id, Path`. A process whose `Path` is this repo's `target\debug\manifold-view-servo.exe` holds the file: ask the user to close manifold, and wait. Then from bash:

```bash
rm -- target/debug/manifold-view-servo.exe target/debug/libEGL.dll target/debug/libGLESv2.dll
target/debug/gate.exe sync-runtime-deps; ls target/debug | grep -ci "servo\|libEGL\|libGLESv2"
```

Expected: the second sync prints `target-servo/debug/libEGL.dll already current` (and the same for libGLESv2.dll) and exits 0; the `grep -c` prints `0`, so nothing put the deleted files back. If `rm` fails with a file in use, ask the user to close manifold and run it again; never force it.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git diff --stat
git add crates/gate/src/sync_deps.rs
git commit -F - <<'EOF'
gate: sync-runtime-deps puts Servo's DLLs beside its host in target-servo, and runs every step

The app now runs the dev Servo host from target-servo/debug, so its ANGLE
DLLs go there, from the newest mozangle output of either build folder; a
built host with no DLL is an error. Failures are collected so one locked
file no longer hides the rest, and a held file says to close manifold.
The stale target/debug copies of the host and its DLLs were deleted.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 9: Rebuild every host, and check the app's verdict on each

**Files:**
- Modify: `docs/runs.md` (rows run.ps1 writes).
- Create (scratchpad only): `<scratchpad>\viewstate.txt`, `<scratchpad>\servo-build-before.txt`.

**Interfaces:**
- Consumes: the headless `VIEWSTATE <engine>` command, which prints `view-state <name>: <state label> [<on_disk_text>]` to stderr (`debug_control.rs`, the `VIEWSTATE kind` row), with Task 6's `on_disk_text`; `gate sync-runtime-deps` (Task 8); the CEF guard's `build` mode (P23).
- Produces: all four hosts built from this tree and stamped, each reading Installed (D9, P26).

- [ ] **Step 1: Build the app and the hosts that build in `target/`**

Do the start check, and the first item of the pre-run check (`Get-Process cargo, gate, manifold`) too, since this step writes into `target/debug` and not only a `run.ps1` launch does: a `manifold` running from this repo's `target\debug` holds `manifold.exe` (and the host exes it started), and the link then fails on a locked file. Ask the user to close it; never stop it yourself. Then run the gate build line. Expected: it finishes with no error. This also rebuilds `target/debug/manifold-view-webview2.exe` stamped.

- [ ] **Step 2: Read the app's verdict before the rebuilds**

Write `<scratchpad>\viewstate.txt` with the Write tool, holding these five lines:

```text
VIEWSTATE servo
VIEWSTATE cef
VIEWSTATE webkit
VIEWSTATE webview2
QUIT
```

Do the pre-run check, then from bash (`APPDATA` is redirected into the run folder so the real `engine-hosts/` is never read, P26):

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name host-stamps-before -Command 'mkdir "%RUN_DIR%\appdata\manifold"& set "APPDATA=%RUN_DIR%\appdata"& target\debug\manifold.exe --headless --profile-dir "%RUN_DIR%\appdata\manifold" --script "<scratchpad>\viewstate.txt"'
```

It returns when the run ends. Set `RUN` to its folder as "Waiting for a run" says (`<Name>` is `host-stamps-before`), then run `grep -h view-state "$RUN/err.log"`.

The script has no `WAITTABS` before its first `VIEWSTATE`, on purpose: `VIEWSTATE` reads the engine's state from the disk when it runs and needs no tab (its handler, `debug_control.rs:447-456` at bfacedc2, prints `view_status` and `view_on_disk` and touches no tab; the module doc, `debug_control.rs:30-32`, says only the commands that need the focused tab must wait for it), and `WAITTABS` would hold the run for the first-launch demo tab's spawn, which can take 30 s or more headless. If a `view-state` line is missing from `err.log`, put `WAITTABS` first in the file and run again, as `gate package` does before its `VIEWSTATE` lines (`package.rs:149-155`), and say so in the report.

Expected (Servo's and CEF's hosts were built before this plan, WebKit's in Task 2, WebView2's in Step 1):

```text
view-state servo: Unavailable: the engine host carries no protocol number (built before hosts were stamped); rebuild the engine host [host unstamped]
view-state cef: Unavailable: the engine host carries no protocol number (built before hosts were stamped); rebuild the engine host [host unstamped, runtime ready]
view-state webkit: Installed [host ready]
view-state webview2: Installed [host ready]
```

Diff the Downloads folder. If a line differs, stop and report all four lines: the lookup or the stamp reads another file than this plan expects.

- [ ] **Step 3: Rebuild CEF's host and sync its copy**

Do the first item of the pre-run check again (`Get-Process cargo, gate, manifold`): the build writes `target/debug/manifold-view-cef.exe` and the sync below rewrites `target/debug/cef-runtime/manifold-view-cef.exe`, and a running dev `manifold` (or the CEF host it started) holds the exe. Ask the user to close it; never stop it yourself. Then run `powershell -NoProfile -File "<scratchpad>/cef-check.ps1" -Cmd build -Logs "<scratchpad>"`. Expected: exit 0. Exit 2 means the warm `cef-dll-sys` build is gone and the guard refused a cold one: stop and report it.

Then run `target/debug/gate.exe sync-runtime-deps`. Expected: `recopied cef-runtime/manifold-view-cef.exe`, no error line, exit 0. An error line `<file> is in use: close manifold and run again` is that same lock: ask the user to close manifold, then run it again.

- [ ] **Step 4: Rebuild Servo's host, watching for a cold start**

Save the build folders: `find target-servo/debug/build -mindepth 1 -maxdepth 2 -type d | sort > "<scratchpad>/servo-build-before.txt"` (two levels, so a new `<crate>/<hash>/` folder of the newer layout shows too). Do the pre-run check, then:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name servo-rebuild-stamp -Detach -Command 'set "CARGO_TARGET_DIR=target-servo"& cargo build -j 4 -p manifold-view-servo'
```

Wait for it with this loop, run as a background Bash call (`run_in_background: true`), deadline 900 s. It also stops waiting as soon as a new build folder appears:

```bash
RUN="$LOCALAPPDATA/claude-runs/manifold/<id>"; deadline=$((SECONDS+900)); until grep -Eq '"end": +"' "$RUN/run.json" || ! find target-servo/debug/build -mindepth 1 -maxdepth 2 -type d | sort | diff -q "<scratchpad>/servo-build-before.txt" - >/dev/null || [ $SECONDS -ge $deadline ]; do sleep 5; done; grep -E '"(exit|headline|end)"' "$RUN/run.json"; find target-servo/debug/build -mindepth 1 -maxdepth 2 -type d | sort | diff "<scratchpad>/servo-build-before.txt" -; tail -n 5 "$RUN/out.log" "$RUN/err.log"
```

Expected: `run.json`'s `"exit"` is `0` within a minute or two (a warm build: only `manifold-view-protocol` and the host relink), and the `diff` prints nothing.

If the `diff` lists a new folder, or the deadline passed with no `end`, cargo began rebuilding Servo's dependency tree (about 33 minutes and several GB, P38). Find the wrapper pid (`"notes"` in `run.json`), stop the run with `taskkill //T //F //PID <wrapper pid>`, close its row as lost (RUNS.md "Finding and closing"), and report the new folders. Delete nothing, skip the sync below, and go on to Step 5 without Servo.

Then run `target/debug/gate.exe sync-runtime-deps`. Expected: `target-servo/debug/libEGL.dll already current` and the same for libGLESv2.dll, no error line, exit 0.

- [ ] **Step 5: Rebuild WebKit's host**

Run the WebKit check. Expected: it builds and its tests pass.

- [ ] **Step 6: Read the app's verdict after the rebuilds**

Do the pre-run check, then run the Step 2 command again with `-Name host-stamps-after`, set `RUN` to that run's folder the same way, and `grep -h view-state "$RUN/err.log"`.
Expected:

```text
view-state servo: Installed [host ready]
view-state cef: Installed [host ready, runtime ready]
view-state webkit: Installed [host ready]
view-state webview2: Installed [host ready]
```

(Servo still reads `Unavailable ... [host unstamped]` if Step 4 stopped a cold build; report that.) Diff the Downloads folder.

If a host that was just rebuilt still reads `[host unstamped]` while Task 1's tests pass, stop and report it: the linker dropped the record from that host's link. The host's only reference to it is the generic `send_version`, and Task 1's tests prove the section survives in a test exe, not in each host's link. The fix is a design question for the owner (a non-generic reference, or the link attribute that retains a section), not a looser check.

- [ ] **Step 7: Load a page on Servo and CEF**

Do the pre-run check, then:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name host-stamps-live -Command 'set "VIEWS=servo cef"& target\debug\gate.exe page-text "%RUN_DIR%"'
```

(If Step 4 stopped a cold build, use `VIEWS=cef` and say so in the report.) Expected: `PASS` lines for `servo` and for `cef`, no `FAIL`, and `run.json`'s `"exit"` is `0`. A failed stage keeps its work folder: read its `log`, report the cause, then remove it (`RUN` set to `host-stamps-live`'s folder, then `rm -rf -- "$RUN"/gate-page-text-*`). Diff the Downloads folder.

- [ ] **Step 8: Commit the run rows**

Set each run's headline if run.ps1 left it empty (for the VIEWSTATE runs, the four `view-state` lines joined by `; `), then commit. The body below assumes Step 4 rebuilt Servo. If Step 4 stopped a cold build, replace the body's text after "(unstamped hosts);" with "after rebuilding the CEF, WebKit and WebView2 hosts each reads Installed, and a page loads on CEF. Servo's host was not rebuilt (its build went cold, servo-rebuild-stamp closed as lost) and still reads Unavailable (host unstamped)." The commit block is:

```bash
git diff --stat
git add docs/runs.md
git commit -F - <<'EOF'
runs: host-stamps-before, servo-rebuild-stamp, host-stamps-after, host-stamps-live

Before the rebuilds the app read Servo and CEF as Unavailable (unstamped
hosts); after rebuilding all four hosts each reads Installed, and a page
loads on Servo and CEF.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 10: The docs say where each host runs from and what a skewed host reads

**Files:**
- Modify: `README.md`:
  - a new paragraph before `**CI** (` (:33);
  - "## Building": the Windows crate list (:56), the build command (:59), a new Servo paragraph after the `PATH` filtering line (:62);
  - "### Runtime-file placement": the first sentence of :74, the first sentence of :82, a phrase in :86;
  - the release paragraph's no-pin sentence (:379, the bullet starting `- Releases run on GitHub Actions`);
  - `gate sleep` (:462), `gate trail` (:476), the Servo rebuild line (:492), "Building Servo on its own" (:497);
  - the engine states in the `**On-demand CEF install (2026-10-02; ...` bullet (:704): its Unavailable list and its dev-tree sentence.
- Modify: `crates/gate/src/all.rs:13-17` (the module doc's five-line `WITH_ENGINES` paragraph).
- Modify: `crates/manifold/src/host_pin.rs:7-8` (the module doc's no-manifest sentence, "unless a sibling host exists").
- Modify: the code comments that still describe a host in place as one beside `manifold.exe` (round 4; a dev Servo or WebKit host now runs from its dev folder, so "sibling host" is too narrow). Comments only:
  - `crates/manifold/src/process_manager.rs`: the `HostSource::Embedded` and `HostSource::Download` docs (:39-44);
  - `crates/manifold/src/view_install.rs`: `no_download_reason`'s doc (:56) and `OnDisk::host`'s (:72);
  - `crates/manifold/src/process_manager/availability.rs`: the module doc's first two lines and `host_on_disk`'s doc (:124-125);
  - `crates/manifold/src/process_manager/tests.rs`: the Servo availability test's doc (:569) and one comment in it (:585);
  - `crates/gate/src/package.rs`: the module doc's WebView2 bullet (:9).
- Modify: `docs/superpowers/plans/2026-10-11-bare-exe-host-downloads.md:39` (item 9).

The line numbers are at bfacedc2. Another workflow may edit README.md meanwhile: find each place by its quoted text, and stop and report if a quoted text is not there.

**Interfaces:**
- Consumes: the names Tasks 1-8 introduced (`.mfproto`, `stamp.rs`, `run_in_place`, `view_binary_path`'s TODO, `Skew`'s texts, `sync_servo`).
- Produces: README "Which host runs, and whether it speaks this build's protocol", the one home of this plan's behaviour; Task 11's PARKED section links to it.

- [ ] **Step 1: Add the behaviour paragraph**

In README.md, insert this paragraph and a blank line before the line that starts `**CI** (`.github/workflows/ci.yml`) runs on every push and pull request:`

```markdown
**Which host runs, and whether it speaks this build's protocol** (2026-10-29, plan `docs/superpowers/plans/2026-10-29-host-protocol-stamp.md`). A built host runs in place: from its dev build folder first (`target-servo/debug/` for Servo, `target-linux/debug/` for WebKit, found by walking up from `manifold.exe`'s folder), then from beside `manifold.exe` (CEF only as its `cef-runtime/` copy, the one a tab runs); otherwise the host downloaded or extracted into `engine-hosts/` runs (`run_in_place` and `view_binary_path` in `crates/manifold/src/process_manager/availability.rs`). Every host exe carries the protocol number it speaks, in a `.mfproto` section (`crates/manifold-view-protocol/src/stamp.rs`). The app reads it from the host that would run in place, never from `engine-hosts/`, whose files are hash-pinned. A host speaking another protocol, or carrying none (built before 2026-10-29), reads Unavailable wherever engines are listed and is refused before spawning, with one reason that names which side to rebuild: "the engine host speaks protocol 7, manifold speaks 8; rebuild the engine host", or "rebuild manifold" for a newer host. Its failed card says "unavailable (click for why)", and the click explains instead of offering a download. The handshake check stays as the backstop for a host the app does not read first: its failed card gives the same reason, and the tab is not restarted. In a dev tree with a custom CEF folder set, a tab runs a copy of the flat `target/debug/manifold-view-cef.exe` (when there is one) while the app reads the `cef-runtime/` copy, so a stale flat exe is caught only by the handshake, and a stale `cef-runtime/` copy reads Unavailable until `gate sync-runtime-deps` refreshes it (PARKED "Host protocol stamp").
```

- [ ] **Step 2: Fix the build and placement text**

In README.md:
- Replace `**Windows-side crates** (`manifold`, `manifold-view-servo`, `manifold-view-cef`, `manifold-view-webview2`)` with `**Windows-side crates** (`manifold`, `manifold-view-cef`, `manifold-view-webview2`, `manifold-view-pdfium`)`.
- In the command below it, replace ` cargo build -p manifold -p manifold-view-servo -p manifold-view-cef` with ` cargo build -p manifold -p manifold-view-cef`.
- After the line `The `PATH` filtering is only needed if `depot_tools` is on your PATH; harmless otherwise.`, insert a blank line, then this paragraph, a blank line, and a `bash` code block holding the one command below it:
  - paragraph: `**Servo engine-host** — built into its own target directory, which keeps its dependency tree out of `target/` (see "Building Servo on its own"); manifold runs it from there, and the sync puts its DLLs beside it:`
  - command: `CARGO_TARGET_DIR=target-servo cargo build -p manifold-view-servo && target/debug/gate.exe sync-runtime-deps`
- Replace `Cargo doesn't do this itself — see the `TODO` in `crates/manifold/src/process_manager.rs`'s `engine_binary_path`.` with `Cargo doesn't do this itself — see the `TODO` on `view_binary_path` in `crates/manifold/src/process_manager/availability.rs`, which also says where each host runs from.`
- Replace the first sentence of the paragraph after the `gate.exe sync-runtime-deps` block:

```markdown
It finds Servo's ANGLE DLLs and CEF's runtime under `target/debug/build/{mozangle,cef-dll-sys}-*/` (or `…/{mozangle,cef-dll-sys}/*/`, where newer cargo nightlies put build-script output; it takes the newest of either), `cmp`s each against the copy already in place, and only recopies what's actually stale (safe to run any time — a no-op if nothing changed).
```

with:

```markdown
It finds Servo's ANGLE DLLs under `{target-servo,target}/debug/build/mozangle-*/` and CEF's runtime under `target/debug/build/cef-dll-sys-*/` (or `…/<crate>/*/`, where newer cargo nightlies put build-script output; it takes the newest of either), and puts each beside the host manifold runs: Servo's beside `target-servo/debug/manifold-view-servo.exe`, CEF's, with the CEF host itself, in `target/debug/cef-runtime/`. It `cmp`s each against the copy already in place and only recopies what's actually stale (safe to run any time — a no-op if nothing changed). Every step runs even when one fails; the failures are listed at the end, and a file a running host holds open is named with "close manifold and run again".
```

- Replace `except whichever one `scripts/sync-runtime-deps.sh` just used — re-run the script afterward` with `except whichever one `gate sync-runtime-deps` just used — re-run it afterward`.

- [ ] **Step 3: Fix the run and Servo lines, the gate, host-pin and "sibling host" docs, and the old plan**

In README.md:
- Replace `run `scripts/sync-runtime-deps.sh` after building or the probe runs an old CEF engine.` with `run `target/debug/gate.exe sync-runtime-deps` after building or the probe runs an old CEF engine.` (P35).
- Replace ``ENGINE=servo` runs it on a Servo host copied beside `manifold.exe`, built with `CARGO_TARGET_DIR=target-servo`` with ``ENGINE=servo` runs it on the Servo host built in `target-servo`, which manifold finds there`.
- Replace `, then `cp target-servo/debug/manifold-view-servo.exe target/debug/`.` with `, then `target/debug/gate.exe sync-runtime-deps` (it puts Servo's DLLs beside the host in `target-servo/debug/`, where manifold runs it).`
- After `so there is little to trim; `target-servo` is git-ignored.` append ` manifold runs the host from `target-servo/debug/` (it walks up from `manifold.exe`'s folder, ahead of any copy beside the exe), so nothing is copied into `target/`; run `target/debug/gate.exe sync-runtime-deps` after each Servo build for its DLLs (2026-10-29). Build Servo only there: a Servo host built into `target/` would run from beside `manifold.exe` with no DLLs placed for it, and its tabs fail to start.` (P17)
- In the release bullet (starts `- Releases run on GitHub Actions`), replace `An engine whose host this build pins no download for, and that has no sibling host, reads Unavailable ("this build has no Servo download"); dev builds and Linux CI pin nothing and keep sibling lookup.` with `An engine whose host this build pins no download for, and that has no built host, reads Unavailable ("this build has no Servo download"); dev builds and Linux CI pin nothing and keep the in-place lookup ("Which host runs, and whether it speaks this build's protocol").`
- In the `**On-demand CEF install (2026-10-02; ...` bullet:
  - replace `and Unavailable (this OS cannot run it, or this build has nothing to download and no sibling host:` with `and Unavailable (this OS cannot run it, the host that would run speaks another protocol or carries none (2026-10-29, "Which host runs, and whether it speaks this build's protocol"), or this build has nothing to download and no built host:`;
  - replace `pins none for the platform and no sibling host exists, re-checked every 5 s)` with `pins none for the platform and no built host exists, re-checked every 5 s)`;
  - replace `A dev tree (`target/debug/cef-runtime/`, see Runtime-file placement) always reads Installed.` with `A dev tree (`target/debug/cef-runtime/`, see Runtime-file placement) reads Installed while its host speaks this build's protocol.`

In `crates/gate/src/all.rs`, replace the module doc's `WITH_ENGINES` paragraph:

```rust
//! `WITH_ENGINES="servo webkit"` adds an engine's steps to any mode, after `sleep-history`. `servo` adds `trail-servo`
//! and `sleep-servo` (cases history, crash, restart) and needs the Servo host built and placed beside `manifold.exe`
//! (README "Building Servo on its own"). `webkit` adds `trail-webkit` and `sleep-webkit`, needs the WebKit host built in
//! WSL, and takes `SITE_HOST=<ip>`, the WSL gateway as Windows sees it (`wsl.exe -d Ubuntu -- ip route`, the default
//! route's `via`; README "WebKit under the gate"), for those two steps only; without it they fail.
```

with (the same words after the Servo clause, rewrapped to 120 columns):

```rust
//! `WITH_ENGINES="servo webkit"` adds an engine's steps to any mode, after `sleep-history`. `servo` adds `trail-servo`
//! and `sleep-servo` (cases history, crash, restart) and needs the Servo host built in `target-servo` with its DLLs
//! beside it (`gate sync-runtime-deps`; README "Building Servo on its own"). `webkit` adds `trail-webkit` and
//! `sleep-webkit`, needs the WebKit host built in WSL, and takes `SITE_HOST=<ip>`, the WSL gateway as Windows sees it
//! (`wsl.exe -d Ubuntu -- ip route`, the default route's `via`; README "WebKit under the gate"), for those two steps
//! only; without it they fail.
```

In `crates/manifold/src/host_pin.rs`, replace the module doc's lines:

```rust
//! pins nothing, so its engines read Unavailable unless a sibling host exists. A debug build also reads the file named
//! by `MANIFOLD_HOST_PINS_FILE`, which is how the gate shows the download states.
```

with:

```rust
//! pins nothing, so its engines read Unavailable unless a host built in place (`run_in_place`: its dev build folder,
//! or beside `manifold.exe`) speaks this build's protocol. A debug build also reads the file named by
//! `MANIFOLD_HOST_PINS_FILE`, which is how the gate shows the download states.
```

(The dated decision `docs/decisions/2026-10-18-hosts-in-the-release.md:15` says the same as the old line and stays, P35.)

Nine more comments, in five files, call a host in place a "sibling host" or the lookup "sibling-binary". The word is still the name of `sibling_host` (P10, P40), but these comments use it for where a host is looked for, which is now "in place": the dev folder or beside the exe. Change the words and nothing else. In `crates/manifold/src/process_manager.rs`, replace the `HostSource` docs:

```rust
    /// Carried inside `manifold.exe` when the build embeds it (`MANIFOLD_WEBVIEW2_HOST`, see `embedded_host`) and
    /// extracted at spawn when no sibling host is there. A build that embeds nothing finds it like a sibling.
    Embedded(embedded_host::Embedded),
    /// Pinned in `host_pin` and installed on demand into `engine-hosts/` (`view_install`), unless a sibling
    /// host (a dev tree's, or a Linux package's) is already there.
    Download,
```

with:

```rust
    /// Carried inside `manifold.exe` when the build embeds it (`MANIFOLD_WEBVIEW2_HOST`, see `embedded_host`) and
    /// extracted at spawn when no host is in place (`run_in_place`). A build that embeds nothing finds it there like
    /// any other.
    Embedded(embedded_host::Embedded),
    /// Pinned in `host_pin` and installed on demand into `engine-hosts/` (`view_install`), unless a host is already in
    /// place (a dev tree's build, or a Linux package's).
    Download,
```

In `crates/manifold/src/view_install.rs`, replace `a release writes it): there is nothing to fetch and no sibling host.` with `a release writes it): there is nothing to fetch and no host in place.`, and replace `` `host_state`, or Ready when a sibling host is there. `` with `` `host_state`, or Ready when a host is in place. ``

In `crates/manifold/src/process_manager/availability.rs`, replace the module doc's first two lines:

```rust
//! Where each engine host and runtime lives on disk, and whether an engine can run here: the sibling-binary
//! lookup, the cached on-disk check, and the availability shown in the engine lists.
```

with:

```rust
//! Where each engine host and runtime lives on disk, and whether an engine can run here: the in-place host
//! lookup, the cached on-disk check, and the availability shown in the engine lists.
```

and `host_on_disk`'s doc:

```rust
/// The host part of `kind` on disk: Ready when a sibling host is there, else what the `<kind>.host` marker says
/// about this build's pinned files (`host_state`; Missing when it pins none).
```

with:

```rust
/// The host part of `kind` on disk: Ready when a host is in place (`sibling_host`), else what the `<kind>.host`
/// marker says about this build's pinned files (`host_state`; Missing when it pins none).
```

In `crates/manifold/src/process_manager/tests.rs`, replace `(a sibling host counts, whatever the pin` with `(a host in place counts, whatever the pin` in the Servo test's doc, and `// Nothing pinned and no sibling: Unavailable,` with `// Nothing pinned and no host in place: Unavailable,` in its body. In `crates/gate/src/package.rs`, replace `(no sibling host exists);` with `(no host is in place);`.

What stays, on purpose: the names `sibling_host`, `sibling_binary` and `cef_sibling_exe`; `prepare_webview2`'s "A sibling host wins" (WebView2 has no dev folder, so its host in place is literally beside the exe); `runtime_on_disk`'s "a bundled sibling `cef-runtime/`" (CEF's runtime is beside the exe); and `probes.rs`' comment, which Task 7 rewrites.

In `docs/superpowers/plans/2026-10-11-bare-exe-host-downloads.md`, at the end of item 9 (the line ending `with exact-pin hosts a mismatch cannot occur from a download, so it stays).`), append ` (Closed 2026-10-29, plan `2026-10-29-host-protocol-stamp`: a mismatch is now a "can't start" that names which side to rebuild, and the app refuses such a host before spawning it.)`

- [ ] **Step 4: Check nothing still says the old thing**

Run:

```bash
grep -rnE "PROTOCOL_MISMATCH|check_version|built_webkit_host|cp target-servo|engine_binary_path|placed beside .manifold\.exe|scripts/sync-runtime-deps\.sh|copied beside .manifold\.exe|render protocol mismatch: rebuild|always reads Installed|keep sibling lookup|nothing to download and no sibling host|unless a sibling host exists|no sibling[: ]|a sibling host (is there|counts)|unless a sibling$|the sibling-binary$" crates README.md PARKED.md CLAUDE.md ROADMAP.md docs/decisions docs/superpowers/specs | cut -c1-160
```

Expected: these lines only, each a dated record left as it is (P35):
- `crates/gate/src/sync_deps.rs:1` (its "(was scripts/sync-runtime-deps.sh)" history);
- README's dated 2026-10 disk-full entry (`**Real disk-full crisis hit mid-session**`, :702 at bfacedc2);
- `docs/superpowers/specs/2026-10-02-engine-install-design.md` :60, :76 and :120;
- `docs/superpowers/specs/2026-10-12-file-renderers-design.md:78` (the render mismatch's old wording);
- `docs/superpowers/specs/2026-10-14-trail-design.md:258` (a 2026-10-15 measurement on a copied Servo host).

Fix any other line it lists the same way. (At bfacedc2 this grep lists those five spec lines and nothing under `docs/decisions`; the README and `crates` lines it lists there are the ones Tasks 2, 4 and 7-10 change. The last four alternatives are round 4's: at bfacedc2 they list only the comments Step 3 rewrites, in `process_manager.rs`, `view_install.rs`, `availability.rs`, `process_manager/tests.rs` and `gate/src/package.rs`, plus the README lines it already fixes.)

Then run `cargo fmt --all --check` and the clippy line. Expected: no fmt output, no warning.

- [ ] **Step 5: Commit**

```bash
git diff --stat
git add README.md crates/gate/src/all.rs crates/gate/src/package.rs crates/manifold/src/host_pin.rs crates/manifold/src/process_manager.rs crates/manifold/src/process_manager/availability.rs crates/manifold/src/process_manager/tests.rs crates/manifold/src/view_install.rs docs/superpowers/plans/2026-10-11-bare-exe-host-downloads.md
git commit -F - <<'EOF'
docs: where each host runs from, and what a host speaking another protocol reads

README gains the one paragraph on the in-place lookup, the .mfproto stamp
and the Unavailable refusal; the build and placement text drops the
manual Servo copy for target-servo and the new sync; host_pin.rs's doc
and the comments that called a host in place a "sibling host" name the
in-place lookup. Plan 2026-10-11 item 9 is closed.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 11: PARKED holds what this plan leaves

**Files:**
- Modify: `PARKED.md`: a new section at the end; the GATE-CEF-DIR entry's cites (:61 at bfacedc2).

**Interfaces:**
- Consumes: README "Which host runs, and whether it speaks this build's protocol" (Task 10), which the section links to rather than restates.
- Produces: the PARKED "Host protocol stamp" section that README names.

- [ ] **Step 1: Find the lines the entries cite**

Each entry's evidence is `file:line` with the date it was verified (VERSIONING.md), and Tasks 2-10 have moved the code since this plan was drafted. Run:

```bash
grep -n "child.wait()" crates/manifold-view-protocol/src/render.rs
grep -n "^fn prepare_cef\|^fn shipped_cef_host\|^fn ensure_host_copy" crates/manifold/src/process_manager.rs
grep -n "^fn in_place_paths" crates/manifold/src/process_manager/availability.rs
sed -n 18p crates/manifold/src/pdf_render.rs; sed -n 11p crates/manifold-view-protocol/src/render.rs
grep -n "pub fn export_state" crates/gate/src/instance.rs
```

Expected: one `child.wait()` line (in `run_child`); the three `fn` lines of `process_manager.rs`; one `fn in_place_paths` line; `const WORKERS: usize = 2;`; `pub const RENDER_PROTOCOL_VERSION: u32 = 1;`; one `export_state` line (`instance.rs:259` at bfacedc2). Note the numbers: the entries below cite them, and `pdf_render.rs:18`, `render.rs:11` and `instance.rs:259` are the ones to re-check: no task edits `pdf_render.rs` or `instance.rs`, and Task 3 adds to `render.rs` only after line 12 (`RENDER_MISMATCH`, after `MAGIC`), so `render.rs:11` should not have moved. Stop and report any grep that prints nothing or more than expected.

- [ ] **Step 2: Add the section**

Append at the end of PARKED.md, after the last section (at bfacedc2, `## Chrome shots (2026-10-28)`; if another workflow has added one since, after that), a blank line and the block below. Replace each `LINE-…` marker with the number Step 1 printed for that item (`LINE-WAIT` the `child.wait()` line, `LINE-PREPARE`, `LINE-SHIPPED` and `LINE-ENSURE` the three `process_manager.rs` functions, `LINE-PATHS` the `in_place_paths` line). If Step 1 printed another number for `pdf_render.rs`, `render.rs:11` or `instance.rs`, use that one. Then `grep -c "LINE-" PARKED.md` must print `0`.

```markdown
## Host protocol stamp (2026-10-29)

How hosts are found and checked: README "Which host runs, and whether it speaks this build's protocol".

- **The PDF renderer carries no stamp.** *What:* `manifold-view-pdfium` is not stamped and the app reads no number from it before spawning; a renderer speaking another render protocol is caught only by `read_frame`'s handshake, whose reason names the side to rebuild. *Why parked:* `RENDER_PROTOCOL_VERSION` has never changed, and the renderer is built with the app by the one build line (plan `2026-10-29-host-protocol-stamp`, D2). *Trigger:* the first `RENDER_PROTOCOL_VERSION` bump. *Fix:* stamp the renderer the way the engine hosts are (a record its frame writer reads its version from), and read it before spawning through the same reader. *Evidence:* `crates/manifold-view-protocol/src/render.rs:11` (`RENDER_PROTOCOL_VERSION = 1`), verified 2026-10-29. *Status:* parked.
- **A renderer that writes a bad frame and then hangs holds a worker.** *What:* `run_child` leaves a child that wrote a bad frame (other than a protocol mismatch, which it kills) to exit by itself, so its exit status can say why, and `child.wait()` has no deadline; a renderer that hangs after a bad frame holds one of the two render workers until it exits. *Why parked:* never seen; pdfium exits after writing. *Trigger:* a PDF card stuck rendering while its renderer process stays alive. *Fix:* wait with the same `timeout`, then kill. *Evidence:* `crates/manifold-view-protocol/src/render.rs:LINE-WAIT` (`run_child`'s `child.wait()`), `crates/manifold/src/pdf_render.rs:18` (`WORKERS = 2`), verified 2026-10-29. *Status:* parked.
- **A gate helper that reads an engine's state.** *What:* a helper reading `view_kinds[<engine>].state` from `export_state`, so a stage whose engine is Unavailable fails naming that state rather than with a missing tab. *Why parked:* the refusal and its reason are already in the stage's log (`spawn tab failed for <id>: <engine> is unavailable: ...`; plan `2026-10-29-host-protocol-stamp`, D7). *Trigger:* a stage failure whose cause took more than reading its log to find. *Fix:* the stage reads that state after `export_state` and fails naming it. *Evidence:* `crates/gate/src/instance.rs:259` (`export_state`), verified 2026-10-29. *Status:* parked.
- **Hosts outside the in-place lookup are not stamp-checked.** *What:* the app reads the protocol number only from the host `run_in_place` finds (for CEF, its `cef-runtime/` copy). Two hosts that can run are outside it: the host copy in a custom ("Use your own") CEF folder (`prepare_cef` copies `shipped_cef_host()` there through `ensure_host_copy`; in a dev tree that is the flat `target/debug/manifold-view-cef.exe` when it exists), and a dev tree's flat exe when no `cef-runtime/` copy exists. With a custom CEF folder in a dev tree this cuts both ways. A skewed flat exe beside a current `cef-runtime/` copy starts, fails the handshake and leaves a failed card with the same reason; it is not restarted. A stale `cef-runtime/` copy beside a current flat exe reads Unavailable although the tab would start, until `gate sync-runtime-deps` refreshes the copy. *Why parked:* by decision (plan `2026-10-29-host-protocol-stamp`, D4, P20, P28): the handshake backstop covers the first, the sync every CEF build needs anyway clears the second, and checking the exe a custom folder runs would add a second, CEF-only lookup. *Trigger:* a report of a custom-CEF tab failing on a protocol mismatch, or of CEF reading Unavailable with a custom folder set and a current host. *Fix:* read `shipped_cef_host()`'s stamp when a custom runtime is set (a per-kind CEF branch in the check). *Evidence:* `crates/manifold/src/process_manager.rs:LINE-PREPARE` (`prepare_cef`), `:LINE-SHIPPED` (`shipped_cef_host`) and `:LINE-ENSURE` (`ensure_host_copy`), and `crates/manifold/src/process_manager/availability.rs:LINE-PATHS` (`in_place_paths`, the lookup the check reads), verified 2026-10-29. *Status:* decided against.
```

- [ ] **Step 3: Re-cite GATE-CEF-DIR**

Run `grep -n "cef-runtime" crates/gate/src/*.rs | cut -c1-80` and `grep -n "pub(crate) const CEF_RUNTIME_DIR" crates/manifold/src/process_manager.rs`.
Expected (after Tasks 7 and 8): `fresh.rs:58` and `fresh.rs:62` (Task 7's comment is one line longer), `jar.rs:744`, `package.rs:133`, `shots.rs:1630`, several `sync_deps.rs` lines, and `process_manager.rs:48`. Use the numbers the grep prints if they differ.

In the GATE-CEF-DIR entry, replace `The `gate` crate spells `cef-runtime` five times (`fresh.rs:57`, `jar.rs:748`, `package.rs:133`, `shots.rs:1384`, `sync_deps.rs:84`) while `manifold` has `CEF_RUNTIME_DIR` (`process_manager.rs:46`).` with `The `gate` crate spells `cef-runtime` in five files (`fresh.rs:58`, `jar.rs:744`, `package.rs:133`, `shots.rs:1630`, `sync_deps.rs` `sync_cef`; re-cited 2026-10-29) while `manifold` has `CEF_RUNTIME_DIR` (`process_manager.rs:48`).`, using the numbers from the grep, and replace `*Evidence:* the five lines above.` with `*Evidence:* the five files above.`

- [ ] **Step 4: Commit**

```bash
git diff --stat
git add PARKED.md
git commit -F - <<'EOF'
docs: PARKED: what the host protocol stamp leaves; GATE-CEF-DIR re-cited

The PDF renderer's stamp, a renderer hanging after a bad frame, a gate
helper for engine state, and the hosts outside the in-place lookup
(decided against). GATE-CEF-DIR's cites had gone stale.

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

---

### Task 12: Final checks and the full gate

**Files:**
- Modify: `docs/runs.md` (the gate row, and the `sleep-history-rerun-stamp` row if Step 3 ran it), `docs/superpowers/plans/2026-10-29-host-protocol-stamp.md` (front matter `status: done`).

**Interfaces:**
- Consumes: everything above.
- Produces: the gate run `gate-host-stamp`, and the plan marked done.

- [ ] **Step 1: Run every local check**

Do the start check, then run `cargo fmt --all --check`, the clippy line and the four test lines. Expected: no fmt output, no warning, `test result: ok.` from each.

Then run the CEF guard with `-Cmd check` and with `-Cmd test`, and the WebKit check. Expected: exit 0 from both guard runs; WebKit builds and its tests pass. Report exit 2 or 3 as "not checked" or "compiled, not run"; never work around it.

- [ ] **Step 2: Pick the gate tier**

Run `target/debug/gate.exe tier <B>`, with `<B>` the commit Task 1 recorded. Expected: its last line is `TIER full` (engine and session files changed). Run `full` whatever it prints (CLAUDE.md "Gate"; P27).

- [ ] **Step 3: Run the full gate**

Do the pre-run check, then from bash:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name gate-host-stamp -Detach -Command 'cargo build -j 4 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'
```

Wait for it (see "Waiting for a run", deadline 4500 s). Then:

```bash
tail -n 5 "$RUN/gate-log.txt"; grep -E '"(exit|conditions)"' "$RUN/run.json"
grep -E '^GATE [^ ]+ exit=' "$RUN/gate-log.txt" | grep -v ' exit=0 ' | cut -c1-200
```

Expected: `run.json`'s `"exit"` is `0` and the last grep prints nothing (every stage line reads `exit=0`). Set the row's headline from the log's last lines (CLAUDE.md "Gate"). Keep the CPU load and Defender state from `conditions` for the report. Diff the Downloads folder.

The previous full gate on this tree family (`2026-10-28-gate-download-failure`, docs/runs.md) ended with `"exit"` 1 and 18 of 20 stages, for two reasons this plan does not touch. So `"exit"` 1 is the expected result only when the last grep lists nothing but these two stages and each shows its signature; it is then reported, not fixed here and not rerun under a new name, and the row's headline names the failed stages and the rerun id, as the earlier row's does:
- `shots-aa`: PARKED.md "`shots-aa` can differ by a few pixels where edges overlap" (held for the ordered `Nodes` container). Its signature, in `$RUN/gate-log.txt.shots-aa.log`: the lines before `FAIL: the two runs' shots match pixel for pixel` list only shots that differ by a few pixels (`<shot>.png: N px, max M, x ..`, N between 2 and 8 in the earlier runs; `json-zoom-in`, 8 px, max 3 on 2026-10-28), and the summary line ends `0 missing in B; 0 only in B`. A `size ... vs ...` line, a `missing in B` or `only in B` count above 0, or a shot with no edges crossing in it is a regression.
- `sleep-history`: a timing flake under load. Its signature: its `GATE sleep-history exit=1` line in `gate-log.txt` has `took=` far over the 6 s a quiet run takes, and the CPU was high around its start (`gate-log.txt.cpu` holds `<epoch seconds> <CPU %>` samples; the start epoch is the fourth whitespace-separated field of the `TIMING gate/sleep-history <took> <start epoch>` line, `awk '{print $4}'`; the earlier run's line was `TIMING gate/sleep-history 194 1793247441`). The earlier run: 100%, 194 s, the second Back stayed on p2. Rerun it alone three times, as `2026-10-28-sleep-history-rerun` did (about 20 s; do the pre-run check first):

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name sleep-history-rerun-stamp -Command 'set "VIEWS=webview2"& set "CASES=history"& (target\debug\gate.exe sleep && echo run1 PASS || echo run1 FAIL)& (target\debug\gate.exe sleep && echo run2 PASS || echo run2 FAIL)& (target\debug\gate.exe sleep && echo run3 PASS || echo run3 FAIL)'
```

  It is the known flake only if the three runs print `run1 PASS`, `run2 PASS` and `run3 PASS` in `out.log`. Any `FAIL` there is a regression.

Any other failing stage, or either of these two without its signature, is a regression: read its work folder (`<run folder>/gate-<stage>-<pid>`) and its lines in `gate-log.txt`, report the cause, fix it in the task that owns the code, and run the gate again under a new name (`gate-host-stamp-2`). Never mark a failed step as passed.

- [ ] **Step 4: Mark the plan done and commit**

In `docs/superpowers/plans/2026-10-29-host-protocol-stamp.md`, change the front matter's `status: active` to `status: done`. Then:

```bash
git diff --stat
git add docs/runs.md docs/superpowers/plans/2026-10-29-host-protocol-stamp.md
git commit -F - <<'EOF'
runs: gate-host-stamp; plans: host protocol stamp done

<attribution>
EOF
git log -1 --format=%B | tail -n 1
```

- [ ] **Step 5: Check for leftovers and report**

In PowerShell, run `Get-Process python, manifold, gate, msedgewebview2, cargo, manifold-view-servo, manifold-view-cef, manifold-view-webview2, manifold-view-pdfium, wsl -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, StartTime, Path`. Anything this plan's runs started (a `Path` in this repo or a run folder, a start time inside a run) is a leftover: report it. Leave the user's own processes alone (`wsl` may be the user's own sessions: report one only when its start time is inside a run).

WebKit's host and the WSL builds run inside the distro, where `Get-Process` does not see them. Run `wsl.exe -d Ubuntu -e pgrep -af "[m]anifold-view-webkitgtk|[c]argo"` (`-e` runs it without a shell, and the bracketed first letters keep the pattern from matching a command line that contains it). Expected: nothing (exit 1). Report anything it lists with its command line; leave the user's own processes alone.

The report says:
- what is committed locally (each commit's subject), and that nothing is pushed, tagged or bumped;
- the CEF guard and WebKit results, and the ELF evidence: Task 1 Step 5's WSL run of the stamp tests (a real linked ELF test exe that keeps the section only through `send_version`'s read), Task 2's and Task 9's WebKit builds, and Task 9 Step 6's verdict on the rebuilt WebKit host. On a Windows-only machine the ELF half of the parser is otherwise covered by synthetic images and CI's Linux job;
- the gate run id, its result (exit 0, or exit 1 with only `shots-aa` and `sleep-history` failing and each showing the signature in Step 3, with the rerun ids), and the CPU load and Defender state it recorded;
- the P10 and P22 answers this was built on;
- the architecture check: what grew special cases (the failed-spawn stage builds an unloadable copy; `sync_servo` and `sync_cef` are two hand-written placements of runtime files) and what would remove them (one table naming each host's runtime files and where they go, read by both the app's lookup and the sync).

---

## Review rounds

**Round 1 (2026-10-29; lenses code, tests, complete).** Each Important finding was checked against the code at bfacedc2 before it was applied.

- Code, Minor, Task 10 cites `all.rs:13-15` for a five-line paragraph: applied, the cite is `:13-17` (`sed -n 13,17p` prints the whole `WITH_ENGINES` paragraph).
- Code, Minor, `"exit": 0` with one space in Tasks 7, 9 and 12: applied. The four Expected lines now say "`run.json`'s `"exit"` is `0`", and "Waiting for a run" notes that `ConvertTo-Json` writes two spaces (`2026-10-28-servo-build-settings/run.json` holds `"exit":  0,`).
- Tests, Important, Task 5's `host_skew` glue untested: applied. `skew_of(exe, cache)` is split out, and `an_exe_is_judged_by_the_number_it_carries` tests it on the test's own exe and on an unstamped temp file (P41). It holds because the manifold test exe calls `send_version` (`process_manager/tests.rs:77`), so it is stamped on ELF too. `read_on_disk` filling `skew` stays pinned only live, by Task 9 Step 2, and the task says so.
- Tests, Important, Task 1's stamp test passes on MSVC even if `send_version` stops reading `STAMP`: applied. The test now asserts the bytes `send_version` writes (`into_inner` exists on `MessageChannel`, channel.rs:65). Step 5 runs the stamp tests in WSL, and the step names Linux (CI `ci.yml:24`, and that WSL run) as the proof that the section is kept. On Windows no test can prove that, as the step says.
- Tests, Minor, the card body's removed " (click to download)" unpinned: applied. `missing_view_text` builds the whole body text, its test pins two full strings, and Step 4 greps `tab_body.rs` (P42).
- Tests, Minor, `unloadable_copy` untested: applied. The pure `unloadable(image)` gets a test on a synthetic PE image (only the two magic bytes change; a non-PE file, a missing signature or a cut magic is `None`), P44. The os error 193 claim is not rerun here: Step 4's stage run proves it, because its new check needs an `(os error ` line.
- Tests, Minor, Task 2 Step 3 says "until Step 6": applied, it says Step 7.
- Tests, Minor, the file-renderers spec's old mismatch text missed by the grep: applied to the grep. It now covers `docs/superpowers/specs` and `docs/decisions` with `render protocol mismatch: rebuild` in the pattern. The spec line is listed as a dated record (`status: done`) and not edited (P35).
- Tests, Minor, `run_child`'s kill on a mismatch unpinned: applied, with the stub's `mismatch-linger` mode and `a_mismatched_renderer_that_lingers_is_killed` (P43).
- Complete, Important, README :704 ("always reads Installed", an Unavailable list without the skew) and :379 ("no sibling host", "keep sibling lookup") left false: applied. Task 10 Step 3 edits both by quoted text, the Files list names them, and Step 4's pattern gains `always reads Installed|keep sibling lookup|nothing to download and no sibling host` (none of these matches anything else in `crates`, PARKED, CLAUDE.md or ROADMAP at bfacedc2; the finished plan `2026-10-11-bare-exe-host-downloads.md:36` matches and stays as history, P35).
- Complete, Minor, the grep skips `docs/decisions` and the specs: applied to the grep. The decision `2026-10-18-hosts-in-the-release.md:15` and the matching `status: done` spec lines are recorded in P35 as dated records and are not edited.
- Complete, Minor, README :74's ANGLE collision rationale "no longer true": rejected. The pinned CEF build (152.0.6, `cef_pin.rs:8`) ships both DLLs: `target/debug/build/cef-dll-sys/1350bf2d9e1a185f/out/cef_windows_x86_64/libEGL.dll` and `target/debug/cef-runtime/libEGL.dll` exist. README :704's "CEF 154's archive has neither" is about 154 (spec 2026-10-02 :120), not this pin. So the collision is still why CEF lives in `cef-runtime/`, the reason Task 4's `beside_exe` doc gives too.
- Complete, Minor, a Servo host built into `target/` gets no DLLs: applied as a statement in P17 and in Task 10's "Building Servo on its own" sentence. The optional `sync_servo` error for such an exe is not added: it would check one wrong layout that the docs already rule out (P17's alternative).

**Round 2 (2026-10-29; lenses code, tests, complete).** The Important finding and each Minor one were checked against the code at bfacedc2 (`git show`, `git grep`; nothing built or run) before they were applied.

- Code, Minor, Task 8 says `sync_deps.rs` is 156 lines: applied, 155 (`git show bfacedc2:crates/gate/src/sync_deps.rs | wc -l`).
- Tests, Important, Task 8's CEF half (`sync_cef`, `copy_runtime`) and the exit-1 mapping untested: applied. `a_cef_file_that_cant_be_copied_is_listed_and_the_rest_still_copy` blocks `cef-runtime/a.dll` with a folder and pins one listed error, `b.dll`, `locales/` and the host exe still copied, `changed`, and a failing `finish`. `finish(outcome)` is split out of `run` (P18). One correction to the suggested test: after the blocker is removed, the next sync copies `a.dll` (it never copied before), so it asserts `changed`; only a third sync is all current (`!changed`, `finish` is `Ok`).
- Tests, Minor, nothing pins `read_on_disk` filling `skew`: applied. `read_on_disk_at(kind, in_place, cache)` is the seam, tested with WebView2 (`downloads` is false for it, `view_install.rs:128`, so nothing else is read) on the test exe, an unstamped temp file and no host. Round 1's `skew_of` and `host_skew` are folded into it, since with the host passed in neither had a second caller, and `an_exe_is_judged_by_the_number_it_carries` becomes `the_disk_check_judges_the_host_in_place_by_the_number_it_carries` (P41, P12, Task 2's reader doc now names `OnDisk::skew`).
- Tests, Minor, `sync_servo`'s quiet arm untested and dead `cfg(windows)` conditions: applied. The servo test adds a folder with no host and no DLL (no error, `!changed`). `cfg!(windows) &&` in `step_error` and the held-file test's `#[cfg(windows)]` are dropped (`main.rs:66-67` declares `#[cfg(windows)] mod sync_deps;`). The held file is now opened with `share_mode(1)` (FILE_SHARE_READ, as a loaded DLL allows), so the compare reads it and the copy over it is the step that fails, as for a real DLL. Errors 33 and 1224 stay unpinned: they come from a byte-range lock and a user-mapped section, not from a loaded DLL, and `matches!` on the three codes is one pattern, so a test per code would add little.
- Tests, Minor, Task 9 Step 8's fixed commit message: applied. Step 8 gives the body to use when Step 4 stopped a cold Servo build.
- Complete, Minor, P28 is false for CEF with a custom runtime: applied. `prepare_cef` runs `ensure_host_copy(&dir, &shipped_cef_host())` once `host_home` returns the custom dir, and `shipped_cef_host` is `sibling_host(ViewKind::Cef)` (flat first, `process_manager.rs:632`), while the check reads `cef-runtime/`. P28, the comment in `availability_with`'s skew branch (Task 5 Step 4), the PARKED gap entry (Task 11) and README's paragraph (Task 10 Step 1) now state both directions: a stale `cef-runtime/` copy reads Unavailable beside a current flat exe, and a stale flat exe is caught only by the handshake.
- Complete, Minor, `host_pin.rs:7` ("unless a sibling host exists") goes stale: applied in Task 10 (Files, Step 3, `git add`, the commit body), and Step 4's pattern gains `unless a sibling host exists`. At bfacedc2 it matches only `host_pin.rs:7`: the decision `2026-10-18-hosts-in-the-release.md:15` wraps after "sibling host", and it stays as a dated record (P35).
- Complete, Minor, the sync_deps line count (filed under Task 7; the claim is in Task 8): applied with the code finding above.

**Round 3 (2026-10-29; lenses code, tests, complete).** The Important finding was checked against `docs/runs.md` and `PARKED.md`, and each Minor one against the code at bfacedc2 (`git show`, grep; nothing built or run). No finding needed a new architectural choice, so there is no new decision.

- Complete, Important, Task 12 Step 3 expects `"exit"` 0 though the baseline gate fails twice: applied, and it holds. `docs/runs.md:365` (`2026-10-28-gate-download-failure`: exit 1, 18 of 20 stages; `sleep-history` failed once at 100% CPU and passed 3 of 3 alone, row :366) and `PARKED.md:147` (`shots-aa`, held for the ordered `Nodes` container) are both there. Step 3 now lists the stages whose `GATE <stage> exit=` is not 0, names each known failure's signature (`shots-aa`: a few differing pixels in shots and no missing or extra one; `sleep-history`: a long `took=` with high CPU around its start, and 3 of 3 passes alone by the rerun command copied from row :366), treats exit 1 as expected only then (reported, not fixed here, not rerun under a new name), and keeps every other failure a regression fixed in its owning task. Step 5's report says which of the two showed. Task 9 Step 7 is unchanged: it runs one stage (`page-text`), which has no known failure.
- Complete, Minor, Task 11's entries are not in the house format: applied. A new Step 1 greps the lines the entries cite (the other steps move down one); `LINE-…` markers stand for the numbers it prints and `grep -c "LINE-" PARKED.md` must print 0. The entries without a *Fix:* gain one (the pdfium stamp, the gate helper, the hosts outside the lookup); the renderer-hang entry had one.
- Code, Minor, two Task 11 entries cite symbols, not `file:line`: applied with the finding above (the `child.wait()` line and `prepare_cef`, `shipped_cef_host`, `ensure_host_copy`, `in_place_paths`). `pdf_render.rs:18`, `render.rs:11` and `instance.rs:259` were read at bfacedc2 and are re-checked by Step 1 (round 4 corrected the reason given here: no task edits `pdf_render.rs` or `instance.rs`, and Task 3 touches `render.rs` only after line 12).
- Code, Minor, Task 7 Step 2's Expected says "the log line": applied. The grep prints a `spawn tab failed for <id>` line per failed spawn (`spawn_result.rs:169` logs each; the stage spawns at startup, on each New and on Retry), so Expected says one per failed spawn and each reads the unstamped text. The "is unavailable" text there is the intermediate state this RED run shows on purpose (the new refusal), not the old behaviour.
- Code, Minor, `retry_missing_file`'s comment names only the install case: applied, and it holds (`fetched.rs:296-297` against the four arms at :299-308, one of which is `PickOutcome::Unavailable`). Task 6 gains the edit (Files, a Step 3 paragraph, the `git add`, File Structure).
- Code, Minor, `missing_view_phrases`' Unavailable tooltip phrase is unreachable: rejected. The function's doc, in Task 6 Step 3, already says so ("The tooltip of an unavailable engine is its toast, so its tooltip phrase only keeps the match whole"), and the tab_body tooltip match takes `Unavailable` to `unavailable_toast` before it reaches `.0` (`tab_body.rs:55`). Returning an `Option` for the tooltip half would change the tuple and its test for one unused value; the doc is where a reader of the function looks.
- Code, Minor, `send_version<S>` is generic over a `pub(crate)` static and the spike used a plain function: no defect, so applied as one stop-and-report sentence in Task 9 Step 6 (a rebuilt host that still reads unstamped while Task 1's tests pass). Task 1 Step 5's WSL run already links a generic instantiation (`MessageChannel<Cursor<Vec<u8>>>`) on ELF, and Task 9 Step 6 reads each real host. The finding's `#[used]` fallback is not planted: nothing here shows `#[used]` alone survives `--gc-sections`, and the spike's unreferenced static did not.
- Tests, Minor, the host's log site is unpinned: rejected. `spawn_host_reader` calls `recv_version().map(skew_with_app)`, whose one parameter is manifold's number (the host's own is `PROTOCOL_VERSION` inside it), so the call site has no pair of numbers to swap. The suggested `Skew::between(Some(theirs), PROTOCOL_VERSION)` at the call site would put the labels in the order that was the old bug. The line goes out by `eprintln!`, which a test cannot read without a seam this check does not justify; `the_host_names_which_side_to_rebuild` pins the text, and `a_wrong_version_from_the_host_becomes_shutdown` (transport.rs:430-443) drives the path.
- Tests, Minor, the 5-folder walk (P34) is unpinned: applied, with a correction. `ancestors()` starts with the exe's own folder, so `take(5)` is that folder and four parents, not five parents. `the_dev_folder_is_searched_four_parents_up_and_no_further` (Task 4) puts a Servo host in the fifth parent's `target-servo` (not found), then in the fourth's (found). P34 and Task 4's Files and Step 5 name it.
- Tests, Minor, the ELF half is proved only on Linux/CI: applied in part. Task 1 Step 5 already runs the five stamp tests in WSL, a real linked ELF test exe, so a local ELF proof exists; the finding's premise (CI only) is not quite right. Task 12's report now names that run, the WebKit builds and Task 9 Step 6's WebKit verdict as the ELF evidence.
- Complete, Minor, Task 12 Step 5 misses hosts the plan starts: applied. The `Get-Process` list gains `manifold-view-webview2`, `manifold-view-pdfium` and `wsl`, and a `wsl.exe -d Ubuntu -e pgrep -af "[m]anifold-view-webkitgtk|[c]argo"` line covers what runs inside the distro (`-e` and the bracketed letters keep `pgrep` from matching its own command line).
- Complete, Minor, Task 9 Step 2's `VIEWSTATE` script has no `WAITTABS`: applied as a stated reason and a fallback, not as a change to the script. `VIEWSTATE` reads the disk when it runs and needs no tab (the module doc, `debug_control.rs:30-32`, says only commands that need the focused tab wait for it), and `WAITTABS` would hold each run for the first-launch demo tab, which can take 30 s or more headless. `gate package` waits first (`package.rs:149-155`), so the step says: if a `view-state` line is missing, put `WAITTABS` first and run again, and report it.

**Round 4 (2026-10-29; lenses code, tests, complete).** All six findings were Minor. Each was checked against the code at bfacedc2 and against `~/.claude/runs/run.ps1` and the earlier gate run's files (`git grep`, `sed`, `ls`; nothing built or run). No finding needed a new architectural choice, so there is no new decision.

- Code, Minor, Task 11 Step 1 (and Round 3's line) says no task edits `render.rs`: applied, and it holds. Task 3 adds `RENDER_MISMATCH` after `MAGIC` (:12) and edits `read_frame` and `run_child`, all below line 11, so `render.rs:11` does not move; Step 1 and Round 3's line now say that, and that no task edits `pdf_render.rs` or `instance.rs`.
- Tests, Minor, three PARKED entries cite "design D2/D4/D7", a scratchpad file no task commits: applied. The entries cite "plan `2026-10-29-host-protocol-stamp`, D2/D4/D7", the committed copy that restates D1-D11 under "Approved decisions". The same defect was in two code comments (the Task 4 test doc's "(P34, ...)" and the Task 5 test doc's "(D3: ...)"), whose bare numbers resolve nowhere in the repo: both numbers are dropped from the comments.
- Complete, Minor, Task 12 Step 3 calls the start epoch the third field: applied, and it holds. `gate/src/all.rs:158` writes `TIMING gate/{name} {took} {s0}`, and the earlier run's line is `TIMING gate/sleep-history 194 1793247441` (`gate-log.txt:145`), so the epoch is the fourth whitespace-separated field (`awk '{print $4}'`); `gate-log.txt.cpu` lines are `<epoch> <cpu%>`.
- Complete, Minor, a run without `-Detach` leaves `RUN` undefined: applied, and it holds. `run.ps1` prints `run <id> started: ...` only under `-Detach` (line 49 of the script), and a plain run's folder is `<YYYY-MM-DD>-<Name>` with `-2`, `-3` appended on a repeat (lines 54-56). "Waiting for a run" now defines `RUN` and gives the lookup (`ls -dt "$LOCALAPPDATA"/claude-runs/manifold/????-??-??-<Name>* | head -1`, run here against `sleep-history-rerun`: it names the folder, and prints nothing for a name that never ran); Task 9 Steps 2, 6 and 7 point at it.
- Complete, Minor, the pre-run check does not cover the steps that write into `target/debug`: applied to Task 9 Steps 1 and 3, rejected for Task 12 Step 1. Step 1's gate build line relinks `manifold.exe` and the host exes, and Step 3's build and `sync-runtime-deps` rewrite `manifold-view-cef.exe` and its `cef-runtime/` copy, all of which a running dev `manifold` holds; both now do the first item of the pre-run check (`Get-Process cargo, gate, manifold`), ask the user to close it and never stop it, and Step 3 names the sync's `is in use: close manifold and run again` line as the same lock. Task 12 Step 1 runs `cargo fmt --check`, clippy and the test lines, which write only check metadata and test exes under `target/debug/deps` (`manifold` and `gate` have no `tests/` directory, and `--bin manifold` builds only the unit-test exe), so there is no held file for it to hit; the full gate in Step 3 already does the pre-run check. Task 8 Step 5 already stops on a sync error and asks the user to close manifold before its `rm`.
- Complete, Minor, five "sibling host" comments are not in Task 10 Step 4's pattern and stay as a second, older description: applied by rewording, not by listing them as reviewed. Under P10 `sibling_host` keeps its name but now means "a host in place", which includes the dev folder, so a comment that says "sibling" for where a host is looked for is too narrow. Task 10 Step 3 rewrites nine comments in five files (the finding's `process_manager.rs:40` and `:42`, `view_install.rs:56` and `:72`, `gate/package.rs:9`, and four the same grep found: `availability.rs` module doc and `host_on_disk`'s doc, and two comments of the Servo availability test in `process_manager/tests.rs`), the Files list, `git add` and the commit body name them, and Step 4's pattern gains `no sibling[: ]`, `a sibling host (is there|counts)`, `unless a sibling$` and `the sibling-binary$` (the four new alternatives, run alone on the tree at bfacedc2, list exactly those nine comments plus README :379 and :704, which Step 3 already fixes). Left alone on purpose, and said so in Step 3: the names `sibling_host`, `sibling_binary`, `cef_sibling_exe`; `prepare_webview2`'s "A sibling host wins" (WebView2 has no dev folder, so its host in place is literally beside the exe); `runtime_on_disk`'s "a bundled sibling `cef-runtime/`"; and `probes.rs`' comment, which Task 7 rewrites.
