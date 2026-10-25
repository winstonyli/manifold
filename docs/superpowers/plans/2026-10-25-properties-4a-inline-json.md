---
status: done
---

# Properties 4a: Inline Json Cards (read-only) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, the user's choice) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Json card can hold its document in the sync document (the `BODY` tree) instead of a file: you can make one (New JSON), turn a file card into one (Detach to inline), see and browse it in the existing JSON level, and export it. Editing is 4b.

**Architecture:** `BODY` joins `TREE_FIELDS`. A new `store/body.rs` converts a `serde_json::Value` to and from a `Tree` (objects are `Key` segments, arrays are id-keyed maps of `{o, v}` where `o` is an order string, the root value sits under the fixed key `v`). `JsonPayload` gets a `source: File{path} | Inline{version}`; the level and card code read the value from the payload as before, so the JSON level needs no change beyond path-reader fixes and a value swap when a remote `BODY` change rebuilds the payload. Export/import writes `BODY` as a real JSON value under `"body"`.

**Tech Stack:** Rust, egui, `serde_json`; the `store` crate-module tree types (`Tree`, `TreePath`, `Leaf`, `Seg`).

**Spec:** `docs/superpowers/specs/2026-10-24-properties-design.md` (section 4 inline Json, 2.1 mirror, 2.3 export). Decisions: `docs/decisions/2026-10-24-properties-tags-json.md`. Plan 4b (write path and editing UI) is not written; this plan leaves it the seams listed under "Seams for 4b".

## Global Constraints

- Work on `main`; small topical commits; `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass (CLAUDE.md "Formatting and lints"). Never `--no-verify`.
- No test may read or write the real `%APPDATA%\manifold` (CLAUDE.md "Tests and the user's profile"): use `store::testutil::temp_dir` and `App::default()`.
- No migration code (memory: no-migration-until-users). `BODY` is new, so no stored data needs converting. Format changes themselves are fine; this plan adds a tree field and an export key, both purely additive (no `FORMAT_VERSION` bump: old readers keep unknown tree fields raw, `store/model.rs:505`).
- Limits: `MAX_DEPTH = 64` (`store/tree_json.rs:10`); `INLINE_MAX_LEAVES = 10_000` tree entries per inline body (new, Task 2). An element of an array costs two entries (`o`, plus its value's).
- `$ref` is plain data in `BODY` (spec 4); a `Leaf::Ref` assembles to `{"$ref": "<id>"}` and nothing in 4a creates one.
- Run each cargo command as `cargo test -j 8 -p manifold <filter>`; the gate (`gate.exe`) is run only in Task 9, through `~/.claude/runs/run.ps1` (CLAUDE.md "Gate").
- Never `python -` / stdin python; scripts run as `python -P f.py < /dev/null`. No fixed sleeps in tests.
- Agents never push, tag or bump. Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

## Review Focus

Each line has its pinning test in the named task.

1. **An array whose elements share an order key, or lack one** (two replicas insert at the same place; a partial merge leaves an element with `v` but no `o`). Expected: stable order (order string, then element id), never a panic, an element with no `v` is skipped. Task 2 `ties_and_gaps_in_order_are_stable`; a deep tree from another replica, Task 2 `a_hostile_deep_tree_assembles_without_overflowing`.
2. **A body that is huge or deep** (a 50 MB file detached, a 100-level nest typed into an import). Expected: refused with a message, card unchanged. Task 2 `too_many_leaves_or_too_deep_is_refused`, Task 7 `detach_over_the_cap_changes_nothing`.
3. **Root values that are not objects** (`[]`, `3`, `"s"`, `null`, `{}`), and keys that look special (`o`, `v`, `$ref`, `@1`). Expected: each round-trips exactly. Task 2 `every_root_shape_round_trips`.
4. **A remote replica edits `BODY` while the level is open** (selection, expansion, search must survive; a worker search must not report on the old value). Expected: the value swaps, the view state stays, a running search restarts. Task 5 `a_remote_body_change_swaps_the_open_levels_value`.
5. **A file card and an inline card behave the same everywhere a path was assumed** (Open Containing Folder, tooltip, crumb root, preview filter, missing-file retry). Expected: no empty-path file operations, the card name stands in. Task 6 `inline_cards_do_no_file_work`.

## File Structure

- Create `crates/manifold/src/store/body.rs`: `INLINE_MAX_LEAVES`, `initial_order`, `value_to_tree`, `tree_to_value`. One responsibility: the BODY dialect of a tree.
- Modify `crates/manifold/src/store/tree.rs` (`TREE_FIELDS`), `store/tree_json.rs` (share leaf helpers; `field_to_json` / `field_from_json` dispatch), `store/json.rs` (use the dispatch in export and import), `store/model.rs` (drop `dead_code` on `BODY`), `store/mod.rs` (declare `body`).
- Modify `node.rs` (`JsonSource`, `JsonPayload` constructors/accessors, `enterable`, `file_path`), and every `JsonPayload` literal / `.path` reader listed in Task 4 and Task 6.
- Modify `session_sync/remote.rs` (inline arm in `insert_materialized`, BODY hook in `update_live_node`), `json_level.rs` (`swap_value`), `session_sync/edits.rs` (`set_body`), `fetched.rs` (inline creation, detach), `graph_ui.rs` / `graph_ui/cards.rs` / `debug_control.rs` (New JSON), `node_menu.rs` + `caps/rows.rs` (menu), `crates/gate/src/smoke.rs` (stage).
- Tests live beside the code (`store/body.rs` `mod tests`, `json_level/tests.rs`, `session_sync/tests/{remote,writes}.rs`, `fetched/tests.rs`, `card/tests.rs`).

## Seams for 4b

`set_body(id, tree)` (Task 3) is the bulk writer 4b's one-undo-step edits sit beside (`edit_prop` per leaf stays the single-leaf writer). `JsonSource::Inline{version}` is where 4b's write path bumps the version. `initial_order` is the only order-key code; 4b adds `between(a, b)` next to it. `JsonView::retain_resolvable` (pointer remap after structural edits) and Ref rows are 4b.

---

### Task 1: BODY joins the tree fields; export/import dialect

**Files:**
- Modify: `crates/manifold/src/store/tree.rs:232-233`
- Modify: `crates/manifold/src/store/model.rs:301` (remove the `dead_code` allow on `BODY`)
- Modify: `crates/manifold/src/store/tree_json.rs` (add `field_to_json`, `field_from_json`; leaf helpers shared)
- Modify: `crates/manifold/src/store/json.rs:89-93` and `:290-299` (use the dispatch)
- Create: `crates/manifold/src/store/body.rs` (Task 2 fills it; Task 1 needs only the two signatures, so create it with the stubs below)
- Modify: `crates/manifold/src/store/mod.rs` (`pub mod body;`)
- Test: `crates/manifold/src/store/json.rs` `mod tests`

**Interfaces:**
- Consumes: `body::tree_to_value(&Tree) -> Value` and `body::value_to_tree(&Value, &mut dyn FnMut() -> NodeId) -> Result<Tree, String>` (Task 2 implements them; Task 1 adds them as working but minimal versions so export/import round-trips scalars and objects, and Task 2 replaces the bodies).
- Produces: `tree_json::field_to_json(no: u32, tree: &Tree) -> Value` and `tree_json::field_from_json(no: u32, value: &Value, resolve: &dyn Fn(&str) -> Option<NodeId>, mint: &mut dyn FnMut() -> NodeId) -> Result<Tree, String>`; `TREE_FIELDS` of length 2.

Design note: `import` already takes `next_id: impl FnMut() -> NodeId` (`store/json.rs:210`); array elements need fresh ids, so the tree loop passes it through as `mint`.

- [ ] **Step 1: Write the failing test** (in `store/json.rs` `mod tests`, next to the props tests at ~line 648)

```rust
    #[test]
    fn a_body_exports_as_plain_json_and_imports_with_fresh_element_ids() {
        let json = r##"{"nodes": [{"id": "a", "type": "application/json",
            "body": {"k": [1, {"x": true}, []], "e": {}, "$ref": "plain"}}]}"##;
        let doc: JsonDoc = serde_json::from_str(json).unwrap();
        let imported = import(&doc, ids(7), Trust::Own).unwrap();
        let tree = &imported[0].trees[&field::BODY];
        // Two array elements: the ids are the importer's (`ids(7)` mints under actor 7), not the file's.
        assert!(tree.keys().flat_map(|p| p.segments()).any(|s| matches!(s, Seg::Id(id) if id.actor.0 == 7)));
        let back = node_json("a", "application/json", &Default::default(), &imported[0].trees, None);
        assert_eq!(back.fields["body"], doc.nodes[0].fields["body"], "export is the document, arrays and all");
    }
