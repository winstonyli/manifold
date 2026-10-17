---
status: done
---

# Unified Toolbar Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the Graph and Tab toolbars one shared composition model (universal chip, item-view-shared chip, per-kind content) and one shared visual system (segmented chips), replacing their current independently-styled flat button rows.

**Architecture:** Extract a generic `toolbar_chip` visual helper from the `toolbar-chip-wip` prototype (already proven live via pixel measurement); introduce a cheap `ItemNodeKind` classifier + `App::render_item_toolbar_content` dispatch mirroring the existing `close_node`/`MemberKind` pattern; move the Tab toolbar's existing content into `App::render_tab_toolbar_content` behind that dispatch, unchanged in behavior; extract two new shared functions (`render_universal_chip`, `render_return_to_graph`) so both toolbars compose from the same pieces.

**Tech Stack:** Rust, egui 0.36.2, egui_phosphor icon font. Single crate (`manifold`), single file (`crates/manifold/src/main.rs`) — no new files, matching this codebase's existing convention.

**Spec:** `docs/superpowers/specs/2026-09-23-toolbar-architecture-design.md`

## Global Constraints

- No new crate dependencies. Everything needed (egui, egui_phosphor) is already in `Cargo.toml`.
- `cargo build -p manifold` must stay warning-free after every task (this codebase's established bar — every prior toolbar change in this branch's history was verified this way).
- Every UI-visible task is verified live via the isolated-`%APPDATA%` headless-screenshot technique already used throughout this branch's history, not just by reading the diff. Never touch the real user's `%APPDATA%\manifold` session/settings files.
- Preserve every existing doc comment on code that moves without a behavior change; only add new doc comments where this plan introduces genuinely new logic.
- `main.rs` stays a single file — this codebase's own established convention (confirmed: no per-kind modules exist today). Do not split it as part of this plan.
- The chip fill color is `TOOLBAR_FILL.gamma_multiply(1.6)` (matches the already-approved, already-verified `toolbar-chip-wip` prototype); chip corner radius is `6.0`.

---

## How to verify a toolbar change live (used by every task below)

Each UI-visible task's verification step uses this exact sequence — an isolated `%APPDATA%` so the real user's session/settings are never touched, a headless screenshot, and pixel-column sampling to confirm alignment. Substitute the task's specific screenshot filename and pixel checks.

```bash
rm -rf /tmp/claude/manifold_toolbar_verify_TASKNAME
mkdir -p /tmp/claude/manifold_toolbar_verify_TASKNAME/AppData
```

```powershell
$exe = "C:\Users\Winston Li\Documents\GitHub\manifold\target\debug\manifold.exe"
$appData = "C:\Users\WINSTO~1\AppData\Local\Temp\claude\manifold_toolbar_verify_TASKNAME\AppData"
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $exe
$psi.Arguments = "--headless"
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.EnvironmentVariables["APPDATA"] = $appData
$proc = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds 2
$proc.StandardInput.WriteLine("SCREENSHOT C:\Users\WINSTO~1\AppData\Local\Temp\claude\manifold_toolbar_verify_TASKNAME\shot.png")
Start-Sleep -Seconds 2
$proc.StandardInput.WriteLine("QUIT")
Start-Sleep -Milliseconds 500
try { $proc.Kill() } catch {}
```

