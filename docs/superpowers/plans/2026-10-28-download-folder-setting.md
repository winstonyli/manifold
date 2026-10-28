---
status: active
---

# Download Folder Setting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Settings "Download folder" that every engine host (WebView2, CEF, WebKit under WSL) obeys. It defaults to Windows' own Downloads folder, reaches running hosts at once, and names files by one rule in every engine. Gate runs never touch the user's real Downloads folder.

**Architecture:** The app computes one `HostSettings { dark, download_dir }` per host runtime. It hands that to a host at spawn (`VIEW_SETTINGS`, hex of the postcard bytes) and sends it again as `HostToView::SetSettings` (protocol 7, replacing `SetTheme`) whenever it differs from what a tab was last given. Each host applies it with `host_env::apply` (the page palette and the download folder). Each host then names every download with `manifold_view_protocol::downloads::destination`; a download it can't place is reported started and failed. The WebView2 host sets every path itself with `SetResultFilePath`. A retry of a broken transfer takes the broken download's path, so a resumed download stays one card and one file. WSL paths are mapped in both directions in one place (`HostRuntime::host_path` / `app_path`): to the host at spawn and send, and back when a host message arrives.

**Tech Stack:** Rust (nightly-2026-09-23, edition 2021), egui 0.36, `windows` 0.62 (`SHGetKnownFolderPath`), webview2-com 0.39.1, cef-rs, webkit2gtk-rs, postcard, the `gate` harness (chrome-shots, `~/.claude/runs/run.ps1`).

**Spec:** `C:\Users\WINSTO~1\AppData\Local\Temp\claude\C--Users-Winston-Li-Documents-GitHub-manifold\a2c355a1-5493-43d2-a2ce-bd6f49ba9245\scratchpad\plan_brief.md` together with the session's decision list. The brief's approved decisions (binding) and probe facts are restated below, because the scratchpad is temporary. The decisions this plan makes itself are in "Decisions for review" (S1-S20).

## Approved decisions (the spec, restated)

- **A1.** The default folder (setting empty) is Windows' Known Folder for Downloads, resolved in `crates/manifold/src/profile.rs`. `pictures_dir` switches to the same Known Folder lookup (Pictures).
- **A3.** `download_directory: Option<String>` in `Settings`; empty or `None` means the default. Its Settings row copies the CEF directory row (`settings_cef_directory_row`): a text field, Browse, Clear, and a verdict when the folder doesn't exist.
- **A5.** One `HostSettings { dark, download_dir }` value.
  - A host gets it at spawn through the environment, and later as a new `HostToView::SetSettings`, which REPLACES `HostToView::SetTheme`.
  - `PROTOCOL_VERSION` goes 6 -> 7, and the wire-shape fingerprint test is updated.
  - Open tabs use a new folder straight away; downloads already running keep their path.
- **A-WebView2.** The WebView2 host sets EVERY download's path itself with `SetResultFilePath`.
  - The path follows the shared rule in `manifold-view-protocol/src/downloads.rs` (`safe_name` + `unique_path`). A name is taken when the file exists, or when one of this host's held operations has that path (compared with `same_download_path`).
  - The name is the leaf of Edge's proposed `ResultFilePath`.
  - The host points the profile's `DefaultDownloadFolderPath` at an empty folder it owns (a "naming folder"; nothing is saved there), so Edge does not number its proposal against the user's real Downloads folder.
  - A retry takes the path of the broken download it retries.
- **A11.** WebKit under WSL gets the folder as `/mnt/<drive>/...`, hex-encoded on the WSL command line like the URL.
  - Paths coming back as `/mnt/<drive>/...` are mapped to `X:\...` as soon as host messages arrive in the app.
  - The `/`-rooted -> `\\wsl.localhost\Ubuntu\...` mapping moves to that same spot, so the stored data, toasts, Reveal and restore all see Windows paths.
- **A13.** The app always passes an absolute folder. The protocol's `downloads_dir()` and its empty-path fallback are deleted. A host started without a folder (only possible by hand) reports each download failed.
- **A15.** `seed_settings` (gate `instance.rs`) writes `download_directory` = `<instance profile>\downloads` for EVERY gate instance, so the download shots can expect exact names (`slow.zip`, `real.zip`, ...).
- **A16.** `PARKED.md` gets an entry for two hosts that download one file name into one folder at the same moment: they can pick the same file, because each host reserves only its own in-flight names (unverified). Each tab is its own host process, WebView2 included (`process_manager::spawn_tab` starts one per tab), so this covers WebView2 too.
- **A5-shot.** A gate shot for a resumed download.
  - The test server (`crates/gate/src/shots_net.rs`) answers a resumable URL: HTTP/1.1 with a strong `ETag` and `Accept-Ranges: bytes`. It breaks the first transfer partway, and answers the engine's `Range` retry with `206 Partial Content` and the rest.
  - The shot checks for one completed card for that URL, and a whole, correct file at the exact path in the instance's download folder.
  - The PARKED item "A resumed WebView2 retry gets a second card" is deleted once this passes.

**Probe facts the design rests on** (raw logs under `%LOCALAPPDATA%\claude-runs\manifold\<id>`, rows in `docs/runs.md`):

- `put_ResultFilePath` needs an absolute path that includes the file name. It overwrites an existing file and creates a missing directory (Microsoft Learn, 2026-10-28).
- The host's profile is InPrivate. `SetDefaultDownloadFolderPath` works on it, but the setting is gone at the next run (`2026-10-28-dldir-profile`).
- Edge numbers its proposal against the profile folder (`2026-10-28-dldir-profile-seed`).
- If the profile folder can't be created, Edge fires NO `DownloadStarting` (`2026-10-28-dldir-profile-bad`).
- `SetResultFilePath` from the handler works, and Edge then writes nothing into its default folder (`2026-10-28-dldir-result`).
- A broken transfer is retried as a SECOND `DownloadStarting`. At start, the new operation's `InterruptReason` is the first one's (24), not NONE, and the first operation still reads InProgress with that reason. The first operation never reports again (`2026-10-28-dldir-result2`, `-result-etag2`).
- Resumable server:
  - With Edge's own naming, the retry is proposed `slow (1).zip`, which gives two cards (`2026-10-28-dldir-profile-etag`).
  - Set back to the first path, the retry resumes at 300000 bytes into the same file (`2026-10-28-dldir-result-etag`).
  - Never yet seen: a resume that runs to completion into the set path. The resume shot is the first check of that.
- Plain HTTP/1.0 server: the first try's partial file is deleted at the break, and the retry restarts from 0.
- The retry reports its outcome only while its operation is held.
- The app restarts a card when the same tab starts a download whose path equals one in flight (`fetched.rs` `insert_download_node`).

## Global Constraints

- Repo `C:\Users\Winston Li\Documents\GitHub\manifold`, branch `main`, base commit `42a4c80b`; every line number below is as of that commit and drifts as earlier steps land, so locate each edit by its quoted text, not by the number.
- `PROTOCOL_VERSION` 6 -> 7; `HostToView::SetSettings(HostSettings)` takes `SetTheme { dark }`'s place (variant index 20).
- `pub struct HostSettings { pub dark: bool, pub download_dir: Option<String> }` lives in `manifold-view-protocol` (`messages.rs`), re-exported at the crate root.
- `VIEW_SETTINGS` (`host_env::ENV_SETTINGS`) = hex of `HostSettings`' postcard bytes; it replaces `VIEW_DARK`.
- `Settings::download_directory: Option<String>`: `None`, empty, or not absolute -> `profile::downloads_dir()`; absolute -> used in Windows' own form (`settings::full_folder`: `/` read as `\`, `.` and `..` resolved), made at the first download.
- The app adds no new way to read the disk: the Download folder row reads its folder through the Settings page's `cached`, like every other Settings read (on the UI thread, at most every `DISK_CACHE_AGE`, 3 s), and the Known Folder lookup does not verify (`KF_FLAG_DONT_VERIFY`). Moving the page's reads off the UI thread (`cached` itself, for all its users, plus `App::cef_dir_verdict`) and the hosts' naming on their own UI threads are parked (S20).
- Every gate instance that `seed_settings` seeds (S11) gets `download_directory` = `std::path::absolute(<profile>\downloads)`; stages that seed nothing (e.g. `arrange`, `labels`, `focus`, `stress`) keep the default folder and run no page download (Task 8 Step 5 checks).
- The WebView2 naming folder is `std::env::temp_dir().join("manifold-webview2-names")`.
- Formatting and the commit hook:
  - run `cargo fmt --all` before each commit;
  - the pre-commit hook runs fmt and clippy with `-D warnings`;
  - never `--no-verify`, never an `#[allow]` without `reason`.
- Commits: small and topical, after `git diff --stat`. Every message ends with the executing session's own attribution trailer: the heredocs below show `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, which a session on another model replaces with the trailer its system reminder gives.
- No test reads or writes the real `%APPDATA%` or the user's folders. `profile::dir()`, `downloads_dir()` and `pictures_dir()` are `None` under `cfg(test)`; tests use `store::testutil::temp_dir` or `std::env::temp_dir()`.
- No CHANGELOG edit (entries are written at the version bump), no migration code, no compatibility shims.
- Python only as `python -P f.py < /dev/null`, with the script written to a file first (no step here needs Python).
- Build before any gate run: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`.
- Gate runs go through `~/.claude/runs/run.ps1` from bash, with `-Detach`.
  - Wait on `run.json` for a condition with a deadline, in a background Bash call or a Monitor, never with a fixed sleep.
  - The app is launched only in the runs named in Tasks 6 and 8.
- No download shot runs before Task 5's seed and Task 6's WebView2 code are committed.
- Cargo on Windows uses `-j 8` and the default `target/`. The only other target dirs are README's `target-linux` (inside WSL) and `target-servo`.
- Never push, tag or bump.

## Decisions for review

Each item is a choice this plan makes inside the approved design. The user approved all of them as recommended on 2026-10-28 ("approve"), together with approval item 21: five gaps the reviews left undecided are parked (Task 7 Step 3, the last five entries), and the full gate in Task 8.

- **S1. Spawn-time encoding.**
  - Recommended (A): one variable `VIEW_SETTINGS`, the hex of `HostSettings`' postcard bytes.
    - One encoder and one decoder live in the protocol. WebView2, CEF and Servo read it through `HostEnv` (whose `dark: bool` becomes `settings: HostSettings`); WebKit reads the same variable.
    - The WSL launch passes it as an inline assignment, which is shell-safe, like the URL.
    - The WSL command line stays within budget: URL hex <= 16 384, settings hex <= 4 096 (S15), plus about 300 characters for the rest, under Windows' 32 767.
  - Alternative (B): one variable per field (`VIEW_DARK` kept, a new `VIEW_DOWNLOAD_DIR` hex-encoded for WSL). Every future field would cost a new variable and decoder, and one value would have two encodings.
- **S2. Type of `download_dir`.**
  - Recommended (A): `Option<String>`. Under WSL the value is a Linux path, and the wire must carry it exactly as the host sees it.
  - Alternative (B): `Option<PathBuf>`.
- **S3. One source for both paths.** `App::host_settings(runtime)` is used at spawn (`spawn_tab_async`) and by `sync_host_settings`, which replaces `sync_theme`.
  - After the first frame, a theme change also reloads the tabs that show a `manifold://` page. "Shows" is judged by the live `TabPayload::url`; the old code read the shared `doc.mirror.url`, which can lag the page.
  - The app's own `manifold_pages::set_dark` calls (`main.rs:505`, `event_loop/frame.rs:333`) and `is_dark` reads (`process_manager.rs:1045`, `webkit.rs:109`) are deleted, because the app never serves a `manifold://` page itself: `crates/manifold` has no `manifold_pages::serve` call (checked by grep at 42a4c80b).
  - This differs from the brief's S3 wording ("`set_dark` in the app stays tied to the dark change").
  - Alternative: keep the app's `set_dark` as the spawn-time source of `dark`.
  - Recommended: delete it, so the theme has one source (`egui_ctx.theme()`).
- **S4. Setting semantics** (the brief's S4).
  - Text that is not a full path gets a danger verdict and is not used; downloads go to the default folder.
  - An absolute folder that doesn't exist is used, and the host makes it at the first download (the verdict says so).
  - An absolute folder is used in Windows' own form (`settings::full_folder`, one function for the verdict and `App::download_folder`): `D:/dl` typed from a shell is `D:\dl`, so every download path built on it, and Explorer's `/select` on Reveal, read one separator. The field keeps what was typed.
  - A file in its place gets a danger verdict.
  - A folder the host can't make is refused: Started and then Failed, so the user sees a failed card.
  - Alternative: refuse relative text at commit time and keep the old value.
  - Recommended: as stated, since the field then shows exactly what was typed.
- **S5. WebView2 naming folder.**
  - Recommended (A): `std::env::temp_dir().join("manifold-webview2-names")`, set only once it has been made.
    - If making or setting it fails, the host logs it and Edge proposes in its own folder.
    - Names still come from the shared rule. The only effect is an odd leaf: `a (1).zip` when `a.zip` exists in the user's real Downloads folder.
  - Alternative (B): a folder inside the WebView2 profile folder (`webview2_profile_dir`), deleted with it.
- **S6. WebView2 retry rule.**
  - Recommended (A): pure functions `retry_path` / `save_path`, unit-tested. A download whose `InterruptReason` is not NONE at `DownloadStarting` takes the path of the latest held download with the same `Uri` whose reason is not NONE; otherwise the shared rule applies.
  - Known limit: two downloads of one URL in one tab that break at the same time both take the later one's path. It gets a PARKED entry (Task 7).
  - The broken download never reports again, so when its retry reports finished or failed, the host lets go of every held download with that path (`release`), the broken one included. Otherwise the broken one would keep the name taken until the host exits, and after a failed retry whose file went, the next download of that name would be numbered for nothing.
    - Alternative for this part: count a broken download as taking its name only while its file exists.
  - Alternative (B): match on `BytesReceived` at start against each broken download's bytes.
- **S7. How hosts apply settings.**
  - Recommended (A): `host_env::apply(&HostSettings)` sets the page palette and `downloads::set_folder`. The folder lives in a protocol-crate global that `downloads::folder()` reads, so the protocol crate gains a dependency on `manifold-pages`. There is no cycle: `manifold-pages` has no dependencies.
  - Alternative (B): each of the four hosts keeps the folder in its own state and calls `set_dark` itself.
- **S8. WSL path mapping.**
  - Recommended (A): `HostRuntime::host_path` / `app_path` replace `webkit.rs`'s `windows_path_to_wsl` and `node_store.rs`'s `App::windows_path_for_download`. The reader thread maps every host message as it arrives (`in_app_terms`).
    - `host_path` returns `None` for a folder WSL can't see (a UNC share, or a drive-relative `C:x`), so WebKit downloads are then refused. It drops a `\\?\` prefix.
    - WebKit downloads stored before this keep their Linux path (no migration), so Reveal of such a card fails.
  - Alternative (B): keep `windows_path_to_wsl`'s lossy mapping. A UNC folder would then become a nonsense Linux path that WebKit fails to create.
- **S9. Live sends.**
  - Recommended (A): every frame, compare each tab's `ViewTab::settings` with the app's (`set_settings` sends only a difference). This covers a tab whose spawn lands after a change, at the cost of building two `HostSettings` per frame.
  - Alternative (B): a "settings changed" flag set by `set_download_directory` / `choose_theme`, plus a send when a spawn lands.
- **S10. Where the draft lives.**
  - Recommended (A): `SettingsState::download_dir_draft`, like `home_page_draft`, with a `NOT_FORGOTTEN` entry in `main/tests.rs`. CEF's draft stays in `ViewInstallUi::cef_dir_draft`.
  - Alternative (B): move both drafts into one map.
- **S11. Gate seed.**
  - Recommended (A): `seed_settings` writes `std::path::absolute(<profile>\downloads)` for every caller (14 in `crates/gate/src`, plus its own test in `instance.rs`). The folder is not created at seed time, so every run exercises "made at the first download".
  - Alternative (B): create it at seed time.
- **S12. Refusal mechanics.**
  - CEF: `on_before_download` returns 0. CEF 152's header says false means "proceed with default handling (cancel with Alloy style, download shelf with Chrome style)", and that "Alloy style will always be used if |windowless_rendering_enabled| is true" (`cef-dll-sys` 152.1.0 bindings, the `on_before_download` and `runtime_style` docs). The host sets `windowless_rendering_enabled` (`crates/manifold-view-cef/src/main.rs:154`, `:199`; `nav_handler.rs:110-116` notes the Alloy runtime it forces). This is documented behavior, never run here (S19).
  - WebKit: `download.cancel()` inside `decide-destination`, then return `true`.
  - WebView2: `SetCancel(true)`.
  - Alternatives for CEF: return 1 without calling `cont` (the download hangs pending), or cancel unassigned ids from `on_download_updated` through `DownloadItemCallback::cancel`.
  - Recommended: return 0.
- **S13. Shared folder row.**
  - Recommended (A): `typed_folder` and `folder_field` are shared with the CEF row. The CEF draft then also follows outside edits while unfocused, and a pasted CEF folder loses its quotes too.
  - Alternative (B): copy the CEF row's code into the new row.
- **S14. What the resume shot requires.**
  - Recommended (A): `ranges_answered() > 0`, so the engine provably resumed rather than restarting.
  - Alternative (B): accept a restart from 0, as long as there is one completed card and a whole file.
- **S15. Settings too long for WSL.**
  - Recommended (A): a WebKit spawn whose settings hex is over 4 096 characters is refused with a named error.
  - Alternative (B): spawn with `download_dir: None`, so downloads are refused instead.
- **S16. Shot framing.**
  - Recommended (A): keep the download views' existing `SLEEP` framing, so shots match earlier runs. Every wait on a download's state is an `export_until`.
  - Alternative (B): drop the SLEEPs.
- **S17. Breaking the resume transfer.**
  - Recommended (A): `shots_net`'s held connections break "stickily": a connection kept after `break_all` breaks at once. The shot can then break the transfer as soon as any byte shows.
  - Alternative (B): wait for `bytes >= 300000` before breaking.
- **S18. A rerun of the download views.**
  - Recommended (A): the download views empty the instance's download folder first, so a rerun in the same work folder still gets exact names.
  - Alternative (B): leave it, and name a fresh work folder for every run.
- **S19. Runtime checks of the CEF and WebKit download paths.**
  - The gate's download views run on WebView2 only: `Shots::start` seeds `webview2` (`crates/gate/src/shots.rs:218`). So CEF's destination and refusal (S12) and WebKit's `/mnt/<drive>/...` destination get unit tests and builds here, but no run.
  - Recommended (A): no runtime check in this plan. Task 7 parks "CEF and WebKit download paths unexercised at runtime" with its trigger, and Task 8's report names both as unverified at runtime.
  - Alternative (B): a `download` stage in `gate`'s probes (`crates/gate/src/probes.rs`, which already runs under `VIEWS="webview2 cef"`). It would download into a good folder and into a refused one (a file in the folder's place) and check the cards. That is new harness code, planned separately. WebKit under WSL would stay unchecked, since no gate stage runs it.
