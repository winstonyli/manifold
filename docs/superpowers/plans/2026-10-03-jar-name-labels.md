---
status: done
---

# Jar Name Labels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Jar names need not be unique. Wherever a jar's name is shown, two jars with the same name stay apart by a derived marker, so the user can always tell them apart without any uniqueness rule.

**Architecture:**
- **Identity.** A jar is its node id, never its name. Import (Open Session, Import Session, Import Group, Reopen Closed) no longer matches jars by name: every imported jar node becomes a new jar under its fresh id. This retires the per-tab jars plan's deviation 4.
- **Label.** A pure function, `jar_defs::named_jar_label(id, &jar_defs) -> String`, derives the label; it is never stored or synced.
  - A unique name (compared trimmed and case-insensitively) is shown unchanged.
  - When two or more jars share a name, each one's label is `"<name> · #<frag>"`.
  - `<frag>` is a prefix of the SHA-256 hex of the node id's text (`sha2` is already a dependency). A plain hex prefix of the id would not work: jars made on one device share the id's actor half and differ only in the decimal counter. The prefix is 4 characters, lengthened just enough that the clashing jars' fragments differ.
  - `App::jar_label(key)` calls it, so the chip, its tooltip, Tab view's dot, the will-switch text and the confirm dialogs all get the label.
- **Lookup.** `jar_named(text)` (headless `SETJAR`) matches a label first, then a stored name. Several jars with that stored name: the lowest node id (`jar_defs`' `BTreeMap` order) wins.
- **Settings.** The name field edits the stored name. A clashing jar shows its marker (`· #frag`) as separate read-only text beside the tab count, so the marker can never be saved into the name.
- **Not added:** uniqueness enforcement, a created-at field, or any new synced field. A rename to a name in use is allowed.

**Tech Stack:** Rust, egui, `sha2`.

**Spec:** `docs/superpowers/specs/2026-10-03-per-tab-jars-design.md` (gains a "Names" paragraph). Builds on `docs/superpowers/plans/2026-10-03-per-tab-jars.md`.

## Display sites

These are every place a jar's name reaches the screen or stdout, found by grepping `.name`, `jar_label` and `JarRow`:
- `jar_defs.rs`:
  - `jar_chip`, which gives the chip and Tab view their `name` and the will-switch target;
  - `jar_confirm_text`: "Clear cookies in X?", "Delete jar X?" and "Make X ephemeral?". Its "N tabs use this" detail names no jar.
- `node_menu.rs` `jar_menu`, the `JarRow.name` shared by the tab, unloaded and group menus.
- `settings_page.rs` `settings_jars_section`, the name field and the new marker.
- `tab_view.rs`, the dot's "Jar: X" hover, via `JarChip.name`.
- Debug commands: none prints a jar name. `SETJAR`'s help text changes to say it accepts a label.

## Tasks

### Task 1: the label function, used by `App::jar_label` and the menus
- [ ] Tests in `jar_defs::tests`:
  - a unique name is plain;
  - "Work", " work " and "WORK" clash, and each gets `· #` plus 4 hex characters, all different;
  - two ids whose 4-character fragments collide (found by search in the test) get longer, distinct fragments, while a third clashing jar keeps 4 characters if they suffice;
  - a label does not change when a jar with another name is added or deleted;
  - `jar_chip`, `jar_confirm_text` and `jar_menu` rows show the label.
- [ ] Add `jar_fragment(id, defs) -> Option<String>` and `named_jar_label(id, defs)`. Make `App::jar_label` call them, and `jar_menu` use `named_jar_label`.

### Task 2: `jar_named` takes a label, then a stored name
- [ ] Tests:
  - `SETJAR "Work · #frag"` picks that jar;
  - `SETJAR Work` with two "Work" jars picks the lower id and makes no third jar;
  - a unique name still works.
- [ ] Implement it. Update the `SETJAR` doc comment and help text, and the README headless line.

### Task 3: the Settings marker
- [ ] Tests in `settings_page::tests`:
  - with two "Work" jars the page draws each marker;
  - focusing a field and leaving it unchanged requests no rename;
  - typing into it requests the typed stored name, with no `#` in it;
  - the 480 px fit test includes a clashing pair.
- [ ] Pass the marker into `head` as a weak label right of the field. The field still starts from `def.name`.

### Task 4: import never reuses a jar
- [ ] Rewrite `opening_a_session_reuses_a_jar_with_the_same_name` as `opening_a_session_gives_each_imported_jar_a_new_jar`:
  - an existing "Work" is kept untouched;
  - the imported "Work" is a second jar with a fresh id, not in `jar_defs` before;
  - the imported tab's `jar` and the imported group's `default_jar` name the fresh id.
- [ ] Add the same check for Import Group. Keep `teardown_running_session` keeping `jar_defs`.
- [ ] Delete the alias map in `materialize_imported`.

### Task 5: docs
- [ ] Spec: a "Names" paragraph under "UI", plus an Open questions note.
- [ ] Old plan deviation 4: struck, with a retirement note (2026-10-03) and the reason.
- [ ] README "Per-tab jars": a sentence on labels.

### Task 6: live check (no commit)
- [ ] `cargo build -j 8 -p manifold`.
- [ ] Use `scripts/drive.sh` on a throwaway profile:
  - two tabs on two jars both named "Work" (`SETJAR` makes the first; the second comes from "New Jar..." with `EGUITEXT`), and a third on "Home";
  - screenshot the tab menu, the cards' chips and Settings into `target/shots/jarlabels/`.
