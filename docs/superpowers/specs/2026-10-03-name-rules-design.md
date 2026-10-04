# Name rules: design, 2026-10-03

Status: built 2026-10-03 (plan `docs/superpowers/plans/2026-10-03-name-rules.md`; deviations in its Outcome). Closes the Open question in `2026-10-03-per-tab-jars-design.md` ("Name rules ... should be designed once for every user-editable name"). Decisions from the user on 2026-10-03: protect against **display breakage and spoofing** (not uniqueness or reserved words); the rule is **applied on input and at display**, never by rewriting stored names; hidden characters are **revealed as placeholders**, not dropped; **64 characters** and an **"Untitled"** fallback (assumptions the user accepted by proceeding). Revised the same day, by the user's decision: emoji sequences keep their joiners, selectors and tags; bookmark folder names and a note's first line are covered; `display_name`'s allocation is stated exactly.

## Goal

Every name the user can edit (group names, jar names, the saved-session name, bookmark folder names) follows one rule, and so does a note's first line where it stands in as the note's title. A name can neither break a row, chip or card layout (newlines, huge length) nor imitate UI that is drawn differently from text (invisible characters, bidi overrides, a typed "Work · #3f2a"). Today each editor only trims, and names from sync, import or older data are unchecked (`session_sync.rs` group names, `jar_defs.rs` `materialize_jar`).

## Out of scope

Uniqueness (identity is the node id), reserved words (the built-in jar is distinguished by its styled label), confusable letters from other alphabets (needs a confusables table), the Control Pictures glyphs (␉ ␊ ␀: none of the loaded fonts has U+2400-243F, nor U+FFFD; revisit if a font is ever added or switched), tab, card and bookmark titles, which come from pages, and the text of a note. A note's body is user content: it is never cleaned or rewritten, and only its first line, where it is shown as the note's title, goes through the display rule.

## `names.rs`

One module holds both rules.

### `clean_name(&str) -> Cow<'_, str>` (input rule)

Applied when the user commits a name. Returns `Cow::Borrowed` when the text is already clean (one scan, no allocation) and an owned string otherwise.

- Drops hidden characters: those whose Unicode general category is `Cc` (controls, including tab and newline), `Cf` (format: zero-width space, joiners, word joiner, BOM, soft hyphen, bidi marks and overrides, tag characters), `Cs`, `Co` or `Cn`, and variation selectors (`Mn` U+FE00-FE0F, U+E0100-E01EF), except inside an emoji sequence (below).
- Turns every other whitespace character (`Zs`, `Zl`, `Zp`: non-breaking, ideographic, thin spaces) into a plain space, collapses runs of whitespace into one, and trims both ends.
- Caps the result at 64 grapheme clusters, by cutting at a cluster boundary.
- Idempotent: `clean_name(clean_name(s)) == clean_name(s)`.
- Leaves ordinary letters in any script, emoji (sequences included), punctuation and case alone.

**Emoji sequences.** An emoji here is an `Extended_Pictographic` character that is assigned (the property reserves unassigned code points in the emoji blocks; those stay hidden). Three kinds of hidden character are kept, each only in its place in a sequence, and dropped anywhere else:

- U+200D ZERO WIDTH JOINER, when an emoji comes before it and an emoji directly after it. "Before" follows the Unicode grapheme rule GB11: the emoji may carry skin-tone modifiers (U+1F3FB-1F3FF), a kept selector or a flag's tags between it and the joiner, so 👩‍💻, 👩🏽‍💻, ❤️‍🔥 and 🏳️‍🌈 keep their joiners. Anything else between them, including a space or another hidden character, drops the joiner.
- U+FE0F and U+FE0E (emoji and text presentation), directly after an emoji (☺️), or between a keycap base (`0`-`9`, `#`, `*`) and U+20E3 COMBINING ENCLOSING KEYCAP (1️⃣, #️⃣). Keycaps are kept because they are standard emoji a name may well use, and the check is one comparison. Any other variation selector, U+FE00-FE0D and U+E0100-E01EF included, stays hidden.
- Tag characters U+E0020-E007F, only as the tags of a subdivision flag: 🏴 U+1F3F4, then one or more tags U+E0020-E007E, then the cancel tag U+E007F (🏴󠁧󠁢󠁥󠁮󠁧󠁿). Tags without the flag, without the cancel tag, or after it are hidden.

Skin-tone modifiers (`Sk`) and U+20E3 (`Me`) are not hidden characters, so they stay wherever they are. Every kept character's context is made only of characters the rule keeps, so cleaning cannot strand one, and idempotence holds. The 64-cluster cap counts a sequence as one cluster, since UAX #29 makes each of these sequences a single grapheme cluster. A cut that ends at a joiner trims it, as it trims a space (only possible if the segmenter's emoji data were older than the property data).

