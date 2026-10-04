# Name rules: design, 2026-10-03

Status: designed, not built. Closes the Open question in `2026-10-03-per-tab-jars-design.md` ("Name rules ... should be designed once for every user-editable name"). Decisions from the user on 2026-10-03: protect against **display breakage and spoofing** (not uniqueness or reserved words); the rule is **applied on input and at display**, never by rewriting stored names; hidden characters are **revealed as placeholders**, not dropped; **64 characters** and an **"Untitled"** fallback (assumptions the user accepted by proceeding).

## Goal

Every name the user can edit (group names, jar names, the saved-session name) follows one rule. A name can neither break a row, chip or card layout (newlines, huge length) nor imitate UI that is drawn differently from text (invisible characters, bidi overrides, a typed "Work · #3f2a"). Today each editor only trims, and names from sync, import or older data are unchecked (`session_sync.rs` group names, `jar_defs.rs` `materialize_jar`).

## Out of scope

Uniqueness (identity is the node id), reserved words (the built-in jar is distinguished by its styled label), confusable letters from other alphabets (needs a confusables table), the Control Pictures glyphs (␉ ␊ ␀: none of the loaded fonts has U+2400-243F, nor U+FFFD; revisit if a font is ever added or switched), and tab or card titles, which come from pages.

## `names.rs`

One module holds both rules.

### `clean_name(&str) -> Cow<'_, str>` (input rule)

Applied when the user commits a name. Returns `Cow::Borrowed` when the text is already clean (one `chars().all(is_plain)` scan, no allocation) and an owned string otherwise.

- Drops characters whose Unicode general category is `Cc` (controls, including tab and newline), `Cf` (format: zero-width space, joiners, word joiner, BOM, soft hyphen, bidi marks and overrides, tag characters), `Cs`, `Co` or `Cn`, and variation selectors (`Mn` U+FE00-FE0F, U+E0100-E01EF).
- Turns every other whitespace character (`Zs`, `Zl`, `Zp`: non-breaking, ideographic, thin spaces) into a plain space, collapses runs of whitespace into one, and trims both ends.
- Caps the result at 64 grapheme clusters, by cutting at a cluster boundary.
- Idempotent: `clean_name(clean_name(s)) == clean_name(s)`.
- Leaves ordinary letters in any script, emoji, punctuation and case alone.

Category lookups use `unicode-general-category` and the cap uses `unicode-segmentation`. Both are already in `Cargo.lock` (transitive); `crates/manifold/Cargo.toml` gains them as direct dependencies at the locked versions.

### `display_name(&str, fallback: &str) -> Vec<NameSeg<'_>>` (display rule)

Applied wherever a stored name is drawn. `NameSeg` is `Plain(&str)` or `Hidden(char)`.

- A character `clean_name` would drop becomes `Hidden(c)`; odd whitespace and whitespace runs become a single plain space; the name is trimmed and capped at 64 graphemes, each `Hidden` counting as one, so placeholders cannot be used to dodge the cap.
- An empty result yields the per-kind fallback ("Untitled") as one `Plain` segment.
- `to_plain(&[NameSeg]) -> String` renders `Hidden('\u{200B}')` as `<U+200B>` for plain-text sites (tooltips, confirm dialogs, debug output). A clean name allocates nothing in `display_name`; `to_plain` allocates only where a string is needed anyway.
- Rich-text sites paint `Hidden(c)` as `⟦200B⟧` (uppercase hex, at least four digits) in the dim monospace marker style from jar labels (`JarLabel`), with a hover naming the character. `⟦ ⟧` are in Hack (the monospace fallback); the plain form is pure ASCII. A test asserts that every character of the placeholder form is in the monospace font, so a font change cannot turn it into tofu unnoticed.

## Where each rule applies

| Site | Rule |
|---|---|
| Group rename commit (`node_menu.rs` `CommitRename`), jar create (`create_jar`), jar rename (`rename_jar`, Settings), saved-session name (`settings_page.rs`) | `clean_name`; an empty result behaves as today (rename keeps the old name, create is ignored) |
| Every place a group, jar or session name is drawn: cards, menus, chips, Settings rows, Tab view, confirm texts, tooltips, debug output | `display_name` (+ `to_plain` for plain text) |
| `materialize` of remote or imported names, `session_sync` group names, older data | Not rewritten. The stored value stays as received (no sync churn between builds); display handles it |
| `jar_label` clash detection (`named_jar_label`) | Compares the `clean_name` form of the stored names (trimmed, case-insensitive as today), so a hidden character cannot make two same-looking jars look distinct |

The Settings jar edit field still edits the stored name, never the label, and never the placeholders.

## Guard against missed sites

A test in the style of `profile::tests` fails when a source file outside `names.rs` and the store layer reads a group, jar or session `.name` straight into UI text without `display_name`. The list of allowed files is explicit in the test, so adding a site means adding it there.

## Testing

Unit tests, no profile access: each stripped class (controls, zero-width, bidi, variation selectors, odd spaces), run collapsing, trimming, idempotence, grapheme-boundary cap (a ZWJ emoji sequence is not cut mid-way; a name of 65 clusters is cut to 64), `Cow::Borrowed` for a clean name, placeholder segments and their plain form (`<U+200B>`), the fallback, placeholder count against the cap, placeholder characters present in Hack, jar-label clashing with hidden characters, and the grep guard. A live check on a throwaway profile (`scripts/drive.sh`, `EGUITEXT`): a group named with a pasted zero-width space and a typed "Work · #3f2a" jar look different from a genuine marker.

## Migration

None. Existing names are valid; names with hidden characters start showing placeholders.

## Open questions

- The cap is 64 graphemes everywhere. A per-kind cap (a group title may want more room than a chip) can wait for a case that needs it.
- Whether to add the Control Pictures glyphs for common controls if a font that has them is ever added or switched.
- Whether `clean_name` should also run on a name typed into the jar-name field per keystroke (so a paste shows the cap at once) rather than only on commit.