A fresh `%APPDATA%` boots directly into Graph view with a demo "Try grouping" group already present at the top level (seeded by `App::new`'s first-run demo content, `main.rs:4041-4054`) — this is what makes the Graph toolbar's Groups chip (Import Group is hidden in headless mode, but Ungroup All / Collapse Groups show because `groups_at_level` is non-empty) visible without any extra setup.

To reach Tab view for Tab-toolbar checks, send `DEBUGCOMMAND` sequences the existing headless protocol already supports (e.g. click a tab card's coordinates, or use `GRAPHOPENFOCUSED` after `GRAPHFOCUSDOWN`/`GRAPHFOCUSRIGHT` to focus a tab node, then open it) — do not add a new debug command for this; the protocol already has everything needed to focus and open a tab.

Pixel sampling (adjust `x` columns and `y` range per task):

```python
from PIL import Image
im = Image.open('shot.png').convert('RGB')
px = im.load()
def edges(x):
    top=None; bottom=None
    for y in range(0,40):
        r,g,b = px[x,y]
        if (r,g,b) != (29,26,20) and (r,g,b) != (60,60,60):
            if top is None: top = y
            bottom = y
    return top, bottom
```

---

### Task 1: `toolbar_chip` helper + apply to the Graph toolbar's Groups cluster

**Files:**
- Modify: `crates/manifold/src/main.rs` — add `chip_icon_button` and `toolbar_chip` near `icon_button_eased` (`main.rs:1328-1363`); modify the Graph toolbar's Groups cluster (`main.rs:9217-9240`).

**Interfaces:**
- Produces: `fn chip_icon_button(ui: &mut egui::Ui, id: egui::Id, icon: impl Into<String>) -> egui::Response` — a borderless, transparent-idle icon button for use inside a chip. `fn toolbar_chip(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> egui::Rect) -> egui::Rect` — paints a raised background behind `content`, returns the chip's own bounding rect. Both consumed by every later task that adds a chip.

- [ ] **Step 1: Add `chip_icon_button`**

Insert directly after `icon_button_eased` (`main.rs:1363`), i.e. right before the blank line preceding whatever function currently follows it:

```rust
/// Same hover-eased feedback as `icon_button_eased`, but with no visible
/// border ever (idle or hovered) and a transparent idle fill -- for icons
/// already sitting inside a shared background chip (see `toolbar_chip`),
/// where `icon_button_eased`'s own always-drawn box reads as a border
/// nested inside the chip's own border. Deliberately still a real,
/// fully-framed `egui::Button` (not `.frame(false)`, which shrinks the
/// widget's own allocated size well below a normal button's and was
/// found live to make a chip noticeably shorter than its "+ New Tab"/
/// "Reset View" neighbors) -- keeping the frame but making it invisible
/// at rest matches every sibling button's sizing exactly while still
/// hiding the box until the icon is hovered, at which point a soft fill
/// (no stroke) confirms the hit target.
fn chip_icon_button(ui: &mut egui::Ui, id: egui::Id, icon: impl Into<String>) -> egui::Response {
    let was_hovered: bool = ui.ctx().memory(|m| m.data.get_temp(id).unwrap_or(false));
    let hover_t = ui.ctx().animate_bool_responsive(id, was_hovered);
    let bg = lerp_color32(egui::Color32::TRANSPARENT, ui.visuals().widgets.hovered.weak_bg_fill, hover_t);
    let fg = lerp_color32(
        ui.visuals().widgets.inactive.fg_stroke.color,
        ui.visuals().widgets.hovered.fg_stroke.color,
        hover_t,
    );
    let response = ui.add(
        egui::Button::new(egui::RichText::new(icon.into()).color(fg))
            .fill(bg)
            .stroke(egui::Stroke::NONE),
    );
    ui.ctx().memory_mut(|m| m.data.insert_temp(id, response.hovered()));
    response
}

/// Paints a raised chip background behind `content` -- e.g. a cluster of
/// `chip_icon_button`s that should read as one grouped control instead of
/// a flat row of buttons. Uses egui's "reserve then backfill" idiom
/// (`Shape::Noop` now, `Painter::set` once the real size is known) since
/// the chip's own size depends on content not yet laid out.
///
/// `content` MUST run directly against the `ui` it's given -- never
/// inside its own nested `ui.horizontal`/`ui.new_child`. A nested Ui
/// becomes an independently-sized layout block that the OUTER row then
/// centers (or top-aligns) using its own possibly-smaller reported
/// height, which is exactly what caused a confirmed-live ~5px vertical
/// misalignment in the `toolbar-chip-wip` prototype: the outer row
/// centered the nested block within its own taller row height, and no
/// amount of forcing the nested block's *height* fixed it, because the
/// bug was the block's *existence*, not its size. `content` must instead
/// call the SAME `ui` for every widget (making them direct siblings of
/// whatever sits before/after the chip in the row) and return the union
/// of its own widgets' rects -- exact by construction, since each
/// widget's own returned `Response::rect` is real.
fn toolbar_chip(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> egui::Rect) -> egui::Rect {
    let chip_bg_slot = ui.painter().add(egui::Shape::Noop);
    let (lead_rect, _) =
        ui.allocate_exact_size(egui::Vec2::new(5.0, 0.0), egui::Sense::hover());
    let content_rect = content(ui);
    let (trail_rect, _) =
        ui.allocate_exact_size(egui::Vec2::new(5.0, 0.0), egui::Sense::hover());
    let rect = lead_rect.union(content_rect).union(trail_rect);
    ui.painter().set(chip_bg_slot, egui::Shape::rect_filled(rect, 6.0, TOOLBAR_FILL.gamma_multiply(1.6)));
    rect
}
```

- [ ] **Step 2: Build to confirm the new helpers compile (unused is fine — Rust won't warn on unused private fns called nowhere yet, but they ARE used starting this same task's Step 3)**

Run: `cargo build -p manifold`
Expected: builds clean, zero warnings.

- [ ] **Step 3: Apply `toolbar_chip` to the Graph toolbar's Groups cluster**

Replace the block at `main.rs:9206-9240` (Import Group / Ungroup All / Collapse-Expand, currently three independent `icon_button_eased` calls with no visual grouping) with:

```rust
                    if !self.headless || !groups_at_level.is_empty() {
                        toolbar_chip(ui, |ui| {
                            let mut bounds: Option<egui::Rect> = None;
                            let mut grow = |r: egui::Rect| {
                                bounds = Some(bounds.map_or(r, |b| b.union(r)));
                            };
                            if !self.headless {
                                let response = chip_icon_button(
                                    ui,
                                    egui::Id::new("icon_btn_import_group"),
                                    egui_phosphor::light::UPLOAD_SIMPLE,
                                );
                                grow(response.rect);
                                if response.on_hover_text("Import Group...").clicked() {
                                    import_group_requested = true;
                                }
                            }
                            if !groups_at_level.is_empty() {
                                let response = chip_icon_button(
                                    ui,
                                    egui::Id::new("icon_btn_ungroup_all"),
                                    egui_phosphor::light::FOLDER_MINUS,
                                );
                                grow(response.rect);
                                if response.on_hover_text("Ungroup All").clicked() {
                                    ungroup_all_requested = true;
                                }
                            }
                            if !groups_at_level.is_empty() {
                                let (icon, hover) = if self.groups_collapsed {
                                    (egui_phosphor::light::CORNERS_OUT, "Expand Groups")
                                } else {
                                    (egui_phosphor::light::CORNERS_IN, "Collapse Groups")
                                };
                                let response = chip_icon_button(ui, egui::Id::new("icon_btn_collapse_groups"), icon);
                                grow(response.rect);
                                if response.on_hover_text(hover).clicked() {
                                    self.groups_collapsed = !self.groups_collapsed;
                                }
                            }
                            bounds.unwrap_or(ui.cursor())
                        });
                    }
```

This is a direct behavior-preserving swap: same three conditions, same flags set (`import_group_requested`, `ungroup_all_requested`, `self.groups_collapsed`), same hover tooltips — only the visual container and the `icon_button_eased` → `chip_icon_button` swap (needed since `icon_button_eased` always draws its own border, which would nest inside the chip's border) are new. The outer `if !self.headless || !groups_at_level.is_empty()` guard is unchanged from today's implicit behavior (today each icon button independently guards itself; wrapping requires one shared guard so the chip doesn't paint an empty background — headless mode with no groups skips the whole chip, matching today's "nothing renders" outcome exactly).

- [ ] **Step 4: Build**

Run: `cargo build -p manifold`
Expected: builds clean, zero warnings.

- [ ] **Step 5: Verify live**

Follow "How to verify a toolbar change live" above (fresh `%APPDATA%`, which seeds the demo "Try grouping" group, so `groups_at_level` is non-empty and the chip renders with Ungroup All + Collapse Groups). Screenshot the toolbar row, crop/upscale the region around the chip, and sample pixel columns across "+ New Tab", the chip, and "Reset View" the same way the `toolbar-chip-wip` prototype was verified (top/bottom edges within 1-2px across all three, not the ~5px systematic gap a nested-Ui bug would produce).

- [ ] **Step 6: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "$(cat <<'EOF'
Add reusable toolbar_chip helper, apply to Graph toolbar's Groups cluster

Extracts the reserve-then-backfill chip visual (with the direct-sibling-
not-nested-Ui discipline that fixed a confirmed vertical-alignment bug
in the earlier prototype) into a generic helper, and uses it for the
first of several planned chip groupings across both toolbars.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `ItemNodeKind` classifier + unit test

**Files:**
- Modify: `crates/manifold/src/main.rs` — add near `NodePayload`'s own `impl` block (`main.rs:1552-1562` and following `as_*` methods).

**Interfaces:**
- Consumes: `NodePayload` enum (`main.rs:1552-1560`, variants `Tab`, `Download`, `Group`, `Note`, `Image`, `Pdf`, `Json`).
- Produces: `enum ItemNodeKind { Tab, Download, Group, Note, Image, Pdf, Json }` and `fn item_node_kind(payload: &NodePayload) -> ItemNodeKind`, consumed by Task 4's `App::render_item_toolbar_content`.

- [ ] **Step 1: Add the classifier**

Insert after `NodePayload`'s closing `}` of its `impl` block (find the line after the last `as_*`/`as_*_mut` method inside `impl NodePayload { ... }`, which starts at `main.rs:1562`):

```rust
/// A cheap, owned, `Copy` classifier for `&NodePayload` -- computed once
/// up front so its immutable borrow of `self.nodes` ends immediately,
/// before a dispatcher needs `&mut self` to render that kind's toolbar
/// content. Mirrors `close_node`'s existing `MemberKind` match
/// (`main.rs:7375`) for exactly the same reason: matching directly on
/// `&NodePayload` would keep that borrow alive for as long as the match
/// arms run, which conflicts with the `&mut self` those arms need.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemNodeKind {
    Tab,
    Download,
    Group,
    Note,
    Image,
    Pdf,
    Json,
}

fn item_node_kind(payload: &NodePayload) -> ItemNodeKind {
    match payload {
        NodePayload::Tab(_) => ItemNodeKind::Tab,
        NodePayload::Download(_) => ItemNodeKind::Download,
        NodePayload::Group(_) => ItemNodeKind::Group,
        NodePayload::Note(_) => ItemNodeKind::Note,
        NodePayload::Image(_) => ItemNodeKind::Image,
        NodePayload::Pdf(_) => ItemNodeKind::Pdf,
        NodePayload::Json(_) => ItemNodeKind::Json,
    }
}

#[cfg(test)]
mod item_node_kind_tests {
    use super::*;

    #[test]
    fn maps_each_payload_variant_to_its_own_kind() {
        assert_eq!(item_node_kind(&NodePayload::Tab(TabPayload::test_default())), ItemNodeKind::Tab);
        assert_eq!(item_node_kind(&NodePayload::Download(DownloadPayload::test_default())), ItemNodeKind::Download);
        assert_eq!(item_node_kind(&NodePayload::Group(GroupPayload { name: "g".into() })), ItemNodeKind::Group);
        assert_eq!(item_node_kind(&NodePayload::Note(NotePayload { text: "n".into(), editing: false })), ItemNodeKind::Note);
    }
}
```

**Note for the implementer:** `TabPayload::test_default()` and `DownloadPayload::test_default()` are placeholders for whatever minimal-construction pattern this codebase already uses elsewhere for these two structs in tests, or for their real constructors if simple enough to call directly (their exact field lists weren't part of this plan's investigation). Before writing this step, run:

```bash
grep -n "struct TabPayload" -A 20 crates/manifold/src/main.rs
grep -n "struct DownloadPayload" -A 10 crates/manifold/src/main.rs
```

and either construct each directly with real field values (most fields are likely simple: a URL string, booleans, etc. — `TabPayload` also embeds a live `tab: EngineHandle`-style connection per other code read during planning, e.g. `node.tab.send(...)` at `main.rs:8827`, which may not be constructible outside a real spawn path) or, if `Tab`/`Download` can't be cheaply constructed in a unit test, drop those two assertions and keep the test to `Group`/`Note`/`Image`/`Pdf`/`Json` (five of seven variants is still a real regression guard, and `Tab`'s mapping is exercised end-to-end by every other task's live verification anyway).

- [ ] **Step 2: Run the test**

Run: `cargo test -p manifold item_node_kind`
Expected: PASS (after resolving the constructibility note above; adjust the test body first if `Tab`/`Download` can't be constructed directly, per Step 1's note, then re-run).

- [ ] **Step 3: Build**

Run: `cargo build -p manifold`
Expected: builds clean, zero warnings. (`ItemNodeKind`/`item_node_kind` are unused outside the test until Task 4 — confirm this doesn't trigger `dead_code`; if it does, that's expected and resolves itself once Task 4 wires them in. Do not add `#[allow(dead_code)]` to silence it — leaving a real warning as a visible signal that Task 4 isn't done yet is preferable to suppressing it.)

- [ ] **Step 4: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "$(cat <<'EOF'
Add ItemNodeKind classifier for per-kind toolbar dispatch

A cheap Copy classifier computed from &NodePayload before dispatching
to a &mut self per-kind render method, mirroring close_node's existing
MemberKind pattern (main.rs:7375) so the immutable borrow of self.nodes
ends before the dispatched method needs &mut self.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Extract `render_return_to_graph` from the Tab toolbar

**Files:**
- Modify: `crates/manifold/src/main.rs:8448-8450` (inside `build_tab_ui`).

**Interfaces:**
- Produces: `fn render_return_to_graph(ui: &mut egui::Ui) -> bool` — returns `true` when clicked. Consumed by `build_tab_ui` (this task) and available for any future non-Tab item view (per the spec's "item-view shared chip" layer).

- [ ] **Step 1: Add the function**

Insert near `toolbar_panel` (`main.rs:941-950`), directly after it:

```rust
/// The "back to Graph" control every non-Graph item view shares (today,
/// only Tab view; see the design spec's "item-view shared chip" layer).
/// Deliberately not chip-wrapped, unlike the icon clusters elsewhere in
/// this file -- it sits immediately before those clusters in the Tab
/// toolbar's own row, and merging two different composition layers (this
/// shared control and Tab's own per-kind nav icons) into one visual chip
/// would require either layer to know about the other's rendering, which
/// the composition model deliberately keeps separate. A plain button
/// here reads as a clearly distinct action from the icon cluster next to
/// it, which is an acceptable, honest reading of the approved mockup
/// rather than a literal pixel reproduction of it.
fn render_return_to_graph(ui: &mut egui::Ui) -> bool {
    ui.button("Graph").clicked()
}
```

- [ ] **Step 2: Replace the call site**

In `build_tab_ui`, replace:

```rust
                    if ui.button("Graph").clicked() {
                        back_to_graph = true;
                    }
```

(`main.rs:8448-8450`) with:

```rust
                    if render_return_to_graph(ui) {
                        back_to_graph = true;
                    }
```

- [ ] **Step 3: Build**

Run: `cargo build -p manifold`
Expected: builds clean, zero warnings.

- [ ] **Step 4: Verify live**

Follow "How to verify a toolbar change live." Enter Tab view (see the note in that section on reaching Tab view headlessly), screenshot, confirm the "Graph" button still renders in its original position. Then send the debug command sequence that clicks it (or the existing `Escape`-to-leave-Tab-view path, which this same code also feeds via `back_to_graph`) and screenshot again to confirm it still returns to Graph view.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "$(cat <<'EOF'
Extract render_return_to_graph from the Tab toolbar

Pulls the "back to Graph" button into its own function, matching the
design spec's item-view shared chip layer, so a future non-Tab item
view gets it without duplicating the button.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `ItemToolbarActions` + `render_tab_toolbar_content` + dispatch

This is the largest task: moving `build_tab_ui`'s existing per-kind content into its own function behind the `ItemNodeKind` dispatch, with no behavior change. Read this task fully before starting — it's one atomic move, not several independent edits.

**Files:**
- Modify: `crates/manifold/src/main.rs` — `build_tab_ui` (`main.rs:8361-8954`).

**Interfaces:**
- Consumes: `ItemNodeKind`, `item_node_kind` (Task 2).
- Produces: `struct ItemToolbarActions { ... }`, `impl App { fn render_item_toolbar_content(&mut self, focused_id: &str, ui: &mut egui::Ui) -> ItemToolbarActions }`, `impl App { fn render_tab_toolbar_content(&mut self, focused_id: &str, ui: &mut egui::Ui) -> ItemToolbarActions }`. Consumed by Task 5 (which further edits `render_tab_toolbar_content`'s trailing-cluster portion) and Task 6 (which wraps its nav portion in a chip).

- [ ] **Step 1: Add `ItemToolbarActions`**

Insert next to `render_return_to_graph` (added in Task 3):

```rust
/// Everything the Tab toolbar's own content (see `render_tab_toolbar_
/// content`) needs to report back to `build_tab_ui`, which applies these
/// exactly where the equivalent bare `let mut` locals were applied
/// before this struct existed -- this is a pure "package the same flags
/// as a return value" move, not a behavior change. `navigate` is
/// deliberately NOT a field here: today it's computed by `build_tab_ui`
/// itself, after this content returns, from `enter_pressed` plus the
/// suggestion list (`main.rs:8781-8786`) -- only `enter_pressed` needs
/// to cross the function boundary.
#[derive(Default)]
struct ItemToolbarActions {
    go_back: bool,
    go_forward: bool,
    reload_requested: bool,
    home_requested: bool,
    enter_pressed: bool,
    toggle_bookmark_requested: bool,
    close_split_requested: bool,
    tab_actions_button_rect: Option<egui::Rect>,
    split_engine_button_rect: Option<egui::Rect>,
    address_bar_rect: Option<egui::Rect>,
    shortcuts_help_button_rect: Option<egui::Rect>,
}
```

- [ ] **Step 2: Add the dispatcher**

Insert directly after `ItemToolbarActions`, as an `impl App` block (or add these two methods to an existing `impl App` block if one already immediately follows — check `main.rs` around where `build_tab_ui` itself is defined, since these methods belong in the same `impl App { ... }` as `build_tab_ui`, not a new separate `impl` block):

```rust
    fn render_item_toolbar_content(&mut self, focused_id: &str, ui: &mut egui::Ui) -> ItemToolbarActions {
        let kind = match self.nodes.get(focused_id) {
            Some(node) => item_node_kind(&node.payload),
            None => return ItemToolbarActions::default(),
        };
        match kind {
            ItemNodeKind::Tab => self.render_tab_toolbar_content(focused_id, ui),
            ItemNodeKind::Download
            | ItemNodeKind::Group
            | ItemNodeKind::Note
            | ItemNodeKind::Image
            | ItemNodeKind::Pdf
            | ItemNodeKind::Json => ItemToolbarActions::default(),
        }
    }
```

- [ ] **Step 3: Move the Tab content into `render_tab_toolbar_content`**

Cut lines `main.rs:8451-8680` (from `let node = self.nodes[&focused_id].as_tab().unwrap();` through the closing `});` of the `if editing_address` autocomplete... no — stop at the end of the `ui.with_layout(...)` closure body, i.e. everything from `let node = ...` at 8451 through `enter_pressed = true;` and its closing `}` at 8680, NOT including the closure's own closing `});` at 8681-8682 which belongs to `toolbar_panel`/`with_layout` scaffolding that stays in `build_tab_ui`) into a new method:

```rust
    fn render_tab_toolbar_content(&mut self, focused_id: &str, ui: &mut egui::Ui) -> ItemToolbarActions {
        let mut actions = ItemToolbarActions::default();
        let node = self.nodes[focused_id].as_tab().unwrap();
        // ... (main.rs:8452-8528 unchanged verbatim: back/forward/reload/
        // home buttons and the identity slot, EXCEPT each `go_back = true`
        // becomes `actions.go_back = true`, `go_forward = true` becomes
        // `actions.go_forward = true`, `reload_requested = true` becomes
        // `actions.reload_requested = true`, `home_requested = true`
        // becomes `actions.home_requested = true`) ...

        let remaining_rect = ui.available_rect_before_wrap();
        let mut trailing_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(remaining_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Min)),
        );
        self.build_settings_button(&mut trailing_ui);
        let shortcuts_help_rect = self.build_help_button(&mut trailing_ui);
        let tab_actions_response = trailing_ui.button("Tab actions");
        if tab_actions_response.clicked() {
            self.tab_actions_open = !self.tab_actions_open;
        }
        actions.tab_actions_button_rect = Some(tab_actions_response.rect);
        if self.split_tab.is_some()
            && icon_button_eased(&mut trailing_ui, egui::Id::new("icon_btn_close_split"), egui_phosphor::light::X)
                .on_hover_text("Close Split")
                .clicked()
        {
            actions.close_split_requested = true;
        }
        let split_engine_response =
            trailing_ui.button(if self.split_tab.is_some() { "Split ▾" } else { "Split" });
        if split_engine_response.clicked() {
            self.split_engine_picker_open = !self.split_engine_picker_open;
        }
        actions.split_engine_button_rect = Some(split_engine_response.rect);
        if self
            .nodes
            .get(focused_id)
            .and_then(|n| n.as_tab())
            .is_some_and(|n| n.json_body.is_some())
            && trailing_ui.button(egui_phosphor::light::BRACKETS_CURLY).on_hover_text("View as JSON").clicked()
        {
            self.json_viewer_node = Some(focused_id.to_string());
        }
        let is_bookmarked = self
            .nodes
            .get(focused_id)
            .and_then(|n| n.as_tab())
            .is_some_and(|n| self.bookmarks.iter().any(|b| b.url == n.url));
        if trailing_ui
            .selectable_label(is_bookmarked, egui_phosphor::light::BOOKMARK_SIMPLE)
            .on_hover_text("Bookmark")
            .clicked()
        {
            actions.toggle_bookmark_requested = true;
        }
        let trailing_width = trailing_ui.min_rect().width();
        actions.shortcuts_help_button_rect = Some(shortcuts_help_rect);

        let text_width = (remaining_rect.width() - trailing_width).max(40.0);
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.address_bar_text)
                .desired_width(text_width)
                .margin(egui::Margin::symmetric(8, 4)),
        );
        if std::mem::take(&mut self.pending_focus_address_bar) {
            response.request_focus();
        }
        self.address_bar_focused = response.has_focus();
        actions.address_bar_rect = Some(response.rect);
        ui.advance_cursor_after_rect(trailing_ui.min_rect());

        if response.changed() {
            self.open_tab_suggestions = self.compute_open_tab_suggestions(&self.address_bar_text, focused_id);
            self.suggestion_fetcher.note_text_changed(&self.address_bar_text);
            self.dropdown_highlighted = None;
        }
        if std::mem::take(&mut self.pending_debug_enter)
            || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            actions.enter_pressed = true;
        }
        actions
    }