**Data.** General categories and `Extended_Pictographic` come from `icu_properties` 2.3.0 (its `compiled_data` lookups, `GeneralCategory::for_char` and `ExtendedPictographic::for_char`), which `url`'s `idna` already builds with the same feature, so it adds no crate and no build. Grapheme clusters come from `unicode-segmentation` 1.13.3 (Unicode 17, rule GB11 included), also already in `Cargo.lock`. `unicode-segmentation`'s own `Extended_Pictographic` table is private, so it cannot serve the per-character checks. `crates/manifold/Cargo.toml` gains both as direct dependencies at the locked versions. `unicode-general-category`, named in the first version of this spec, is no longer needed: `icu_properties` has the same categories.

### `display_name(&str, fallback: &str) -> Vec<NameSeg<'_>>` (display rule)

Applied wherever a stored name is drawn. `NameSeg` is `Plain(&str)` or `Hidden(char)`.

- A character `clean_name` would drop becomes `Hidden(c)`: the same rule, so a joiner, selector or tag inside an emoji sequence stays in the `Plain` text and one outside a sequence (a joiner between two letters, a selector after a letter, a lone joiner) is revealed. Odd whitespace and whitespace runs become a single plain space; the name is trimmed and capped at 64 graphemes, each `Hidden` counting as one, so placeholders cannot be used to dodge the cap.
- An empty result yields the per-kind fallback ("Untitled") as one `Plain` segment.
- `to_plain(&[NameSeg]) -> String` renders `Hidden('\u{200B}')` as `<U+200B>` for plain-text sites (tooltips, confirm dialogs, debug output).
- Cost: for a clean name, `display_name` makes one small allocation, the `Vec` holding a single `Plain` segment that borrows the whole name; the name itself is not copied. `to_plain` allocates only where a string is needed anyway. The `Vec` API stays: the segments are what the rich sites draw.
- Rich-text sites paint `Hidden(c)` as `⟦200B⟧` (uppercase hex, at least four digits) in the dim monospace marker style from jar labels (`JarLabel`), with a hover naming the character. `⟦ ⟧` are in Hack (the monospace fallback); the plain form is pure ASCII. A test asserts that every character of the placeholder form is in the monospace font, so a font change cannot turn it into tofu unnoticed.

## Where each rule applies

| Site | Rule |
|---|---|
| Group rename commit (`node_menu.rs` `CommitRename`), jar create (`create_jar`), jar rename (`rename_jar`, Settings), saved-session name (`settings_page.rs`) | `clean_name`; an empty result behaves as today (rename keeps the old name, create is ignored) |
| Bookmark folder commit (`popups.rs`, the folder field under a bookmark) | `clean_name`; an empty result means no folder, as a blank field does today |
| Export Group's suggested file name | `clean_name`, or "Untitled" when nothing is left. Never the placeholder form: Windows forbids `<` and `>` in file names |
| Every place a group, jar, session or bookmark folder name is drawn: cards, menus, chips, Settings rows, Tab view, the Bookmarks popup's folder headers and folder labels, Recently Closed, confirm texts, tooltips, debug output | `display_name` (+ `to_plain` for plain text). A group whose stored name is empty, or holds nothing to show, is drawn "Untitled" (screen-reader name "Group: Untitled") |
| A note's first non-blank line where it stands in as the note's title: the screen-reader name (`accessible_name`) and the Recently Closed row | `display_name` at display only, on the trimmed line (+ `to_plain` for the screen reader). The note's text is never cleaned: it is the note's content, edited in the note's own field. A tab inside the line therefore shows as a placeholder in the title, and the title is capped at 64 units like any name, in `accessible_name` as in the Recently Closed row. (The note card's header reads "Note", not the first line.) |
| `materialize` of remote or imported names, `session_sync` group names, older data | Not rewritten. The stored value stays as received (no sync churn between builds); display handles it |
| `jar_label` clash detection (`named_jar_label`) | Compares the `clean_name` form of the stored names (trimmed, case-insensitive as today), so a hidden character cannot make two same-looking jars look distinct |

