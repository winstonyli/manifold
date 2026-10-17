---
status: done
---

# Name Rules Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every user-editable name (group, jar, saved session, bookmark folder) follows one rule. A name is cleaned when the user commits it (`clean_name`). Wherever a stored name is drawn, it is drawn through `display_name`, which reveals hidden characters as placeholders. Emoji sequences keep their joiners, selectors and tags. Stored names are never rewritten. A note's first line, where it stands in as the note's title, is drawn through `display_name` too; the note's text is never cleaned.

**Architecture:**
- A new module, `crates/manifold/src/names.rs`, holds both rules and their helpers:
  - `clean_name` and `display_name`;
  - `NameSeg`, `to_plain`, `placeholder` and `hidden_hint`;
  - `append_name` and `name_job` for rich text.
- **Input sites** call `clean_name` on commit.
- **Rich display sites:**
  - Menus, chips, card headers, the breadcrumb, Settings rows, the Bookmarks popup's folder names and Recently Closed (group and note rows) draw `display_name` segments into a `LayoutJob`.
  - A hidden character is drawn as `⟦200B⟧`, in the jar-marker style (`JarLabel::marker_format`).
- **Plain-text sites** use `to_plain` (`<U+200B>`). These are tooltips, confirm texts and the screen-reader name.
- A source-scan test with an explicit list of allowed files fails when a new `.name` read reaches the UI without a rule.

**Tech Stack:** Rust, egui/epaint 0.36.2.
- Dependencies, both already in `Cargo.lock` transitively:
  - `icu_properties` 2.3.0, for general categories and `Extended_Pictographic`. `url` → `idna` → `idna_adapter` already builds it with `compiled_data`, its default feature, so it adds no crate and no build. It replaces `unicode-general-category`, which the first version of this plan used: that crate is built too (through epaint) but has no emoji property, and one crate for both lookups keeps them on one Unicode version.
  - `unicode-segmentation` 1.13.3, for grapheme clusters (Unicode 17, rule GB11). Its own `Extended_Pictographic` table is private.
- Dev-dependency: `skrifa` 0.44.0, already in `Cargo.lock` through epaint.

**Spec:** `docs/superpowers/specs/2026-10-03-name-rules-design.md`

## Global Constraints

**Repo rules:**
- Work on `dev`.
- Run `cargo fmt --all` before each commit.
- `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings` must pass.
- Never use `--no-verify`.
- The crate is bin-only, so tests run as `cargo test -j 8 -p manifold --bins`.
- No test may read or write the real `%APPDATA%\manifold`. Use `store::testutil::temp_dir`.
- `profile::tests` fails if another source file mentions `"APPDATA"` or `etcetera`.
- Never log cookie values.
- Cap CPU with `-j 8`.
- C: is about 95% full (38 GB free on 2026-10-03). Check `df -h /c` before a full build.
- The repo is LF.
- Run Python as `python -P script < /dev/null`.
- Live checks use `scripts/drive.sh` on an isolated profile, with `EGUITEXT`/`BACKSPACE`. Build first: drive.sh runs the existing exe.
- Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

**From the spec (verbatim values):**
- **Dropped by `clean_name`:**
  - general category `Cc`, `Cf`, `Cs`, `Co`, `Cn`;
  - variation selectors U+FE00-FE0F and U+E0100-E01EF;
  - except inside an emoji sequence (an emoji is an assigned `Extended_Pictographic` character):
    - U+200D between an emoji (optionally followed by skin-tone modifiers, kept selectors or flag tags; rule GB11) and an emoji directly after it;
    - U+FE0F/U+FE0E directly after an emoji, or between a keycap base (`0`-`9`, `#`, `*`) and U+20E3;
    - tags U+E0020-E007F only as 🏴 U+1F3F4, one or more tags U+E0020-E007E, then U+E007F.
  - Skin-tone modifiers U+1F3FB-1F3FF and U+20E3 are not hidden.
- **Whitespace:** `Zs`, `Zl` and `Zp` become a plain space. Runs collapse to one space, and both ends are trimmed.
- **Cap:** 64 grapheme clusters, cut at a cluster boundary. In display, each `Hidden` counts as one.
- **Fallback:** "Untitled".
- **Placeholders:**
  - The plain form is `<U+200B>`.
  - The rich form is `⟦200B⟧`: uppercase hex, at least four digits, in the dim monospace marker style, with a hover naming the character.
- **What is never rewritten:**
  - `materialize` of remote or imported names, `session_sync` group names, and older data. Display handles them.
  - The Settings jar field edits the stored name, never the label or the placeholders.
- **Clash detection:** `jar_label` compares the `clean_name` form (trimmed, case-insensitive).
- **Migration:** none.

## Sites

Line numbers are at `af81954`. "Rule" says which rule the site gets.

