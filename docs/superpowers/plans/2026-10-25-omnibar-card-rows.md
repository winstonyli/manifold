---
status: draft
---

# Omnibar rows for every card kind Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The omnibar lists notes, groups, downloads, images, PDFs and Json cards as rows after the tab rows, and choosing one shows its level in the Graph and reveals it.

**Architecture:** One new `Row::Other { id, label, glyph, snippet }`, fed by one new `Corpus::others()`. `rows()` builds the section with the same matching rules as tabs. One table, `other_glyph(Kind) -> Option<&'static str>`, says both which kinds are offered and their icon. One activation path, `App::reveal_in_graph`, serves every host.

**Tech Stack:** Rust, egui. Crate `manifold` (`crates/manifold/src`).

**Spec:** `docs/superpowers/specs/2026-10-25-omnibar-card-rows-design.md`

## Global Constraints

- Work on `main`. Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never push, tag or bump.
- `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold --all-targets -- -D warnings` must pass; no `--no-verify`. Width 120.
- A test never reads the real `%APPDATA%\manifold`: use `App::default()` or `store::testutil::temp_dir`.
- `Row::Card` is unchanged (tabs only). Each section is at most `ROW_CAP` (5). An empty query offers tabs only.
- A python call, if any is unavoidable, is `python -P f.py < /dev/null`; edit files with the Edit tool.

## Review Focus

1. A note whose first line is empty is labelled "Note"; "note" lists it, harmlessly (test: `an_empty_note_is_labelled_note`).
2. A card closed or deleted between `rows` and Enter: activation does nothing and the bar closes (test: `activating_a_card_that_has_gone_does_nothing_and_closes_the_bar`).
3. A card matching by label and by page text is listed once (test: `a_text_hit_on_a_non_tab_card_carries_its_snippet_and_is_listed_once`).
4. A renamed note, group or image is listed and found by its stored name (test: `a_renamed_card_is_found_by_its_stored_name`).
5. Activating from the image viewer leaves it for the Graph with the reveal set (test: `an_other_row_chosen_in_the_image_viewer_returns_to_the_graph`).

## Decisions made in this plan (not in the spec)

- `Row::Other` and `OtherInfo` carry `glyph: &'static str`, so `row_icon` stays a pure function of the row and needs no `App`. The spec said the icon "comes from the node's kind through one small table"; it still does, the table is read in `AppCorpus::others`.
- The table is keyed on the stored `Kind` (`mirror.kind_of`), not the runtime, and is also the membership rule: a kind outside it (tab, jar, link, label, unknown) is not offered. The `match` is exhaustive, so a new `Kind` forces a decision there.

---

### Task 1: `Row::Other` end to end

**Files:**
- Modify: `crates/manifold/src/omnibar/rows.rs` (enum, `OtherInfo`, `Corpus::others`, `rows`, tests)
- Modify: `crates/manifold/src/omnibar.rs` (`other_glyph`, `AppCorpus::others`, `reveal_in_graph`, `omni_activate`, tests)
- Modify: `crates/manifold/src/omnibar/ui.rs` (`row_icon`, `row_text`, test)
- Modify: `crates/manifold/src/debug_export.rs:61`

**Interfaces:**
- Produces: `Row::Other { id: NodeId, label: String, glyph: &'static str, snippet: Option<String> }`; `OtherInfo { id: NodeId, label: String, glyph: &'static str, labels: Vec<String> }`; `Corpus::others(&self) -> Vec<OtherInfo>`; `fn other_glyph(kind: Kind) -> Option<&'static str>` (in `omnibar.rs`); `App::reveal_in_graph(&mut self, id: NodeId)` (private to `omnibar.rs`).

- [ ] **Step 1: Write the failing tests in `rows.rs`**

In `mod tests`, extend `Fake` and add the helper, the impl method and the `kinds` arm:

```rust
    #[derive(Default)]
    struct Fake {
        cards: Vec<(u64, &'static str, &'static str, bool)>,
        /// `(id, label, label names)` of the live cards that are not tabs.
        others: Vec<(u64, &'static str, Vec<&'static str>)>,
        history: Vec<(&'static str, &'static str)>,
        text_hits: Vec<(u64, &'static str)>,
        commands: bool,
    }

    fn other(n: u64, label: &'static str) -> (u64, &'static str, Vec<&'static str>) {
        (n, label, Vec::new())
    }

    fn other_ids(shown: Vec<Row>) -> Vec<NodeId> {
        shown.into_iter().filter_map(|r| if let Row::Other { id, .. } = r { Some(id) } else { None }).collect()
    }
```

Inside `impl Corpus for Fake` add:

```rust
        fn others(&self) -> Vec<OtherInfo> {
            self.others
                .iter()
                .map(|(n, label, names)| OtherInfo {
                    id: id(*n),
                    label: (*label).into(),
                    glyph: "g",
                    labels: names.iter().map(|s| (*s).into()).collect(),
                })
                .collect()
        }
```

and in `kinds()` add `Row::Other { .. } => "other",`. Then add the tests:

```rust
    #[test]
    fn other_rows_sit_after_tab_rows_and_before_history() {
        let fake = Fake {
            cards: vec![(1, "Rust book", "https://doc.test/", false)],
            others: vec![other(5, "Note: rust ideas")],
            history: vec![("Rust blog", "https://blog.test/")],
            ..Default::default()
        };
        let mut order = kinds(&rows(&fake, "rust", &[]));
        order.dedup();
        order.truncate(3);
        assert_eq!(order, ["card", "other", "history"]);
    }

    #[test]
    fn a_card_matches_by_label_word_by_label_name_and_by_hash_label() {
        let fake = Fake {
            others: vec![(5, "Note: milk", vec!["Urgent"]), other(6, "Group: Cheese")],
            ..Default::default()
        };
        let ids = |text: &str| other_ids(rows(&fake, text, &[]));
        assert_eq!(ids("milk"), [id(5)]);
        assert_eq!(ids("urgent"), [id(5)], "a label name matches");
        assert_eq!(ids("#urg"), [id(5)], "only the tag filters");
        assert!(ids("#urg chee").is_empty(), "a tag and a plain word are both needed");
        assert_eq!(ids("group"), [id(6)], "the kind word is part of the label");
        assert!(ids("zebra").is_empty());
    }

    #[test]
    fn other_rows_are_capped_and_an_empty_text_offers_none() {
        let others = (0..ROW_CAP as u64 + 3).map(|n| other(n, "Note: a")).collect();
        let fake = Fake { others, ..Default::default() };
        assert_eq!(other_ids(rows(&fake, "a", &[])).len(), ROW_CAP);
        assert!(rows(&fake, "  ", &[]).is_empty());
    }

    #[test]
    fn a_text_hit_on_a_non_tab_card_carries_its_snippet_and_is_listed_once() {
        let fake = Fake {
            others: vec![other(5, "Note: other words"), other(6, "Note: borrow list")],
            text_hits: vec![(5, "…the borrow checker…"), (6, "…borrow…")],
            ..Default::default()
        };
        let shown: Vec<Row> = rows(&fake, "borrow", &[]).into_iter().filter(|r| matches!(r, Row::Other { .. })).collect();
        assert_eq!(shown.len(), 2, "the label match and the text hit, not three");
        assert_eq!(shown[0], Row::Other { id: id(6), label: "Note: borrow list".into(), glyph: "g", snippet: None });
        assert_eq!(
            shown[1],
            Row::Other {
                id: id(5),
                label: "Note: other words".into(),
                glyph: "g",
                snippet: Some("…the borrow checker…".into())
            }
        );
    }
```

- [ ] **Step 2: Write the failing tests in `omnibar.rs`**

In `mod tests` (it already imports `add`, `app_on`, `live_tab`, `sleeping_tab`, `Kind`; add `add_group`, `add_note` to the `session_sync::tests` import, and `OtherInfo` is reached through `rows::`):

