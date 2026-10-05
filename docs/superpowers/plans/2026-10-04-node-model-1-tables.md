# Node model, plan 1: capability and edge tables, behavior-neutral consumers

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the MIME pattern matcher, the capability row table with `caps()`, and the edge-type table, and move four per-kind decisions onto them without changing any behavior.

**Architecture:** A new `caps` module holds plain-data rows keyed by MIME type (vendor `application/vnd.manifold.*` types for the app's own kinds). A thin `bridge` maps today's two kind enums (`store::model::Kind`, `ItemNodeKind`) to `(mime, tags)`; plan 2 deletes the enums and the bridge. Rows describe today's behavior only.

**Tech Stack:** Rust (egui app, crate `manifold`), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-04-node-model-design.md` (sections 1 to 3 and "Rename plan and order", plan 1).

## Global Constraints

- Work on `dev`, never `main`. No new worktrees. Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` (use your own model's name if different).
- Run `cargo fmt --all` before each commit. Before the final task is done, `cargo fmt --all --check` and `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` must pass. `.githooks/pre-commit` runs both when Rust files are staged; never use `--no-verify`.
- Always pass `-j 8` to cargo. Test command: `cargo test -j 8 -p manifold --bins <filter>`. Baseline before this plan: 696 passed, 4 ignored.
- No `#[allow(dead_code)]` (clippy runs with `-D warnings` in a bin crate): every non-test item must be used by non-test code in the same commit. Test-only helpers are `#[cfg(test)]`.
- No test may read or write the real `%APPDATA%\manifold`. These tests are pure and need no files.
- Behavior-neutral: the existing tests `drop_action_matches_the_drop_table` (`card.rs`), `card_control_actions...` near `card.rs:2672`, `del_hint_*` (`node_menu.rs`) and the `closed.rs` tests must pass unchanged. Do not edit an existing assertion to make it pass.
- Python, if used, runs as `python -P script < /dev/null`.
- Do not push, merge or tag.

## Rulings (spec gaps decided here)

- **Rank.** A row's rank is the pair `(pattern rank, number of `requires` tags)`, compared lexicographically; pattern rank is `*/*` 0, `image/*` 1, `*/*+json` 2, `application/vnd.manifold.*` 3, exact 4. (The spec said tags "raise rank" without saying whether they outrank a more specific pattern; they do not.) Equal rank: the later registration wins; the same pattern and tags registered twice is an error.
- **No cache.** The spec's per-(type, tags) cache is not built: there are about nine rows, so merging per call costs nothing. Add it only if profiling shows a need (amend the spec then).
- **No `#[non_exhaustive]` on `Tag`.** It has no effect inside the defining crate.
- **Only fields with a consumer.** Rows carry just `restorable` now. `pinnable`, `renamable`, `portable`, `activate`, `fields`, `draw_layer` arrive with their consumers in plans 2 and 3. Tags carry just `Loaded` and `Blank`; the edge table has just `contains`, with endpoints only (no cardinality, `acyclic`, removal or drawing yet).
- **Bridge types** (replaced by real MIME types in plan 2): `Tab` and `Unloaded` map to `application/vnd.manifold.tab` (a loaded tab also gets `Tag::Loaded`); `Group` to `application/vnd.manifold.group`; `Note` to `text/markdown`; `Image` to `image/png` (stands for `image/*`); `Pdf` to `application/pdf`; `Json` to `application/json`; `JsonLeaf` to `application/vnd.manifold.json-leaf`; `Download` to `application/octet-stream`; `Jar` to `application/vnd.manifold.jar`; `Unknown` to `application/x-manifold-unknown`.

## File structure

- Create `crates/manifold/src/caps.rs`: module root, re-exports, the `registry()` and `edges()` singletons.
- Create `crates/manifold/src/caps/pattern.rs`: `Pattern`, `Tag`, `TagSet`.
- Create `crates/manifold/src/caps/rows.rs`: `Row`, `Caps`, `Registry`, `builtin_rows()`.
- Create `crates/manifold/src/caps/edges.rs`: `EdgeRow`, `EdgeTable`, `builtin_edges()`.
- Create `crates/manifold/src/caps/bridge.rs`: `store_node`, `item_node`, `is_container*`, `can_contain`.
- Modify `crates/manifold/src/main.rs` (add `mod caps;` in alphabetical position, after `mod bookmarks;`), `closed.rs`, `card.rs`.

---

### Task 1: Pattern matcher, tags, capability rows, `worth_reopening` on `caps()`

One commit: the new items are only non-dead once `worth_reopening` uses them, so do not commit before Step 8.

**Files:**
- Create: `crates/manifold/src/caps.rs`, `crates/manifold/src/caps/pattern.rs`, `crates/manifold/src/caps/rows.rs`, `crates/manifold/src/caps/bridge.rs`
- Modify: `crates/manifold/src/main.rs` (module list), `crates/manifold/src/closed.rs:30-52` (`worth_reopening`) and its `mod tests`

**Interfaces:**
- Produces:
  - `caps::pattern::{Pattern, Tag, TagSet}` with `Pattern::parse(&str) -> Pattern`, `Pattern::rank(&self) -> u8`, `Pattern::matches(&self, mime: &str) -> bool`, `TagSet::EMPTY`, `TagSet::of(&[Tag]) -> TagSet`, `TagSet::contains_all(self, other: TagSet) -> bool`, `TagSet::len(self) -> u32`.
  - `caps::rows::{Row, Caps, Registry}` with `Row::new(&str) -> Row`, `Row::requires(self, &[Tag]) -> Row`, `Row::restorable(self, bool) -> Row`, `Registry::new() -> Registry`, `Registry::register(&mut self, Row) -> Result<(), String>`, `Registry::caps(&self, mime: &str, tags: TagSet) -> Caps`, `Caps { pub restorable: bool }`.
  - `caps::registry() -> &'static Registry` (the builtin rows).
  - `caps::bridge::store_node(kind: Kind, fields: &Fields) -> (&'static str, TagSet)`.

- [ ] **Step 1: Wire the module and write the pattern tests**

In `main.rs`, add `mod caps;` after `mod bookmarks;`. Create `caps.rs`:

```rust
//! Capability and edge tables keyed by MIME type (spec 2026-10-04-node-model-design.md, sections 1 to 3).

pub(crate) mod bridge;
mod pattern;
mod rows;

pub(crate) use pattern::{Tag, TagSet};
use rows::{builtin_rows, Registry};
use std::sync::OnceLock;

/// The builtin capability rows.
pub(crate) fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::new();
        for row in builtin_rows() {
            registry.register(row).expect("builtin rows are distinct");
        }
        registry
    })
}
```

Create `caps/pattern.rs` with only the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_picks_the_pattern_kind() {
        assert_eq!(Pattern::parse("*/*"), Pattern::Any);
        assert_eq!(Pattern::parse("image/*"), Pattern::Type("image".into()));
        assert_eq!(Pattern::parse("*/*+json"), Pattern::Suffix("json".into()));
        assert_eq!(
            Pattern::parse("application/vnd.manifold.*"),
            Pattern::Prefix { ty: "application".into(), prefix: "vnd.manifold.".into() }
        );
        assert_eq!(Pattern::parse(" Text/Markdown "), Pattern::Exact("text/markdown".into()));
    }

    #[test]
    fn matches_each_pattern_kind() {
        assert!(Pattern::parse("*/*").matches("anything/at-all"));
        assert!(Pattern::parse("image/*").matches("image/png"));
        assert!(!Pattern::parse("image/*").matches("video/mp4"));
        assert!(Pattern::parse("*/*+json").matches("application/ld+json"));
        assert!(!Pattern::parse("*/*+json").matches("application/json"));
        assert!(Pattern::parse("application/vnd.manifold.*").matches("application/vnd.manifold.tab"));
        assert!(!Pattern::parse("application/vnd.manifold.*").matches("text/vnd.manifold.tab"));
        assert!(Pattern::parse("text/markdown").matches("TEXT/Markdown"));
        assert!(!Pattern::parse("text/markdown").matches("text/plain"));
    }

    #[test]
    fn a_type_with_no_slash_matches_only_the_catch_all() {
        assert!(Pattern::parse("*/*").matches("odd"));
        assert!(!Pattern::parse("image/*").matches("odd"));
        assert!(!Pattern::parse("odd").matches("other"));
    }

    #[test]
    fn rank_orders_the_patterns() {
        let ranks: Vec<u8> = ["*/*", "image/*", "*/*+json", "application/vnd.manifold.*", "text/markdown"]
            .iter()
            .map(|p| Pattern::parse(p).rank())
            .collect();
        assert_eq!(ranks, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn tag_sets_hold_and_count_tags() {
        let both = TagSet::of(&[Tag::Loaded, Tag::Blank]);
        assert_eq!(both.len(), 2);
        assert!(both.contains_all(TagSet::of(&[Tag::Blank])));
        assert!(both.contains_all(TagSet::EMPTY));
        assert!(!TagSet::of(&[Tag::Loaded]).contains_all(both));
        assert_eq!(TagSet::of(&[Tag::Blank, Tag::Blank]).len(), 1);
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test -j 8 -p manifold --bins caps::pattern`
Expected: compile error, `Pattern` and `TagSet` not found.

- [ ] **Step 3: Implement `Pattern`, `Tag`, `TagSet`**

Prepend to `caps/pattern.rs` (above the tests):

```rust
//! MIME patterns and the tags that refine a match.

/// A MIME pattern, parsed once. Case-insensitive.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Pattern {
    /// `*/*`.
    Any,
    /// `image/*`: any subtype of one type.
    Type(String),
    /// `*/*+json`: any type whose subtype ends in `+json`.
    Suffix(String),
    /// `application/vnd.manifold.*`: a type and a subtype prefix.
    Prefix { ty: String, prefix: String },
    Exact(String),
}

impl Pattern {
    pub(crate) fn parse(text: &str) -> Pattern {
        let text = text.trim().to_ascii_lowercase();
        if text == "*/*" {
            return Pattern::Any;
        }
        if let Some(suffix) = text.strip_prefix("*/*+") {
            return Pattern::Suffix(suffix.to_owned());
        }
        if let Some((ty, sub)) = text.split_once('/') {
            if sub == "*" {
                return Pattern::Type(ty.to_owned());
            }
            if let Some(prefix) = sub.strip_suffix('*') {
                return Pattern::Prefix { ty: ty.to_owned(), prefix: prefix.to_owned() };
            }
        }
        Pattern::Exact(text)
    }

    /// Low to high: `*/*`, type wildcard, suffix wildcard, subtype prefix, exact.
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Pattern::Any => 0,
            Pattern::Type(_) => 1,
            Pattern::Suffix(_) => 2,
            Pattern::Prefix { .. } => 3,
            Pattern::Exact(_) => 4,
        }
    }

    pub(crate) fn matches(&self, mime: &str) -> bool {
        let mime = mime.to_ascii_lowercase();
        let Some((ty, sub)) = mime.split_once('/') else { return matches!(self, Pattern::Any) };
        match self {
            Pattern::Any => true,
            Pattern::Type(t) => ty == t,
            Pattern::Suffix(s) => sub.strip_suffix(s.as_str()).is_some_and(|rest| rest.ends_with('+')),
            Pattern::Prefix { ty: t, prefix } => ty == t && sub.starts_with(prefix.as_str()),
            Pattern::Exact(e) => mime == *e,
        }
    }
}

/// A runtime fact about a node that refines which rows apply (spec "Tags"). Not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tag {
    /// A tab with a running engine.
    Loaded,
    /// A note with no text.
    Blank,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct TagSet(u16);

impl TagSet {
    pub(crate) const EMPTY: TagSet = TagSet(0);

    pub(crate) fn of(tags: &[Tag]) -> TagSet {
        TagSet(tags.iter().fold(0, |bits, &tag| bits | 1 << tag as u16))
    }

    /// Whether every tag in `other` is in `self`.
    pub(crate) fn contains_all(self, other: TagSet) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) fn len(self) -> u32 {
        self.0.count_ones()
    }
}
```

- [ ] **Step 2b: Run the pattern tests**

Run: `cargo test -j 8 -p manifold --bins caps::pattern`
Expected: 5 passed. (Dead-code warnings are expected until Step 8; do not commit yet.)

- [ ] **Step 4: Write the row tests**

Create `caps/rows.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::pattern::Tag;

    fn registry(rows: Vec<Row>) -> Registry {
        let mut registry = Registry::new();
        for row in rows {
            registry.register(row).unwrap();
        }
        registry
    }

    #[test]
    fn nothing_matching_gives_the_defaults() {
        assert!(!Registry::new().caps("text/markdown", TagSet::EMPTY).restorable);
    }

    #[test]
    fn a_higher_pattern_overrides_a_lower_one_per_field() {
        let r = registry(vec![
            Row::new("*/*").restorable(false),
            Row::new("image/*").restorable(true),
            Row::new("image/gif").restorable(false),
        ]);
        assert!(r.caps("image/png", TagSet::EMPTY).restorable);
        assert!(!r.caps("image/gif", TagSet::EMPTY).restorable);
        assert!(!r.caps("video/mp4", TagSet::EMPTY).restorable);
    }

    #[test]
    fn registration_order_does_not_matter_across_ranks() {
        let high_first = registry(vec![Row::new("image/gif").restorable(true), Row::new("*/*").restorable(false)]);
        assert!(high_first.caps("image/gif", TagSet::EMPTY).restorable);
    }

    #[test]
    fn the_same_pattern_and_tags_twice_is_an_error() {
        let mut r = Registry::new();
        r.register(Row::new("image/*")).unwrap();
        assert!(r.register(Row::new("IMAGE/*")).is_err());
        r.register(Row::new("image/*").requires(&[Tag::Blank])).unwrap();
    }

    #[test]
    fn at_equal_rank_the_later_registration_wins() {
        // Both are subtype-prefix patterns (rank 3) and both match `application/vnd.abc`.
        let r = registry(vec![
            Row::new("application/vnd.a*").restorable(true),
            Row::new("application/vnd.ab*").restorable(false),
        ]);
        assert!(!r.caps("application/vnd.abc", TagSet::EMPTY).restorable);
    }

    #[test]
    fn a_required_tag_makes_a_row_apply_only_with_it_and_outrank_its_twin() {
        let r = registry(vec![
            Row::new("text/markdown").restorable(true),
            Row::new("text/markdown").requires(&[Tag::Blank]).restorable(false),
        ]);
        assert!(r.caps("text/markdown", TagSet::EMPTY).restorable);
        assert!(!r.caps("text/markdown", TagSet::of(&[Tag::Blank])).restorable);
    }

    #[test]
    fn a_tag_does_not_outrank_a_more_specific_pattern() {
        let r = registry(vec![
            Row::new("image/*").requires(&[Tag::Loaded]).restorable(false),
            Row::new("image/png").restorable(true),
        ]);
        assert!(r.caps("image/png", TagSet::of(&[Tag::Loaded])).restorable);
    }
}
```

- [ ] **Step 5: Run the row tests to see them fail**

Run: `cargo test -j 8 -p manifold --bins caps::rows`
Expected: compile error, `Registry`, `Row` not found.

- [ ] **Step 6: Implement rows, registry and the builtin rows**

Prepend to `caps/rows.rs` (above the tests):

```rust
//! The capability row table: which rows apply to a node, layered by rank (spec section 2).