```

The elided middle section (back/forward/reload/home/identity, `main.rs:8452-8528`) must be copied verbatim from the current file with exactly the four flag-name substitutions named in the comment above — open `main.rs` at those line numbers and copy them directly rather than retyping from memory, since this section includes exact doc comments (the CARET_LEFT/CARET_RIGHT migration note, the favicon-centering note) that must carry over unchanged.

**Note on `let node = self.nodes[focused_id].as_tab().unwrap();`:** this line's borrow of `self.nodes` is used for `node.can_go_back`/`node.can_go_forward`/`node.loading`/`node.url` in the copied middle section, and (per Rust's non-lexical lifetimes) that borrow ends at `node`'s last use, before this function's later lines take `&mut self` for `self.build_settings_button(...)`, `trailing_ui` construction, etc. This is the exact same borrow pattern the original inline code already relied on — moving it into its own function changes nothing about it.

- [ ] **Step 4: Update `build_tab_ui`'s call site**

Replace the `ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| { ... })` closure body (`main.rs:8447-8681`, now that its content has moved out in Step 3) with:

```rust
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    if render_return_to_graph(ui) {
                        back_to_graph = true;
                    }
                    let item_actions = self.render_item_toolbar_content(&focused_id, ui);
                    go_back = item_actions.go_back;
                    go_forward = item_actions.go_forward;
                    reload_requested = item_actions.reload_requested;
                    home_requested = item_actions.home_requested;
                    enter_pressed = item_actions.enter_pressed;
                    toggle_bookmark_requested = item_actions.toggle_bookmark_requested;
                    close_split_requested = item_actions.close_split_requested;
                    tab_actions_button_rect = item_actions.tab_actions_button_rect;
                    split_engine_button_rect = item_actions.split_engine_button_rect;
                    address_bar_rect = item_actions.address_bar_rect;
                    shortcuts_help_button_rect = item_actions.shortcuts_help_button_rect;
                });
