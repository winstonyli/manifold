---
status: draft
---

# Card-kind icon, name and omnibar membership in the capability rows — Design

## Question

A card kind's icon, its kind word ("Note", "PDF") and whether the omnibar lists it are each a hand-written match on the
kind, in 17 places (counted 2026-10-25: 5 card bodies, 2 in `card.rs`, 6 in Recently Closed, 2 menu buttons,
`other_glyph`, `accessible_name`). The matches disagree: Recently Closed draws a group as `FOLDER_SIMPLE` (cards and the omnibar use
`FOLDER`) and draws a download or a cookie jar as `BRACKETS_CURLY`, the Json icon. The capability rows (`caps/rows.rs`)
already hold every other static per-kind fact, keyed by type and tags. This moves those three facts there, so a new
kind needs one row, not a dozen edits. It supersedes PARKED.md "One icon table for card kinds".

## Decisions (conversation 2026-10-25, "go" on the four recommended)

1. **Glyph table.** One `glyph` per kind in `Caps`; every site reads it. Recently Closed changes on screen (group icon,
   download and jar icons) to match the cards.
2. **`omni_row` flag.** A boolean on the six offered kinds, not "has a glyph" (tab, jar and unknown have glyphs and are
   not `Other` rows).
3. **New Note / New JSON buttons** (`graph_ui/cards.rs`) read the registry too.
4. **Card bodies keep their `.busy(..)` wrappers** and their payload-dependent chrome; only the glyph constant moves.

## Rulings (mine, for review)

- **R1. The tab keeps a glyph row (user, 2026-10-25).** `BROWSER` on the `TYPE_TAB` row, inherited by the loaded
  row. Nothing draws it yet (a tab shows its favicon, `MOON` or `WARNING`, by state); it is the table's complete answer
  for "what icon is this kind", so every card kind has a non-default glyph except unknown (R3), and the coverage test
  has no tab exemption.
- **R2. `json_level_ui::header_parts` (`BRACKETS_SQUARE` for an array, `BRACKETS_CURLY` for an object) stays.** I said
  it would read the registry. It picks by the JSON value's shape, not by card kind, so the registry cannot answer it.
- **R3. Unknown needs no row.** `Caps::default()` becomes the unknown card: glyph `QUESTION`, kind word "Card",
  `omni_row` false. A type with no row, a link or a label, and an unknown type all read it. This deletes the
  `TYPE_UNKNOWN` glyph site as a special case.
- **R4. `GraphNode::accessible_name` becomes `accessible_detail`, and `App::accessible_name` joins the word to it.**
  `GraphNode` sees only the `Runtime`, not the type, and a unit test calls it with no `App`. The `Runtime` match keeps
  only the per-kind detail (file name, first line, group name) and returns `Option<String>`; `App::accessible_name(id)`
  reads the word from `card_kind(id).caps().kind_name` and formats `"{word}: {detail}"` (a rename replaces the detail
  for a non-container, as before; this also drops the old split-on-`": "` of the derived string). The loaded and
  sleeping tab words come from the registry by tag (`Tag::Loaded` row), the same tags `card_kind` already sets.
  (Refined 2026-10-25 while planning: the first draft passed the word in as an argument, which made the direct unit
  tests assert a literal against itself.)

## Design

- **`Caps`** gains `glyph: &'static str`, `kind_name: &'static str` and `omni_row: bool`, with `Row` builders
  `.glyph(..)`, `.kind_name(..)` and `.omni_row(..)` and the same `Option` fold in `Registry::caps`. Defaults: `glyph`
  `egui_phosphor::light::QUESTION`, `kind_name` "Card", `omni_row` false.