use super::pattern::{Pattern, Tag, TagSet};

/// One row. A `None` field says nothing, so a lower-ranked row's value stands.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pattern: Pattern,
    requires: TagSet,
    restorable: Option<bool>,
}

impl Row {
    pub(crate) fn new(pattern: &str) -> Row {
        Row { pattern: Pattern::parse(pattern), requires: TagSet::EMPTY, restorable: None }
    }

    pub(crate) fn requires(mut self, tags: &[Tag]) -> Row {
        self.requires = TagSet::of(tags);
        self
    }

    pub(crate) fn restorable(mut self, restorable: bool) -> Row {
        self.restorable = Some(restorable);
        self
    }

    fn rank(&self) -> (u8, u32) {
        (self.pattern.rank(), self.requires.len())
    }
}

/// A node's merged capabilities.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Caps {
    /// Closing it is recorded, so Recently Closed can reopen it.
    pub(crate) restorable: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    rows: Vec<Row>,
}

impl Registry {
    pub(crate) fn new() -> Registry {
        Registry::default()
    }

    /// Adds a row. The same pattern and required tags twice is an error.
    pub(crate) fn register(&mut self, row: Row) -> Result<(), String> {
        if self.rows.iter().any(|r| r.pattern == row.pattern && r.requires == row.requires) {
            return Err(format!("capability row {:?} with tags {:?} is already registered", row.pattern, row.requires));
        }
        self.rows.push(row);
        Ok(())
    }

