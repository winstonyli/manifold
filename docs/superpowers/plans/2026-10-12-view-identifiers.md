# Rename `engine` to `view` in code (step 0b) Implementation Plan

**Goal:** the code says what the crates say: `manifold-view-*` run "views", so the identifiers, stored keys, env vars and debug commands that say `engine` say `view` or `view_kind`.

**Decision (user, 2026-10-12):** rename the stored keys too, with no migration; breakage of old `settings.json` keys and old import-JSON seeds is accepted.

**Spec:** `docs/superpowers/specs/2026-10-12-file-renderers-design.md`, "Parked", step 0b (its rule: `engine` becomes `view` only as a prefix to a noun, never a bare `view`).

## What changes, and what does not

Changes (one mechanical commit, by script, then fixed by the compiler):

| Surface | Old | New |
|---|---|---|
| Types | `EngineKind`, `EngineState`, `EngineTab`, `EngineMessage`, `EngineDesc`, `EngineTier`, `EngineConfirm`, `EngineMissing`, `EngineOutdated`, `EngineUnavailable`, `EngineInstallUi`, `EngineJobHandle`, `EngineApp`, CEF `Engine*Handler`, `FieldType::Engine` | `ViewKind`, `ViewStatus` (not `ViewState`: that reads as zoom/scroll), `ViewTab`, `ViewMessage`, `ViewDesc`, `ViewTier`, `ViewConfirm`, `ViewMissing`, `ViewOutdated`, `ViewUnavailable`, `ViewInstallUi`, `ViewJobHandle`, `ViewApp`, `View*Handler`, `FieldType::ViewKind` |
| Wire enums | `HostToEngine`, `EngineToHost` (+ `HOST_TO_ENGINE_VARIANTS`, `ENGINE_TO_HOST_VARIANTS`) | `HostToView`, `ViewToHost` |
| Other identifiers | any identifier containing the word-part `engine`/`Engine`/`ENGINE`/`engines`, by the rule in Step 1.2 | `view`/`View`/`VIEW` before a noun, `view_kind` where a bare `view` would clash or mislead |
| JSON store field | `"engine"` in `FIELDS` (`store/model.rs`), `field::ENGINE` | `"view_kind"`, `field::VIEW_KIND` (`"view"` is taken: a JSON node's saved overrides) |
| Settings key | `default_starting_engine` | `default_starting_view_kind` |
| Debug state JSON | `"engines"` (`debug_control.rs`) | `"view_kinds"` |
| Debug commands | `INSTALLENGINE`, `ENGINESTATE`, `RESTARTENGINE`, `ENDENGINE` | `INSTALLVIEW`, `VIEWSTATE`, `RESTARTVIEW`, `ENDVIEW` |
| Host env vars (ten) | `ENGINE_SOCKET_ID`, `ENGINE_TAB_ID`, `ENGINE_TCP_PORT`, `ENGINE_TOKEN`, `ENGINE_HOST_IP`, `ENGINE_INITIAL_URL(_HEX)`, `ENGINE_CACHE_DIR`, `ENGINE_AWAIT_COOKIES`, `ENGINE_WEBVIEW2_PROFILE` | `VIEW_*` |
| Seeds and scripts | `"engine": "webview2"` in `crates/gate/src/**`, `scripts/*.sh`, `package.sh`; the four debug commands in `scripts/` | follow the table |

Does not change:

- **Search engines:** `SearchEngine`, `search_engine`, `default_search_engine`, `settings_search_engine`, and the "Search engine" UI text. Different meaning. The script's exclusion list.
- **Exe names** `manifold-engine-*` (the `[[bin]]` pin) and the **disk folder** `engine-hosts/` with its `<kind>.host` markers. Renaming the folder would orphan downloaded hosts (CEF is 172 MB) and is not a key. Parked, with the exe rename.
- **User-visible prose** (UI labels, toasts, comments, docs): "engine" stays the word users know. Only code identifiers, the table's strings and backticked identifier references in comments change.
- **Wire bytes:** `postcard` is positional, so renaming enums and variants changes no byte. `PROTOCOL_VERSION` does not bump. (The spec said it would; Task 1 Step 3 proves otherwise.)
- **`.mnf` store files:** the binary store keys fields by number (`field::ENGINE = n`), so the JSON name change touches only import/export, not saved sessions.
- **History:** dated specs and plans, `CHANGELOG.md` entries, `.superpowers/`.

What breaks (accepted): a saved `default_starting_engine` reads as the platform default; old `--import-json` seed files with `"engine"` are rejected as an unknown field; scripts and notes using the old debug commands.

## Task 1: Script, apply, compile

**Files:** create `scripts/rename_engine_to_view.py` (kept only until the commit; delete it in Step 7), modify everything it touches.

- [ ] **Step 1: Write the script.** Python, UTF-8, run as `python -P scripts/rename_engine_to_view.py < /dev/null`.
  1. Tokenize each `.rs` file into code, comment, string and char spans (raw strings, lifetimes and nested block comments handled).
  2. Collect every identifier in code spans that contains `engine` (case-insensitive) and build the mapping. Split into word parts (`snake_case`, `CamelCase`, `SCREAMING_CASE`). Rule: the part `engine`/`engines` becomes `view`/`views` when it is the first part and a noun follows (`EngineTab`, `engine_io`, `engine_info`, `ENGINE_TOKEN`) or it follows `to`/`from` (`HostToView`, `to_view`); otherwise it becomes `view_kind`/`view_kinds` (a bare `engine`, `default_engine`, `edit_engine`, `engine_of` -> `view_kind_of`, `ENGINE` -> `VIEW_KIND`). Overrides: `EngineState`/`engine_state*` -> `ViewStatus`/`view_status*` (`ViewState` reads as zoom/scroll). Existing identifiers already use `view` for other things (`view`, `views`, `view_of`, `edit_view`, `ViewMode`, `NO_VIEW`, `field::VIEW`, `JsonView`, `image_view`), which is why the default for anything not clearly a prefix is `view_kind`. Skip the exclusion list (anything with the part `search`, plus `engine-hosts` strings).
  3. **Collision check before writing:** every new name must not already exist as an identifier anywhere in the workspace; print the clashes and stop. (This is what stops a local `engine` silently shadowing an existing `view`.)
  4. Rewrite code spans by the mapping; rewrite comment and string spans only for whole-token matches of *multi-part* mapped identifiers (`EngineKind`, `engine_state`), never the bare words; apply the table's string literals explicitly (`"engine"` in `FIELDS` and in seed JSON, `"engines"`, the four commands, the ten env var names, `default_starting_engine`).
  5. Apply the same explicit string list to `scripts/*.sh`, `scripts/*.ps1`, `.github/`, `.githooks/`, `README.md` and `CLAUDE.md` (not `docs/superpowers/`, not `.superpowers/`).
  6. Print the mapping (old to new, count) as `mapping.txt` in the scratchpad.
- [ ] **Step 2: Dry run.** Run with `--dry-run`; read the mapping and the clash list; fix the exclusion list until there are no clashes and no surprising rows (search, `engine-hosts`, CEF API names, test names reading oddly).
- [ ] **Step 3: Prove the wire claim.** Before applying, add a throwaway test in `ipc-protocol` that serializes one value of each `HostToEngine` and `EngineToHost` variant with `postcard`, writes the hex to a file; run it; after the rename, run the same test (renamed) and `diff` the two files. Identical means no bump. Delete the test afterwards.
- [ ] **Step 4: Apply, then compile.** `cargo fmt --all` (renames change line widths), then `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`, `cargo clippy` (the CLAUDE.md line), `cargo test -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate`. Fix what the compiler finds by hand, never by widening the script.
- [ ] **Step 5: The crates this machine cannot build.** `manifold-view-cef`, `-servo` and `-webkit` get no compile here, so review their diffs by eye (`git diff --stat` for the three; about 15 files) and check them: `cargo check -p manifold-view-cef` if the CEF download is present, WebKit through WSL (`CARGO_TARGET_DIR=target-linux cargo check -p manifold-view-webkit`, command in README). Servo is not checked (about 33 minutes); the diff is one file, read it line by line. Say in the status line which were compiled and which were only read.
- [ ] **Step 6: Gate.** `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`, then `scripts/gate.sh LOG full` detached at BelowNormal; report CPU load and Defender state. The gate drives the renamed debug commands and seeds, so it proves the script-side renames.
- [ ] **Step 7: Docs and commit.** README (architecture list, the debug command names, env var and settings key mentions), CLAUDE.md if it names any of them, the spec (step 0b shipped, with the "accepted breakage" line). Delete the script. `git add -A`, commit "Rename engine to view in code, stored keys, env vars and debug commands (step 0b)".

## Risks

- **Silent shadowing** if the collision check misses a case: guarded by Step 1.3 and by tests plus the gate.
- **Unbuilt crates** (CEF, Servo, WebKit): Step 5; the residual risk is a name typo in code only those crates compile, found at the next CEF or release build.
- **Persisted data**: only `settings.json`'s one key; stated above.

## Stress-test record

- **Round 1** (claims checked against the code): 3 defects. The part-by-part rule alone gives `ViewState` for `EngineState` (the table said `ViewStatus`); it also gives `view_of`, `edit_view` and `field::VIEW` for `engine_of`, `edit_engine` and `field::ENGINE`, all already taken (the collision check would stop, but the rule should not produce them), so the rule now prefers `view_kind` outside the prefix-before-noun case; the seed-JSON field cannot be `"view"` (taken), which the table already had. Confirmed: `postcard` framing is positional (`channel.rs`), the `.mnf` store keys fields by number (`FIELDS` has `no` and `name`), the four debug commands and ten env vars are the full set.
- **Execution** (2026-10-12): the first apply also rewrote bare prose `engine` in comments and strings (the bare word was in the code mapping); caught by reading the Servo diff, stashed, and fixed so non-code spans change only multi-part tokens, backticked identifiers in comments and `{arg}` format args. Dry runs also caught `an_view_*` article grammar and a lost underscore. Step 3 needed no throwaway test: `wire_shape_matches_the_recorded_fingerprint` hashes every variant and passed unchanged. CEF and Servo were read, not compiled (every changed name there is one of our own: `HostToView`, `ViewToHost`, `View*Handler`, `ViewApp`, `VIEW_*`); WebKit's WSL check did not run (WSL unresponsive).