```rust
    fn others(app: &App) -> Vec<(NodeId, String)> {
        AppCorpus { app, exclude: None }.others().into_iter().map(|o| (o.id, o.label)).collect()
    }

    fn other_ids(app: &App, host: Host) -> Vec<NodeId> {
        app.omni_rows(host).into_iter().filter_map(|r| if let Row::Other { id, .. } = r { Some(id) } else { None }).collect()
    }

    #[test]
    fn every_card_kind_but_a_tab_or_a_jar_is_an_other_card_named_by_its_accessible_name() {
        let mut app = App::default();
        let note = add_note(&mut app, "buy milk\nmore");
        let group = add_group(&mut app, "Research");
        add(&mut app, Kind::Jar, Runtime::Jar);
        tab(&mut app, "https://a.test/", "Alpha");
        assert_eq!(others(&app), [(note, "Note: buy milk".to_owned()), (group, "Group: Research".to_owned())]);
        let corpus = AppCorpus { app: &app, exclude: Some(note) };
        assert_eq!(corpus.others().len(), 1, "the excluded card is left out");
    }

    #[test]
    fn an_empty_note_is_labelled_note() {
        let mut app = App::default();
        let note = add_note(&mut app, "");
        assert_eq!(others(&app), [(note, "Note".to_owned())]);
        app.omni.text = "note".into();
        assert_eq!(other_ids(&app, Host::Graph), [note], "listed, harmlessly");
    }

    #[test]
    fn a_renamed_card_is_found_by_its_stored_name() {
        let mut app = App::default();
        let note = add_note(&mut app, "buy milk");
        app.edit_name(note, "Groceries");
        assert_eq!(others(&app), [(note, "Note: Groceries".to_owned())]);
        app.omni.text = "grocer".into();
        assert_eq!(other_ids(&app, Host::Graph), [note]);
    }

    #[test]
    fn enter_on_an_other_row_in_graph_switches_level_and_sets_reveal() {
        let mut app = app_on(&crate::store::testutil::temp_dir("omni-other").join("session.mnf"), 1);
        let group = add_group(&mut app, "G");
        let inner = add_note(&mut app, "kept");
        app.edit_group(inner, Some(group));
        app.view_mode = ViewMode::Graph;
        assert_eq!(app.current_group, None);
        let row = app.omni_rows_for_test_other(inner);
        app.omni_activate(row, Host::Graph);
        assert_eq!(app.current_group, Some(group));
        assert_eq!(app.camera.reveal, Some(inner));
        assert_eq!(app.view_mode, ViewMode::Graph);
    }

    #[test]
    fn an_other_row_chosen_in_the_item_view_hides_the_tab_and_shows_the_graph() {
        let mut app = App::default();
        let (tab_id, _peer) = live_tab(&mut app, crate::ViewKind::Servo, "https://a.test/");
        let note = add_note(&mut app, "kept");
        app.focused = Some(tab_id);
        app.view_mode = ViewMode::Item;
        let row = app.omni_rows_for_test_other(note);
        app.omni_activate(row, Host::Item);
        assert_eq!(app.view_mode, ViewMode::Graph);
        assert_eq!(app.camera.reveal, Some(note));
    }

    #[test]
    fn an_other_row_chosen_in_the_image_viewer_returns_to_the_graph() {
        let mut app = App::default();
        let note = add_note(&mut app, "kept");
        app.viewed_image = Some(note);
        app.view_mode = ViewMode::Item;
        let row = app.omni_rows_for_test_other(note);
        app.omni_activate(row, Host::Item);
        assert_eq!((app.view_mode, app.viewed_image), (ViewMode::Graph, None));
        assert_eq!(app.camera.reveal, Some(note));
    }

    #[test]
    fn activating_a_card_that_has_gone_does_nothing_and_closes_the_bar() {
        let mut app = App::default();
        let gone = app.alloc_node_id();
        app.view_mode = ViewMode::Settings;
        app.omni.text = "x".into();
        app.omni_activate(Row::Other { id: gone, label: "Note".into(), glyph: "g", snippet: None }, Host::Settings);
        assert_eq!(app.view_mode, ViewMode::Settings);
        assert_eq!(app.camera.reveal, None);
        assert!(app.omni.text.is_empty() && !app.omni.open, "the bar closed");
    }
```