    /// The rows matching `mime` whose required tags `tags` holds, applied from lowest rank to highest (equal rank:
    /// registration order), each overriding the fields it sets.
    pub(crate) fn caps(&self, mime: &str, tags: TagSet) -> Caps {
        let mut matching: Vec<&Row> =
            self.rows.iter().filter(|r| r.pattern.matches(mime) && tags.contains_all(r.requires)).collect();
        matching.sort_by_key(|r| r.rank()); // stable: ties keep registration order
        let mut caps = Caps::default();
        for row in matching {
            if let Some(restorable) = row.restorable {
                caps.restorable = restorable;
            }
        }
        caps
    }
}

/// What today's `worth_reopening` decided per kind: only a completed download, a jar, a blank note and an unknown kind
/// are not recorded when closed.
pub(crate) fn builtin_rows() -> Vec<Row> {
    use super::bridge::{GROUP, JSON_LEAF, TAB, TEXT_MARKDOWN};
    vec![
        Row::new("*/*").restorable(false),
        Row::new(TAB).restorable(true),
        Row::new(GROUP).restorable(true),
        Row::new("image/*").restorable(true),
        Row::new("application/pdf").restorable(true),
        Row::new("application/json").restorable(true),
        Row::new(JSON_LEAF).restorable(true),
        Row::new(TEXT_MARKDOWN).restorable(true),
        Row::new(TEXT_MARKDOWN).requires(&[Tag::Blank]).restorable(false),
    ]
}
```

(`TagSet` needs `Debug`, already derived.)

- [ ] **Step 7: Bridge, and `worth_reopening` through `caps()`**

Create `caps/bridge.rs`:

```rust
//! Maps today's kind enums to `(mime, tags)`. Plan 2 deletes the enums and this file with them.

