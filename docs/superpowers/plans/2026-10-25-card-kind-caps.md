---
status: done
---

# Card-kind caps (glyph, kind word, omnibar membership) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (chosen: inline) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move a card kind's icon, kind word and omnibar membership into the capability rows so 17 hand-written matches become one table.

**Architecture:** `Caps` gains `glyph`, `kind_name`, `omni_row`; `builtin_rows` sets them per type; `card_kind::kind_caps(Kind)` is the lookup for sites that hold a `Kind`. The sites read it and their matches go. `GraphNode::accessible_name` becomes `accessible_detail`; `App::accessible_name` joins the registry's word to it.

**Tech Stack:** Rust, egui, egui_phosphor (`light`).

**Spec:** `docs/superpowers/specs/2026-10-25-card-kind-caps-design.md` (rulings R1-R4 bind).

## Global Constraints

- Work on `main`; commit per task; `cargo fmt --all` before each commit; run `cargo` with `-j 8`.
- Clippy must stay clean: `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings`.
- Glyph/word table (exact): tab `BROWSER` "Sleeping tab" / loaded "Tab"; group `FOLDER` "Group"; note `NOTE_PENCIL` "Note"; image `IMAGE` "Image"; pdf `FILE_PDF` "PDF"; json `BRACKETS_CURLY` "JSON"; download `DOWNLOAD_SIMPLE` "Download"; jar `COOKIE` "Cookie jar"; unknown = `Caps::default()` (`QUESTION`, "Card"). `omni_row` true for group, note, image, pdf, json, download only.
- Accessible names stay byte-for-byte what they are today.
- No test touches the real `%APPDATA%\manifold`; Python, if any, runs as `python -P f.py < /dev/null`.
- Do not push, tag or bump. Delete PARKED.md "One icon table for card kinds" when this lands.

## Review Focus

- A node whose mirror holds no type reads `Caps::default()`: `accessible_name` says "Card" (was: from its `Runtime`). Test: Task 2.
- A renamed non-container keeps `"{word}: {renamed}"`; a renamed group keeps its own name. Test: Task 2 (existing + new).
- A failed or waking tab (`Runtime::Unloaded`) says "Sleeping tab", a live one "Tab". Test: Task 2.
- Recently Closed draws exactly one icon per row: none doubled for a tab (favicon only), the table glyph for every other kind, `QUESTION` for an unknown type. Test: Task 3.
- A link or label node is never an omnibar `Other` row. Test: Task 1 (`kind_caps(Kind::Link).omni_row`).
- The loaded-tab row overrides only the word; it keeps the tab glyph (fold order). Test: Task 1.

---

### Task 1: `Caps` fields, rows and `kind_caps`

**Files:**
- Modify: `crates/manifold/src/caps/rows.rs` (Row, Caps, Default, `Registry::caps`, `builtin_rows`)
- Modify: `crates/manifold/src/card_kind.rs` (`kind_caps`, test)

**Interfaces:**
- Produces: `Caps { glyph: &'static str, kind_name: &'static str, omni_row: bool, .. }`; `Row::glyph(&'static str)`, `Row::kind_name(&'static str)`, `Row::omni_row(bool)`; `card_kind::kind_caps(kind: Kind) -> Caps`.

- [ ] **Step 1: Write the failing test** at the end of `mod tests` in `card_kind.rs` (before its closing brace):