- **S20. Folder reads that can block.** A read of a folder on a share that is offline (a NAS, a disconnected mapped drive) can block for tens of seconds.
  - Recommended (A), the app: the Download folder row reads its folder through the existing `cached` (`settings_page.rs`), exactly as the CEF row reads `cef_newer_than_tested`, like every other Settings read: on the UI thread, at most every `DISK_CACHE_AGE` (3 s) or after `forget_disk_cache`. A second cache beside `cached` that reads on a worker thread would be a second mechanism of the same shape, and would leave the page's other reads blocking. One PARKED item (Task 7, "The Settings page reads the disk on the UI thread") covers moving the page's disk reads off the UI thread by making `cached` itself off-thread for all its users (`view_runtime`, `view_leftovers`, `cef_newer_than_tested`, `named_sessions`, `view_dir_size`, and the new row's `download_folder_kind`), plus `App::cef_dir_verdict`.
  - Alternative (B), the app: do that move in this plan. `cached` reads on a worker thread for every user (at most one read per key at a time, the last value kept while the next is read, nothing until the first lands), each caller says what it shows until then, and `cef_dir_verdict` moves onto it.
  - Either way, the Known Folder lookup passes `KF_FLAG_DONT_VERIFY`: without it Windows checks that the folder exists, which can wait on the network and fails for a deleted Downloads folder (Microsoft Learn, `KNOWN_FOLDER_FLAG`, read 2026-10-28). With it, a deleted Downloads folder still names its path, and the host makes it at the first download.
  - Recommended (A), the hosts: each host still names a download on its own UI thread (`downloads::destination`: `create_dir_all` and the name checks), so an offline share freezes that tab's host until Windows gives up, and the download is then refused. Each tab is its own host process, so the app and the other tabs keep running. Task 7 parks it in its own item ("A download folder on an offline share freezes the tab's host").
  - Alternative, the hosts: name the download on a worker thread and finish the engine's decision later, new code in three hosts. WebView2 has `DownloadStartingEventArgs::GetDeferral`; CEF 152's header says `on_before_download`'s callback may be run later, after the handler returns (read 2026-10-28); WebKit's way is unchecked.

## Review Focus

Each of the five most likely failures for a user has its test in the task that owns the code:

1. A folder pasted with Explorer's quotes or stray spaces is stored without them, rather than read as a relative path that silently sends downloads elsewhere; one typed with `/` is used in Windows' form, rather than giving every download a mixed-separator path that Reveal can't select. Task 1: `a_pasted_folder_loses_its_quotes_and_padding`, `a_download_folder_is_stored_as_typed_without_quotes_and_cleared_by_empty_text`, `a_full_folder_is_read_in_windows_form`.
2. A folder with spaces, non-ASCII letters, quotes or `$` reaches every host exactly: through the environment, the WSL command line and the `/mnt` mapping. Task 3: `settings_cross_the_environment_exactly`, `the_webkit_command_carries_the_settings_as_hex_only`. Task 2: `a_wsl_host_sees_a_windows_folder_as_mnt_and_its_paths_come_back_as_windows_ones`.
3. A folder that can't be made (a file in its place, no folder at all) gives a failed card, never a silent save somewhere else. Task 4: `a_destination_is_made_in_the_folder_or_refused`, `a_refused_download_starts_and_fails_at_once`.
4. A second download of one name, started while the first is still arriving, is numbered; the engine's retry of a broken one is not; and once the retry ends, the name is free again. Task 6: `a_second_download_of_one_name_is_numbered_and_a_retry_is_not`, `a_retry_that_ends_frees_the_name_of_the_download_it_retried`.
5. A folder change reaches each open tab once, and also reaches a tab whose spawn lands after the change. Task 3: `a_folder_change_reaches_each_tab_once_and_a_tab_landing_later`.

---

### Task 1: The Download folder setting and the Known Folders

**Files:**
- Modify: `docs/superpowers/plans/2026-10-28-download-folder-setting.md` (front matter `status`, Step 0)
- Modify: `crates/manifold/Cargo.toml:49-51`
- Modify: `crates/manifold/src/profile.rs:1-6`, `:43-51`, `:148-156`
- Modify: `crates/manifold/src/settings.rs:154-158`, `:160-162`, `:167-182`, tests after `:326`
- Modify: `crates/manifold/src/view_install.rs:1045`
- Modify: `crates/manifold/src/settings_page.rs:41-42`, `:241-261`, `:420-459`, before `mod tests` (`:831`), tests
- Modify: `crates/manifold/src/settings_rows.rs:38`, `:74-80`, `:172`
- Modify: `crates/manifold/src/main/tests.rs:88`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub fn profile::downloads_dir() -> Option<PathBuf>` (cached once per run; `None` under `cfg(test)`)
  - `pub fn profile::pictures_dir() -> Option<PathBuf>` (same signature, now the Known Folder)
  - `Settings::download_directory: Option<String>`
  - `pub(crate) fn settings::typed_folder(text: &str) -> &str`
  - `pub(crate) fn settings::full_folder(text: &str) -> Option<PathBuf>` (the setting as the folder it names, in Windows' form; `None` when not a full path)
  - `SettingsState::download_dir_draft: Option<String>`
  - `pub(crate) fn App::set_download_directory(&mut self, path: &str)`
  - `pub(crate) fn App::settings_download_folder_row(&mut self, ui: &mut egui::Ui)`

- [ ] **Step 0: Mark the plan active and commit it**

Run: `cd "/c/Users/Winston Li/Documents/GitHub/manifold" && git status --short`
Expected: only `?? docs/superpowers/plans/2026-10-28-download-folder-setting.md`. Anything else is another session's work: stop and report it.

With the Edit tool, change this file's front matter line `status: draft` to `status: active` (VERSIONING.md "Specs and plans"). Then:

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git add docs/superpowers/plans/2026-10-28-download-folder-setting.md
git diff --cached --stat
git commit -F - <<'EOF'
docs: plan for the download folder setting

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: one file in the stat; the hook passes (no Rust file is staged, so it checks nothing).

- [ ] **Step 1: Write the failing tests**

In `crates/manifold/src/settings.rs`'s `mod tests`, after `cef_directory_defaults_when_missing_and_round_trips` (ends at line 326), add:

```rust
    #[test]
    fn download_directory_defaults_when_missing_and_round_trips() {
        let old: Settings = serde_json::from_str(r#"{"home_page":"https://a.test"}"#).unwrap();
        assert_eq!(old.download_directory, None);
        let settings = Settings { download_directory: Some("D:\\Zoë Q\\dl".into()), ..Settings::default() };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back.download_directory.as_deref(), Some("D:\\Zoë Q\\dl"));
    }

    /// Review Focus 1: Explorer's "Copy as path" quotes a folder; the setting holds it without them.
    #[test]
    fn a_pasted_folder_loses_its_quotes_and_padding() {
        assert_eq!(typed_folder(" \"C:\\Users\\Zoë Q\\Downloads\" "), "C:\\Users\\Zoë Q\\Downloads");
        assert_eq!(typed_folder("\" C:\\a \""), "C:\\a");
        assert_eq!(typed_folder("C:\\a"), "C:\\a");
        assert_eq!(typed_folder("  "), "");
        assert_eq!(typed_folder("\"\""), "");
        assert_eq!(typed_folder("\""), "\"", "a lone quote is not a pair");
    }

    /// Review Focus 1: a full path typed with `/` (from a shell or a config file) is the folder in Windows' own form,
    /// so every download path built on it, and Explorer's `/select` on Reveal, read one separator. Compared as text:
    /// `Path` equality ignores the separator.
    #[cfg(windows)]
    #[test]
    fn a_full_folder_is_read_in_windows_form() {
        let text = |t: &str| full_folder(t).map(|d| d.to_string_lossy().into_owned());
        assert_eq!(text("D:/dl").as_deref(), Some(r"D:\dl"));
        let file = full_folder("D:/dl/").map(|d| d.join("a.zip").to_string_lossy().into_owned());
        assert_eq!(file.as_deref(), Some(r"D:\dl\a.zip"), "no doubled separator");
        assert_eq!(text(r"D:\dl\..\x").as_deref(), Some(r"D:\x"));
        assert_eq!(text("//nas/share/dl").as_deref(), Some(r"\\nas\share\dl"));
        assert_eq!(text(r"C:\Users\Zoë Q\Downloads").as_deref(), Some(r"C:\Users\Zoë Q\Downloads"));
        assert_eq!(full_folder("Downloads2"), None);
        assert_eq!(full_folder("D:dl"), None, "drive-relative");
        assert_eq!(full_folder(""), None);
    }
```

In `crates/manifold/src/settings_page.rs`'s `mod tests` (starts at line 832), add:

```rust
    #[test]
    fn the_download_folder_verdict_names_what_is_wrong() {
        let never = |path: &std::path::Path| -> Option<bool> { panic!("read {}", path.display()) };
        assert_eq!(download_folder_verdict("", never), None, "empty: the default, nothing to say");
        assert!(matches!(download_folder_verdict("Downloads2", never), Some(Err(m)) if m.contains("full path")));
        let abs = crate::store::testutil::temp_dir("download-verdict").join("Zoë Q downloads");
        let abs = abs.to_str().unwrap();
        assert_eq!(download_folder_verdict(abs, |_| Some(true)), None, "a folder: nothing to say");
        assert!(matches!(download_folder_verdict(abs, |_| Some(false)), Some(Err(m)) if m.contains("Not a folder")));
        assert!(
            matches!(download_folder_verdict(abs, |_| None), Some(Ok(m)) if m.contains("made at the first download"))
        );
    }

    /// Review Focus 1, through the App: what the row commits is stored as the setting holds it.
    #[test]
    fn a_download_folder_is_stored_as_typed_without_quotes_and_cleared_by_empty_text() {
        let mut app = App::default();
        app.set_download_directory(" \"D:\\dl\" ");
        assert_eq!(app.settings.value.download_directory.as_deref(), Some("D:\\dl"));
        app.set_download_directory("  ");
        assert_eq!(app.settings.value.download_directory, None);
    }
```

In `crates/manifold/src/profile.rs`, `tests_have_no_profile_dir` (line 148), add a last line:

```rust
        assert_eq!(super::downloads_dir(), None);
```

In `crates/manifold/src/settings_rows.rs`'s `keywords_match_but_unrelated_text_does_not` (line 172), after `assert_eq!(ids("search engine"), ["search_engine"]);`, add:

```rust
        assert!(ids("save location").contains(&"download_folder"));
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -j 8 -p manifold settings 2>&1 | tail -n 20`
Expected: compile errors naming `typed_folder`, `full_folder`, `download_directory`, `download_folder_verdict`, `set_download_directory` and `downloads_dir`.

- [ ] **Step 3: The Known Folders in `profile.rs`**

In `crates/manifold/Cargo.toml`, under the `[target.'cfg(windows)'.dependencies]` header (line 48, which stays), replace lines 49-51 (the two comment lines `# Only for SetProcessDpiAwarenessContext ...` and the `windows = ...` line) with:

```toml
# DPI awareness at startup (`main()`), DPAPI for the cookie jars (`jar.rs`) and the Known Folders (`profile.rs`:
# Downloads, Pictures).
windows = { workspace = true, features = [
    "Win32_Foundation",
    "Win32_UI_HiDpi",
    "Win32_Security_Cryptography",
    "Win32_UI_Shell",
    "Win32_System_Com",
] }
```

In `crates/manifold/src/profile.rs`, replace the module doc's last two lines (5-6, "or, for engines, `view_data_dir()`. The one user folder outside it, / the screenshot folder, comes from `pictures_dir()`.") with:

```rust
//! or, for engines, `view_data_dir()`. The user folders outside it come from Windows' Known Folders: Downloads
//! (`downloads_dir()`, the default download folder) and Pictures (`pictures_dir()`, the screenshot folder).
```

Replace `pictures_dir` (lines 43-51, its doc comment included) with:

```rust
/// The user's Downloads folder (Windows' Known Folder; `~/Downloads` elsewhere): where downloads go when the
/// Download folder setting is empty or not a full path. Read once per run. `None` when the OS has none, and always
/// under `cargo test`, like `dir()`.
pub fn downloads_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| user_folder(UserFolder::Downloads)).clone()
}

/// The user's Pictures folder (Windows' Known Folder; `~/Pictures` elsewhere), where the toolbar's Screenshot saves.
/// `None` like `downloads_dir()`.
pub fn pictures_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    user_folder(UserFolder::Pictures)
}

/// The user folders the app reads (`user_folder`).
#[derive(Clone, Copy)]
enum UserFolder {
    Downloads,
    Pictures,
}

#[cfg(windows)]
fn user_folder(folder: UserFolder) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, FOLDERID_Pictures};
    known_folder(match folder {
        UserFolder::Downloads => &FOLDERID_Downloads,
        UserFolder::Pictures => &FOLDERID_Pictures,
    })
}

/// A Known Folder's path, wherever the user moved it (`SHGetKnownFolderPath`); `None` when Windows has none. Not
/// checked for existence (`KF_FLAG_DONT_VERIFY`): the check can wait on a network folder, on the UI thread, and fails
/// for a deleted folder, which the first download (or screenshot) makes again.
#[cfg(windows)]
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY};
    // Safety: on success the string is Windows' allocation, ours to free once, after it is copied.
    let raw = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY, None) }.ok()?;
    let path = PathBuf::from(std::ffi::OsString::from_wide(unsafe { raw.as_wide() }));
    unsafe { CoTaskMemFree(Some(raw.as_ptr() as *const _)) };
    Some(path)
}

#[cfg(not(windows))]
fn user_folder(folder: UserFolder) -> Option<PathBuf> {
    let name = match folder {
        UserFolder::Downloads => "Downloads",
        UserFolder::Pictures => "Pictures",
    };
    Some(PathBuf::from(std::env::var_os("HOME")?).join(name))
}
```

(`profile::tests::app_default_touches_no_profile_file` skips `profile.rs` itself, so `known_folder` and `"HOME"` are allowed here.)

- [ ] **Step 4: The setting**

In `crates/manifold/src/settings.rs`, after the `cef_directory` field (line 157), add:

```rust
    /// Where downloads are saved: an absolute folder, made at the first download if missing. `None` (or text that
    /// is not a full path) means the user's Downloads folder (`profile::downloads_dir`).
    #[serde(default)]
    pub download_directory: Option<String>,
```

In `impl Default for Settings`, after `cef_directory: None,` (line 180), add `download_directory: None,`. After `fn default_already_shown` (lines 160-162), add:

```rust
/// A typed or pasted folder as a setting holds it: trimmed, and one pair of surrounding quotes dropped (Explorer's
/// "Copy as path" adds them), then trimmed again.
pub(crate) fn typed_folder(text: &str) -> &str {
    let text = text.trim();
    text.strip_prefix('"').and_then(|t| t.strip_suffix('"')).map_or(text, str::trim)
}

/// The folder a folder setting names, in Windows' own form (`std::path::absolute`: `/` read as `\`, `.` and `..`
/// resolved, no disk read), so every path built on it reads one separator; the setting keeps the text as typed.
/// `None` when the text is not a full path, which a folder setting does not use.
pub(crate) fn full_folder(text: &str) -> Option<PathBuf> {
    let path = std::path::Path::new(text);
    path.is_absolute().then(|| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}
```

(`settings.rs` already imports `PathBuf`: `use std::path::PathBuf;`, line 1.)

In `crates/manifold/src/view_install.rs:1045` (`set_cef_directory`), replace `let path = path.trim();` with:

```rust
        let path = crate::settings::typed_folder(path);
```

- [ ] **Step 5: The shared folder field, the row, and the CEF row on it**

In `crates/manifold/src/settings_page.rs`, `SettingsState`: after `home_page_draft` (lines 41-42), add:

```rust
    /// The Download folder field's text while it's being edited (`settings_download_folder_row`).
    pub(crate) download_dir_draft: Option<String>,
```

(If `SettingsState` has a hand-written constructor or `Default` that lists fields, add `download_dir_draft: None,` there too; `cargo build` names it if so.)

After `settings_home_page_row` (ends line 261), add:

```rust
    /// Settings, General: the folder downloads are saved in, for every engine.
    pub(crate) fn settings_download_folder_row(&mut self, ui: &mut egui::Ui) {
        ui.label("Download folder");
        let configured = self.settings.value.download_directory.clone().unwrap_or_default();
        let hint =
            profile::downloads_dir().map_or_else(|| "Your Downloads folder".to_string(), |d| d.display().to_string());
        if let Some(text) = folder_field(ui, &mut self.settings.download_dir_draft, &configured, &hint) {
            if crate::settings::typed_folder(&text) != configured {
                self.set_download_directory(&text);
            }
            // The folder may have been made or fixed on disk since its verdict was read.
            forget_disk_cache(ui.ctx());
        }
        let setting = self.settings.value.download_directory.clone().unwrap_or_default();
        // Read like every other Settings disk read (`cached`, as the CEF row's `cef_newer_than_tested`): on the UI
        // thread, at most every `DISK_CACHE_AGE`. A folder on an offline share can block it (S20, parked).
        let verdict = download_folder_verdict(&setting, |dir| {
            cached(ui.ctx(), ("download_folder_kind", dir), DISK_CACHE_AGE, || {
                std::fs::metadata(dir).ok().map(|m| m.is_dir())
            })
        });
        match verdict {
            Some(Err(message)) => {
                ui.add(egui::Label::new(egui::RichText::new(message).small().color(danger(ui.visuals()))).wrap());
            }
            Some(Ok(note)) => {
                ui.add(egui::Label::new(egui::RichText::new(note).small().weak()).wrap());
            }
            None => {}
        }
    }

    /// Set (or clear, for empty text) the Download folder, as `typed_folder` holds it, and save it.
    pub(crate) fn set_download_directory(&mut self, path: &str) {
        let path = crate::settings::typed_folder(path);
        self.settings.value.download_directory = (!path.is_empty()).then(|| path.to_string());
        self.save_settings();
    }
```

In `settings_cef_directory_row` (lines 420-476), keep `ui.add_space(6.0);` and the label line. Replace everything from `let mut commit = None;` (line 423) through the end of the `if let Some(text) = commit { ... }` block (line 459) with:

```rust
        let configured = self.settings.value.cef_directory.clone().unwrap_or_default();
        let hint = format!("Folder with {}", crate::cef_platform::PLATFORM.runtime_lib);
        if let Some(text) = folder_field(ui, &mut self.view_ui.cef_dir_draft, &configured, &hint) {
            if crate::settings::typed_folder(&text) != configured {
                self.set_cef_directory(&text);
            } else {
                // Same folder (Browse again, or Enter): its contents may have been fixed on disk.
                self.invalidate_cef_dir_check();
            }
        }
```

(The `match self.cef_dir_verdict() { ... }` that follows stays.) Before `#[cfg(test)] mod tests` (line 831), add the two free functions:

```rust
/// A folder setting's text field, Browse and Clear (`settings_cef_directory_row`, `settings_download_folder_row`).
/// Returns the text to commit: the draft when the field loses focus (Escape gives `configured` back instead), the
/// folder Browse picked, or "" from Clear. While the field isn't focused the draft follows `configured`, so an
/// outside change (a settings reload) shows.
fn folder_field(ui: &mut egui::Ui, draft: &mut Option<String>, configured: &str, hint: &str) -> Option<String> {
    let mut commit = None;
    ui.horizontal(|ui| {
        let text = draft.get_or_insert_with(|| configured.to_string());
        let response = toolbar_field(ui, |ui| ui.add(field_edit(text).desired_width(220.0).hint_text(hint)));
        if response.lost_focus() {
            // Escape gives up the edit instead of committing it.
            commit = Some(if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                configured.to_string()
            } else {
                text.clone()
            });
        } else if !ui.memory(|m| m.has_focus(response.id)) {
            // Not `response.has_focus()`: that is also false while the OS window is unfocused, yet the field still
            // takes typed text (`settings_home_page_row`).
            *draft = None;
        }
        if ui.button("Browse").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                commit = Some(dir.to_string_lossy().into_owned());
            }
        }
        if !configured.is_empty() && ui.button("Clear").clicked() {
            commit = Some(String::new());
        }
    });
    if commit.is_some() {
        *draft = None;
    }
    commit
}

/// What the Download folder row says of `setting` (`kind` reads whether the folder it names, `settings::full_folder`,
/// is a folder: `Some(true)`, a file: `Some(false)`, or nothing: `None`). `None` when there is nothing to say.
fn download_folder_verdict(
    setting: &str,
    kind: impl FnOnce(&std::path::Path) -> Option<bool>,
) -> Option<Result<&'static str, &'static str>> {
    if setting.is_empty() {
        return None;
    }
    let Some(dir) = crate::settings::full_folder(setting) else {
        return Some(Err("Not a full path (like C:\\Users\\you\\Downloads), so downloads go to your Downloads folder"));
    };
    match kind(&dir) {
        Some(true) => None,
        Some(false) => Some(Err("Not a folder: downloads fail until it is")),
        None => Some(Ok("Doesn't exist yet: made at the first download")),
    }
}
```

In `crates/manifold/src/settings_rows.rs`, change line 38 to `static ROWS: [SettingRow; 14] = [`. After the `home_page` row (lines 74-80), add:

```rust
    SettingRow {
        id: "download_folder",
        section: "General",
        title: "Download folder",
        keywords: "downloads save location directory path files",
        draw: |app, ui, _| app.settings_download_folder_row(ui),
    },
```

In `crates/manifold/src/main/tests.rs`, after `("settings.home_page_draft", "a url draft, not a node id"),` (line 88), add:

```rust
    ("settings.download_dir_draft", "a path draft, not a node id"),
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -j 8 -p manifold 2>&1 | grep -E "^test result|FAILED|panicked" | head -n 20`
Expected: every `test result:` line says `ok. ... 0 failed`. That includes the five new tests, `tests_have_no_profile_dir`, `app_default_touches_no_profile_file`, the settings-row and `NOT_FORGOTTEN` tests, and `the_settings_page_keeps_its_text_inside_a_480_px_window`, which now draws the new row.

- [ ] **Step 7: Commit**

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cargo fmt --all
git diff --stat
git add crates/manifold/Cargo.toml crates/manifold/src/profile.rs crates/manifold/src/settings.rs crates/manifold/src/view_install.rs crates/manifold/src/settings_page.rs crates/manifold/src/settings_rows.rs crates/manifold/src/main/tests.rs
git commit -F - <<'EOF'
settings: a Download folder setting, defaulting to the Downloads Known Folder

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: `git diff --stat` lists exactly those seven files (plus `Cargo.lock` only if the feature change touched it, then add it too); the hook passes.

### Task 2: WSL paths mapped in one place, both ways

**Files:**
- Modify: `crates/manifold/src/process_manager/runtime.rs:8`, `:70-91`, tests (`:115`)
- Modify: `crates/manifold/src/process_manager.rs:841-857`, `:902-909`, `:1091`
- Modify: `crates/manifold/src/process_manager/webkit.rs:6-19`, `:62`, `:112-118`, `:165`
- Modify: `crates/manifold/src/process_manager/tests.rs:48-62` and the `reader_fed` calls (`:69`, `:84`, `:114`, `:176`, `:213`)
- Modify: `crates/manifold/src/node_store.rs:209-227`
- Modify: `crates/manifold/src/reveal.rs:35-46`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub(crate) const runtime::WSL_DISTRO: &str = "Ubuntu"`
  - `pub(crate) fn HostRuntime::host_path(self, path: &Path) -> Option<String>`
  - `pub(crate) fn HostRuntime::app_path(self, path: String) -> String`
  - `fn spawn_reader_thread<S>(from_view: MessageChannel<S>, frame: view_io::FrameSlot, runtime: HostRuntime, wake: view_io::Wake) -> mpsc::Receiver<ViewToHost>`
  - `fn in_app_terms(runtime: HostRuntime, msg: ViewToHost) -> ViewToHost`
  - Deletes `webkit::windows_path_to_wsl` and `App::windows_path_for_download`.

- [ ] **Step 1: Write the failing tests**

In `crates/manifold/src/process_manager/runtime.rs`'s `mod tests`, add:

```rust
    /// Review Focus 2 (WSL): a folder with a space and a non-ASCII letter reaches the host as its drvfs path, and the
    /// host's paths come back in the form Explorer opens.
    #[test]
    fn a_wsl_host_sees_a_windows_folder_as_mnt_and_its_paths_come_back_as_windows_ones() {
        let wsl = HostRuntime::Wsl;
        let host = |p: &str| wsl.host_path(Path::new(p));
        assert_eq!(host(r"C:\Users\Zoë Q\Downloads").as_deref(), Some("/mnt/c/Users/Zoë Q/Downloads"));
        assert_eq!(host(r"\\?\D:\dl").as_deref(), Some("/mnt/d/dl"));
        assert_eq!(host(r"D:\").as_deref(), Some("/mnt/d/"));
        assert_eq!(host(r"\\nas\dl"), None, "a share WSL can't see");
        assert_eq!(host("C:dl"), None, "drive-relative");
        let app = |p: &str| wsl.app_path(p.to_string());
        assert_eq!(app("/mnt/c/Users/Zoë Q/Downloads/a.zip"), r"C:\Users\Zoë Q\Downloads\a.zip");
        assert_eq!(app("/mnt/d/"), r"D:\");
        assert_eq!(app("/mnt/d"), r"D:\");
        assert_eq!(app("/home/u/a.zip"), r"\\wsl.localhost\Ubuntu\home\u\a.zip");
        assert_eq!(app("/mnt/data/a.zip"), r"\\wsl.localhost\Ubuntu\mnt\data\a.zip", "not a drive letter");
        assert_eq!(app("b.zip"), "b.zip");
        assert_eq!(HostRuntime::Native.host_path(Path::new(r"C:\a b")).as_deref(), Some(r"C:\a b"));
        assert_eq!(HostRuntime::Native.app_path("/home/u/a".into()), "/home/u/a");
    }
```

In `crates/manifold/src/process_manager/tests.rs`, add after `reader_fed`:

```rust
/// A WebKit host under WSL reports `/mnt/...` paths; the app receives Windows ones.
#[test]
fn a_wsl_hosts_download_paths_arrive_in_windows_terms() {
    let (rx, ..) = reader_fed(HostRuntime::Wsl, |stream| {
        let mut view_kind = manifold_view_protocol::MessageChannel::new(stream);
        view_kind.send_version().unwrap();
        let path = "/mnt/d/dl/a.zip".to_string();
        view_kind.send(&ViewToHost::DownloadStarted { path: path.clone(), total_bytes: None, url: None }).unwrap();
        view_kind.send(&ViewToHost::DownloadCompleted { path }).unwrap();
    });
    let paths: Vec<String> = (0..2)
        .map(|_| match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            ViewToHost::DownloadStarted { path, .. } | ViewToHost::DownloadCompleted { path } => path,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(paths, [r"D:\dl\a.zip", r"D:\dl\a.zip"]);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 8 -p manifold process_manager 2>&1 | tail -n 20`
Expected: compile errors: no method `host_path` / `app_path` on `HostRuntime`; `reader_fed` takes 1 argument but 2 were supplied.

- [ ] **Step 3: `host_path` and `app_path`**

In `crates/manifold/src/process_manager/runtime.rs`, add after `use super::ViewKind;` (line 8):

```rust
use std::path::Path;

/// The WSL distro the WebKit host runs in (`webkit.rs`), and whose share Explorer opens for its other paths.
pub(crate) const WSL_DISTRO: &str = "Ubuntu";
```

Add to `impl HostRuntime` (after `for_os`, before the `}` at line 91):

```rust
    /// `path` (a Windows path in the app) as the host sees it: unchanged natively; through WSL a drive path as
    /// `/mnt/<drive>/...` (drvfs), a `\\?\` prefix dropped. `None` when the host cannot see it: through WSL a path not
    /// on a drive (a UNC share, a drive-relative `C:x`), and anywhere one that is not UTF-8.
    pub(crate) fn host_path(self, path: &Path) -> Option<String> {
        let text = path.to_str()?;
        match self {
            HostRuntime::Native => Some(text.to_string()),
            HostRuntime::Wsl => {
                let text = text.strip_prefix(r"\\?\").unwrap_or(text);
                let mut chars = text.chars();
                let drive = chars.next().filter(char::is_ascii_alphabetic)?;
                let rest = chars.as_str().strip_prefix(':').filter(|rest| rest.starts_with(['\\', '/']))?;
                Some(format!("/mnt/{}{}", drive.to_ascii_lowercase(), rest.replace('\\', "/")))
            }
        }
    }

    /// A path the host reported, in the app's terms (`host_path` the other way): unchanged natively; through WSL
    /// `/mnt/<drive>/...` as `X:\...`, and any other absolute path as the `\\wsl.localhost\<distro>\...` share
    /// Explorer opens.
    pub(crate) fn app_path(self, path: String) -> String {
        match self {
            HostRuntime::Native => path,
            HostRuntime::Wsl => {
                if let Some(rest) = path.strip_prefix("/mnt/") {
                    let mut chars = rest.chars();
                    if let Some(drive) = chars.next().filter(char::is_ascii_alphabetic) {
                        let tail = chars.as_str();
                        if tail.is_empty() || tail.starts_with('/') {
                            let tail = if tail.is_empty() { "/" } else { tail };
                            return format!("{}:{}", drive.to_ascii_uppercase(), tail.replace('/', "\\"));
                        }
                    }
                }
                match path.strip_prefix('/') {
                    Some(posix) => format!("\\\\wsl.localhost\\{WSL_DISTRO}\\{}", posix.replace('/', "\\")),
                    None => path,
                }
            }
        }
    }
```

- [ ] **Step 4: Map host messages as they arrive**

In `crates/manifold/src/process_manager.rs`, change `spawn_reader_thread`'s signature (line 857) and add a line to its doc (after the "Frames go to `frame` ..." paragraph, line 856):

```rust
///
/// A WSL host's download paths are put in the app's terms here (`in_app_terms`), before anything stores, shows or
/// reveals them.
fn spawn_reader_thread<S: std::io::Read + std::io::Write + Send + 'static>(
    mut from_view: manifold_view_protocol::MessageChannel<S>,
    frame: view_io::FrameSlot,
    runtime: HostRuntime,
    wake: view_io::Wake,
) -> mpsc::Receiver<ViewToHost> {
```

In its `msg =>` arm (line 903), replace `let sent = tx.send(msg);` with:

```rust
                    let sent = tx.send(in_app_terms(runtime, msg));
```

After `spawn_reader_thread` (after its closing `}`, line 916), add:

```rust
/// A host's message in the app's terms: a WSL host's download paths as Windows ones (`HostRuntime::app_path`), so the
/// store, the toasts and Reveal see one form.
fn in_app_terms(runtime: HostRuntime, msg: ViewToHost) -> ViewToHost {
    match msg {
        ViewToHost::DownloadStarted { path, total_bytes, url } => {
            ViewToHost::DownloadStarted { path: runtime.app_path(path), total_bytes, url }
        }
        ViewToHost::DownloadProgress { path, bytes_received } => {
            ViewToHost::DownloadProgress { path: runtime.app_path(path), bytes_received }
        }
        ViewToHost::DownloadCompleted { path } => ViewToHost::DownloadCompleted { path: runtime.app_path(path) },
        ViewToHost::DownloadFailed { path } => ViewToHost::DownloadFailed { path: runtime.app_path(path) },
        other => other,
    }
}
```

Change line 1091 (in `spawn_tab`) to:

```rust
    // A pipe-linked host runs natively (`runtime::tests::a_wsl_host_speaks_tcp`).
    let events_rx = spawn_reader_thread(from_view, Arc::clone(&frame), HostRuntime::Native, wake);
```

In `crates/manifold/src/process_manager/webkit.rs`:
- Delete `windows_path_to_wsl` (lines 6-19, doc included).
- Add `use super::runtime::WSL_DISTRO;` under `use super::*;`.
- In `webkit_shell_command`'s doc (line 62), change "`wsl.exe -d Ubuntu -- bash -c`" to "`wsl.exe -d <WSL_DISTRO> -- bash -c`".
- In `spawn_webkit_tab`'s `HostRuntime::Wsl` arm, replace the two lines `let shell = webkit_shell_command(initial_url, port, &token, &windows_path_to_wsl(&host), dark)?;` and `command.args(["-d", "Ubuntu", "--", "bash", "-c", &shell]);` (with the `let mut command = Command::new("wsl.exe");` between them kept) so that the arm reads:

```rust
        HostRuntime::Wsl => {
            // `wsl.exe` does NOT forward this Command's environment into the Linux process (verified: env vars set
            // via `.env()` here simply don't appear on the other side) -- WSL has its own opt-in `WSLENV`
            // passthrough, more trouble than it is worth. Pass values as inline shell assignments in the command
            // string instead, and run under `xvfb-run` for the virtual display GTK's offscreen rendering needs.
            let binary = runtime
                .host_path(&host)
                .ok_or_else(|| std::io::Error::other("the WebKit host is not on a drive WSL can see"))?;
            let shell = webkit_shell_command(initial_url, port, &token, &binary, dark)?;
            let mut command = Command::new("wsl.exe");
            command.args(["-d", WSL_DISTRO, "--", "bash", "-c", &shell]);
            command
        }
```

- Change line 165 to `let events_rx = spawn_reader_thread(from_view, Arc::clone(&frame), runtime, wake);`.

In `crates/manifold/src/process_manager/tests.rs`, make `reader_fed` take the runtime:

```rust
/// The host's reader of one engine connection, as a host of `runtime` feeds it; `feed` writes the engine's side of
/// it, its version first.
fn reader_fed(
    runtime: HostRuntime,
    feed: impl FnOnce(std::net::TcpStream) + Send + 'static,
) -> (mpsc::Receiver<ViewToHost>, view_io::FrameSlot, Arc<AtomicUsize>) {
```

and its last line:

```rust
    let reader = spawn_reader_thread(manifold_view_protocol::MessageChannel::new(stream), Arc::clone(&frame), runtime, wake);
    (reader, frame, wakes)
```

At its five existing callers (lines 69, 84, 114, 176, 213: `reader_fed(move |...` / `reader_fed(|...`), insert `HostRuntime::Native, ` as the first argument.

- [ ] **Step 5: Reveal and Copy Path read the stored path as-is**

In `crates/manifold/src/node_store.rs`, delete `windows_path_for_download` with its doc comment (lines 209-227).

In `crates/manifold/src/reveal.rs`, replace the doc lines 37-40 ("\"Open Containing Folder\" (`reveal_node`) and \"Copy Path\" use. A / download's path may be a WSL-side POSIX path (WebKit downloads), so / it goes through `windows_path_for_download`; an image/PDF/JSON path / always came from a real Windows file picker, so it's used as-is.") with:

```rust
    /// "Open Containing Folder" (`reveal_node`) and "Copy Path" use. A
    /// download's path is already in Windows terms (a WSL host's are mapped as
    /// they arrive, `process_manager::in_app_terms`), and an image/PDF/JSON path
    /// came from a Windows file picker, so both are used as-is.
```

(Lines 35-36 stay; 37-40 are replaced by the block above.) Change line 46 to:

```rust
            Runtime::Download(download) => Some(download.path.clone()),
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -j 8 -p manifold process_manager 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: `test result: ok.` with 0 failed, including both new tests and the existing reader tests.

Run: `cargo test -j 8 -p manifold reveal 2>&1 | grep -E "^test result|FAILED"`
Expected: ok, 0 failed.

- [ ] **Step 7: Commit**

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cargo fmt --all
git diff --stat
git add crates/manifold/src/process_manager/runtime.rs crates/manifold/src/process_manager.rs crates/manifold/src/process_manager/webkit.rs crates/manifold/src/process_manager/tests.rs crates/manifold/src/node_store.rs crates/manifold/src/reveal.rs
git commit -F - <<'EOF'
wsl: a WebKit host's paths are mapped in one place, both ways

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: six files in the stat; the hook passes.

### Task 3: One settings value for every host (`HostSettings`, protocol 7)

**Files:**
- Modify: `crates/manifold-view-protocol/Cargo.toml` (dependencies)
- Modify: `crates/manifold-view-protocol/src/messages.rs:216` (before `HostToView`), `:301-303`, `:714-716`, `:746`, `:843`
- Modify: `crates/manifold-view-protocol/src/lib.rs:15-19`, `:30-32`
- Modify: `crates/manifold-view-protocol/src/host_env.rs` (whole file's env part and tests)
- Modify: `crates/manifold-view-protocol/src/downloads.rs` (imports, new `folder` / `set_folder`)
- Modify: `crates/manifold-pages/src/lib.rs:101-102`
- Modify: `crates/manifold/src/process_manager.rs:22`, `ViewTab` (`:255-283`), `fake_fed` (`:395-407`), `spawn_tab` (`:980-1013`, `:1045-1047`, `:1095-1107`)
- Modify: `crates/manifold/src/process_manager/webkit.rs:33-78`, `:82-109`, `:167-182`
- Modify: `crates/manifold/src/process_manager/tests.rs:10-46`, `:1030-1036`
- Modify: `crates/manifold/src/settings_page.rs` (`download_folder`, screenshot doc `:816-822`, a test)
- Modify: `crates/manifold/src/tabs.rs:372-390`
- Modify: `crates/manifold/src/event_loop/frame.rs:1-3`, `:324-359`, tests `:363-374`
- Modify: `crates/manifold/src/event_loop.rs:339`
- Modify: `crates/manifold/src/main.rs:189`, `:505`
- Modify: `crates/manifold-view-webview2/src/main.rs:133`, `:415`
- Modify: `crates/manifold-view-cef/src/main.rs:142`, `:393`
- Modify: `crates/manifold-view-servo/src/main.rs:154-156`, `:331`
- Modify: `crates/manifold-view-webkitgtk/src/main.rs:23`, `:437`

**Interfaces:**
- Consumes: `HostRuntime::host_path` (Task 2), `profile::downloads_dir`, `settings::full_folder` and `Settings::download_directory` (Task 1).
- Produces:
  - `pub struct manifold_view_protocol::HostSettings { pub dark: bool, pub download_dir: Option<String> }` with `pub fn to_hex(&self) -> String`, `pub fn from_hex(hex: &str) -> Option<Self>`
  - `HostToView::SetSettings(HostSettings)`; `PROTOCOL_VERSION = 7`
  - `host_env::ENV_SETTINGS = "VIEW_SETTINGS"`; `HostEnv::settings: HostSettings`; `pub fn host_env::apply(settings: &HostSettings)`; `pub fn host_env::settings_from_env() -> HostSettings`
  - `pub fn downloads::folder() -> Option<PathBuf>`; `pub(crate) fn downloads::set_folder(dir: Option<&str>)`
  - `pub(crate) fn process_manager::host_settings(runtime: HostRuntime, dark: bool, folder: Option<&Path>) -> HostSettings`
  - `pub fn ViewTab::set_settings(&mut self, settings: &HostSettings)`
  - `pub fn spawn_tab(kind: ViewKind, tab_id: NodeId, initial_url: &str, custom_cef: Option<&Path>, settings: &HostSettings, jar_json: &dyn Fn() -> String, wake: view_io::Wake) -> Result<ViewTab, SpawnError>`
  - `pub(super) fn spawn_webkit_tab(runtime: HostRuntime, tab_id: NodeId, initial_url: &str, settings: &HostSettings, wake: view_io::Wake) -> std::io::Result<ViewTab>`
  - `pub(crate) fn App::download_folder(&self) -> Option<PathBuf>`, `pub(crate) fn App::host_settings(&self, runtime: HostRuntime) -> HostSettings`, `pub(crate) fn App::sync_host_settings(&mut self)`
  - Deletes `HostToView::SetTheme`, `host_env::ENV_DARK`, `HostEnv::dark`, `App::sync_theme`, `theme_updates`.

- [ ] **Step 1: Write the failing tests**

In `crates/manifold-view-protocol/src/host_env.rs`'s `mod tests`, add:

```rust
    /// Review Focus 2: every field crosses the environment exactly, as text no shell can read.
    #[test]
    fn settings_cross_the_environment_exactly() {
        for download_dir in [None, Some(r"C:\Users\Zoë Q\Downloads"), Some("/mnt/c/it's \"here\" $(x)")] {
            for dark in [false, true] {
                let settings = HostSettings { dark, download_dir: download_dir.map(str::to_string) };
                let hex = settings.to_hex();
                assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()), "{hex}");
                assert_eq!(HostSettings::from_hex(&hex).as_ref(), Some(&settings));
                let env = HostEnv::from_vars(
                    |name| match name {
                        ENV_SOCKET_ID => Some("t".into()),
                        ENV_SETTINGS => Some(hex.clone()),
                        _ => None,
                    },
                    false,
                );
                assert_eq!(env.settings, settings);
            }
        }
        assert_eq!(HostSettings::from_hex("zz"), None);
        assert_eq!(settings_from(Some("zz")), HostSettings::default(), "not ours: the defaults");
        assert_eq!(settings_from(None), HostSettings::default(), "missing: the defaults");
    }

    /// The only test in this crate that sets the global palette and download folder; the others pass a folder in.
    /// Keep it so, or serialize it with a lock: a crate's tests run on parallel threads.
    #[test]
    fn applied_settings_set_the_palette_and_the_download_folder() {
        apply(&HostSettings { dark: false, download_dir: Some(r"C:\Zoë Q\dl".into()) });
        assert!(!manifold_pages::is_dark());
        assert_eq!(crate::downloads::folder(), Some(std::path::PathBuf::from(r"C:\Zoë Q\dl")));
        apply(&HostSettings { dark: true, download_dir: None });
        assert!(manifold_pages::is_dark());
        assert_eq!(crate::downloads::folder(), None);
    }
```

In `crates/manifold/src/process_manager/tests.rs`, add:

```rust
/// Each host is given the folder as it sees it; one WSL can't see means no folder (downloads refused).
#[test]
fn a_hosts_settings_carry_the_folder_in_its_own_terms() {
    let folder = Path::new(r"C:\Users\Zoë Q\Downloads");
    assert_eq!(
        host_settings(HostRuntime::Native, true, Some(folder)),
        HostSettings { dark: true, download_dir: Some(r"C:\Users\Zoë Q\Downloads".into()) }
    );
    assert_eq!(
        host_settings(HostRuntime::Wsl, false, Some(folder)).download_dir.as_deref(),
        Some("/mnt/c/Users/Zoë Q/Downloads")
    );
    assert_eq!(host_settings(HostRuntime::Wsl, false, Some(Path::new(r"\\nas\dl"))).download_dir, None);
    assert_eq!(host_settings(HostRuntime::Native, false, None), HostSettings::default());
}
```

Replace `the_webkit_command_carries_the_dark_flag_only_when_dark` (lines 1030-1036) with:

```rust
/// Review Focus 2 (WSL): the settings reach `wsl.exe` only as hex, and settings too long for the command line are
/// refused by name.
#[test]
fn the_webkit_command_carries_the_settings_as_hex_only() {
    let token = [7u8; transport::TOKEN_LEN];
    let settings = HostSettings { dark: true, download_dir: Some("/mnt/c/Users/Zoë Q/it's \"dl\" $(x)".into()) };
    let shell = webkit_shell_command("https://a", 4000, &token, "/x", &settings).unwrap();
    assert!(shell.starts_with(&format!("VIEW_SETTINGS={} ", settings.to_hex())), "{shell}");
    assert!(!shell.contains("Zo") && !shell.contains("$(x)"), "{shell}");
    let long = HostSettings { dark: false, download_dir: Some("d".repeat(5000)) };
    let refused = webkit_shell_command("https://a", 4000, &token, "/x", &long).unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
}
```

In `crates/manifold/src/settings_page.rs`'s `mod tests`, add:

```rust
    #[test]
    fn a_relative_or_missing_download_folder_falls_back_and_an_absolute_one_is_used() {
        let mut app = App::default();
        assert_eq!(app.download_folder(), None, "no setting, and no Downloads folder under test");
        app.set_download_directory("Downloads2");
        assert_eq!(app.download_folder(), None, "not a full path: the default");
        let abs = crate::store::testutil::temp_dir("download-folder").join("Zoë Q downloads");
        app.set_download_directory(abs.to_str().unwrap());
        assert_eq!(app.download_folder(), Some(abs));
        // Typed with `/`: every path built on it reads one separator, which `Path` equality would not show.
        #[cfg(windows)]
        {
            app.set_download_directory("D:/dl");
            let folder = app.download_folder().map(|d| d.to_string_lossy().into_owned());
            assert_eq!(folder.as_deref(), Some(r"D:\dl"));
            app.set_download_directory("D:/dl/");
            let file = app.download_folder().map(|d| d.join("a.zip").to_string_lossy().into_owned());
            assert_eq!(file.as_deref(), Some(r"D:\dl\a.zip"), "no doubled separator");
        }
    }
```

In `crates/manifold/src/event_loop/frame.rs`, replace the test `a_theme_change_tells_every_tab_then_reloads_only_the_manifold_pages` (lines 365-374) with the two tests and helpers below, and add `use crate::session_sync::tests::live_tab;` under `use super::*;` in that `mod tests`:

```rust
    /// The settings in what a fake host was sent.
    fn settings_sent(sent: Vec<HostToView>) -> Vec<HostSettings> {
        sent.into_iter()
            .filter_map(|msg| match msg {
                HostToView::SetSettings(settings) => Some(settings),
                _ => None,
            })
            .collect()
    }

    /// What a fake host was sent, by kind.
    fn kinds(sent: Vec<HostToView>) -> Vec<&'static str> {
        sent.iter()
            .map(|msg| match msg {
                HostToView::SetSettings(_) => "settings",
                HostToView::Reload => "reload",
                _ => "other",
            })
            .collect()
    }

    /// Review Focus 5: a folder change reaches each open tab once, however many frames follow, and a tab whose
    /// spawn lands after it.
    #[test]
    fn a_folder_change_reaches_each_tab_once_and_a_tab_landing_later() {
        let mut app = App::default();
        let (_, mut first) = live_tab(&mut app, crate::ViewKind::Servo, "https://a.test/");
        app.sync_host_settings();
        let _ = first.sent();
        let folder = crate::store::testutil::temp_dir("host-settings").join("Zoë Q downloads");
        app.set_download_directory(folder.to_str().unwrap());
        let wanted = app.host_settings(HostRuntime::Native);
        assert_eq!(wanted.download_dir.as_deref(), folder.to_str());
        app.sync_host_settings();
        app.sync_host_settings();
        assert_eq!(settings_sent(first.sent()), [wanted.clone()], "once, though synced twice");
        let (_, mut later) = live_tab(&mut app, crate::ViewKind::Servo, "https://b.test/");
        app.sync_host_settings();
        assert_eq!(settings_sent(later.sent()), [wanted]);
        assert!(first.sent().is_empty(), "nothing new for the first tab");
    }

    #[test]
    fn a_theme_change_resends_the_settings_then_reloads_only_the_manifold_pages() {
        let mut app = App::default();
        let (_, mut web) = live_tab(&mut app, crate::ViewKind::Servo, "https://a.test/");
        let (_, mut page) = live_tab(&mut app, crate::ViewKind::Servo, "manifold://start");
        app.choose_theme(ThemeChoice::Dark);
        app.sync_host_settings();
        assert!(!kinds(page.sent()).contains(&"reload"), "the first frame only records the theme");
        let _ = web.sent();
        app.choose_theme(ThemeChoice::Light);
        app.sync_host_settings();
        assert_eq!(kinds(web.sent()), ["settings"]);
        assert_eq!(kinds(page.sent()), ["settings", "reload"]);
    }
```

- [ ] **Step 2: Run the protocol tests to see them fail**

Run: `cargo test -j 8 -p manifold-view-protocol 2>&1 | tail -n 15`
Expected: compile errors: `HostSettings`, `ENV_SETTINGS`, `settings_from`, `apply` and `downloads::folder` not found, and `from_vars` takes 3 arguments.

- [ ] **Step 3: The protocol**

`crates/manifold-view-protocol/Cargo.toml`, `[dependencies]`: add `manifold-pages.workspace = true`.

`crates/manifold-view-protocol/src/messages.rs`, before `/// Messages sent from the manifold process manager to an view-host.` (line 216), add:

```rust
/// The app's settings a host acts on, given at spawn (`host_env::ENV_SETTINGS`) and sent again on every change
/// (`HostToView::SetSettings`); applied by `host_env::apply`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSettings {
    /// Serve `manifold://` pages in the dark palette (a page already open keeps its colors until `Reload`).
    pub dark: bool,
    /// The absolute folder downloads are saved in, as this host sees it (a `/mnt/...` path under WSL). `None`: every
    /// download is refused (reported started and failed).
    pub download_dir: Option<String>,
}
```

Replace `HostToView::SetTheme`'s doc and variant (lines 301-303) with:

```rust
    /// The app's settings changed (`HostSettings`): sent to every host whose settings differ, once per change. A
    /// download already running keeps its path.
    SetSettings(HostSettings),
```

In the wire-shape tests: line 746 becomes `HostToView::SetSettings(_) => 20,`; line 843 becomes:

```rust
            HostToView::SetSettings(HostSettings {
                dark: true,
                download_dir: Some("C:\\Users\\Zoë Q\\Downloads".into()),
            }),
```

`crates/manifold-view-protocol/src/lib.rs`: add `HostSettings` to the `pub use messages::{...}` list (lines 15-19). Add the history line after `/// 6: ...` (line 31), and set the version (line 32):

```rust
/// 7: `HostToView::SetSettings` (the theme and the download folder) replaces `SetTheme`.
pub const PROTOCOL_VERSION: u32 = 7;
```

`crates/manifold-view-protocol/src/downloads.rs`: under the existing `use std::path::{Path, PathBuf};` (line 5), add `use std::sync::{Mutex, PoisonError};`, and add after the imports:

```rust
/// The folder the app last set (`host_env::apply`), read for each new download.
static FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets `folder` (`host_env::apply`): a download already running keeps its path.
pub(crate) fn set_folder(dir: Option<&str>) {
    *FOLDER.lock().unwrap_or_else(PoisonError::into_inner) = dir.map(PathBuf::from);
}

/// The folder this host saves its next download in, as the app last set it (`host_env::apply`); `None` until then,
/// and when the app has none to give.
pub fn folder() -> Option<PathBuf> {
    FOLDER.lock().unwrap_or_else(PoisonError::into_inner).clone()
}
```

`crates/manifold-view-protocol/src/host_env.rs`:
- Header: after its first sentence, add "The app's settings a host acts on (`HostSettings`) come the same way at spawn, as hex (`ENV_SETTINGS`), and later as `HostToView::SetSettings`; a host applies both with `apply`."
- Imports: `use crate::{transport, HostSettings, HostToView};`.
- Replace `ENV_DARK` (lines 16-17) with:

```rust
/// The app's settings at spawn, as `HostSettings::to_hex`; later changes arrive as `HostToView::SetSettings`. The
/// WebKit host under WSL reads it from its command line, where hex is shell-safe.
pub const ENV_SETTINGS: &str = "VIEW_SETTINGS";
```

- In `HostEnv`, replace the `dark` field and its doc (lines 32-33) with:

```rust
    /// The app's settings at spawn (`ENV_SETTINGS`), for the host to `apply`.
    pub settings: HostSettings,
```

- Replace `from_env` and `from_vars` (lines 46-62) with:

```rust
    /// Panics without `ENV_SOCKET_ID`: a host started by hand cannot work.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok(), std::env::var_os(ENV_AWAIT_COOKIES).is_some())
    }

    fn from_vars(get: impl Fn(&str) -> Option<String>, await_cookies: bool) -> Self {
        Self {
            socket_id: get(ENV_SOCKET_ID).unwrap_or_else(|| panic!("{ENV_SOCKET_ID} env var required")),
            initial_url: get(ENV_INITIAL_URL).unwrap_or_else(|| DEFAULT_URL.to_string()),
            await_cookies,
            settings: settings_from(get(ENV_SETTINGS).as_deref()),
        }
    }
```

- After `impl HostEnv { ... }` (before `#[cfg(test)]`), add:

```rust
impl HostSettings {
    /// Hex of the postcard bytes: one variable for every field, shell-safe on the WSL command line as the URL is.
    pub fn to_hex(&self) -> String {
        transport::to_hex(&postcard::to_stdvec(self).expect("HostSettings always encodes"))
    }

    /// `to_hex` read back; `None` for anything else.
    pub fn from_hex(hex: &str) -> Option<Self> {
        postcard::from_bytes(&transport::from_hex(hex)?).ok()
    }
}

/// `ENV_SETTINGS` for a host that reads no other `HostEnv` variable (WebKit).
pub fn settings_from_env() -> HostSettings {
    settings_from(std::env::var(ENV_SETTINGS).ok().as_deref())
}

/// The settings `ENV_SETTINGS` holds: the defaults when it is missing, or (logged) when it is not `to_hex`'s.
fn settings_from(hex: Option<&str>) -> HostSettings {
    let Some(hex) = hex else { return HostSettings::default() };
    HostSettings::from_hex(hex).unwrap_or_else(|| {
        eprintln!("{ENV_SETTINGS} is not a HostSettings encoding; using the defaults");
        HostSettings::default()
    })
}

/// Acts on `settings`, at spawn and on every `HostToView::SetSettings`: the palette `manifold://` pages are served
/// in, and the folder the next download is saved in (`downloads::folder`).
pub fn apply(settings: &HostSettings) {
    manifold_pages::set_dark(settings.dark);
    crate::downloads::set_folder(settings.download_dir.as_deref());
}
```

- Existing tests:
  - In `the_url_defaults_and_the_socket_id_is_required`:
    - the `HostEnv { ..., dark: false }` literal becomes `HostEnv { ..., settings: HostSettings::default() }`;
    - every `from_vars` call drops its third argument;
    - `assert!(env.dark);` (line 95) becomes the line below (every variable there reads `<NAME>`, which is not hex):

```rust
        assert_eq!(env.settings, HostSettings::default(), "<VIEW_SETTINGS> is not hex: the defaults");
```

  - In `a_seed_is_split_from_any_other_first_message` and `a_host_not_awaiting_cookies_leaves_its_messages_alone`, `dark: false` becomes `settings: HostSettings::default()`.

`crates/manifold-pages/src/lib.rs`, `set_dark`'s doc (lines 101-102) becomes:

```rust
/// Which palette `serve` fills in: each host sets it from the app's settings
/// (`manifold_view_protocol::host_env::apply`); a page already open keeps the colors it was served with until reloaded.
```

- [ ] **Step 4: Record the new wire shape**

Run: `cargo test -j 8 -p manifold-view-protocol wire_shape 2>&1 | grep -E "fingerprint|PROTOCOL_VERSION" | head -n 3`
Expected: FAIL whose message reads "the HostToView/ViewToHost wire shape changed (fingerprint 0x<16 hex digits>, recorded 0x956dba4de3c76183)" (the `#018x` format prints no underscores).

In `messages.rs` (lines 714-716), set `RECORDED_FOR_VERSION` to `7` and `RECORDED_FINGERPRINT` to the `0x....` value that message printed (keep the `0x956d_ba4d_e3c7_6183` grouping style: underscores every four hex digits).

Run: `cargo test -j 8 -p manifold-view-protocol 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: every `test result:` is `ok`, 0 failed (both new host_env tests included).

- [ ] **Step 5: The app**

`crates/manifold/src/process_manager.rs`:
- Line 22 becomes `use manifold_view_protocol::{HostSettings, HostToView, ViewToHost};`.
- In `ViewTab`, after the `first_frame` field (line 282, the struct's last), add:

```rust
    /// The settings this host was last given (at spawn, then `set_settings`).
    settings: HostSettings,
```

- In `fake_fed`'s `ViewTab { ... }` literal (line 406), add `settings: HostSettings::default(),`.
- Add to `impl ViewTab` (after `set_visible` or any `pub fn` in it):

```rust
    /// Tells the host the app's settings (`HostToView::SetSettings`), if they differ from what it was last given; only
    /// `App::sync_host_settings` decides them.
    pub fn set_settings(&mut self, settings: &HostSettings) {
        if self.settings != *settings {
            self.settings = settings.clone();
            self.send(&HostToView::SetSettings(settings.clone()));
        }
    }
```

- Add a free function after `in_app_terms`:

```rust
/// The settings a host that runs as `runtime` acts on: `dark`, and `folder` as that host sees it
/// (`HostRuntime::host_path`; `None` when it cannot see it, so its downloads are refused).
pub(crate) fn host_settings(runtime: HostRuntime, dark: bool, folder: Option<&Path>) -> HostSettings {
    HostSettings { dark, download_dir: folder.and_then(|folder| runtime.host_path(folder)) }
}
```

- `spawn_tab`:
  - Add the parameter `settings: &HostSettings,` after `custom_cef: Option<&Path>,` (line 990).
  - Add a doc paragraph before `/// \`wake\` is called ...` (line 985): `/// \`settings\` is what the host acts on from the start (\`App::host_settings\`).` followed by `///`.
  - Line 1013 becomes `return Ok(spawn_webkit_tab(runtime, tab_id, initial_url, settings, wake)?);`.
  - Replace lines 1045-1047 (`if manifold_pages::is_dark() { command.env(host_env::ENV_DARK, "1"); }`) with `command.env(host_env::ENV_SETTINGS, settings.to_hex());`.
  - In the `ViewTab { ... }` literal (line 1095), add `settings: settings.clone(),`.

`crates/manifold/src/process_manager/webkit.rs`:
- Replace `webkit_env` (lines 33-58, doc included) with:

```rust
/// What the host reads (`manifold-view-webkitgtk`'s `main`) besides `VIEW_HOST_IP`, the address it dials back, which
/// differs by runtime. Page text (the URL) and the settings (a download folder's path) appear only as hex.
pub(super) fn webkit_env(
    initial_url: &str,
    port: u16,
    token: &transport::Token,
    settings: &HostSettings,
) -> std::io::Result<Vec<(&'static str, String)>> {
    /// Windows' command line holds 32 767 characters; leave the rest of this one room.
    const MAX_URL_HEX: usize = 16_384;
    /// The settings' share of it: a download folder's path of about 2 000 bytes.
    const MAX_SETTINGS_HEX: usize = 4_096;
    let url_hex = transport::to_hex(initial_url.as_bytes());
    if url_hex.len() > MAX_URL_HEX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the URL is too long to start a WebKit tab with",
        ));
    }
    let settings_hex = settings.to_hex();
    if settings_hex.len() > MAX_SETTINGS_HEX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the download folder's path is too long to start a WebKit tab with",
        ));
    }
    Ok(vec![
        (host_env::ENV_SETTINGS, settings_hex),
        ("VIEW_INITIAL_URL_HEX", url_hex),
        ("VIEW_TCP_PORT", port.to_string()),
        ("VIEW_TOKEN", transport::to_hex(token)),
    ])
}
```

- `webkit_shell_command`: the last parameter `dark: bool` becomes `settings: &HostSettings`, and its body's call becomes `webkit_env(initial_url, port, token, settings)?`.
- `spawn_webkit_tab`:
  - Its parameters become `(runtime: HostRuntime, tab_id: NodeId, initial_url: &str, settings: &HostSettings, wake: view_io::Wake)`.
  - Delete `let dark = manifold_pages::is_dark();` (line 109).
  - `webkit_shell_command(initial_url, port, &token, &binary, dark)?` becomes `webkit_shell_command(initial_url, port, &token, &binary, settings)?`.
  - `webkit_env(initial_url, port, &token, dark)?` becomes `webkit_env(initial_url, port, &token, settings)?`.
  - In its `ViewTab { ... }` literal, add `settings: settings.clone(),`.

`crates/manifold/src/process_manager/tests.rs`:
- Lines 13-14 (`page_controlled_text_never_reaches_the_wsl_command_line`): the `build` closure passes `&HostSettings::default()` instead of `false`.
- Line 29 (`a_url_too_long_for_a_command_line_is_refused_by_name`): the same change.
- Replace lines 37-45 (`the_webkit_host_environment_is_the_same_under_every_runtime`'s body) with:

```rust
    let token = [7u8; transport::TOKEN_LEN];
    let settings = HostSettings { dark: true, download_dir: Some("/mnt/c/dl".into()) };
    let env = webkit_env("https://a", 4000, &token, &settings).unwrap();
    let names: Vec<_> = env.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["VIEW_SETTINGS", "VIEW_INITIAL_URL_HEX", "VIEW_TCP_PORT", "VIEW_TOKEN"]);
    let wsl = webkit_shell_command("https://a", 4000, &token, "/x", &settings).unwrap();
    for (name, value) in &env {
        assert!(wsl.contains(&format!("{name}={value} ")), "{name} in {wsl}");
    }
```

`crates/manifold/src/settings_page.rs`, in the `impl App` that holds `set_download_directory`, add:

```rust
    /// The folder downloads go to: the Download folder setting when it is a full path, in Windows' own form
    /// (`settings::full_folder`, the form the row's verdict reads too), else the user's Downloads folder
    /// (`profile::downloads_dir`; `None` without one, and under `cargo test`).
    pub(crate) fn download_folder(&self) -> Option<std::path::PathBuf> {
        let setting = self.settings.value.download_directory.as_deref();
        setting.and_then(crate::settings::full_folder).or_else(profile::downloads_dir)
    }
```

and replace `screenshot_default_path`'s doc lines 816-819 ("`profile::pictures_dir()`\manifold-<unix-seconds>.png, ... on the manifold side for the toolbar's "Screenshot" button.") with:

```rust
/// `profile::pictures_dir()`\manifold-<unix-seconds>.png, or the temp folder without one — the same
/// "pick the obvious default, no save-as dialog" posture downloads take (`App::download_folder`),
/// on the manifold side for the toolbar's "Screenshot" button. The
```

(the doc's remaining lines, from "timestamp is the simplest", stay).

`crates/manifold/src/tabs.rs`: after `let custom_cef = self.custom_cef_dir();` (line 372), add:

```rust
        let runtime = process_manager::runtime::HostRuntime::of(spawn.kind)
            .unwrap_or(process_manager::runtime::HostRuntime::Native);
        let settings = self.host_settings(runtime);
```

and the call (line 389-390) becomes:

```rust
            let result = process_manager::spawn_tab(
                spawn.kind,
                spawn.id,
                &spawn.url,
                custom_cef.as_deref(),
                &settings,
                &seed,
                wake,
            );
```

`crates/manifold/src/event_loop/frame.rs`: under `use super::*;` (line 3), add:

```rust
use crate::process_manager::runtime::HostRuntime;
use manifold_view_protocol::HostSettings;
```

Delete the free function `theme_updates` with its doc (lines 352-359, after the `impl` block) and the blank line before it. Then replace `sync_theme` with its doc (lines 324-349, the last method of the `impl` block; the block's closing `}` at line 350 stays) with:

```rust
    /// What a host that runs as `runtime` is told of the app's settings, at spawn (`spawn_tab_async`) and on a change
    /// (`sync_host_settings`): the theme the app shows, and the download folder (`download_folder`).
    pub(crate) fn host_settings(&self, runtime: HostRuntime) -> HostSettings {
        let dark = self.egui_ctx.theme() == egui::Theme::Dark;
        crate::process_manager::host_settings(runtime, dark, self.download_folder().as_deref())
    }

    /// Keeps every tab's host settings equal to the app's (`ViewTab::set_settings` sends only a difference: a change,
    /// or a tab that spawned before one landed). After the first frame, a theme change also reloads the tabs showing
    /// a `manifold://` page so they take the new palette.
    pub(crate) fn sync_host_settings(&mut self) {
        let now = self.egui_ctx.theme();
        let first = self.last_theme.is_none();
        // The first frame only records the theme: the spawn environment already carried it.
        let reload_pages = theme_changed(&mut self.last_theme, now) && !first;
        let native = self.host_settings(HostRuntime::Native);
        let wsl = self.host_settings(HostRuntime::Wsl);
        let scheme = format!("{}://", manifold_pages::SCHEME);
        for node in self.nodes.values_mut() {
            let Some(tab) = node.as_tab_mut() else { continue };
            let wanted = if HostRuntime::of(tab.tab.kind) == Some(HostRuntime::Wsl) { &wsl } else { &native };
            tab.tab.set_settings(wanted);
            if reload_pages && tab.url.starts_with(&scheme) {
                tab.tab.send(&HostToView::Reload);
            }
        }
    }
```

`crates/manifold/src/event_loop.rs:339`: `self.sync_theme();` becomes `self.sync_host_settings();`.

`crates/manifold/src/main.rs`: the doc at line 189 becomes `/// The theme \`sync_host_settings\` last saw (\`None\` before the first frame): a later change reloads the tabs showing a \`manifold://\` page.`; delete line 505 (`manifold_pages::set_dark(egui_ctx.theme() == egui::Theme::Dark);`).

- [ ] **Step 6: The hosts**

- `crates/manifold-view-webview2/src/main.rs:133` and `crates/manifold-view-cef/src/main.rs:142`: `manifold_pages::set_dark(host_env.dark);` becomes:

```rust
    manifold_view_protocol::host_env::apply(&host_env.settings);
```

- `crates/manifold-view-servo/src/main.rs:154-156` becomes:

```rust
    let manifold_view_protocol::HostEnv { socket_id, initial_url, settings, .. } =
        manifold_view_protocol::HostEnv::from_env();
    manifold_view_protocol::host_env::apply(&settings);
```

- `crates/manifold-view-webkitgtk/src/main.rs:23` becomes:

```rust
    manifold_view_protocol::host_env::apply(&manifold_view_protocol::host_env::settings_from_env());
```

- The four match arms (webview2 `main.rs:415`, cef `main.rs:393`, servo `main.rs:331`, webkitgtk `main.rs:437`), `HostToView::SetTheme { dark } => manifold_pages::set_dark(dark),`, each become:

```rust
                HostToView::SetSettings(settings) => manifold_view_protocol::host_env::apply(&settings),
```

(PDFium's host has no `HostToView` match: `grep -n SetTheme crates/manifold-view-pdfium/src` finds nothing, so it needs no change.)

- [ ] **Step 7: Run the tests and build what builds here**

Run: `cargo test -j 8 -p manifold-view-protocol -p manifold-pages -p manifold 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: all `ok`, 0 failed; the new tests listed in Step 1 among them.

Run: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate 2>&1 | tail -n 3`
Expected: `Finished`.

Run: `ls target/debug/build | grep -c '^cef-dll-sys'`
If it prints 1 or more (the CEF download is present), run `cargo check -j 8 -p manifold-view-cef 2>&1 | tail -n 3` and expect `Finished`. If it prints 0, or the check fails inside `cef-dll-sys`'s build script, do not run the build that downloads CEF: write "CEF host not compiled: <reason>" into this task's report.

Run (WebKit, in WSL; a warm build is a few minutes):
`wsl.exe -d Ubuntu --cd "C:\\Users\\Winston Li\\Documents\\GitHub\\manifold" -- bash -lc 'CARGO_TARGET_DIR=target-linux cargo build -j 6 -p manifold-view-webkitgtk' 2>&1 | tail -n 3`
Expected: `Finished`. If WSL or its toolchain is missing, report "WebKit host not compiled: <first error line>" rather than claiming it.

Run: `ls target-servo/debug/manifold-view-servo.exe && df -h /c | tail -n 1`
If the exe exists and at least 5 GB are free, rebuild warm through the wrapper:

```bash
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name servo-build-settings -Detach -Command 'set "CARGO_TARGET_DIR=target-servo"& cargo build -j 6 -p manifold-view-servo'
```

Then wait in a background Bash call for `"end"` in that run's `run.json` (Task 6 Step 9 shows the loop; deadline 2400 s). Expected: exit 0. With no `target-servo` build, or less than 5 GB free, report "Servo host not compiled (no warm target-servo)" instead.

- [ ] **Step 8: Commit**

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cargo fmt --all
git diff --stat
git add crates/manifold-view-protocol crates/manifold-pages/src/lib.rs crates/manifold/src crates/manifold-view-webview2/src/main.rs crates/manifold-view-cef/src/main.rs crates/manifold-view-servo/src/main.rs crates/manifold-view-webkitgtk/src/main.rs Cargo.lock
git commit -F - <<'EOF'
hosts: one HostSettings (theme and download folder) at spawn and live; protocol 7

SetSettings replaces SetTheme; VIEW_SETTINGS replaces VIEW_DARK.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: the stat lists only the Files above (plus `Cargo.lock` for the new protocol dependency, plus `docs/runs.md` NOT staged if the Servo run added a row: commit that row separately as `runs: servo warm build row`); the hook passes.

### Task 4: CEF and WebKit save into the app's folder, or report the download failed

**Files:**
- Modify: `crates/manifold-view-protocol/src/downloads.rs:1-14` (module doc, delete `downloads_dir`), new `destination` and `refused`, tests
- Modify: `crates/manifold-view-cef/src/download_handler.rs:63-93`
- Modify: `crates/manifold-view-webkitgtk/src/main.rs:72-83`, `:109-124`

**Interfaces:**
- Consumes: `downloads::folder()` (Task 3), `safe_name`, `unique_path`, `same_download_path` (existing).
- Produces:
  - `pub fn downloads::destination(dir: Option<&Path>, suggested: &str, taken: impl Fn(&Path) -> bool) -> Result<PathBuf, PathBuf>`
  - `pub fn downloads::refused(path: String, url: Option<String>) -> [ViewToHost; 2]`
  - Deletes `downloads::downloads_dir`.

- [ ] **Step 1: Write the failing tests**

In `crates/manifold-view-protocol/src/downloads.rs`'s `mod tests`, add:

```rust
    /// Review Focus 3: a folder that can't be had refuses the download; nothing is saved anywhere else.
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
        assert_eq!(destination(None, "b.zip", none), Err(PathBuf::from("b.zip")), "no folder");
        let rel = Path::new("manifold-relative-dl");
        assert_eq!(destination(Some(rel), "b.zip", none), Err(rel.join("b.zip")), "not a full path");
        assert!(!rel.exists(), "a relative folder is never made");
        let file = root.join("a file");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(destination(Some(file.as_path()), "c.zip", none), Err(file.join("c.zip")), "a file in its place");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_download_starts_and_fails_at_once() {
        let [started, failed] = refused(r"D:\dl\a.zip".into(), Some("http://h/a".into()));
        assert!(
            matches!(&started, ViewToHost::DownloadStarted { path, total_bytes: None, url }
                if path == r"D:\dl\a.zip" && url.as_deref() == Some("http://h/a")),
            "{started:?}"
        );
        assert!(matches!(&failed, ViewToHost::DownloadFailed { path } if path == r"D:\dl\a.zip"), "{failed:?}");
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 8 -p manifold-view-protocol downloads:: 2>&1 | tail -n 10`
Expected: compile errors: `destination` and `refused` not found.

- [ ] **Step 3: The shared rule's last two pieces**

Replace `downloads.rs`'s module doc (lines 1-3) with:

```rust
//! Where engines put downloads and how they name them: the folder the app set (`folder`), and one rule for every
//! engine (`destination`), so a download's `path` (the host's key for it) is the same string in every message about
//! it.
```

Delete `downloads_dir` with its doc (lines 9-14). After `same_download_path` (before `ntfs_upper`), add:

```rust
/// Where a download the server suggested as `suggested` is saved in `dir`: `unique_path` of its `safe_name`, the
/// folder made first if missing. `Err` is the path it would have had when there is no folder, it is not absolute, or
/// it cannot be made: the host then refuses the download (`refused`) rather than saving it anywhere else.
pub fn destination(dir: Option<&Path>, suggested: &str, taken: impl Fn(&Path) -> bool) -> Result<PathBuf, PathBuf> {
    let name = safe_name(suggested);
    let Some(dir) = dir.filter(|dir| dir.is_absolute()) else {
        return Err(dir.map_or_else(|| PathBuf::from(&name), |dir| dir.join(&name)));
    };
    std::fs::create_dir_all(dir).map_err(|_| dir.join(&name))?;
    Ok(unique_path(dir, &name, taken))
}

/// The messages for a download the host refused (`destination`'s `Err`): it starts and fails at once, so the app
/// shows a failed card.
pub fn refused(path: String, url: Option<String>) -> [ViewToHost; 2] {
    [ViewToHost::DownloadStarted { path: path.clone(), total_bytes: None, url }, ViewToHost::DownloadFailed { path }]
}
```

- [ ] **Step 4: CEF**

In `crates/manifold-view-cef/src/download_handler.rs`, replace the comment and body of `on_before_download` (lines 63-93: the `// Must call ...` comment through the closing `}` of the function) with:

```rust
        // CEF has no default save location of its own and must be told one (`callback.cont`), or the download never
        // starts. The path comes from the rule every host uses (`downloads::destination`): the folder the app set,
        // made if missing, and the suggested name's last component, numbered if a file or a download still in
        // `assigned` has it. `show_dialog: false`: there is no window for a save-as dialog. A download that can't be
        // placed is reported started and failed (`downloads::refused`) and handed back to CEF's default handling
        // (return 0), which cancels it for a windowless browser (Alloy style); `on_download_updated` reports nothing
        // of it, since its id was never assigned.
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(callback)) = (download_item, callback) else { return true as _ };
            let suggested = suggested_name.map(|s| s.to_string()).unwrap_or_default();
            let mut results = self.handler.result.borrow_mut();
            let decided = manifold_view_protocol::downloads::destination(
                manifold_view_protocol::downloads::folder().as_deref(),
                &suggested,
                |p| {
                    p.exists()
                        || results
                            .assigned
                            .values()
                            .any(|taken| manifold_view_protocol::downloads::same_download_path(taken, p))
                },
            );
            match decided {
                Ok(path) => {
                    results.assigned.insert(item.id(), path.clone());
                    drop(results);
                    callback.cont(Some(&path.to_string_lossy().as_ref().into()), false as _);
                    true as _
                }
                Err(path) => {
                    let url = Some(CefString::from(&item.url()).to_string()).filter(|u| !u.is_empty());
                    let messages = manifold_view_protocol::downloads::refused(path.to_string_lossy().into_owned(), url);
                    results.queue.extend(messages);
                    0
                }
            }
        }
```

- [ ] **Step 5: WebKit**

In `crates/manifold-view-webkitgtk/src/main.rs`, replace the first paragraph of the downloads comment (lines 72-83, "Downloads: WebKitGTK has no default ... so a `#` or space in a name works.") with:

```rust
    // Downloads: WebKitGTK's own default would save into ~/Downloads, and `decide-destination` must call
    // `set_destination` or the download silently never starts, the same requirement as CEF's
    // `on_before_download`/`BeforeDownloadCallback::cont`. The path comes from the rule every host uses
    // (`manifold_view_protocol::downloads::destination`): the folder the app set (`downloads::folder`, a
    // `/mnt/<drive>/...` path under WSL), made if missing, and the suggested name's last component, numbered if
    // taken (WebKit doesn't overwrite, so a second download of one name would otherwise fail). With no folder, or one
    // that can't be made, the download is reported started and failed (`downloads::refused`) and cancelled. `reserved`
    // holds names handed out and not yet finished, so concurrent same-name downloads get distinct names before either
    // file exists. The destination is set as a properly escaped `file://` URI (`glib::filename_to_uri`), so a `#` or
    // space in a name works.
```

Replace the body of the `connect_decide_destination` closure (lines 109-124, from `let dir = ...downloads_dir();` through `true`) with:

```rust
                let url = download.request().and_then(|r| r.uri()).map(|u| u.to_string());
                let decided = manifold_view_protocol::downloads::destination(
                    manifold_view_protocol::downloads::folder().as_deref(),
                    suggested_filename,
                    |p| p.exists() || reserved_for_destination.borrow().contains(p),
                )
                .and_then(|dest| match gtk::glib::filename_to_uri(&dest, None) {
                    Ok(uri) => Ok((dest, uri)),
                    Err(_) => Err(dest),
                });
                let (dest, uri) = match decided {
                    Ok(pair) => pair,
                    Err(dest) => {
                        // Refused: reported started and failed, then cancelled, since WebKit's own default would save
                        // it into ~/Downloads. `dest_for_events` stays empty, so its later signals report nothing.
                        pending_for_destination
                            .borrow_mut()
                            .extend(manifold_view_protocol::downloads::refused(dest.to_string_lossy().into_owned(), url));
                        download.cancel();
                        return true;
                    }
                };
                download.set_destination(&uri);
                reserved_for_destination.borrow_mut().insert(dest.clone());
                let path = dest.to_string_lossy().to_string();
                *dest_for_destination.borrow_mut() = Some(path.clone());
                let total_bytes = download.response().map(|r| r.content_length()).filter(|&n| n > 0);
                pending_for_destination.borrow_mut().push(ViewToHost::DownloadStarted { path, total_bytes, url });
                true
```

- [ ] **Step 6: Run the tests and the host checks**

Run: `cargo test -j 8 -p manifold-view-protocol 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: ok, 0 failed (the two new tests and the existing `downloads::` ones).

Run: `git grep -n "downloads_dir" -- crates/manifold-view-protocol crates/manifold-view-cef crates/manifold-view-webkitgtk crates/manifold-view-webview2`
Expected: no output.

Run the CEF check and the WSL WebKit build exactly as in Task 3 Step 7 (same conditions, same reporting rule). Expected: `Finished` for each one that can run.

- [ ] **Step 7: Commit**

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cargo fmt --all
git diff --stat
git add crates/manifold-view-protocol/src/downloads.rs crates/manifold-view-cef/src/download_handler.rs crates/manifold-view-webkitgtk/src/main.rs
git commit -F - <<'EOF'
downloads: CEF and WebKit save into the folder the app sets, or report the download failed

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: three files; the hook passes.

### Task 5: Every gate instance downloads into its own profile

**Files:**
- Modify: `crates/gate/src/instance.rs:98-111`, tests (`:397-406`)

**Interfaces:**
- Consumes: `Settings::download_directory` (Task 1; the app reads it from `settings.json`).
- Produces: `pub fn instance::download_dir(profile: &Path) -> io::Result<PathBuf>`; `seed_settings` writes `"download_directory"`.

- [ ] **Step 1: Write the failing tests**

In `crates/gate/src/instance.rs`'s `seeded_settings_skip_the_demo` (line 398), after `assert_eq!(v["x"], 1);`, add:

```rust
        assert_eq!(v["download_directory"], download_dir(&dir).unwrap().to_str().unwrap());
        assert!(Path::new(v["download_directory"].as_str().unwrap()).is_absolute());
```

and add a test:

```rust
    #[test]
    fn an_instance_downloads_into_its_own_profile_by_full_path() {
        let dir = download_dir(Path::new("rel-profile")).unwrap();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(dir.ends_with(Path::new("rel-profile").join("downloads")), "{}", dir.display());
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 8 -p gate instance:: 2>&1 | tail -n 8`
Expected: compile error: `download_dir` not found.

- [ ] **Step 3: Seed the folder**

Replace `seed_settings` and its doc (lines 98-111) with:

```rust
/// The download folder of an instance seeded with `seed_settings(profile, ..)`: `<profile>\downloads`, absolute, so
/// the app uses it (it ignores a relative one) and a shot can name each file exactly.
pub fn download_dir(profile: &Path) -> io::Result<PathBuf> {
    std::path::absolute(profile.join("downloads"))
}

/// `PROFILE/settings.json` that skips the first-launch demo and starts tabs on `view_kind`. (`--profile-dir DIR`
/// names the manifold folder itself, so there is no `manifold/` level as under `%APPDATA%`.) `more` is extra JSON
/// members, starting with `", "`. The theme is pinned (default `dark`, or `GATE_THEME=light`) so a run does not
/// depend on the machine's own theme, and every instance downloads into its own profile (`download_dir`), never the
/// user's Downloads folder; the app makes that folder at the first download.
pub fn seed_settings(profile: &Path, view_kind: &str, more: &str) -> io::Result<()> {
    fs::create_dir_all(profile)?;
    let theme = if std::env::var("GATE_THEME").is_ok_and(|t| t == "light") { "light" } else { "dark" };
    let downloads = serde_json::Value::from(download_dir(profile)?.to_string_lossy().into_owned());
    fs::write(
        profile.join("settings.json"),
        format!(
            "{{\"has_shown_first_launch_demo\": true, \"default_starting_view_kind\": \"{view_kind}\", \"theme\": \"{theme}\", \"download_directory\": {downloads}{more}}}"
        ),
    )
}
```

(`instance.rs` already imports `PathBuf`: `use std::path::{Path, PathBuf};`, line 17.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -j 8 -p gate instance:: 2>&1 | grep -E "^test result|FAILED"`
Expected: ok, 0 failed.

Run: `git grep -n "download_directory" -- crates/gate/src | grep -v "^crates/gate/src/instance.rs:"`
Expected: no output (no caller's `more` adds the key a second time; in a `more` string it would read `\"download_directory\"`, which this plain pattern also matches).

- [ ] **Step 5: Commit**

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cargo fmt --all
git diff --stat
git add crates/gate/src/instance.rs
git commit -F - <<'EOF'
gate: every instance downloads into its own profile folder

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: one file; the hook passes.

### Task 6: WebView2 names every download (the shared rule and the retry rule) and the resume shot

The shared rule and the retry rule land in one commit. The shared rule alone would number the plain retry `slow (1).zip` and break `download-failed`.

**Files:**
- Modify (rewrite): `crates/manifold-view-webview2/src/download.rs`
- Modify (rewrite): `crates/gate/src/shots_net.rs`
- Modify: `crates/gate/src/shots.rs:59`, `:63`, `:194-206` (`Shots`), `:264-273` (`Shots::start`'s literal), `:660`, `:1326-1374` (`download_views`)

**Interfaces:**
- Consumes: `downloads::destination`, `downloads::refused`, `downloads::folder`, `downloads::same_download_path` (Tasks 3-4); `instance::download_dir` (Task 5).
- Produces:
  - WebView2: `fn retry_path<'a>(held: &[Seen<'a>], uri: &str) -> Option<&'a Path>`, `fn save_path(held: &[Seen<'_>], uri: &str, broken_at_start: bool, proposed: &str, folder: Option<&Path>) -> Result<PathBuf, PathBuf>`, `fn release<T>(held: &mut Vec<T>, path: &Path, path_of: impl Fn(&T) -> &Path)`; `register` unchanged in signature.
  - Gate: `pub fn shots_net::resume_body() -> Vec<u8>`, `Downloads::break_transfers(&self)`, `Downloads::ranges_answered(&self) -> usize`; the chrome-shots view `download-resume`; `Shots::downloads: PathBuf`.

- [ ] **Step 1: Write the failing tests of the retry rule (RED)**

In `crates/manifold-view-webview2/src/download.rs`:
- Change the imports to:

```rust
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use manifold_view_protocol::{downloads, ViewToHost};
```

  (the `webview2_com` and `windows::core` imports stay as they are for now).
- Append at the end of the file:

```rust
/// What `save_path` reads of a held download.
struct Seen<'a> {
    path: &'a Path,
    uri: &'a str,
    /// Its transfer broke (`InterruptReason` not none) while it still reads in progress: the engine is retrying it.
    broken: bool,
}

/// The path of the download a retry of `uri` retries: the latest held one of that URL whose transfer broke.
fn retry_path<'a>(held: &[Seen<'a>], uri: &str) -> Option<&'a Path> {
    held.iter().rev().find(|h| h.broken && h.uri == uri).map(|h| h.path)
}

/// RED: the shared rule only, no retry branch yet.
fn save_path(
    held: &[Seen<'_>],
    uri: &str,
    broken_at_start: bool,
    proposed: &str,
    folder: Option<&Path>,
) -> Result<PathBuf, PathBuf> {
    let _ = (uri, broken_at_start);
    downloads::destination(folder, proposed, |p| {
        p.exists() || held.iter().any(|h| downloads::same_download_path(h.path, p))
    })
}

/// RED: lets go of the download that reported only (the latest held one with its path), as the host does today.
fn release<T>(held: &mut Vec<T>, path: &Path, path_of: impl Fn(&T) -> &Path) {
    if let Some(at) = held.iter().rposition(|h| downloads::same_download_path(path_of(h), path)) {
        held.remove(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen<'a>(path: &'a Path, uri: &'a str, broken: bool) -> Seen<'a> {
        Seen { path, uri, broken }
    }

    /// A fresh, empty folder of this test's own.
    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manifold-wv2-{name}-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_retry_takes_the_path_of_the_broken_download_it_retries() {
        let dir = temp_folder("retry");
        let slow = dir.join("slow.zip");
        let broken = [seen(&slow, "http://h/slow", true)];
        let proposed = r"C:\names\slow (1).zip";
        let folder = Some(dir.as_path());
        assert_eq!(save_path(&broken, "http://h/slow", true, proposed, folder), Ok(slow.clone()));
        assert_eq!(save_path(&broken, "http://h/slow", true, proposed, None), Ok(slow.clone()), "needs no folder");
        assert_eq!(
            save_path(&broken, "http://h/other", true, r"C:\names\other.zip", folder),
            Ok(dir.join("other.zip")),
            "another URL is not its retry"
        );
        let healthy = [seen(&slow, "http://h/slow", false)];
        assert_eq!(
            save_path(&healthy, "http://h/slow", true, proposed, folder),
            Ok(dir.join("slow (1).zip")),
            "only a broken download is retried"
        );
        assert_eq!(
            save_path(&broken, "http://h/slow", false, r"C:\names\slow.zip", folder),
            Ok(dir.join("slow (1).zip")),
            "a fresh download of the same URL is a new one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_latest_broken_download_of_the_url_is_the_one_retried() {
        let (a, b, c) = (Path::new(r"C:\d\a.zip"), Path::new(r"C:\d\b.zip"), Path::new(r"C:\d\c.zip"));
        let held = [seen(a, "u", true), seen(b, "u", true), seen(c, "u", false)];
        assert_eq!(retry_path(&held, "u"), Some(b));
        assert_eq!(retry_path(&held, "v"), None);
        assert_eq!(retry_path(&held[2..], "u"), None, "a healthy one is not retried");
        assert_eq!(retry_path(&[], "u"), None);
    }

    /// Review Focus 4: a second download of one name while the first is arriving is numbered; a retry is not.
    #[test]
    fn a_second_download_of_one_name_is_numbered_and_a_retry_is_not() {
        let dir = temp_folder("second");
        let folder = Some(dir.as_path());
        let a = dir.join("a.zip");
        let healthy = [seen(&a, "http://h/a", false)];
        assert_eq!(
            save_path(&healthy, "http://h/a", false, r"C:\names\a.zip", folder),
            Ok(dir.join("a (1).zip")),
            "the first is still arriving"
        );
        let upper = dir.join("A.ZIP");
        let other_case = [seen(&upper, "http://h/A", false)];
        assert_eq!(
            save_path(&other_case, "http://h/a", false, r"C:\names\a.zip", folder),
            Ok(dir.join("a (1).zip")),
            "Windows names ignore case"
        );
        std::fs::write(dir.join("b.zip"), b"old").unwrap();
        assert_eq!(
            save_path(&[], "http://h/b", false, r"C:\names\b.zip", folder),
            Ok(dir.join("b (1).zip")),
            "an existing file keeps its name"
        );
        assert_eq!(
            save_path(&[], "http://h/c", false, r"C:\names\..\c:x.zip", folder),
            Ok(dir.join("c_x.zip")),
            "only the leaf, made safe"
        );
        assert_eq!(
            save_path(&[], "http://h/a", false, r"C:\names\a.zip", None),
            Err(PathBuf::from("a.zip")),
            "no folder: refused"
        );
        let broken = [seen(&a, "http://h/a", true)];
        assert_eq!(
            save_path(&broken, "http://h/a", true, r"C:\names\a (1).zip", folder),
            Ok(a.clone()),
            "the retry keeps the name"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review Focus 4: a retry that ends takes the broken download it retried with it. That one never reports
    /// again; held, it would keep the name taken after the retry failed and its file went.
    #[test]
    fn a_retry_that_ends_frees_the_name_of_the_download_it_retried() {
        let dir = temp_folder("release");
        let folder = Some(dir.as_path());
        let (slow, other) = (dir.join("slow.zip"), dir.join("other.zip"));
        let mut held = vec![
            seen(&slow, "http://h/slow", true),
            seen(&other, "http://h/other", false),
            seen(&slow, "http://h/slow", false),
        ];
        release(&mut held, &slow, |h| h.path);
        assert_eq!(held.len(), 1, "the retry and the download it retried are both over");
        assert_eq!(held[0].path, other.as_path());
        assert_eq!(
            save_path(&held, "http://h/slow", false, r"C:\names\slow.zip", folder),
            Ok(slow.clone()),
            "the failed retry's file is gone, so the name is free again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -j 8 -p manifold-view-webview2 download:: 2>&1 | grep -E "^test |panicked|left|right" | head -n 20`
Expected: 3 failures and no warning (the tests use every new item; a warning is a stop-and-report):
- `a_retry_takes_the_path_of_the_broken_download_it_retries`: the left side ends `slow (1).zip`, the right `slow.zip`.
- `a_second_download_of_one_name_is_numbered_and_a_retry_is_not`: fails at "the retry keeps the name".
- `a_retry_that_ends_frees_the_name_of_the_download_it_retried`: fails at "the retry and the download it retried are both over" (left 2, right 1).
- `the_latest_broken_download_of_the_url_is_the_one_retried` passes.

- [ ] **Step 3: Add the retry branch and the release rule**

Replace the RED `save_path` and the RED `release` (their docs and bodies) with:

```rust
/// Where a download of `uri` that Edge proposed as `proposed` is saved: a retry (`broken_at_start`) of a broken held
/// download takes that download's path; anything else is named by `downloads::destination` in `folder`, numbered
/// against existing files and every held download. `Err` is a download to refuse.
fn save_path(
    held: &[Seen<'_>],
    uri: &str,
    broken_at_start: bool,
    proposed: &str,
    folder: Option<&Path>,
) -> Result<PathBuf, PathBuf> {
    match retry_path(held, uri) {
        Some(path) if broken_at_start => Ok(path.to_path_buf()),
        _ => downloads::destination(folder, proposed, |p| {
            p.exists() || held.iter().any(|h| downloads::same_download_path(h.path, p))
        }),
    }
}

/// Lets go of what a download that reported finished or failed at `path` leaves held: itself, and the broken
/// download its retry took the path of, which never reports again (see the module doc). Two held downloads share a
/// path only as a retry and what it retried, since `save_path` numbers every other one.
fn release<T>(held: &mut Vec<T>, path: &Path, path_of: impl Fn(&T) -> &Path) {
    held.retain(|h| !downloads::same_download_path(path_of(h), path));
}
```

- [ ] **Step 4: Run them to see them pass (GREEN)**

Run: `cargo test -j 8 -p manifold-view-webview2 download:: 2>&1 | grep -E "^test result|FAILED"`
Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 5: Wire the rule into the host (the whole file)**

Replace all of `crates/manifold-view-webview2/src/download.rs` with:

```rust
//! Download handling via WebView2's native `DownloadStarting`/`BytesReceivedChanged`/`StateChanged` events
//! (`ICoreWebView2_4`). This project never prompts for a save location. `SetHandled(true)` suppresses WebView2's own
//! download UI (a flyout a windowed Edge would show): this off-screen host has nowhere for it to appear, the same
//! caution `popup.rs`/`find.rs` take.
//!
//! The host names every download by the rule every host uses (`downloads::destination`: the leaf of Edge's proposed
//! `ResultFilePath`, made safe and numbered against existing files and this host's held downloads, in the folder the
//! app set) and sets it with `SetResultFilePath` before reporting it (run 2026-10-28-dldir-result: it works from this
//! handler, and Edge then writes nothing into its own folder). So `path` is final at `DownloadStarted` and is the key
//! of every later message about the download.
//!
//! Edge numbers its proposal against the profile's `DefaultDownloadFolderPath` (run 2026-10-28-dldir-profile-seed),
//! so `register` points that at an empty naming folder the host owns, where nothing is saved (`naming_folder`). The
//! profile is InPrivate and keeps no setting to the next run, so it is set at every start (2026-10-28-dldir-profile),
//! and only once the folder is made: Edge fires no `DownloadStarting` at all when it can't make that folder
//! (2026-10-28-dldir-profile-bad).
//!
//! A download the host can't place (no folder from the app, one that can't be made, a path Edge refuses) is
//! cancelled and reported started and failed (`downloads::refused`), so the app shows a failed card.
//!
//! A transfer that breaks is retried by the engine as a second `DownloadStarting`, whose operation starts with the
//! first one's `InterruptReason` rather than none (runs 2026-10-28-dldir-result2, -result-etag2). The retry takes the
//! broken download's path (`retry_path`): restarted from 0 it overwrites that file; resumed from the bytes the engine
//! kept (an HTTP/1.1 reply with an ETag and `Accept-Ranges`) it continues it (2026-10-28-dldir-result-etag), where
//! Edge's own naming proposed `slow (1).zip` (2026-10-28-dldir-profile-etag). One path keeps one card: the app restarts
//! the card of a download in flight under the same path (`fetched.rs` `insert_download_node`).
//!
//! The host holds each download's operation until it reports finished or failed. The retry reports its outcome only
//! while held: dropped, it never did (runs 2026-10-27-download-host-events, 2026-10-28-dl-failed-red); held, it does
//! (runs 2026-10-27-dl-probe-*). The first operation never reports again, so it is let go with its retry (`release`):
//! its name stays taken while the retry runs, and is free once a failed retry's file is gone.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use manifold_view_protocol::{downloads, ViewToHost};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2DownloadOperation, ICoreWebView2_13, ICoreWebView2_4,
    COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE, COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
    COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
};
use webview2_com::{
    take_pwstr, BytesReceivedChangedEventHandler, DownloadStartingEventHandler, StateChangedEventHandler,
};
use windows::core::{Interface, HSTRING, PWSTR};

/// A queue, not a single slot -- two downloads progressing within the same
/// tick would otherwise silently clobber each other (each gets its own
/// `BytesReceivedChangedEventHandler`/`StateChangedEventHandler` closures
/// below, all writing into whatever this type is).
pub type PendingDownload = Rc<RefCell<Vec<ViewToHost>>>;

/// A download the host reported and has not seen finish or fail (see the module doc).
struct Held {
    operation: ICoreWebView2DownloadOperation,
    /// Where it saves: its key in every message.
    path: PathBuf,
    /// What it fetches, to match a retry to it.
    uri: String,
}

/// What `save_path` reads of a held download.
struct Seen<'a> {
    path: &'a Path,
    uri: &'a str,
    /// Its transfer broke (`InterruptReason` not none) while it still reads in progress: the engine is retrying it.
    broken: bool,
}

/// The empty folder Edge proposes names in (see the module doc). Nothing is saved there.
fn naming_folder() -> PathBuf {
    std::env::temp_dir().join("manifold-webview2-names")
}

/// Points the profile's `DefaultDownloadFolderPath` at `naming_folder`, made first.
fn set_naming_folder(webview: &ICoreWebView2) -> Result<(), String> {
    let folder = naming_folder();
    std::fs::create_dir_all(&folder).map_err(|e| format!("{} not made: {e}", folder.display()))?;
    let set = || -> windows::core::Result<()> {
        let webview13: ICoreWebView2_13 = webview.cast()?;
        unsafe { webview13.Profile()?.SetDefaultDownloadFolderPath(&HSTRING::from(folder.as_path())) }
    };
    set().map_err(|e| e.to_string())
}

/// Whether `operation`'s transfer broke: a retry starts with the broken one's reason (see the module doc).
fn interrupted(operation: &ICoreWebView2DownloadOperation) -> bool {
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
    unsafe { operation.InterruptReason(&mut reason) }.is_ok() && reason != COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE
}

/// The path of the download a retry of `uri` retries: the latest held one of that URL whose transfer broke.
fn retry_path<'a>(held: &[Seen<'a>], uri: &str) -> Option<&'a Path> {
    held.iter().rev().find(|h| h.broken && h.uri == uri).map(|h| h.path)
}

/// Where a download of `uri` that Edge proposed as `proposed` is saved: a retry (`broken_at_start`) of a broken held
/// download takes that download's path; anything else is named by `downloads::destination` in `folder`, numbered
/// against existing files and every held download. `Err` is a download to refuse.
fn save_path(
    held: &[Seen<'_>],
    uri: &str,
    broken_at_start: bool,
    proposed: &str,
    folder: Option<&Path>,
) -> Result<PathBuf, PathBuf> {
    match retry_path(held, uri) {
        Some(path) if broken_at_start => Ok(path.to_path_buf()),
        _ => downloads::destination(folder, proposed, |p| {
            p.exists() || held.iter().any(|h| downloads::same_download_path(h.path, p))
        }),
    }
}

/// Lets go of what a download that reported finished or failed at `path` leaves held: itself, and the broken
/// download its retry took the path of, which never reports again (see the module doc). Two held downloads share a
/// path only as a retry and what it retried, since `save_path` numbers every other one.
fn release<T>(held: &mut Vec<T>, path: &Path, path_of: impl Fn(&T) -> &Path) {
    held.retain(|h| !downloads::same_download_path(path_of(h), path));
}

/// Best-effort like this project's other version-gated WebView2 APIs -- an
/// `ICoreWebView2_4`-less Runtime just means downloads on this tab proceed
/// with WebView2's own default UI/behavior instead of being silently
/// tracked, rather than failing to download at all.
pub fn register(webview: &ICoreWebView2) -> windows::core::Result<PendingDownload> {
    let webview4: ICoreWebView2_4 = webview.cast()?;
    if let Err(e) = set_naming_folder(webview) {
        eprintln!("download: naming folder not set ({e}); Edge numbers its proposals against its own folder");
    }

    let pending: PendingDownload = Rc::new(RefCell::new(Vec::new()));
    let mut token = Default::default();

    let pending_for_handler = pending.clone();
    // Every download that hasn't finished or failed yet, and each broken one whose retry hasn't (see the module doc).
    let held_for_handler: Rc<RefCell<Vec<Held>>> = Rc::default();
    let handler = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let _ = args.SetHandled(true);
        }
        let Ok(operation) = (unsafe { args.DownloadOperation() }) else { return Ok(()) };
        let mut proposed_raw = PWSTR::null();
        let proposed =
            if unsafe { args.ResultFilePath(&mut proposed_raw) }.is_ok() { take_pwstr(proposed_raw) } else { String::new() };
        let mut uri_raw = PWSTR::null();
        let uri = if unsafe { operation.Uri(&mut uri_raw) }.is_ok() { take_pwstr(uri_raw) } else { String::new() };
        let broken_at_start = interrupted(&operation);
        let decided = {
            let held = held_for_handler.borrow();
            let seen: Vec<Seen> = held
                .iter()
                .map(|h| Seen { path: h.path.as_path(), uri: h.uri.as_str(), broken: interrupted(&h.operation) })
                .collect();
            save_path(&seen, &uri, broken_at_start, &proposed, downloads::folder().as_deref())
        };
        let url = Some(uri.clone()).filter(|u| !u.is_empty());
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
        let path_text = path.to_string_lossy().into_owned();
        let mut total_bytes_raw: i64 = -1;
        let total_bytes = unsafe { operation.TotalBytesToReceive(&mut total_bytes_raw) }
            .is_ok()
            .then_some(total_bytes_raw)
            .filter(|n| *n >= 0)
            .map(|n| n as u64);
        pending_for_handler.borrow_mut().push(ViewToHost::DownloadStarted { path: path_text.clone(), total_bytes, url });
        // A retry that resumed starts with the bytes the engine kept.
        let mut bytes_received: i64 = 0;
        if unsafe { operation.BytesReceived(&mut bytes_received) }.is_ok() && bytes_received > 0 {
            pending_for_handler
                .borrow_mut()
                .push(ViewToHost::DownloadProgress { path: path_text.clone(), bytes_received: bytes_received as u64 });
        }
        held_for_handler.borrow_mut().push(Held { operation: operation.clone(), path, uri });

        let pending_for_bytes = pending_for_handler.clone();
        let path_for_bytes = path_text.clone();
        let mut bytes_token = Default::default();
        let bytes_handler = BytesReceivedChangedEventHandler::create(Box::new(move |sender, _args| {
            if let Some(sender) = &sender {
                let mut bytes_received: i64 = 0;
                if unsafe { sender.BytesReceived(&mut bytes_received) }.is_ok() && bytes_received >= 0 {
                    pending_for_bytes.borrow_mut().push(ViewToHost::DownloadProgress {
                        path: path_for_bytes.clone(),
                        bytes_received: bytes_received as u64,
                    });
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = operation.add_BytesReceivedChanged(&bytes_handler, &mut bytes_token);
        }

        let pending_for_state = pending_for_handler.clone();
        let held_for_state = held_for_handler.clone();
        let mut state_token = Default::default();
        let state_handler = StateChangedEventHandler::create(Box::new(move |sender, _args| {
            if let Some(sender) = &sender {
                let mut state = Default::default();
                if unsafe { sender.State(&mut state) }.is_ok() {
                    let message = if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                        Some(ViewToHost::DownloadCompleted { path: path_text.clone() })
                    } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                        Some(ViewToHost::DownloadFailed { path: path_text.clone() })
                    } else {
                        None
                    };
                    if let Some(message) = message {
                        pending_for_state.borrow_mut().push(message);
                        // Over: nothing more will come from it, nor from a broken download it retried.
                        release(&mut *held_for_state.borrow_mut(), Path::new(&path_text), |h| h.path.as_path());
                    }
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = operation.add_StateChanged(&state_handler, &mut state_token);
        }
        Ok(())
    }));
    unsafe {
        let _ = webview4.add_DownloadStarting(&handler, &mut token);
    }

    Ok(pending)
}
```

and then, at the end of the file, the `#[cfg(test)] mod tests { ... }` block from Step 1, verbatim (all four tests). A whole-file Write must include it.

- [ ] **Step 6: Build and test the host**

Run: `cargo test -j 8 -p manifold-view-webview2 2>&1 | grep -E "^test result|FAILED|^warning|^error" | head`
Expected: `test result: ok`, 0 failed, no warnings.

Run: `cargo clippy -j 8 -p manifold-view-webview2 --all-targets -- -D warnings 2>&1 | tail -n 3`
Expected: `Finished` with no warning.

- [ ] **Step 7: The resumable download server**

Replace all of `crates/gate/src/shots_net.rs` with:

```rust
//! The two servers `gate chrome-shots` needs that http.rs cannot be (they hold a connection open after answering):
//! the download server (was the inline python `serve.py`) and the fake proxy (was scripts/fake_proxy.py).

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The connections a server keeps open after answering, and whether `break_all` has run: a connection kept after
/// that is broken at once, so a break never misses a reply still being written.
#[derive(Default)]
struct Held(Mutex<(bool, Vec<TcpStream>)>);

impl Held {
    /// Keeps `stream` open until `break_all`, or breaks it now when that already ran.
    fn keep(&self, stream: TcpStream) {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if held.0 {
            let _ = stream.shutdown(Shutdown::Both);
        } else {
            held.1.push(stream);
        }
    }

    /// Breaks every kept connection, and every one kept from now on.
    fn break_all(&self) {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        held.0 = true;
        for stream in held.1.drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// A listener on 127.0.0.1 (or `SITE_HOST`) that hands each connection to `on_conn` on its own thread. A connection
/// the handler keeps (`Held::keep`) stays open until drop or `Held::break_all`.
struct Hold {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    held: Arc<Held>,
    thread: Option<JoinHandle<()>>,
}

impl Hold {
    /// Listens on `port`, or on any free port if that one is busy (0 asks for any) and says so.
    fn start(port: u16, on_conn: impl Fn(TcpStream, &Held) + Send + Sync + 'static) -> io::Result<Hold> {
        let host = crate::http::site_host();
        let listener = match TcpListener::bind((host.as_str(), port)) {
            Err(e) if port != 0 => {
                println!(
                    "port {port} is busy ({e}), so the shots that show this server's url won't match another run's"
                );
                TcpListener::bind((host.as_str(), 0))?
            }
            bound => bound?,
        };
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let held = Arc::new(Held::default());
        let on_conn = Arc::new(on_conn);
        let (stop2, held2) = (stop.clone(), held.clone());
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let (on_conn, held) = (on_conn.clone(), held2.clone());
                    thread::spawn(move || on_conn(stream, &held));
                }
            }
        });
        Ok(Hold { addr, stop, held, thread: Some(thread) })
    }
}

/// Stops listening (the accept thread exits and closes the listener) and breaks the held connections.
impl Drop for Hold {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        self.held.break_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Reads the request head (up to the blank line), as text.
fn request_head(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(n) if n > 0 => head.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// The path a request head asks for ("" when there is none).
fn request_path(head: &str) -> &str {
    head.lines().next().and_then(|line| line.split(' ').nth(1)).unwrap_or("")
}

/// The start of an open-ended `Range: bytes=N-`, the form an engine resuming a download sends.
fn range_start(head: &str) -> Option<usize> {
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("range") {
            return None;
        }
        value.trim().strip_prefix("bytes=")?.strip_suffix('-')?.parse().ok()
    })
}

/// The download server listens here: the download views show its url, so two runs' shots match only on the same port.
const DOWNLOADS_PORT: u16 = 47_311;

/// `/resume`'s size, and where its first transfer breaks.
const RESUME_LEN: usize = 1_000_000;
const RESUME_BREAK: usize = 300_000;

/// `/resume`'s bytes: not all one value, so a resume that continues from the wrong place makes a different file.
pub fn resume_body() -> Vec<u8> {
    (0..RESUME_LEN).map(|i| (i % 251) as u8).collect()
}

/// What the download server sends for one request: the bytes, whether to keep the connection open after them
/// (`Held::keep`), and whether it is the rest of `/resume` (a `206`).
struct Answer {
    bytes: Vec<u8>,
    hold: bool,
    partial: bool,
}

/// The reply to `path`. `range` is the start of an open-ended `Range: bytes=N-`; `first` marks the first plain
/// request for `/resume`, the one whose transfer breaks.
fn answer(path: &str, range: Option<usize>, first: bool) -> Answer {
    if path == "/resume" {
        let body = resume_body();
        let common = "Content-Type: application/zip\r\nContent-Disposition: attachment; filename=\"resume.zip\"\r\nETag: \"resume-1\"\r\nAccept-Ranges: bytes\r\nConnection: close\r\n";
        if let Some(start) = range.filter(|&start| start < RESUME_LEN) {
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\n{common}Content-Range: bytes {start}-{}/{RESUME_LEN}\r\nContent-Length: {}\r\n\r\n",
                RESUME_LEN - 1,
                RESUME_LEN - start
            );
            return Answer { bytes: [head.as_bytes(), &body[start..]].concat(), hold: false, partial: true };
        }
        let head = format!("HTTP/1.1 200 OK\r\n{common}Content-Length: {RESUME_LEN}\r\n\r\n");
        let sent = if first { &body[..RESUME_BREAK] } else { body.as_slice() };
        return Answer { bytes: [head.as_bytes(), sent].concat(), hold: first, partial: false };
    }
    let slow = path == "/slow";
    let (name, len) = if slow { ("slow", 1_000_000) } else { ("real", 4) };
    let head = format!(
        "HTTP/1.0 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"{name}.zip\"\r\nContent-Length: {len}\r\n\r\n"
    );
    let body: &[u8] = if slow { &[b'x'; 300_000] } else { b"data" };
    Answer { bytes: [head.as_bytes(), body].concat(), hold: slow, partial: false }
}

/// `/real.zip` answers a 4-byte attachment; `/slow` promises 1 MB, sends 300 KB and then holds the connection until
/// `kill`: the server dying mid-transfer. `/resume` is a file an engine can resume (HTTP/1.1, an `ETag`,
/// `Accept-Ranges`): the first request gets 300 KB of 1 MB and is held until `break_transfers`, and a `Range`
/// request gets the rest as a `206`, so a resumed download completes `resume_body`.
pub struct Downloads {
    hold: Hold,
    /// How many `Range` requests were answered with the rest (`ranges_answered`).
    ranges: Arc<AtomicUsize>,
}

impl Downloads {
    pub fn start() -> io::Result<Downloads> {
        let ranges = Arc::new(AtomicUsize::new(0));
        let (counted, plain) = (ranges.clone(), AtomicUsize::new(0));
        let hold = Hold::start(DOWNLOADS_PORT, move |mut stream, held| {
            let head = request_head(&mut stream);
            let (path, range) = (request_path(&head), range_start(&head));
            let first = path == "/resume" && range.is_none() && plain.fetch_add(1, Ordering::SeqCst) == 0;
            let reply = answer(path, range, first);
            // Counted before the reply goes out, so a check that saw the reply sees the count.
            if reply.partial {
                counted.fetch_add(1, Ordering::SeqCst);
            }
            if stream.write_all(&reply.bytes).and_then(|()| stream.flush()).is_err() {
                return;
            }
            if reply.hold {
                held.keep(stream);
            }
        })?;
        Ok(Downloads { hold, ranges })
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}:{}{path}", crate::http::site_host(), self.hold.addr.port())
    }

    /// Breaks the transfers in progress, and any that would be held from now on, while the server keeps answering:
    /// a dropped connection, not a dead server.
    pub fn break_transfers(&self) {
        self.hold.held.break_all();
    }

    /// How many `Range` requests for `/resume` were answered with the rest.
    pub fn ranges_answered(&self) -> usize {
        self.ranges.load(Ordering::SeqCst)
    }

    /// The server dies: transfers in progress break and new connections are refused (the engine retries a broken
    /// download at once, and a server that still answered would serve it again).
    pub fn kill(self) {}
}

/// An HTTP proxy for the engine installer's download: `refuse` answers each CONNECT 502 (the install fails at once);
/// otherwise it opens the tunnel and says nothing (the install sits in Installing with no bytes).
pub struct Proxy {
    hold: Hold,
    refuse: Arc<AtomicBool>,
}

impl Proxy {
    pub fn start(refuse: bool) -> io::Result<Proxy> {
        let refuse = Arc::new(AtomicBool::new(refuse));
        let mode = refuse.clone();
        let hold = Hold::start(0, move |mut stream, held| {
            let _ = request_head(&mut stream);
            let reply: &[u8] = if mode.load(Ordering::SeqCst) {
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n"
            } else {
                b"HTTP/1.1 200 Connection established\r\n\r\n"
            };
            if stream.write_all(reply).is_ok() && !mode.load(Ordering::SeqCst) {
                held.keep(stream);
            }
        })?;
        Ok(Proxy { hold, refuse })
    }

    /// The mode is read per connection, so it can change between installs.
    pub fn set_refuse(&self, refuse: bool) {
        self.refuse.store(refuse, Ordering::SeqCst);
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}", crate::http::site_host(), self.hold.addr.port())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url_port: u16, path: &str, headers: &str, read: usize) -> Vec<u8> {
        let mut s = TcpStream::connect((crate::http::site_host().as_str(), url_port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\n{headers}\r\n").unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while got.len() < read {
            match s.read(&mut buf) {
                Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
                _ => break,
            }
        }
        got
    }

    /// A reply's head, as text, and its body.
    fn split(bytes: &[u8]) -> (String, &[u8]) {
        let at = bytes.windows(4).position(|w| w == b"\r\n\r\n").expect("a whole head") + 4;
        (String::from_utf8_lossy(&bytes[..at]).into_owned(), &bytes[at..])
    }

    #[test]
    fn downloads_answer_real_in_full_and_slow_in_part_until_killed() {
        let server = Downloads::start().unwrap();
        let port = server.hold.addr.port();
        let real = String::from_utf8_lossy(&get(port, "/real.zip", "", usize::MAX)).into_owned();
        assert!(real.contains("filename=\"real.zip\"") && real.ends_with("data"), "{real}");
        let slow = get(port, "/slow", "", 300_000);
        assert!(slow.len() > 300_000 && slow.len() < 1_000_000, "{}", slow.len());
        server.kill();
    }

    /// `kill` is a dead server, not one that still answers the engine's retry of the broken download.
    #[test]
    fn a_killed_download_server_refuses_the_retry() {
        let server = Downloads::start().unwrap();
        let addr = server.hold.addr;
        server.kill();
        assert!(TcpStream::connect(addr).is_err(), "the killed server still accepts connections");
    }

    #[test]
    fn the_resume_answers_break_first_then_serve_ranges() {
        let body = resume_body();
        let first = answer("/resume", None, true);
        let (head, sent) = split(&first.bytes);
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
        assert!(head.contains("ETag: \"resume-1\"\r\n") && head.contains("Accept-Ranges: bytes\r\n"), "{head}");
        assert!(head.contains(&format!("Content-Length: {RESUME_LEN}\r\n")), "{head}");
        let want = &body[..RESUME_BREAK];
        assert!(sent == want && first.hold && !first.partial, "{} bytes", sent.len());
        let rest = answer("/resume", Some(RESUME_BREAK), false);
        let (head, sent) = split(&rest.bytes);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"), "{head}");
        let range = format!("Content-Range: bytes {RESUME_BREAK}-{}/{RESUME_LEN}\r\n", RESUME_LEN - 1);
        assert!(head.contains(&range), "{head}");
        let want = &body[RESUME_BREAK..];
        assert!(sent == want && !rest.hold && rest.partial, "{} bytes", sent.len());
        let again = answer("/resume", None, false);
        let (_, sent) = split(&again.bytes);
        let whole = body.as_slice();
        assert!(sent == whole && !again.hold, "{} bytes", sent.len());
        assert_eq!(range_start("GET /resume HTTP/1.1\r\nHost: x\r\nrange: bytes=300000-\r\n\r\n"), Some(300_000));
        assert_eq!(range_start("GET / HTTP/1.1\r\nRange: bytes=0-99\r\n\r\n"), None, "not open-ended");
        assert_eq!(range_start("GET /Range:bytes=1- HTTP/1.1\r\n\r\n"), None, "the request line is no header");
        assert_eq!(request_path("GET /resume HTTP/1.1\r\n\r\n"), "/resume");
        assert_eq!(request_path(""), "");
    }

    #[test]
    fn a_broken_resume_is_answered_from_where_it_stopped() {
        let server = Downloads::start().unwrap();
        let port = server.hold.addr.port();
        let body = resume_body();
        let first = get(port, "/resume", "", RESUME_BREAK + 1);
        assert!(first.starts_with(b"HTTP/1.1 200 OK\r\n"), "{}", String::from_utf8_lossy(&first[..first.len().min(64)]));
        server.break_transfers();
        let rest = get(port, "/resume", &format!("Range: bytes={RESUME_BREAK}-\r\n"), usize::MAX);
        let (head, sent) = split(&rest);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"), "{head}");
        let want = &body[RESUME_BREAK..];
        assert!(sent == want, "{} bytes", sent.len());
        assert_eq!(server.ranges_answered(), 1);
        let again = get(port, "/resume", "", usize::MAX);
        let (_, sent) = split(&again);
        let whole = body.as_slice();
        assert!(sent == whole, "{} bytes", sent.len());
        server.kill();
    }

    #[test]
    fn proxy_refuses_or_tunnels_by_mode() {
        let proxy = Proxy::start(true).unwrap();
        let port = proxy.hold.addr.port();
        assert!(String::from_utf8_lossy(&get(port, "x:443", "", usize::MAX)).starts_with("HTTP/1.1 502"));
        proxy.set_refuse(false);
        assert!(String::from_utf8_lossy(&get(port, "x:443", "", 10)).starts_with("HTTP/1.1 200"));
    }
}
```

Run: `cargo test -j 8 -p gate shots_net 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: `ok. 5 passed; 0 failed`. (Parallel tests may print "port 47311 is busy"; that is the existing fallback.) A lone failure of `a_killed_download_server_refuses_the_retry` ("still accepts connections") is the existing fixed-port race: another test's `Downloads::start` can bind 47311 between the kill and the connect. Rerun as `cargo test -j 8 -p gate shots_net -- --test-threads=1 2>&1 | grep -E "^test result|FAILED|panicked"`, and say in the report that it needed that.

- [ ] **Step 8: The download views check exact paths, and the resume shot**

In `crates/gate/src/shots.rs`:
- Line 59: `use crate::shots_net::{resume_body, Downloads, Proxy};`.
- `DEFAULT_VIEWS` (line 63): after `download-failed`, insert ` download-resume` (so it reads `... download-real download-progress download-failed download-resume json-zoom ...`).
- `struct Shots` (lines 194-206): after `work: PathBuf,` add:

```rust
    /// The instance's download folder (`instance::download_dir`), emptied before the download views.
    downloads: PathBuf,
```

- `Shots::start`: before `Ok(Shots {` (line 264), add `let downloads = crate::instance::download_dir(&profile)?;`, and add `downloads,` after `work: work.to_path_buf(),` in the literal.
- Line 660: `if self.has_any(&["download-real", "download-progress", "download-failed", "download-resume"]) {`.
- Replace `download_views` with its doc (lines 1326-1374) with:

```rust
    /// Real downloads into the instance's own folder (`instance::download_dir`), each checked by its exact path: a
    /// local HTTP server answers /real.zip as a small attachment, /slow as a 1 MB one it stalls after 300 KB
    /// (`download-failed` kills the server mid-transfer), and /resume as one an engine can resume after its transfer
    /// breaks; the live tab navigates to it, so the engine starts the download (the card keeps the URL as its source
    /// line). Before json-zoom, which must run last.
    fn download_views(&mut self) -> io::Result<()> {
        let folder = self.downloads.clone();
        // A rerun in the same work folder starts from an empty one, so every name below is exact.
        if folder.exists() {
            fs::remove_dir_all(&folder)?;
        }
        let at = |name: &str| folder.join(name).to_string_lossy().into_owned();
        if self.has_any(&["download-progress", "download-failed"]) {
            let server = Downloads::start()?;
            self.open_tab()?;
            let (url, slow) = (server.url("/slow"), at("slow.zip"));
            // Still arriving: 300 KB of 1 MB. The fit button frames the new card, which opens above the tab's.
            send!(self, format!("ADDRESSBAR {url}"), "SLEEP 1", "ENTER", "SLEEP 6")?;
            let named = |s: &State| matches!(s.downloads_from(&url)[..], [d] if d["path"] == slow.as_str());
            let state = self.inst.export_until("download-progress", named)?;
            if !named(&state) {
                return Err(io::Error::other(format!(
                    "the download of {url} is not one card saving to {slow}: {:?}",
                    state.downloads_from(&url)
                )));
            }
            send!(self, "TOGGLEVIEW", "SLEEP 2")?;
            self.inst.click_control(crate::state::FIT_VIEW)?;
            send!(self, "SLEEP 2", "MOUSEMOVE 5 790", "SLEEP 1")?;
            if self.has("download-progress") {
                self.shot("download-progress")?;
            }
            if self.has("download-failed") {
                // The server dies mid-transfer. WebView2 retries the download as a second one, which its host gives
                // the first one's path (`retry_path`) and which fails once the retry is refused (runs
                // 2026-10-27-dl-probe-*); the card shows it failed, and the shot waits for its toast to go.
                server.kill();
                let failed = |s: &State| {
                    matches!(s.downloads_from(&url)[..], [d] if d["failed"] == true && d["path"] == slow.as_str())
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
                send!(self, "MOUSEMOVE 5 780", "SLEEP 1")?;
                self.shot("download-failed")?;
            }
        }
        if self.has("download-real") {
            // A server of its own: `download-failed` killed the first.
            let server = Downloads::start()?;
            self.open_tab()?;
            let (url, real) = (server.url("/real.zip"), at("real.zip"));
            send!(self, format!("ADDRESSBAR {url}"), "SLEEP 1", "ENTER", "SLEEP 6")?;
            // Framed as before (S16): the tab six seconds after the download started.
            self.shot("download-real-tab")?;
            let saved = |s: &State| {
                matches!(s.downloads_from(&url)[..], [d] if d["completed"] == true && d["path"] == real.as_str())
                    && s.toast().is_none()
            };
            let state = self.inst.export_until("download-real", saved)?;
            if !saved(&state) {
                return Err(io::Error::other(format!(
                    "the download of {url} is not one completed card saving to {real}: {:?}",
                    state.downloads_from(&url)
                )));
            }
            let bytes = fs::read(folder.join("real.zip")).unwrap_or_default();
            if bytes != b"data" {
                return Err(io::Error::other(format!("{real} holds {} bytes, not the server's 4", bytes.len())));
            }
            // The card opens above the tab's, off the frame an earlier step's fit left.
            send!(self, "TOGGLEVIEW", "SLEEP 2")?;
            self.inst.click_control(crate::state::FIT_VIEW)?;
            send!(self, "SLEEP 2", "MOUSEMOVE 5 790", "SLEEP 1")?;
            self.shot("download-real")?;
        }
        if self.has("download-resume") {
            // A file the engine can resume: its transfer breaks, the engine asks for the rest with a `Range` request,
            // and the WebView2 host gives that retry the first one's path (`retry_path`), so one card completes one
            // whole file.
            let server = Downloads::start()?;
            self.open_tab()?;
            let (url, resume) = (server.url("/resume"), at("resume.zip"));
            send!(self, format!("ADDRESSBAR {url}"), "SLEEP 1", "ENTER")?;
            let arriving = |s: &State| s.downloads_from(&url).iter().any(|d| d["bytes"].as_u64() > Some(0));
            let state = self.inst.export_until("download-resume-arriving", arriving)?;
            if !arriving(&state) {
                return Err(io::Error::other(format!("no bytes of {url} arrived: {:?}", state.downloads_from(&url))));
            }
            // The connection drops while the engine still expects the rest; the server keeps answering.
            server.break_transfers();
            let settled = |s: &State| {
                let cards = s.downloads_from(&url);
                cards.len() > 1
                    || (cards.iter().any(|d| d["completed"] == true || d["failed"] == true) && s.toast().is_none())
            };
            let state = self.inst.export_until("download-resume", settled)?;
            let cards = state.downloads_from(&url);
            if !matches!(cards[..], [d] if d["completed"] == true && d["path"] == resume.as_str()) {
                return Err(io::Error::other(format!(
                    "the resumed download of {url} is not one completed card saving to {resume} ({} ranges answered): \
                     {cards:?}",
                    server.ranges_answered()
                )));
            }
            let saved = fs::read(folder.join("resume.zip")).unwrap_or_default();
            let sent = resume_body();
            if saved != sent {
                return Err(io::Error::other(format!(
                    "{resume} is not the file the server sent ({} of {} bytes)",
                    saved.len(),
                    sent.len()
                )));
            }
            if server.ranges_answered() == 0 {
                return Err(io::Error::other(format!(
                    "the engine never asked for the rest of {url} with an open-ended Range (`bytes=N-`, the only form \
                     shots_net answers with a 206)"
                )));
            }
            send!(self, "TOGGLEVIEW", "SLEEP 2")?;
            self.inst.click_control(crate::state::FIT_VIEW)?;
            send!(self, "SLEEP 2", "MOUSEMOVE 5 790", "SLEEP 1")?;
            self.shot("download-resume")?;
        }
        Ok(())
    }
```

Run: `cargo test -j 8 -p gate 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: ok, 0 failed.

- [ ] **Step 9: Commit the code, then prove the resume shot can fail (RED), on a throwaway edit**

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cargo fmt --all
git diff --stat
git add crates/manifold-view-webview2/src/download.rs crates/gate/src/shots_net.rs crates/gate/src/shots.rs
git commit -F - <<'EOF'
webview2: every download named by the shared rule, a retry keeps its path; resume shot

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
C=$(git rev-parse HEAD); echo "$C"
```
Expected: three files; the hook passes; the commit id printed (call it C).

Before the throwaway edit, check that nothing else holds the build or the app, and record the user's Downloads folder:
- In PowerShell, `Get-Process cargo, gate, manifold -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, Path`. Any `cargo` or `gate` is another session's: wait for it to exit. A `manifold` whose `Path` is this repo's `target\debug\manifold.exe` holds the exe the build replaces: ask the user to close it. Any other `manifold` is the user's own app: leave it.
- `grep -nE "T[0-9:]+ \|  \|  \|" docs/runs.md | cut -c1-100` lists the rows with an empty `end`. For each, find the run's `wrapper pid N` (its `notes` cell, or `"notes"` in its `run.json`) and run `tasklist //FI "PID eq N"`. A live wrapper is a run in progress: wait for it. A dead one is a lost run: close it as RUNS.md "Finding and closing" says. At 42a4c80b, `2026-10-18-layout-immediate-rerun-2` and `2026-10-20-smoke-diag2-x16` are such rows. If you closed any, commit them on their own, so the RED and GREEN rows commit alone:

  ```bash
  git diff --stat
  git add docs/runs.md
  git commit -F - <<'EOF'
  runs: close two lost rows

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  EOF
  ```
  Expected: one file (change "two" in the message if you closed a different number).
- `ls -A "$USERPROFILE/Downloads" > "<scratchpad>/downloads-before.txt"`, where `<scratchpad>` is your session's scratchpad directory (Step 10 compares against it).

Throwaway edit: in `crates/manifold-view-webview2/src/download.rs`'s `register`, change `let broken_at_start = interrupted(&operation);` to `let broken_at_start = false;`. (Do not commit this.)

Then:

```bash
cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate 2>&1 | tail -n 2
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name dl-resume-red -Detach -Command 'set "VIEWS=download-resume"& set "OUT=%RUN_DIR%\shots"& target\debug\gate.exe chrome-shots "%RUN_DIR%\work"'
```
Expected: `Finished` for the build with no warning (`save_path` still calls `retry_path`, and `interrupted` still reads the held downloads, so nothing goes unused; a warning is a stop-and-report); run.ps1 prints the run id (`<date>-dl-resume-red`).

Wait for it in a background Bash call (`run_in_background: true`), deadline 15 minutes:

```bash
RUN="$LOCALAPPDATA/claude-runs/manifold/<the printed id>"; deadline=$((SECONDS+900)); until grep -Eq '"end": +"' "$RUN/run.json" || [ $SECONDS -ge $deadline ]; do sleep 5; done; grep -E '"(exit|headline|commit)"' "$RUN/run.json"; tail -n 15 "$RUN/out.log"
```
Expected:
- `"exit"` is 1;
- `out.log` has `FAIL: downloads: the resumed download of http://...:47311/resume is not one completed card saving to ...\work\gate-chrome-shots-<pid>\appdata\manifold\downloads\resume.zip (N ranges answered)`, whatever the pid and N are (`gate`'s `work_dir` adds the `gate-chrome-shots-<pid>` level), with two cards listed (the second saving to `resume (1).zip`);
- `"commit"` ends `-dirty`.

If it passes instead, stop: the shot does not pin the rule, and the plan needs revising. If `"exit"` is 1 but the `FAIL:` line is another one (`no bytes of ... arrived`, `is not the file the server sent`, `never asked for the rest`, an error from the instance), or it lists anything but those two cards (one card stuck mid-transfer reaches the same message when the wait times out), stop and report it with the run id: that is RED for another reason (the server, the wait condition or the setup), not the retry rule being pinned. Keep its work folder for the diagnosis.

Once the `FAIL:` line is the expected one, remove the run's kept work folder (`probes::finish` deletes it only on a pass; it holds the instance profile, `resume.zip` and `resume (1).zip`): `RUN="$LOCALAPPDATA/claude-runs/manifold/<the printed id>"; rm -rf -- "$RUN"/work/gate-chrome-shots-*`.

Revert to C and prove the tree matches it:

```bash
C=<the commit id printed above>  # each Bash call starts a fresh shell
git restore --source="$C" -- crates/manifold-view-webview2/src/download.rs
git status --short -- crates
git diff "$C" --stat -- crates
```
Expected: both commands print nothing. (`docs/runs.md` is dirty from the run's row: that is expected and is committed in Step 10.)

- [ ] **Step 10: The download views GREEN, with the Settings shot**

```bash
cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate 2>&1 | tail -n 2
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind probe -Name dl-resume-green -Detach -Command 'set "KEEP=1"& set "VIEWS=download-progress download-failed download-real download-resume settings"& set "OUT=%RUN_DIR%\shots"& target\debug\gate.exe chrome-shots "%RUN_DIR%\work"'
```
(`KEEP=1` keeps the work folder, which `probes::finish` otherwise deletes on a pass, so the download folder can be listed below; as in the `download-real-log` row of `docs/runs.md`.)

Wait exactly as in Step 9 (deadline 900 s). If `"exit"` is 1, stop: read the `FAIL:` line in `out.log` and report it with the run id. Don't loosen the shot's checks or S18's folder emptying to make it pass. Expected:
- `"exit"` is 0, and `"commit"` is `git rev-parse --short HEAD` (C, or Step 9's lost-rows commit on it) with no `-dirty`. The wrapper ignores `docs/runs.md` but adds `-dirty` for any other modified or untracked file (`run.ps1`: `git status --porcelain -- . ':!docs/runs.md'`), this plan file included if its checkboxes were ticked. If it ends `-dirty`, run `git status --short` and name each such file in the report; any change under `crates/` means the run did not test C: stop and report it.
- `$RUN/shots/` holds `download-progress.png`, `download-failed.png`, `download-real-tab.png`, `download-real.png`, `download-resume.png` and `settings.png`.
- The instance's download folder holds `real.zip` and `resume.zip` and nothing numbered (no `(1)` in any name). `gate`'s `work_dir` puts the instance under `gate-chrome-shots-<pid>`, so list it with a glob, setting `RUN` again since each Bash call starts a fresh shell: `RUN="$LOCALAPPDATA/claude-runs/manifold/<the printed id>"; ls "$RUN"/work/gate-chrome-shots-*/appdata/manifold/downloads`. `slow.zip` may or may not be there: the engine can delete a failed download's partial file, and that is not part of this check. Then remove the kept work folder, as the gate does itself on a pass without `KEEP` (it holds the instance profile and the downloaded files; the shots are in `$RUN/shots`): `RUN="$LOCALAPPDATA/claude-runs/manifold/<the printed id>"; rm -rf -- "$RUN"/work/gate-chrome-shots-*`.
- The user's real Downloads folder gained nothing from either run: `ls -A "$USERPROFILE/Downloads" | diff "<scratchpad>/downloads-before.txt" -` prints nothing. Any difference is a stop-and-report: name each new file and whether it is a `slow`, `real` or `resume` file.

Open `$RUN/shots/settings.png` with the Read tool:
- the Download folder row shows under General, with the seeded path in the field;
- the field shows only the path's start (the run folder), clipped at 220 px, so `work\a` and `work\b` would look the same to `shots-aa`;
- under the field, the note "Doesn't exist yet: made at the first download": the settings views run before the download views (`shots.rs` calls `settings_views` first), and the seed does not make the folder.

If the field shows the end of the path, or the row or its note is missing, stop and report it; don't fix it silently.

Then set each row's headline. The wrapper writes the last `out.log` line; edit the row in `docs/runs.md` if that line isn't the verdict. Commit the rows:

```bash
git diff --stat
git add docs/runs.md
git commit -F - <<'EOF'
runs: download-resume RED (retry branch off) and GREEN rows

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: one file.

### Task 7: Docs and leftovers

**Files:**
- Modify: `crates/manifold-view-protocol/src/messages.rs:513-527` (`DownloadStarted`'s doc)
- Modify: `crates/manifold/src/fetched.rs:140-146`
- Modify: `README.md:129`, `:214`, `:295`, `:298`, `:329`, `:331`, `:341`
- Modify: `PARKED.md:29`, `:128`, `:129`, new entries in "Downloads (2026-10-28)"
- Modify: `docs/superpowers/mockups/README.md:56` and a new paragraph after it

**Interfaces:**
- Consumes: everything above (names cited in docs).
- Produces: nothing in code.

- [ ] **Step 1: The protocol contract and the card rule**

In `crates/manifold-view-protocol/src/messages.rs`, replace `DownloadStarted`'s whole doc comment (lines 513-527, "A download started -- `path` is where the engine will save it ... (shown as the card's source line)."; the `DownloadStarted { ... }` line 528 stays) with:

```rust
    /// A download started -- `path` is where the host saves it (this project
    /// never prompts for a save location). Every host names the file by one
    /// rule, `crate::downloads::destination`: the suggested name's last
    /// component, made safe and numbered `name (n).ext` if taken, in the
    /// folder `HostSettings::download_dir` names, made if missing. The WebView2
    /// host sets it with `SetResultFilePath` (a retry of a broken transfer
    /// keeps the broken download's path); CEF and WebKit hand it to their
    /// engines. A download the host can't place is reported started and then
    /// failed (`crate::downloads::refused`). `total_bytes` is the
    /// expected final size if the server reported a real `Content-Length`;
    /// `None` for a chunked/unknown-length response, shown as a byte count
    /// with no percentage rather than a guessed bar. `path` is the stable
    /// key `DownloadProgress`/`DownloadCompleted` for this same download are
    /// reported under, final at this point (the host names the file before it
    /// reports it). A WebKit host under WSL reports a Linux path, which the app
    /// maps to a Windows one as it arrives (`process_manager::in_app_terms`).
    /// WebView2, CEF, and WebKit all have a
    /// real native download-progress API; Servo's embedding surface has no
    /// download hook of any kind, so it never sends any `Download*` message.
    /// `url` is what the engine is fetching, when it can say (shown as the card's source line).
```

In `crates/manifold/src/fetched.rs`, replace lines 140-146 (`/// A download \`tab_id\` already has in flight ... second card").`) with:

```rust
    /// A download `tab_id` already has in flight for `path` restarts
    /// instead: the engine starts a download whose transfer broke again
    /// under the same path (two downloads in flight never share one, the
    /// host numbers the second's file), from 0 or from the bytes it kept,
    /// which a `DownloadProgress` right after says. The WebView2 host keeps
    /// the path either way (`retry_path` in its `download.rs`).
```

- [ ] **Step 2: README**

Each edit replaces one sentence or clause exactly (find it with `grep -n`):

- Line 129: replace "and `sync_theme` sends `HostToView::SetTheme` (protocol 5) to every engine host and reloads open `manifold://` tabs when the theme changes; hosts also read `VIEW_DARK` at spawn." with "and `sync_host_settings` sends `HostToView::SetSettings` (protocol 7, which replaced protocol 5's `SetTheme`; the theme and the download folder) to every host whose settings differ, and reloads open `manifold://` tabs when the theme changes; hosts read the same settings from `VIEW_SETTINGS` at spawn."
- Line 214: replace "Copy Path and Open Containing Folder share a new `windows_path_for_download` helper (factored out of what `reveal_download` already did) so a WebKit-sourced WSL2 path gets the same `\\wsl.localhost\...` translation either way." with "Copy Path and Open Containing Folder read the same stored path, which is already in Windows terms: a WebKit host's WSL paths are mapped as they arrive (`process_manager::in_app_terms`, since 2026-10-28)."
- Line 295: replace "final by contract at `Started` time, since every engine has already decided the save location before that fires" with "final at `Started` time, since the host names the file before it reports the download (`downloads::destination`, 2026-10-28)".
- Line 298: replace "`path` is read once, from `DownloadStartingEventArgs::ResultFilePath` at the moment the download starts, and reused as" with "`path` is set by the host when the download starts (`SetResultFilePath`, since 2026-10-28; Edge's proposed `ResultFilePath` supplies only the name), and reused as".
- Line 329: replace from ", and each supplies its own default save location the same way" (the comma before it included) through "the same "pick the obvious default, don't build a save-as dialog" posture this project already takes elsewhere." with the text below, which starts with a space (one space, then the parenthesis):

  " (WebView2 `ICoreWebView2_4::add_DownloadStarting`/`DownloadOperation::add_StateChanged`; CEF `DownloadHandler::on_before_download`/`BeforeDownloadCallback::cont`; WebKitGTK `WebContext::connect_download_started`/`Download::connect_decide_destination`). Since 2026-10-28 every host names the file by one rule (`manifold_view_protocol::downloads::destination`: the suggested name's last component, made safe, numbered `name (n).ext` if taken) in one folder: Settings "Download folder" (`download_directory`; empty, or text that is not a full path, means the Downloads Known Folder, `profile::downloads_dir()`), made at the first download if missing. Hosts get it at spawn (`VIEW_SETTINGS`) and on every change (`HostToView::SetSettings`); a WebKit host under WSL gets it as `/mnt/<drive>/...`. WebView2 proposes a path of its own, so its host sets each one with `SetResultFilePath` (Edge's proposal supplies only the name, proposed in an empty naming folder the host owns), and the retry of a broken transfer keeps the broken download's path, so a resumed download continues the same file on the same card; CEF and WebKit take the host's path as their only one. With no folder, or one that can't be made, the download shows a failed card rather than saving anywhere else — the same "pick the obvious default, don't build a save-as dialog" posture this project already takes elsewhere."

  (The sentence's start, "All three of WebView2, CEF, and WebKit have a real native download API", stays; with its comma gone, the leading space and parenthesis follow it: "... download API (WebView2 ...; WebKitGTK ...). Since 2026-10-28 ...". Check with `grep -c "download API (WebView2" README.md`, which prints 1.)
- Line 331: replace "— a WebKit-sourced path is translated from its WSL2-side form to the `\\wsl.localhost\Ubuntu\...` UNC path Explorer can actually browse first." with "— a WebKit download's path already arrives in Windows terms (`/mnt/<drive>/...` as `X:\...`, any other Linux path as `\\wsl.localhost\Ubuntu\...`; `process_manager::in_app_terms`)."
- Line 341: replace "(`profile::pictures_dir()`; the temp folder without a home folder)" with "(`profile::pictures_dir()`, the Pictures Known Folder; the temp folder without one)".

- [ ] **Step 3: PARKED.md**

- Line 29 (Soft theme leftovers):
  - replace "(dark at spawn through `VIEW_DARK`, then Light" with "(dark at spawn through `VIEW_DARK`, now `VIEW_SETTINGS`, then Light";
  - replace "`sync_theme`'s sends are covered by `theme_updates`' test (the wiring around it is not)" with "`sync_host_settings`' sends are covered by its two tests in `event_loop/frame.rs` (the wiring around it is not)";
  - replace "(5) `sync_theme` runs at the start of a frame" with "(5) `sync_host_settings` runs at the start of a frame".
- Line 128:
  - replace `crates/manifold-view-webview2/src/download.rs:58` with the line that `grep -n "held_for_handler.borrow_mut().push" crates/manifold-view-webview2/src/download.rs` prints;
  - replace "The first operation of a broken transfer is also held until the host exits (one per break)." with "The first operation of a broken transfer is held until its retry reports (`release`), so a retry that stalls keeps both."
- Delete line 129 ("**A resumed WebView2 retry gets a second card.** ...").
- Add after line 128, in "Downloads (2026-10-28)":

```markdown
- **Two hosts can name the same file.** *What:* each host reserves only the names of its own downloads in flight (`downloads::destination`'s `taken`: the file exists, or one of this host's downloads has the path), and every tab is its own host process, WebView2 included (`process_manager::spawn_tab` starts one per tab). Two tabs that start a download of one name into one folder at the same moment can both pick it; the second engine then overwrites (WebView2's `SetResultFilePath` overwrites an existing file) or fails. *Why parked:* unverified; it needs two downloads started within the moment before either file exists. *Trigger:* two cards with one path, or a corrupt file after two same-name downloads. *Fix:* the app reserves names for every host, or each host creates the file exclusively (`create_new`) before handing the path over. *Evidence:* `crates/manifold-view-protocol/src/downloads.rs` (`destination`), `crates/manifold/src/process_manager.rs` (`spawn_tab`), verified 2026-10-28. *Status:* parked.
- **Two broken downloads of one URL in one tab.** *What:* a WebView2 retry takes the path of the latest broken download of the same URL (`retry_path`, `crates/manifold-view-webview2/src/download.rs`); when two downloads of one URL break at once, both retries take the later one's path. *Why parked:* by decision (plan 2026-10-28 S6); it needs two concurrent broken transfers of one URL in one tab. *Trigger:* two cards of one URL where one shows the other's bytes. *Fix:* match a retry to the broken download whose `BytesReceived` equals the retry's at start. *Evidence:* `crates/manifold-view-webview2/src/download.rs` (`retry_path`, `register`), verified 2026-10-28. *Status:* parked.
- **CEF and WebKit download paths unexercised at runtime.** *What:* the gate's download views run on WebView2 only (`crates/gate/src/shots.rs` seeds `webview2`), so two paths have unit tests and builds but have never run. One is CEF's refusal: `on_before_download` returns 0, which CEF 152's header says cancels in Alloy style, the style a windowless browser always uses. The other is WebKit's download into a `/mnt/<drive>/...` folder under WSL. *Why parked:* by decision (plan 2026-10-28 S19); a CEF check needs new harness code, and no gate stage runs WebKit. *Trigger:* the first CEF or WebKit download into an unusable folder, a CEF upgrade, or a `download` probe stage under `VIEWS="webview2 cef"`. *Fix:* that probe stage (a good folder and a refused one, checking the cards). *Evidence:* `crates/manifold-view-cef/src/download_handler.rs` (`on_before_download`), `crates/manifold-view-webkitgtk/src/main.rs` (`connect_decide_destination`), verified 2026-10-28. *Status:* parked.
- **A download folder on an offline share freezes the tab's host.** *What:* each host names a download on its own UI thread (`downloads::destination`: `create_dir_all` and the checks for a free name), so with the Download folder on a share that is offline (a NAS, a disconnected mapped drive) the tab's host freezes until Windows gives up, then refuses the download. Every tab is its own host process, so the app and the other tabs keep running. *Why parked:* by decision (plan 2026-10-28 S20); new code in three hosts. *Trigger:* a host freezing with its download folder on a share. *Fix:* each host names the download on a worker thread and finishes the engine's decision later (WebView2 `DownloadStartingEventArgs::GetDeferral`; CEF's `on_before_download` may run its callback later; WebKit unchecked). *Evidence:* `crates/manifold-view-protocol/src/downloads.rs` (`destination`), verified 2026-10-28. *Status:* parked.
- **The Settings page reads the disk on the UI thread.** *What:* the Settings page reads the disk through `settings_page::cached`, on the app's UI thread, at most every `DISK_CACHE_AGE` (3 s): `view_runtime`, `view_leftovers`, `cef_newer_than_tested`, `named_sessions`, `view_dir_size`, and the Download folder row's `download_folder_kind`. `App::cef_dir_verdict` reads the CEF folder on the UI thread with its own cache, at most every `CEF_DIR_CHECK_TTL` (5 s), for the page and at tab spawn (`custom_cef_dir`). A read of a folder on a share that is offline (a NAS, a disconnected mapped drive) can block for tens of seconds, and the whole window with it. The Known Folder lookup is not one of them: it skips its existence check (`KF_FLAG_DONT_VERIFY`). *Why parked:* by decision (plan 2026-10-28 S20): one change to `cached` for all its users, rather than a second cache beside it for the new row alone. *Trigger:* the app freezing with a download or CEF folder on a share. *Fix:* `cached` itself reads on a worker thread (at most one read per key at a time, the last value kept while the next is read, each caller saying what it shows until the first lands), and `cef_dir_verdict` moves onto it. *Evidence:* `crates/manifold/src/settings_page.rs` (`cached`), `crates/manifold/src/view_install.rs` (`cef_dir_verdict`), verified 2026-10-28. *Status:* parked.
- **The default Downloads folder is read once per run.** *What:* `profile::downloads_dir()` keeps the Known Folder's path for the whole run (a `OnceLock`), so after the user moves Downloads (the folder's Properties, Location), downloads with an empty Download folder setting keep going to the old folder until the app restarts. `pictures_dir()` reads its folder on every call. *Why parked:* by decision (plan 2026-10-28, approval item 21): `App::host_settings` runs every frame (S9), so an uncached lookup would call `SHGetKnownFolderPath` every frame. *Trigger:* a download landing in the old Downloads folder after a move. *Fix:* read it again at most every few seconds, as `settings_page::cached` does, or when the Settings page opens. *Evidence:* `crates/manifold/src/profile.rs` (`downloads_dir`), verified 2026-10-28. *Status:* parked.
- **WSL drives are assumed at `/mnt`.** *What:* `HostRuntime::host_path` gives a WebKit host a drive folder as `/mnt/<drive>/...`, and `app_path` maps `/mnt/<drive>/...` back, ignoring a distro's `[automount] root` in `/etc/wsl.conf`. Under another root the host cannot make `/mnt/<drive>/...` (`/mnt` is root's), so every WebKit download is refused. The two are also not exact inverses: `host_path` drops a `\\?\` prefix, and `app_path` maps any `/mnt/<letter>` path. *Why parked:* by decision (plan 2026-10-28, approval item 21): `/mnt` is WSL's default, and reading the real root needs a WSL call at spawn. *Trigger:* WebKit downloads refused under a distro with a custom automount root. *Fix:* ask the distro once (`wslpath -u 'C:\'`) and use its answer both ways. *Evidence:* `crates/manifold/src/process_manager/runtime.rs` (`host_path`, `app_path`), verified 2026-10-28. *Status:* parked.
- **A download refused for want of a folder shows a bare file name.** *What:* with no download folder (`HostSettings::download_dir` is `None`: a host started by hand, or a WebKit host whose folder WSL can't see, such as a UNC share), `downloads::destination` returns the safe file name alone, and `refused` reports it as the failed card's path, so the card names a file with no folder and Reveal finds nothing. *Why parked:* by decision (plan 2026-10-28, approval item 21): what the card says for "no folder" is a design question (a reason line, or the folder the app meant). *Trigger:* a failed card whose path is a bare file name. *Fix:* `DownloadFailed` carries a reason the card shows, or the app shows the folder it set. *Evidence:* `crates/manifold-view-protocol/src/downloads.rs` (`destination`, `refused`), verified 2026-10-28. *Status:* parked.
- **A `manifold://` tab spawning during a theme change can keep the old palette.** *What:* `sync_host_settings` reloads `manifold://` tabs only on the frame the app's theme changes (`last_theme`). A tab whose host was still spawning then gets the new theme through `SetSettings` once it lands, but its page is not reloaded, so a page served before the message keeps the old palette until it is reloaded. *Why parked:* by decision (plan 2026-10-28, approval item 21; review round 2): the fix keys the reload on each tab's own last-sent theme and deletes `last_theme`, `theme_changed` and the first-frame case. *Trigger:* a `manifold://` page in the wrong palette after a theme change. *Fix:* reload a `manifold://` tab whenever `ViewTab::set_settings` sends it a different `dark`. *Evidence:* `crates/manifold/src/event_loop/frame.rs` (`sync_host_settings`), verified 2026-10-28. *Status:* parked.
- **A settings message refused by a full send queue counts as sent.** *What:* `ViewTab::set_settings` records the settings as the tab's before sending them, and `SendQueue::push` refuses a message once `SEND_QUEUE_CAP` (256) are waiting, so a host that falls that far behind keeps its old folder and theme until the next change. *Why parked:* by decision (plan 2026-10-28, approval item 21; review round 2): the fix changes `Writer::send` and `ViewTab::send` to say whether a message was accepted. *Trigger:* a tab saving to an old folder after it was unresponsive. *Fix:* those two report acceptance, and `set_settings` records only an accepted send. *Evidence:* `crates/manifold/src/process_manager.rs` (`set_settings`), `crates/manifold/src/process_manager/view_io.rs` (`push`), verified 2026-10-28. *Status:* parked.
```

Then, in each entry added above, write every *Evidence* item as `file:line`, as line 128's `download.rs:58` is (VERSIONING.md "Roadmap and parked work"): for `file` (`symbol`), the line `grep -nw "fn symbol" file` prints (`-w`, so `fn cached` skips `fn cached_dir_size`), or `grep -nw "symbol" file`'s first hit where no `fn` matches (`connect_decide_destination`).

- [ ] **Step 4: The mockups record**

In `docs/superpowers/mockups/README.md`, line 56: replace "WebView2 retries the download as a second one under the same path, which fails once the retry is refused." with "WebView2 retries the download as a second one, which its host gives the first one's path (`retry_path`, 2026-10-28) and which fails once the retry is refused." Add a new paragraph after line 56:

```markdown
**2026-10-28, resumed download.** Shot: `download-resume`. The test server's `/resume` answers HTTP/1.1 with an `ETag` and `Accept-Ranges`, breaks the first transfer partway (`Downloads::break_transfers`), and answers the engine's `Range` retry with `206` and the rest. The shot requires one completed card saving to `resume.zip` in the instance's own download folder, the file byte-for-byte the server's, and at least one `Range` request answered. With the WebView2 retry branch disabled it failed with a second card on `resume (1).zip` (run <date>-dl-resume-red); with it, it passed (run <date>-dl-resume-green).
```

(Fill both `<date>` parts with the run ids Task 6 printed, e.g. `2026-10-28-dl-resume-red`.)

- [ ] **Step 5: The stale-name sweep**

Run:

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
git grep -nE 'SetTheme|VIEW_DARK|ENV_DARK|downloads_dir|sync_theme|windows_path_to_wsl|windows_path_for_download|theme_updates|Downloads-folder default|~/Downloads|resumed WebView2 retry|DownloadStartingEventArgs::ResultFilePath|held until the host exits' -- ':!docs/superpowers/plans' ':!docs/superpowers/specs' ':!.superpowers' ':!CHANGELOG.md' ':!docs/runs.md' | cut -c1-160
```

Expected: every hit is one of these, and there are no others:
- `crates/manifold-view-protocol/src/lib.rs`: the version history lines `5: HostToView::SetTheme` and `7: ... replaces SetTheme`;
- `README.md:129`: "protocol 5's `SetTheme`";
- `PARKED.md`: "`VIEW_DARK`, now `VIEW_SETTINGS`";
- `profile::downloads_dir` in `crates/manifold/src/profile.rs`, `settings.rs`, `settings_page.rs` and `README.md:329`;
- `~/Downloads` in `profile.rs` (the non-Windows doc) and in `crates/manifold-view-webkitgtk/src/main.rs` (the refusal comments).

Any other hit is stale: fix it in this step. If it is history (a dated record), leave it and name it in the report.

- [ ] **Step 6: Commit**

```bash
git diff --stat
git add crates/manifold-view-protocol/src/messages.rs crates/manifold/src/fetched.rs README.md PARKED.md docs/superpowers/mockups/README.md
git commit -F - <<'EOF'
docs: the download folder, one naming rule, protocol 7, WSL paths

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: those five files (plus any stale-hit fixes from Step 5); the hook passes (it runs fmt and clippy because two `.rs` files are staged).

### Task 8: Final verification

**Files:** none changed, except `docs/runs.md` (the gate row), this plan's front matter (`status: done`, once the gate passes), and `PARKED.md` only if a check below fails and is parked.

**Interfaces:**
- Consumes: everything above.
- Produces: the verification record.

- [ ] **Step 1: Format and lints**

Run: `cargo fmt --all --check`
Expected: no output, exit 0.

Run: `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings 2>&1 | tail -n 3`
Expected: `Finished`, no warnings.

- [ ] **Step 2: Tests of every touched crate built here**

Run: `cargo test -j 8 -p manifold-view-protocol -p manifold-pages -p manifold -p manifold-view-webview2 -p gate 2>&1 | grep -E "^test result|FAILED|panicked"`
Expected: every `test result:` is `ok`, 0 failed.

- [ ] **Step 3: The Linux side**

These run inside WSL, with a separate `target-linux`. A cold check can take 10 minutes or more; run `df -h /c` first and skip both if less than 5 GB is free.

Run: `wsl.exe -d Ubuntu --cd "C:\\Users\\Winston Li\\Documents\\GitHub\\manifold" -- bash -lc 'CARGO_TARGET_DIR=target-linux cargo check -j 6 -p manifold' 2>&1 | tail -n 3`
Expected: `Finished`. This covers `profile.rs`'s `cfg(not(windows))` branch and the WSL mapping code on Linux.

Run: `wsl.exe -d Ubuntu --cd "C:\\Users\\Winston Li\\Documents\\GitHub\\manifold" -- bash -lc 'CARGO_TARGET_DIR=target-linux cargo build -j 6 -p manifold-view-webkitgtk' 2>&1 | tail -n 3`
Expected: `Finished`.

If either fails for an environment reason (no distro, no toolchain, missing system libraries), report "not checked: <first error line>" and don't claim it.

- [ ] **Step 4: Which gate tier**

Run: `cargo build -j 8 -p gate 2>&1 | tail -n 1 && target/debug/gate.exe tier 42a4c80b | tail -n 2`
Expected: the last line is `TIER full`, because the protocol, process manager and engine hosts changed. Run whatever tier it prints.

- [ ] **Step 5: The gate**

Before launching:
- the `Get-Process cargo, gate, manifold` check of Task 6 Step 9, read the same way (wait for any `cargo` or `gate`; ask the user to close a `manifold` running from this repo's `target\debug`; leave any other `manifold`);
- `docs/runs.md` has no live row without an `end`: the check in Task 6 Step 9 (the `grep -nE "T[0-9:]+ \|  \|  \|"` rows and their wrapper pids);
- `df -h /c` shows at least 5 GB free;
- `ls -A "$USERPROFILE/Downloads" > "<scratchpad>/downloads-before-gate.txt"`: the full gate runs the download views, and several stages launch instances with no `seed_settings` (e.g. `arrange.rs`, `labels.rs`, `focus.rs`, `stress.rs`; none of these runs a page download today), which would save into the default folder.

Then, from bash:

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
powershell -NoProfile -File "$USERPROFILE/.claude/runs/run.ps1" -Kind gate -Name gate-download-folder -Detach -Command 'cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate && target\debug\gate.exe all "%RUN_DIR%\gate-log.txt" full'
```

A full gate takes about 23-40 minutes. Wait in a background Bash call with a deadline of 4500 s:

```bash
RUN="$LOCALAPPDATA/claude-runs/manifold/<the printed id>"; deadline=$((SECONDS+4500)); until grep -Eq '"end": +"' "$RUN/run.json" || [ $SECONDS -ge $deadline ]; do sleep 30; done; grep -E '"(exit|cpu|defender|commit)' "$RUN/run.json"; tail -n 25 "$RUN/gate-log.txt"
```

Expected: `"exit"` is 0; the gate log's last lines report every stage passed.
- The user's real Downloads folder gained nothing: `ls -A "$USERPROFILE/Downloads" | diff "<scratchpad>/downloads-before-gate.txt" -` prints nothing. Any difference is a stop-and-report, as in Task 6 Step 10: name each new file (one the user saved during the run is theirs; ask).
- If `shots-aa` reports differences, list each view.
  - A difference in `settings` is not the download path (Task 6 Step 10 showed the field clips it): treat it as a regression or a known flake, name which, and don't rerun silently.
  - Differences in other views by colour alone are the known flakes.
  - Name which is which; don't rerun silently.
- Report the CPU load and Defender state that `run.json` recorded.
- Set the row's headline in `docs/runs.md` from the gate log's last lines.
- If the gate passed, this is the last task landing (VERSIONING.md "Specs and plans"; its leftovers went to `PARKED.md` in Task 7): with the Edit tool, change this plan's front matter `status: active` to `status: done`. If it failed, leave it `active` and report the failure instead. Then commit:

```bash
git diff --stat
git add docs/runs.md docs/superpowers/plans/2026-10-28-download-folder-setting.md
git commit -F - <<'EOF'
runs: gate-download-folder row; the download folder plan is done

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```
Expected: the stat lists `docs/runs.md`, and the plan file when the gate passed (drop "; the download folder plan is done" from the message if it failed).

- [ ] **Step 6: Report what was and wasn't checked, and clean up**

Run: `head -n 3 docs/superpowers/plans/2026-10-28-download-folder-setting.md | grep -c "^status: done$"`
Expected: `1` when the gate passed (`0`, with the plan still `active`, when it failed).

Collect into the final report:
- the CEF check, the WebKit build and the Servo build from Tasks 3-4, and the Linux check from Step 3, each as "passed", or "not checked" with its reason;
- "CEF's refusal (`on_before_download` returning 0) and WebKit's `/mnt/<drive>/...` destination: unit tests and builds only, not exercised at runtime (S19; PARKED "CEF and WebKit download paths unexercised at runtime")";
- the RED and GREEN run ids from Task 6;
- the gate run id, and whether Step 5's diff found the real Downloads folder unchanged;
- the CEF and Servo hosts in `target/debug` (Servo's exe, and CEF's in `target/debug/cef-runtime/`) still speak protocol 6 until they are rebuilt and placed as README "Engine setup for the gate" says (lines 492-493): name them stale unless this plan built and placed them;
- the close-out WORKING_STYLE.md asks for:
  - a status line: commits local only (nothing pushed or tagged), anything uncommitted or unverified, processes still running;
  - the docs check: for each doc Task 7 touched (README, PARKED, the mockups README, `messages.rs`'s download contract, `downloads.rs`'s and the WebView2 `download.rs`'s module docs, `fetched.rs`), whether Step 5's sweep found it fixed or stale;
  - the architecture check: what grew a special case here (the WebView2 retry branch in `save_path`, the WSL arm of `host_path`/`app_path`), and what would remove it; and that the Settings page still has one way to read the disk (`cached`, the new row among its users), whose move off the UI thread for every user, with `App::cef_dir_verdict`, is the S20 PARKED item;
  - ranked, numbered next steps, each with a one-line why (S19's probe stage among them).

Run (PowerShell): `Get-Process python, manifold, gate, msedgewebview2 -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, StartTime`
Expected: nothing this plan started is still running. A leftover of this plan's own runs is stopped; another session's is left alone and named.

## Plan review

This plan was stress-tested in rounds after writing (writing-plans self-review plus the adversarial pass WORKING_STYLE asks for). Each round's catches were fixed in place:

- **Round 1 (while drafting, against the code at 42a4c80b):**
  - dead code between commits: `profile::downloads_dir` moved into Task 1, and `App::download_folder` into Task 3, where they are first used;
  - doc comments citing items that did not exist yet: the screenshot doc moved to Task 3, the message contract to Task 7;
  - a let-else on an `unsafe` initializer needs parentheses;
  - `Option<&PathBuf>` is not `Option<&Path>`: the code uses `as_path()`;
  - clippy's `op_ref` on byte-slice comparisons: slices are bound before `==`;
  - the RED check scoped to `crates/`, since the wrapper dirties `docs/runs.md`;
  - the `run.json` end pattern;
  - a break arriving before `keep`: the sticky `Held`;
  - the range counter incremented before the write.
- **Round 2 (fresh read):**
  - `ids("save location")` could match other rows, so the test now checks `contains`;
  - the app's `set_dark` had no remaining reader once the hosts take `dark` from the settings, so it is deleted (S3);
  - a rerun of the download views in one work folder would number every name, so the folder is emptied first (S18);
  - `COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default()` is avoided by starting from `..._NONE`;
  - the `download-real` check keeps the old `SLEEP 6` framing (S16);
  - the WSL commands use `--cd`, so no quoted path crosses `wsl.exe`'s re-join.
- **Round 3 (adversarial check of every anchor and API against 42a4c80b and the cargo registry):**
  - checked against the registry sources: the WebView2 calls (`SetResultFilePath`, `SetCancel`, `InterruptReason`, `ICoreWebView2_13::Profile`, `SetDefaultDownloadFolderPath`, `COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE`), `Param<PCWSTR>` for `&HSTRING` and `From<&Path>` for `HSTRING`. They exist with the signatures the code uses; none of the plan's code is compiled yet;
  - re-checked: every line anchor the tasks edit; the only `spawn_tab` caller (`tabs.rs:390`, after the `cfg(test)` early return); every host's `dark` read and `SetTheme` arm; every hit of the stale-name sweep on today's tree (each has a task that rewrites it);
  - the fingerprint test prints `{:#018x}`, without underscores, so Task 3's expected message was fixed;
  - Task 2 Step 6 named a Linux check it does not run; its heading was fixed;
  - Task 6's GREEN check required `slow.zip`, but the engine may delete a failed download's partial file. The check now requires only `real.zip`, `resume.zip` and no numbered name;
  - the README 329 edit would have left ", (WebView2 ..." behind, so it now starts at the comma;
  - S11 miscounted the `seed_settings` callers. There are 14 in `crates/gate/src`, plus the test;
  - the sweep pattern did not cover the PARKED item this plan deletes. It now includes `resumed WebView2 retry`.

## Review log

Round 1: 30 findings (0 critical, 3 important, 27 minor).

Fixed:
- (important) Plan lifecycle: Task 1 Step 0 sets `status: active` and commits the plan; Task 8 Step 5 sets `done` with the gate row, and Step 6 greps for it.
- (important) Phantom held entries: Task 6 adds `release`, which lets go of every held download with the reported path (the broken one with its retry), plus a RED/GREEN test `a_retry_that_ends_frees_the_name_of_the_download_it_retried`; S6, Review Focus 4, the module doc and the PARKED line 128 rewrite follow. The helper's shape was compile-checked with rustc and clippy in a scratch crate.
- (important) CEF refusal and WebKit `/mnt` destination have no runtime check: S12 cites the cef-dll-sys 152.1.0 header (`windowless_rendering_enabled` forces Alloy style, `main.rs:199`); new S19 parks a gate probe stage (alternative B: a `download` stage under `VIEWS="webview2 cef"`); Task 7 adds the PARKED entry; Task 8's report says these are unit tests and builds only.
- runs.md liveness grep: `T[0-9:]+ \|  \|  \|` matches an empty `end`, then each wrapper pid is checked with `tasklist`; the two lost rows at 42a4c80b are named and closed per RUNS.md in Task 6.
- messages.rs `DownloadStarted` doc range is 513-527, with line 528 kept (two findings).
- Task 1 anchors: settings.rs test ends at 326, settings_rows test at 172, `settings_cef_directory_row` 420-476 with `commit` at 423.
- Cargo.toml: the `[target.'cfg(windows)'.dependencies]` header at 48 stays; lines 49-51 are replaced.
- Task 3 anchors: messages.rs 216, `first_frame` at 282, `spawn_tab` doc at 985; frame.rs replaces `sync_theme` at 324-349 without the `impl` block's closing brace at 350, and deletes `theme_updates` at 352-359 (that brace would otherwise have been deleted).
- host_env test: the assert with nested backticks is now a fenced block.
- Task 5 anchors: `seed_settings` 98-111, test at 398, `PathBuf` already imported at line 17.
- Grep guard: `git grep -n "download_directory" -- crates/gate/src | grep -v "^crates/gate/src/instance.rs:"`, empty today.
- Shots anchors: struct at 194-206, constructor at 264-273.
- download.rs whole-file rewrite: Step 5 says the Step 1 tests block (all four tests) is kept verbatim.
- Dead-code notes: Step 2 expects 3 failures and no warning, and the Step 9 build expects no warning (two findings).
- downloads.rs imports: only `use std::sync::{Mutex, PoisonError};` is added under line 5.
- Task 8 close-out: the report carries the status line, docs check, architecture check and numbered next steps.
- Real Downloads folder: Step 9 records `ls -A` before; Step 10 diffs after and stops on any change.
- `download-real-tab` is shot right after `SLEEP 6`, framed as before (S16).
- Commit trailer: the executing session replaces the shown `Co-Authored-By` with the one its system reminder gives.
- README:298 gets the `SetResultFilePath` wording, and the Step 5 sweep adds `DownloadStartingEventArgs::ResultFilePath` and `held until the host exits`.

Rejected: none.

Minor deferred:
- Task 3's app-side tests are never run RED: a RED run needs stubs across several files, not a one-line fix.
- The REDs of Tasks 1, 2 and 4 fail only at compile time: a behavioral RED needs stub bodies per function, not a one-line fix.
- The verdict reports every metadata failure as "Doesn't exist yet": telling causes apart needs an ancestor walk, which is a design change. (Done in the final review's fix pass, b64deb11: the nearest existing part of the path decides.)
- A failed or moved Known Folder lookup refuses every download, and the `OnceLock` cache goes stale: needs a decision on refresh and on the fallback.
- `host_path` is not the inverse of `app_path`, and `/mnt` is hardcoded: needs a decision on reading `wsl.conf`'s automount root.
- With no folder, a refused download shows a bare relative name as its path: the card text needs a design decision, not a one-line fix.

Round 2: 18 findings (0 critical, 2 important, 16 minor).

Fixed:
- (important) A folder typed with `/` gave every download a mixed-separator path: one function, `settings::full_folder` (`std::path::absolute`, no disk read), now gives the folder in Windows' form to both the row's verdict and `App::download_folder`. The tests are `a_full_folder_is_read_in_windows_form` (Task 1) and a Windows block in Task 3's folder test, both comparing text because `Path` equality ignores separators. Global Constraints, S4 and Review Focus 1 were updated to match. The outputs were checked with a scratch program on this machine.
- (important) Folder reads on the UI thread could block on an offline share:
  - The row's check ran through a second cache beside `cached` that read on a worker thread, with its own test. Round 3 removed both (see its entry).
  - `known_folder` passes `KF_FLAG_DONT_VERIFY`. The flag exists in windows 0.61.3 and 0.62.2. It also removes round 1's deferred case of a deleted Downloads folder refusing every download.
  - New S20 records the decision. Task 7 parks the hosts' naming and the CEF folder's existing UI-thread reads. CEF 152's header allows calling `on_before_download`'s callback later (read 2026-10-28).
  - Task 6 Step 10 checks the row's "Doesn't exist yet" note in `settings.png`.
  - Verification so far: that cache, the row closure and both tests passed clippy `-D warnings` against egui 0.36.2 in a scratch crate. Linking failed on the scratchpad's path length, so the tests first run at Task 1 Step 6.
- Line numbers drift as steps land: the Global Constraints say to find each edit by its quoted text.
- `webkit_shell_command`'s doc is at webkit.rs:62, not 60 (the step and the Files line).
- `HostEnv`'s `dark` doc and field are lines 32-33.
- reveal.rs: lines 35-36 stay and 37-40 are replaced. The quoted old text now starts at line 37 (two findings).
- The README 329 replacement starts with a space, and a grep checks for "download API (WebView2" (two findings).
- PARKED "Two broken downloads of one URL in one tab" gets its *Evidence*.
- Task 8: a `settings` difference in `shots-aa` is not the download path (the field clips it), so it is named as a regression or a flake.
- Task 6 Step 7: a lone failure of `a_killed_download_server_refuses_the_retry` is the existing fixed-port race, rerun with `--test-threads=1` and reported.
- The resume shot's card error now carries the number of ranges answered. RED's expected text does not depend on that number, and GREEN stops on exit 1 without the checks being loosened.
- The process check is `Get-Process cargo, gate, manifold` with `Path`. It waits for cargo or gate, asks the user to close a manifold running from this repo's `target\debug`, and leaves the user's own app alone (Task 6 Step 9, Task 8 Step 5).
- Lost runs.md rows are committed on their own before RED. Step 10's commit and its `"commit"` expectation follow.
- Task 8's report names the CEF and Servo hosts in `target/debug` as stale at protocol 6 unless this plan rebuilt and placed them (README 492-493).
- Found on the re-read after these fixes:
  - The Global Constraint no longer claims the app reads no network folder on its UI thread. The CEF folder's reads still do, and they are parked with `cef_newer_than_tested` named.
  - That cache's doc said the app sets no repaint callback, so a landed value showed on the next frame, at most `IDLE_FRAME` (100 ms) later (removed in round 3).
  - Task 6 Step 9's revert block sets `C` itself, since each Bash call starts a fresh shell.

Rejected: none.

Minor deferred:
- Reload `manifold://` tabs by each tab's own last-sent theme instead of the app-wide `last_theme`/`theme_changed`/`first`: a design change that deletes those, not a one-line fix.
- `set_settings` records settings as sent even when `SendQueue::push` drops the message on a full queue (`SEND_QUEUE_CAP` 256). Fixing it needs `Writer::send` and `ViewTab::send` to report acceptance: three signatures, not a one-line fix.

Round 3: 5 findings (0 critical, 1 important, 4 minor), after a coordinator directive applied first.

Directive (one way to read the disk for the Settings page):
- Round 2's second cache beside `cached` (a worker-thread read), its doc and its test are gone. The Download folder row reads `download_folder_kind` through `cached`, as the CEF row reads `cef_newer_than_tested`. `KF_FLAG_DONT_VERIFY` stays.
- S20 now recommends `cached` for the row, with one PARKED item for making `cached` itself off-thread for all its users plus `App::cef_dir_verdict`; the alternative is doing that move in this plan. Its round-2 alternative "read only at commit" is dropped, since `cached` already reads at most every 3 s.
- Updated to match: Global Constraints; Task 1's Files, Interfaces, Steps 1, 2 and 5, and Step 6's count (five new tests); Task 6 Step 10's settings note; Task 7's PARKED text (the host freeze and the Settings page's reads are now two entries); Task 8's architecture check; round 2's log lines. Review Focus never named the cache, so it is unchanged.

Fixed:
- (important) Task 6 Step 10 could not list the download folder. The GREEN command now sets `KEEP=1` (precedent: the `download-real-log` row), the listing globs `work/gate-chrome-shots-*/appdata/manifold/downloads` (`work_dir` adds `gate-<stage>-<pid>`, gate `main.rs:255`), and the kept work folder is removed after the check. Verified against `probes.rs:181-186` and the 2026-10-28 dl and dldir runs: every exit-0 run has an empty `work`, and every exit-1 run kept `gate-chrome-shots-<pid>`. Step 9's RED reads only `out.log`, so it never relied on the folder.
- Task 6 Step 9's expected FAIL path includes the `gate-chrome-shots-<pid>` level.
- Task 7 Step 3 ends with a `grep -nw` instruction that writes each new entry's *Evidence* as `file:line`, once Tasks 3-6 have moved the lines.
- Task 8 Step 5 records the real Downloads folder before the full gate and diffs it after, and Step 6 reports the result. This was applied although it is three bullets: it is the only check of the Goal's "Gate runs never touch the user's real Downloads folder" on the full gate, where stages such as `arrange` and `labels` launch instances with no `seed_settings` (round 4 widened the list).

Rejected: none.

Minor deferred:
- The `NOT_FORGOTTEN` entry for `settings.download_dir_draft` is inert, because `ID_SHAPES` skips an `Option<String>` (`main/tests.rs:156-160`), as `home_page_draft`'s already is. It is harmless and accurate as described. Dropping it touches the Files list, Steps 5-7 and S10, so it is not a one-line fix.

Round 4: 7 findings (0 critical, 0 important, 7 minor).

Fixed:
- Unseeded gate stages: Task 8 Step 5 names them as examples (`arrange.rs`, `labels.rs`, `focus.rs`, `stress.rs`; none runs a page download). The Global Constraint no longer says every gate instance is seeded: it says every one `seed_settings` seeds (S11). Round 3's log line says "such as". Checked at 42a4c80b: `Instance::launch` callers in arrange, labels, focus, stress, sleep, surface, suggest_groups and package have no `seed_settings`, and only package.rs mentions downloads (the engine and PDFium installs).
- The resume shot's `ranges_answered() == 0` message now says the server resumes only an open-ended `bytes=N-`, so a closed range points at `range_start` as well as the engine. Logging request heads and serving `N-M` were not adopted: each is more than a one-line change.
- Task 6 Step 9: a RED with exit 1 is RED for the wrong reason, and a stop-and-report with its work folder kept, if the `FAIL:` line is another one or does not list the two cards. A wait that times out on one stuck card reaches the same message.
- Task 6 Step 9: once the FAIL line is the expected one, the RED run's work folder is removed. The finding's premise was wrong: the RED command does not set `KEEP=1`. Its conclusion holds, because `probes::finish` (`probes.rs:181-186`) deletes the folder only on a pass.
- Task 6 Step 10: `"commit"` is the short hash (`run.ps1:62`) with no `-dirty`. The wrapper ignores only `docs/runs.md` (`run.ps1:63`), so any other modified or untracked file is named; ticked checkboxes in this plan file count. A change under `crates/` is a stop.
- Task 3 Step 1: the `apply` test's doc says it is the only test in the crate that sets the global palette and download folder. Checked: no other test in the plan's protocol code reads `downloads::folder()` or `manifold_pages::is_dark()`.

Rejected: none.

Minor deferred:
- The new `shots_net.rs` tests are never seen failing first. A stubs-first RED needs stub bodies for `answer`, `range_start` and `ranges_answered` plus a run step, so it is not a one-line fix. A mutation RED has no commit to revert to before Step 9. Steps 9-10's RED/GREEN pair does not stand in for it: RED pins `retry_path` whatever the server does, and only GREEN's file comparison and `ranges_answered() > 0` exercise the 206 path.
