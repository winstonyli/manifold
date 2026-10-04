# Name Rules Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every user-editable name (group, jar, saved session) follows one rule. A name is cleaned when the user commits it (`clean_name`). Wherever a stored name is drawn, it is drawn through `display_name`, which reveals hidden characters as placeholders. Stored names are never rewritten.

**Architecture:**
- A new module, `crates/manifold/src/names.rs`, holds both rules and their helpers:
  - `clean_name` and `display_name`;
  - `NameSeg`, `to_plain`, `placeholder` and `hidden_hint`;
  - `append_name` and `name_job` for rich text.
- **Input sites** call `clean_name` on commit.
- **Rich display sites:**
  - Menus, chips, card headers, the breadcrumb, Settings rows and Recently Closed draw `display_name` segments into a `LayoutJob`.
  - A hidden character is drawn as `⟦200B⟧`, in the jar-marker style (`JarLabel::marker_format`).
- **Plain-text sites** use `to_plain` (`<U+200B>`). These are tooltips, confirm texts and the screen-reader name.
- A source-scan test with an explicit list of allowed files fails when a new `.name` read reaches the UI without a rule.

**Tech Stack:** Rust, egui/epaint 0.36.2.
- Dependencies: `unicode-general-category` 1.1.0 and `unicode-segmentation` 1.13.3, both already in `Cargo.lock` transitively.
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
  - variation selectors U+FE00-FE0F and U+E0100-E01EF.
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
| `jar_defs.rs:187-193` `JarLabel::append_to`, which feeds the jar rows at `node_menu.rs:224-249`, the Settings Default row and Tab view's dot hover (`jar_defs.rs:42` `hover_job`) | display, rich | `display_name` via `JarLabel::display` |
| `card.rs:157-169` `paint_jar_chip` name | display, rich | `JarLabel::display` + `append_name` |
| `card.rs:1825` group card name into `paint_card_header` (`card.rs:1894-1989`) | display, rich | `display_name`; placeholders through a `LayoutJob` when present |
| `card.rs:1817` group card tooltip | display, plain | `hidden_hint` |
| `node_menu.rs:212` "Move to {name}" rows | display, rich | `name_job` |
| `graph_ui.rs:885-950` breadcrumb crumbs and their hovers | display, rich + plain | `display_name`, `shorten` to 20 units, `crumb_hover` |
| `settings_page.rs:416` saved-session buttons | display, rich | `name_job`; hover is `with_hint("Open", ..)` |
| `settings_page.rs:500-505` jar name field | display, hover | a `hidden_hint` hover; the field still edits the stored name |
| `popups.rs:510` Recently Closed group row (`str_field("name")`, not `.name`, so the guard cannot see it) | display, rich | `name_job` |
| `jar_defs.rs:191-197` `JarLabel` `Display`, which feeds `jar_confirm_text :276`, the will-switch tooltip `:233`, the label match in `jar_named :328` and `popups.rs:417` | display, plain | `to_plain` |
| `jar_defs.rs:207` `jar_chip` tooltip | display, plain | adds `hidden_hint` |
| `node.rs:577` `accessible_name` | display, plain | `to_plain(display_name(..))` |
| `main.rs:1873,1880` "Couldn't open {name}" toasts | display, plain | `to_plain` (the name is a file stem on disk) |
| `main.rs:1856` "Couldn't save as {name}" | display, plain | already clean (sanitised) |
| `jar_defs.rs:96` `name_key` | clash key | `clean_name(..).to_lowercase()` |
| `session_sync.rs:342,389,516`, `jar_defs.rs:669` `materialize_jar` | sync | raw, marked `// name rule:` |
| `card.rs:846` `card_filter_text` | filter | raw, marked |
| `main.rs:2135` `default_group_name` | comparison | raw, marked |
| `node_menu.rs:762`, `shortcuts.rs:202` rename prefill; `settings_page.rs:500` `jar_name_draft` | edit field | raw (the field edits the stored name), marked |
| `graph_nav.rs:207` breadcrumb source; `node_menu.rs:659` move targets; `jar_defs.rs:201` `named_jar_label` | carry | raw, drawn later through a rule, marked |
| `settings_page.rs:416-420` open and delete of a saved session | file identity | raw stem |
| Debug output | none | no debug command or log prints a group, jar or session name. `EXPORTSTATE` carries none, and jar logs use the id |