| Site | Kind | Rule |
|---|---|---|
| `node_menu.rs:766-774` `CommitRename` (reached from the menu field `:194` and the breadcrumb field `graph_ui.rs:918`) | input | `clean_name`; an empty result keeps the old name |
| `jar_defs.rs:310` `create_jar` | input | stores `clean_name` |
| `node_menu.rs:805-812` `NewJar` | input | ignored when `clean_name` is empty |
| `jar_defs.rs:334` `set_focused_tab_jar_named` (headless `SETJAR`) | input | an empty clean form is ignored; lookup by the raw text, then by the clean text, then create from the clean text |
| `jar_defs.rs:345` `rename_jar` (Settings) | input | `clean_name`; an empty result is ignored |
| `settings_page.rs:508` jar field commit | input | requests a rename only when `draft != stored` and `clean_name(draft) != stored` (focusing a synced dirty name and leaving does not rewrite it) |
| `main.rs:127` `sanitize_session_name`; `settings_page.rs:584-588`; `event_loop.rs:1054` `SAVESESSIONAS` | input | `clean_name`, then the file-safe filter, then `clean_name` again; `save_named_session` sanitises itself, closing the headless bypass |
| `node_menu.rs:793` Export Group default file name | input (file name) | `clean_name`, or "Untitled" when that is empty |
| `popups.rs:676-684` bookmark folder commit | input | `folder_from_draft`: `clean_name`; an empty result means no folder |
| `jar_defs.rs:187-193` `JarLabel::append_to`, which feeds the jar rows at `node_menu.rs:224-249`, the Settings Default row and Tab view's dot hover (`jar_defs.rs:42` `hover_job`) | display, rich | `display_name` via `JarLabel::display` |
| `card.rs:157-169` `paint_jar_chip` name | display, rich | `JarLabel::display` + `append_name` |
| `card.rs:1825` group card name into `paint_card_header` (`card.rs:1894-1989`) | display, rich | `display_name`; placeholders through a `LayoutJob` when present |
| `card.rs:1817` group card tooltip | display, plain | `hidden_hint` |
| `node_menu.rs:212` "Move to {name}" rows | display, rich | `name_job` |
| `graph_ui.rs:885-950` breadcrumb crumbs and their hovers | display, rich + plain | `display_name`, `shorten` to 20 units, `crumb_hover` |
| `settings_page.rs:416` saved-session buttons | display, rich | `name_job`; hover is `with_hint("Open", ..)` |
| `settings_page.rs:500-505` jar name field | display, hover | a `hidden_hint` hover; the field still edits the stored name |
| `popups.rs:510` Recently Closed group row (`str_field("name")`, not `.name`, so the guard cannot see it) | display, rich | `name_job` |
| `popups.rs:513-516` Recently Closed note row (`str_field("text")`'s first line) | display, rich | `name_job` on the first non-blank line, trimmed; the text is never cleaned |
| `popups.rs:612-614` Bookmarks popup folder header (`.folder`, which the guard cannot see) | display, rich | `folder_job`; hover `hidden_hint` |
| `popups.rs:647-659` folder label under each bookmark | display, rich | `folder_job` ("No folder" when there is none); hover `with_hint("Click to set folder", ..)` |
| `jar_defs.rs:191-197` `JarLabel` `Display`, which feeds `jar_confirm_text :276`, the will-switch tooltip `:233`, the label match in `jar_named :328` and `popups.rs:417` | display, plain | `to_plain` |
| `jar_defs.rs:207` `jar_chip` tooltip | display, plain | adds `hidden_hint` |
| `node.rs:577` `accessible_name`, group | display, plain | `to_plain(display_name(..))` |
| `node.rs:578-580` `accessible_name`, note (first non-blank line; `.text`, which the guard cannot see) | display, plain | `to_plain(display_name(..))` on the trimmed line; the text is never cleaned |
| `main.rs:1873,1880` "Couldn't open {name}" toasts | display, plain | `to_plain` (the name is a file stem on disk) |
| `main.rs:1856` "Couldn't save as {name}" | display, plain | already clean (sanitised) |
| `jar_defs.rs:96` `name_key` | clash key | `clean_name(..).to_lowercase()` |
| `session_sync.rs:342,389,516`, `jar_defs.rs:669` `materialize_jar` | sync | raw, marked `// name rule:` |
| `card.rs:846` `card_filter_text` | filter | raw, marked |
| `main.rs:2135` `default_group_name` | comparison | raw, marked |
| `node_menu.rs:762`, `shortcuts.rs:202` rename prefill; `settings_page.rs:500` `jar_name_draft` | edit field | raw (the field edits the stored name), marked |
| `popups.rs:657-658` bookmark folder field prefill; the folder sort and grouping at `:596-608` | edit field, grouping | raw (the field edits the stored name; folders group by their stored text) |
| `graph_nav.rs:207` breadcrumb source; `node_menu.rs:659` move targets; `jar_defs.rs:201` `named_jar_label` | carry | raw, drawn later through a rule, marked |
| `settings_page.rs:416-420` open and delete of a saved session | file identity | raw stem |
| Debug output | none | no debug command or log prints a group, jar or session name. `EXPORTSTATE` carries none, and jar logs use the id |

Notes on the sites:
- Bookmark folder names and a note's first line were raised as questions by the first version of this plan; the user included both on 2026-10-03 (rows above). A note card's header reads "Note", not the first line, so the first line is a title only in the screen-reader name and the Recently Closed row.
- The JSON-level crumb is a file name, out of scope. It still goes through `display_name` along with the other crumbs.

## Order and interfaces

The tasks run in the order 1 → 2 → 3 → 4 → 5. Tasks 2 to 5 use only Task 1's interface. Task 4 also uses `JarLabel::display` from Task 3.

**`crate::names` (Task 1):**
- Constants:
  - `pub(crate) const NAME_CAP: usize = 64;`
  - `pub(crate) const UNTITLED: &str = "Untitled";`
  - `PLACEHOLDER_CHARS` (`"⟦⟧0123456789ABCDEF"`) is private and `#[cfg(test)]`: only the font test reads it, and a non-test build would flag it as dead code.
- `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub(crate) enum NameSeg<'a> { Plain(&'a str), Hidden(char) }`
- The two rules:
  - `pub(crate) fn clean_name(s: &str) -> Cow<'_, str>`
  - `pub(crate) fn display_name<'a>(s: &'a str, fallback: &'a str) -> Vec<NameSeg<'a>>`
- Segment helpers:
  - `pub(crate) fn units(segs: &[NameSeg]) -> usize`
  - `pub(crate) fn take_units<'a>(segs: &[NameSeg<'a>], max: usize) -> Vec<NameSeg<'a>>`
  - `pub(crate) fn shorten<'a>(segs: &[NameSeg<'a>], max: usize) -> Option<Vec<NameSeg<'a>>>`
- Text forms and hints:
  - `pub(crate) fn to_plain(segs: &[NameSeg]) -> String`
  - `pub(crate) fn placeholder(c: char) -> String`
  - `pub(crate) fn has_hidden(segs: &[NameSeg]) -> bool`
  - `pub(crate) fn hidden_hint(segs: &[NameSeg]) -> Option<String>`
  - `pub(crate) fn with_hint(base: &str, segs: &[NameSeg]) -> String`
- Rich text:
  - `pub(crate) fn append_name(job: &mut egui::text::LayoutJob, segs: &[NameSeg], format: &egui::TextFormat, hidden_color: egui::Color32)`
  - `pub(crate) fn name_job(ui: &egui::Ui, prefix: &str, segs: &[NameSeg], suffix: &str) -> egui::text::LayoutJob`

**`crate::theme` (Task 1):** `pub(crate) fn font_definitions() -> egui::FontDefinitions`. `apply_fonts(ctx)` becomes `ctx.set_fonts(font_definitions())`.

**`crate::jar_defs::JarLabel` (Task 3):**
- `pub(crate) fn display(&self) -> Vec<NameSeg<'_>>`
- `pub(crate) fn hidden_hint(&self) -> Option<String>`

**`crate::card::CardHeader` (Task 3):** a new field, `pub(crate) user_name: bool`.

**`crate::node_menu` (Task 2):** `fn export_file_name(name: &str) -> String`.

**`crate::popups`:**
- Task 2: `fn folder_from_draft(draft: &str) -> Option<String>`
- Task 3: `fn folder_job(ui: &egui::Ui, segs: &[NameSeg], color: egui::Color32) -> egui::text::LayoutJob`; `App::closed_entry_label` returns `egui::WidgetText`.

**`crate::graph_ui` (Task 3):**
- `const CRUMB_UNITS: usize = 20;`
- `fn crumb_hover(segs: &[NameSeg], short: &str, long: impl Fn(&str) -> String) -> String`

---

### Task 1: `names.rs`, its dependencies and the font check

**Files:**
- Create: `crates/manifold/src/names.rs`
- Modify: `crates/manifold/Cargo.toml:79-82` (dependencies; a new `[dev-dependencies]`)
- Modify: `crates/manifold/src/main.rs:31` (`mod names;` before `mod nav_history;`)
- Modify: `crates/manifold/src/theme.rs:458-508` (`font_definitions`)
- Test: `crates/manifold/src/names.rs` (`mod tests`)

**Interfaces:**
- Consumes: `crate::jar_defs::JarLabel::marker_format(size: f32, color: egui::Color32) -> egui::TextFormat` and `crate::jar_defs::MARKER_SCALE` (both existing).
- Produces: everything listed under `crate::names` and `crate::theme` above.

**Verified:** this task's code (Steps 1, 2, 3 and 5 as written) was built in a scratch copy of `69fe04c` on 2026-10-03. `cargo test -p manifold --bins names::` passed 19 of 19, `theme::` passed, and `cargo fmt --all --check` and `cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings` were clean. Two bugs in the first version of this task were fixed then: two tests borrowed a temporary `String` past its statement (E0716), and `name_job`'s test read the wrong section (`LayoutJob::append` merges a section into the one before it when their formats match).

- [ ] **Step 1: Add the dependencies.** `Cargo.toml` starts with a UTF-8 BOM and has no trailing newline. Edit it with the Edit tool, not a shell rewrite.

In `[dependencies]`, between `tar = "0.4.46"` and `ureq = "3"`:

```toml
# Name rules (names.rs): general categories and Extended_Pictographic for the hidden characters and emoji
# sequences (already built for url's idna), grapheme clusters for the 64 cap.
icu_properties = "2.3.0"
unicode-segmentation = "1.13.3"
```

After `winit = "0.30.13"` and its blank line, before the `# Windows-only dependencies.` comment:

```toml
[dev-dependencies]
# Reads the monospace font's character map in names::tests (already in Cargo.lock through epaint).
skrifa = "0.44.0"

```

Run: `cargo tree -p manifold --offline -e normal,dev --depth 1 | grep -E "skrifa|icu_properties|unicode-segm"`

Expected: three lines, `icu_properties v2.3.0`, `unicode-segmentation v1.13.3` and `skrifa v0.44.0`. `git diff Cargo.lock` must add exactly `"icu_properties 2.3.0"`, `"skrifa 0.44.0"` and `"unicode-segmentation"` to manifold's `dependencies` list and no new `[[package]]` (`icu_properties` carries its version because 1.5.1 is locked too).

- [ ] **Step 2: Split `theme::apply_fonts`.** Rename the body to a function that returns the definitions, and keep `apply_fonts` as a one-line caller. In `theme.rs`, change the signature line and the last line of the current body:

```rust
pub(crate) fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    // ... the current body, unchanged, down to the Familjen loop ...
    fonts
}

/// Installs `font_definitions` in `ctx`.
pub(crate) fn apply_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}
```

The doc comment above the old `apply_fonts` stays on `font_definitions`. The line `ctx.set_fonts(fonts);` at `theme.rs:507` becomes `fonts`.

- [ ] **Step 3: Write the failing tests.** Create `crates/manifold/src/names.rs` containing only a one-line module doc and this test module, and add `mod names;` to `main.rs` before `mod nav_history;`.

```rust
//! Name rules (spec `docs/superpowers/specs/2026-10-03-name-rules-design.md`).

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    /// 👩‍💻, 🏴󠁧󠁢󠁥󠁮󠁧󠁿 and other sequences written out, so the hidden characters in them are visible in the source.
    const CODER: &str = "👩\u{200D}💻";
    const ENGLAND: &str = "🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";

    #[test]
    fn clean_name_drops_hidden_characters() {
        for (raw, want) in [
            ("Re\u{200B}ading", "Reading"),
            ("a\tb\nc\r", "abc"),
            ("\u{0}nul", "nul"),
            ("\u{202E}evil\u{202C}", "evil"),
            ("\u{2066}iso\u{2069}", "iso"),
            ("soft\u{00AD}hyphen", "softhyphen"),
            ("\u{FEFF}bom", "bom"),
            ("x\u{FE0F}", "x"),
            ("y\u{E0100}", "y"),
            ("\u{E000}p", "p"),
            ("\u{0378}q", "q"),
            ("tag\u{E0041}", "tag"),
        ] {
            assert_eq!(clean_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn clean_name_makes_odd_spaces_one_plain_space_and_trims() {
        for (raw, want) in [
            ("a\u{00A0}b", "a b"),
            ("a\u{3000}b", "a b"),
            ("a\u{2028}b\u{2029}c", "a b c"),
            ("  a \u{2009}\u{2009} b  ", "a b"),
            ("a \u{200B} b", "a b"),
            ("\u{200B} a", "a"),
            (" \u{00A0} ", ""),
            ("\u{200B}\u{202E}", ""),
        ] {
            assert_eq!(clean_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn clean_name_borrows_a_clean_name_and_leaves_it_alone() {
        for name in ["Work", "Café", "東京", "Работа", "👍🏽 Team", "🇯🇵", "Work · #3f2a", "A-b_c (2)!", "", "x"]
        {
            assert!(matches!(clean_name(name), Cow::Borrowed(n) if n == name), "{name:?}");
        }
    }

    /// An emoji sequence keeps its joiners, selectors and tags: the name is already clean.
    #[test]
    fn clean_name_keeps_emoji_sequences_whole() {
        for name in [
            CODER,
            "👩🏽\u{200D}💻",
            "❤\u{FE0F}\u{200D}🔥",
            "🏳\u{FE0F}\u{200D}🌈",
            "☺\u{FE0F}",
            "✈\u{FE0E}",
            "1\u{FE0F}\u{20E3}",
            "#\u{FE0F}\u{20E3}",
            "1\u{20E3}",
            ENGLAND,
            "Team 👩\u{200D}💻 2",
        ] {
            assert!(matches!(clean_name(name), Cow::Borrowed(n) if n == name), "{name:?}");
        }
    }

    /// The same characters outside an emoji sequence are dropped.
    #[test]
    fn clean_name_drops_emoji_formatting_outside_a_sequence() {
        for (raw, want) in [
            ("a\u{200D}b", "ab"),
            ("\u{200D}", ""),
            ("👩\u{200D}", "👩"),
            ("\u{200D}💻", "💻"),
            ("👩 \u{200D}💻", "👩 💻"),
            ("👩\u{200B}\u{200D}💻", "👩💻"),
            ("👩\u{200D}\u{200D}💻", "👩💻"),
            ("a\u{FE0F}", "a"),
            ("\u{FE0F}", ""),
            ("👍🏽\u{FE0F}", "👍🏽"),
            ("1\u{FE0F}", "1"),
            ("🏴\u{E0067}\u{E0062}", "🏴"),
            ("🏴\u{E007F}", "🏴"),
            ("x\u{E0067}\u{E007F}", "x"),
            ("👍\u{E0067}\u{E007F}", "👍"),
            ("🏴\u{E0067}\u{E007F}\u{E0067}", "🏴\u{E0067}\u{E007F}"),
        ] {
            assert_eq!(clean_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn clean_name_is_idempotent() {
        let long = "x".repeat(63) + " y";
        let coders = CODER.repeat(70);
        let inputs = [
            "Re\u{200B}ading",
            "  a \u{2009}\u{2009} b  ",
            "e\u{200B}\u{0301}",
            "\u{202E}..\\",
            "a\tb",
            long.as_str(),
            "a\u{200D}b",
            "👩\u{200B}\u{200D}💻",
            "👩\u{FE0E}\u{FE0F}\u{200D}💻",
            "🏴\u{E0067}\u{E007F}\u{E0067}",
            coders.as_str(),
        ];
        for raw in inputs {
            let once = clean_name(raw).into_owned();
            assert!(matches!(clean_name(&once), Cow::Borrowed(_)), "{raw:?} -> {once:?}");
        }
    }

    #[test]
    fn clean_name_caps_at_64_clusters_on_a_cluster_boundary() {
        assert_eq!(clean_name(&"é".repeat(65)), "é".repeat(64));
        assert_eq!(clean_name(&"👍🏽".repeat(65)), "👍🏽".repeat(64), "a skin-tone emoji is never cut from its modifier");
        assert_eq!(clean_name(&"🇯🇵".repeat(65)), "🇯🇵".repeat(64), "a flag is never split");
        assert_eq!(clean_name(&CODER.repeat(65)), CODER.repeat(64), "a ZWJ sequence is one cluster");
        assert_eq!(clean_name(&ENGLAND.repeat(65)), ENGLAND.repeat(64), "a tag sequence is one cluster");
        assert_eq!(clean_name(&"e\u{0301}".repeat(65)), "e\u{0301}".repeat(64), "a combining mark stays");
        assert_eq!(clean_name(&("x".repeat(63) + " y")), "x".repeat(63), "the space left at the cut is trimmed");
        assert_eq!(clean_name(&"x".repeat(64)), "x".repeat(64));
    }

    #[test]
    fn display_name_borrows_a_clean_name_whole() {
        for name in ["Work Home", CODER, ENGLAND] {
            let segs = display_name(name, UNTITLED);
            assert!(matches!(segs[..], [NameSeg::Plain(p)] if std::ptr::eq(p, name)), "{segs:?}");
        }
    }

    #[test]
    fn display_name_reveals_hidden_characters() {
        let segs = display_name("Re\u{200B}ading", UNTITLED);
        assert_eq!(segs, [NameSeg::Plain("Re"), NameSeg::Hidden('\u{200B}'), NameSeg::Plain("ading")]);
        assert_eq!(to_plain(&segs), "Re<U+200B>ading");
        assert!(has_hidden(&segs));
        assert_eq!(display_name("\u{200B}", UNTITLED), [NameSeg::Hidden('\u{200B}')], "revealed, not the fallback");
    }

    /// Emoji formatting outside a sequence is revealed like any hidden character; a sequence counts as one unit.
    #[test]
    fn display_name_reveals_emoji_formatting_outside_a_sequence() {
        let joined = display_name("a\u{200D}b", UNTITLED);
        assert_eq!(joined, [NameSeg::Plain("a"), NameSeg::Hidden('\u{200D}'), NameSeg::Plain("b")]);
        assert_eq!(display_name("a\u{FE0F}", UNTITLED), [NameSeg::Plain("a"), NameSeg::Hidden('\u{FE0F}')]);
        let segs = display_name("👩\u{200B}\u{200D}💻", UNTITLED);
        assert_eq!(to_plain(&segs), "👩<U+200B><U+200D>💻");
        let coders = CODER.repeat(70);
        let segs = display_name(&coders, UNTITLED);
        assert_eq!((units(&segs), to_plain(&segs)), (64, CODER.repeat(64)));
    }

    #[test]
    fn display_name_collapses_spaces_and_trims() {
        let segs = display_name("  a\u{00A0}\u{00A0}b \u{200B} c ", UNTITLED);
        assert_eq!(
            segs,
            [
                NameSeg::Plain("a"),
                NameSeg::Plain(" "),
                NameSeg::Plain("b"),
                NameSeg::Plain(" "),
                NameSeg::Hidden('\u{200B}'),
                NameSeg::Plain(" "),
                NameSeg::Plain("c"),
            ]
        );
        assert_eq!(to_plain(&segs), "a b <U+200B> c");
    }

    #[test]
    fn display_name_falls_back_when_nothing_is_left() {
        assert_eq!(display_name("", UNTITLED), [NameSeg::Plain("Untitled")]);
        assert_eq!(display_name(" \u{00A0}\u{3000} ", UNTITLED), [NameSeg::Plain("Untitled")]);
    }

    #[test]
    fn placeholders_count_against_the_cap() {
        let hidden = "\u{200B}".repeat(70);
        let segs = display_name(&hidden, UNTITLED);
        assert_eq!((segs.len(), units(&segs)), (64, 64));
        let raw = "a".repeat(10) + &"\u{200B}".repeat(60);
        let segs = display_name(&raw, UNTITLED);
        assert_eq!(units(&segs), 64);
        assert_eq!(segs[0], NameSeg::Plain("aaaaaaaaaa"));
        assert_eq!(segs.iter().filter(|s| matches!(s, NameSeg::Hidden(_))).count(), 54);
        let raw = "x".repeat(70) + "\u{200B}";
        assert_eq!(display_name(&raw, UNTITLED), [NameSeg::Plain(&raw[..64])]);
    }

    #[test]
    fn shorten_keeps_max_minus_one_units_and_an_ellipsis() {
        let long = "x".repeat(30);
        let segs = display_name(&long, UNTITLED);
        let short = shorten(&segs, 20).expect("30 units is over 20");
        assert_eq!(to_plain(&short), "x".repeat(19) + "…");
        assert_eq!(shorten(&display_name(&"x".repeat(20), UNTITLED), 20), None);
        let hidden = display_name("ab\u{200B}\u{200B}cd", UNTITLED);
        assert_eq!(to_plain(&shorten(&hidden, 4).unwrap()), "ab<U+200B>…");
    }

    #[test]
    fn placeholders_are_uppercase_hex_of_at_least_four_digits() {
        assert_eq!(placeholder('\u{200B}'), "⟦200B⟧");
        assert_eq!(placeholder('\t'), "⟦0009⟧");
        assert_eq!(placeholder('\u{E0100}'), "⟦E0100⟧");
        assert_eq!(to_plain(&[NameSeg::Hidden('\u{10FFFD}')]), "<U+10FFFD>");
        for c in ['\0', '\u{200B}', '\u{E01EF}', '\u{10FFFD}'] {
            assert!(placeholder(c).chars().all(|p| PLACEHOLDER_CHARS.contains(p)), "{c:?}");
            assert!(to_plain(&[NameSeg::Hidden(c)]).is_ascii(), "{c:?}");
        }
    }

    #[test]
    fn the_hint_names_each_hidden_character_once() {
        let segs = display_name("a\u{200B}b\u{200B}\u{202E}c\u{E000}\u{200D}", UNTITLED);
        assert_eq!(
            hidden_hint(&segs).as_deref(),
            Some(
                "Hidden characters: U+200B zero width space, U+202E right-to-left override, \
                 U+E000 private-use character, U+200D zero width joiner"
            )
        );
        assert_eq!(hidden_hint(&display_name("Work", UNTITLED)), None);
        assert_eq!(with_hint("Open", &display_name("Work", UNTITLED)), "Open");
        assert_eq!(with_hint("Open", &segs).lines().count(), 2);
        assert_eq!(with_hint("", &segs), hidden_hint(&segs).unwrap());
    }

    #[test]
    fn a_hidden_character_is_appended_in_the_marker_style() {
        let format = egui::TextFormat::simple(egui::FontId::proportional(13.0), egui::Color32::RED);
        let mut job = egui::text::LayoutJob::default();
        append_name(&mut job, &display_name("Re\u{200B}ading", UNTITLED), &format, egui::Color32::GRAY);
        assert_eq!(job.text, "Re⟦200B⟧ading");
        assert_eq!(job.sections.len(), 3);
        assert_eq!(job.sections[0].format, format);
        assert_eq!(job.sections[2].format, format);
        let hidden = &job.sections[1].format;
        assert_eq!(hidden.font_id, egui::FontId::monospace(13.0 * crate::jar_defs::MARKER_SCALE));
        assert_eq!(hidden.color, egui::Color32::GRAY);
    }

    #[test]
    fn name_job_wraps_the_name_in_plain_text() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = name_job(ui, "Move to ", &display_name("Re\u{200B}ading", UNTITLED), " (3)");
            assert_eq!(job.text, "Move to Re⟦200B⟧ading (3)");
            // `LayoutJob::append` merges a section into the one before it when their formats match.
            let colors: Vec<_> = job.sections.iter().map(|s| s.format.color).collect();
            let plain = egui::Color32::PLACEHOLDER;
            assert_eq!(colors, [plain, ui.visuals().weak_text_color(), plain]);
            assert_eq!(name_job(ui, "", &display_name("Work", UNTITLED), "").sections.len(), 1);
        });
        output.textures_delta.clear();
    }

    /// Spec: every character of the rich placeholder is in the monospace font, so a font change cannot turn it into
    /// tofu unnoticed. Checked against the font file's character map (egui's `has_glyph` cannot tell: Hack has the
    /// replacement glyph ◻, so egui reports every character Hack lacks as present).
    #[test]
    fn the_placeholder_characters_are_in_the_monospace_font() {
        use skrifa::MetadataProvider;
        let defs = crate::theme::font_definitions();
        let first = &defs.families[&egui::FontFamily::Monospace][0];
        let data = &defs.font_data[first];
        let font = skrifa::FontRef::from_index(&data.font, data.index).expect("a readable font file");
        let missing: Vec<char> = PLACEHOLDER_CHARS.chars().filter(|&c| font.charmap().map(c).is_none()).collect();
        assert_eq!(missing, Vec::<char>::new(), "{first} lacks these, so a placeholder would draw tofu");
    }
}
```

- [ ] **Step 4: Run the tests and expect failure.**

Run: `cargo test -j 8 -p manifold --bins names::`

Expected: FAIL to compile, with errors such as `cannot find function 'clean_name' in this scope` and `cannot find type 'NameSeg'`.

- [ ] **Step 5: Implement.** Replace the one-line module doc with this; the test module stays below it.

`classify` is the one place that decides what is hidden. `is_clean`, `clean_name` and `display_name` all read its classes, so the emoji-sequence exceptions cannot drift between the input rule and the display rule.

```rust
//! Name rules (spec `docs/superpowers/specs/2026-10-03-name-rules-design.md`). There is one input rule,
//! `clean_name`, applied when the user commits a group, jar, session or bookmark folder name, and one display rule,
//! `display_name`, for every stored name that is drawn. Stored names are never rewritten. A name from sync, an import
//! or older data is shown through the display rule, which reveals a hidden character as a placeholder.

// The callers arrive in the next commits (name-rules plan, Tasks 2-4); Task 5 removes this.
#![allow(dead_code)]

use icu_properties::props::{BinaryProperty, EnumeratedProperty, ExtendedPictographic, GeneralCategory};
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

/// The most grapheme clusters a name keeps (input) or shows (display, where a placeholder counts as one).
pub(crate) const NAME_CAP: usize = 64;
/// What a name with nothing left to show is drawn as.
pub(crate) const UNTITLED: &str = "Untitled";
/// Every character a rich placeholder (`placeholder`) can use. `tests` checks that the monospace font has them all.
#[cfg(test)]
const PLACEHOLDER_CHARS: &str = "⟦⟧0123456789ABCDEF";

const ZWJ: char = '\u{200D}';
const KEYCAP: char = '\u{20E3}';
const BLACK_FLAG: char = '\u{1F3F4}';
const CANCEL_TAG: char = '\u{E007F}';

/// A piece of a name as displayed: text drawn as it is, or a hidden character drawn as a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameSeg<'a> {
    Plain(&'a str),
    Hidden(char),
}

/// What the name rules do with one character of a name, in its place (`classify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Keep,
    /// Whitespace, kept as one plain space per run.
    Space,
    /// Dropped by `clean_name`, a placeholder in `display_name`.
    Hidden,
}