use super::pattern::{Tag, TagSet};
use crate::session_sync::Fields;
use crate::store::model::{field, FieldValue, Kind};

pub(crate) const TAB: &str = "application/vnd.manifold.tab";
pub(crate) const GROUP: &str = "application/vnd.manifold.group";
pub(crate) const JSON_LEAF: &str = "application/vnd.manifold.json-leaf";
pub(crate) const TEXT_MARKDOWN: &str = "text/markdown";

/// A stored node's type and tags, as `App::shared_nodes` sees it: a note with no (or only blank) text is `Blank`.
pub(crate) fn store_node(kind: Kind, fields: &Fields) -> (&'static str, TagSet) {
    let none = TagSet::EMPTY;
    match kind {
        Kind::Tab => (TAB, none),
        Kind::Group => (GROUP, none),
        Kind::Note => {
            let blank = !matches!(fields.get(&field::TEXT), Some(FieldValue::Str(text)) if !text.trim().is_empty());
            (TEXT_MARKDOWN, if blank { TagSet::of(&[Tag::Blank]) } else { none })
        }
        Kind::Image => ("image/png", none),
        Kind::Pdf => ("application/pdf", none),
        Kind::Json => ("application/json", none),
        Kind::JsonLeaf => (JSON_LEAF, none),
        Kind::Download => ("application/octet-stream", none),
        Kind::Jar => ("application/vnd.manifold.jar", none),
        Kind::Unknown(_) => ("application/x-manifold-unknown", none),
    }
}
```

In `closed.rs`, replace the body of `worth_reopening` (keep its doc comment, update the last sentence to say the rows in `caps/rows.rs` decide):

```rust
pub(crate) fn worth_reopening(kind: store::model::Kind, fields: &session_sync::Fields) -> bool {
    let (mime, tags) = crate::caps::bridge::store_node(kind, fields);
    crate::caps::registry().caps(mime, tags).restorable
}
```

Add to `closed.rs`'s `mod tests` (the oracle is the old function's table, written out):

```rust
    #[test]
    fn worth_reopening_is_what_it_was_for_every_kind() {
        use store::model::{FieldValue, Kind};
        let text = |s: &str| session_sync::Fields::from([(field::TEXT, FieldValue::Str(s.into()))]);
        let none = session_sync::Fields::new();
        for (kind, fields, want) in [
            (Kind::Tab, &none, true),
            (Kind::Group, &none, true),
            (Kind::Image, &none, true),
            (Kind::Pdf, &none, true),
            (Kind::Json, &none, true),
            (Kind::JsonLeaf, &none, true),
            (Kind::Download, &none, false),
            (Kind::Jar, &none, false),
            (Kind::Unknown(99), &none, false),
            (Kind::Note, &text("hi"), true),
            (Kind::Note, &text("  \n"), false),
            (Kind::Note, &none, false),
        ] {
            assert_eq!(worth_reopening(kind, fields), want, "{kind:?}");
        }
    }