Name-like fields that are editable but not in the spec (raised as questions, not built):
- Bookmark folder names (`popups.rs:630`).
- A note's first line, which acts as the card title and the screen-reader name (`node.rs:578`).
- The JSON-level crumb is a file name, out of scope. It still goes through `display_name` along with the other crumbs.

## Order and interfaces

The tasks run in the order 1 → 2 → 3 → 4 → 5. Tasks 2 to 5 use only Task 1's interface. Task 4 also uses `JarLabel::display` from Task 3.

**`crate::names` (Task 1):**
- Constants:
  - `pub(crate) const NAME_CAP: usize = 64;`
  - `pub(crate) const UNTITLED: &str = "Untitled";`
  - `pub(crate) const PLACEHOLDER_CHARS: &str = "⟦⟧0123456789ABCDEF";`
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

- [ ] **Step 1: Add the dependencies.** `Cargo.toml` starts with a UTF-8 BOM and has no trailing newline. Edit it with the Edit tool, not a shell rewrite.

In `[dependencies]`, between `tar = "0.4.46"` and `ureq = "3"`:

```toml
# Name rules (names.rs): general categories for the hidden characters, grapheme clusters for the 64 cap.
unicode-general-category = "1.1.0"
unicode-segmentation = "1.13.3"
```

After `winit = "0.30.13"` and its blank line, before the `# Windows-only dependencies.` comment:

```toml
[dev-dependencies]
# Reads the monospace font's character map in names::tests (already in Cargo.lock through epaint).
skrifa = "0.44.0"

```

Run: `cargo tree -p manifold --offline -e normal,dev --depth 1 | grep -E "skrifa|unicode-(general|segm)"`

Expected: three lines, `skrifa v0.44.0`, `unicode-general-category v1.1.0` and `unicode-segmentation v1.13.3`. `git diff Cargo.lock` must add exactly those three names to manifold's `dependencies` list and no new `[[package]]`. This was verified offline at `af81954`.

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

- [ ] **Step 3: Write the failing tests.** Create `crates/manifold/src/names.rs` containing only the module doc and this test module, and add `mod names;` to `main.rs` before `mod nav_history;`.

```rust
//! Name rules (spec `docs/superpowers/specs/2026-10-03-name-rules-design.md`).

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

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
        for name in ["Work", "Café", "東京", "Работа", "👍🏽 Team", "🇯🇵", "Work · #3f2a", "A-b_c (2)!", "", "x"] {
            assert!(matches!(clean_name(name), Cow::Borrowed(n) if n == name), "{name:?}");
        }
    }

    #[test]
    fn clean_name_is_idempotent() {
        let long = "x".repeat(63) + " y";
        let inputs =
            ["Re\u{200B}ading", "  a \u{2009}\u{2009} b  ", "e\u{200B}\u{0301}", "\u{202E}..\\", "a\tb", long.as_str()];
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
        assert_eq!(clean_name(&"e\u{0301}".repeat(65)), "e\u{0301}".repeat(64), "a combining mark stays");
        assert_eq!(clean_name(&("x".repeat(63) + " y")), "x".repeat(63), "the space left at the cut is trimmed");
        assert_eq!(clean_name(&"x".repeat(64)), "x".repeat(64));
    }

    /// The spec drops every `Cf`, the zero-width joiner included, so a ZWJ emoji sequence loses its joiners (raised
    /// with the user as an open question; this pins today's literal reading).
    #[test]
    fn clean_name_drops_a_zero_width_joiner() {
        assert_eq!(clean_name("👩\u{200D}💻"), "👩💻");
    }

    #[test]
    fn display_name_borrows_a_clean_name_whole() {
        let name = "Work Home";
        let segs = display_name(name, UNTITLED);
        assert!(matches!(segs[..], [NameSeg::Plain(p)] if std::ptr::eq(p, name)), "{segs:?}");
    }

    #[test]
    fn display_name_reveals_hidden_characters() {
        let segs = display_name("Re\u{200B}ading", UNTITLED);
        assert_eq!(segs, [NameSeg::Plain("Re"), NameSeg::Hidden('\u{200B}'), NameSeg::Plain("ading")]);
        assert_eq!(to_plain(&segs), "Re<U+200B>ading");
        assert!(has_hidden(&segs));
        assert_eq!(display_name("\u{200B}", UNTITLED), [NameSeg::Hidden('\u{200B}')], "revealed, not the fallback");
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
        let segs = display_name(&"\u{200B}".repeat(70), UNTITLED);
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
        let segs = display_name("a\u{200B}b\u{200B}\u{202E}c\u{E000}", UNTITLED);
        assert_eq!(
            hidden_hint(&segs).as_deref(),
            Some(
                "Hidden characters: U+200B zero width space, U+202E right-to-left override, \
                 U+E000 private-use character"
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
            assert_eq!(job.sections[0].format.color, egui::Color32::PLACEHOLDER);
            assert_eq!(job.sections[2].format.color, ui.visuals().weak_text_color());
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

- [ ] **Step 5: Implement.** Put this above the test module in `names.rs`.

```rust
//! Name rules (spec `docs/superpowers/specs/2026-10-03-name-rules-design.md`). There is one input rule,
//! `clean_name`, applied when the user commits a group, jar or session name, and one display rule, `display_name`,
//! for every stored name that is drawn. Stored names are never rewritten. A name from sync, an import or older data
//! is shown through the display rule, which reveals a hidden character as a placeholder.

