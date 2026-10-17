---
status: done
---

# Unified Node Menu and Close Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the seven per-kind context-menu state pairs/builders with one `node_menu` state and one `build_node_menu` dispatcher, and the per-kind close/reveal/copy-path code with one `close_node`, `reveal_node`, `node_file_path`.

**Architecture:** Same "shared frame + per-kind content, dispatched on `ItemNodeKind`" pattern as the toolbar rewrite. Pure rules (`closed_node_for`, `node_menu_config`) are free functions with unit tests; `App` methods (`close_node`, `close_node_and_save`, `node_file_path`, `reveal_node`, `apply_node_menu_action`, `build_node_menu`) apply them. Group and Tab menu bodies stay bespoke, reached from the dispatcher.

**Tech Stack:** Rust, egui 0.36.2.

**Spec:** `docs/superpowers/specs/2026-09-23-node-menu-and-close-design.md`. The "Plan-time corrections" below supersede it where they conflict; Task 3 copies them into the spec.

## Plan-time corrections (to the spec)

1. **One shared fade Id** (`"node_menu_fade"`), not one per kind. Per-kind fade Ids keyed off the *current* kind break `fade_popup_state`'s "poll every frame" rule (a kind's animation would go unpolled while another kind's menu is open, so the next open jumps straight to visible). Cost: right-clicking a card of a *different* kind while a menu is open now moves the menu instantly instead of cross-fading — exactly what right-clicking a second card of the *same* kind already does today. The popup **area** Ids stay per kind (`"download_context_menu"`, …, `"tab_context_menu"`), carried in `NodeMenuConfig::area_id` for the five simple kinds.
2. **`node_file_path`**, not `node_copy_path` — it feeds both Reveal and Copy Path, so it's named for what it returns.
3. **Escape closes any node menu.** Today Graph view's Escape clears only `group_context_menu`; with one state it clears `node_menu`, so Escape now also dismisses a Download/Note/Image/Pdf/Json/Tab menu. A second, deliberate, tiny behavior change (flag it in the final report).
4. **Tab hover ✕ keeps its own `close_requested` local** (applied as `close_tab`, before `clicked_focus`) — `close_node`'s Tab arm is exactly `close_tab`, and folding it in would move its apply point past `clicked_focus` for no gain.
5. **The Downloads popup's per-row Remove** also goes through `close_node` (it is a single node closing; spec: "used everywhere a single node closes").

## Global Constraints