```

- [ ] **Step 8: Run everything, format, commit**

Run: `cargo fmt --all` then `cargo test -j 8 -p manifold --bins` (expect 696 + the new tests, 4 ignored) and `cargo clippy -j 8 -p manifold --all-targets -- -D warnings` (expect clean: `Tag::Loaded` is not constructed outside tests yet, which is a dead-code error; if clippy reports it, move `Loaded` and the `loaded` assertions into Task 2, where `item_node` constructs it).

```bash
git add crates/manifold/src/main.rs crates/manifold/src/caps.rs crates/manifold/src/caps crates/manifold/src/closed.rs
git commit -m "Add the capability row table; worth_reopening reads it

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Edge table; drop rules, group close and closed-group walk on it

**Files:**
- Create: `crates/manifold/src/caps/edges.rs`
- Modify: `crates/manifold/src/caps.rs` (add `mod edges;`, `edges()`), `caps/bridge.rs` (`item_node`, `is_container`, `is_container_store`, `can_contain`), `card.rs:495-504` (`CardControl::action`), `card.rs:593-603` (`drop_action`), `closed.rs:130-135`

**Interfaces:**
- Consumes: `Pattern` (Task 1).
- Produces:
  - `caps::edges::{EdgeRow, EdgeTable}` with `EdgeRow::new(id: &'static str, sources: &[&str], targets: &[&str]) -> EdgeRow`, `EdgeTable::new(rows: Vec<EdgeRow>) -> EdgeTable`, `EdgeTable::can_connect(&self, id: &str, source: &str, target: &str) -> bool`, `EdgeTable::target_accepts(&self, id: &str, target: &str) -> bool`, `pub(crate) const CONTAINS: &str`.
  - `caps::edges() -> &'static EdgeTable`.
  - `caps::bridge::{item_node(ItemNodeKind) -> (&'static str, TagSet), is_container(ItemNodeKind) -> bool, is_container_store(Kind) -> bool, can_contain(group: ItemNodeKind, member: ItemNodeKind) -> bool}`.