- **Rows** (`builtin_rows`), added to the existing rows:

  | Row | glyph | kind_name | omni_row |
  | --- | --- | --- | --- |
  | `TYPE_TAB` | `BROWSER` | "Sleeping tab" | false |
  | `TYPE_TAB` + `Loaded` | (inherits) | "Tab" | false |
  | `TYPE_GROUP` | `FOLDER` | "Group" | true |
  | `TYPE_NOTE` | `NOTE_PENCIL` | "Note" | true |
  | `image/*` | `IMAGE` | "Image" | true |
  | `application/pdf` | `FILE_PDF` | "PDF" | true |
  | `application/json` | `BRACKETS_CURLY` | "JSON" | true |
  | `TYPE_DOWNLOAD` | `DOWNLOAD_SIMPLE` | "Download" | true |
  | `TYPE_JAR` | `COOKIE` | "Cookie jar" | false |

  The values are exactly those the cards, the omnibar and `accessible_name` use today.
- **Lookup helper.** `card_kind::kind_caps(kind: Kind) -> Caps`: the registry's answer for a kind with no tags
  (`Kind::type_str()` or `TYPE_UNKNOWN`, as `canonical_type` does). Bodies, the omnibar and Recently Closed each hold a
  `Kind` and no `CardKind`, so they call it. `accessible_name` and anything with an `App` and an id uses `card_kind`.
- **Sites, after:**
  - `card/{note,group,download,image,pdf}_body.rs` and `card.rs` (the jar and unknown identities): the glyph is
    `kind_caps(Kind::X).glyph`; `.busy(..)` stays.
  - `omnibar.rs`: `other_glyph` is deleted. `AppCorpus::others` reads `let caps = kind_caps(kind); caps.omni_row` to
    include a card and `caps.glyph` for its icon.
  - `popups/dropdowns.rs` `closed_entry_label`: one `ui.label(kind_caps(kind).glyph)` before the per-kind text, for
    every kind but a tab (which keeps its favicon). The group, note, unknown, image, pdf and fall-through icon lines
    go. A node with no known kind (`JsonNode::kind()` is `None`) reads the default.
  - `graph_ui/cards.rs`: the New Note and New JSON labels use `kind_caps(Kind::Note).glyph` and
    `kind_caps(Kind::Json).glyph`.
  - `node.rs`: `accessible_name` per R4.
- **Not moved** (payload-dependent, or not a kind fact): `node_card_size`, `card_filter_text`, the `Runtime` accessors,
  the `ContentKind` MIME mapping, the tab body's favicon / `MOON` / `WARNING` identity, error glyphs
  (`FILE_X`, `IMAGE_BROKEN`, `WARNING_CIRCLE`), the Json array glyph (R2), and the `+`, `FOLDER_PLUS` and `FILE_PLUS`
  menu glyphs (actions, not kinds).

## Testing

- `caps/rows.rs`: a table test over `card_kind::fixtures::kinds()` pinning each kind's `glyph`, `kind_name` and
  `omni_row` against the table above (so a kind added to `kinds()` without a row fails), plus that the unknown kind
  equals `Caps::default()` and every other kind's glyph differs from the default.
- `kinds_cover_every_node_type` stays; the table test shares its fixture.
- The existing omnibar tests (kind membership, labels) and the `accessible_name` tests (`node.rs`, `node_menu`,
  `session_sync`) pass unchanged. The `node.rs` tests that call `GraphNode::accessible_name` directly now call
  `accessible_detail` and assert the detail only; the words are pinned by `App`-level tests (the existing ones plus one
  per word: `Tab`, `Sleeping tab`, `Cookie jar`, `Download`, `Group`) and the table test.
- A test that `closed_entry_label`'s icon for a group, download and jar is `kind_caps(..).glyph` (the drift this
  removes), if the function's side-effecting `ui.label` can be read back; otherwise the table test carries it and the
  light gate's icon checks do.
- Gate: `light`. A change that moves icons the gate pins (`FOLDER_SIMPLE`, `BRACKETS_CURLY` in `gate`) updates them in
  the same commit; the plan greps for them first.

## Docs

Delete PARKED.md "One icon table for card kinds". The omnibar spec's `other_glyph` sentence is history (status done);
leave it. If `docs/superpowers/specs/2026-10-04-node-model-design.md` lists the `Caps` fields, add the three; the plan
greps for it.

## Out of scope

Moving any payload-dependent decision into the registry; a registry-driven card draw; renaming kind words (the
accessible names stay byte-for-byte); the Json array glyph.