- Files in scope: `crates/manifold/src/main.rs`, `README.md`, the spec above. No new dependencies.
- `cargo build -p manifold` must finish with **zero warnings** after every task (baseline has zero). `cargo test -p manifold` must pass after every task (baseline: 12 tests).
- Behavior stays identical except: (a) hover ✕ on Note/Image/Pdf/Json now records to Recently Closed; (b) Escape closes any node menu (correction 3); (c) cross-kind menu switch moves instantly (correction 1).
- Menu look is unchanged: same items, labels, order, widths (Download 170 "Remove from List" and never "Remove from Group"; Note 150; Image/Pdf/Json 170), same Group and Tab bodies.
- Menu "Remove from Group" keeps setting `group = None` (top level) — do not "fix" it to the grandparent rule the hover eject uses.
- `close_node` never saves; `close_node_and_save` saves unless the node was a Download (downloads aren't persisted).
- A synthetic JSON leaf (`JsonPayload::key.is_some()`) is never recorded to Recently Closed.
- When moving code, carry its existing comments verbatim; rewrite only comments the change makes false (including ones naming removed fields/functions). Don't add `main.rs:NNNN` line references in comments.
- **Live verification** (tasks that say "live"): `cargo build -p manifold`, then drive `target/debug/manifold.exe --headless` over stdin with a bash `coproc`, the way `scripts/headless-smoke-test.sh` does — read that script first (SCREENSHOT path translation, timing). **Always** isolate APPDATA: `rm -rf target/verify/appdata && mkdir -p target/verify/appdata && APPDATA="$(cygpath -w "$PWD/target/verify/appdata")" target/debug/manifold.exe --headless` — headless mode otherwise reads and overwrites the user's real `%APPDATA%\manifold\session.json`. Send `RESIZE 1300 800` first. Right-click is `CLICK x y right` (check `HELP` output for the exact button word). There is no ADDNOTE command: create a note by right-clicking empty canvas and clicking the pending card's "Note" entry. Test files: generate a PNG with Python (`zlib`+`struct`), a minimal one-page PDF, and a small JSON file into `target/verify/files/`; add with `ADDIMAGE`/`ADDPDF`/`ADDJSON <windows-path>`. Inspect screenshots with the Read tool; crop/zoom with Python if needed. Find card coordinates from screenshots.

---

### Task 1: One close path (`close_node`, `reveal_node`, `node_file_path`)

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - new free fn `closed_node_for` next to `enum ClosedNode`
  - `close_group_member` → `close_node` (and its one caller in `close_group`)
  - new `close_node_and_save` right after it
  - `reveal_download`/`reveal_image`/`reveal_pdf`/`reveal_json` → `node_file_path` + `reveal_node`
  - callers of the old `reveal_*` (the Downloads popup, `render_image_toolbar_content`, `download_clicked` in `build_graph_ui`, the five simple menu builders)
  - hover ✕ locals and apply blocks in `build_graph_ui`
  - the doc comment above `enum ItemNodeKind` that names `close_group_member`'s `MemberKind`
  - `mod tests`

**Interfaces:**
- Produces (Task 2 uses these):
  - `fn closed_node_for(payload: &NodePayload) -> Option<ClosedNode>` (free fn)
  - `fn close_node(&mut self, id: &str)` — no save
  - `fn close_node_and_save(&mut self, id: &str)` — saves unless Download
  - `fn node_file_path(&self, id: &str) -> Option<String>` — Download via `windows_path_for_download`; Image/Pdf/Json stored `path`; else `None`
  - `fn reveal_node(&self, id: &str)`

- [ ] **Step 0: Capture menu baselines (before any code change)**

Live, with the current `main` binary (`cargo build -p manifold` first). Add one image, one PDF, one JSON (`ADDIMAGE`/`ADDPDF`/`ADDJSON`), create one note, and `NEWGROUP` so a Group card exists. A Download card can't be created headless without a real download — skip its baseline and note that in the report. For each of Note, Image, Pdf, Json, Group, Tab: `CLICK <card> right`, wait ~300ms (fade-in done), `SCREENSHOT target/verify/baseline/menu-<kind>.png`, then `CLICK` empty canvas to dismiss. Record the card coordinates used in the report so Task 3 can reuse them. Also `SCREENSHOT target/verify/baseline/recently-closed-empty.png` of the toolbar. Do not commit screenshots.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` at the end of `main.rs`:

```rust
    // `Tab` is left out for the same reason as the test above: `TabPayload`
    // can't be constructed in a unit test. Its arm returns `None` by the
    // match's exhaustiveness, and `close_node` routes tabs to `close_tab`
    // (which keeps its own Recently Closed stack) before this is consulted.
    #[test]
    fn closed_node_for_records_only_undo_worthy_payloads() {
        assert!(matches!(
            closed_node_for(&NodePayload::Note(NotePayload { text: "n".into(), editing: false })),
            Some(ClosedNode::Note(text)) if text == "n"
        ));
        assert!(matches!(
            closed_node_for(&NodePayload::Image(ImagePayload { path: "img.png".into(), width: 1, height: 1 })),
            Some(ClosedNode::Image(path)) if path == "img.png"
        ));
        assert!(matches!(
            closed_node_for(&NodePayload::Pdf(PdfPayload { path: "doc.pdf".into(), outcome: None })),
            Some(ClosedNode::Pdf(path)) if path == "doc.pdf"
        ));
        assert!(matches!(
            closed_node_for(&NodePayload::Json(JsonPayload {
                path: "data.json".into(),
                value: serde_json::Value::Null,
                key: None,
            })),
            Some(ClosedNode::Json(path)) if path == "data.json"
        ));
        // A synthetic "Expand as Graph" leaf: never recorded.
        assert!(closed_node_for(&NodePayload::Json(JsonPayload {
            path: "data.json".into(),
            value: serde_json::Value::Null,
            key: Some("k".into()),
        }))
        .is_none());
        assert!(closed_node_for(&NodePayload::Group(GroupPayload { name: "g".into() })).is_none());
        assert!(closed_node_for(&NodePayload::Download(DownloadPayload {
            path: "file.bin".into(),
            bytes_received: 0,
            total_bytes: None,
            completed: true,
        }))
        .is_none());
    }
```

(If `JsonPayload::key` isn't `Option<String>`, match its real type — check the struct.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p manifold closed_node_for`
Expected: compile error, `cannot find function closed_node_for`.

- [ ] **Step 3: Add `closed_node_for`**

Directly after `enum ClosedNode { … }`:

```rust
/// Which `ClosedNode` (if any) closing a node with this payload should add
/// to "Recently Closed" -- the one rule behind every single-node close
/// (`App::close_node`: hover ✕, context-menu Delete, closing a group's
/// members). `None` for a Tab (it has its own `closed_tabs` stack via
/// `close_tab`), a Group (`close_group` snapshots it into
/// `closed_groups`), a Download (no "closed" state), and a synthetic
/// "Expand as Graph" JSON leaf -- recording that as a `ClosedNode::Json`
/// would let "undo" re-add the *whole source file* as a fresh top-level
/// card, not the leaf that was actually closed. Only a genuine top-level
/// JSON node's own path is real undo-worthy state.
fn closed_node_for(payload: &NodePayload) -> Option<ClosedNode> {
    match payload {
        NodePayload::Note(note) => Some(ClosedNode::Note(note.text.clone())),
        NodePayload::Image(image) => Some(ClosedNode::Image(image.path.clone())),
        NodePayload::Pdf(pdf) => Some(ClosedNode::Pdf(pdf.path.clone())),
        NodePayload::Json(json) => json.key.is_none().then(|| ClosedNode::Json(json.path.clone())),
        NodePayload::Tab(_) | NodePayload::Group(_) | NodePayload::Download(_) => None,
    }
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p manifold closed_node_for`
Expected: PASS. (`closed_node_for` is unused outside tests until Step 5, so don't run the zero-warning build yet.)

- [ ] **Step 5: Replace `close_group_member` with `close_node` + `close_node_and_save`**

Replace the whole `close_group_member` fn (doc comment, `MemberKind` enum, body) with:

```rust
    /// Closes one node, applying its kind's full cleanup: a Group recurses
    /// (`close_group`), a Tab goes through `close_tab` (feeding "Recently
    /// Closed" tabs), and a Download/Note/Image/Pdf/Json records whatever
    /// `closed_node_for` says (feeding "Recently Closed" nodes), frees its
    /// GPU texture if it's an Image/Pdf, clears `json_viewer_node` if it
    /// was showing this node, then `remove_node`s it. The single path
    /// behind a card's hover ✕, its context-menu Delete/"Remove from
    /// List", the Downloads popup's Remove, and `close_group`'s per-member
    /// loop -- which is why it doesn't save: `close_group` saves once for
    /// the whole group, and the UI paths go through `close_node_and_save`.
    /// (Before this was shared, `close_tab`'s own no-op-on-non-tab guard
    /// left every non-tab, non-group member of a closed group merely
    /// reparented by `dissolve_group`, never actually destroyed -- a gap
    /// confirmed while building "Expand as Graph", whose whole output is
    /// `JsonPayload` leaves nested inside Groups.)
    fn close_node(&mut self, id: &str) {
        let Some((kind, closed)) =
            self.nodes.get(id).map(|node| (item_node_kind(&node.payload), closed_node_for(&node.payload)))
        else {
            return;
        };
        match kind {
            ItemNodeKind::Group => self.close_group(id),
            ItemNodeKind::Tab => self.close_tab(id),
            ItemNodeKind::Download | ItemNodeKind::Note | ItemNodeKind::Image | ItemNodeKind::Pdf | ItemNodeKind::Json => {
                if let Some(closed) = closed {
                    self.record_closed_node(closed);
                }
                if matches!(kind, ItemNodeKind::Image | ItemNodeKind::Pdf) {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.free_image(id);
                    }
                }
                if self.json_viewer_node.as_deref() == Some(id) {
                    self.json_viewer_node = None;
                }
                self.remove_node(id);
            }
        }
    }

    /// `close_node` for a single user action (hover ✕, context-menu
    /// Delete, Downloads popup Remove), then persists the session -- except
    /// for a Download, which isn't part of the saved session at all.
    fn close_node_and_save(&mut self, id: &str) {
        let persisted = self.nodes.get(id).is_some_and(|node| node.as_download().is_none());
        self.close_node(id);
        if persisted {
            self.save_session();
        }
    }
```

In `close_group`, change `self.close_group_member(&tid);` to `self.close_node(&tid);`, and update `close_group`'s doc comment where it says "via `close_group_member`" to "via `close_node`". Update the doc comment above `enum ItemNodeKind` ("Mirrors `close_group_member`'s existing `MemberKind` match…") to name `close_node` instead and drop the `MemberKind` mention, keeping its borrow-reasoning sentence.

- [ ] **Step 6: Replace the four `reveal_*` fns with `node_file_path` + `reveal_node`**

Delete `reveal_download`, `reveal_image`, `reveal_pdf`, `reveal_json` (and their doc comments). In their place:

```rust
    /// The real file behind a Download/Image/Pdf/Json node, in the form
    /// Explorer and a pasted clipboard path understand -- what both
    /// "Open Containing Folder" (`reveal_node`) and "Copy Path" use. A
    /// download's path may be a WSL-side POSIX path (WebKit downloads), so
    /// it goes through `windows_path_for_download`; an image/PDF/JSON path
    /// always came from a real Windows file picker, so it's used as-is.
    /// `None` for kinds with no backing file (Tab, Group, Note).
    fn node_file_path(&self, id: &str) -> Option<String> {
        match &self.nodes.get(id)?.payload {
            NodePayload::Download(_) => self.windows_path_for_download(id),
            NodePayload::Image(image) => Some(image.path.clone()),
            NodePayload::Pdf(pdf) => Some(pdf.path.clone()),
            NodePayload::Json(json) => Some(json.path.clone()),
            NodePayload::Tab(_) | NodePayload::Group(_) | NodePayload::Note(_) => None,
        }
    }

    /// Opens a node's file's containing folder with the file pre-selected
    /// (a plain click on a download card does this too -- same "just show
    /// the obvious thing, no custom UI" posture as the download itself, see
    /// `EngineToHost::DownloadCompleted`'s doc comment). No-op for kinds
    /// with no file.
    fn reveal_node(&self, id: &str) {
        if let Some(path) = self.node_file_path(id) {
            Self::reveal_in_explorer(&path);
        }
    }
```

Update `windows_path_for_download`'s doc ("Shared by `reveal_download` and the download context menu's "Copy Path"") → "Used by `node_file_path`." Update `reveal_in_explorer`'s doc ("the shared mechanism behind every `reveal_*` method below, previously hand-copied four times…") → "the mechanism behind `reveal_node`."

Replace every remaining call: `self.reveal_download(&id)` / `self.reveal_image(…)` / `self.reveal_pdf(&id)` / `self.reveal_json(&id)` → `self.reveal_node(…)` (sites: Downloads popup `reveal_id`, `render_image_toolbar_content`'s `clicked.0`, `build_graph_ui`'s `download_clicked`, and the Reveal arm of each simple menu builder). Verify: `grep -n "reveal_download\|reveal_image\|reveal_pdf\|reveal_json" crates/manifold/src/main.rs` → no matches.

- [ ] **Step 7: Route every single-node close through `close_node`**

In the five simple menu builders (still separate until Task 2):
- Download builder: `Some(NodeMenuAction::Delete) => self.remove_node(&id),` → `Some(NodeMenuAction::Delete) => self.close_node_and_save(&id),`; its CopyPath arm → `if let Some(path) = self.node_file_path(&id) { ui.ctx().copy_text(path); }`.
- Note/Image/Pdf/Json builders: replace each whole `Some(NodeMenuAction::Delete) => { … }` block with `Some(NodeMenuAction::Delete) => self.close_node_and_save(&id),`; replace each Image/Pdf/Json CopyPath arm body with the same `node_file_path` form as above.

In the Downloads popup (the block `if let Some(id) = remove_id { self.remove_node(&id); }`): → `self.close_node_and_save(&id);`.

In `build_graph_ui`:
- Delete the locals `download_close_requested`, `note_close_requested`, `image_close_requested`, `pdf_close_requested`, `json_close_requested`; add one, where `download_close_requested` was declared:

```rust
        // Set by a Download/Note/Image/Pdf/Json card's hover ✕ -- all five
        // close through the same `close_node_and_save` path their context
        // menu's Delete uses. A tab's ✕ keeps `close_requested` (above) and
        // a group's ✕ keeps `group_close_requested` (it dissolves, not closes).
        let mut node_close_requested: Option<String> = None;
```

- At each card's hover-✕ site, replace `download_close_requested = Some(id.clone());`, `note_close_requested = …`, `image_close_requested = …`, `pdf_close_requested = …`, `json_close_requested = …` with `node_close_requested = Some(id.clone());`. The image card also has `if response.clicked() && image_close_requested.as_deref() != Some(id.as_str())` — change to `node_close_requested.as_deref()`. Grep each old name for any other use and convert it the same way.
- Replace the apply block `if let Some(id) = download_close_requested { self.remove_node(&id); }` with:

```rust
        if let Some(id) = node_close_requested {
            self.close_node_and_save(&id);
        }
```

- Delete the apply blocks for `note_close_requested`, `image_close_requested.or(pdf_close_requested)` (including its comment about `.or()`), and `json_close_requested`.
- Update the doc comment on `draw_node_close_button` — "(which `*_close_requested` to set)" → "(`node_close_requested` for every kind that uses it)".
- Leave `close_requested` (tab) and `group_close_requested` (→ `dissolve_group`) as they are.

Verify: `grep -n "close_group_member\|download_close_requested\|note_close_requested\|image_close_requested\|pdf_close_requested\|json_close_requested\|MemberKind" crates/manifold/src/main.rs` → no matches.

- [ ] **Step 8: Build and test**

Run: `cargo build -p manifold` → zero warnings. `cargo test -p manifold` → 13 pass.

- [ ] **Step 9: Live check of the behavior change**

Live (`target/verify/task1/`), fresh APPDATA: add an image, a PDF, a JSON, create a note. Hover each card and click its ✕ → after each, screenshot the toolbar: "Recently Closed" count increments (1→4). Open Recently Closed, reopen the note → it reappears with its text. Then add another image and delete it via right-click → Delete → count increments; reopen → image card back. `NEWGROUP`, drag the root tab card onto the group card, hover the group card's ✕ → group dissolved, the tab remains, Recently Closed count unchanged by the dissolve.

- [ ] **Step 10: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "Route every single-node close through close_node

Hover ✕ on Note/Image/Pdf/Json now records to Recently Closed, matching
menu Delete. reveal_* collapse into reveal_node/node_file_path.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: One context-menu state and dispatcher

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - `NodeMenuConfig` (add `area_id`, derives) and `build_node_context_menu` (drop its `id` param)
  - new free fn `node_menu_config` after `build_node_context_menu`
  - `App` fields `group_/download_/note_/image_/pdf_/json_/tab_context_menu(_cache)` and their initializers
  - the five simple builders → deleted, replaced by `build_node_menu` + `apply_node_menu_action`
  - `build_group_context_menu` / `build_tab_context_menu` prologues and closing assignments
  - `build_graph_ui`: Escape handler, right-click dispatch, group enter/leave clears, the seven builder calls
  - `DebugCommand::ExportGroup` handler
  - `mod tests`

**Interfaces:**
- Consumes (from Task 1): `close_node_and_save(&mut self, &str)`, `reveal_node(&self, &str)`, `node_file_path(&self, &str) -> Option<String>`.
- Produces: `App::node_menu: Option<(String, egui::Pos2)>`, `App::node_menu_cache: Option<(String, egui::Pos2)>`, `fn node_menu_config(kind: ItemNodeKind, has_group: bool) -> Option<NodeMenuConfig>`, `fn build_node_menu(&mut self, ui: &mut egui::Ui)`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests`:

```rust
    // One row per kind, pinned to the exact menus each kind showed before
    // the per-kind builders were merged (see the node-menu spec).
    #[test]
    fn node_menu_config_matches_each_kinds_menu() {
        let simple = |area_id, copy_text, reveal_and_copy_path, show_expand, has_group, delete_label, width| {
            Some(NodeMenuConfig { area_id, copy_text, reveal_and_copy_path, show_expand, has_group, delete_label, width })
        };
        for has_group in [false, true] {
            // Download never offers "Remove from Group", grouped or not.
            assert_eq!(
                node_menu_config(ItemNodeKind::Download, has_group),
                simple("download_context_menu", false, true, false, false, "Remove from List", 170.0)
            );
            assert_eq!(
                node_menu_config(ItemNodeKind::Note, has_group),
                simple("note_context_menu", true, false, false, has_group, "Delete", 150.0)
            );
            assert_eq!(
                node_menu_config(ItemNodeKind::Image, has_group),
                simple("image_context_menu", false, true, false, has_group, "Delete", 170.0)
            );
            assert_eq!(
                node_menu_config(ItemNodeKind::Pdf, has_group),
                simple("pdf_context_menu", false, true, false, has_group, "Delete", 170.0)
            );
            assert_eq!(
                node_menu_config(ItemNodeKind::Json, has_group),
                simple("json_context_menu", false, true, true, has_group, "Delete", 170.0)
            );
            // Group and Tab draw their own bespoke menus.
            assert_eq!(node_menu_config(ItemNodeKind::Group, has_group), None);
            assert_eq!(node_menu_config(ItemNodeKind::Tab, has_group), None);
        }
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p manifold node_menu_config`
Expected: compile errors (`node_menu_config` not found; `NodeMenuConfig` has no `area_id`; no `PartialEq`/`Debug`).

- [ ] **Step 3: Extend `NodeMenuConfig`, add `node_menu_config`**

Change `struct NodeMenuConfig` to (keep its doc comment, appending one sentence: "`area_id` is the popup's own egui `Area` id -- still distinct per kind, so egui's per-area memory never bleeds between kinds."):

```rust
#[derive(Debug, PartialEq)]
struct NodeMenuConfig {
    area_id: &'static str,
    copy_text: bool,
    reveal_and_copy_path: bool,
    /// Shows an "Expand as Graph" button -- only ever `true` for a JSON
    /// node's own menu. See `App::expand_json_as_graph`.
    show_expand: bool,
    has_group: bool,
    delete_label: &'static str,
    width: f32,
}
```

Change `build_node_context_menu`'s signature to drop `id: &str` and use `config.area_id`:

```rust
fn build_node_context_menu(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    fade: f32,
    config: NodeMenuConfig,
) -> (Option<NodeMenuAction>, bool) {
    let mut action = None;
    let anchor = egui::Rect::from_min_size(pos, egui::Vec2::ZERO);
    let area_rect =
        build_list_popup_frame(ui.ctx(), config.area_id, anchor, config.width, config.width, fade, |ui| {
```

(rest of the body unchanged). In its doc comment, "the caller still owns every kind-specific effect (`reveal_image`/`as_pdf().path`/`gpu.free_image`/etc.)" → "the caller (`App::apply_node_menu_action`) owns every effect".

Add after `build_node_context_menu`:

```rust
/// Which items a Download/Note/Image/Pdf/Json card's right-click menu
/// shows -- each kind's own row of what used to be five separate
/// builders. `None` for Group and Tab, whose menus (rename field, color
/// swatches, pin, …) are drawn by their own bespoke builders instead.
/// Download hardcodes `has_group: false`: that menu never offered
/// "Remove from Group", regardless of whether the download actually has
/// one (pre-existing, kept as-is).
fn node_menu_config(kind: ItemNodeKind, has_group: bool) -> Option<NodeMenuConfig> {
    let file = |area_id, show_expand| NodeMenuConfig {
        area_id,
        copy_text: false,
        reveal_and_copy_path: true,
        show_expand,
        has_group,
        delete_label: "Delete",
        width: 170.0,
    };
    match kind {
        ItemNodeKind::Download => Some(NodeMenuConfig {
            has_group: false,
            delete_label: "Remove from List",
            ..file("download_context_menu", false)
        }),
        ItemNodeKind::Note => Some(NodeMenuConfig {
            copy_text: true,
            reveal_and_copy_path: false,
            width: 150.0,
            ..file("note_context_menu", false)
        }),
        ItemNodeKind::Image => Some(file("image_context_menu", false)),
        ItemNodeKind::Pdf => Some(file("pdf_context_menu", false)),
        ItemNodeKind::Json => Some(file("json_context_menu", true)),
        ItemNodeKind::Group | ItemNodeKind::Tab => None,
    }
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p manifold node_menu_config`
Expected: the only remaining compile errors are at the five old builders' `build_node_context_menu(ui, "<kind>_context_menu", …, NodeMenuConfig { … })` calls — Step 6 deletes those builders, so don't patch them. The test goes green at Step 9, when the whole crate compiles again.

- [ ] **Step 5: One state field**

Replace the fourteen fields `group_context_menu` … `tab_context_menu_cache` (with their doc comments) by:

```rust
    /// `(node_id, screen_pos)` while a right-click context menu is showing
    /// for that node's card -- one field for every kind, so at most one
    /// menu is ever open by construction. Right-click is otherwise the
    /// rectangle-select gesture over the graph panel (see that gesture's
    /// own doc comment in `build_graph_ui`); a press landing on any card
    /// opens this instead of starting a marquee. Drawn by
    /// `App::build_node_menu`, which dispatches on the node's kind.
    node_menu: Option<(String, egui::Pos2)>,
    /// Last real value of `node_menu`, kept around so `build_node_menu`
    /// can keep drawing (fading out, not vanishing instantly) for a few
    /// frames after `node_menu` itself goes back to `None` -- see
    /// `fade_popup_state`.
    node_menu_cache: Option<(String, egui::Pos2)>,
```

Replace the fourteen initializers with `node_menu: None,` and `node_menu_cache: None,`. Fix the `renaming_group` doc comment's "(see `group_context_menu`)" → "(see `node_menu`)".

- [ ] **Step 6: The dispatcher**

Delete `build_download_context_menu`, `build_note_context_menu`, `build_image_context_menu`, `build_pdf_context_menu`, `build_json_context_menu` (and their doc comments). In their place:

```rust
    /// The one right-click context menu for every node card. Fades via the
    /// shared `node_menu`/`node_menu_cache` (one fade Id for all kinds --
    /// per-kind Ids would go unpolled while another kind's menu is open,
    /// breaking `fade_popup_state`'s continuous-polling rule), resolves the
    /// node's kind, and dispatches: Group and Tab to their own bespoke
    /// builders, every other kind to the shared `build_node_context_menu`
    /// shell configured by `node_menu_config`. A node that's gone (closed
    /// from its own menu, or elsewhere) clears the menu at once.
    fn build_node_menu(&mut self, ui: &mut egui::Ui) {
        let current = self.node_menu.clone();
        let Some(((id, pos), fade)) =
            fade_popup_state(ui.ctx(), egui::Id::new("node_menu_fade"), &current, &mut self.node_menu_cache)
        else {
            return;
        };
        let Some((kind, has_group)) =
            self.nodes.get(&id).map(|node| (item_node_kind(&node.payload), node.group.is_some()))
        else {
            self.node_menu = None;
            return;
        };
        match kind {
            ItemNodeKind::Group => self.build_group_context_menu(ui, id, pos, fade),
            ItemNodeKind::Tab => self.build_tab_context_menu(ui, id, pos, fade),
            ItemNodeKind::Download | ItemNodeKind::Note | ItemNodeKind::Image | ItemNodeKind::Pdf | ItemNodeKind::Json => {
                let Some(config) = node_menu_config(kind, has_group) else {
                    return;
                };
                let (action, clicked_outside) = build_node_context_menu(ui, pos, fade, config);
                let close = action.is_some() || clicked_outside;
                if let Some(action) = action {
                    self.apply_node_menu_action(ui.ctx(), &id, action);
                }
                if close {
                    self.node_menu = None;
                }
            }
        }
    }

    /// Applies whichever item was clicked in a Download/Note/Image/Pdf/Json
    /// card's menu. Each action only ever appears on kinds where it makes
    /// sense (`node_menu_config`), so none of these re-check the kind.
    fn apply_node_menu_action(&mut self, ctx: &egui::Context, id: &str, action: NodeMenuAction) {
        match action {
            NodeMenuAction::CopyText => {
                if let Some(text) = self.nodes.get(id).and_then(|n| n.as_note()).map(|n| n.text.clone()) {
                    ctx.copy_text(text);
                }
            }
            NodeMenuAction::Reveal => self.reveal_node(id),
            NodeMenuAction::CopyPath => {
                if let Some(path) = self.node_file_path(id) {
                    ctx.copy_text(path);
                }
            }
            NodeMenuAction::ExpandAsGraph => {
                self.expand_json_as_graph(id);
                self.save_session();
            }
            NodeMenuAction::RemoveFromGroup => {
                if let Some(node) = self.nodes.get_mut(id) {
                    node.group = None;
                }
            }
            NodeMenuAction::Delete => self.close_node_and_save(id),
        }
    }
```

If the borrow checker rejects `ui.ctx()` alongside `&mut self` here, bind `let ctx = ui.ctx().clone();` first (an `egui::Context` clone is a cheap `Arc` clone).

- [ ] **Step 7: Adapt the Group and Tab builders**

`build_group_context_menu`: change the signature to `fn build_group_context_menu(&mut self, ui: &mut egui::Ui, id: String, pos: egui::Pos2, fade: f32)` and delete its prologue — the `let current = self.group_context_menu.clone();`, the `fade_popup_state(…)` let-else, and the `if !self.nodes.contains_key(&id) { … }` block (the dispatcher already did all three). Everything from `let mut close = false;` on stays, except the final `self.group_context_menu = None;` → `self.node_menu = None;`. If its doc comment describes owning the fade/state, reword to "Reached from `build_node_menu` for a Group node."

`build_tab_context_menu`: same signature change (`id: String, pos: egui::Pos2, fade: f32`), delete the `let current = …` and `fade_popup_state` let-else. Keep the `let Some((pinned, color, group)) = … else { … }` extraction, with its `self.tab_context_menu = None;` → `self.node_menu = None;`. Final `self.tab_context_menu = None;` → `self.node_menu = None;`. Its doc comment's "See `App::build_group_context_menu` for the identical pattern this mirrors" stays true.

If either body uses `id` in a way that needs `&str` vs `String` differently than before, adjust at the use sites only (before, `id` was also an owned `String` from the destructured cache, so none should be needed).

- [ ] **Step 8: Callers of the old fields**

In `build_graph_ui`:
- Escape handler (`if ui.input(|i| i.key_pressed(egui::Key::Escape)) { self.selected_tabs.clear(); self.group_context_menu = None; … }`): `self.group_context_menu = None;` → `self.node_menu = None;` (correction 3: now closes any kind's menu). Update the comment above it if it says only the group menu is dismissed.
- Right-click dispatch: replace the whole chain from `if let Some(gid) = hit_group {` through the matching `else { … self.rect_select_start = Some(pos); }` with:

```rust
                if let Some(id) =
                    hit_group.or(hit_download).or(hit_note).or(hit_image).or(hit_pdf).or(hit_json).or(hit_tab)
                {
                    self.node_menu = Some((id, pos));
                    self.rect_select_start = None;
                } else {
                    self.node_menu = None;
                    self.rect_select_start = Some(pos);
                }
```

  (The `hit_*` chain above it already makes at most one `Some`, in priority order group → download → note → image → pdf → json → tab; the `.or()` chain keeps that order. Read the old arms first to confirm each did exactly "set own field, clear the other six, `rect_select_start = None`" — if any arm did more, keep that extra work.)
- Group enter and leave blocks: each run of seven `self.<kind>_context_menu = None;` → one `self.node_menu = None;`.
- The seven `self.build_<kind>_context_menu(ui);` calls at the end of `build_graph_ui` → one `self.build_node_menu(ui);` in the same position (after the Recently Closed popup, so z-order is unchanged).

`DebugCommand::ExportGroup`: replace `if let Some((group_id, _)) = self.group_context_menu.clone() {` with:

```rust
                if let Some((group_id, _)) = self
                    .node_menu
                    .clone()
                    .filter(|(id, _)| self.nodes.get(id).is_some_and(|node| node.as_group().is_some()))
                {
```

Verify: `grep -n "_context_menu\b\|_context_menu_cache\|_context_menu(ui)\|build_download_context_menu\|build_note_context_menu\|build_image_context_menu\|build_pdf_context_menu\|build_json_context_menu" crates/manifold/src/main.rs` → only the Area-id string literals (`"group_context_menu"`, `"tab_context_menu"`, and the five inside `node_menu_config`) and the two remaining fn names `build_group_context_menu`/`build_tab_context_menu`. Also grep comments across the file (including README-style doc comments) for the removed field names and reword them to `node_menu`. The Tab-view link menu `App::context_menu` / `context_menu_cache` is a different menu — do not touch it.

- [ ] **Step 9: Build and test**

Run: `cargo build -p manifold` → zero warnings. `cargo test -p manifold` → 14 pass.

- [ ] **Step 10: Live check**

Live (`target/verify/task2/`), fresh APPDATA, same setup and card coordinates as Task 1 Step 0. For each of Note, Image, Pdf, Json, Group, Tab: right-click, wait ~300ms, `SCREENSHOT target/verify/task2/menu-<kind>.png`; compare with `target/verify/baseline/menu-<kind>.png` (same items, labels, order, width, position — pixel-diff with Python; differences only from unrelated live content such as page thumbnails are acceptable, explain any). Then: right-click note → right-click image (menu switches to the image menu, one menu only); `ESCAPE` → menu gone (any kind); right-click a card then click empty canvas → menu fades out; right-click empty canvas → no menu, marquee starts. Menu actions: note Copy Text; image Remove from Group on a grouped image (goes to top level); json Expand as Graph; tab menu Close closes the tab (Recently Closed +1); group menu Rename field still works; with the group menu open, `EXPORTGROUP target/verify/task2/group.json` writes the file, and with a note menu open it writes nothing.

- [ ] **Step 11: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "One node_menu state and build_node_menu dispatcher

Seven per-kind context-menu state pairs and builders collapse into one
field and a dispatch on ItemNodeKind; Download/Note/Image/Pdf/Json share
node_menu_config + apply_node_menu_action. Escape now closes any node menu.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Docs and final verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-23-node-menu-and-close-design.md`
- Modify: `README.md`

- [ ] **Step 1: Spec**

Add a "## Plan-time corrections" section right after "## Behavior change (decided)", copying the five numbered corrections from this plan's "Plan-time corrections" verbatim, prefixed with: "Found while writing the implementation plan; these supersede the sections below where they conflict." Add a "## Follow-ups" section at the end listing:
- Menu "Remove from Group" sends a node to the top level, while the card's hover eject sends it one level up — inconsistent for nested groups (pre-existing).
- A Download's menu never offers "Remove from Group" (pre-existing).
- The graph card rendering loops are still per kind (the next generalization of this pattern).

- [ ] **Step 2: README**

Find the README's "Recently Closed" nodes entry (grep `ClosedNode\|Recently Closed` in README.md) and add, in the same voice as neighboring entries, a short paragraph: "**One close path and one node menu**: every node card's hover ✕ and context-menu Delete now go through the same `close_node`, so hover-closing a Note/Image/Pdf/Json also lands in Recently Closed (before, only menu Delete did). The seven per-kind right-click menus share one `node_menu` state and one `build_node_menu` dispatcher; Escape now dismisses any of them (before, only the group menu). Verified live: …" — fill "…" with what Task 1 Step 9 and Task 2 Step 10 actually observed. If the README has a sentence claiming hover ✕ does not record, or naming `group_context_menu`/`download_context_menu`/`tab_context_menu` as current fields, reword it. (README entries are history; leave older entries that describe past states as history.)

- [ ] **Step 3: Final checks**

Run: `cargo build -p manifold` → zero warnings. `cargo test -p manifold` → 14 pass. Then, APPDATA isolated as in Global Constraints, `bash scripts/headless-smoke-test.sh` → exits 0.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-09-23-node-menu-and-close-design.md README.md
git commit -m "Record node-menu plan-time corrections and follow-ups

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