```

The `let mut` declarations for all of these locals already exist earlier in `build_tab_ui` (`main.rs:8411-8422`) and stay exactly as they are — everything below this closure (escape handling, the suggestion dropdown `Area`, `if go_back { ... }`, etc., `main.rs:8684-8953`) already reads these same local names and needs no further changes.

- [ ] **Step 5: Build**

Run: `cargo build -p manifold`
Expected: builds clean, zero warnings. If a "moved value" or borrow error appears around `focused_id` (now passed as `&focused_id` into a `&mut self` method while `self.nodes[&focused_id]` is used elsewhere in the same closure), resolve it the same way the codebase already does elsewhere: pass `focused_id: &str` (a borrow of the existing `String`), not an owned clone, and confirm no other live borrow of `self` overlaps the `self.render_item_toolbar_content(&focused_id, ui)` call.

- [ ] **Step 6: Verify live**

Follow "How to verify a toolbar change live." This task is a pure refactor with zero intended visual change — enter Tab view, screenshot, and confirm the toolbar looks pixel-identical to before this task (nav buttons, identity slot, address bar, trailing cluster all in their original positions with no chip styling yet — that's Tasks 5-6). Additionally smoke-test behavior: type into the address bar and confirm suggestions still appear (proves `render_tab_toolbar_content`'s `response.changed()` branch still fires), and confirm back/forward buttons are still enabled/disabled correctly based on navigation history.

- [ ] **Step 7: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "$(cat <<'EOF'
Extract Tab toolbar content into render_tab_toolbar_content

Moves build_tab_ui's per-kind content behind the ItemNodeKind dispatch
added in the previous task, returning an ItemToolbarActions struct
instead of leaving bare locals set by a closure. Pure refactor, no
behavior change -- verified live against the pre-refactor screenshot.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `render_universal_chip` (Help + Settings), wired into both toolbars

**Files:**
- Modify: `crates/manifold/src/main.rs` — `render_tab_toolbar_content` (edited in Task 4), Graph toolbar (`main.rs:9319-9320`).

**Interfaces:**
- Consumes: `toolbar_chip`, `chip_icon_button` (Task 1); `build_help_button`, `build_settings_button` (existing, `main.rs:8336`, `main.rs:8352`).
- Produces: `impl App { fn render_universal_chip(&mut self, ui: &mut egui::Ui) -> egui::Rect }`, returning the chip's rect (so `render_tab_toolbar_content` can still capture `shortcuts_help_button_rect` — `build_help_button` itself already returns the rect popups anchor to, so this wrapper just needs to preserve that return value, not the chip's own outer rect, for that specific field; see Step 2).

- [ ] **Step 1: Make `build_settings_button` return its rect**

`build_settings_button` (`main.rs:8352-8359`) already computes a `Response` internally but discards its rect, unlike its sibling `build_help_button` (`main.rs:8336-8347`), which already returns one. Change it to match:

```rust
    fn build_settings_button(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let response = icon_button_eased(ui, egui::Id::new("icon_btn_settings_gear"), egui_phosphor::light::GEAR)
            .on_hover_text("Settings");
        if response.clicked() {
            self.toggle_settings_page();
        }
        response.rect
    }