use std::borrow::Cow;
use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_segmentation::UnicodeSegmentation;

/// The most grapheme clusters a name keeps (input) or shows (display, where a placeholder counts as one).
pub(crate) const NAME_CAP: usize = 64;
/// What a name with nothing left to show is drawn as.
pub(crate) const UNTITLED: &str = "Untitled";
/// Every character a rich placeholder (`placeholder`) can use. `tests` checks that the monospace font has them all.
pub(crate) const PLACEHOLDER_CHARS: &str = "⟦⟧0123456789ABCDEF";

/// A piece of a name as displayed: text drawn as it is, or a hidden character drawn as a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameSeg<'a> {
    Plain(&'a str),
    Hidden(char),
}

/// A character `clean_name` drops and `display_name` reveals: `Cc`, `Cf`, `Cs`, `Co`, `Cn`, and the variation
/// selectors.
fn is_hidden(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
        || matches!(
            get_general_category(c),
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
        get_general_category(c),
        GeneralCategory::SpaceSeparator | GeneralCategory::LineSeparator | GeneralCategory::ParagraphSeparator
    )
}

/// Whether `clean_name` would return `s` unchanged.
fn is_clean(s: &str) -> bool {
    !s.starts_with(' ')
        && !s.ends_with(' ')
        && !s.contains("  ")
        && s.chars().all(|c| c == ' ' || !(is_hidden(c) || is_space(c)))
        && (s.len() <= NAME_CAP || s.graphemes(true).nth(NAME_CAP).is_none())
}

/// The input rule: `s` without hidden characters, with each whitespace run as one plain space, trimmed, and cut to
/// `NAME_CAP` grapheme clusters. It borrows `s` when `s` is already clean, and is idempotent.
pub(crate) fn clean_name(s: &str) -> Cow<'_, str> {
    if is_clean(s) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if is_hidden(c) {
            continue;
        }
        if is_space(c) {
            space = !out.is_empty();
            continue;
        }
        if std::mem::take(&mut space) {
            out.push(' ');
        }
        out.push(c);
    }
    if let Some((cut, _)) = out.grapheme_indices(true).nth(NAME_CAP) {
        out.truncate(cut);
        out.truncate(out.trim_end_matches(' ').len());
    }
    Cow::Owned(out)
}