```
(`ids(actor)` is the existing minting helper at `store/json.rs:346`. `Seg` import: `use super::tree::Seg;` in the test module if absent.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j 8 -p manifold a_body_exports_as_plain_json`
Expected: FAIL (compile error: `body` / `field_to_json` missing, or `trees[&field::BODY]` absent).

- [ ] **Step 3: Implement**

`store/tree.rs`:
```rust
/// The tree fields and their JSON keys (export, import).
pub const TREE_FIELDS: [(u32, &str); 2] = [(field::PROPS, "props"), (field::BODY, "body")];
```
`store/model.rs:301`: delete the `#[allow(dead_code ...)]` line above `pub const BODY`.

`store/body.rs` (stubs that Task 2 completes; keep them correct for scalars/objects now):
```rust
//! The BODY tree: an inline Json card's document (spec 2026-10-24-properties-design.md section 4).

use serde_json::Value;

use super::model::NodeId;
use super::tree::Tree;

pub fn tree_to_value(_tree: &Tree) -> Value {
    Value::Null
}

pub fn value_to_tree(_value: &Value, _mint: &mut dyn FnMut() -> NodeId) -> Result<Tree, String> {
    Ok(Tree::new())
}
```
(Task 2's tests drive the real bodies; the Task 1 test above stays red until Task 2 lands, so Task 1 and Task 2 are ordered one after the other and committed together at the end of Task 2. Run the Task 1 test at Task 2's Step 4.)

`store/tree_json.rs`: make `leaf_json` and the number conversion reusable. Replace the inline number match in `walk` with `number_leaf`:
```rust
pub(super) fn number_leaf(n: &Number) -> Leaf {
    match (n.as_i64(), n.as_f64()) {
        (Some(i), _) => Leaf::Int(i),
        (None, Some(f)) if f.is_finite() && !n.is_u64() => Leaf::Float(f),
        _ => Leaf::Num(n.to_string()),
    }
}
```
and `Value::Number(n) => number_leaf(n),` in `walk`. Make `fn leaf_json` `pub(super)`. Add the dispatch:
```rust
/// `tree` in field `no`'s dialect: PROPS nested objects with `@id` keys; BODY the plain document.
pub fn field_to_json(no: u32, tree: &Tree) -> Value {
    if no == super::model::field::BODY { super::body::tree_to_value(tree) } else { tree_to_json(tree) }
}

/// The inverse; `mint` supplies fresh ids for BODY's array elements.
pub fn field_from_json(
    no: u32,
    value: &Value,
    resolve: &dyn Fn(&str) -> Option<NodeId>,
    mint: &mut dyn FnMut() -> NodeId,
) -> Result<Tree, String> {
    if no == super::model::field::BODY { super::body::value_to_tree(value, mint) } else { tree_from_json(value, resolve) }
}
```
Update the file header comment: "Dialect of PROPS; `store/body.rs` is BODY's" (delete the "plan 4 adds" sentence).

`store/json.rs:89`: `out.insert(name.into(), tree_json::field_to_json(no, tree));` (the filter `resolved(tree).next().is_some()` stays). `:290`: 
```rust
                let tree = field_from_json(no, value, &resolve, &mut next_id)
                    .map_err(|e| format!("node {:?}: bad {name}: {e}", node.id))?;
```
Import `field_from_json`/`field_to_json` from `super::tree_json` and drop `tree_to_json`/`tree_from_json` from json.rs's imports if now unused.

- [ ] **Step 4–5:** run in Task 2 (see above).

---

### Task 2: Value ↔ Tree converter (arrays, root `v`, caps)

**Files:**
- Modify: `crates/manifold/src/store/body.rs` (replace the stubs)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `tree_json::{number_leaf, leaf_json}` (Task 1), `tree::{resolved, Leaf, Seg, Tree, TreePath}`.
- Produces:
  - `pub const INLINE_MAX_LEAVES: usize = 10_000;`
  - `pub fn initial_order(i: usize) -> String`
  - `pub fn tree_to_value(tree: &Tree) -> Value` (`Value::Null` for an empty tree)
  - `pub fn value_to_tree(value: &Value, mint: &mut dyn FnMut() -> NodeId) -> Result<Tree, String>`