```

This changes its signature, so its two existing call sites (`main.rs:8569` inside the Tab toolbar's trailing cluster, and `main.rs:9320` inside the Graph toolbar) will now produce an unused-return-value situation once Step 2/3 below route through `render_universal_chip` instead — but check both call sites now: if either is still calling `build_settings_button` directly elsewhere after this task's edits, it simply ignores the return value (Rust allows discarding a returned value without `let _ =`), so no other code needs to change for this signature change alone.

- [ ] **Step 2: Add `render_universal_chip`**

Insert next to `render_return_to_graph`:

```rust
    /// Help + Settings, present in every toolbar (see the design spec's
    /// "universal chip" layer). Returns the same rect `build_help_button`
    /// already returns -- the shortcuts-help popup anchors to that
    /// specific button, not to the chip as a whole, so callers that need
    /// it (the Tab toolbar's `shortcuts_help_button_rect`) keep working
    /// unchanged.
    fn render_universal_chip(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let mut help_rect = egui::Rect::NOTHING;
        toolbar_chip(ui, |ui| {
            let help = self.build_help_button(ui);
            help_rect = help;
            let settings = self.build_settings_button(ui);
            help.union(settings)
        });
        help_rect
    }
```

- [ ] **Step 3: Wire into the Tab toolbar**

In `render_tab_toolbar_content` (Task 4's Step 3 code), replace:

```rust
        self.build_settings_button(&mut trailing_ui);
        let shortcuts_help_rect = self.build_help_button(&mut trailing_ui);
