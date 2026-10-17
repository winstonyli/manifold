---
status: done
---

# JSON "Expand as Graph" Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user explode a JSON node's structure into real, nested graph nodes (Groups for objects/arrays, reused `JsonPayload` leaves for scalars), navigable and organizable like any other node.

**Architecture:** A new "Expand as Graph" context-menu action on a JSON node converts that node's `payload` in place from `NodePayload::Json` to `NodePayload::Group`, then recursively materializes one child node per JSON entry (nested Groups for containers, `JsonPayload` leaves for scalars), reusing the existing unlimited Group-nesting model. Read-only, one-way, session-only — no new `NodePayload` variant. A pre-existing bug in `close_group` (it silently leaks Note/Image/Pdf/Json members instead of deleting them) is fixed as part of this work, since it's exposed directly by this feature's core use case.

**Tech Stack:** Rust, egui/wgpu (existing `manifold` crate). No test framework exists in this codebase — verification is build (`cargo build`, zero warnings) plus live headless testing via this app's own stdin-driven `DebugCommand` protocol (see `crates/manifold/src/debug_control.rs`).

**Spec:** `docs/superpowers/specs/2026-09-19-json-expand-as-graph-design.md`

## Global Constraints

- No new `NodePayload` enum variant — leaves reuse `JsonPayload`.
- Fully recursive expansion, no size cap (per spec — deliberate, not an oversight).
- Read-only: nothing written back to the source JSON file.
- The exploded subgraph is session-only: never persisted to `session.json`. Groups already work this way (`GroupPayload`'s own doc comment); a leaf's new `key` field must never be written into `session::SessionJson` either.
- One-way: no "collapse back to a compact card." Re-adding the file is the accepted recovery path.
- Every new headless `DebugCommand` takes an explicit target id as an argument rather than relying on `self.focused`, to keep tests deterministic (this app's `self.focused` isn't reliably set after a plain `ADDJSON`/node-creation call).

---

### Task 1: Implement JSON "Expand as Graph"

**Files:**
- Modify: `crates/manifold/src/main.rs` (JsonPayload struct ~line 1223, its one construction site ~line 4972, JSON card rendering ~line 9874-9905, `NodeMenuAction`/`NodeMenuConfig`/`build_node_context_menu` ~lines 212-281, the 5 `NodeMenuConfig{...}` call sites in `build_download_context_menu`/`build_note_context_menu`/`build_image_context_menu`/`build_pdf_context_menu`/`build_json_context_menu` ~lines 6493-6729, `build_json_context_menu`'s action match ~line 6732, `handle_debug_command` ~line 11457)
- Modify: `crates/manifold/src/debug_control.rs` (`DebugCommand` enum ~line 230, `parse_line` ~line 429)

**Interfaces:**
- Produces: `App::expand_json_as_graph(&mut self, json_node_id: &str)` — the entry point Task 2 relies on for its own test fixture setup (via the new `EXPANDJSONASGRAPH <id>` debug command).
- Produces: `App::expand_json_entries(&mut self, parent_group_id: &str, value: &serde_json::Value, path: &str)` — the recursive step, private to `main.rs`.
- Produces: `JsonPayload { path: String, value: serde_json::Value, key: Option<String> }` (new `key` field).
- Produces: `DebugCommand::ExpandJsonAsGraph(String)` (explicit target node id).

- [ ] **Step 1: Add the `key` field to `JsonPayload`**

In `crates/manifold/src/main.rs`, find the `JsonPayload` struct (~line 1211-1226):

```rust
struct JsonPayload {
    path: String,
    value: serde_json::Value,
}
```

Replace with:

```rust
struct JsonPayload {
    path: String,
    value: serde_json::Value,
    /// The JSON key (or `[index]` for an array element) this value came
    /// from -- `None` for a genuine top-level file-backed JSON node (its
    /// card just shows the value's own shape), `Some(k)` for a leaf created
    /// by "Expand as Graph" (see `App::expand_json_entries`), whose card
    /// shows `"{k}: {value}"` instead so it's clear which field this is.
    /// Never persisted -- exploded leaves are session-only, matching
    /// `GroupPayload`'s own doc comment for the containers they live in.
    key: Option<String>,
}
```

- [ ] **Step 2: Update the one existing `JsonPayload` construction site**

In `crates/manifold/src/main.rs`, find `insert_json_node_with_id` (~line 4964-4977):

```rust
                payload: NodePayload::Json(JsonPayload { path, value }),
```

Replace with:

```rust
                payload: NodePayload::Json(JsonPayload { path, value, key: None }),
```

- [ ] **Step 3: Show the key on a JSON card when present**

In `crates/manifold/src/main.rs`, find the JSON card render loop (~line 9874-9880):

```rust
                for id in jsons_at_level.clone() {
                    let Some((node_pos, summary, path)) = self.nodes.get(&id).and_then(|n| {
                        let json = n.as_json()?;
                        Some((n.pos, json_summary(&json.value), json.path.clone()))
                    }) else {
                        continue;
                    };
```

Replace with:

```rust
                for id in jsons_at_level.clone() {
                    let Some((node_pos, summary, path)) = self.nodes.get(&id).and_then(|n| {
                        let json = n.as_json()?;
                        let summary = match &json.key {
                            Some(key) => format!("{key}: {}", json_summary(&json.value)),
                            None => json_summary(&json.value),
                        };
                        Some((n.pos, summary, json.path.clone()))
                    }) else {
                        continue;
                    };
```

- [ ] **Step 4: Build and confirm it still compiles**

Run:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold
```
Expected: builds successfully (the struct-literal at Step 2 is the only construction site, so nothing else needs updating yet).

- [ ] **Step 5: Add `ExpandAsGraph` to the shared context-menu action/config types**

In `crates/manifold/src/main.rs`, find `NodeMenuAction` (~line 212-218):

```rust
enum NodeMenuAction {
    CopyText,
    Reveal,
    CopyPath,
    RemoveFromGroup,
    Delete,
}
```

Replace with:

```rust
enum NodeMenuAction {
    CopyText,
    Reveal,
    CopyPath,
    ExpandAsGraph,
    RemoveFromGroup,
    Delete,
}
```

Find `NodeMenuConfig` (~line 227-233):

```rust
struct NodeMenuConfig {
    copy_text: bool,
    reveal_and_copy_path: bool,
    has_group: bool,
    delete_label: &'static str,
    width: f32,
}
```

Replace with:

```rust
struct NodeMenuConfig {
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

Find `build_node_context_menu`'s body (~line 260-267):

```rust
        if config.reveal_and_copy_path {
            if ui.button("Open Containing Folder").clicked() {
                action = Some(NodeMenuAction::Reveal);
            }
            if ui.button("Copy Path").clicked() {
                action = Some(NodeMenuAction::CopyPath);
            }
        }
```

Replace with:

```rust
        if config.reveal_and_copy_path {
            if ui.button("Open Containing Folder").clicked() {
                action = Some(NodeMenuAction::Reveal);
            }
            if ui.button("Copy Path").clicked() {
                action = Some(NodeMenuAction::CopyPath);
            }
        }
        if config.show_expand && ui.button("Expand as Graph").clicked() {
            action = Some(NodeMenuAction::ExpandAsGraph);
        }
```

- [ ] **Step 6: Add `show_expand: false` to the four `NodeMenuConfig` call sites that aren't JSON**

In `crates/manifold/src/main.rs`, four separate edits. Each of these `NodeMenuConfig { ... }` literals needs one new line, `show_expand: false,`, added anywhere inside the braces (placed here right after `reveal_and_copy_path`/`copy_text` for consistency with Step 5's field order):

`build_download_context_menu` (~line 6493-6499):
```rust
            NodeMenuConfig {
                copy_text: false,
                reveal_and_copy_path: true,
                show_expand: false,
                has_group: false,
                delete_label: "Remove from List",
                width: 170.0,
            },
```

`build_note_context_menu` (~line 6542-6548):
```rust
            NodeMenuConfig {
                copy_text: true,
                reveal_and_copy_path: false,
                show_expand: false,
                has_group,
                delete_label: "Delete",
                width: 150.0,
            },
```

`build_image_context_menu` (~line 6600-6606):
```rust
            NodeMenuConfig {
                copy_text: false,
                reveal_and_copy_path: true,
                show_expand: false,
                has_group,
                delete_label: "Delete",
                width: 170.0,
            },
```

`build_pdf_context_menu` (~line 6661-6667):
```rust
            NodeMenuConfig {
                copy_text: false,
                reveal_and_copy_path: true,
                show_expand: false,
                has_group,
                delete_label: "Delete",
                width: 170.0,
            },
```

- [ ] **Step 7: Add `show_expand: true` to the JSON context menu's own config, and handle the new action**

In `crates/manifold/src/main.rs`, `build_json_context_menu` (~line 6718-6730):

```rust
        let (action, clicked_outside) = build_node_context_menu(
            ui,
            "json_context_menu",
            pos,
            fade,
            NodeMenuConfig {
                copy_text: false,
                reveal_and_copy_path: true,
                has_group,
                delete_label: "Delete",
                width: 170.0,
            },
        );

        match action {
            Some(NodeMenuAction::Reveal) => self.reveal_json(&id),
```

Replace with:

```rust
        let (action, clicked_outside) = build_node_context_menu(
            ui,
            "json_context_menu",
            pos,
            fade,
            NodeMenuConfig {
                copy_text: false,
                reveal_and_copy_path: true,
                show_expand: true,
                has_group,
                delete_label: "Delete",
                width: 170.0,
            },
        );

        match action {
            Some(NodeMenuAction::Reveal) => self.reveal_json(&id),
            Some(NodeMenuAction::ExpandAsGraph) => {
                self.expand_json_as_graph(&id);
                self.save_session();
            }
```

- [ ] **Step 8: Build and confirm zero warnings**

Run:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold 2>&1 | tail -40
```
Expected: builds successfully. This will currently fail with "no method named `expand_json_as_graph`" — that's expected, Step 9 adds it. If any other error appears (a missed `NodeMenuConfig` call site), fix it before continuing.

- [ ] **Step 9: Implement the recursive expansion**

In `crates/manifold/src/main.rs`, add these two new methods directly after `insert_json_node_with_id` (which ends ~line 4977, right before the `/// The Graph toolbar's single "+ File" button` comment at ~line 4979):

```rust
    /// "Expand as Graph" -- explodes a JSON node's structure into real,
    /// nested graph nodes: an object/array becomes a `Group` (matching this
    /// app's existing unlimited Group-nesting model), a scalar becomes a
    /// plain `JsonPayload` leaf (see that struct's own doc comment for why
    /// no new node kind exists for this). Read-only, one-way, and
    /// session-only, per this feature's own design doc
    /// (`docs/superpowers/specs/2026-09-19-json-expand-as-graph-design.md`):
    /// there's no "collapse back," and nothing here is written to
    /// `session.json`. The original node is converted in place (`payload`
    /// is swapped, not the node removed and reinserted) so its existing
    /// `pos`/`group`/`color` (all on the shared `GraphNode`) carry over
    /// untouched. A bare scalar at the JSON's own root has nothing to
    /// explode into -- this just leaves such a node as it already is,
    /// a harmless no-op rather than something worth guarding against.
    fn expand_json_as_graph(&mut self, json_node_id: &str) {
        let Some((path, value)) =
            self.nodes.get(json_node_id).and_then(|n| n.as_json().map(|j| (j.path.clone(), j.value.clone())))
        else {
            return;
        };
        if matches!(value, serde_json::Value::Object(_) | serde_json::Value::Array(_)) {
            let root_name = download_filename(&path).to_string();
            if let Some(node) = self.nodes.get_mut(json_node_id) {
                node.payload = NodePayload::Group(GroupPayload { name: root_name });
            }
            self.expand_json_entries(json_node_id, &value, &path);
        }
    }

    /// Recursive step shared by `expand_json_as_graph`: creates one child
    /// node per entry of `value` (already known to be an `Object`/`Array`)
    /// inside `parent_group_id`, recursing into any entry that's itself a
    /// container. `path` is the *source file's* path, inherited by every
    /// leaf purely for "Reveal in Explorer"/"Copy Path" display purposes --
    /// it does not mean the leaf independently owns that file (see
    /// `JsonPayload`'s own doc comment on `key`).
    fn expand_json_entries(&mut self, parent_group_id: &str, value: &serde_json::Value, path: &str) {
        let entries: Vec<(String, &serde_json::Value)> = match value {
            serde_json::Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v)).collect(),
            serde_json::Value::Array(items) => {
                items.iter().enumerate().map(|(i, v)| (format!("[{i}]"), v)).collect()
            }
            _ => return,
        };
        let parent_pos = self.nodes.get(parent_group_id).map(|n| n.pos).unwrap_or(GRAPH_WORLD_CENTER);
        for (key, entry_value) in entries {
            if matches!(entry_value, serde_json::Value::Object(_) | serde_json::Value::Array(_)) {
                let id = self.alloc_group_id();
                let pos = parent_pos + graph_layout::spawn_jitter(&id);
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        parent: None,
                        group: Some(parent_group_id.to_string()),
                        pos,
                        vel: egui::Vec2::ZERO,
                        color: None,
                        payload: NodePayload::Group(GroupPayload { name: key }),
                    },
                );
                self.node_order.push(id.clone());
                self.expand_json_entries(&id, entry_value, path);
            } else {
                let id = self.alloc_json_id();
                let pos = parent_pos + graph_layout::spawn_jitter(&id);
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        parent: None,
                        group: Some(parent_group_id.to_string()),
                        pos,
                        vel: egui::Vec2::ZERO,
                        color: None,
                        payload: NodePayload::Json(JsonPayload {
                            path: path.to_string(),
                            value: entry_value.clone(),
                            key: Some(key),
                        }),
                    },
                );
                self.node_order.push(id);
            }
        }
    }
```

- [ ] **Step 10: Build and confirm zero warnings**

Run:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold 2>&1 | tail -40
```
Then:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold 2>&1 | grep -i warning
```
Expected: first command ends with `Finished`; second command has no output (exit code 1 from `grep` finding nothing is expected and fine).

- [ ] **Step 11: Add the headless debug command**

In `crates/manifold/src/debug_control.rs`, find the end of the `DebugCommand` enum (~line 229-231):

```rust
    /// The Tab-view toolbar's "Close Split" button — see `App::close_split`.
    CloseSplit,
}
```

Replace with:

```rust
    /// The Tab-view toolbar's "Close Split" button — see `App::close_split`.
    CloseSplit,
    /// The JSON node context menu's "Expand as Graph" button, targeting an
    /// explicit node id instead of `self.focused` (a plain `ADDJSON` doesn't
    /// focus the node it creates, so a headless test can't rely on focus
    /// here) — see `App::expand_json_as_graph`.
    ExpandJsonAsGraph(String),
}
```

Find the `parse_line` match arm for `CLOSESPLIT` (~line 429):

```rust
        "CLOSESPLIT" => Some(DebugCommand::CloseSplit),
        _ => None,
```

Replace with:

```rust
        "CLOSESPLIT" => Some(DebugCommand::CloseSplit),
        "EXPANDJSONASGRAPH" => {
            Some(DebugCommand::ExpandJsonAsGraph(rest.to_string())).filter(|_| !rest.is_empty())
        }
        _ => None,
```

- [ ] **Step 12: Wire the debug command**

In `crates/manifold/src/main.rs`, find (~line 11456-11457):

```rust
            DebugCommand::Split(kind) => self.start_split(kind),
            DebugCommand::CloseSplit => self.close_split(),
```

Replace with:

```rust
            DebugCommand::Split(kind) => self.start_split(kind),
            DebugCommand::CloseSplit => self.close_split(),
            DebugCommand::ExpandJsonAsGraph(id) => {
                self.expand_json_as_graph(&id);
                self.save_session();
            }
```

- [ ] **Step 13: Build and confirm zero warnings**

Run:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold 2>&1 | tail -40
```
Expected: `Finished` with no errors.

- [ ] **Step 14: Live-verify headlessly**

Back up the real session/settings files, write a fresh session so the very first `ADDJSON` call deterministically produces id `json-0` (JSON ids are persisted and counted via `next_json_id`; Group ids are not persisted at all, so `next_group_id` always starts at 0 on a fresh launch regardless of `session.json`), and write a small fixture JSON with a mix of a nested object, a nested array, and scalars so the test exercises every branch of `expand_json_entries`:

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cp "$APPDATA/manifold/session.json" "$APPDATA/manifold/session.json.bak"
cp "$APPDATA/manifold/settings.json" "$APPDATA/manifold/settings.json.bak"
cat > "$APPDATA/manifold/session.json" <<'EOF'
{
  "tabs": [], "notes": [], "images": [], "pdfs": [], "jsons": [],
  "focused": null, "view_mode_tab": false,
  "next_tab_id": 0, "next_note_id": 0, "next_image_id": 0, "next_pdf_id": 0, "next_json_id": 0
}
EOF
cat > /tmp/expand_fixture.json <<'EOF'
{"name": "Alice", "tags": ["x", "y"], "meta": {"age": 30}}
EOF
taskkill //F //IM manifold.exe //T 2>/dev/null
taskkill //F //IM manifold-engine-webview2.exe //T 2>/dev/null
coproc APP { ./target/debug/manifold.exe --headless 2>&1; }
sleep 2
echo "ADDJSON $(cygpath -w /tmp/expand_fixture.json)" >&${APP[1]}
sleep 1
echo "EXPANDJSONASGRAPH json-0" >&${APP[1]}
sleep 1
echo "EXPORTSESSION $(cygpath -w /tmp/expand_result.json)" >&${APP[1]}
sleep 1
echo "SCREENSHOT $(cygpath -w /tmp/expand_screenshot.png)" >&${APP[1]}
sleep 1
echo "QUIT" >&${APP[1]}
sleep 1
taskkill //F //IM manifold.exe //T 2>/dev/null
taskkill //F //IM manifold-engine-webview2.exe //T 2>/dev/null
```

Then restore the real files:
```bash
mv "$APPDATA/manifold/session.json.bak" "$APPDATA/manifold/session.json"
mv "$APPDATA/manifold/settings.json.bak" "$APPDATA/manifold/settings.json"
```

Check the screenshot (Windows path via `cygpath -w /tmp/expand_screenshot.png`) — expect to see, at the top level, one Group card (labeled after the fixture file's own name) with no other cards floating loose at that level, since expansion converts the original node in place. Enter that group (double-click it, or check via a second screenshot after navigating in) and confirm three children: a `name: "Alice"` leaf card, a `tags` Group card, and a `meta` Group card. Entering `tags` should show two leaf cards, `[0]: "x"` and `[1]: "y"`; entering `meta` should show one leaf card, `age: 30`.

`EXPORTSESSION`'s dump reflects live in-memory state (it's not reading back the persisted-only `session.json` fields) — confirm it contains none of the synthetic Group/leaf nodes this expansion created, since they're deliberately session-only per the spec.

- [ ] **Step 15: Commit**

```bash
git add crates/manifold/src/main.rs crates/manifold/src/debug_control.rs
git commit -m "$(cat <<'EOF'
Add "Expand as Graph" for JSON nodes

A JSON node's context menu gained a new action that explodes its
structure into real, nested graph nodes: objects/arrays become Groups
(reusing the existing unlimited-nesting model), scalars become plain
JsonPayload leaves carrying a new optional key field for display.
Read-only, one-way, and session-only per the feature's own design doc.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Fix `close_group` to destroy Note/Image/Pdf/Json members instead of leaking them

**Files:**
- Modify: `crates/manifold/src/main.rs` (`close_group` ~line 6349-6377, `handle_debug_command` ~line 11457)
- Modify: `crates/manifold/src/debug_control.rs` (`DebugCommand` enum, `parse_line`)

**Interfaces:**
- Consumes: `App::expand_json_as_graph` (Task 1) — used to build this task's own live-verification fixture (a Group containing Json leaves) without needing a real drag gesture.
- Produces: `App::close_group_member(&mut self, id: &str)` — private, used only by `close_group`.
- Produces: `DebugCommand::CloseGroup(String)` (explicit target group id).

- [ ] **Step 1: Add the per-kind member-cleanup helper**

In `crates/manifold/src/main.rs`, find `close_group` (~line 6349-6377):

```rust
    fn close_group(&mut self, id: &str) {
        if let Some(export) = self.export_group(id) {
            self.closed_groups.push(export);
            if self.closed_groups.len() > MAX_CLOSED_GROUPS {
                self.closed_groups.remove(0);
            }
        }
        let closed_tabs_before = self.closed_tabs.len();
        let member_ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.group.as_deref() == Some(id))
            .map(|(tid, _)| tid.clone())
            .collect();
        for tid in member_ids {
            // A member can itself be a nested subgroup now that groups
            // nest -- `close_tab` silently no-ops on a non-tab node, which
            // left a nested subgroup (and everything in it) alive,
            // reparented to the closed group's own parent instead of
            // destroyed. Recurse into it instead.
            if self.nodes.get(&tid).is_some_and(|n| n.as_group().is_some()) {
                self.close_group(&tid);
            } else {
                self.close_tab(&tid);
            }
        }
        self.closed_tabs.truncate(closed_tabs_before);
        self.dissolve_group(id);
    }
```

Replace with:

```rust
    fn close_group(&mut self, id: &str) {
        if let Some(export) = self.export_group(id) {
            self.closed_groups.push(export);
            if self.closed_groups.len() > MAX_CLOSED_GROUPS {
                self.closed_groups.remove(0);
            }
        }
        let closed_tabs_before = self.closed_tabs.len();
        let member_ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.group.as_deref() == Some(id))
            .map(|(tid, _)| tid.clone())
            .collect();
        for tid in member_ids {
            self.close_group_member(&tid);
        }
        self.closed_tabs.truncate(closed_tabs_before);
        self.dissolve_group(id);
    }

    /// Destroys one member of a group being closed, applying the same
    /// per-kind cleanup its own "Delete"/"Close" action already would: a
    /// nested subgroup recurses (`close_group`), a tab goes through
    /// `close_tab` (feeding "Recently Closed" tabs), and a Note/Image/Pdf/
    /// Json member captures a `ClosedNode` (feeding "Recently Closed"
    /// nodes) before `remove_node`, freeing a GPU texture first for an
    /// Image/Pdf. Without this, `close_tab`'s own no-op-on-non-tab guard
    /// left every non-tab, non-group member merely reparented to the
    /// closed group's own parent by `dissolve_group` below, never actually
    /// destroyed -- a real gap confirmed while building "Expand as Graph"
    /// (see that feature's own design doc), whose whole output is
    /// `JsonPayload` leaves nested inside Groups.
    fn close_group_member(&mut self, id: &str) {
        enum MemberKind {
            Group,
            Tab,
            Note(String),
            Image(String),
            Pdf(String),
            Json(String),
            Download,
        }
        let Some(kind) = self.nodes.get(id).map(|node| match &node.payload {
            NodePayload::Group(_) => MemberKind::Group,
            NodePayload::Tab(_) => MemberKind::Tab,
            NodePayload::Note(note) => MemberKind::Note(note.text.clone()),
            NodePayload::Image(image) => MemberKind::Image(image.path.clone()),
            NodePayload::Pdf(pdf) => MemberKind::Pdf(pdf.path.clone()),
            NodePayload::Json(json) => MemberKind::Json(json.path.clone()),
            NodePayload::Download(_) => MemberKind::Download,
        }) else {
            return;
        };
        match kind {
            MemberKind::Group => self.close_group(id),
            MemberKind::Tab => self.close_tab(id),
            MemberKind::Note(text) => {
                self.record_closed_node(ClosedNode::Note(text));
                self.remove_node(id);
            }
            MemberKind::Image(path) => {
                self.record_closed_node(ClosedNode::Image(path));
                if let Some(gpu) = &mut self.gpu {
                    gpu.free_image(id);
                }
                self.remove_node(id);
            }
            MemberKind::Pdf(path) => {
                self.record_closed_node(ClosedNode::Pdf(path));
                if let Some(gpu) = &mut self.gpu {
                    gpu.free_image(id);
                }
                self.remove_node(id);
            }
            MemberKind::Json(path) => {
                self.record_closed_node(ClosedNode::Json(path));
                if self.json_viewer_node.as_deref() == Some(id) {
                    self.json_viewer_node = None;
                }
                self.remove_node(id);
            }
            MemberKind::Download => self.remove_node(id),
        }
    }
```

- [ ] **Step 2: Build and confirm zero warnings**

Run:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold 2>&1 | tail -40
```
Expected: `Finished` with no errors or warnings.

- [ ] **Step 3: Add a headless debug command to close a specific group**

In `crates/manifold/src/debug_control.rs`, find the end of the `DebugCommand` enum you extended in Task 1 (~line 234-236 after that change):

```rust
    ExpandJsonAsGraph(String),
}
```

Replace with:

```rust
    ExpandJsonAsGraph(String),
    /// A group card's own "Close" context-menu action, targeting an
    /// explicit group id instead of `self.focused` -- same "deterministic
    /// headless test" reasoning as `ExpandJsonAsGraph`. No-ops if `id`
    /// isn't actually a group. See `App::close_group`.
    CloseGroup(String),
}
```

Find the `parse_line` match arm you added in Task 1:

```rust
        "EXPANDJSONASGRAPH" => {
            Some(DebugCommand::ExpandJsonAsGraph(rest.to_string())).filter(|_| !rest.is_empty())
        }
        _ => None,
```

Replace with:

```rust
        "EXPANDJSONASGRAPH" => {
            Some(DebugCommand::ExpandJsonAsGraph(rest.to_string())).filter(|_| !rest.is_empty())
        }
        "CLOSEGROUP" => Some(DebugCommand::CloseGroup(rest.to_string())).filter(|_| !rest.is_empty()),
        _ => None,
```

- [ ] **Step 4: Wire the debug command**

In `crates/manifold/src/main.rs`, find the arm you added in Task 1:

```rust
            DebugCommand::ExpandJsonAsGraph(id) => {
                self.expand_json_as_graph(&id);
                self.save_session();
            }
```

Replace with:

```rust
            DebugCommand::ExpandJsonAsGraph(id) => {
                self.expand_json_as_graph(&id);
                self.save_session();
            }
            DebugCommand::CloseGroup(id) => {
                if self.nodes.get(&id).is_some_and(|n| n.as_group().is_some()) {
                    self.close_group(&id);
                }
            }
```

- [ ] **Step 5: Build and confirm zero warnings**

Run:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold 2>&1 | tail -40
```
Expected: `Finished` with no errors or warnings.

- [ ] **Step 6: Live-verify headlessly, using Task 1's own feature as the test fixture**

This reuses `EXPANDJSONASGRAPH` to build a group containing `JsonPayload` leaves in one deterministic step, sidestepping the drag-based (headless-fragile) way a Note/Image/Pdf/Json would normally join a group:

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cp "$APPDATA/manifold/session.json" "$APPDATA/manifold/session.json.bak"
cp "$APPDATA/manifold/settings.json" "$APPDATA/manifold/settings.json.bak"
cat > "$APPDATA/manifold/session.json" <<'EOF'
{
  "tabs": [], "notes": [], "images": [], "pdfs": [], "jsons": [],
  "focused": null, "view_mode_tab": false,
  "next_tab_id": 0, "next_note_id": 0, "next_image_id": 0, "next_pdf_id": 0, "next_json_id": 0
}
EOF
cat > /tmp/close_group_fixture.json <<'EOF'
{"name": "Alice", "age": 30}
EOF
taskkill //F //IM manifold.exe //T 2>/dev/null
taskkill //F //IM manifold-engine-webview2.exe //T 2>/dev/null
coproc APP { ./target/debug/manifold.exe --headless 2>&1; }
sleep 2
echo "ADDJSON $(cygpath -w /tmp/close_group_fixture.json)" >&${APP[1]}
sleep 1
echo "EXPANDJSONASGRAPH json-0" >&${APP[1]}
sleep 1
echo "CLOSEGROUP json-0" >&${APP[1]}
sleep 1
echo "SCREENSHOT $(cygpath -w /tmp/close_group_screenshot.png)" >&${APP[1]}
sleep 1
echo "QUIT" >&${APP[1]}
sleep 1
taskkill //F //IM manifold.exe //T 2>/dev/null
taskkill //F //IM manifold-engine-webview2.exe //T 2>/dev/null
mv "$APPDATA/manifold/session.json.bak" "$APPDATA/manifold/session.json"
mv "$APPDATA/manifold/settings.json.bak" "$APPDATA/manifold/settings.json"
```

Note: `expand_json_as_graph` converts the original `json-0` node's own payload to a Group *in place*, so `CLOSEGROUP json-0` targets that same id — correct, not a typo.

Check the screenshot: expect a completely empty graph (both leaf children destroyed, not reparented up to the top level) — this is exactly the failure mode Step 1's fix addresses; before the fix, this same sequence would have shown two orphaned leaf cards (`name: "Alice"` and `age: 30`) sitting at the top level instead of a clean empty graph.

- [ ] **Step 7: Commit**

```bash
git add crates/manifold/src/main.rs crates/manifold/src/debug_control.rs
git commit -m "$(cat <<'EOF'
Fix close_group leaking Note/Image/Pdf/Json members instead of deleting them

close_group only ever destroyed Tab members (and recursed into nested
Group members) -- close_tab silently no-ops on any other kind, so a
Note/Image/Pdf/Json member of a closed group was left behind, merely
reparented to the closed group's own parent by dissolve_group. Exposed
directly by "Expand as Graph," whose exploded subgraphs are exactly
this: JsonPayload leaves nested inside Groups.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Post-implementation: update README

After both tasks are committed, add one README.md entry (matching this project's own established changelog convention — see existing entries after "## Known limitations") describing: the new "Expand as Graph" action, the design decisions (read-only, one-way, session-only, no size cap), the `close_group` bug it exposed and the general fix applied, and how both were live-verified headlessly. This isn't its own task since it has no independent test cycle — fold it into whichever of the two tasks' sessions is still open, or do it immediately after Task 2's commit.