/// A character that is hidden outside an emoji sequence: `Cc`, `Cf`, `Cs`, `Co`, `Cn`, and the variation selectors.
fn is_hidden(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
        || matches!(
            GeneralCategory::for_char(c),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
        )
}

/// Whitespace that a name keeps as one plain space: `Zs` (which includes the plain space), `Zl` and `Zp`.
fn is_space(c: char) -> bool {
    matches!(
        GeneralCategory::for_char(c),
        GeneralCategory::SpaceSeparator | GeneralCategory::LineSeparator | GeneralCategory::ParagraphSeparator
    )
}

/// An emoji (Extended_Pictographic) that an emoji sequence can start from or join to. The unassigned code points
/// that Extended_Pictographic reserves are hidden, so they are not emoji here.
fn is_pict(c: char) -> bool {
    ExtendedPictographic::for_char(c) && GeneralCategory::for_char(c) != GeneralCategory::Unassigned
}

/// The byte length of the subdivision-flag tags at the start of `rest`, the text after a 🏴: one or more tag
/// characters U+E0020-E007E, then the cancel tag U+E007F. 0 when `rest` does not start with such a run.
fn flag_tags_len(rest: &str) -> usize {
    let spec: usize = rest.chars().take_while(|c| ('\u{E0020}'..='\u{E007E}').contains(c)).map(char::len_utf8).sum();
    if spec > 0 && rest[spec..].starts_with(CANCEL_TAG) {
        spec + CANCEL_TAG.len_utf8()
    } else {
        0
    }
}

/// Each character of `s` with its byte offset and its class. A hidden character (`is_hidden`) is `Hidden` except
/// inside an emoji sequence, which is kept whole:
/// - a zero-width joiner between two emoji, the first of which may carry skin-tone modifiers and selectors
///   (👩‍💻, 👩🏽‍💻, ❤️‍🔥; UAX #29 rule GB11);
/// - U+FE0E or U+FE0F directly after an emoji (☺️), or between a keycap base (0-9, `#`, `*`) and U+20E3 (1️⃣);
/// - the tags of a subdivision flag: 🏴, tags, U+E007F (🏴󠁧󠁢󠁥󠁮󠁧󠁿).
///
/// Skin-tone modifiers (U+1F3FB-1F3FF) and U+20E3 are not hidden characters to begin with.
fn classify(s: &str) -> impl Iterator<Item = (usize, char, Class)> + '_ {
    // After an emoji and only its modifiers, selectors or tags since: where a joiner may follow.
    let mut pict = false;
    let mut prev: Option<char> = None;
    // Where the tags of the last subdivision flag end (a byte offset).
    let mut tags_end = 0;
    s.char_indices().map(move |(i, c)| {
        let after = i + c.len_utf8();
        let next = s[after..].chars().next();
        let kept = |ok: bool| if ok { Class::Keep } else { Class::Hidden };
        let class = if i < tags_end {
            Class::Keep
        } else if c == ZWJ {
            let ok = pict && next.is_some_and(is_pict);
            pict = false;
            kept(ok)
        } else if matches!(c, '\u{FE0E}' | '\u{FE0F}') {
            let keycap = matches!(prev, Some('0'..='9' | '#' | '*')) && next == Some(KEYCAP);
            let ok = prev.is_some_and(is_pict) || keycap;
            pict &= ok;
            kept(ok)
        } else if ('\u{1F3FB}'..='\u{1F3FF}').contains(&c) {
            Class::Keep
        } else if is_pict(c) {
            pict = true;
            if c == BLACK_FLAG {
                tags_end = after + flag_tags_len(&s[after..]);
            }
            Class::Keep
        } else {
            pict = false;
            if is_hidden(c) {
                Class::Hidden
            } else if is_space(c) {
                Class::Space
            } else {
                Class::Keep
            }
        };
        prev = Some(c);
        (i, c, class)
    })
}

/// Whether `clean_name` would return `s` unchanged.
fn is_clean(s: &str) -> bool {
    !s.starts_with(' ')
        && !s.ends_with(' ')
        && !s.contains("  ")
        && classify(s).all(|(_, c, class)| class == Class::Keep || c == ' ')
        && (s.len() <= NAME_CAP || s.graphemes(true).nth(NAME_CAP).is_none())
}

/// What a cut at a cluster boundary must not leave at the end: a space, or a joiner (should the segmenter's emoji
/// data be older than `is_pict`'s, it could end a cluster at a joiner that `classify` keeps).
const CUT_TRIM: [char; 2] = [' ', ZWJ];

/// The input rule: `s` without hidden characters (`classify`: emoji sequences keep theirs), with each whitespace run
/// as one plain space, trimmed, and cut to `NAME_CAP` grapheme clusters. It borrows `s` when `s` is already clean,
/// and is idempotent.
pub(crate) fn clean_name(s: &str) -> Cow<'_, str> {
    if is_clean(s) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for (_, c, class) in classify(s) {
        match class {
            Class::Hidden => {}
            Class::Space => space = !out.is_empty(),
            Class::Keep => {
                if std::mem::take(&mut space) {
                    out.push(' ');
                }
                out.push(c);
            }
        }
    }
    if let Some((cut, _)) = out.grapheme_indices(true).nth(NAME_CAP) {
        out.truncate(cut);
        out.truncate(out.trim_end_matches(CUT_TRIM).len());
    }
    Cow::Owned(out)
}

/// The display rule: `s` as segments, with each hidden character (`classify`) a `Hidden`, each whitespace run one
/// plain space, trimmed, and at most `NAME_CAP` units (`units`). It gives `fallback` when nothing is left. A clean
/// name is one segment borrowing all of `s`: the `Vec` is the only allocation, and the name is not copied.
pub(crate) fn display_name<'a>(s: &'a str, fallback: &'a str) -> Vec<NameSeg<'a>> {
    if is_clean(s) {
        return vec![NameSeg::Plain(if s.is_empty() { fallback } else { s })];
    }
    let mut segs = Vec::new();
    let mut run: Option<usize> = None;
    let mut space = false;
    for (i, c, class) in classify(s) {
        if class != Class::Keep {
            if let Some(start) = run.take() {
                segs.push(NameSeg::Plain(&s[start..i]));
            }
            if class == Class::Space {
                space = !segs.is_empty();
                continue;
            }
        }
        if std::mem::take(&mut space) {
            segs.push(NameSeg::Plain(" "));
        }
        if class == Class::Hidden {
            segs.push(NameSeg::Hidden(c));
        } else if run.is_none() {
            run = Some(i);
        }
    }
    if let Some(start) = run {
        segs.push(NameSeg::Plain(&s[start..]));
    }
    let segs = take_units(&segs, NAME_CAP);
    if segs.is_empty() {
        vec![NameSeg::Plain(fallback)]
    } else {
        segs
    }
}

/// How long `segs` is against a cap: grapheme clusters, with a `Hidden` counting as one.
pub(crate) fn units(segs: &[NameSeg]) -> usize {
    segs.iter()
        .map(|s| match s {
            NameSeg::Plain(p) => p.graphemes(true).count(),
            NameSeg::Hidden(_) => 1,
        })
        .sum()
}

/// The first `max` units of `segs` (`units`), cut at a cluster boundary, with no trailing space.
pub(crate) fn take_units<'a>(segs: &[NameSeg<'a>], max: usize) -> Vec<NameSeg<'a>> {
    let mut out = Vec::new();
    let mut left = max;
    for &seg in segs {
        if left == 0 {
            break;
        }
        match seg {
            NameSeg::Hidden(_) => {
                out.push(seg);
                left -= 1;
            }
            NameSeg::Plain(p) => match p.grapheme_indices(true).nth(left) {
                Some((cut, _)) => {
                    out.push(NameSeg::Plain(&p[..cut]));
                    left = 0;
                }
                None => {
                    left -= p.graphemes(true).count();
                    out.push(seg);
                }
            },
        }
    }
    if let Some(NameSeg::Plain(p)) = out.last_mut() {
        let text = *p;
        *p = text.trim_end_matches(CUT_TRIM);
    }
    out.retain(|s| *s != NameSeg::Plain(""));
    out
}

/// `segs` cut to `max - 1` units plus "…", or `None` when `segs` already fits in `max`.
pub(crate) fn shorten<'a>(segs: &[NameSeg<'a>], max: usize) -> Option<Vec<NameSeg<'a>>> {
    (units(segs) > max).then(|| {
        let mut short = take_units(segs, max.saturating_sub(1));
        short.push(NameSeg::Plain("…"));
        short
    })
}

/// The plain-text form, for tooltips, confirm texts and the screen reader, where a hidden character is `<U+200B>`.
pub(crate) fn to_plain(segs: &[NameSeg]) -> String {
    let mut out = String::new();
    for seg in segs {
        match *seg {
            NameSeg::Plain(p) => out.push_str(p),
            NameSeg::Hidden(c) => out.push_str(&format!("<U+{:04X}>", c as u32)),
        }
    }
    out
}

/// The rich form of a hidden character: `⟦200B⟧`, in uppercase hex of at least four digits.
pub(crate) fn placeholder(c: char) -> String {
    format!("⟦{:04X}⟧", c as u32)
}

pub(crate) fn has_hidden(segs: &[NameSeg]) -> bool {
    segs.iter().any(|s| matches!(s, NameSeg::Hidden(_)))
}