```

with:

```rust
        let shortcuts_help_rect = self.render_universal_chip(&mut trailing_ui);
```

- [ ] **Step 4: Wire into the Graph toolbar**

Replace `main.rs:9319-9320`:

```rust
                    shortcuts_help_button_rect = Some(self.build_help_button(ui));
                    self.build_settings_button(ui);
```

with:

```rust
                    shortcuts_help_button_rect = Some(self.render_universal_chip(ui));
```

- [ ] **Step 5: Build**

Run: `cargo build -p manifold`
Expected: builds clean, zero warnings.

- [ ] **Step 6: Verify live**

Follow "How to verify a toolbar change live" for BOTH toolbars: Graph view screenshot (confirm Help+Settings now render inside a chip at the row's right edge) and Tab view screenshot (same check, inside the trailing cluster). Pixel-sample both chips' top/bottom edges against their respective row's other buttons, same technique as Task 1. Also confirm the shortcuts-help popup still opens correctly in both views (it's positioned from the rect this task's `render_universal_chip` returns) — trigger it via whatever debug command or click sequence the existing headless tests already use for it.

- [ ] **Step 7: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "$(cat <<'EOF'
Add render_universal_chip, wire into both toolbars

Help and Settings now render inside one shared chip in both the Graph
and Tab toolbars, replacing each toolbar's own independent calls to
build_help_button/build_settings_button.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Chip the Tab toolbar's nav cluster and Bookmark/View-JSON pair

**Files:**
- Modify: `crates/manifold/src/main.rs` — `render_tab_toolbar_content` (edited in Tasks 4-5).

**Interfaces:**
- Consumes: `toolbar_chip`, `chip_icon_button` (Task 1).
- Produces: no new interfaces — purely visual, within `render_tab_toolbar_content`.

- [ ] **Step 1: Wrap the nav cluster**

In `render_tab_toolbar_content`, replace the back/forward/reload/home block (copied from `main.rs:8458-8493` in Task 4's Step 3) with the same four buttons wrapped in `toolbar_chip`:

```rust
        toolbar_chip(ui, |ui| {
            let mut bounds: Option<egui::Rect> = None;
            let mut grow = |r: egui::Rect| {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            };
            let back = ui.add_enabled(node.can_go_back, egui::Button::new(egui_phosphor::light::CARET_LEFT));
            grow(back.rect);
            if back.clicked() {
                actions.go_back = true;
            }
            let forward = ui.add_enabled(node.can_go_forward, egui::Button::new(egui_phosphor::light::CARET_RIGHT));
            grow(forward.rect);
            if forward.clicked() {
                actions.go_forward = true;
            }
            let reload = icon_button_eased(ui, egui::Id::new("icon_btn_reload"), egui_phosphor::light::ARROW_CLOCKWISE)
                .on_hover_text("Reload");
            grow(reload.rect);
            if reload.clicked() {
                actions.reload_requested = true;
            }
            let home = icon_button_eased(ui, egui::Id::new("icon_btn_home"), egui_phosphor::light::HOUSE)
                .on_hover_text("Go to home page");
            grow(home.rect);
            if home.clicked() {
                actions.home_requested = true;
            }
            bounds.unwrap_or(ui.cursor())
        });
