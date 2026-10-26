---
status: done
---

# Omnibar rows for every card kind — Design

## Question

Search indexes notes, groups, downloads, images, PDFs and Json cards, but the omnibar can only list open tabs
(`Row::Card`, built from `listed_tab_label`/`listed_tab_url`). Typing a word from a note finds nothing to choose.
This lists every other live card as a row and jumps to it. It closes PARKED.md "Notes, groups and downloads as omnibar
rows (M7)".

## Decisions (conversation 2026-10-25)

1. **Activation (Q1).** Choosing a non-tab card shows its level in the Graph and reveals it, in every host. In the
   Item and Tab views that means switching to the Graph.
2. **Which cards (Q2).** Every live card that is not a tab: note, group, download, image, PDF, Json. Jars and labels
   are not cards. One rule, no per-kind allow-list.
3. **Placement (Q3).** A separate section after the tab rows, with its own `ROW_CAP`. The empty query still offers
   the first open tabs only. Ranking tabs and cards together is parked ("Rank omnibar tab and card rows together").

## Design

- **Row.** One new variant, `Row::Other { id: NodeId, label: String, snippet: Option<String> }`. It has no url or jar:
  only tabs have them. `Row::Card` is unchanged.
- **Corpus.** `Corpus` gains `others() -> Vec<OtherInfo>` (`id`, `label`, `labels`), one entry per live non-tab card in
  `node_order`. The label is `App::accessible_name(id)` ("Note: first line", "Group: Research", "Download: a.zip"): the
  one naming rule the app already has, so there is no new one. The Graph host excludes nothing; the popup's `exclude`
  applies to both lists.
- **Matching** (`rows`): the same rules as tabs. A plain word is a case-insensitive substring of the label or a label
  name; `#tags` filter by label prefix (`labels_match`); with no plain word, only the `#label` filter applies and
  only the tag-matching cards are listed, as for tabs. Page-text hits (`text_hits`) that are not tabs become `Other`
  rows with their `snippet`, after the label matches, skipping ids already listed. Empty query: no `Other` rows.
- **Order.** Tabs, then Other, then history, settings, commands, typed, suggestions. Each section is at most
  `ROW_CAP`.
- **Activation** (`omni_activate`): `Row::Other { id }` does `show_level_of(id)`, sets `view_mode = Graph` if it is
  not, sets `camera.reveal = Some(id)`, and closes the bar. Nothing else per kind.
- **Drawing.** The icon comes from the node's runtime kind through one small table; the text is the label, plus
  ` — snippet` when there is one. `debug_export` lists the row as `card:<id>` (tabs stay `tab:<id>`).
- **Deleted.** The PARKED.md entry for M7.

## Out of scope

Ranking across sections (parked); opening a card in place per kind; jars and labels as rows; PDF text (parked).

## Tests

Fake-corpus unit tests in `omnibar/rows.rs`: a note matches by label word and by `#label`; the section sits after tabs
and before history; the cap holds; an empty query offers no `Other`; a text hit on a non-tab card carries its snippet
and is not duplicated; the popup `exclude` removes the excluded card from both lists. An `app_on` test: activating an
`Other` row for a note on another level shows that level and sets `camera.reveal`, from the Item view it also switches
to the Graph.

## Review Focus

- The kind word is part of the label, so typing "note", "group" or "download" lists every card of that kind. Intended (a
  kind filter for free); a note whose first line is empty is just "Note". Expected: it
  lists, harmlessly.
- A closed or removed card between `rows` and Enter: activation does nothing and closes the bar.
- A very large document: `others()` is built per keystroke like `cards()`; no new cost class.