- [ ] **Step 1: Write the edge tests**

Create `caps/edges.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> EdgeTable {
        EdgeTable::new(vec![
            EdgeRow::new("contains", &["application/vnd.manifold.tab", "image/*"], &["application/vnd.manifold.group"]),
            EdgeRow::new("sees", &["*/*"], &["*/*+json"]),
        ])
    }

    #[test]
    fn can_connect_checks_both_endpoints_against_the_row() {
        let t = table();
        assert!(t.can_connect("contains", "application/vnd.manifold.tab", "application/vnd.manifold.group"));
        assert!(t.can_connect("contains", "image/gif", "application/vnd.manifold.group"));
        assert!(!t.can_connect("contains", "application/vnd.manifold.group", "application/vnd.manifold.tab"));
        assert!(!t.can_connect("contains", "video/mp4", "application/vnd.manifold.group"));
    }

    #[test]
    fn an_unknown_edge_type_connects_nothing() {
        assert!(!table().can_connect("nope", "image/gif", "image/gif"));
        assert!(!table().target_accepts("nope", "image/gif"));
    }

    #[test]
    fn target_accepts_looks_at_the_target_alone() {
        let t = table();
        assert!(t.target_accepts("sees", "application/ld+json"));
        assert!(!t.target_accepts("sees", "application/json"));
    }

    #[test]
    fn builtin_contains_takes_every_card_into_a_group_only() {
        use crate::caps::bridge::{GROUP, TAB};
        let t = edges();
        for source in
            [TAB, GROUP, "text/markdown", "image/png", "application/pdf", "application/json", "application/octet-stream"]
        {
            assert!(t.can_connect(CONTAINS, source, GROUP), "{source}");
            assert!(!t.can_connect(CONTAINS, source, TAB), "{source}");
        }
        assert!(!t.can_connect(CONTAINS, "application/vnd.manifold.jar", GROUP));
    }
}
```

- [ ] **Step 2: Run to see them fail**

Run: `cargo test -j 8 -p manifold --bins caps::edges`
Expected: compile error, `EdgeTable` not found.

- [ ] **Step 3: Implement the edge table**

Prepend to `caps/edges.rs`:

```rust
//! The edge-type table: which node types an edge may join (spec section 3). Endpoints only for now; cardinality,
//! `acyclic`, removal variants and drawing arrive with the code that reads them.

use super::bridge::{GROUP, JSON_LEAF, TAB, TEXT_MARKDOWN};
use super::pattern::Pattern;

/// The containment edge: a node is shown inside a group.
pub(crate) const CONTAINS: &str = "contains";

#[derive(Debug, Clone)]
pub(crate) struct EdgeRow {
    id: &'static str,
    sources: Vec<Pattern>,
    targets: Vec<Pattern>,
}

impl EdgeRow {
    pub(crate) fn new(id: &'static str, sources: &[&str], targets: &[&str]) -> EdgeRow {
        EdgeRow {
            id,
            sources: sources.iter().map(|p| Pattern::parse(p)).collect(),
            targets: targets.iter().map(|p| Pattern::parse(p)).collect(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct EdgeTable {
    rows: Vec<EdgeRow>,
}

impl EdgeTable {
    pub(crate) fn new(rows: Vec<EdgeRow>) -> EdgeTable {
        EdgeTable { rows }
    }

    fn row(&self, id: &str) -> Option<&EdgeRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Whether a `source`-typed node may have a `target`-typed one on the other end of an `id` edge.
    pub(crate) fn can_connect(&self, id: &str, source: &str, target: &str) -> bool {
        self.row(id).is_some_and(|r| {
            r.sources.iter().any(|p| p.matches(source)) && r.targets.iter().any(|p| p.matches(target))
        })
    }

    /// Whether any node may be the target of an `id` edge (a group, for `contains`).
    pub(crate) fn target_accepts(&self, id: &str, target: &str) -> bool {
        self.row(id).is_some_and(|r| r.targets.iter().any(|p| p.matches(target)))
    }
}

pub(crate) fn builtin_edges() -> EdgeTable {
    EdgeTable::new(vec![EdgeRow::new(
        CONTAINS,
        &[TAB, GROUP, TEXT_MARKDOWN, "image/*", "application/pdf", "application/json", JSON_LEAF, "application/octet-stream"],
        &[GROUP],
    )])
}
```

