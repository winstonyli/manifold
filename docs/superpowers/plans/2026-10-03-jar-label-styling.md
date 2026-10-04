# Styled Jar Label Markers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A jar's marker (`#3f2a`, or the built-in jar's `built-in`) is drawn in its own style wherever the label is drawn as rich text, so a jar literally named "Work · #3f2a" never looks like a genuine marker, and the built-in Default never looks like a user jar named "Default". Plain-text sites keep a plain string.

**Architecture:**
- **Two-part label.** `jar_defs::JarLabel { name: String, marker: Option<JarMarker> }`, `JarMarker::{Fragment(String), BuiltIn}`. `named_jar_label` and `App::jar_label` return it; it is still derived each time, never stored or synced.
  - `Display` gives the plain string: `"Work"`, `"Work · #3f2a"` (today's text, unchanged), and `"Default (built-in)"` for the built-in jar (today `"Default"`, which a user jar may also be called). Plain sites (confirm dialogs, the chip's will-switch tooltip, `SETJAR` label matching) call `to_string()`.
  - `App::jar_label(JarKey::Default)`, and a named key whose jar is gone, give the built-in label.
- **One look for every marker** (`JarLabel::append_to`, built on `name_format`/`marker_format`, which the chip uses directly to lay the parts out apart): the marker follows the name after a small gap, in monospace at 80% of the name's size, in the weak text colour, centred on the line. The fragment marker drops the ` · ` (rich text shows `Work  #3f2a`), so a typed `Work · #3f2a` differs in both style and text. The built-in's name is also italic. The name uses `Color32::PLACEHOLDER`, so a button or label still paints it in its own (hover, selected) colour.
- **Rich sites:** tab, unloaded and group menu rows (`JarRow.name: JarLabel`; `menu_button` takes `WidgetText`), Settings' Default row (italic "Default" + `built-in` marker) and a same-named jar's marker beside its name field (the field still edits only the stored name), Tab view's dot hover ("Jar: " + the styled label), and the card chip.
- **Chip:** the name and marker are separate galleys. `fit_jar_chip` reserves the marker's width first and ellipsizes only the name; when even a short name plus the marker do not fit, the chip falls back to today's dots-only steps. The chip's scrim is always near-black, so its marker colour is the chip text dimmed by the theme's `weak_text_alpha` (the same rule as egui's `weak_text_color`), not the panel's weak colour.
- **Not changed:** jar names are not restricted or sanitized (spec Open questions gains the app-wide name-rules item); `SETJAR default` (any case) still means the built-in jar.

**Spec:** `docs/superpowers/specs/2026-10-03-per-tab-jars-design.md` "Names". Builds on `docs/superpowers/plans/2026-10-03-jar-name-labels.md`.

## Tasks

### Task 1: `JarLabel`, with every caller on it
- [x] RED: tests in `jar_defs` for the split (unique: no marker; clash: `Fragment`; built-in and a missing jar: `BuiltIn`), the plain strings (`"Work"`, `"Work · #frag"`, `"Default (built-in)"`), and that existing plain callers (confirm text, will-switch target, `jar_named`) are unchanged for named jars.
- [x] GREEN: the type, `Display`; `named_jar_label`/`jar_label` return it; `JarChip.name` and `JarRow.name` hold it; plain sites call `to_string()`. The delete confirm's "they'll use Default." names the built-in through `jar_label`.

### Task 2: the shared style, in menus and Tab view
- [x] RED: `append_to` tests: name section placeholder-coloured and upright (italic for built-in), marker section monospace, smaller, weak-coloured, text `#frag` / `built-in`; no marker section for a unique name.
- [x] GREEN: `append_to`, `name_format`, `marker_format`, `JarChip::hover_job`; menu rows and Tab view hover use them.

### Task 3: Settings
- [x] Default row: the built-in label's job; the fragment marker beside a name field uses `marker_format`. The field edits the stored name (existing tests stay green).

### Task 4: chip keeps the marker
- [x] RED: `fit_jar_chip` tests: a marker always shown while the name ellipsizes; too narrow for name + marker falls back to dots as today; no marker behaves as before.
- [x] GREEN: `fit_jar_chip(max, lead, name, marker)`; `paint_jar_chip` lays out name and marker apart.

### Task 5: docs
- [x] Spec "Names" and Open questions (drop the look-alike item; add app-wide name rules), README "Per-tab jars".

### Task 6: live check (no commit)
- [x] `scripts/drive.sh` on a seeded throwaway profile: two "Work", a user "Default", a typed "Work · #3f2a"; tab menu, group menu, Settings at 480 px and wider, a narrow card's chip, into `target/shots/jarstyle/`. The app forces a dark theme (`theme::apply_style`), so there is no light theme to shoot.

## Outcome (2026-10-03)

- Done as planned, except: there is no `JarLabel::layout_job`; every rich site builds its own `LayoutJob` through `append_to`.
- The live check found the menu rows drawn in the body serif, with the tick's glyph missing: menus and Settings set an override font (`theme::control_font`) that `TextStyle::Body`/`Button` ignore. The styled sites now resolve `FontSelection::Default`, as egui widgets do, and the menu test compares the row font with a plain row's.
- Shots in `target/shots/jarstyle/` (not committed). Only the dark theme exists.
- At about 48% zoom a chip with a marker keeps the marker and only "…" of the name (`JAR_CHIP_MIN_NAME` is 14 points); below that it falls back to the dots, sooner than a chip without a marker would.