/// What a hover says about the hidden characters in `segs`, each named once, or `None` when there are none.
pub(crate) fn hidden_hint(segs: &[NameSeg]) -> Option<String> {
    let mut seen: Vec<char> = Vec::new();
    for seg in segs {
        if let NameSeg::Hidden(c) = *seg {
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
    }
    let named: Vec<String> = seen.iter().map(|&c| format!("U+{:04X} {}", c as u32, char_name(c))).collect();
    (!named.is_empty()).then(|| format!("Hidden characters: {}", named.join(", ")))
}

/// `base`, then `hidden_hint` on its own line when there is one. An empty `base` gives the hint alone.
pub(crate) fn with_hint(base: &str, segs: &[NameSeg]) -> String {
    match (base.is_empty(), hidden_hint(segs)) {
        (_, None) => base.to_string(),
        (true, Some(hint)) => hint,
        (false, Some(hint)) => format!("{base}\n{hint}"),
    }
}

/// A short name for a hidden character, falling back to its kind.
fn char_name(c: char) -> &'static str {
    match c {
        '\t' => "tab",
        '\n' => "line feed",
        '\r' => "carriage return",
        '\0' => "null",
        '\u{00AD}' => "soft hyphen",
        '\u{200B}' => "zero width space",
        '\u{200C}' => "zero width non-joiner",
        '\u{200D}' => "zero width joiner",
        '\u{200E}' => "left-to-right mark",
        '\u{200F}' => "right-to-left mark",
        '\u{202A}' => "left-to-right embedding",
        '\u{202B}' => "right-to-left embedding",
        '\u{202C}' => "pop directional formatting",
        '\u{202D}' => "left-to-right override",
        '\u{202E}' => "right-to-left override",
        '\u{2060}' => "word joiner",
        '\u{2066}' => "left-to-right isolate",
        '\u{2067}' => "right-to-left isolate",
        '\u{2068}' => "first strong isolate",
        '\u{2069}' => "pop directional isolate",
        '\u{FEFF}' => "zero width no-break space",
        '\u{FE0F}' => "emoji variation selector",
        '\u{FE00}'..='\u{FE0E}' | '\u{E0100}'..='\u{E01EF}' => "variation selector",
        '\u{E0020}'..='\u{E007F}' => "tag character",
        _ => match GeneralCategory::for_char(c) {
            GeneralCategory::Control => "control character",
            GeneralCategory::Format => "format character",
            GeneralCategory::PrivateUse => "private-use character",
            GeneralCategory::Surrogate => "surrogate",
            _ => "unassigned code point",
        },
    }
}

/// Appends `segs` to `job`: plain text in `format`, and each hidden character as its placeholder in the jar-marker
/// style (`JarLabel::marker_format`) in `hidden_color`.
pub(crate) fn append_name(
    job: &mut egui::text::LayoutJob,
    segs: &[NameSeg],
    format: &egui::TextFormat,
    hidden_color: egui::Color32,
) {
    for seg in segs {
        match *seg {
            NameSeg::Plain(p) => job.append(p, 0.0, format.clone()),
            NameSeg::Hidden(c) => job.append(
                &placeholder(c),
                0.0,
                crate::jar_defs::JarLabel::marker_format(format.font_id.size, hidden_color),
            ),
        }
    }
}

/// `prefix`, the name and `suffix` in `ui`'s widget font (`FontSelection::Default`, as egui widgets resolve it) and
/// the fallback colour (`Color32::PLACEHOLDER`, so a button paints it in its own state's colour), with placeholders
/// in the weak colour. Used for a button or label whose text includes a stored name.
pub(crate) fn name_job(ui: &egui::Ui, prefix: &str, segs: &[NameSeg], suffix: &str) -> egui::text::LayoutJob {
    let font = egui::FontSelection::Default.resolve(ui.style());
    let plain = egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER);
    let mut job = egui::text::LayoutJob::default();
    if !prefix.is_empty() {
        job.append(prefix, 0.0, plain.clone());
    }
    append_name(&mut job, segs, &plain, ui.visuals().weak_text_color());
    if !suffix.is_empty() {
        job.append(suffix, 0.0, plain);
    }
    job
}
```

The module-level `#![allow(dead_code)]` is there because nothing outside `names.rs` calls it until Task 2, and clippy's `-D warnings` (and the pre-commit hook) would otherwise refuse this commit. Task 5 removes it, and clippy then confirms every item has a caller.