```

**Note for the implementer:** `icon_button_eased` (used here for Reload/Home) still draws its own always-visible border (per its own doc comment, `main.rs:1328-1346`), which will visually nest inside this chip's border exactly the way `chip_icon_button`'s own doc comment describes as the problem it exists to solve. Replace `icon_button_eased` with `chip_icon_button` for Reload and Home here too, for visual consistency with Task 1's Graph-toolbar chip:

```rust
            let reload = chip_icon_button(ui, egui::Id::new("icon_btn_reload"), egui_phosphor::light::ARROW_CLOCKWISE)
                .on_hover_text("Reload");
            ...
            let home = chip_icon_button(ui, egui::Id::new("icon_btn_home"), egui_phosphor::light::HOUSE)
                .on_hover_text("Go to home page");
```

Use `chip_icon_button` (not `icon_button_eased`) for these two in the actual edit — the block above already reflects this.

- [ ] **Step 2: Wrap Bookmark + View-as-JSON**

In the trailing cluster (Task 4's Step 3 code), replace the View-as-JSON `if` block and the Bookmark `if` block:

```rust
        if self
            .nodes
            .get(focused_id)
            .and_then(|n| n.as_tab())
            .is_some_and(|n| n.json_body.is_some())
            && trailing_ui.button(egui_phosphor::light::BRACKETS_CURLY).on_hover_text("View as JSON").clicked()
        {
            self.json_viewer_node = Some(focused_id.to_string());
        }
        let is_bookmarked = self
            .nodes
            .get(focused_id)
            .and_then(|n| n.as_tab())
            .is_some_and(|n| self.bookmarks.iter().any(|b| b.url == n.url));
        if trailing_ui
            .selectable_label(is_bookmarked, egui_phosphor::light::BOOKMARK_SIMPLE)
            .on_hover_text("Bookmark")
            .clicked()
        {
            actions.toggle_bookmark_requested = true;
        }