The Settings jar edit field and the bookmark folder field still edit the stored name, never the label, and never the placeholders.

## Guard against missed sites

A test in the style of `profile::tests` fails when a source file outside `names.rs` and the store layer reads a group, jar or session `.name` straight into UI text without `display_name`. The list of allowed files is explicit in the test, so adding a site means adding it there. A read that is drawn some other way, or not drawn, is marked `// name rule: <why>` at the end of its line or as a comment on the line just above (where a trailing comment would overflow the line or `cargo fmt` would move it). Bookmark folders (`.folder`), note text (`.text`) and Recently Closed entries (JSON fields) are not `.name` reads, so the guard cannot see those sites; their tests cover them instead.

## Testing

Unit tests, no profile access:
- each stripped class (controls, zero-width, bidi, variation selectors, odd spaces), run collapsing, trimming;
- emoji sequences: 👩‍💻, 👩🏽‍💻, ❤️‍🔥, ☺️, 1️⃣ and 🏴󠁧󠁢󠁥󠁮󠁧󠁿 are already clean and kept as they are; a joiner between two letters, a lone joiner, a joiner with no emoji on one side, a bare VS16 after a letter, and tags without the flag or the cancel tag are dropped (revealed by `display_name`);
- idempotence, including the emoji cases;
- the grapheme-boundary cap: 65 of a cluster (é, 👍🏽, 🇯🇵, 👩‍💻, 🏴󠁧󠁢󠁥󠁮󠁧󠁿) are cut to 64, so a sequence counts as one cluster and is never cut mid-way;
- `Cow::Borrowed` for a clean name, placeholder segments and their plain form (`<U+200B>`), the fallback, placeholder count against the cap, placeholder characters present in Hack;
- a bookmark folder committed clean (nothing left means no folder) and drawn with placeholders; a note's first line shown by the display rule in the screen-reader name and the Recently Closed row, with the note's text unchanged;
- jar-label clashing with hidden characters, and the grep guard.

A live check on a throwaway profile (`scripts/drive.sh`, `EGUITEXT`): a group named with a pasted zero-width space and a typed "Work · #3f2a" jar look different from a genuine marker.

## Migration

None. Existing names are valid; names with hidden characters start showing placeholders.

## Open questions

- The cap is 64 graphemes everywhere. A per-kind cap (a group title may want more room than a chip) can wait for a case that needs it.
- Whether to add the Control Pictures glyphs for common controls if a font that has them is ever added or switched.
- Whether `clean_name` should also run on a name typed into the jar-name field per keystroke (so a paste shows the cap at once) rather than only on commit.
- egui draws a kept emoji sequence piece by piece. It has no text shaping, so 👩‍💻 shows as 👩 and 💻 side by side (epaint draws U+200D as nothing). None of the loaded fonts has U+FE0E, U+FE0F, the tag characters, 🏴 or the skin-tone modifiers (checked against the font files' character maps, 2026-10-03), so egui draws each of those as its replacement glyph. The stored name is right, and other renderers (exports, sync peers) show the emoji. Whether the rich display should skip the selectors and tags it cannot draw, or a colour emoji font should be added, is left for the live check to show.
- Whether a subdivision flag's tags should be limited to the valid form (lowercase letters and digits, as in `gbeng`): any tags U+E0020-E007E before the cancel tag are kept now, so a name can carry invisible tag text after a 🏴.