The tests build the row to activate with a one-line helper in the same `mod tests` (it needs no production code):

```rust
    impl App {
        fn omni_rows_for_test_other(&self, id: NodeId) -> Row {
            Row::Other { id, label: self.accessible_name(id).unwrap(), glyph: "g", snippet: None }
        }
    }
```

(`impl App` inside the `tests` module is legal: same crate. `edit_name` is `session_sync/edits.rs:79`.)

- [ ] **Step 3: Write the failing test in `omnibar/ui.rs`**

In `mod tests`, next to `a_page_text_hit_row_says_the_snippet_and_the_jar`:

```rust
    #[test]
    fn an_other_row_says_its_label_and_the_snippet_and_draws_its_glyph() {
        use crate::store::model::ActorId;
        let row = |snippet: Option<&str>| Row::Other {
            id: NodeId { actor: ActorId(1), counter: 1 },
            label: "Note: milk".into(),
            glyph: egui_phosphor::light::NOTE_PENCIL,
            snippet: snippet.map(str::to_owned),
        };
        assert_eq!(row_text(&row(None)), "Note: milk");
        assert_eq!(row_text(&row(Some("buy some"))), "Note: milk \u{2014} buy some");
        assert_eq!(row_icon(&row(None)), Identity::Glyph { glyph: egui_phosphor::light::NOTE_PENCIL, color: None });
    }
```

- [ ] **Step 4: Run to verify they fail**

Run: `cargo test -j 8 -p manifold omnibar 2>&1 | tail -30`
Expected: FAIL to compile: `no variant named Other` / `cannot find type OtherInfo`.

- [ ] **Step 5: Implement `rows.rs`**

Add the variant (keep `#[rustfmt::skip]`):

```rust
    /// A live card that is not a tab (a note, group, download, image, PDF or Json): chosen, it is shown in the Graph.
    Other { id: NodeId, label: String, glyph: &'static str, snippet: Option<String> },
```

Add after `CardInfo`:

```rust
/// One live card that is not a tab the omnibar may offer.
pub(crate) struct OtherInfo {
    pub(crate) id: NodeId,
    /// What the card is called (`App::accessible_name`): "Note: first line", "Group: Research".
    pub(crate) label: String,
    /// The icon of its kind.
    pub(crate) glyph: &'static str,
    /// The names of the card's live labels.
    pub(crate) labels: Vec<String>,
}
```

Add to `trait Corpus`:

```rust
    /// The live cards that are not tabs, in document order.
    fn others(&self) -> Vec<OtherInfo>;
```

Add the helper before `rows`:

```rust
/// The rows for the live cards that are not tabs: the label matches, then the page-text `hits`, at most `ROW_CAP` in
/// all. Nothing for an empty `text`.
fn other_rows(corpus: &impl Corpus, text: &str, tags: &[String], plain: &str, hits: &[NodeId]) -> Vec<Row> {
    if text.is_empty() {
        return Vec::new();
    }
    let others = corpus.others();
    let mut out: Vec<Row> = others
        .iter()
        .filter(|o| labels_match(tags, &o.labels))
        .filter(|o| {
            let mut haystacks: Vec<&str> = vec![&o.label];
            haystacks.extend(o.labels.iter().map(String::as_str));
            contains(plain, &haystacks)
        })
        .take(ROW_CAP)
        .map(|o| Row::Other { id: o.id, label: o.label.clone(), glyph: o.glyph, snippet: None })
        .collect();
    for &id in hits {
        if out.len() >= ROW_CAP {
            break;
        }
        let Some(o) = others.iter().find(|o| o.id == id && labels_match(tags, &o.labels)) else { continue };
        if out.iter().any(|r| matches!(r, Row::Other { id: seen, .. } if *seen == id)) {
            continue;
        }
        out.push(Row::Other { id, label: o.label.clone(), glyph: o.glyph, snippet: corpus.snippet(id, plain) });
    }
    out
}
```