```rust
    /// Each kind's icon, kind word and omnibar membership (spec "Rows"); `kind_caps` agrees for a kind with no tags.
    #[test]
    fn each_kind_has_its_glyph_word_and_omnibar_membership() {
        use egui_phosphor::light as i;
        let table: [(CardKind, &str, &str, bool); 10] = [
            (loaded_tab(), i::BROWSER, "Tab", false),
            (sleeping_tab(), i::BROWSER, "Sleeping tab", false),
            (plain(TYPE_DOWNLOAD), i::DOWNLOAD_SIMPLE, "Download", true),
            (plain(TYPE_NOTE), i::NOTE_PENCIL, "Note", true),
            (plain(TYPE_JAR), i::COOKIE, "Cookie jar", false),
            (plain(TYPE_UNKNOWN), i::QUESTION, "Card", false),
            (plain(TYPE_IMAGE), i::IMAGE, "Image", true),
            (plain(TYPE_PDF), i::FILE_PDF, "PDF", true),
            (plain(TYPE_JSON), i::BRACKETS_CURLY, "JSON", true),
            (plain(TYPE_GROUP), i::FOLDER, "Group", true),
        ];
        assert_eq!(table.map(|row| row.0), kinds(), "the table lists kinds() in order");
        for (kind, glyph, word, omni) in table {
            let caps = kind.caps();
            assert_eq!((caps.glyph, caps.kind_name, caps.omni_row), (glyph, word, omni), "{kind:?}");
        }
        assert_eq!(plain(TYPE_UNKNOWN).caps().glyph, Caps::default().glyph);
        assert!(kinds().iter().filter(|k| k.ty != TYPE_UNKNOWN).all(|k| k.caps().glyph != Caps::default().glyph));
        for kind in [Kind::Group, Kind::Note, Kind::Image, Kind::Pdf, Kind::Json, Kind::Download, Kind::Jar, Kind::Unknown] {
            let ty = kind.type_str().unwrap_or(TYPE_UNKNOWN);
            assert_eq!(kind_caps(kind), plain(ty).caps(), "{kind:?}");
        }
        // A link or a label is stored, never a card: nothing about it reaches the omnibar.
        assert!(!kind_caps(Kind::Link).omni_row && !kind_caps(Kind::Label).omni_row);
    }
```

- [ ] **Step 2: Run it, expect a compile failure** (`no field glyph`, `kind_caps` not found):
Run: `cargo test -j 8 -p manifold each_kind_has_its_glyph 2>&1 | tail -20`
Expected: error[E0609]/E0425.

- [ ] **Step 3: Implement.** In `caps/rows.rs`:
  - `Row`: add fields `glyph: Option<&'static str>, kind_name: Option<&'static str>, omni_row: Option<bool>`; `Row::new` sets them `None`; add builders (same shape as `menu_area`): `pub(crate) fn glyph(mut self, glyph: &'static str) -> Row { self.glyph = Some(glyph); self }`, likewise `kind_name`, and `omni_row(mut self, on: bool)`.
  - `Caps`: add, with doc comments, `pub(crate) glyph: &'static str` ("Its icon: the card header, the omnibar row, Recently Closed and the New menu."), `pub(crate) kind_name: &'static str` ("The first word of its accessible name."), `pub(crate) omni_row: bool` ("The omnibar lists it as an `Other` row when it matches.").
  - `Default`: `glyph: egui_phosphor::light::QUESTION, kind_name: "Card", omni_row: false`.
  - `Registry::caps`: three more `if let Some(v) = row.glyph { caps.glyph = v; }` blocks, same shape as `menu_area`.
  - `builtin_rows` (add `use egui_phosphor::light as icon;` inside the function), change these rows:

```rust
        Row::new(TYPE_TAB).restorable(true).draw_layer(1).delete_verb("close").click(Click::Open).glyph(icon::BROWSER).kind_name("Sleeping tab"), // a sleeping tab; a loaded one draws under it
        Row::new(TYPE_TAB).requires(&[Tag::Loaded]).draw_layer(0).kind_name("Tab"),
        Row::new(TYPE_GROUP).restorable(true).draw_layer(7).delete_verb("close").glyph(icon::FOLDER).kind_name("Group").omni_row(true),
```
  and append to the existing chains: image `.glyph(icon::IMAGE).kind_name("Image").omni_row(true)`; pdf `.glyph(icon::FILE_PDF).kind_name("PDF").omni_row(true)`; json `.glyph(icon::BRACKETS_CURLY).kind_name("JSON").omni_row(true)`; note `.glyph(icon::NOTE_PENCIL).kind_name("Note").omni_row(true)`; jar `.glyph(icon::COOKIE).kind_name("Cookie jar")`; download `.glyph(icon::DOWNLOAD_SIMPLE).kind_name("Download").omni_row(true)`. The `TYPE_UNKNOWN` and blank-note rows are unchanged. If `icon::BROWSER` does not exist, stop and tell the user (it is the one unverified constant).
  - Update the `builtin_rows` doc comment to say it also holds each type's icon, word and omnibar membership.
  
  In `card_kind.rs`, after `canonical_type`:

```rust
/// The registry's answer for a kind with no tags: its icon, its kind word and its abilities. For a site that holds a
/// `Kind` and no `CardKind` (a card body, the omnibar, Recently Closed). A loaded tab differs only by its tag
/// (`CardKind::caps`).
pub(crate) fn kind_caps(kind: Kind) -> Caps {
    registry().caps(kind.type_str().unwrap_or(TYPE_UNKNOWN), TagSet::EMPTY)
}
```
  (add `Kind` to the test module's imports only if it does not resolve through `use super::*`.)

- [ ] **Step 4: Run the test**
Run: `cargo test -j 8 -p manifold each_kind_has_its_glyph 2>&1 | tail -8` then `cargo test -j 8 -p manifold caps:: card_kind:: 2>&1 | tail -8`
Expected: pass; no other `caps`/`card_kind` test fails (a `Caps` struct-literal elsewhere would fail to compile: add the three fields there).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add -A crates && git diff --cached --stat && git commit -m "caps: glyph, kind word and omnibar membership per card type

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

### Task 2: Accessible names read the registry's word

**Files:**
- Modify: `crates/manifold/src/node.rs` (`GraphNode::accessible_name` at 457, `App::accessible_name` at 495, tests at 549-590)

**Interfaces:**
- Consumes: `CardKind::caps().kind_name` (Task 1), `App::card_kind(id) -> Option<CardKind>`.
- Produces: `GraphNode::accessible_detail(&self, text: &str, source: Option<&str>) -> Option<String>`; `App::accessible_name(id) -> Option<String>` unchanged signature.

- [ ] **Step 1: Write the failing tests.** In `node.rs` tests, replace `a_local_file_card_has_an_accessible_name` with detail asserts and add the word test:

```rust
    /// Review C-B12: a local file's card is named by its file name; a web source still wins.
    #[test]
    fn a_local_file_card_has_an_accessible_detail() {
        let node = |runtime| GraphNode { runtime };
        let pdf = node(Runtime::Pdf(PdfPayload { path: r"C:\docs\report.pdf".into(), outcome: None }));
        assert_eq!(pdf.accessible_detail("", None).as_deref(), Some("report.pdf"));
        let source = Some("https://example.org/r.pdf");
        assert_eq!(pdf.accessible_detail("", source).as_deref(), Some("https://example.org/r.pdf"));
        let image =
            node(Runtime::Image(ImagePayload { path: "/home/u/cat.png".into(), width: 1, height: 1, error: None }));
        assert_eq!(image.accessible_detail("", None).as_deref(), Some("cat.png"));
        let json = node(Runtime::Json(JsonPayload::file(r"D:\data\x.json".into(), Default::default(), None)));
        assert_eq!(json.accessible_detail("", None).as_deref(), Some("x.json"));
    }

    /// The kind word comes from the registry (spec R4): a live tab, a sleeping one, a download, a jar, a group, a
    /// node with no stored type, and a rename that keeps the word.
    #[test]
    fn the_accessible_name_starts_with_the_registry_word() {
        use crate::store::model::{field, FieldValue, Kind};
        let mut app = crate::App::default();
        let place = |app: &mut crate::App, kind, runtime| {
            let id = app.alloc_node_id();
            app.test_node(id, kind, runtime);
            id
        };
        let (tab, _peer) = crate::process_manager::ViewTab::fake(crate::process_manager::ViewKind::Servo);
        let live = place(
            &mut app,
            Kind::Tab,
            Runtime::Tab(TabPayload {
                tab,
                url: "https://a/".into(),
                title: String::new(),
                can_go_back: false,
                can_go_forward: false,
                loading: false,
                audio_playing: false,
                muted: false,
                zoom_factor: 1.0,
                swap_target: None,
            }),
        );
        let sleeping = place(
            &mut app,
            Kind::Tab,
            Runtime::Unloaded(UnloadedTabPayload {
                url: "https://s/".into(),
                title: String::new(),
                muted: false,
                zoom_factor: 1.0,
                state: UnloadedState::Failed,
            }),
        );
        let download = place(
            &mut app,
            Kind::Download,
            Runtime::Download(DownloadPayload {
                path: "dir/f.bin".into(),
                bytes_received: 0,
                total_bytes: None,
                completed: false,
                failed: false,
            }),
        );
        let jar = place(&mut app, Kind::Jar, Runtime::Jar);
        let untyped = place(&mut app, Kind::Note, Runtime::Note(NotePayload::default()));
        let name = |app: &crate::App, id| app.accessible_name(id).unwrap();
        assert_eq!(name(&app, live), "Tab: https://a/");
        assert_eq!(name(&app, sleeping), "Sleeping tab: https://s/");
        assert_eq!(name(&app, download), "Download: f.bin");
        assert_eq!(name(&app, jar), "Cookie jar: Untitled");
        // A renamed non-container keeps its word; the rename is the detail.
        app.doc.mirror.set(download, field::NAME, FieldValue::Str("Mine".into()));
        assert_eq!(name(&app, download), "Download: Mine");
        // No stored type reads `Caps::default()`: "Card" (review focus 1).
        app.doc.mirror.set_type_for_test(untyped, "");
        assert_eq!(name(&app, untyped), "Card");
    }
```
  (Adjust to what compiles: `UnloadedState`'s failed variant, `NotePayload::default`, and a way to blank a node's mirror type are looked up in `card_kind.rs` tests and `Mirror`; if no setter exists for the mirror type, drop the `untyped` case and cover "Card" with the existing `session_sync` tests that name an unknown-type card, then say so in the ledger. `app.test_node` takes `&mut self`, so `place` takes `app` as a parameter.)
  Change the helper in `accessible_name_is_kind_then_title` to `let name = |app: &crate::App, id: NodeId| app.accessible_name(id).unwrap();`.

- [ ] **Step 2: Run, expect compile failure** (`accessible_detail` not found):
Run: `cargo test -j 8 -p manifold accessible 2>&1 | tail -20`

- [ ] **Step 3: Implement.** In `GraphNode`, rename to `accessible_detail(&self, text: &str, source: Option<&str>) -> Option<String>`; the match arms drop their word and yield `Option<String>`:

```rust
        match &self.runtime {
            Runtime::Tab(t) => first_nonempty(&[&t.title, &t.url]),
            Runtime::Unloaded(t) => first_nonempty(&[&t.title, &t.url]),
            Runtime::Download(d) => file_name(&d.path),
            // A group has no runtime of its own (2b-6 Ruling 8); its name is `text`. A jar is named like a group.
            Runtime::None | Runtime::Jar => {
                Some(crate::names::to_plain(&crate::names::display_name(text, crate::names::UNTITLED)))
            }
            Runtime::Unknown => None,
            Runtime::Note(_) => {
                // The note's text is never cleaned; its first non-blank line is shown by the display rule.
                let first = text.lines().map(str::trim).find(|l| !l.is_empty());
                first.map(|l| crate::names::to_plain(&crate::names::display_name(l, crate::names::UNTITLED)))
            }
            Runtime::Image(p) => source_or_file(&p.path),
            Runtime::Pdf(p) => source_or_file(&p.path),
            Runtime::Json(p) => (!p.is_inline()).then(|| source_or_file(p.path())).flatten(),
        }
``` Rewrite the doc comment: it names the detail, and the word is the registry's (`Caps::kind_name`).

  `App::accessible_name`:

```rust
    pub(crate) fn accessible_name(&self, id: NodeId) -> Option<String> {
        let node = self.nodes.get(&id)?;
        let word = self.card_kind(id).map_or(Caps::default().kind_name, |kind| kind.caps().kind_name);
        let renamed = self.doc.mirror.name(id); // name rule: drawn through display_name
        let detail = if renamed.is_empty() || self.is_container(id) {
            node.accessible_detail(self.own_text(id), self.source_url(id))
        } else {
            Some(crate::names::to_plain(&crate::names::display_name(renamed, crate::names::UNTITLED)))
        };
        Some(match detail {
            Some(detail) => format!("{word}: {detail}"),
            None => word.to_string(),
        })
    }
```
  (`card_kind` is `None` only without a node, already returned.) Import `crate::caps::Caps` if needed. Update the `App::accessible_name` doc comment.

- [ ] **Step 4: Run**
Run: `cargo test -j 8 -p manifold accessible 2>&1 | tail -8` and `cargo test -j 8 -p manifold node_menu session_sync omnibar 2>&1 | tail -8`
Expected: pass, including the unchanged `node_menu/tests.rs` ("Note: Groceries") and `session_sync` ("Card: Mine") assertions.

- [ ] **Step 5: Commit** (`node: the accessible name's kind word comes from the capability rows`).

### Task 3: Every glyph site reads the table

**Files:**
- Modify: `card/note_body.rs:44`, `card/group_body.rs:30`, `card/download_body.rs:34`, `card/image_body.rs:103`, `card/pdf_body.rs:114`, `card.rs:1459`, `card.rs:1493`, `omnibar.rs` (97-108 delete; 134-144), `popups/dropdowns.rs:138-194` and its tests, `graph_ui/cards.rs:230,232`

**Interfaces:**
- Consumes: `kind_caps` (Task 1).

- [ ] **Step 1: Write the failing test** in `popups/dropdowns.rs` tests (next to `a_closed_tab_row_has_no_hint`). It reads the glyph a row paints:

```rust
    /// The icon a Recently Closed row paints, for each kind (spec Decision 1): the table's glyph, one icon only, and a
    /// tab's favicon (no glyph) instead.
    #[test]
    fn a_closed_row_paints_its_kinds_table_glyph() {
        let painted = |ty: &str, extra: &[(&str, &str)]| -> String {
            let ctx = egui::Context::default();
            let mut fields = serde_json::Map::new();
            fields.insert("type".into(), ty.into());
            for (k, v) in extra {
                fields.insert((*k).into(), (*v).into());
            }
            let node = store::json::JsonNode { id: "n".into(), fields };
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                App::closed_entry_label(ui, None, &node, None);
            });
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect()
        };
        use egui_phosphor::light as i;
        assert_eq!(painted(store::model::TYPE_GROUP, &[("name", "G")]), i::FOLDER);
        assert_eq!(painted(store::model::TYPE_DOWNLOAD, &[("path", "f")]), i::DOWNLOAD_SIMPLE);
        assert_eq!(painted(store::model::TYPE_JAR, &[("path", "f")]), i::COOKIE);
        assert_eq!(painted(store::model::TYPE_PDF, &[("path", "f")]), i::FILE_PDF);
        assert_eq!(painted("application/x-from-the-future", &[]), i::QUESTION);
    }
```
  If `Shape::Text` galleys are not readable that way in this egui version, assert via `ui.min_rect()` width growth instead, or drop this test and say so in the ledger (the table test then carries Decision 1).

- [ ] **Step 2: Run, expect failure** (group paints `FOLDER_SIMPLE`; download and jar paint `BRACKETS_CURLY`):
Run: `cargo test -j 8 -p manifold a_closed_row_paints 2>&1 | tail -15`

- [ ] **Step 3: Implement.**
  - Bodies and `card.rs`: add `use crate::card_kind::kind_caps;` (and `use crate::store::model::Kind;` where `Kind` is not already in scope via `super::*`; the compiler says). Replace `egui_phosphor::light::NOTE_PENCIL` with `kind_caps(Kind::Note).glyph`, `FOLDER` with `kind_caps(Kind::Group).glyph`, `DOWNLOAD_SIMPLE` with `kind_caps(Kind::Download).glyph`, `IMAGE` (line 103 only; `IMAGE_BROKEN` and `FILE_X` stay) with `kind_caps(Kind::Image).glyph`, `FILE_PDF` with `kind_caps(Kind::Pdf).glyph`, `COOKIE` with `kind_caps(Kind::Jar).glyph`, `QUESTION` (line 1493) with `kind_caps(Kind::Unknown).glyph`. `.busy(..)` stays.
  - `omnibar.rs`: delete `other_glyph` (97-108). Replace the body of `others`' closure with:

```rust
            .filter_map(|&id| {
                let caps = kind_caps(app.doc.mirror.kind_of(id));
                if !caps.omni_row {
                    return None;
                }
                Some(OtherInfo { id, label: app.accessible_name(id)?, glyph: caps.glyph, labels: app.label_names(id) })
            })
```
    add `use crate::card_kind::kind_caps;`; drop `Kind` from the `use crate::store::model::{Kind, NodeId}` line if the compiler reports it unused.
  - `popups/dropdowns.rs` `closed_entry_label`: after `str_field`, add `let kind = node.kind();` then `if kind != Some(Kind::Tab) { ui.label(kind_caps(kind.unwrap_or(Kind::Unknown)).glyph); }`. Delete the icon lines in the Group, Note, unknown branches (`FOLDER_SIMPLE`, `NOTE_PENCIL`, `QUESTION`), and collapse the final `match` to `Some(Kind::Tab) => {.. unchanged ..}` and `_ => str_field("path")` (the Image, Pdf and fall-through arms become one). Use `kind` in the existing `node.kind() == ..` comparisons. Update the doc comment's "icon" sentence to say the icon is the kind's table glyph and a tab paints its favicon.
  - `graph_ui/cards.rs`: lines 230 and 232 use `kind_caps(Kind::Note).glyph` and `kind_caps(Kind::Json).glyph` in the `format!`.
  - Do not touch `json_level_ui.rs` (spec R2).

- [ ] **Step 4: Run**
Run: `cargo test -j 8 -p manifold 2>&1 | tail -15`
Expected: all pass (about 1754 before; plus the new ones), no warnings.

- [ ] **Step 5: Verify nothing hand-written is left**
Run: `grep -rn "FOLDER_SIMPLE\|other_glyph" crates/manifold/src` (expect none) and `grep -rn "light::NOTE_PENCIL\|light::FILE_PDF\|light::DOWNLOAD_SIMPLE\|light::COOKIE\|light::BRACKETS_CURLY" crates/manifold/src` (expect only `caps/rows.rs`, the tests, and `json_level_ui.rs`).

- [ ] **Step 6: Commit** (`cards, omnibar, Recently Closed and the New menu read the glyph from the capability rows`).

### Task 4: Docs and the gate

**Files:**
- Modify: `PARKED.md` (delete "One icon table for card kinds"), spec and plan front matter, `docs/runs.md` (via `run.ps1`)

- [ ] **Step 1:** Delete the PARKED.md entry "One icon table for card kinds" (`grep -n "One icon table" PARKED.md`; remove that bullet whole). Grep the repo docs for the old wording: `grep -rn "icon table\|other_glyph" --include=*.md . | grep -v "docs/superpowers/\(specs\|plans\)/2026-10-25"`. Fix any hit that claims the table is still parked or names `other_glyph` as live (the omnibar spec and plan are history; leave them).
- [ ] **Step 2:** Run `cargo fmt --all --check` and the clippy command in Global Constraints. Expected: no output / no warnings.
- [ ] **Step 3:** Build and run the light gate through `~/.claude/runs/run.ps1` as CLAUDE.md "Gate" says (`-Kind gate -Name card-kind-caps-light`, detached, BelowNormal; launcher in a `.ps1` file under the scratchpad). Set the run's headline from the log. Expected: no failing stage; report CPU load and Defender state.
- [ ] **Step 4:** Set the spec and plan `status: done`. Commit docs and the run row (`docs: card-kind caps landed; icon table unparked`).
- [ ] **Step 5:** Final whole-branch review by a fresh reviewer (Sonnet, diff is under ~300 lines), per the executing-plans skill; fix Critical/Important with a failing test first.