- [ ] **Step 6: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins names::`

Expected: `test result: ok. 19 passed`.

Then run: `cargo test -j 8 -p manifold --bins -- theme:: node_menu:: settings_page::` (several filters go after `--`).

Expected: ok. Only the refactor touched these files.

- [ ] **Step 7: Lint and commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/Cargo.toml Cargo.lock crates/manifold/src/names.rs crates/manifold/src/main.rs crates/manifold/src/theme.rs
git commit -m "Names: clean_name and display_name, with the placeholder font check" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: input sites clean on commit

**Files:**
- Modify: `crates/manifold/src/node_menu.rs:766-774` (`CommitRename`), `:791-799` (Export Group), `:805-812` (`NewJar`)
- Modify: `crates/manifold/src/jar_defs.rs:310` (`create_jar`), `:334-343` (`set_focused_tab_jar_named`), `:345-350` (`rename_jar`)
- Modify: `crates/manifold/src/settings_page.rs:508` (jar field commit), `:584-588` (Save As)
- Modify: `crates/manifold/src/main.rs:120-129` (`sanitize_session_name`), `:1844-1857` (`save_named_session`)
- Modify: `crates/manifold/src/popups.rs:676-684` (bookmark folder commit), `:77` (`folder_from_draft` before `list_row`)
- Test: `node_menu.rs` tests, `jar_defs.rs` tests, `settings_page.rs` tests, `main.rs` tests, `popups.rs` tests

**Interfaces:**
- Consumes: `crate::names::{clean_name, UNTITLED}`.
- Produces: `node_menu::export_file_name(name: &str) -> String` (private), `popups::folder_from_draft(draft: &str) -> Option<String>` (private), and `App::save_named_session(&mut self, raw: &str)`, which now sanitises its argument.

- [ ] **Step 1: Write the failing tests.**

In `node_menu.rs` `mod tests`:

```rust
    /// Name rules: a group rename commits the clean name; one with nothing left keeps the old name.
    #[test]
    fn a_group_rename_commits_the_clean_name() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        let g = app.alloc_node_id();
        app.insert_group_at(g.clone(), None);
        app.apply_node_action(&ctx, &g, NodeAction::CommitRename("  Re\u{200B}ading\u{00A0}\u{00A0}list\n".into()));
        assert_eq!(app.nodes[&g].as_group().unwrap().name, "Reading list");
        app.apply_node_action(&ctx, &g, NodeAction::CommitRename("\u{200B} \u{202E}".into()));
        assert_eq!(app.nodes[&g].as_group().unwrap().name, "Reading list");
    }

    /// Name rules: "New Jar..." makes a jar under the clean name, and none from a name with nothing left.
    #[test]
    fn new_jar_takes_the_clean_name() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        let t = crate::session_sync::tests::add(&mut app, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        app.apply_node_action(&ctx, &t, NodeAction::NewJar("\u{200B} ".into()));
        assert!(app.jar_defs.is_empty());
        app.apply_node_action(&ctx, &t, NodeAction::NewJar(" Wo\u{200B}rk ".into()));
        assert_eq!(app.jar_defs.values().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Work"]);
    }

    #[test]
    fn a_group_exports_under_its_clean_name() {
        assert_eq!(export_file_name("Re\u{200B}ading\n"), "Reading.json");
        assert_eq!(export_file_name("\u{202E}"), "Untitled.json");
    }
```

In `jar_defs.rs` `mod tests`:

```rust
    /// Name rules: a jar is made and renamed under the clean name; a rename with nothing left is ignored.
    #[test]
    fn a_jar_is_made_and_renamed_with_the_clean_name() {
        let mut app = App::default();
        let jar = app.create_jar(" Wo\u{200B}rk\n");
        assert_eq!(app.jar_defs[&jar].name, "Work");
        app.rename_jar(jar, "\u{00A0}Home\u{2028}Office");
        assert_eq!(app.jar_defs[&jar].name, "Home Office");
        app.rename_jar(jar, "\u{200B}");
        assert_eq!(app.jar_defs[&jar].name, "Home Office");
    }

    /// Name rules: `SETJAR` with nothing left of the name does nothing; a name that is clean only after cleaning
    /// finds the jar it cleans to rather than making a twin.
    #[test]
    fn setjar_matches_and_makes_by_the_clean_name() {
        let mut app = App::default();
        let t = crate::session_sync::tests::add(&mut app, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        app.focused = Some(t.clone());
        assert!(!app.set_focused_tab_jar_named("\u{200B} "));
        assert!(app.jar_defs.is_empty());
        let work = app.create_jar("Work");
        assert!(app.set_focused_tab_jar_named("Wo\u{200B}rk"));
        assert_eq!(app.jar_defs.len(), 1, "no twin made");
        assert_eq!(app.tab_jar(&t).map(|c| c.jar), Some(Some(work)));
        assert!(app.set_focused_tab_jar_named(" New\u{00A0}\u{00A0}one "));
        assert!(app.jar_defs.values().any(|d| d.name == "New one"));
    }
```

`tab_jar(&t)` returns the tab's `JarChoice` (`jar.rs:13`, `jar: Option<NodeId>`).

In `settings_page.rs` `mod tests`:

```rust
    /// Name rules: an edit that cleans back to the stored name requests no rename, and neither does focusing a
    /// synced name that holds a hidden character and leaving it unchanged (stored names are never rewritten).
    #[test]
    fn a_jar_field_requests_no_rename_for_the_same_clean_name() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let mut app = App::default();
        let work = app.create_jar("Work");
        let synced = app.create_jar("x");
        app.jar_defs.get_mut(&synced).unwrap().name = "Ho\u{200B}me".into();
        let text = |t: &str| egui::RawInput { events: vec![egui::Event::Text(t.into())], ..Default::default() };
        for (jar, typed) in [(work, Some("\u{200B}")), (synced, None)] {
            ctx.memory_mut(|m| m.request_focus(jar_name_edit_id(jar)));
            jars_frame(&mut app, &ctx);
            if let Some(typed) = typed {
                jars_frame_with(&mut app, &ctx, text(typed));
            }
            jars_frame(&mut app, &ctx);
            ctx.memory_mut(|m| m.surrender_focus(jar_name_edit_id(jar)));
            assert_eq!(jars_frame(&mut app, &ctx).rename_jar, None, "{typed:?}");
        }
    }
```

In `main.rs` `mod tests`:

```rust
    /// Name rules: a session name is cleaned, made file-safe, then cleaned again (spaces the filter leaves collapse).
    #[test]
    fn a_session_name_is_cleaned_then_made_file_safe() {
        assert_eq!(sanitize_session_name("  Trip\u{200B} to\u{00A0}\u{00A0}Rome: day 1 "), "Trip to Rome day 1");
        assert_eq!(sanitize_session_name("a . b"), "a b");
        assert_eq!(sanitize_session_name("\u{202E}..\\"), "");
        assert_eq!(sanitize_session_name(&"é".repeat(80)).chars().count(), 64);
    }

    /// Save As (Settings or headless `SAVESESSIONAS`) with nothing left of the name does nothing; before, the headless
    /// path skipped `sanitize_session_name`.
    #[test]
    fn save_as_with_nothing_left_of_the_name_does_nothing() {
        let mut app = App::default();
        app.save_named_session("\u{200B} ..");
        assert!(app.save_toast_queue.is_empty(), "{:?}", app.save_toast_queue);
        app.save_named_session("Trip");
        assert_eq!(app.save_toast_queue.len(), 1, "under test there is no session file");
    }
```

In `popups.rs` `mod tests` (the module at line 1451, which has `use super::*`):

```rust
    /// Name rules: a bookmark folder is committed under its clean name; nothing left of it means no folder.
    #[test]
    fn a_bookmark_folder_commits_its_clean_name() {
        assert_eq!(folder_from_draft("  Re\u{200B}ading\u{00A0}\u{00A0}list\n").as_deref(), Some("Reading list"));
        assert_eq!(folder_from_draft(" \u{200B}\u{202E} "), None);
        assert_eq!(folder_from_draft(""), None);
    }
```

- [ ] **Step 2: Run the tests and expect failure.**

Run: `cargo test -j 8 -p manifold --bins`

Expected:
- FAIL to compile on `export_file_name` and `folder_from_draft` (not defined).
- With that test commented out, the others fail on the old trimming: "Re\u{200B}ading list" ≠ "Reading list", a jar is made from "\u{200B} ", a rename is requested, and `"a  b"` ≠ `"a b"`.

- [ ] **Step 3: Implement.**

`node_menu.rs`, the `CommitRename` arm:

```rust
            NodeAction::CommitRename(name) => {
                self.renaming_group = None;
                let clean = crate::names::clean_name(&name);
                if !clean.is_empty() {
                    if let Some(group) = self.nodes.get_mut(id).and_then(|n| n.as_group_mut()) {
                        group.name = clean.into_owned();
                    }
                }
            }
```

`node_menu.rs`, the `ExportGroup` arm's first line and dialog call:

```rust
                let group = self.nodes.get(id).and_then(|n| n.as_group());
                let name = export_file_name(group.map_or("", |g| &g.name)); // name rule: export_file_name cleans it
                if let Some(json) = self.export_group_json(id) {
                    if let Some((path, toast)) = save_dialog_with_toast(name, "Group") {
```

And a free function next to `menu_button`:

```rust
/// Export Group's suggested file name: the group's clean name (name rules), or "Untitled" when nothing is left, then
/// ".json". Never the placeholder form, whose `<` `>` Windows forbids in file names.
fn export_file_name(name: &str) -> String {
    let clean = crate::names::clean_name(name);
    format!("{}.json", if clean.is_empty() { crate::names::UNTITLED } else { &clean })
}
```

`node_menu.rs`, the `NewJar` arm:

```rust
            NodeAction::NewJar(name) => {
                self.naming_jar = None;
                if !crate::names::clean_name(&name).is_empty() {
                    let jar = self.create_jar(&name);
                    self.set_tab_jar(id, Some(jar));
                }
```

`jar_defs.rs` `create_jar`. The line that stores the name becomes:

```rust
        self.jar_defs.insert(id, JarDef { name: crate::names::clean_name(name).into_owned(), color, ephemeral: false });
```

Update its doc comment to start with "A new named jar under `name`'s clean form (name rules), with the first palette colour...".

`jar_defs.rs` `set_focused_tab_jar_named`:

```rust
    /// `SETJAR`: the focused tab picks the jar `name` names (`jar_named`: a label or a stored name, tried as typed and
    /// then cleaned; "default", any case, for the default jar), made under the clean name first if none has it.
    /// Nothing happens, and no jar is made, when the focused node is not a tab or nothing is left of the name once
    /// cleaned (name rules). Returns whether it acted.
    pub(crate) fn set_focused_tab_jar_named(&mut self, name: &str) -> bool {
        let Some(tab) = self.focused.clone().filter(|t| self.tab_jar(t).is_some()) else { return false };
        let clean = crate::names::clean_name(name).into_owned();
        if clean.is_empty() {
            return false;
        }
        let jar = if clean.eq_ignore_ascii_case("default") {
            None
        } else {
            Some(self.jar_named(name).or_else(|| self.jar_named(&clean)).unwrap_or_else(|| self.create_jar(&clean)))
        };
        self.set_tab_jar(&tab, jar);
        true
    }
```

`jar_defs.rs` `rename_jar`:

```rust
    /// Renames jar `id` to `name`'s clean form (name rules); nothing left once cleaned leaves the name as it is.
    pub(crate) fn rename_jar(&mut self, id: NodeId, name: &str) {
        let clean = crate::names::clean_name(name);
        if let (false, Some(def)) = (clean.is_empty(), self.jar_defs.get_mut(&id)) {
            def.name = clean.into_owned();
        }
    }
```

`settings_page.rs:508`:

```rust
                    if !escape && draft != def.name && *crate::names::clean_name(&draft) != *def.name {
```

`settings_page.rs:584-588`:

```rust
        if req.save_as_requested {
            let draft = self.session_name_draft.clone();
            self.save_named_session(&draft);
        }
```

`main.rs` `sanitize_session_name`. Keep its doc comment and add the cleaning sentence:

```rust
/// ... (existing text) ... Cleaned first and last by the name rules (`names::clean_name`), so a hidden character
/// never reaches the file name and the spaces the filter leaves behind collapse.
fn sanitize_session_name(raw: &str) -> String {
    let clean = crate::names::clean_name(raw);
    let safe: String = clean.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).collect();
    crate::names::clean_name(&safe).into_owned()
}
```

`main.rs` `save_named_session`:

```rust
    /// ... (existing text) ... `raw` is sanitised here (`sanitize_session_name`), for Settings and the headless
    /// command alike; nothing left of it saves nothing.
    fn save_named_session(&mut self, raw: &str) {
        let name = sanitize_session_name(raw);
        if name.is_empty() {
            return;
        }
        let Some(path) = session::named_session_path(&name) else {
```

The rest of the function is unchanged. `event_loop.rs:1054` stays `self.save_named_session(&name)`.

`popups.rs`, a free function before `list_row`:

```rust
/// A bookmark folder as committed from the folder field: the draft's clean form (name rules), or no folder when
/// nothing is left of it.
fn folder_from_draft(draft: &str) -> Option<String> {
    let clean = crate::names::clean_name(draft);
    (!clean.is_empty()).then(|| clean.into_owned())
}
```

`popups.rs:676-684`, the folder commit in `build_bookmarks_popup`. The `let trimmed = draft.trim();` line goes, and the assignment becomes:

```rust
        if let Some(draft) = folder_rename_commit {
            if let Some((url, _)) = self.renaming_bookmark_folder.take() {
                if let Some(bookmark) = self.bookmarks.iter_mut().find(|b| b.url == url) {
                    bookmark.folder = folder_from_draft(&draft);
                    bookmarks::save(&self.bookmarks);
                }
            }
        }
```

- [ ] **Step 4: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins`

Expected: `test result: ok`. The existing rename, New Jar, SETJAR and Settings tests still pass, because they use clean names.

- [ ] **Step 5: Lint and commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/node_menu.rs crates/manifold/src/jar_defs.rs crates/manifold/src/settings_page.rs crates/manifold/src/main.rs crates/manifold/src/popups.rs
git commit -m "Names: group, jar, session and bookmark folder names are cleaned when committed" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: rich display sites show placeholders

**Files:**
- Modify: `crates/manifold/src/jar_defs.rs:155-193` (`JarLabel::display`, `hidden_hint`, `append_to`), `:224-236` (`jar_chip` tips)
- Modify: `crates/manifold/src/card.rs:9-22` (`CardHeader.user_name`), `:157-169` (`paint_jar_chip`), `:1217,1298,1392,1481,1545,1634` (`user_name: false`), `:1816-1835` (group body), `:1894-1989` (`paint_card_header`)
- Modify: `crates/manifold/src/node_menu.rs:210-216` (MoveTo rows)
- Modify: `crates/manifold/src/graph_ui.rs:878-950` (breadcrumb)
- Modify: `crates/manifold/src/settings_page.rs:413-421` (session rows), `:503` (jar field hover)
- Modify: `crates/manifold/src/popups.rs:497-534` (`closed_entry_label`: group and note rows), `:612-614` and `:647-659` (bookmark folder names), and a new `folder_job` after `folder_from_draft`
- Test: `jar_defs.rs`, `card.rs`, `node_menu.rs`, `graph_ui.rs`, `popups.rs` tests

**Interfaces:**
- Consumes: `crate::names::{display_name, append_name, name_job, shorten, to_plain, has_hidden, hidden_hint, with_hint, NameSeg, UNTITLED}`.
- Produces:
  - `JarLabel::display(&self) -> Vec<NameSeg<'_>>`, `JarLabel::hidden_hint(&self) -> Option<String>`;
  - `CardHeader.user_name: bool`;
  - `const CRUMB_UNITS: usize = 20` and `fn crumb_hover(..)` in `graph_ui.rs`;
  - `fn folder_job(ui: &egui::Ui, segs: &[NameSeg], color: egui::Color32) -> egui::text::LayoutJob` in `popups.rs` (private).

- [ ] **Step 1: Write the failing tests.**

In `jar_defs.rs` `mod tests`:

```rust
    /// Name rules: a label draws a hidden character as a placeholder in the marker style, between the plain name
    /// parts and before a marker, and its hint names it.
    #[test]
    fn a_label_shows_a_hidden_character_in_the_marker_style() {
        let label = JarLabel { name: "Wo\u{200B}rk".into(), marker: Some(JarMarker::Fragment("3f2a".into())) };
        let mut job = egui::text::LayoutJob::default();
        label.append_to(&mut job, egui::FontId::proportional(14.0), egui::Color32::GRAY);
        assert_eq!(job.text, "Wo⟦200B⟧rk #3f2a");
        let parts: Vec<(String, bool)> = job
            .sections
            .iter()
            .map(|s| {
                let text = job.text[s.byte_range.start.0..s.byte_range.end.0].to_string();
                (text, s.format.font_id.family == egui::FontFamily::Monospace)
            })
            .collect();
        let want = [("Wo", false), ("⟦200B⟧", true), ("rk", false), (" #3f2a", true)];
        assert_eq!(parts, want.map(|(t, m)| (t.to_string(), m)));
        assert_eq!(label.hidden_hint().as_deref(), Some("Hidden characters: U+200B zero width space"));
        assert_eq!(JarLabel::built_in().hidden_hint(), None);
    }

    /// Name rules: a chip on a jar whose synced name holds a hidden character says so in its hover.
    #[test]
    fn a_jar_chip_hover_names_a_hidden_character() {
        let mut app = App::default();
        let t = crate::session_sync::tests::add(&mut app, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        let jar = app.create_jar("x");
        app.jar_defs.get_mut(&jar).unwrap().name = "Wo\u{200B}rk".into();
        app.set_tab_jar(&t, Some(jar));
        let tip = app.jar_chip(&t).and_then(|c| c.tooltip).unwrap_or_default();
        assert!(tip.contains("Hidden characters: U+200B zero width space"), "{tip}");
    }
```

In `card.rs` `mod tests`:

```rust
    /// Name rules: a chip draws a hidden character in its jar's name as a placeholder, in the marker style.
    #[test]
    fn a_jar_chip_shows_a_hidden_character_as_a_placeholder() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let chip = crate::jar_defs::JarChip {
            name: crate::jar_defs::JarLabel { name: "Wo\u{200B}rk".into(), marker: None },
            color: egui::Color32::RED,
            following: false,
            missing: false,
            will_switch: false,
            can_switch_now: false,
            was_color: egui::Color32::GRAY,
            engine_ignores: false,
            tooltip: None,
        };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::FontId::proportional(11.0);
            paint_jar_chip(ui.painter(), egui::Pos2::ZERO, 300.0, &chip, font, egui::Color32::WHITE, egui::Color32::YELLOW);
        });
        output.textures_delta.clear();
        let galleys: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.clone()),
                _ => None,
            })
            .collect();
        let name = galleys.iter().find(|g| g.text() == "Wo⟦200B⟧rk");
        let name = name.unwrap_or_else(|| panic!("{:?}", galleys.iter().map(|g| g.text()).collect::<Vec<_>>()));
        let hidden = name.job.sections.iter().find(|s| s.format.font_id.family == egui::FontFamily::Monospace);
        assert_eq!(hidden.expect("a monospace placeholder").format.font_id.size, 11.0 * crate::jar_defs::MARKER_SCALE);
    }

    /// Name rules: a group card's header draws its stored name through the display rule (placeholders, collapsed
    /// spaces, the fallback); another card's title is drawn as before.
    #[test]
    fn a_group_header_draws_its_name_by_the_display_rule() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        for (name, user_name, want) in [
            ("Re\u{200B}ading", true, "Re⟦200B⟧ading"),
            ("  Two\u{00A0}\u{00A0}spaces ", true, "Two spaces"),
            ("", true, "Untitled"),
            ("Re\u{200B}ading", false, "Re\u{200B}ading"),
        ] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let mut overlays: DeferredOverlays = Vec::new();
                let header = CardHeader {
                    glyph: egui_phosphor::light::FOLDER,
                    glyph_color: None,
                    icon_image: None,
                    name: name.into(),
                    user_name,
                    source: "3 items".into(),
                    source_is_path: false,
                    scrim: false,
                    right_margin: 0.0,
                };
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
                app.paint_card_header(ui, &mut overlays, rect, 1.0, 1.0, header);
                for overlay in overlays {
                    overlay(ui.painter(), &|p| p);
                }
            });
            output.textures_delta.clear();
            let galleys: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.clone()),
                    _ => None,
                })
                .collect();
            let title = galleys.iter().find(|g| g.text() == want);
            let title = title.unwrap_or_else(|| panic!("{want}: {:?}", galleys.iter().map(|g| g.text()).collect::<Vec<_>>()));
            let mono = title.job.sections.iter().any(|s| s.format.font_id.family == egui::FontFamily::Monospace);
            assert_eq!(mono, want.contains('⟦'), "{want}");
        }
    }
```

In `node_menu.rs` `mod tests`:

```rust
    /// Name rules: "Move to <group>" draws a hidden character in the group's name as a placeholder.
    #[test]
    fn move_to_rows_show_a_hidden_character_as_a_placeholder() {
        let ctx = menu_ctx();
        let mut app = App::default();
        let (g, note) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(g.clone(), None);
        app.insert_note_at(note.clone(), None);
        app.nodes.get_mut(&g).unwrap().as_group_mut().unwrap().name = "Re\u{200B}ading".into();
        app.node_menu = Some((note, egui::pos2(100.0, 100.0)));
        let mut galleys = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_node_menu(ui));
            output.textures_delta.clear();
            galleys = output
                .shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::Shape::Text(t) => Some(t.galley),
                    _ => None,
                })
                .collect();
        }
        let row = galleys.iter().find(|g| g.text() == "Move to Re⟦200B⟧ading");
        let row = row.unwrap_or_else(|| panic!("{:?}", galleys.iter().map(|g| g.text()).collect::<Vec<_>>()));
        assert!(row.job.sections.iter().any(|s| s.format.font_id.family == egui::FontFamily::Monospace));
    }
```

In `graph_ui.rs` `mod tests` (the one at line 2021 with `use super::*`):

```rust
    /// Name rules: crumbs draw hidden characters as placeholders and shorten to 20 units, a placeholder counting one.
    #[test]
    fn breadcrumb_crumbs_follow_the_display_rule() {
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let mut app = App::default();
        let (outer, inner) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(outer.clone(), None);
        app.insert_group_at(inner.clone(), None);
        app.nodes.get_mut(&inner).unwrap().group = Some(outer.clone());
        app.nodes.get_mut(&outer).unwrap().as_group_mut().unwrap().name = "x".repeat(30);
        app.nodes.get_mut(&inner).unwrap().as_group_mut().unwrap().name = "Re\u{200B}ading".into();
        app.current_group = Some(inner);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.horizontal(|ui| app.toolbar_breadcrumb(ui, false, &mut Vec::new()));
        });
        output.textures_delta.clear();
        let galleys: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.clone()),
                _ => None,
            })
            .collect();
        let texts: Vec<String> = galleys.iter().map(|g| g.text().to_string()).collect();
        assert!(texts.contains(&format!("{}…", "x".repeat(19))), "{texts:?}");
        let current = galleys.iter().find(|g| g.text() == "Re⟦200B⟧ading").unwrap_or_else(|| panic!("{texts:?}"));
        assert!(current.job.sections.iter().any(|s| s.format.font_id.family == egui::FontFamily::Monospace));
    }

    #[test]
    fn a_crumb_hover_names_the_full_name_and_its_hidden_characters() {
        let long = "x".repeat(30);
        let segs = crate::names::display_name(&long, crate::names::UNTITLED);
        assert_eq!(crumb_hover(&segs, "Jump to this group", |f| format!("Jump to this group ({f})")), format!("Jump to this group ({long})"));
        let segs = crate::names::display_name("Re\u{200B}ading", crate::names::UNTITLED);
        assert_eq!(
            crumb_hover(&segs, "Click to rename (F2)", |f| format!("Click to rename, F2 ({f})")),
            "Click to rename (F2)\nHidden characters: U+200B zero width space"
        );
    }
```

In `popups.rs` `mod tests` (the module at line 1451):

```rust
    /// Name rules: a Recently Closed group row draws a hidden character in its name as a placeholder.
    #[test]
    fn a_closed_group_row_shows_a_hidden_character_as_a_placeholder() {
        let ctx = egui::Context::default();
        let mut fields = serde_json::Map::new();
        fields.insert("name".into(), "Re\u{200B}ading".into());
        let node = store::json::JsonNode { id: "g".into(), kind: "group".into(), fields };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(App::closed_entry_label(ui, &node, None, 3).text(), "Re⟦200B⟧ading (3)");
        });
        output.textures_delta.clear();
    }

    /// Name rules: a Recently Closed note row shows the note's first non-blank line by the display rule.
    #[test]
    fn a_closed_note_row_shows_its_first_line_by_the_display_rule() {
        let ctx = egui::Context::default();
        let mut fields = serde_json::Map::new();
        fields.insert("text".into(), "\n\tRe\u{200B}ading\u{00A0}\u{00A0}list \nbody".into());
        let node = store::json::JsonNode { id: "n".into(), kind: "note".into(), fields };
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(App::closed_entry_label(ui, &node, None, 0).text(), "Re⟦200B⟧ading list");
        });
        output.textures_delta.clear();
    }

    /// Name rules: a bookmark folder's name is drawn in the popup's small font, a hidden character as a placeholder.
    #[test]
    fn a_bookmark_folder_shows_a_hidden_character_as_a_placeholder() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let segs = crate::names::display_name("Re\u{200B}ading", crate::names::UNTITLED);
            let job = folder_job(ui, &segs, TEXT_PRIMARY);
            assert_eq!(job.text, "Re⟦200B⟧ading");
            assert_eq!(job.sections[0].format.font_id, egui::TextStyle::Small.resolve(ui.style()));
            assert_eq!(job.sections[0].format.color, TEXT_PRIMARY);
            assert_eq!(job.sections[1].format.font_id.family, egui::FontFamily::Monospace);
        });
        output.textures_delta.clear();
    }
```

The three `popups.rs` tests (with Task 2's `a_bookmark_folder_commits_its_clean_name`) and `closed_entry_label`'s and the folder code below were built and passed in the same scratch copy as Task 1, fmt and clippy clean.

- [ ] **Step 2: Run the tests and expect failure.**

Run: `cargo test -j 8 -p manifold --bins placeholder`, then `cargo test -j 8 -p manifold --bins display_rule`, then `cargo test -j 8 -p manifold --bins hidden_character`.

Expected: FAIL to compile on `JarLabel::hidden_hint`, `CardHeader.user_name`, `crumb_hover` and `folder_job` (not defined). With those stubbed, the galley texts show the raw "Wo\u{200B}rk" and "Re\u{200B}ading".

- [ ] **Step 3: Implement.**

`jar_defs.rs`, in `impl JarLabel`:

```rust
    /// The name as the display rule shows it (`names::display_name`): placeholders for hidden characters, the
    /// "Untitled" fallback.
    pub(crate) fn display(&self) -> Vec<crate::names::NameSeg<'_>> {
        crate::names::display_name(&self.name, crate::names::UNTITLED)
    }

    /// What a hover says about the hidden characters in the name (`names::hidden_hint`).
    pub(crate) fn hidden_hint(&self) -> Option<String> {
        crate::names::hidden_hint(&self.display())
    }

    /// Appends the name in `font` (its hidden characters as placeholders, `names::append_name`), then the marker
    /// (after a space, in its own style), both in `marker_color` where styled.
    pub(crate) fn append_to(&self, job: &mut egui::text::LayoutJob, font: egui::FontId, marker_color: egui::Color32) {
        let size = font.size;
        crate::names::append_name(job, &self.display(), &self.name_format(font), marker_color);
        if let Some(marker) = self.marker_text() {
            job.append(&format!(" {marker}"), 0.0, Self::marker_format(size, marker_color));
        }
    }
```

`jar_defs.rs` `jar_chip`. After `let mut tips = Vec::new();`:

```rust
        if let Some(hint) = name.hidden_hint() {
            tips.push(hint);
        }
```

`card.rs` `CardHeader`. After `name`:

```rust
    /// `name` is a stored group name: drawn by the display rule (`names::display_name`), so a hidden character shows
    /// as a placeholder. Other cards' titles come from pages and files and are drawn as they are.
    pub(crate) user_name: bool,
```

Add `user_name: false,` after `name: ...,` in the six other constructors (`card.rs:1217,1298,1392,1481,1545,1634`).

`card.rs` `paint_group_body`. `let body` becomes `let mut body`, and the group branch becomes:

```rust
            if let Some(node) = self.nodes.get(id).and_then(|n| n.as_group()) {
                body.tooltip = crate::names::hidden_hint(&crate::names::display_name(&node.name, crate::names::UNTITLED));
                let header = CardHeader {
                    glyph: egui_phosphor::light::FOLDER,
                    glyph_color: None,
                    icon_image: None,
                    name: node.name.clone(), // name rule: drawn by display_name in paint_card_header (user_name)
                    user_name: true,
```

Delete the `let name = node.name.clone();` line.

`card.rs` `paint_card_header`. Destructure `user_name`, then replace the `let title = truncate_to_width(...)` line with:

```rust
        // A stored group name with a hidden character is a styled job (placeholders in the marker style, truncated
        // by the layout); any other title is cut text, as before (`truncate_to_width`'s cache).
        let segs = if user_name {
            crate::names::display_name(&name, crate::names::UNTITLED)
        } else {
            vec![crate::names::NameSeg::Plain(&name)]
        };
        let title_size = if tall { HEADER_TITLE_SIZE } else { 12.0 };
        let rich = crate::names::has_hidden(&segs).then(|| {
            let mut job = egui::text::LayoutJob::default();
            let format = egui::TextFormat::simple(egui::FontId::proportional(title_size), title_color);
            crate::names::append_name(&mut job, &segs, &format, source_color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
            job
        });
        let title = match rich {
            Some(_) => String::new(),
            None => {
                let plain = crate::names::to_plain(&segs);
                truncate_to_width(&mut self.truncate_cache, ui.ctx(), &plain, &title_font, max_width)
            }
        };
```

In the deferred closure, replace the two title `overlay_painter.text(...)` calls with:

```rust
                paint_card_title(overlay_painter, base + egui::vec2(text_left, HEADER_TITLE_Y), rich, title, title_font, title_color);
```

(tall branch) and

```rust
                let pos = base + egui::vec2(text_left, band_screen_height / 2.0);
                paint_card_title(overlay_painter, pos, rich, title, egui::FontId::proportional(12.0), title_color);
```

(strip branch). Add a free function after `paint_card_header`'s `impl` block:

```rust
/// A card title, left-centred at `pos`: the styled `rich` job when there is one, else the plain `title`.
fn paint_card_title(
    painter: &egui::Painter,
    pos: egui::Pos2,
    rich: Option<egui::text::LayoutJob>,
    title: String,
    font: egui::FontId,
    color: egui::Color32,
) {
    match rich {
        Some(job) => {
            let galley = painter.layout_job(job);
            let y = pos.y - galley.size().y / 2.0;
            painter.galley(egui::pos2(pos.x, y), galley, color);
        }
        None => {
            painter.text(pos, egui::Align2::LEFT_CENTER, title, font, color);
        }
    }
}
```

`card.rs` `paint_jar_chip`. Replace the `name_job` closure with:

```rust
    let weak = text.gamma_multiply(painter.ctx().global_style().visuals.weak_text_alpha);
    let (segs, name_format) = (chip.name.display(), chip.name.name_format(font.clone()));
    let name_job = |wrap: Option<f32>| {
        let mut job = egui::text::LayoutJob::default();
        crate::names::append_name(&mut job, &segs, &name_format, weak);
        if let Some(room) = wrap {
            job.wrap = egui::text::TextWrapping {
                max_width: room,
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
        }
        painter.layout_job(job)
    };
```

Remove the `let weak = ...` inside the `marker` closure, which now uses the outer `weak`.

`node_menu.rs` MoveTo rows:

```rust
                    NodeMenuItem::MoveTo(targets) => {
                        for (group, name) in targets {
                            let segs = crate::names::display_name(name, crate::names::UNTITLED);
                            let text = crate::names::name_job(ui, "Move to ", &segs, "");
                            let mut response = menu_button(ui, text, None, &mut first_item);
                            if let Some(hint) = crate::names::hidden_hint(&segs) {
                                response = response.on_hover_text(hint);
                            }
                            if response.clicked() {
                                action = Some(NodeAction::MoveToGroup(group.clone()));
                            }
                        }
                    }
```

`graph_ui.rs`. Add near `toolbar_breadcrumb`:

```rust
/// The most units a breadcrumb crumb shows (`names::shorten`; a placeholder counts one).
const CRUMB_UNITS: usize = 20;

/// A crumb's hover: `short`, or `long(full name)` when the crumb is shortened, then the hidden-character hint.
fn crumb_hover(segs: &[crate::names::NameSeg], short: &str, long: impl Fn(&str) -> String) -> String {
    let base = match crate::names::shorten(segs, CRUMB_UNITS) {
        Some(_) => long(&crate::names::to_plain(segs)),
        None => short.to_string(),
    };
    crate::names::with_hint(&base, segs)
}
```

In the loop, replace the `display_name` string and its three uses:

```rust
            for (i, (id, name)) in breadcrumb.iter().enumerate() {
                // (the existing comment on why crumbs are bounded stays, ending:) Shortened by the name rules
                // (`names::shorten`), so a placeholder counts as one and cannot dodge the bound.
                let segs = crate::names::display_name(name, crate::names::UNTITLED);
                let shown = crate::names::shorten(&segs, CRUMB_UNITS);
                let text = crate::names::name_job(ui, "", shown.as_deref().unwrap_or(&segs), "");
                if i + 1 == crumb_count && on_json_level {
                    // The Json node itself: its name is its file's, so there is nothing to rename.
                    let label = ui.label(text);
                    if shown.is_some() || crate::names::has_hidden(&segs) {
                        label.on_hover_text(crumb_hover(&segs, "", str::to_string));
                    }
                } else if i + 1 == crumb_count {
                    // ... the renaming branch unchanged ...
                    } else if ui
                        .button(text)
                        .on_hover_text(crumb_hover(&segs, "Click to rename (F2)", |f| format!("Click to rename, F2 ({f})")))
                        .clicked()
                    {
                        actions.push(GraphAction::Node(id.clone(), NodeAction::Rename));
                    }
                } else {
                    // ... the existing comment ...
                    if ui
                        .button(text)
                        .on_hover_text(crumb_hover(&segs, "Jump to this group", |f| format!("Jump to this group ({f})")))
                        .clicked()
                    {
                        actions.push(GraphAction::EnterGroup(id.clone()));
                    }
```

On the JSON label, `crumb_hover(&segs, "", ..)` with a short crumb and a hidden character gives the hint alone (`with_hint` with an empty base).

`settings_page.rs` session rows:

```rust
        for name in names {
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            ui.horizontal(|ui| {
                let text = crate::names::name_job(ui, "", &segs, "");
                if ui.button(text).on_hover_text(crate::names::with_hint("Open", &segs)).clicked() {
                    req.open_session_name = Some(name.clone());
                }
```

`settings_page.rs` jar field. After `let response = ui.add(edit);`:

```rust
                let hint = crate::names::hidden_hint(&crate::names::display_name(&def.name, crate::names::UNTITLED));
                let response = match hint {
                    Some(hint) => response.on_hover_text(hint),
                    None => response,
                };
```

`popups.rs` `closed_entry_label`:
- The return type becomes `egui::WidgetText`.
- Before the `match`, add early returns for groups and notes:

```rust
        if node.kind == "group" {
            ui.label(egui_phosphor::light::FOLDER_SIMPLE);
            let name = str_field("name");
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            return crate::names::name_job(ui, "", &segs, &format!(" ({member_count})")).into();
        }
        if node.kind == "note" {
            ui.label(egui_phosphor::light::NOTE_PENCIL);
            // The note's text is never cleaned; its first non-blank line is shown by the display rule.
            let text = str_field("text");
            let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
            let segs = crate::names::display_name(first, crate::names::UNTITLED);
            return crate::names::name_job(ui, "", &segs, "").into();
        }
```

- Delete the `"group"` and `"note"` arms.
- Bind the remaining `match` to `let text: String = match ... ;` and end the function with `text.into()`.
- Update the doc comment: a group's row and a note's row are styled jobs by the display rule (name rules); a note shows its first non-blank line (before, its first line even when blank), and "Untitled" when it has none.
- `list_row` already takes `impl Into<WidgetText>`.

`popups.rs`, after `folder_from_draft` (Task 2):

```rust
/// A bookmark folder's name in the popup's small font and `color`, its hidden characters as placeholders (name
/// rules).
fn folder_job(ui: &egui::Ui, segs: &[crate::names::NameSeg], color: egui::Color32) -> egui::text::LayoutJob {
    let format = egui::TextFormat::simple(egui::TextStyle::Small.resolve(ui.style()), color);
    let mut job = egui::text::LayoutJob::default();
    crate::names::append_name(&mut job, segs, &format, ui.visuals().weak_text_color());
    job
}
```

`popups.rs:612-614`, the folder header. `RichText::small().strong().color(TEXT_PRIMARY)` drew the small font in `TEXT_PRIMARY` (an explicit colour wins over `strong`), and `folder_job` draws the same:

```rust
                        if let Some(name) = folder {
                            let segs = crate::names::display_name(name, crate::names::UNTITLED);
                            let header = ui.label(folder_job(ui, &segs, TEXT_PRIMARY));
                            if let Some(hint) = crate::names::hidden_hint(&segs) {
                                header.on_hover_text(hint);
                            }
                        }
```

`popups.rs:647-653`, the folder label under a bookmark (`RichText::small().weak()` was the small font in `weak_text_color`):

```rust
                                let segs = match &bookmark.folder {
                                    Some(name) => crate::names::display_name(name, crate::names::UNTITLED),
                                    None => vec![crate::names::NameSeg::Plain("No folder")],
                                };
                                let job = folder_job(ui, &segs, ui.visuals().weak_text_color());
                                if ui
                                    .add(egui::Label::new(job).sense(egui::Sense::click()))
                                    .on_hover_text(crate::names::with_hint("Click to set folder", &segs))
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
```

The block's body (`folder_rename_start = ...`, which prefills the field with the stored folder) is unchanged.

- [ ] **Step 4: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins`

Expected: `test result: ok`. The existing chip, menu-style, Settings fit and breadcrumb tests still pass, because a clean name lays out exactly as before: one plain section.

- [ ] **Step 5: Lint and commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/jar_defs.rs crates/manifold/src/card.rs crates/manifold/src/node_menu.rs crates/manifold/src/graph_ui.rs crates/manifold/src/settings_page.rs crates/manifold/src/popups.rs
git commit -m "Names: stored names and note titles are drawn with placeholders for hidden characters" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: plain sites and clash detection

**Files:**
- Modify: `crates/manifold/src/jar_defs.rs:95-98` (`name_key`), `:191-199` (`Display`)
- Modify: `crates/manifold/src/node.rs:577-580` (`accessible_name`: the group and note arms)
- Modify: `crates/manifold/src/main.rs:1865-1882` (`load_named_session` toasts)
- Test: `jar_defs.rs` tests (update `a_label_is_the_name_unless_another_jar_shares_it`, add one); `node.rs` `accessible_name_is_kind_then_title`

**Interfaces:**
- Consumes: `crate::names::{clean_name, display_name, to_plain, UNTITLED}`, `JarLabel::display` (Task 3).
- Produces: nothing new. `JarLabel`'s `Display` output changes for dirty names only.

- [ ] **Step 1: Write the failing tests.**

In `jar_defs.rs` `mod tests`:

```rust
    /// Name rules: clash detection compares clean names, so a hidden character cannot make two same-looking jars
    /// look distinct; the plain label shows it as `<U+200B>`.
    #[test]
    fn a_hidden_character_does_not_make_a_name_distinct() {
        let mut defs = BTreeMap::new();
        defs.insert(id(1), def("Work"));
        defs.insert(id(2), def("Wo\u{200B}rk"));
        defs.insert(id(3), def("Work\u{00A0}"));
        for n in [1, 2, 3] {
            assert!(jar_fragment(id(n), &defs).is_some(), "{n} clashes");
        }
        let label = named_jar_label(id(2), &defs).unwrap().to_string();
        assert!(label.starts_with("Wo<U+200B>rk · #"), "{label}");
        assert_eq!(JarLabel::built_in().to_string(), "Default (built-in)");
    }
```

In `a_label_is_the_name_unless_another_jar_shares_it`, the stored " work " is now shown trimmed:

```rust
        // The plain label shows the name by the display rule (trimmed), so " work " reads "work · #frag".
        for (n, (label, name)) in labels.iter().zip(["Work", "work", "WORK"]).enumerate() {
```

In `node.rs` `accessible_name_is_kind_then_title`, append (the note's raw text must come back unchanged: only its title is shown by the rule):

```rust
        app.nodes.get_mut(&g).unwrap().as_group_mut().unwrap().name = "Re\u{200B}ading".into();
        assert_eq!(name(&app, &g), "Group: Re<U+200B>ading");
        app.nodes.get_mut(&g).unwrap().as_group_mut().unwrap().name = " ".into();
        assert_eq!(name(&app, &g), "Group: Untitled");
        let raw = "\n\tRe\u{200B}ading\u{00A0}\u{00A0}list \nbody";
        app.nodes.get_mut(&n).unwrap().payload.as_note_mut().unwrap().text = raw.into();
        assert_eq!(name(&app, &n), "Note: Re<U+200B>ading list");
        assert_eq!(app.nodes[&n].as_note().unwrap().text, raw, "the note's text is not cleaned");
```

- [ ] **Step 2: Run the tests and expect failure.**

Run: `cargo test -j 8 -p manifold --bins a_hidden_character_does_not`, then `cargo test -j 8 -p manifold --bins accessible_name_is_kind`, then `cargo test -j 8 -p manifold --bins a_label_is_the_name_unless`.

Expected:
- FAIL: `2 clashes` (`name_key` keeps the ZWSP);
- `"Group: Re\u{200B}ading"` ≠ `"Group: Re<U+200B>ading"` (and, once the group arm is done, `"Note: Re\u{200B}ading\u{a0}\u{a0}list"` ≠ `"Note: Re<U+200B>ading list"`);
- the label `" work  · #…"` does not start with `"work · #"`.

- [ ] **Step 3: Implement.**

`jar_defs.rs`:

```rust
/// What makes two jar names the same name (jar name labels): their clean forms (name rules: trimmed, hidden
/// characters dropped, spaces collapsed), any case.
fn name_key(name: &str) -> String {
    crate::names::clean_name(name).to_lowercase()
}
```

```rust
/// The plain string: `"Work"`, `"Work · #3f2a"`, or `"Default (built-in)"`, with the name by the display rule
/// (`names::to_plain`: a hidden character reads `<U+200B>`).
impl std::fmt::Display for JarLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = crate::names::to_plain(&self.display());
        match &self.marker {
            None => f.write_str(&name),
            Some(JarMarker::Fragment(frag)) => write!(f, "{name} · #{frag}"),
            Some(JarMarker::BuiltIn) => write!(f, "{name} (built-in)"),
        }
    }
}
```

`node.rs:577-580`, the group and note arms (a note's first non-blank line, trimmed as before, then by the display rule):

```rust
            NodePayload::Group(g) => {
                ("Group", Some(crate::names::to_plain(&crate::names::display_name(&g.name, crate::names::UNTITLED))))
            }
            NodePayload::Note(n) => {
                // The note's text is never cleaned; its first non-blank line is shown by the display rule.
                let first = n.text.lines().map(str::trim).find(|l| !l.is_empty());
                ("Note", first.map(|l| crate::names::to_plain(&crate::names::display_name(l, crate::names::UNTITLED))))
            }
```

This arm and its test were built and passed in the Task 1 scratch copy, fmt and clippy clean.

`main.rs` `load_named_session`. A name from disk is shown by the display rule:

```rust
    fn load_named_session(&mut self, name: &str) {
        let shown = crate::names::to_plain(&crate::names::display_name(name, crate::names::UNTITLED));
```

Then use `{shown}` in place of `{name}` in the two "Couldn't open" toasts. This path returns "No session file" under test, so it has no headless test; the change is two format strings.

- [ ] **Step 4: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins`

Expected: `test result: ok`. `jar_named`'s label match, `jar_confirm_text` and the will-switch tests are unchanged, because they use clean names.

- [ ] **Step 5: Lint and commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/jar_defs.rs crates/manifold/src/node.rs crates/manifold/src/main.rs
git commit -m "Names: plain-text names and jar clashes follow the name rules" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: the guard, docs and the live check

**Files:**
- Modify: `crates/manifold/src/names.rs` (tests: the guard; drop the Task 1 `#![allow(dead_code)]` and its comment)
- Modify (`// name rule:` comments only): `card.rs` (`fit.name.map`, `card_filter_text`), `graph_nav.rs:207`, `jar_defs.rs` (`named_jar_label`, `jar_named`), `main.rs` (`default_group_name`), `node_menu.rs` (move targets, Rename prefill), `session_sync.rs:342,389`, `settings_page.rs` (`jar_name_draft` call), `shortcuts.rs:202`
- Modify: `docs/superpowers/specs/2026-10-03-name-rules-design.md` (status), `docs/superpowers/specs/2026-10-03-per-tab-jars-design.md:195`, `README.md:325`, this plan (Outcome)
- Test: `names.rs` `mod tests`

**Interfaces:**
- Consumes: all of Tasks 1-4.
- Produces: the guard test `every_stored_name_reaches_the_screen_through_a_name_rule`.

- [ ] **Step 1: Write the failing test.** Append to `names.rs` `mod tests`:

```rust
    /// Source files that may read a group, jar or session `.name`: each read there is either drawn through a name
    /// rule (the line calls one of `RULE_MARKS`) or marked raw with `// name rule: <why>`. A new file that reads
    /// `.name` must be added here, after deciding which rule its site gets.
    const ALLOWED: [&str; 9] = [
        "card.rs",
        "graph_nav.rs",
        "jar_defs.rs",
        "main.rs",
        "node.rs",
        "node_menu.rs",
        "session_sync.rs",
        "settings_page.rs",
        "shortcuts.rs",
    ];
    /// Files whose `.name` is some other name: a cookie's (`jar.rs`), a string literal (`json_level_ui.rs`).
    const OTHER_NAMES: [&str; 2] = ["jar.rs", "json_level_ui.rs"];
    const RULE_MARKS: [&str; 8] = [
        "display_name(",
        "clean_name(",
        "name_key(",
        ".display()",
        "append_to(",
        "marker_text(",
        "hidden_hint(",
        "// name rule:",
    ];

    /// `src` up to its first test module. Every file here keeps its test modules last (checked when the guard was
    /// written, 2026-10-03).
    fn production(src: &str) -> &str {
        let starts = ["#[cfg(test)]\nmod ", "#[cfg(test)]\npub(crate) mod "];
        starts.iter().filter_map(|m| src.find(m)).min().map_or(src, |i| &src[..i])
    }

    /// The `.name` reads on `line`: `.name` not followed by more of an identifier (`.name_format`), not assigned
    /// (`.name = `), and not an engine descriptor's (`desc().name`).
    fn name_reads(line: &str) -> usize {
        line.match_indices(".name")
            .filter(|(i, _)| {
                let rest = &line[i + ".name".len()..];
                !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    && !rest.starts_with(" = ")
                    && !line[..*i].ends_with("desc()")
            })
            .count()
    }

    #[test]
    fn name_reads_counts_reads_only() {
        assert_eq!(name_reads("let n = group.name.clone();"), 1);
        assert_eq!(name_reads("f(&a.name, &b.name)"), 2);
        assert_eq!(name_reads("group.name = clean;"), 0);
        assert_eq!(name_reads("label.name_format(font)"), 0);
        assert_eq!(name_reads("engine.desc().name"), 0);
        assert_eq!(production("a\n#[cfg(test)]\nmod tests {\nb.name\n}"), "a\n");
    }

    /// Spec "Guard against missed sites": outside `names.rs` and the store layer, a stored name reaches UI text only
    /// through a name rule.
    #[test]
    fn every_stored_name_reaches_the_screen_through_a_name_rule() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut seen = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                assert_eq!(file, "store", "a new source folder: scan it here too");
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") || file == "names.rs" || OTHER_NAMES.contains(&file.as_str()) {
                continue;
            }
            seen.push(file.clone());
            let src = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            for (no, line) in production(&src).lines().enumerate() {
                if line.trim_start().starts_with("//") || name_reads(line) == 0 {
                    continue;
                }
                if !ALLOWED.contains(&file.as_str()) || !RULE_MARKS.iter().any(|m| line.contains(m)) {
                    offenders.push(format!("{file}:{}: {}", no + 1, line.trim()));
                }
            }
        }
        for file in ALLOWED.iter().chain(&OTHER_NAMES) {
            assert!(seen.contains(&file.to_string()) || OTHER_NAMES.contains(file), "{file} is gone: drop it from the list");
        }
        assert_eq!(
            offenders,
            Vec::<String>::new(),
            "draw a stored name through names::display_name, or mark a raw use `// name rule: <why>`"
        );
    }
```

- [ ] **Step 2: Run the test and expect failure.**

Run: `cargo test -j 8 -p manifold --bins names::tests::every_stored_name`

Expected: FAIL, listing exactly these 12 raw reads (line numbers as they stand after Task 4):
- `card.rs` `let name = fit.name.map(...)` and `card_filter_text`'s `vec![&group.name]`;
- `graph_nav.rs` `NodePayload::Group(group) => group.name.clone(),`;
- `jar_defs.rs` `let name = defs.get(&id)?.name.clone();` and `jar_named`'s `d.name == text`;
- `main.rs` `default_group_name`'s `Some(group.name.as_str()),`;
- `node_menu.rs` the move targets' `sibling.as_group()?.name.clone()` and the Rename prefill `.map(|g| g.name.clone())`;
- `session_sync.rs` `str(&g.name)` and `str(&def.name)`;
- `settings_page.rs` `jar_name_draft(ui, id, &def.name)`;
- `shortcuts.rs` `.map(|g| g.name.clone())`.

Any other line is a site Tasks 2-4 missed. Fix it with its rule; do not mark it.

- [ ] **Step 3: Mark the raw reads.** Append a trailing comment to each of the 12 lines:
- `card.rs` `fit.name.map`: `// name rule: a fitted width, not text`
- `card.rs` `card_filter_text`: `// name rule: the filter matches the stored text`
- `graph_nav.rs:207`: `// name rule: drawn by display_name in toolbar_breadcrumb`
- `jar_defs.rs` `named_jar_label`: `// name rule: drawn through JarLabel::display`
- `jar_defs.rs` `jar_named`: `// name rule: SETJAR matches the stored name`
- `main.rs` `default_group_name`: `// name rule: compares stored names`
- `node_menu.rs` move targets: `// name rule: drawn by display_name in the MoveTo rows`
- `node_menu.rs` Rename prefill: `// name rule: the field edits the stored name`
- `session_sync.rs` (both): `// name rule: synced as stored`
- `settings_page.rs` `jar_name_draft`: `// name rule: the field edits the stored name`
- `shortcuts.rs:202`: `// name rule: the field edits the stored name`

If `cargo fmt` moves a trailing comment onto its own line, the line loses its mark. Bind the read to a local on its own line instead, for example `let stored = &def.name; // name rule: ...`.

Then delete the two lines Task 1 put under `names.rs`'s module doc: the `// The callers arrive in the next commits ...` comment and `#![allow(dead_code)]`. Every item now has a caller; the clippy run in Step 6 proves it, and an item it reports unused is a site Tasks 2-4 missed or a helper to delete.

- [ ] **Step 4: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins`

Expected: `test result: ok`, including `profile::tests` and both guard tests.

- [ ] **Step 5: Update the docs.**
- **Name-rules spec, status line:** "Status: built 2026-10-03 (plan `docs/superpowers/plans/2026-10-03-name-rules.md`; deviations in its Outcome)."
- **Per-tab jars spec, line 195:** "Name rules for every user-editable name (jars, groups, the saved-session name, bookmark folders) are built (`2026-10-03-name-rules-design.md`): names are cleaned when committed, and a stored name's hidden characters are shown as placeholders."
- **README "Per-tab jars":** after "A typed name that copies a marker is drawn as plain name text.", add: "Every group, jar, saved-session and bookmark folder name is cleaned when it is committed: control, zero-width and bidi characters are dropped (emoji sequences such as 👩‍💻 keep theirs), odd spaces collapse to one, and the name is at most 64 characters. A stored name that still holds such a character (from sync or older data) shows it as a dim `⟦200B⟧` placeholder (`<U+200B>` in plain text), with a hover naming it. A note's first line, where it serves as the note's title, is shown the same way; the note itself is never changed."
- **This plan:** add an `## Outcome (date)` section, as the jar-label plans have.

- [ ] **Step 6: Commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src docs/superpowers README.md
git commit -m "Names: guard against unruled name reads; docs" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 7: Live check (no commit).**
1. Run `df -h /c`, then `cargo build -j 8 -p manifold`.
2. Make `target/shots/names/seed/session.json` (`\u200b` and `\u200d` are JSON escapes, so the stored names hold a real U+200B and U+200D):

```json
{"nodes": [
 {"id": "j1", "kind": "jar", "name": "Work · #3f2a"},
 {"id": "j2", "kind": "jar", "name": "Home"},
 {"id": "j3", "kind": "jar", "name": "Home"},
 {"id": "j4", "kind": "jar", "name": "Wo\u200brk"},
 {"id": "j5", "kind": "jar", "name": "👩\u200d💻 Dev"},
 {"id": "g", "kind": "group", "name": "Re\u200bading", "pos": [0.0, 300.0]},
 {"id": "g2", "kind": "group", "name": "Inner", "group": "g"},
 {"id": "t1", "kind": "tab", "url": "about:blank", "title": "Typed look-alike", "engine": "webview2", "pos": [-320.0, 0.0], "jar": "j1", "jar_set": true},
 {"id": "t2", "kind": "tab", "url": "about:blank", "title": "Home A", "engine": "webview2", "pos": [0.0, 0.0], "jar": "j2", "jar_set": true},
 {"id": "t3", "kind": "tab", "url": "about:blank", "title": "Hidden in a jar", "engine": "webview2", "pos": [320.0, 0.0], "jar": "j4", "jar_set": true},
 {"id": "t4", "kind": "tab", "url": "about:blank", "title": "Emoji jar", "engine": "webview2", "pos": [640.0, 0.0], "jar": "j5", "jar_set": true}
]}
```

3. Run 1 (`target/shots/names/run1.txt`), with `SEED=target/shots/names/seed scripts/drive.sh target/shots/names/run1.txt`:

```
WAITTABS 30
SLEEP 3
SCREENSHOT {repo}\target\shots\names\1-graph.png
EXPORTSTATE {repo}\target\shots\names\1-state.json
QUIT
```

4. Read the screen centres of card `t1` and group card `g` from `1-state.json`; call them `TX TY` and `GX GY`.
5. Write `run2.txt` with those numbers. Make the `EGUITEXT` line with `printf 'EGUITEXT Pa\xe2\x80\x8bsted\n' >> run2.txt`, so it carries a real U+200B; the other lines hold no backslash escapes but `{repo}`.

```
WAITTABS 30
SLEEP 2
MOUSEMOVE TX TY
MOUSEDOWN TX TY right
MOUSEUP TX TY right
SLEEP 1
SCREENSHOT {repo}\target\shots\names\2-tabmenu.png
ESCAPE
SLEEP 1
DOUBLECLICK GX GY
SLEEP 2
SCREENSHOT {repo}\target\shots\names\3-breadcrumb.png
RENAMEGROUP
SLEEP 1
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
BACKSPACE
EGUITEXT Pa​sted
ENTER
SLEEP 1
SCREENSHOT {repo}\target\shots\names\4-renamed.png
EXPORTSESSION {repo}\target\shots\names\4-session.json
TOGGLESETTINGSPAGE
RESIZE 900 1800
SLEEP 2
SCREENSHOT {repo}\target\shots\names\5-settings.png
QUIT
```

6. Expected:
   - **`1-graph.png`:** the group card's title reads `Re⟦200B⟧ading` with the placeholder dim and monospace. `t3`'s chip reads `Wo⟦200B⟧rk`, and `t1`'s chip reads `Work · #3f2a` as plain name text. `t4`'s chip has no placeholder, since the joiner in 👩‍💻 is kept; egui, which has no text shaping, should draw 👩 and 💻 side by side (it draws U+200D as nothing). Record what it draws in the Outcome: the spec's open question on emoji drawing waits on it.
   - **`2-tabmenu.png`:**
     - the jar rows show `Work · #3f2a` in the name's font;
     - the two `Home` rows have a smaller, dim, monospace `#xxxx` marker;
     - the `Wo⟦200B⟧rk` row is distinguishable from all of them.
   - **`3-breadcrumb.png`:** the crumb reads `Re⟦200B⟧ading`.
   - **`4-renamed.png`:** the crumb reads `Pasted`.
   - **`4-session.json`:** group `g` is named exactly `"Pasted"`, with no U+200B.
   - **`5-settings.png`:** the Jars section draws the `Home` markers, and the `Wo` U+200B `rk` field shows the stored text, a real U+200B included. Its hover is not captured by a screenshot.
7. Record the results in the Outcome. If RENAMEGROUP's prefill is selected, so that BACKSPACE deletes it all at once, the extra presses do nothing.

---

## Self-review

Re-run on 2026-10-03 after the spec revision (emoji sequences, bookmark folders, note titles, `display_name`'s allocation).

**Spec coverage:**
- **`clean_name` rules** (classes, whitespace, trim, cap, idempotence, Borrowed): Task 1.
- **Emoji sequences** (joiner between emoji per GB11, selectors after an emoji or in a keycap, subdivision-flag tags; dropped elsewhere; idempotence; a sequence is one cluster under the cap): Task 1, `classify` and the tests `clean_name_keeps_emoji_sequences_whole`, `clean_name_drops_emoji_formatting_outside_a_sequence`, `clean_name_is_idempotent`, `clean_name_caps_at_64_clusters_on_a_cluster_boundary`, `display_name_reveals_emoji_formatting_outside_a_sequence`. The spec's required cases are all there: 👩‍💻 kept, a joiner between two letters dropped, a bare VS16 after a letter dropped, 🏴󠁧󠁢󠁥󠁮󠁧󠁿 kept, a lone joiner dropped.
- **Data** (`icu_properties` for categories and `Extended_Pictographic`, `unicode-segmentation` for clusters, no new crate): Task 1 Step 1.
- **`display_name`** (Hidden, single spaces, trim, cap with Hidden = 1, fallback, `to_plain` `<U+200B>`, rich `⟦200B⟧` in the marker style with a hover; one `Vec` allocation and no copy for a clean name, `display_name_borrows_a_clean_name_whole`): Tasks 1 and 3. The hover is `hidden_hint` on the chip, MoveTo, breadcrumb, Settings, the jar field, the group card tooltip and the bookmark folder header and label.
- **The font test:** Task 1, by the font file's character map.
- **Input sites:**
  - `CommitRename`, `create_jar`, `rename_jar`, the session name and the bookmark folder: Task 2.
  - An empty result keeps the old name, is ignored, or (a folder) means no folder: Task 2.
  - `NewJar`, `SETJAR` and Export Group are added as input sites the spec's table implies. Export Group's file name is `clean_name` or "Untitled", as the spec's table now says.
- **Display sites:** "everywhere": Tasks 3 and 4, per the Sites table, bookmark folders (Task 3) included.
- **A note's first line:** display only. Task 3 (Recently Closed row) and Task 4 (`accessible_name`), each with a test; Task 4's test asserts that the note's text is unchanged. No input site touches a note.
- **Never rewritten:** the sync and materialize lines are only marked in Task 5. The Settings guard `draft != def.name` keeps a focused dirty name from being rewritten (Task 2 test).
- **`jar_label` clash on the clean form:** Task 4.
- **The Settings field and the folder field edit the stored name:** unchanged (`jar_name_draft`, the folder prefill).
- **The guard with an explicit list:** Task 5. Folder, note and Recently Closed sites are invisible to it, as the spec says, and covered by their tests.
- **The live check:** Task 5, steps 1-7, now with a 👩‍💻 jar to see how egui draws a kept sequence (the spec's open question).
- **Migration:** none needed.
- **An empty group name** is drawn "Untitled" everywhere, and its screen-reader name is "Group: Untitled" (`node.rs` test); the spec's Sites table says so since Task 5.
- **A note's first line in `accessible_name`** is capped at 64 units by `display_name`, as in the Recently Closed row; the spec's Sites table says so since Task 5.

**Deviations from the spec, to record in the Outcome:**
- The Recently Closed rows and the session toasts are sites the guard cannot see: the name comes through `str_field(..)` or a local, not `.name`. They are listed in the Sites table instead.
- A Recently Closed note row shows the first non-blank line (as `accessible_name` already did) instead of the first line, and "Untitled" for a note with no text.
- (Settled by the revision, no longer deviations: `display_name`'s one allocation is now what the spec says, and the spec keeps emoji sequences, so `clean_name_drops_a_zero_width_joiner` is gone.)

**Placeholder scan:** every code step has its code. "... the current body, unchanged ..." (Task 1, step 2, in `theme.rs`), "... (existing text) ..." in two doc comments and "the renaming branch unchanged" in the breadcrumb are pointers to code that stays as it is, not code left to write.

**Type consistency:**
- `NameSeg<'a>` is used everywhere; `Class` and `classify` are private to `names.rs`.
- `append_name` takes `&TextFormat`: Task 1 defines it that way, and Task 3 calls it with `&format`, `&self.name_format(font)`, `&name_format` and (in `folder_job`) `&format`.
- `name_job(ui, prefix, segs, suffix)` is used in Task 3 (MoveTo, breadcrumb, sessions, popups' group and note rows) and tested in Task 1.
- `folder_from_draft(&str) -> Option<String>` (Task 2) matches `Bookmark::folder: Option<String>`; `folder_job(ui, &segs, color)` is called with `TEXT_PRIMARY` and `ui.visuals().weak_text_color()`.
- `closed_entry_label` returns `egui::WidgetText` in every branch (two early `name_job(..).into()` returns, then `text.into()`).
- `CardHeader.user_name` is set in all seven constructors.
- `crumb_hover(segs, short, long)` has the same argument order in its definition and its three calls.
- `save_named_session(&mut self, raw: &str)` is called from Settings with the draft and from `event_loop.rs:1054` unchanged.
- `PLACEHOLDER_CHARS` is `#[cfg(test)]` and read only by the font test; the interface list says so.

## Outcome (2026-10-03)

- Done as planned (Tasks 1-5), with the deviations listed under Self-review, plus:
  - The guard takes a `// name rule: <why>` mark on the line itself or as a comment on the line just above (`ruled`, tested). After `cargo fmt`, six of the twelve marks sit above their line: five whose trailing comment would pass the 120 width, and `jar_named`'s, which `cargo fmt` moved. A thirteenth mark, above its read, came with the card header change below. The `ALLOWED`/`OTHER_NAMES` presence check now really checks `OTHER_NAMES` (the brief's version excused them).
  - `CardHeader.user_name` is the display segments (`Option<&[NameSeg]>`), not a flag: `paint_group_body` runs `display_name` once for the hover and the header, and other cards' titles no longer build a segment `Vec`.
  - The per-tab jars spec's clash line (149) and the README's "(trimmed, any case)" now name the clean-form key.
- Live check (`scripts/drive.sh`, isolated profile, window 1400x900; shots in `target/shots/names/`, not committed). The seed added to the plan's: group `g` named `Re<U+200B>ading <U+202E>evil`, a 95-character group with U+200B, U+202E and U+202C, an empty-named group, a `Pay<U+202E>roll` jar, and a `bookmarks.json` with a hidden character and a long folder name.
  - Graph (`2-graph.png`): `Re⟦200B⟧ading ⟦2…` and `Quarterly⟦200B⟧…` headers, placeholders dim monospace and legible on the card; the empty group reads "Untitled". Chips: `Wo⟦200B⟧rk`; `Home #2b36`; the typed `Work · #3f2a` is plain serif text, unlike a real marker.
  - **Emoji (the spec's open question):** egui draws 👩‍💻 as 👩 and 💻 side by side, monochrome, with nothing for the joiner, on the chip, in menus and in Settings.
  - Group card hover (`3-group-tooltip.png`): "Hidden characters: U+200B zero width space, U+202E right-to-left override". Over the card's close button it is the only tooltip; with the card's menu open it is suppressed (`4-groupmenu.png`).
  - Menus (`4-groupmenu.png`, `5-tabmenu.png`): jar rows show `Home #2b36` / `Home #2cc2` markers, `Wo⟦200B⟧rk` and `Pay⟦202E⟧roll` distinct from them and from the typed look-alike. Move to rows show placeholders and are cut at 64 units, so the menu widens to about 515 points for the long name.
  - Recently Closed (`7-recently-closed.png`, `7b-recently-closed-hover.png`): the long group's row is truncated at the popup's width with "…" and placeholders kept; its hover is egui's elision hover with the whole label, not a "Hidden characters" hint, and a row that fits has no hover.
  - Breadcrumb (`8-breadcrumb.png`, `8b-breadcrumb-hover.png`): `Re⟦200B⟧ading ⟦202E⟧evil`, hover "Click to rename (F2)" plus the hidden-characters line. Rename (`9a`/`9c`/`9-renamed.png`): the field prefill is the stored text (hidden characters invisible in it); after 16 Backspaces and `EGUITEXT Pa<U+200B>sted`, Enter stores exactly `"Pasted"` (`9-session.json`) and the crumb reads `Pasted`.
  - Settings Jars (`11-settings.png`, a second run, so new ids): the `Home` markers `#5b76`/`#1159` sit beside their fields; the `Wo<U+200B>rk` and `Pay<U+202E>roll` fields show the stored text, which looks like "Work" and "Payroll" (the hover is not captured).
  - Bookmarks list (`13-bookmarks.png`): folder headers and folder labels show `Re⟦200B⟧ading ⟦202E⟧list`; the long folder name wraps at the popup's width and, cut at 64 units with no ellipsis, ends "past the" / "w" on a line of its own.
- Open, for the user: the display cap cuts with no ellipsis, so a capped name looks complete (the bookmark folder's lone "w", the Move to row's "travel and"); and the Recently Closed row has no hover naming its hidden characters.

### Fix wave and open minors (2026-10-04)

The final review's fix wave landed in a0d07e2..8271154: a 256-character budget beside the 64-cluster cap, valid flag tags only, a capped name ends in "…", Move to rows and saved-session buttons shortened, one-line bookmark folder headers, Recently Closed hover hints and member counts. The scoped re-review found nothing load-bearing. Parked minors:

- Recently Closed: a hidden character past the 64-unit cap is not named in the hover. (The member count now reads "(n)" at the row's right edge, kept when the name is cut; the "take_units" space-before-ellipsis, `name_hover` fit-on-cut-text and "click to set folder" wrap minors were fixed on 2026-10-04.)
- Still deferred from the final review: a typed look-alike marker is identical in plain text; the Settings jar field shows the stored text; the guard test's gaps (`.display()` matches `Path::display()`, test modules assumed last, destructured reads).