In `rows`, compute the hits once and use them for both sections. Replace the tab section's `if !plain.is_empty() { for id in corpus.text_hits(&plain) {` with:

```rust
    let hits = if plain.is_empty() { Vec::new() } else { corpus.text_hits(&plain) };
    for &id in &hits {
```

(dropping the `if !plain.is_empty()` wrapper and one closing brace; the loop body is unchanged), then, directly before the `// Only `#label` words` comment, add:

```rust
    out.extend(other_rows(corpus, text, &tags, &plain, &hits));
```

Update the doc comment of `rows`: "open cards, the other cards, history, ...". Update the module's one-line description is unchanged.

- [ ] **Step 6: Implement `omnibar.rs`**

Add `OtherInfo` to the `use rows::{CardInfo, Corpus};` line (`use rows::{CardInfo, Corpus, OtherInfo};`) and `use crate::store::model::{Kind, NodeId};`. Add before `AppCorpus`:

```rust
/// The icon of a card that is not a tab, and so whether the omnibar offers it as a row: one table for both. A jar
/// holds cookies and is no card to open, a card of a type this build does not know has no name to search, and a link
/// or label is never a card. The match is exhaustive: a new `Kind` must be placed here.
fn other_glyph(kind: Kind) -> Option<&'static str> {
    use egui_phosphor::light as icon;
    Some(match kind {
        Kind::Note => icon::NOTE_PENCIL,
        Kind::Group => icon::FOLDER,
        Kind::Download => icon::DOWNLOAD_SIMPLE,
        Kind::Image => icon::IMAGE,
        Kind::Pdf => icon::FILE_PDF,
        Kind::Json => icon::BRACKETS_CURLY,
        Kind::Tab | Kind::Jar | Kind::Link | Kind::Label | Kind::Unknown => return None,
    })
}
```

In `impl Corpus for AppCorpus` add:

```rust
    fn others(&self) -> Vec<OtherInfo> {
        let app = self.app;
        app.node_order
            .iter()
            .filter(|&&id| Some(id) != self.exclude)
            .filter_map(|&id| {
                let glyph = other_glyph(app.doc.mirror.kind_of(id))?;
                Some(OtherInfo { id, label: app.accessible_name(id)?, glyph, labels: app.label_names(id) })
            })
            .collect()
    }
```

Add in `impl App` (after `omni_open_url`):

```rust
    /// Shows card `id` in the Graph on its level and eases the camera to it, leaving the Item or Settings view (and the
    /// image viewer) for the Graph. A card that has since gone does nothing.
    fn reveal_in_graph(&mut self, id: NodeId) {
        if !self.nodes.contains_key(&id) {
            return;
        }
        if self.showing_tab_view() {
            self.hide_focused_tab();
        }
        self.viewed_image = None;
        self.view_mode = ViewMode::Graph;
        self.show_level_of(id);
        self.camera.reveal = Some(id);
        self.save_session();
    }
```

In `omni_activate`, add the arm after `Row::Card` and update the doc comment ("a tab is jumped to in the Graph and opened elsewhere, any other card is shown in the Graph from every host"):

```rust
            Row::Other { id, .. } => self.reveal_in_graph(id),
```

- [ ] **Step 7: Implement `ui.rs` and `debug_export.rs`**