Layout (document it in the module header): the document is at `[Key("v")]`. A scalar is a leaf; an object has `Key` children; an empty object / array is `Leaf::EmptyObj` / `Leaf::EmptyArr`; a non-empty array has `Id(e)` children, each with `Key("o")` = `Leaf::Str(order)` and `Key("v")` = the element. A container with any `Id` child reads as an array (its `Key` children are ignored); elements sort by `(o, id)`; an element with no `v` is skipped, one with no `o` sorts first.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u64) -> NodeId {
        NodeId { actor: crate::store::model::ActorId(1), counter: n }
    }

    fn mint_from(start: u64) -> impl FnMut() -> NodeId {
        let mut n = start;
        move || {
            n += 1;
            id(n)
        }
    }

    fn round(v: &Value) -> Value {
        tree_to_value(&value_to_tree(v, &mut mint_from(0)).unwrap())
    }

    #[test]
    fn every_root_shape_round_trips() {
        for v in [
            json!(null), json!(true), json!(3), json!(-2.5), json!("s"), json!([]), json!({}),
            json!([1, [2, [3]], {"o": 1, "v": 2}]),
            json!({"o": "x", "v": [], "$ref": "r", "@1": 2, "a": {"b": {}}}),
            json!(18446744073709551615u64),
        ] {
            assert_eq!(round(&v), v, "{v}");
        }
    }

    #[test]
    fn arrays_keep_their_order_and_get_distinct_ids() {
        let tree = value_to_tree(&json!(["a", "b", "c"]), &mut mint_from(10)).unwrap();
        let ids: std::collections::BTreeSet<_> =
            tree.keys().filter_map(|p| p.segments().into_iter().find_map(|s| match s { Seg::Id(i) => Some(i), _ => None })).collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(tree_to_value(&tree), json!(["a", "b", "c"]));
    }

    #[test]
    fn ties_and_gaps_in_order_are_stable() {
        let v = TreePath::default().child(&Seg::Key("v".into()));
        let el = |n: u64| v.child(&Seg::Id(id(n)));
        let key = |p: &TreePath, k: &str| p.child(&Seg::Key(k.into()));
        let mut tree = Tree::new();
        // 7 and 5 tie on "b"; 9 has no order; 4 has an order but no value.
        for (n, o) in [(7, "b"), (5, "b")] {
            tree.insert(key(&el(n), "o"), Leaf::Str(o.into()));
            tree.insert(key(&el(n), "v"), Leaf::Int(n as i64));
        }
        tree.insert(key(&el(9), "v"), Leaf::Int(9));
        tree.insert(key(&el(4), "o"), Leaf::Str("a".into()));
        assert_eq!(tree_to_value(&tree), json!([9, 5, 7]), "no order first, then order, then id");
    }

    #[test]
    fn a_scalar_with_a_child_shows_the_child() {
        let v = TreePath::default().child(&Seg::Key("v".into()));
        let mut tree = Tree::new();
        tree.insert(v.clone(), Leaf::Int(1));
        tree.insert(v.child(&Seg::Key("k".into())), Leaf::Int(2));
        assert_eq!(tree_to_value(&tree), json!({"k": 2}), "children win (spec 1.2)");
    }

    #[test]
    fn an_assembled_ref_is_plain_object_data() {
        let v = TreePath::default().child(&Seg::Key("v".into()));
        let mut tree = Tree::new();
        tree.insert(v, Leaf::Ref(id(5)));
        assert_eq!(tree_to_value(&tree), json!({"$ref": id(5).to_string()}));
        assert_eq!(round(&json!({"$ref": "5"})), json!({"$ref": "5"}), "and a typed one stays a string, not a Ref");
    }

    #[test]
    fn too_many_leaves_or_too_deep_is_refused() {
        let wide = Value::Array((0..INLINE_MAX_LEAVES).map(Value::from).collect());
        assert!(value_to_tree(&wide, &mut mint_from(0)).unwrap_err().contains("leaves"));
        let mut deep = json!(1);
        for _ in 0..crate::store::tree_json::MAX_DEPTH + 1 {
            deep = json!([deep]);
        }
        assert!(value_to_tree(&deep, &mut mint_from(0)).unwrap_err().contains("deeper"));
    }

    #[test]
    fn a_hostile_deep_tree_assembles_without_overflowing() {
        let mut path = root();
        for n in 0..5_000 {
            path = path.child(&Seg::Key(format!("k{n}")));
        }
        let tree = Tree::from([(path, Leaf::Int(1))]);
        let _ = tree_to_value(&tree); // must return, not overflow
    }

    #[test]
    fn initial_orders_sort_as_numbers() {
        let keys: Vec<String> = [0, 1, 9, 10, 99, 9_999].iter().map(|&i| initial_order(i)).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j 8 -p manifold store::body`
Expected: FAIL (stubs return `Null` / empty).

- [ ] **Step 3: Implement** (replace the stubs; keep the module header describing the layout above)

```rust
use serde_json::{Map, Value};

use super::model::NodeId;
use super::tree::{resolved, Leaf, Seg, Tree, TreePath};
use super::tree_json::{leaf_json, number_leaf, MAX_DEPTH};

pub const INLINE_MAX_LEAVES: usize = 10_000;

/// The order key of element `i` of a fresh array: fixed-width decimal, so byte order is numeric order, with room
/// between neighbours for 4b's `between`.
pub fn initial_order(i: usize) -> String {
    format!("{:08}", (i + 1) * 1000)
}

fn root() -> TreePath {
    TreePath::default().child(&Seg::Key("v".to_owned()))
}

/// The document `tree` holds; `Null` for an empty tree.
pub fn tree_to_value(tree: &Tree) -> Value {
    let entries: Vec<(Vec<Seg>, &Leaf)> = resolved(tree)
        .map(|(p, l)| (p.segments(), l))
        .filter(|(segs, _)| matches!(segs.first(), Some(Seg::Key(k)) if k == "v"))
        .collect();
    if entries.is_empty() { Value::Null } else { build(&entries, 1) }
}

/// `entries` all share their first `depth` segments and are in path order, which groups children together.
fn build(entries: &[(Vec<Seg>, &Leaf)], depth: usize) -> Value {
    // Another replica could send a tree nested past what `value_to_tree` writes; stop before the stack does.
    if depth > 2 * MAX_DEPTH + 2 {
        return Value::Null;
    }
    if let [(segs, leaf)] = entries {
        if segs.len() == depth {
            return leaf_json(leaf);
        }
    }
    let mut groups: Vec<(&Seg, &[(Vec<Seg>, &Leaf)])> = Vec::new();
    let mut start = 0;
    while start < entries.len() {
        let Some(seg) = entries[start].0.get(depth) else {
            start += 1; // a scalar left beside children cannot happen after `resolved`; skip rather than panic
            continue;
        };
        let len = entries[start..].iter().take_while(|(s, _)| s.get(depth) == Some(seg)).count();
        groups.push((seg, &entries[start..start + len]));
        start += len;
    }
    if groups.iter().any(|(seg, _)| matches!(seg, Seg::Id(_))) {
        let mut elements: Vec<(String, NodeId, Value)> = Vec::new();
        for (seg, group) in &groups {
            let Seg::Id(id) = seg else { continue };
            let part = |name: &str| group.iter().position(|(s, _)| matches!(s.get(depth + 1), Some(Seg::Key(k)) if k == name));
            let range = |name: &str| {
                let first = part(name)?;
                let n = group[first..].iter().take_while(|(s, _)| matches!(s.get(depth + 1), Some(Seg::Key(k)) if k == name)).count();
                Some(&group[first..first + n])
            };
            let Some(value) = range("v") else { continue };
            let order = match range("o").and_then(|r| r.first().map(|(_, l)| *l)) {
                Some(Leaf::Str(s)) => s.clone(),
                _ => String::new(),
            };
            elements.push((order, *id, build(value, depth + 2)));
        }
        elements.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        return Value::Array(elements.into_iter().map(|(_, _, v)| v).collect());
    }
    let mut map = Map::new();
    for (seg, group) in groups {
        if let Seg::Key(k) = seg {
            map.insert(k.clone(), build(group, depth + 1));
        }
    }
    Value::Object(map)
}

/// `value` as a BODY tree, minting an id per array element. Refused past `MAX_DEPTH` or `INLINE_MAX_LEAVES`.
pub fn value_to_tree(value: &Value, mint: &mut dyn FnMut() -> NodeId) -> Result<Tree, String> {
    let mut tree = Tree::new();
    put(value, &root(), 1, mint, &mut tree)?;
    Ok(tree)
}

fn put(value: &Value, at: &TreePath, depth: usize, mint: &mut dyn FnMut() -> NodeId, tree: &mut Tree) -> Result<(), String> {
    if depth > MAX_DEPTH + 1 {
        return Err(format!("nested deeper than {MAX_DEPTH}"));
    }
    match value {
        Value::Object(m) if !m.is_empty() => {
            for (k, v) in m {
                put(v, &at.child(&Seg::Key(k.clone())), depth + 1, mint, tree)?;
            }
        }
        Value::Array(a) if !a.is_empty() => {
            for (i, v) in a.iter().enumerate() {
                let el = at.child(&Seg::Id(mint()));
                tree.insert(el.child(&Seg::Key("o".to_owned())), Leaf::Str(initial_order(i)));
                put(v, &el.child(&Seg::Key("v".to_owned())), depth + 1, mint, tree)?;
            }
        }
        scalar => {
            tree.insert(
                at.clone(),
                match scalar {
                    Value::Null => Leaf::Null,
                    Value::Bool(b) => Leaf::Bool(*b),
                    Value::Number(n) => number_leaf(n),
                    Value::String(s) => Leaf::Str(s.clone()),
                    Value::Object(_) => Leaf::EmptyObj,
                    Value::Array(_) => Leaf::EmptyArr,
                },
            );
        }
    }
    if tree.len() >= INLINE_MAX_LEAVES {
        return Err(format!("more than {INLINE_MAX_LEAVES} leaves"));
    }
    Ok(())
}
```
Notes for the implementer: `leaf_json` must render `Leaf::Ref(id)` as `{"$ref": "<id>"}`, `EmptyObj` as `{}`, `EmptyArr` as `[]` (read it; the PROPS exporter already needs the first; fix only if a test above fails). The `>=` in the cap makes a body of exactly `INLINE_MAX_LEAVES` entries refuse; the test builds that many scalars in an array, which is `2 * INLINE_MAX_LEAVES` entries, so it fails either way; `>=` keeps "up to 9,999 entries" a stated limit (README note in Task 10).

- [ ] **Step 4: Run to verify they pass** (this also turns Task 1's test green)

Run: `cargo test -j 8 -p manifold store::body` then `cargo test -j 8 -p manifold a_body_exports_as_plain_json`
Expected: PASS (8 body tests, 1 json test).

- [ ] **Step 5: Whole-suite check and commit**

Run: `cargo fmt --all && cargo test -j 8 -p manifold store::`
Expected: PASS (the `profile::tests` and tree tests unchanged).

```bash
git add crates/manifold/src/store
git commit -m "store: BODY tree field with a plain-JSON dialect and array converter"
```

---

### Task 3: `set_body`, the bulk BODY writer

**Files:**
- Modify: `crates/manifold/src/session_sync/edits.rs` (after `edit_prop`, ~line 30)
- Test: `crates/manifold/src/session_sync/tests/props.rs` (beside the other `edit_prop` tests; use its helpers)

**Interfaces:**
- Consumes: `Mirror::{tree, set_tree}`, `writes.queue_tree(id, field, path, enc)`.
- Produces: `impl App { pub(crate) fn set_body(&mut self, id: NodeId, tree: Tree) -> u64 }` returning the mirror's new `tree_version(id, BODY)`.

Why a bulk writer: `edit_prop` clones the whole tree per call (`Mirror::edit_tree`), so seeding a 10,000-entry body through it is quadratic.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn set_body_replaces_the_tree_and_queues_only_the_difference() {
    let mut app = App::default();
    let id = add(&mut app, Kind::Json, json_inline_placeholder());
    let a = crate::store::body::value_to_tree(&serde_json::json!({"a": 1, "b": 2}), &mut || app_next_id()).unwrap();
    let v1 = app.set_body(id, a);
    let b = crate::store::body::value_to_tree(&serde_json::json!({"a": 1, "c": 3}), &mut || app_next_id()).unwrap();
    let v2 = app.set_body(id, b);
    assert_ne!(v1, v2);
    assert_eq!(app.doc.writes.tree_edits(id, field::BODY).count(), 3, "a, b (tombstone), c: a was queued once, then b/c");
    assert!(app.save_session());
    assert_eq!(
        crate::store::body::tree_to_value(app.doc.mirror.tree(id, field::BODY).unwrap()),
        serde_json::json!({"a": 1, "c": 3})
    );
}
```
(`add`, `json_inline_placeholder`, `app_next_id`: use the existing helpers in `session_sync/tests.rs`; a Json card needs a runtime, use the same one `session_sync/tests/writes.rs:951` builds, changed to `JsonPayload::file(String::new(), Default::default(), None)` after Task 4. Until Task 4 lands the literal `JsonPayload { path, value, error }` still compiles; write the test with that literal and Task 4 converts it with the others. Adapt the `tree_edits(...).count()` call to the real return type: it is an iterator of `(&TreePath, &[u8])` per `remote.rs:567`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j 8 -p manifold set_body_replaces`
Expected: FAIL (no method `set_body`).

- [ ] **Step 3: Implement**

```rust
    /// Replaces `id`'s BODY tree with `tree`: the mirror shows it at once and the next save commits the difference
    /// (a `Set` per changed leaf, a tombstone per vanished one). The bulk twin of `edit_prop`, which clones the whole
    /// tree per leaf. Returns the new `Mirror::tree_version`.
    pub(crate) fn set_body(&mut self, id: NodeId, tree: Tree) -> u64 {
        debug_assert!(self.doc.mirror.has(id), "a node the mirror knows");
        let old = self.doc.mirror.tree(id, field::BODY).cloned().unwrap_or_default();
        for path in old.keys().filter(|p| !tree.contains_key(*p)) {
            self.doc.writes.queue_tree(id, field::BODY, path.clone(), Vec::new());
        }
        for (path, leaf) in tree.iter().filter(|(p, l)| old.get(*p) != Some(*l)) {
            self.doc.writes.queue_tree(id, field::BODY, path.clone(), leaf.encode());
        }
        self.doc.mirror.set_tree(id, field::BODY, tree);
        self.doc.mirror.tree_version(id, field::BODY)
    }
```
Remove `#[allow(dead_code, reason = "the render cache key of plans 3 and 4")]` from `Mirror::tree_version` (`mirror.rs:111`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -j 8 -p manifold set_body_replaces`
Expected: PASS. Also `cargo test -j 8 -p manifold session_sync::tests::props`; expected PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/manifold/src/session_sync crates/manifold/src/mirror.rs
git commit -m "session_sync: set_body, the bulk writer for a BODY tree"
```

---

### Task 4: `JsonSource` / `JsonPayload` constructors, accessor, mechanical call-site move

**Files:**
- Modify: `crates/manifold/src/node.rs:~242` (payload), `:105` and `:435` (readers), `enterable`
- Modify (literal constructions → `JsonPayload::file(path, value, error)`): `bench.rs:119`, `card/tests.rs:639,700`, `card_kind.rs:168`, `fetched.rs:379,628`, `json_level/tests.rs:394`, `node.rs:530`, `session_files.rs:207`, `session_sync/remote.rs:251`, `session_sync/tests/writes.rs:951`
- Modify (readers → `.path()`): `card/json_body.rs:27,57,60`, `fetched.rs:301,431,434`, `graph_nav.rs:389`, `json_level_ui.rs:517,864`, `node.rs:105,435`, `reveal.rs:49`, `fetched/tests.rs:201`, `session_sync/tests/remote.rs:89,96`
- Test: `node.rs` `mod tests`

**Interfaces:**
- Produces:
```rust
pub enum JsonSource { File { path: String }, Inline { version: u64 } }
pub struct JsonPayload { pub source: JsonSource, pub value: Arc<Value>, pub error: Option<String> }
impl JsonPayload {
    pub fn file(path: String, value: Arc<Value>, error: Option<String>) -> Self;
    pub fn inline(version: u64, value: Arc<Value>) -> Self;
    pub fn path(&self) -> &str;          // "" for inline
    pub fn is_inline(&self) -> bool;
}
```
`Runtime::file_path()` returns `None` for an inline Json (so `place_node` seeds PATH `""`), and `Some(path)` otherwise. `enterable()`: inline → `error.is_none()`; file → unchanged.

This task is a pure refactor plus the two new behaviours below; the compiler finds every site.

- [ ] **Step 1: Write the failing test** (`node.rs` `mod tests`)

```rust
    #[test]
    fn an_inline_json_has_no_path_and_is_enterable_even_when_null() {
        let p = JsonPayload::inline(3, Arc::new(Value::Null));
        assert_eq!(p.path(), "");
        assert!(p.is_inline() && p.enterable());
        assert!(Runtime::Json(p).file_path().is_none());
        let missing = JsonPayload::file("no/such/file.json".into(), Arc::new(Value::Null), None);
        assert!(!missing.enterable(), "a file card whose file is gone still isn't");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j 8 -p manifold an_inline_json_has_no_path`
Expected: FAIL (no `inline`).

- [ ] **Step 3: Implement.** Edit `node.rs` payload and impl as in Interfaces. Then `cargo check -p manifold --all-targets`, and fix every error mechanically: literal `JsonPayload { path, value, error }` → `JsonPayload::file(path, value, error)` (keep `value: x.into()` as `x.into()`); reads `p.path` → `p.path()`. `fetched.rs:434`'s destructure `Runtime::Json(JsonPayload { path, .. })` becomes `Runtime::Json(p)` using `p.path()`.

- [ ] **Step 4: Run to verify it passes, then the full unit suite**

Run: `cargo test -j 8 -p manifold` 
Expected: PASS everywhere (no behaviour change for file cards).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/manifold/src
git commit -m "node: JsonSource and JsonPayload constructors; file/inline accessors"
```

---

### Task 5: Inline materialize, remote BODY change, level value swap

**Files:**
- Modify: `crates/manifold/src/session_sync/remote.rs` (`insert_materialized` Json arm ~243; `update_live_node` ~120-150)
- Modify: `crates/manifold/src/json_level.rs` (add `JsonLevel::swap_value`, ~after `relayout` at 132)
- Test: `crates/manifold/src/session_sync/tests/remote.rs`, `crates/manifold/src/json_level/tests.rs`

**Interfaces:**
- Consumes: `body::tree_to_value`, `Mirror::{tree, tree_version}`, `JsonPayload::{inline, is_inline}`.
- Produces:
  - `App::inline_payload(&self, id: NodeId) -> JsonPayload`: the payload for `id`'s current BODY tree (`Arc` of `tree_to_value`, version from the mirror).
  - `JsonLevel::swap_value(&mut self, value: Arc<Value>)`: replaces `value`, recomputes `nodes` (`count_nodes`), keeps `selected` / `expanded` / `outline` when their pointers still resolve (drop those that don't), cancels a running worker search (bump the generation) and re-runs the current query, then relays out.

Rule: a Json node is inline iff the mirror holds a non-empty BODY tree for it (set by New JSON / Detach, Task 7; or arriving from another replica).

- [ ] **Step 1: Write the failing tests**

`session_sync/tests/remote.rs`:
```rust
#[test]
fn a_remote_inline_json_card_materializes_with_its_value() {
    // Another replica creates a Json card with a BODY and no file; this one shows it, no placeholder, no refetch.
    let (mut app, mut other) = two_replicas();
    let id = remote_create_inline_json(&mut other, serde_json::json!({"k": [1, 2]}));
    assert!(app.save_session());
    let Runtime::Json(p) = &app.nodes[&id].runtime else { panic!("a json card") };
    assert!(p.is_inline() && p.error.is_none());
    assert_eq!(*p.value, serde_json::json!({"k": [1, 2]}));
}

#[test]
fn a_remote_body_change_updates_the_open_cards_value() {
    let (mut app, mut other) = two_replicas();
    let id = remote_create_inline_json(&mut other, serde_json::json!({"k": 1}));
    assert!(app.save_session());
    remote_set_body(&mut other, id, serde_json::json!({"k": 2}));
    assert!(app.save_session());
    let Runtime::Json(p) = &app.nodes[&id].runtime else { panic!() };
    assert_eq!(*p.value, serde_json::json!({"k": 2}));
}
```
(`two_replicas`, `remote_create_inline_json`, `remote_set_body`: add to `session_sync/tests.rs` beside `remote_set`, built from `remote_set`'s mechanism: `remote_set_body` encodes `value_to_tree` leaves as `OpBody::Set` the way `tree_sets` in `store/json.rs` does, `remote_create_inline_json` is `OpBody::create(id, "application/json", fields)` plus those sets. Copy `remote_set`'s transport lines verbatim.)

`json_level/tests.rs` (helper `app_with_json` exists there):
```rust
#[test]
fn a_remote_body_change_swaps_the_open_levels_value() {
    let mut app = app_with_json(serde_json::json!({"a": {"b": 1}, "c": [1, 2, 3]}));
    let id = app.json_level.as_ref().unwrap().node_id;
    app.json_level.as_mut().unwrap().selected = Some("/a/b".to_owned());
    app.json_level.as_mut().unwrap().expanded.insert("/a".to_owned());
    app.json_search("1"); // the headless JSONSEARCH path; adapt to the helper this file uses for a search
    let gen_before = app.json_level.as_ref().unwrap().search_generation();
    app.json_level.as_mut().unwrap().swap_value(Arc::new(serde_json::json!({"a": {"b": 5}, "c": [1]})));
    let level = app.json_level.as_ref().unwrap();
    assert_eq!(level.node_id, id);
    assert_eq!(level.selected.as_deref(), Some("/a/b"), "still resolves, so kept");
    assert!(level.expanded.contains("/a"));
    assert!(level.search_generation() > gen_before, "a running search is cancelled and restarted");
    app.json_level.as_mut().unwrap().swap_value(Arc::new(serde_json::json!({"x": 1})));
    assert_eq!(app.json_level.as_ref().unwrap().selected, None, "/a/b no longer exists");
}
```
(`search_generation()` is a tiny test-visible accessor over the existing `Arc<AtomicU64>`; if the field is already readable from `tests.rs`, read it directly and skip the accessor. Use the actual field/helper names after reading `json_level.rs:15-60` and `json_level/tests.rs`: `selected` is the pointer string, `expanded` a set of pointers.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j 8 -p manifold a_remote_inline_json a_remote_body_change`
Expected: FAIL (placeholder error / no `swap_value`).

- [ ] **Step 3: Implement**

`App::inline_payload` (in `session_sync/remote.rs` near `insert_materialized`):
```rust
    pub(crate) fn inline_payload(&self, id: NodeId) -> JsonPayload {
        let tree = self.doc.mirror.tree(id, field::BODY);
        let value = tree.map_or(serde_json::Value::Null, crate::store::body::tree_to_value);
        JsonPayload::inline(self.doc.mirror.tree_version(id, field::BODY), Arc::new(value))
    }
```
`insert_materialized`, at the top of the `Kind::Json` arm (before `content_path` use):
```rust
            Kind::Json if self.doc.mirror.tree(id, field::BODY).is_some() => {
                let payload = self.inline_payload(id);
                self.insert_placeholder_node(id, pos, kind, Runtime::Json(payload));
            }
```
Also make the earlier `content_path` computation skip inline: change the `matches!(kind, Kind::Image | Kind::Pdf | Kind::Json)` guard to `(matches!(kind, Kind::Image | Kind::Pdf) || kind == Kind::Json && self.doc.mirror.tree(id, field::BODY).is_none())`. (`insert_placeholder_node` is the existing helper the other arms use for building the node without a fetch; name verified at `remote.rs:244`.)

`update_live_node`: replace `| Runtime::Json(_)` in the no-op arm with its own arm, before it:
```rust
            Runtime::Json(p) => {
                if changed(field::BODY) && self.doc.mirror.tree(id, field::BODY).is_some() {
                    let fresh = self.inline_payload(id);
                    let value = fresh.value.clone();
                    *p = fresh;
                    if let Some(level) = self.json_level.as_mut().filter(|l| l.node_id == id) {
                        level.swap_value(value);
                    }
                }
            }
```
(`p` borrows `self.nodes`; if the borrow checker objects to `self.inline_payload(id)` inside the arm, compute `let fresh = (changed(field::BODY)).then(|| self.inline_payload(id));` before the `match` and use it in the arm.)

`JsonLevel::swap_value` in `json_level.rs`:
```rust
    /// A new document for the level being shown (another replica changed it): selection, expansion and outline
    /// stay where their pointers still resolve, a search restarts on the new value, the cards are laid out again.
    pub(crate) fn swap_value(&mut self, value: Arc<Value>) {
        self.value = value;
        self.nodes = count_nodes(&self.value);
        self.selected.retain(|p| self.value.pointer(p).is_some());
        self.expanded.retain(|p| p.is_empty() || self.value.pointer(p).is_some());
        self.cancel_and_rerun_search();
        self.relayout();
    }
```
Adapt names/types to `JsonLevel`'s real fields (`selected` may be `Option<String>`: use `if self.selected.as_ref().is_some_and(|p| self.value.pointer(p).is_none()) { self.selected = None; }`). `cancel_and_rerun_search`: bump the existing generation counter and call whatever `JSONSEARCH`'s handler calls with the current query (read `json_level.rs` search code; if the handler needs `&mut App`, do the rerun in the `update_live_node` arm instead and keep only the bump in `swap_value`). `count_nodes` is `json_view.rs:732`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -j 8 -p manifold a_remote_inline_json a_remote_body_change`
Expected: PASS (3 tests). Then `cargo test -j 8 -p manifold session_sync json_level`; expected PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/manifold/src
git commit -m "session_sync: inline Json cards materialize from BODY and follow remote changes"
```

---

### Task 6: Inline cards do no file work (path-reader fixes)

**Files:**
- Modify: `reveal.rs:49,68` (Open Containing Folder: not offered for inline), `graph_nav.rs:389` and `json_level_ui.rs:517,864` (crumb root falls back to the card name), `card.rs:878,902` (`preview_size`, `card_filter_text` use the card name), `node.rs:435` (tooltip), `node_menu.rs:467,540` (file items), `caps/rows.rs:298`, `fetched.rs` (`retry_missing_file`, `refresh_node`, `reload_fetched_node` skip inline), `session_sync/remote.rs` (`missing_with_url` already skipped in Task 5)
- Test: `crates/manifold/src/fetched/tests.rs`, `crates/manifold/src/card/tests.rs`, `crates/manifold/src/node_menu/tests.rs`

**Interfaces:** consumes `JsonPayload::{path, is_inline}`; produces `Runtime::json_is_inline(&self) -> bool` (one helper so each site is a one-liner).

- [ ] **Step 1: Write the failing test** (`fetched/tests.rs`, using the same helpers as the neighbouring Json tests)

```rust
#[test]
fn inline_cards_do_no_file_work() {
    let mut app = App::default();
    let id = app.alloc_node_id();
    app.place_node(Some(id), Kind::Json, Runtime::Json(JsonPayload::inline(1, Arc::new(serde_json::json!({})))), placement());
    assert!(app.nodes[&id].runtime.file_path().is_none());
    app.retry_missing_file(id); // must not touch the disk or toast
    app.reload_fetched_node(id);
    assert!(app.take_toasts().is_empty());
    assert!(matches!(&app.nodes[&id].runtime, Runtime::Json(p) if p.is_inline()), "still inline, not an error card");
    assert!(app.reveal_target(id).is_none(), "nothing to open in a folder");
    assert_eq!(app.card_filter_text(id), app.doc.mirror.name(id), "searchable by its name");
}
```
(Replace `take_toasts`, `reveal_target`, `card_filter_text` with the real function names found at `reveal.rs:49`, `card.rs:902`, and the toast queue accessor the neighbouring tests use. If `reveal.rs` has no separate predicate, extract one: `fn can_reveal(&self, id) -> bool` and test that.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j 8 -p manifold inline_cards_do_no_file_work`
Expected: FAIL (empty path read/ "Couldn't open ''" toast, or reveal offered).

- [ ] **Step 3: Implement** each site as a one-line branch on `json_is_inline()` / `p.is_inline()`:
  - `reveal.rs`: return early for inline.
  - `graph_nav.rs:389`, `json_level_ui.rs:517,864`: `let root = if p.is_inline() { card_name } else { file_name(p.path()) };` (reuse the existing `file_name`-style expression; the fallback to the card name already exists when the path has no file name, so an empty path from `path()` may already work: check by running the crumb test before editing, edit only if it fails).
  - `card.rs:878,902`, `node.rs:435`: the tooltip shows `"Inline JSON"`; `preview_size` and `card_filter_text` use the name.
  - `node_menu.rs:467,540` and `caps/rows.rs:298`: the Json row stays `file(true)` for file cards; give the inline card the same capabilities but no file items: in `node_menu.rs` the Json branch calls `file_items` only when `!runtime.json_is_inline()`; keep `url_items` off for inline. Add Task 7's two items here as well when Task 7 lands (do not add them in this task).
  - `fetched.rs`: guard the three file functions with `if self.nodes.get(&id).is_some_and(|n| n.runtime.json_is_inline()) { return; }`.
  - `node.rs`: `impl Runtime { pub fn json_is_inline(&self) -> bool { matches!(self, Runtime::Json(p) if p.is_inline()) } }`.

- [ ] **Step 4: Run to verify it passes and nothing else moved**

Run: `cargo test -j 8 -p manifold`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/manifold/src
git commit -m "json: inline cards skip file paths, reveal, refetch and file menu items"
```

---

### Task 7: New JSON and Detach to inline

**Files:**
- Modify: `crates/manifold/src/graph_ui.rs` (`GraphAction::NewJson(egui::Vec2)` ~113; handler beside `NewNote` ~210)
- Modify: `crates/manifold/src/graph_ui/cards.rs:215-250` (menu item "New JSON")
- Modify: `crates/manifold/src/fetched.rs` (add `insert_inline_json_at`, `detach_json`)
- Modify: `crates/manifold/src/node_menu.rs` + `NodeAction::DetachJson` (menu item "Detach to inline" for a file Json card)
- Modify: `crates/manifold/src/debug_control.rs` (verbs `NEWJSON [x y]`, `DETACHJSON <id>`)
- Test: `crates/manifold/src/fetched/tests.rs`

**Interfaces:**
- Consumes: `body::{value_to_tree, INLINE_MAX_LEAVES}`, `App::set_body`, `place_node`, `inline_payload` (Task 5).
- Produces:
  - `App::insert_inline_json_at(&mut self, id: NodeId, pos: Option<egui::Vec2>)`: a card holding `{}`, selected for entry like `insert_note_at` does (copy its position/jitter and focus handling).
  - `App::detach_json(&mut self, id: NodeId) -> Result<(), String>`: converts a file card to inline: builds the tree from the payload's value, `set_body`, `edit_field(id, field::PATH, Str(""))`, replaces the runtime payload with `inline_payload(id)`. On `value_to_tree` error: toast the message, change nothing. Not undoable in 4a (listed in PARKED, Task 10).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn new_json_makes_an_empty_object_that_survives_a_save_and_reload() {
    let mut app = App::default();
    let id = app.alloc_node_id();
    app.insert_inline_json_at(id, None);
    assert!(app.save_session());
    let Runtime::Json(p) = &app.nodes[&id].runtime else { panic!() };
    assert!(p.is_inline());
    assert_eq!(*p.value, serde_json::json!({}));
    let reloaded = reload_from_document(&app); // the helper session_sync/tests uses to restore a saved document
    let Runtime::Json(q) = &reloaded.nodes[&id].runtime else { panic!() };
    assert!(q.is_inline() && *q.value == serde_json::json!({}));
}

#[test]
fn detach_moves_a_file_cards_value_into_the_document() {
    let dir = crate::store::testutil::temp_dir("detach");
    let file = dir.join("d.json");
    std::fs::write(&file, r#"{"k": [1, {"x": null}]}"#).unwrap();
    let mut app = App::default();
    let id = app.alloc_node_id();
    app.insert_json_node_with_id(id, file.to_string_lossy().into_owned(), egui::vec2(0.0, 0.0), None).unwrap();
    app.detach_json(id).unwrap();
    let Runtime::Json(p) = &app.nodes[&id].runtime else { panic!() };
    assert!(p.is_inline());
    assert_eq!(*p.value, serde_json::json!({"k": [1, {"x": null}]}));
    assert_eq!(app.doc.mirror.get(id, field::PATH), FieldValue::Str(String::new()));
    std::fs::remove_file(&file).unwrap(); // the file is no longer needed
    assert!(app.save_session());
    assert!(matches!(&reload_from_document(&app).nodes[&id].runtime, Runtime::Json(q) if q.is_inline()));
}

#[test]
fn detach_over_the_cap_changes_nothing() {
    let dir = crate::store::testutil::temp_dir("detach-big");
    let file = dir.join("big.json");
    let big: Vec<u32> = (0..20_000).collect();
    std::fs::write(&file, serde_json::to_string(&big).unwrap()).unwrap();
    let mut app = App::default();
    let id = app.alloc_node_id();
    app.insert_json_node_with_id(id, file.to_string_lossy().into_owned(), egui::vec2(0.0, 0.0), None).unwrap();
    let err = app.detach_json(id).unwrap_err();
    assert!(err.contains("leaves"), "{err}");
    assert!(matches!(&app.nodes[&id].runtime, Runtime::Json(p) if !p.is_inline()));
    assert!(app.doc.mirror.tree(id, field::BODY).is_none());
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j 8 -p manifold new_json_makes detach_`
Expected: FAIL (no methods).

- [ ] **Step 3: Implement**

`fetched.rs`:
```rust
    /// A new inline Json card holding `{}` (`GraphAction::NewJson`, headless `NEWJSON`).
    pub(crate) fn insert_inline_json_at(&mut self, id: NodeId, pos: Option<egui::Vec2>) {
        let pos = pos.unwrap_or_else(|| self.jittered_world_center()); // the same default insert_note_at uses
        let placeholder = JsonPayload::inline(0, Default::default());
        self.place_node(Some(id), Kind::Json, Runtime::Json(placeholder), Placement { group: self.current_group, pos });
        let tree = crate::store::body::value_to_tree(&serde_json::json!({}), &mut || self.alloc_node_id())
            .expect("an empty object is within the caps");
        self.set_body(id, tree);
        let fresh = self.inline_payload(id);
        if let Some(node) = self.nodes.get_mut(&id) {
            node.runtime = Runtime::Json(fresh);
        }
    }

    /// Moves a file card's document into the synced document (`BODY`) and drops the path. The file stays on disk.
    pub(crate) fn detach_json(&mut self, id: NodeId) -> Result<(), String> {
        let Some(Runtime::Json(p)) = self.nodes.get(&id).map(|n| &n.runtime) else { return Err("not a JSON card".into()) };
        if p.is_inline() || p.error.is_some() {
            return Err("nothing to detach".into());
        }
        let value = p.value.clone();
        let tree = crate::store::body::value_to_tree(&value, &mut || self.alloc_node_id()).inspect_err(|e| {
            self.queue_toast(format!("Can't detach: {e}"));
        })?;
        self.set_body(id, tree);
        self.edit_field(id, field::PATH, FieldValue::Str(String::new()));
        let fresh = self.inline_payload(id);
        if let Some(node) = self.nodes.get_mut(&id) {
            node.runtime = Runtime::Json(fresh);
        }
        Ok(())
    }
```
(`jittered_world_center` stands for whatever `insert_note_at` calls when `pos` is `None`: read `node_ops.rs:29-60` and copy it; `self.current_group`: the field `GroupSelected` uses in `graph_ui.rs`, pass `None` if notes do. The closure borrowing `self` while `self.set_body` is needed: allocate ids into a `Vec` first with `let mut ids = Vec::new(); value_to_tree(.., &mut || { let i = self.alloc_node_id(); ids.push(i); i })`, or mint from a local counter seeded by `alloc_node_id`. Pick whichever the compiler accepts; the semantics are one fresh id per element.)

`graph_ui.rs`: add variant `NewJson(egui::Vec2)` and
```rust
            GraphAction::NewJson(pos) => {
                let id = self.alloc_node_id();
                self.insert_inline_json_at(id, Some(pos));
            }
```
`graph_ui/cards.rs`: add `json` to the bool tuple, a `ui.button(format!("{} New JSON", egui_phosphor::light::BRACKETS_CURLY))` after New Group, `else if json { actions.push(GraphAction::NewJson(pos)); }`, and `|| json` in the dismiss condition. `node_menu.rs`: `NodeAction::DetachJson` + a `NodeMenuItem` "Detach to inline" for a Json runtime that is not inline, dispatched to `detach_json` (errors already toasted). `debug_control.rs` verbs next to `ADDFILE`:
```rust
            "NEWJSON" => { /* parse optional x y exactly as ADDFILE does; call insert_inline_json_at */ }
            "DETACHJSON" => { /* parse a node id as JSONSELECT does; call detach_json; reply ok/err text */ }
```
(Write them as complete arms following the arm for `ADDFILE` in `debug_control.rs:~693`; they are three or four lines each. The gate stage in Task 9 drives them.)

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -j 8 -p manifold new_json_makes detach_`
Expected: PASS (3 tests). Then `cargo test -j 8 -p manifold node_menu graph_ui`; expected PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/manifold/src
git commit -m "json: New JSON and Detach to inline"
```

---

### Task 8: Duplicate, export and import of an inline card

**Files:**
- Test: `crates/manifold/src/duplicate/tests.rs` (or wherever duplicate's tests live; `grep -rn "fn duplicate" crates/manifold/src` to find them) and `crates/manifold/src/store/json.rs` `mod tests`
- Modify only if a test fails: `crates/manifold/src/duplicate.rs`

**Interfaces:** consumes everything above; produces no new API. This task is the proof that BODY rides the existing generic paths (the survey says `duplicate.rs` carries trees via `node_json(.., &root.trees, ..)` and `store/json.rs` export iterates `TREE_FIELDS`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn duplicating_an_inline_json_card_copies_the_body_with_fresh_element_ids() {
    let mut app = App::default();
    let a = app.alloc_node_id();
    app.insert_inline_json_at(a, None);
    let tree = body::value_to_tree(&json!({"list": [1, 2]}), &mut || app.alloc_node_id()).unwrap();
    app.set_body(a, tree);
    let b = app.duplicate_node(a).expect("a copy"); // use the real entry point duplicate/tests.rs uses
    let (ta, tb) = (app.doc.mirror.tree(a, field::BODY).unwrap(), app.doc.mirror.tree(b, field::BODY).unwrap());
    assert_eq!(body::tree_to_value(ta), body::tree_to_value(tb));
    let ids = |t: &Tree| t.keys().flat_map(|p| p.segments()).filter_map(|s| if let Seg::Id(i) = s { Some(i) } else { None }).collect::<std::collections::BTreeSet<_>>();
    assert!(ids(ta).is_disjoint(&ids(tb)), "elements of the copy are new ids");
}
```
and in `store/json.rs`:
```rust
    #[test]
    fn an_inline_card_exports_without_a_path_and_round_trips() {
        let json = r##"{"nodes": [{"id": "a", "type": "application/json", "body": {"k": [1, 2]}}]}"##;
        let doc: JsonDoc = serde_json::from_str(json).unwrap();
        let imported = import(&doc, ids(7), Trust::Foreign).unwrap();
        let out = node_json("a", "application/json", &imported[0].fields, &imported[0].trees, None);
        assert_eq!(out.fields["body"], serde_json::json!({"k": [1, 2]}));
        assert!(!out.fields.contains_key("path"), "no path field for an inline card");
    }
```

- [ ] **Step 2: Run to verify they fail or pass**

Run: `cargo test -j 8 -p manifold duplicating_an_inline an_inline_card_exports`
Expected: the duplicate test may FAIL if element ids are copied verbatim (ids are per-node maps, so equal ids across different nodes are legal: if the copy shares element ids with the original, that is harmless for sync (different `(node, field, path)` keys) and the disjointness assertion is too strict: decide by reading `duplicate.rs`, and if copies share ids, change the assertion to `ta == tb` and note it in the ledger as a ruling). The export test should already PASS (Task 1).

- [ ] **Step 3–4:** adjust the assertion or `duplicate.rs` per the ruling; rerun.

Run: `cargo test -j 8 -p manifold duplicating_an_inline an_inline_card_exports`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/manifold/src
git commit -m "tests: inline Json cards duplicate and export through the generic paths"
```

---

### Task 9: Gate stage

**Files:**
- Modify: `crates/gate/src/smoke.rs:803-860` (the "json-level" mark; add an "inline-json" mark after it)
- Test: the gate itself (this is an integration stage)

**Interfaces:** consumes the headless verbs `NEWJSON`, `JSONSELECT`, `JSONTOGGLE`, `DETACHJSON`, `ADDFILE` and the debug export of the open level. Produces a smoke mark `inline-json`.

Steps the mark performs (mirror the style of the json-level mark right above it; read lines 803-860 first and copy its helpers for sending a verb and reading the exported state):
1. `NEWJSON 100 100` → expect a card with type `application/json` whose exported level shows an empty object.
2. `ADDFILE <fixture json>` then `DETACHJSON <id>` → expect the card's level to show the file's top-level keys and the card's `path` export to be empty.
3. Save, close, reopen the session (the existing session round-trip helper in smoke.rs) → expect both inline cards to open the same levels with no "Couldn't open" toast.

- [ ] **Step 1: Add the mark** following the existing mark's structure, with the three steps above and assertions on the debug export (`debug_export.rs` already exports the level; if it lacks the value's top-level keys, add them: `"json_keys"` next to the level's existing fields, a two-line change, and assert on it).
- [ ] **Step 2: Build and run it:** `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`, then launch the `light` tier through `~/.claude/runs/run.ps1` as CLAUDE.md "Gate" describes (`-Kind gate`, `target\debug\gate.exe light "%RUN_DIR%\gate-log.txt"`), detached, BelowNormal. Expected: all marks pass including `inline-json`; record CPU load and Defender state with the result.
- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add crates/gate crates/manifold/src/debug_export.rs docs/runs.md
git commit -m "gate: inline-json smoke mark"
```
(The full `all ... full` gate belongs to the pre-tag step and is the user's call.)

---

### Task 10: Docs

**Files:**
- Modify: `README.md` (Properties/Json cards section and the headless command list: add `NEWJSON`, `DETACHJSON`; the cap and "arrays are id-keyed maps" in one sentence each)
- Modify: `docs/superpowers/specs/2026-10-24-properties-design.md` (section 4: mark 4a done, link this plan; 4b open)
- Modify: `PARKED.md`: add **Detach to inline is not undoable** (what; why parked: undo for a runtime swap plus PATH plus BODY is a three-part step, 4b's undo work covers it; trigger: user detaches by mistake; status `parked`), keep the existing "Inline Json view overrides keyed by element id" entry and extend its evidence line with this plan's `retain_resolvable` decision
- Modify: `ROADMAP.md` if it lists plan 4: split into 4a (done) and 4b
- Modify: this plan's front matter to `status: done` once Tasks 1-9 land (leftovers are in PARKED)

- [ ] **Step 1:** grep for stale wording: `grep -rn "plan 4\|BODY\|dead_code.*BODY\|inline" README.md ROADMAP.md PARKED.md docs/superpowers/specs/2026-10-24-properties-design.md` and fix each hit that says inline Json is unbuilt.
- [ ] **Step 2: Commit**

```bash
git add README.md PARKED.md ROADMAP.md docs
git commit -m "docs: inline Json cards (4a) shipped; 4b and detach undo parked"
```

---

## Self-review

- **Spec coverage:** section 4 inline Json read side (Tasks 2, 4, 5, 6, 7), 2.1 mirror (3, 5), 2.3 export/import (1, 8), cap (2, 7), gate (9), docs (10). Write path, Ref rows, pointer remap are 4b by decision.
- **Placeholders:** the adaptation notes in Tasks 3, 5, 6, 7 name the exact existing helper to copy or the symbol to look up when this plan could not read its exact signature; each has the semantics spelled out. No "TBD".
- **Type consistency:** `JsonPayload::{file, inline, path, is_inline}`, `Runtime::json_is_inline`, `App::{set_body, inline_payload, insert_inline_json_at, detach_json}`, `JsonLevel::swap_value`, `body::{tree_to_value, value_to_tree, initial_order, INLINE_MAX_LEAVES}`, `field_to_json` / `field_from_json` are used with one signature throughout.
- **Review Focus:** all five have a named pinning test (Tasks 2, 5, 6, 7).

## Adversarial rounds

Two rounds, each against the code, not memory.

1. **Round 1** caught: `NodeId` is `{actor, counter}`, not a tuple (every test in Tasks 1, 2, 8 would not compile; now the `ids(actor)` helper and a local `id(n)`); `tree_to_value` recursed without a bound, so a hostile tree from another replica could overflow the stack (added a depth guard and `a_hostile_deep_tree_assembles_without_overflowing`); Task 1's test cannot pass until Task 2 (stated, and the two commit together); `place_node` seeds PATH from `file_path()`, so inline needs `file_path() == None` (Task 4); `MAX_DEPTH` counts nesting but arrays cost two path segments (the guard uses `2 * MAX_DEPTH + 2`).
2. **Round 2** verified: `ActorId` path and `TYPE_JSON == "application/json"` exist; `leaf_json` already renders Ref / EmptyObj / EmptyArr as the plan needs; `queue_tree` with empty bytes is a tombstone and hides the subtree; `materialize` runs `mirror_remote` first, so the BODY tree is in the mirror when `insert_materialized` runs. Left as stated adaptation notes (the real helper must be read at execution): the `two_replicas` / `remote_create_inline_json` test helpers (copy `remote_set`), the `JsonLevel` search-restart call, the borrow-checker shape of the `update_live_node` arm, and the gate mark's helpers.

Architecture check: nothing grew a special case. One kind of branch was added, `JsonSource` (File vs Inline), read through `path()` / `is_inline()` at about ten sites; what would remove it is 4b keying the whole Json card on its source rather than its path. The `BODY` dialect dispatches on field number in `field_to_json` / `field_from_json` (two fields, two dialects); a third tree field with its own dialect should turn that into a table on `TREE_FIELDS`.