```

with:

```rust
        let has_json = self.nodes.get(focused_id).and_then(|n| n.as_tab()).is_some_and(|n| n.json_body.is_some());
        let is_bookmarked = self
            .nodes
            .get(focused_id)
            .and_then(|n| n.as_tab())
            .is_some_and(|n| self.bookmarks.iter().any(|b| b.url == n.url));
        toolbar_chip(&mut trailing_ui, |ui| {
            let mut bounds: Option<egui::Rect> = None;
            let mut grow = |r: egui::Rect| {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            };
            if has_json {
                let response = chip_icon_button(ui, egui::Id::new("icon_btn_view_json"), egui_phosphor::light::BRACKETS_CURLY)
                    .on_hover_text("View as JSON");
                grow(response.rect);
                if response.clicked() {
                    self.json_viewer_node = Some(focused_id.to_string());
                }
            }
            let bookmark = chip_icon_button(ui, egui::Id::new("icon_btn_bookmark"), egui_phosphor::light::BOOKMARK_SIMPLE)
                .on_hover_text("Bookmark");
            grow(bookmark.rect);
            if bookmark.clicked() {
                actions.toggle_bookmark_requested = true;
            }
            bounds.unwrap_or(ui.cursor())
        });
```

**Note for the implementer:** `chip_icon_button` doesn't support a "pressed"/selected visual state the way `selectable_label` did for the already-bookmarked case — check `is_bookmarked` and, if true, pass a visually-distinct icon (e.g. a filled bookmark glyph if `egui_phosphor::light` has one, such as `BOOKMARK_SIMPLE` vs. a `_FILL`/`_BOLD` variant — grep `egui_phosphor::light::BOOKMARK` for what's available) instead of trying to force `chip_icon_button` to support a persistent-pressed look it wasn't designed for. If no filled variant exists in the `light` icon set, keep `selectable_label` for just this one button instead of `chip_icon_button` (breaking chip-visual consistency for one icon is a smaller cost than losing the bookmarked-state affordance entirely) and note this deviation in the commit message.

- [ ] **Step 3: Build**

Run: `cargo build -p manifold`
Expected: builds clean, zero warnings.

- [ ] **Step 4: Verify live**

Follow "How to verify a toolbar change live" for Tab view. Screenshot, pixel-sample the nav chip and the Bookmark/JSON chip against "Tab actions"/"Split" (which stay plain buttons, per this task's scope) and the address bar. Confirm all four nav buttons and both trailing icons still function (back/forward navigate, reload reloads, home navigates home, bookmark toggles, JSON viewer opens when a tab has a JSON body).

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/main.rs
git commit -m "$(cat <<'EOF'
Chip the Tab toolbar's nav cluster and Bookmark/View-JSON pair

Applies the same toolbar_chip visual used by the Graph toolbar's Groups
cluster and the universal Help/Settings chip to Tab view's back/
forward/reload/home buttons and its Bookmark/View-as-JSON pair.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Final whole-branch verification

**Files:** none modified — verification only.

**Interfaces:** none.

- [ ] **Step 1: Full clean build**

Run: `cargo build -p manifold`
Expected: builds clean, zero warnings.

- [ ] **Step 2: Run the test suite**

Run: `cargo test -p manifold`
Expected: all tests pass, including `item_node_kind_tests` from Task 2.

- [ ] **Step 3: Live screenshot both toolbars side by side**

Follow "How to verify a toolbar change live" once more for each view, saving both screenshots. Confirm:
- Graph toolbar: Groups chip and Help/Settings chip both present, pixel-aligned with "+ New Tab"/"Reset View".
- Tab toolbar: nav chip, Bookmark/JSON chip, and Help/Settings chip all present, pixel-aligned with "Tab actions"/"Split"/the address bar.
- Neither toolbar shows the old flat, ungrouped icon rows.

- [ ] **Step 4: Compare against the approved mockup**

Re-open the `toolbar_redesign_options` mockup (Option A) referenced in the design spec's brainstorming history, or simply confirm by eye that both toolbars now read as: a primary text action, a grouped icon chip, another primary/secondary text action, then a filter/address field, then a trailing universal chip — matching Option A's intent even where Task 3's/Task 6's noted simplifications (return-to-Graph as a plain button; possibly bookmark staying `selectable_label`) mean it isn't a pixel-exact reproduction.

- [ ] **Step 5: Report completion**

No commit for this task (verification only) — if all checks pass, this plan is complete and ready for `superpowers:finishing-a-development-branch`.