`row_icon`: add `Row::Other { glyph, .. } => glyph,` (the match yields `&str` glyphs; `Row::Other`'s is `&'static str`, so write the arm as `Row::Other { glyph, .. } => glyph,`). Fix the doc comment: "The icon of a row that is not a tab (`Row::Card` is drawn with the tab's identity)."

`row_text`: add before `Row::History`:

```rust
        Row::Other { label, snippet: Some(snippet), .. } => format!("{label} \u{2014} {snippet}"),
        Row::Other { label, .. } => label.clone(),
```

`debug_export.rs`: add `crate::omnibar::Row::Other { id, .. } => format!("card:{id}"),` after the `Row::Card` arm.

- [ ] **Step 8: Run to verify they pass**

Run: `cargo test -j 8 -p manifold omnibar 2>&1 | tail -15`
Expected: PASS, every `omnibar` test (the new ones and the existing ones). If `ui.rs`'s `omni_rows_ui` match reports `Row::Other` falling to `other => (row_icon(other), None)`, that is intended: the glyph rides on the row.

- [ ] **Step 9: Whole suite, fmt, clippy**

Run: `cargo fmt --all && cargo test -j 8 -p manifold 2>&1 | tail -6 && cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings 2>&1 | tail -5`
Expected: all tests pass (1741 before, plus the new ones); clippy clean. `git diff --stat` shows only the five files above.

- [ ] **Step 10: Commit**

```bash
git add crates/manifold/src/omnibar.rs crates/manifold/src/omnibar/rows.rs crates/manifold/src/omnibar/ui.rs crates/manifold/src/debug_export.rs
git commit -m "omnibar: rows for notes, groups, downloads, images, PDFs and Json cards

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Docs, parked work and the gate

**Files:**
- Modify: `PARKED.md` (delete the M7 entry near line 98; add the glyph-table entry)
- Modify: `README.md:216` and `README.md:392`
- Modify: `ROADMAP.md:9`
- Modify: the spec and this plan (`status: done`)

- [ ] **Step 1: Docs**

1. `PARKED.md`: delete the whole bullet "Notes, groups and downloads as omnibar rows (M7)". Add after the "Rank omnibar tab and card rows together" entry:

```markdown
- **One icon table for card kinds.** *What:* the glyph of each card kind is written in the card bodies (`card/note_body.rs`, `group_body.rs`, `download_body.rs`, `image_body.rs`, `pdf_body.rs`), `popups/dropdowns.rs` and now `omnibar.rs` `other_glyph`; one table keyed on `Kind` would replace them. *Why parked:* a refactor of five painters, not needed for the omnibar rows. *Trigger:* a sixth place needing a kind icon, or an icon change that must be made in several files. *Evidence:* `grep -rn "light::NOTE_PENCIL" crates/manifold/src`, 2026-10-25. *Status:* parked.
```

2. `README.md:216` (Omnibar paragraph): after "open cards across *every* level (title, url or page text, see Search)" add ", then the other cards (notes, groups, downloads, images, PDFs and Json, named as a screen reader reads them, `Row::Other`)". Replace "Jumping to a card calls `focus_tab` directly (no node to animate from when the card is in another group)." with "Jumping to a tab calls `focus_tab` directly (no node to animate from when the card is in another group); choosing any other card shows its level in the Graph, from every host, and reveals it (`reveal_in_graph`)."
3. `README.md:392`: "the dropdown lists up to five cards with a snippet of the page text" becomes "the dropdown lists up to five tabs and up to five other cards, each with a snippet of the page text when that is what matched".
4. `ROADMAP.md:9`: delete "notes, groups and downloads as omnibar rows, " from the search-leftovers line.
5. Spec and plan front matter: `status: done`.

Run: `grep -rn "omnibar rows\|M7" PARKED.md ROADMAP.md README.md | head` and confirm nothing stale (the "Rank omnibar tab and card rows together" entry is the intended survivor).

- [ ] **Step 2: Gate**

Run: `target/debug/gate.exe tier` (after `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`) and run the advised gate through `~/.claude/runs/run.ps1` per CLAUDE.md "Gate" (a `.ps1` launcher with plain quotes, detached, BelowNormal). Report CPU load and Defender state. Two stages have flaked once each (`trail`, `page-text`): rerun a failing stage alone before calling it a failure, and say so.

- [ ] **Step 3: Commit**

```bash
git add PARKED.md README.md ROADMAP.md docs
git commit -m "docs: omnibar card rows shipped; one icon table parked

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```