In `caps.rs` add `mod edges;`, `pub(crate) use edges::CONTAINS;` is not needed (bridge uses `edges::CONTAINS` via `super::edges`); add:

```rust
/// The builtin edge rows.
pub(crate) fn edges() -> &'static edges::EdgeTable {
    static EDGES: OnceLock<edges::EdgeTable> = OnceLock::new();
    EDGES.get_or_init(edges::builtin_edges)
}
```

and declare `pub(crate) mod edges;` (the test module reaches `CONTAINS` and `edges()` through `super::*`; `edges()` lives in `caps.rs`, so in the test use `crate::caps::edges()`; adjust the `use` in the test accordingly).

- [ ] **Step 4: Bridge helpers, consumers and tests**

Append to `caps/bridge.rs` (add `use crate::node::ItemNodeKind;`, `use super::edges::CONTAINS;`):

```rust
/// A card's type and tags. A loaded tab is `Loaded`; a sleeping one is the same type without it.
pub(crate) fn item_node(kind: ItemNodeKind) -> (&'static str, TagSet) {
    match kind {
        ItemNodeKind::Tab => (TAB, TagSet::of(&[Tag::Loaded])),
        ItemNodeKind::Unloaded => (TAB, TagSet::EMPTY),
        ItemNodeKind::Group => (GROUP, TagSet::EMPTY),
        ItemNodeKind::Note => (TEXT_MARKDOWN, TagSet::EMPTY),
        ItemNodeKind::Image => ("image/png", TagSet::EMPTY),
        ItemNodeKind::Pdf => ("application/pdf", TagSet::EMPTY),
        ItemNodeKind::Json => ("application/json", TagSet::EMPTY),
        ItemNodeKind::Download => ("application/octet-stream", TagSet::EMPTY),
    }
}

/// Whether nodes can sit inside this kind of card (a group).
pub(crate) fn is_container(kind: ItemNodeKind) -> bool {
    super::edges().target_accepts(CONTAINS, item_node(kind).0)
}

/// `is_container` for a stored node's kind.
pub(crate) fn is_container_store(kind: Kind) -> bool {
    super::edges().target_accepts(CONTAINS, store_node(kind, &Fields::new()).0)
}

/// Whether a `member` card may be put inside a `group` card.
pub(crate) fn can_contain(group: ItemNodeKind, member: ItemNodeKind) -> bool {
    super::edges().can_connect(CONTAINS, item_node(member).0, item_node(group).0)
}
```

`card.rs`, `CardControl::action`: replace the Group arm

```rust
            CardControl::Close if crate::caps::bridge::is_container(kind) => NodeAction::Dissolve,
```

`card.rs`, `drop_action`: replace the body

```rust
pub(crate) fn drop_action(dragged: ItemNodeKind, target: DropTarget, inside_group: bool) -> Option<DropEffect> {
    use crate::caps::bridge::{can_contain, is_container};
    use ItemNodeKind::{Tab, Unloaded};
    match (dragged, target, inside_group) {
        (_, DropTarget::BackButton, true) => Some(DropEffect::Eject),
        (dragged, DropTarget::Card(onto), _) if is_container(dragged) && is_container(onto) => Some(DropEffect::Merge),
        (dragged, DropTarget::Card(onto), _) if can_contain(onto, dragged) => Some(DropEffect::Join),
        (Tab | Unloaded, DropTarget::Card(Tab | Unloaded), false) => Some(DropEffect::GroupWith),
        (Tab | Unloaded, DropTarget::Card(Tab | Unloaded), true) => Some(DropEffect::Reparent),
        _ => None,
    }
}
```

`closed.rs`, in `closed_entry`, replace the `is_group` closure

```rust
        |id| shared.get(id).is_some_and(|n| crate::caps::bridge::is_container_store(n.kind)),
```