/// The display rule: `s` as segments, with each hidden character a `Hidden`, each whitespace run one plain space,
/// trimmed, and at most `NAME_CAP` units (`units`). It gives `fallback` when nothing is left. A clean name is one
/// segment borrowing all of `s`.
pub(crate) fn display_name<'a>(s: &'a str, fallback: &'a str) -> Vec<NameSeg<'a>> {
    if is_clean(s) {
        return vec![NameSeg::Plain(if s.is_empty() { fallback } else { s })];
    }
    let mut segs = Vec::new();
    let mut run: Option<usize> = None;
    let mut space = false;
    for (i, c) in s.char_indices() {
        let hidden = is_hidden(c);
        if hidden || is_space(c) {
            if let Some(start) = run.take() {
                segs.push(NameSeg::Plain(&s[start..i]));
            }
            if !hidden {
                space = !segs.is_empty();
                continue;
            }
        }
        if std::mem::take(&mut space) {
            segs.push(NameSeg::Plain(" "));
        }
        if hidden {
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
        *p = text.trim_end_matches(' ');
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
        _ => match get_general_category(c) {
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

`is_hidden` is private. Task 2 does not need it; the guard in Task 5 is the only cross-module check.

- [ ] **Step 6: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins names::`

Expected: `test result: ok. 17 passed`.

Then run: `cargo test -j 8 -p manifold --bins theme:: node_menu:: settings_page::`

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
- Test: `node_menu.rs` tests, `jar_defs.rs` tests, `settings_page.rs` tests, `main.rs` tests

**Interfaces:**
- Consumes: `crate::names::{clean_name, UNTITLED}`.
- Produces: `node_menu::export_file_name(name: &str) -> String` (private), and `App::save_named_session(&mut self, raw: &str)`, which now sanitises its argument.

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

- [ ] **Step 2: Run the tests and expect failure.**

Run: `cargo test -j 8 -p manifold --bins`

Expected:
- FAIL to compile on `export_file_name` (not defined).
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

- [ ] **Step 4: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins`

Expected: `test result: ok`. The existing rename, New Jar, SETJAR and Settings tests still pass, because they use clean names.

- [ ] **Step 5: Lint and commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/node_menu.rs crates/manifold/src/jar_defs.rs crates/manifold/src/settings_page.rs crates/manifold/src/main.rs
git commit -m "Names: group, jar and session names are cleaned when committed" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: rich display sites show placeholders

**Files:**
- Modify: `crates/manifold/src/jar_defs.rs:155-193` (`JarLabel::display`, `hidden_hint`, `append_to`), `:224-236` (`jar_chip` tips)
- Modify: `crates/manifold/src/card.rs:9-22` (`CardHeader.user_name`), `:157-169` (`paint_jar_chip`), `:1217,1298,1392,1481,1545,1634` (`user_name: false`), `:1816-1835` (group body), `:1894-1989` (`paint_card_header`)
- Modify: `crates/manifold/src/node_menu.rs:210-216` (MoveTo rows)
- Modify: `crates/manifold/src/graph_ui.rs:878-950` (breadcrumb)
- Modify: `crates/manifold/src/settings_page.rs:413-421` (session rows), `:503` (jar field hover)
- Modify: `crates/manifold/src/popups.rs:497-534` (`closed_entry_label`)
- Test: `jar_defs.rs`, `card.rs`, `node_menu.rs`, `graph_ui.rs`, `popups.rs` tests

**Interfaces:**
- Consumes: `crate::names::{display_name, append_name, name_job, shorten, to_plain, has_hidden, hidden_hint, with_hint, NameSeg, UNTITLED}`.
- Produces:
  - `JarLabel::display(&self) -> Vec<NameSeg<'_>>`, `JarLabel::hidden_hint(&self) -> Option<String>`;
  - `CardHeader.user_name: bool`;
  - `const CRUMB_UNITS: usize = 20` and `fn crumb_hover(..)` in `graph_ui.rs`.

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
```

- [ ] **Step 2: Run the tests and expect failure.**

Run: `cargo test -j 8 -p manifold --bins placeholder`, then `cargo test -j 8 -p manifold --bins display_rule`, then `cargo test -j 8 -p manifold --bins hidden_character`.

Expected: FAIL to compile on `JarLabel::hidden_hint`, `CardHeader.user_name` and `crumb_hover` (not defined). With those stubbed, the galley texts show the raw "Wo\u{200B}rk" and "Re\u{200B}ading".

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
- Before the `match`, add an early return for groups:

```rust
        if node.kind == "group" {
            ui.label(egui_phosphor::light::FOLDER_SIMPLE);
            let name = str_field("name");
            let segs = crate::names::display_name(&name, crate::names::UNTITLED);
            return crate::names::name_job(ui, "", &segs, &format!(" ({member_count})")).into();
        }
```

- Delete the `"group"` arm.
- Bind the remaining `match` to `let text: String = match ... ;` and end the function with `text.into()`.
- Update the doc comment to say a group's row is a styled job (name rules).
- `list_row` already takes `impl Into<WidgetText>`.

- [ ] **Step 4: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins`

Expected: `test result: ok`. The existing chip, menu-style, Settings fit and breadcrumb tests still pass, because a clean name lays out exactly as before: one plain section.

- [ ] **Step 5: Lint and commit.**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol --all-targets -- -D warnings
git add crates/manifold/src/jar_defs.rs crates/manifold/src/card.rs crates/manifold/src/node_menu.rs crates/manifold/src/graph_ui.rs crates/manifold/src/settings_page.rs crates/manifold/src/popups.rs
git commit -m "Names: stored names are drawn with placeholders for hidden characters" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: plain sites and clash detection

**Files:**
- Modify: `crates/manifold/src/jar_defs.rs:95-98` (`name_key`), `:191-199` (`Display`)
- Modify: `crates/manifold/src/node.rs:577` (`accessible_name`)
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

In `node.rs` `accessible_name_is_kind_then_title`, append:

```rust
        app.nodes.get_mut(&g).unwrap().as_group_mut().unwrap().name = "Re\u{200B}ading".into();
        assert_eq!(name(&app, &g), "Group: Re<U+200B>ading");
        app.nodes.get_mut(&g).unwrap().as_group_mut().unwrap().name = " ".into();
        assert_eq!(name(&app, &g), "Group: Untitled");
```

- [ ] **Step 2: Run the tests and expect failure.**

Run: `cargo test -j 8 -p manifold --bins a_hidden_character_does_not`, then `cargo test -j 8 -p manifold --bins accessible_name_is_kind`, then `cargo test -j 8 -p manifold --bins a_label_is_the_name_unless`.

Expected:
- FAIL: `2 clashes` (`name_key` keeps the ZWSP);
- `"Group: Re\u{200B}ading"` ≠ `"Group: Re<U+200B>ading"`;
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

`node.rs:577`:

```rust
            NodePayload::Group(g) => {
                ("Group", Some(crate::names::to_plain(&crate::names::display_name(&g.name, crate::names::UNTITLED))))
            }
```

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
- Modify: `crates/manifold/src/names.rs` (tests: the guard)
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

- [ ] **Step 4: Run the tests and expect them to pass.**

Run: `cargo test -j 8 -p manifold --bins`

Expected: `test result: ok`, including `profile::tests` and both guard tests.

- [ ] **Step 5: Update the docs.**
- **Name-rules spec, status line:** "Status: built 2026-10-03 (plan `docs/superpowers/plans/2026-10-03-name-rules.md`; deviations in its Outcome)."
- **Per-tab jars spec, line 195:** "Name rules for every user-editable name (jars, groups, the saved-session name) are built (`2026-10-03-name-rules-design.md`): names are cleaned when committed, and a stored name's hidden characters are shown as placeholders."
- **README "Per-tab jars":** after "A typed name that copies a marker is drawn as plain name text.", add: "Every group, jar and saved-session name is cleaned when it is committed: control, zero-width and bidi characters are dropped, odd spaces collapse to one, and the name is at most 64 characters. A stored name that still holds such a character (from sync or older data) shows it as a dim `⟦200B⟧` placeholder (`<U+200B>` in plain text), with a hover naming it."
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
2. Make `target/shots/names/seed/session.json` (in `​`, JSON's escape puts the real character in the stored name):

```json
{"nodes": [
 {"id": "j1", "kind": "jar", "name": "Work · #3f2a"},
 {"id": "j2", "kind": "jar", "name": "Home"},
 {"id": "j3", "kind": "jar", "name": "Home"},
 {"id": "j4", "kind": "jar", "name": "Wo​rk"},
 {"id": "g", "kind": "group", "name": "Re​ading", "pos": [0.0, 300.0]},
 {"id": "g2", "kind": "group", "name": "Inner", "group": "g"},
 {"id": "t1", "kind": "tab", "url": "about:blank", "title": "Typed look-alike", "engine": "webview2", "pos": [-320.0, 0.0], "jar": "j1", "jar_set": true},
 {"id": "t2", "kind": "tab", "url": "about:blank", "title": "Home A", "engine": "webview2", "pos": [0.0, 0.0], "jar": "j2", "jar_set": true},
 {"id": "t3", "kind": "tab", "url": "about:blank", "title": "Hidden in a jar", "engine": "webview2", "pos": [320.0, 0.0], "jar": "j4", "jar_set": true}
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
   - **`1-graph.png`:** the group card's title reads `Re⟦200B⟧ading` with the placeholder dim and monospace. `t3`'s chip reads `Wo⟦200B⟧rk`, and `t1`'s chip reads `Work · #3f2a` as plain name text.
   - **`2-tabmenu.png`:**
     - the jar rows show `Work · #3f2a` in the name's font;
     - the two `Home` rows have a smaller, dim, monospace `#xxxx` marker;
     - the `Wo⟦200B⟧rk` row is distinguishable from all of them.
   - **`3-breadcrumb.png`:** the crumb reads `Re⟦200B⟧ading`.
   - **`4-renamed.png`:** the crumb reads `Pasted`.
   - **`4-session.json`:** group `g` is named exactly `"Pasted"`, with no `​`.
   - **`5-settings.png`:** the Jars section draws the `Home` markers, and the `Wo​rk` field shows the stored text. Its hover is not captured by a screenshot.
7. Record the results in the Outcome. If RENAMEGROUP's prefill is selected, so that BACKSPACE deletes it all at once, the extra presses do nothing.

---

## Self-review

**Spec coverage:**
- **`clean_name` rules** (classes, whitespace, trim, cap, idempotence, Borrowed): Task 1.
- **`display_name`** (Hidden, single spaces, trim, cap with Hidden = 1, fallback, `to_plain` `<U+200B>`, rich `⟦200B⟧` in the marker style with a hover): Tasks 1 and 3. The hover is `hidden_hint` on the chip, MoveTo, breadcrumb, Settings, the jar field and the group card tooltip.
- **The font test:** Task 1, by the font file's character map.
- **Input sites:**
  - `CommitRename`, `create_jar`, `rename_jar` and the session name: Task 2.
  - An empty result keeps the old name or is ignored: Task 2.
  - `NewJar`, `SETJAR` and Export Group are added as input sites the spec's table implies.
- **Display sites:** "everywhere": Tasks 3 and 4, per the Sites table.
- **Never rewritten:** the sync and materialize lines are only marked in Task 5. The Settings guard `draft != def.name` keeps a focused dirty name from being rewritten (Task 2 test).
- **`jar_label` clash on the clean form:** Task 4.
- **The Settings field edits the stored name:** unchanged (`jar_name_draft`).
- **The guard with an explicit list:** Task 5.
- **The live check:** Task 5, step 7.
- **Migration:** none needed.

**Deviations from the spec, to record in the Outcome:**
- `display_name` returns a one-element `Vec` for a clean name, so it allocates once. The spec says "allocates nothing"; the segment borrows the whole name, and nothing else is allocated.
- The spec drops `Cf`, which includes ZWJ, and VS16 (U+FE0F). Those drops break ZWJ emoji sequences and emoji presentation, against "leaves emoji alone". The plan follows the drop list literally and pins it (`clean_name_drops_a_zero_width_joiner`). The spec's cap example ("a ZWJ emoji sequence is not cut mid-way") becomes skin-tone and flag sequences. This is raised with the user.
- The Recently Closed group row and the session toasts are sites the guard cannot see: the name comes through `str_field("name")` or a local, not `.name`. They are listed in the Sites table instead.

**Placeholder scan:** every code step has its code.

**Type consistency:**
- `NameSeg<'a>` is used everywhere.
- `append_name` takes `&TextFormat`: Task 1 defines it that way, and Task 3 calls it with `&format`, `&self.name_format(font)` and `&name_format`.
- `name_job(ui, prefix, segs, suffix)` is used in Task 3 (MoveTo, breadcrumb, sessions, popups) and tested in Task 1.
- `CardHeader.user_name` is set in all seven constructors.
- `crumb_hover(segs, short, long)` has the same argument order in its definition and its three calls.
- `save_named_session(&mut self, raw: &str)` is called from Settings with the draft and from `event_loop.rs:1054` unchanged.