Add to `caps/bridge.rs` a test module (the oracle is today's behavior written out):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [ItemNodeKind; 8] = [
        ItemNodeKind::Tab,
        ItemNodeKind::Unloaded,
        ItemNodeKind::Download,
        ItemNodeKind::Group,
        ItemNodeKind::Note,
        ItemNodeKind::Image,
        ItemNodeKind::Pdf,
        ItemNodeKind::Json,
    ];

    #[test]
    fn only_a_group_card_is_a_container() {
        for kind in KINDS {
            assert_eq!(is_container(kind), kind == ItemNodeKind::Group, "{kind:?}");
        }
        for kind in [Kind::Tab, Kind::Note, Kind::Jar, Kind::JsonLeaf, Kind::Unknown(7)] {
            assert!(!is_container_store(kind), "{kind:?}");
        }
        assert!(is_container_store(Kind::Group));
    }

    #[test]
    fn every_card_kind_can_go_in_a_group_and_nothing_goes_in_anything_else() {
        for member in KINDS {
            for onto in KINDS {
                assert_eq!(can_contain(onto, member), onto == ItemNodeKind::Group, "{member:?} into {onto:?}");
            }
        }
    }

    #[test]
    fn a_loaded_tab_is_tagged_and_a_sleeping_one_is_not() {
        assert_eq!(item_node(ItemNodeKind::Tab).1, TagSet::of(&[Tag::Loaded]));
        assert_eq!(item_node(ItemNodeKind::Unloaded).1, TagSet::EMPTY);
        assert_eq!(item_node(ItemNodeKind::Tab).0, item_node(ItemNodeKind::Unloaded).0);
    }
}
```

- [ ] **Step 5: Run, format, gate, commit**

Run: `cargo fmt --all`, then `cargo test -j 8 -p manifold --bins` (all previous plus new pass; `drop_action_matches_the_drop_table`, the close-action test near `card.rs:2672` and the `del_hint_*` tests untouched and green), then `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings` (clean).

```bash
git add crates/manifold/src/caps.rs crates/manifold/src/caps crates/manifold/src/card.rs crates/manifold/src/closed.rs
git commit -m "Add the edge table; drop rules, group close and closed-group walk read it

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Record the plan in the docs

**Files:**
- Modify: `docs/superpowers/specs/2026-10-04-node-model-design.md` (section 2: replace the cache sentence with the no-cache ruling, state the rank rule, drop `#[non_exhaustive]`; "Rename plan and order": mark plan 1 done with its commits)
- Modify: `docs/superpowers/specs/2026-10-04-homogeneity-decisions.md` (one line under the 2026-10-04 update: plan 1 shipped, link to the plan)

- [ ] **Step 1: Edit the spec** so the "Rulings" section above is reflected (rank is `(pattern rank, tag count)`; no cache until measured; `Tag` is a plain enum), and add under "Rename plan and order": `Plan 1 done: <commit hashes from Tasks 1 and 2>. Plans 2 and 3 are written after it.`
- [ ] **Step 2: Verify** with `git diff --stat` that only the two spec files changed, then commit:

```bash
git add docs/superpowers
git commit -m "Docs: node model plan 1 done; spec rulings recorded

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 3: Final gate** (once, after all three commits): `cargo fmt --all --check`; `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2 --all-targets -- -D warnings`; `cargo test -j 8 -p manifold --bins`; `cargo test -j 8 -p ipc-protocol` (43 passed). Report any failure with its output; do not fix by editing an existing assertion.

## Self-review

- **Spec coverage (plan 1 scope only):** matcher and rank (Task 1); tags `Loaded` and `Blank` (Task 1); capability rows, layering, tie rule, duplicate error (Task 1); `restorable` replacing `worth_reopening` (Task 1); edge table with endpoints and `contains`, `can_connect`, `target_accepts` (Task 2); group-close, drop `Join`/`Merge` and the closed-group walk on the table (Task 2); docs (Task 3). Everything else in the spec belongs to plans 2 and 3 (type string in the journal, live state, verbs, `closed`, history, enum removal, fixtures, grep guard).
- **Deliberately not migrated:** `del_verb` (`card.rs:521`) and the per-kind menu in `node_menu_config`: they enumerate every kind for labels, so a table read would not remove a `match`. They go with the live-state rewrite.
- **Type consistency:** `Row::restorable` (builder) versus `Caps::restorable` (field) are distinct on purpose; `store_node` and `item_node` are the only mime sources; constants `TAB`, `GROUP`, `JSON_LEAF`, `TEXT_MARKDOWN` live in `bridge.rs` and are imported by `rows.rs` and `edges.rs`.
- **Known risk:** if clippy reports dead code for `Tag::Loaded` after Task 1, the fallback is written into Step 8.
