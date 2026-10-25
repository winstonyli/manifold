---
status: draft
---

# Properties 4b: editing inline Json cards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An inline Json card can be edited on the JSON level (scalars, keys, array elements, types), each edit one undo step; plus Export, `Ref` chips, `PROPS` text in the global index, and the parked items that needed order keys.

**Architecture:** One op enum (`JsonOp`) and one writer (`App::edit_json`) resolve a JSON pointer against the BODY tree (`body::pointer_path`), write through `edit_prop`, record one `UndoKind::Props`, and call one refresh hook (`refresh_inline`) that local edits, undo, detach and remote changes all share. Array order is `body::between`. Every UI gesture and the headless `JSONEDIT` verb build a `JsonOp`.

**Tech Stack:** Rust, egui; `crates/manifold` (store/body.rs, session_sync, json_level*.rs, undo.rs), `crates/gate` (smoke).

**Spec:** `docs/superpowers/specs/2026-10-24-properties-design.md` section 4 "Editing", 2.2 "Undo", 1.3 "Shape rules". Builds on `docs/superpowers/plans/2026-10-25-properties-4a-inline-json.md` (done). Closes these `PARKED.md` entries: "Inline Json view overrides keyed by element id", "Detach to inline is not undoable", "Concurrent detach of one file duplicates array elements", "A whole-BODY tombstone leaves a stale or broken inline card".

## Decisions (settled by the user's go-ahead on this plan; D1 to D9 are open until then)

Each is stated with a recommendation; the plan below is written for the recommended option.

- **D1 Write path.** *Recommended:* one `JsonOp` enum + `App::edit_json(op)`; scalar edit, retype, add key, rename, delete, insert, move are all ops (7 gestures, one writer). *Alt:* one writer per gesture (more code, 7 undo recorders).
- **D2 Array order keys.** *Recommended:* `body::between(a, b)` returns `Option<String>`: 8-digit decimal steps of 1000 while they fit (so appends and prepends never grow keys), midpoint digits otherwise, `None` when no key fits (equal or all-zero neighbours), and the caller then renumbers the array with `initial_order` in the same undo step. *Alt:* always-midpoint (an append-heavy array grows keys by a char per ~3 appends).
- **D3 What makes a card inline.** *Recommended:* BODY present, or (Json, PATH empty, URL empty). Edits never leave BODY empty (deleting the root writes `null`), and a remote whole-BODY tombstone on a live inline card shows `null` and stays inline. Closes the parked tombstone entry. *Alt:* PATH empty alone (wrong: a pending-fetch URL card also has an empty PATH).
- **D4 Concurrent detach.** *Recommended:* deterministic element ids, `NodeId { actor: ActorId(0), counter: fnv1a64(pointer) }`: two replicas detaching one file write identical registers, so nothing doubles; `value_to_tree`'s mint closure takes the element's pointer. *Alt:* a detach claim register (another field and a sync protocol for one race).
- **D5 View overrides.** *Recommended:* structural edits remap override keys, selection and outline rows (`remap_pointer`), so a local insert, delete, move or rename keeps card positions; a remote structural change still prunes (`retain_resolvable`). Element-id keying stays `decided against`. *Alt:* prune only (cheapest; every reorder loses positions).
- **D6 Gestures.** *Recommended:* double-click a value or key edits it in place (Enter or blur commits, Esc cancels); the right-click menu carries Add key, Add element, Insert above/below, Move up/down, Delete, Type; `F2` edits and `Delete` deletes the selection. No drag-reorder in 4b. *Alt:* drag-reorder rows (parked below).
- **D7 Refs.** *Recommended:* the level shows a `Ref` leaf as a chip (the target's title, or "missing"), a click reveals the target; the level's `Value` carries the id text and a side map `pointer -> NodeId` marks the chips. 4b adds no UI that creates a Ref (parked). *Alt:* leave Refs as `{"$ref": id}` objects (an object card per Ref).
- **D8 Undo for Detach.** *Recommended:* `UndoKind::Detach { node, path, body }`, skipped on replay when the BODY tree is no longer the one the detach wrote. *Alt:* leave it parked.
- **D9 `PROPS` index text.** *Recommended:* scalar values only, the `labels` subtree skipped, 4 KiB a card, no keys. *Alt:* keys and values.

## Global Constraints

- Line endings LF. Run `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass; never `--no-verify`; never silence a lint without `#[allow(..., reason = "...")]`.
- `cargo test` takes one filter per run (`cargo test -p manifold <filter>`).
- No test reads or writes the real `%APPDATA%\manifold`: paths start from `profile::dir()`; tests that need files use `store::testutil::temp_dir`.
- Names rule: a stored name reaches the screen through `display_name`; a raw `mirror.name(` needs a `// name rule:` comment and the file in `names/tests.rs` `ALLOWED`.
- No format change: `PROTOCOL_VERSION`, `RENDER_PROTOCOL_VERSION`, store `FORMAT_VERSION` stay (BODY's shape is unchanged; `o` keys stay strings that sort bytewise). No migration code.
- `INLINE_MAX_LEAVES` = 10,000 (`tree.len() >= cap` refuses), `MAX_DEPTH` = 64 (`store/tree_json.rs`).
- Python only as `python -P f.py < /dev/null` from a file; edit files with the Edit tool.
- Gate before merging: `target\debug\gate.exe all LOG light` through `~/.claude/runs/run.ps1` (README "Headless mode"); this plan changes the JSON level UI and the store writers, so `light`. Only on request.
- Do not push, tag or bump.

## Review Focus

Spec-implied inputs no task's happy-path tests cover; each has its test in the named task.

1. Deleting the last key or element leaves `{}` or `[]`, not `null` and not a vanished entry (Task 4).
2. Two replicas insert into one slot, or move one element both ways: the document converges and loses nothing; equal order keys then renumber on the next insert between them (Tasks 1, 4).
3. An undo whose path a remote edit touched since is skipped, never clobbers (Task 4, existing `replay_props` rule, pinned for BODY).
4. An insert, set or paste past 10,000 leaves or depth 64 is refused with a toast and the document is unchanged (Task 4).
5. A text edit or menu left open while another replica reorders the array commits nowhere else: it is cancelled with a toast (Task 7).
6. File cards and error cards offer no edit and `edit_json` refuses them (Tasks 4, 7).
7. Undo of an edit that emptied or retyped a container restores every descendant (Task 4).

## File Structure

- `crates/manifold/src/store/body.rs`: `between`, `elements`, `pointer_path`, `tree_to_value_refs`, `det_id`, `value_to_tree` mint change.
- `crates/manifold/src/json_edit.rs` (new): `JsonOp`, `App::edit_json`, `apply_ops` helpers, `PointerMap` + `remap_pointer`. One responsibility: turning an op into tree writes plus undo plus refresh.
- `crates/manifold/src/session_sync/remote.rs`, `node_ops.rs`, `node.rs`: `refresh_inline`, the inline predicate, payload `refs`.
- `crates/manifold/src/undo.rs`: BODY refresh after replay, `UndoKind::Detach`.
- `crates/manifold/src/json_view.rs`, `json_level.rs`, `json_level_ui.rs`: view remap, `RowEdit`, menu rows, keys, chips.
- `crates/manifold/src/debug_control.rs`, `crates/gate/src/smoke.rs`: `JSONEDIT`, `EXPORTJSON`, the `inline-json-edit` mark.
- `crates/manifold/src/search/sync.rs`: `PROPS` text.
- Tests sit beside the code (`body.rs` tests, `json_edit/tests.rs`, `json_level/tests.rs`, `session_sync/tests/props.rs`, `undo/tests.rs`).

---

### Task 1: `between`

**Files:** Modify `crates/manifold/src/store/body.rs` (add `between`, tests).

**Interfaces:**
- Produces: `pub fn between(a: Option<&str>, b: Option<&str>) -> Option<String>`: a key `k` with `a < k < b` bytewise (`None` bound = open end; an absent `o` counts as `""`); `None` when no key fits (`a >= b`, or `b` is all `'0'`). Never ends in `'0'` unless it is an 8-digit step.

- [ ] **Step 1: Write the failing tests** (in `body.rs` `mod tests`)

```rust
fn ok(a: Option<&str>, b: Option<&str>) -> String {
    let k = between(a, b).expect("room");
    assert!(a.map_or(true, |a| a < k.as_str()) && b.map_or(true, |b| k.as_str() < b), "{a:?} < {k} < {b:?}");
    k
}

#[test]
fn between_steps_ends_and_splits_gaps() {
    assert_eq!(ok(None, None), "00001000");
    assert_eq!(ok(Some("00003000"), None), "00004000", "an append steps by 1000");
    assert_eq!(ok(None, Some("00003000")), "00002000", "a prepend steps down");
    assert_eq!(ok(Some("00001000"), Some("00002000")), "000015");
    assert_eq!(ok(None, Some("00001000")), "000005", "no 1000 step fits below 1000");
}

#[test]
fn between_refuses_when_nothing_fits() {
    assert_eq!(between(Some("b"), Some("b")), None, "equal neighbours");
    assert_eq!(between(Some("c"), Some("b")), None);
    assert_eq!(between(None, Some("000")), None, "nothing sorts below an all-zero key");
}

#[test]
fn many_random_inserts_stay_strictly_ordered_and_appends_stay_short() {
    let mut keys: Vec<String> = Vec::new();
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    for _ in 0..2_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let at = (seed >> 33) as usize % (keys.len() + 1);
        let k = ok(at.checked_sub(1).map(|i| keys[i].as_str()), keys.get(at).map(String::as_str));
        keys.insert(at, k);
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
    }
    let mut tail: Vec<String> = Vec::new();
    for _ in 0..3_000 {
        let k = ok(tail.last().map(String::as_str), None);
        tail.push(k);
    }
    assert!(tail.iter().all(|k| k.len() == 8), "appends step, they don't grow");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p manifold between`
Expected: FAIL to compile (`between` not found).

- [ ] **Step 3: Implement** (below `initial_order`)

```rust
/// An order key strictly between `a` and `b` (`None`: no bound on that side), or `None` when none fits. Keys are
/// digit strings. A neighbour that is an 8-digit multiple of 1000 (what `initial_order` writes) gets the next step of
/// 1000 on the open side, so appending or prepending never lengthens keys; anything else gets the midpoint digit at
/// the first position where the bounds leave room, which never ends in `'0'` (so a lower key always exists).
pub fn between(a: Option<&str>, b: Option<&str>) -> Option<String> {
    let step = |s: &str| (s.len() == 8 && s.bytes().all(|c| c.is_ascii_digit())).then(|| s.parse::<u64>().ok()).flatten();
    match (a, b) {
        (None, None) => return Some(initial_order(0)),
        (Some(a), None) => {
            if let Some(n) = step(a).filter(|n| n % 1000 == 0 && n + 1000 < 100_000_000) {
                return Some(format!("{:08}", n + 1000));
            }
        }
        (None, Some(b)) => {
            if let Some(n) = step(b).filter(|n| n % 1000 == 0 && *n >= 2000) {
                return Some(format!("{:08}", n - 1000));
            }
        }
        (Some(a), Some(b)) if a >= b => return None,
        _ => {}
    }
    let (a, b) = (a.unwrap_or("").as_bytes(), b.map(str::as_bytes));
    let digit = |c: Option<&u8>| c.map_or(0, |c| i32::from(c.wrapping_sub(b'0')));
    let (mut out, mut open) = (Vec::new(), b.is_none());
    for i in 0..a.len().max(b.map_or(0, <[u8]>::len)) + 2 {
        let lo = digit(a.get(i));
        let hi = if open { 10 } else { digit(b.and_then(|b| b.get(i))) };
        if hi - lo > 1 {
            out.push(b'0' + ((lo + hi) / 2) as u8);
            return String::from_utf8(out).ok();
        }
        out.push(b'0' + lo as u8);
        open |= hi > lo;
    }
    None
}
```

(`a >= b` for `(Some, Some)` is checked first so equal and reversed neighbours return `None`; the all-zero `b` case exhausts the loop and returns `None`.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p manifold between`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/manifold/src/store/body.rs
git commit -m "body: between(a, b), an order key between two neighbours"
```

---

### Task 2: Pointer resolution, elements, deterministic ids

**Files:** Modify `crates/manifold/src/store/body.rs`; callers of `value_to_tree` (grep: `node_ops.rs`, `session_sync/*`, `store/json.rs`, tests).

**Interfaces:**
- Consumes: `json_view::pointer_tokens(&str) -> Vec<String>` (already unescapes `~0 ~1`).
- Produces:
  - `pub struct Element { pub id: NodeId, pub order: String, pub at: TreePath }` (`at` is the element's `Id(e)` path).
  - `pub fn elements(tree: &Tree, array: &TreePath) -> Vec<Element>`: elements with a `v`, in document order.
  - `pub fn pointer_path(tree: &Tree, pointer: &str) -> Option<TreePath>`: the path of the entry at `pointer` (an array index resolves to the element's `.../Id(e)/Key("v")` path); `None` if it does not resolve. The root pointer `""` is `[Key("v")]`.
  - `pub fn det_id(pointer: &str) -> NodeId` (`ActorId(0)`, FNV-1a 64 of the pointer).
  - `value_to_tree(value, mint: &mut dyn FnMut(&str) -> NodeId)`: the mint closure now gets the element's pointer (`"/a/0"`). `value_to_tree_at(value, at: &TreePath, base_pointer: &str, base_depth: usize, mint)` for a subtree (`base_depth` = the pointer's token count + 1, so `MAX_DEPTH` counts from the document root).

- [ ] **Step 1: Write failing tests** (`body.rs` tests): `pointer_path` of `""`, `"/a"`, `"/a/1"` (array index through an element), `"/a/9"` (None), `"/~1x"` (key `/x`); `elements` order equals `tree_to_value` order after two elements are given `o` out of id order; `det_id("/a/0") == det_id("/a/0") != det_id("/a/1")`; `value_to_tree` mint receives `"/0"` and `"/1/0"` for `[1,[2]]` in call order.

- [ ] **Step 2: Run:** `cargo test -p manifold pointer_path` Expected: FAIL (not found).

- [ ] **Step 3: Implement.** `elements`: range-scan `tree.range(array.clone()..array.subtree_end())`, for each key with `segs.len() > depth + 1`, `segs[depth] == Seg::Id(e)` and `segs[depth+1] == Key("o"|"v")` fold into a `BTreeMap<NodeId, (Option<String>, bool)>` (`o` only when the leaf is `Str` at exactly `depth + 2`; any key at or under `.../v` marks the value present); emit `Element` for those with a value, sort by `(order, id)` (absent order `""`). `pointer_path`: start `root()`; per token, if `elements(tree, &cur)` is non-empty parse the token as an index and take `elements[i].at.child(Key("v"))`, else `cur.child(Key(token))`; finally require `tree.range(cur..cur.subtree_end()).next().is_some()`. `det_id`: FNV-1a over the bytes, `NodeId { actor: ActorId(0), counter }`. In `put`, thread a `pointer: &mut String` (push `/{i}` for array elements and `/{escaped key}` for members, truncate after) and call `mint(pointer)` per element.

- [ ] **Step 4: Update the callers** of `value_to_tree`: `insert_inline_json_at` and the duplicate path pass `&mut |_| self.alloc_node_id()`; `detach_json` passes `&mut |p| body::det_id(p)` (D4). Run `cargo build -p manifold` until it compiles.

- [ ] **Step 5: Tests for D4 in `session_sync/tests/props.rs`:** two apps (`app_on(path, 1)`, `app_on(path, 2)`) detach the same file card (`json_file_card`, a file `[1,2,3]`), sync (`remote_set_tree` helpers as the existing remote tests do), and `tree_to_value` of each mirror's BODY is `[1,2,3]`, not doubled. Name: `two_replicas_detaching_one_file_do_not_double_its_arrays`.

- [ ] **Step 6: Run** `cargo test -p manifold pointer_path`, `cargo test -p manifold doubl`, then `cargo test -p manifold` (whole suite) Expected: PASS.

- [ ] **Step 7: Commit** `body: pointer_path, elements, deterministic detach ids (closes the concurrent-detach doubling)`.

---

### Task 3: One refresh hook, inline predicate, undo refresh

**Files:** Modify `session_sync/remote.rs` (`inline_payload` area, `insert_materialized`, `update_live_node`), `node_ops.rs` (`insert_inline_json_at`, `detach_json`), `undo.rs` (`replay_props`), tests in `session_sync/tests/props.rs`, `undo/tests.rs`.

**Interfaces:**
- Produces:
  - `App::refresh_inline(&mut self, id: NodeId)`: rebuilds the payload from the mirror's BODY (`inline_payload`), skips when the version is unchanged, stores it in the node's runtime, and calls `swap_json_value(id, payload.value)`. The one landing point.
  - `fn is_inline_card(&self, id, fields: &Fields) -> bool` = BODY present, or (kind Json, `str_of(fields, PATH)` empty, `str_of(fields, URL)` empty).
- `inline_payload` for a missing tree yields `Value::Null` at version 0 (D3).

- [ ] **Step 1: Failing tests.**
  - `a_remote_whole_body_tombstone_keeps_the_card_inline_and_empty`: a live inline card, a remote tombstone at `[Key("v")]` (mirror tree becomes `None`): `json_is_inline()` stays true and the payload value is `Null`.
  - `a_restored_json_card_with_no_body_path_or_url_is_inline_and_empty`, and `a_pending_fetch_json_card_with_a_url_stays_a_file_card` (URL set, PATH empty, no BODY).
  - `undoing_a_body_edit_refreshes_the_open_level` (in `undo/tests.rs`): set a BODY leaf via `edit_prop` + `record_undo(Props)`, open the level, `undo()`: the level's `value` equals the pre-edit document.
- [ ] **Step 2: Run** each by name; Expected FAIL.
- [ ] **Step 3: Implement.** `refresh_inline` in `remote.rs`; `update_live_node` calls it when `changed(BODY)` (replacing the `body`/`new_json_value` special-casing for the Json arm) and treats an emptied tree as `Null`; `insert_materialized` uses `is_inline_card`; `insert_inline_json_at` and `detach_json` replace their hand-rolled `inline_payload` + `node.runtime =` blocks with `refresh_inline`; `replay_props` collects the distinct `node` of `PropEdit`s with `field == BODY` and calls `refresh_inline` for each after the loop.
- [ ] **Step 4: Run** the three tests, then `cargo test -p manifold` Expected PASS.
- [ ] **Step 5: Commit** `inline cards: one refresh hook for local, undo and remote changes; a card stays inline when its BODY empties`.

---

### Task 4: `JsonOp` and `App::edit_json`

**Files:** Create `crates/manifold/src/json_edit.rs`, `crates/manifold/src/json_edit/tests.rs`; add `mod json_edit;` in `main.rs`/`lib` beside `json_level`.

**Interfaces:**
- Consumes: `body::{pointer_path, elements, between, value_to_tree_at, initial_order}`, `App::{edit_prop, record_undo, refresh_inline, alloc_node_id, queue_toast}`, `tree::subtree`.
- Produces:

```rust
pub(crate) enum JsonOp {
    /// The entry at `pointer` becomes `value` (a scalar edit, or a retype when the kind changes).
    Set { pointer: String, value: serde_json::Value },
    /// The member at `pointer` is renamed to `to`; refused when `to` exists.
    Rename { pointer: String, to: String },
    /// Member `key` = `value` joins the object at `pointer` (an `EmptyObj` becomes an object).
    AddKey { pointer: String, key: String, value: serde_json::Value },
    /// `value` joins the array at `pointer` at `index` (`index == len` appends).
    Insert { pointer: String, index: usize, value: serde_json::Value },
    /// The member or element at `pointer` goes; the last one leaves `{}` or `[]`.
    Remove { pointer: String },
    /// The element at `pointer` moves to index `to` of its array.
    Move { pointer: String, to: usize },
}
impl App { pub(crate) fn edit_json(&mut self, id: NodeId, op: JsonOp) -> Result<(), String>; }
```
`edit_json` returns `Err(message)` (and toasts it) for: a file or error card, an unresolvable pointer ("That entry is gone"), a duplicate key, past `INLINE_MAX_LEAVES` or `MAX_DEPTH`, removing the root. On `Ok` it has written (an op that changes nothing, such as setting a value to itself, records no step and returns `Ok`), recorded one `UndoKind::Props(edits)` (message from the op: "Edit value", "Change type", "Add key", "Rename key", "Add element", "Delete", "Move element"), and called `refresh_inline`. On `Err` nothing is written. Removing an array element tombstones the element's `Id(e)` path (the parent of its pointer path), so its `o` goes too; "the last one" means `elements(..).len() == 1` for an array and one `Key` child for an object (helper `child_count(tree, container)`). Depth for `Set`, `AddKey` and `Insert` is checked with `value_to_tree_at`'s `base_depth`. Helper: `fn replace_at(&mut self, id, path: TreePath, new: Vec<(TreePath, Leaf)>) -> Vec<PropEdit>`: when `new` is the one leaf at `path`, one `edit_prop(path, Some(leaf))`; otherwise `edit_prop(path, None)` then one `edit_prop` per leaf (the `replay_props` shape).

- [ ] **Step 1: Failing tests** (`json_edit/tests.rs`; helpers such as `app_with`, `shared` and `remote_set_tree` live in `session_sync/tests`: widen one that is private to `pub(crate)` under `cfg(test)` rather than copying it; build with `app_with` + `insert_inline_json_at`, then `edit_json`; assert on `tree_to_value(mirror BODY)` and on `app.undo()` / `redo()`):
  - `set_replaces_a_scalar_and_undo_restores_it`
  - `set_retypes_a_container_to_a_scalar_and_undo_restores_every_descendant` (Review Focus 7: `{a:{b:1,c:[2,3]}}` → `/a` = 5 → undo → original)
  - `add_key_to_an_empty_object_and_to_a_populated_one`
  - `remove_the_last_member_leaves_an_empty_object` and `remove_the_last_element_leaves_an_empty_array` (Review Focus 1)
  - `insert_at_each_end_and_between_orders_the_array` (`[]` → append a, append b, insert c at 0, insert d at 2 → `[c,a,d,b]`)
  - `move_reorders_one_register_and_undo_puts_it_back` (assert the recorded undo step holds exactly one `PropEdit`, at a path ending in `o`)
  - `insert_between_equal_order_keys_renumbers_the_array` (set two elements' `o` equal via `edit_prop`, insert between: result order stable, all `o` distinct afterwards)
  - `rename_moves_the_subtree_and_refuses_an_existing_key`
  - `edits_refuse_a_file_card_a_stale_pointer_and_the_root_delete` (assert document unchanged and one toast each)
  - `an_insert_past_the_leaf_cap_changes_nothing` (Review Focus 4; a document of `INLINE_MAX_LEAVES - 1` leaves, then `Insert`)
  - `an_undo_is_skipped_when_a_remote_edit_touched_its_path` (Review Focus 3: edit `/a`, then a remote-style `edit_prop` on a leaf under `/a`; `undo()` changes nothing)
  - `two_replicas_inserting_into_one_slot_converge` (Review Focus 2: two apps insert at index 1 of one `[a,b]`, sync, both documents equal and contain both new elements)
- [ ] **Step 2: Run:** `cargo test -p manifold json_edit` Expected FAIL (module missing).
- [ ] **Step 3: Implement** per the interface. Order for `Insert`/`Move`: take `elements(tree, array_path)` (without the moving element for `Move`), `a = orders[index-1]`, `b = orders[index]`, `key = between(a, b)`; on `None`, `renumber`: write `initial_order(i)` for every element (skipping unchanged) and place the new element at its slot, all inside the same edit vector. New element id `alloc_node_id()`. Leaf-cap check: `tree.len() - removed + added >= INLINE_MAX_LEAVES` is `Err("more than 10,000 leaves")`. `Rename`: `subtree(tree, old)` rebased to the new path, `replace_at(old, vec![])` then `replace_at(new, rebased)`.
- [ ] **Step 4: Run** `cargo test -p manifold json_edit`, then the whole suite. Expected PASS.
- [ ] **Step 5: Commit** `json_edit: JsonOp and App::edit_json, one writer for every inline edit`.

---

### Task 5: View remap on structural edits

**Files:** Modify `json_view.rs` (`JsonView::remap`), `json_edit.rs` (`PointerMap`, `remap_pointer`, call after each op), `json_level.rs` (`selected`, `outline.open`); tests in `json_edit/tests.rs`.

**Interfaces:**
- Produces: `pub(crate) enum PointerMap { Drop(String), Shift { parent: String, from: usize, delta: isize }, Permute { parent: String, from: usize, to: usize }, Prefix { old: String, new: String } }`; `pub(crate) fn remap_pointer(maps: &[PointerMap], pointer: &str) -> Option<String>` (applied in order; a Remove of an element is `[Drop(p), Shift{parent, i+1, -1}]`); `JsonView::remap(&mut self, f: &dyn Fn(&str) -> Option<String>)` (rebuilds the override map; `None` drops). `edit_json` derives the map from the op (Remove: `Drop(p)` then `Shift{parent, i+1, -1}` for an array element; Insert: `Shift{parent, index, +1}`; Move: `Permute`; Rename: `Prefix`), writes the remapped view with `edit_view`, and remaps `level.selected` and `level.outline.open`.

- [ ] **Step 1: Failing tests:** `remap_pointer` table test (drop a subtree, shift `/a/3/b` to `/a/4/b` on insert at 3, permute `/a/1` to `/a/3` while `/a/2` and `/a/3` move down, prefix rename); `inserting_an_element_keeps_a_later_cards_position` (override at `/a/2`, insert at 0, override now at `/a/3`); `deleting_an_element_drops_its_card_override_and_selection`.
- [ ] **Step 2: Run** `cargo test -p manifold remap` Expected FAIL.
- [ ] **Step 3: Implement.** Tokens via `pointer_tokens`; compare the token at `parent`'s depth as an index; re-escape with `child_pointer`.
- [ ] **Step 4: Run** that filter, then the suite. **Step 5: Commit** `json view: structural edits remap overrides, selection and outline rows`.

---

### Task 6: Headless verb, level export, `edit_json` over the wire of the app

**Files:** Modify `debug_control.rs`, `json_level.rs` (`json_level_state`: `editable`), `README.md` verb list; tests in `debug_control.rs` tests.

**Interfaces:**
- Produces: `JSONEDIT <op> <pointer> [args]` on the open level: `set <ptr> <json>`, `add <ptr> <key> <json>`, `insert <ptr> <index> <json>`, `remove <ptr>`, `rename <ptr> <to>`, `move <ptr> <to>` (the pointer has no spaces, and the word `root` stands for the root pointer `""`; `<json>` is the rest of the line). It calls `save_session()` after a successful edit, as `DETACHJSON` does. The `json_level` export gains `"editable": bool`.

- [ ] **Step 1: Failing test:** a verb table test that every `JSONEDIT` form parses to the expected `JsonOp` (extract `parse_json_edit(&str) -> Result<JsonOp, String>`), including a bad index and bad JSON.
- [ ] **Step 2-4:** run, implement, run (`cargo test -p manifold json_edit_verb`).
- [ ] **Step 5: Commit** `headless: JSONEDIT, json_level editable`.

---

### Task 7: Editing UI

**Files:** Modify `json_level.rs` (`RowEdit`, `JsonEdit` variants), `json_level_ui.rs` (double-click, text overlay, menu rows, keys), `json_level/tests.rs`.

**Interfaces:**
- Consumes: `App::edit_json`, `JsonOp`, `JsonLevel::{row_at, entry_rect}`.
- Produces:
  - `pub(crate) struct RowEdit { pub pointer: String, pub on: EditWhat, pub buffer: String, pub original: serde_json::Value }`, `pub(crate) enum EditWhat { Key, Value }`; `JsonLevel.edit: Option<RowEdit>`.
  - `JsonEdit::{Begin(String, EditWhat), Commit, CancelEdit, Op(JsonOp)}`.
  - `fn parse_edit(original: &Value, text: &str) -> Result<Value, String>`: a string original takes the text as a string; any other scalar original parses `text` as a JSON scalar (not a container), else `Err("Not a JSON value")` and the editor stays open.
  - `fn type_default(kind: ValueKind, from: &Value) -> Value` for the Type menu (string: the display text; number: the parsed number or 0; boolean: `false`; null; object: `{}`; array: `[]`).

- [ ] **Step 1: Failing tests** (`json_level/tests.rs`): `parse_edit` table; `a_double_click_on_a_value_begins_a_value_edit_only_on_an_inline_level` (apply `JsonEdit::Begin` through `apply_json_edit`; a file level leaves `edit` as `None`); `commit_applies_one_set_and_clears_the_edit`; `a_commit_after_the_entry_changed_underneath_is_cancelled` (Review Focus 5: begin an edit on `/a/1`, apply a remote-style `edit_json` Insert at 0 on the same card, then `Commit`: the document has no extra change and `edit` is `None`; the toast says it changed); `the_menu_rows_depend_on_the_entry` (object: Add key; array: Add element; element: Insert above/below, Move up/down; any non-root: Delete; none on a file level) via a pure `fn json_menu_rows(inline: bool, value: &Value, is_root: bool, parent_is_array: bool) -> Vec<&'static str>` that `build_json_menu` renders.
- [ ] **Step 2: Run** `cargo test -p manifold parse_edit` Expected FAIL.
- [ ] **Step 3: Implement.**
  - `draw_json_level`: in addition to the existing `clicked` branch (a double click also reports `clicked`, which selects the row first), when `response.double_clicked()` below the header, `edits.push(JsonEdit::Begin(pointer, what))` where `what` is `Key` when the pointer's x is left of `KEY_WIDTH` and the parent is an object, else `Value` for a scalar; only when `json.is_inline()`.
  - After the card loop, when `level.edit` is `Some`, place `egui::TextEdit::singleline(&mut edit.buffer)` over `level.entry_rect(pointer)` (grab focus on the first frame); `Enter` or focus loss pushes `Commit`, `Esc` pushes `CancelEdit`.
  - `apply_json_edit`: `Begin` stores `RowEdit` with `original = value at pointer`; `Commit` compares the current value at the pointer with `original`, on a mismatch toasts "That entry changed; edit cancelled", else builds `Set`/`Rename` (Key edits) from `parse_edit`; `Op(op)` calls `edit_json`. `swap_json_value` clears `edit` and `menu` when the value they point at differs from `original`.
  - `build_json_menu`: the rows of `json_menu_rows`, each pushing `JsonEdit::Op(..)` (Add key opens a Key edit on the new `"new"`/`"new2"` member, and Add element inserts `null` and opens a Value edit; each opens its edit after the op has landed, since the op's refresh clears an open edit); Type is a submenu of the six kinds.
  - `handle_json_keys`: `F2` begins an edit on the selection, `Delete` removes it, only when not typing and the level is inline.
  - Undo keys: `graph_keys_free` is false on a JSON level, so `handle_undo_keys` never fires there. Give it its own guard (`!modal_open && !showing_tab_view && !egui_wants_keyboard_input`, no `json_level` condition) so Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y reach `step_history` on a level; test `ctrl_z_on_a_json_level_undoes_an_edit` (press the chord via `undo_chord`'s path, assert the document). The digit-jump and arrow handlers keep `graph_keys_free`.
  - `apply_json_edit` for `Commit` and `Op` ends with `save_session()` (as `edit_json_view` does).
- [ ] **Step 4: Run** each test by name, then the suite and `cargo clippy` command. **Step 5: Commit** `json level: edit values, keys, structure and types in place`.
- [ ] **Step 6 (gate mark):** add `inline-json-edit` to `crates/gate/src/smoke.rs` after `inline-json`: `NEWJSON x y` (use `uncovered_xy`), `JSONEDIT add root a 1`, `JSONEDIT add root b [1,2]`, `JSONEDIT insert /b 0 9`, assert `json_level` keys/`outline_rows` show `a`, `b`, `/b/0 = 9`; `UNDOKEY` three times, assert the root is `{}`; `UNDOKEY shift`, assert `{"a":1}`; toggle back as `inline-json` does. Run `target\debug\gate.exe smoke` (rebuild first). Commit `gate: inline-json-edit smoke mark`.

---

### Task 8: `Ref` chips

**Files:** Modify `store/body.rs` (`tree_to_value_refs`), `node.rs` (`JsonPayload.refs`), `session_sync/remote.rs` (`inline_payload`), `json_level.rs` (`refs`), `json_level_ui.rs` (`CardLook.chips`, chip paint and click), tests in `body.rs`, `json_level/tests.rs`.

**Interfaces:**
- Produces: `pub fn tree_to_value_refs(tree: &Tree) -> (Value, BTreeMap<String, NodeId>)`: a `Ref` leaf becomes `Value::String(id.to_string())` and its pointer is recorded; `tree_to_value` (the exported `{"$ref"}` form) is the same walk with the recording off. `JsonPayload::inline(version, value, refs: Arc<BTreeMap<String, NodeId>>)` (every constructor and test call site of `JsonPayload::{inline, file}` is updated; file payloads carry an empty map); `JsonLevel.refs` (set in `new` and `swap_json_value`); `CardLook.chips: &[(usize, Chip)]` where `Chip { text: String, missing: bool }` (row index in the card, as `selected_row` does).

- [ ] **Step 1: Failing tests:** `a_ref_leaf_is_an_id_string_plus_a_marked_pointer` (`{"x": Ref(5), "l": [Ref(6)]}` gives refs `{"/x": 5, "/l/0": 6}` and the `Value` strings); `a_typed_dollar_ref_string_is_not_a_chip` (a `Str` under `$ref` stays a plain object, `refs` empty); `chips_show_the_target_title_or_missing` (level with a Ref to a live note named "N" and one to an unknown id: chip texts `N` and `missing`, the second `missing: true`); `clicking_a_chip_reveals_its_target_and_a_missing_chip_does_nothing` (a click calls `leave_json_level()` then `reveal_node(target)`; assert with the same check the `reveal.rs` tests use).
- [ ] **Step 2-4:** run, implement (title via `display_name` of `mirror.name(target)`, falling back to the kind's untitled text, per the names rule; chip click handled in the same `response.clicked()` branch when `row_at` is in `refs`), run suite.
- [ ] **Step 5: Commit** `json level: Ref leaves are card chips (target title, "missing" when dangling)`.

---

### Task 9: Export

**Files:** Modify `node_menu.rs` (`NodeAction::ExportJson`, row for inline Json), `debug_control.rs` (`EXPORTJSON <id> <path>`), `export.rs` (a `write_value` next to `write_json_export`), tests in `node_menu/tests.rs`, `fetched/tests.rs`.

**Interfaces:**
- Produces: `App::export_inline_json(&self, id: NodeId) -> Option<serde_json::Value>` (the `tree_to_value` object form, so Refs export as `{"$ref": "<id>"}`); the menu row "Export JSON..." for an inline Json card (the file-card menu keeps Reveal and Copy path); default file name `<display_name>.json`, the dialog is `export::save_dialog_with_toast`, the write is `store::atomic::replace`.

- [ ] **Step 1: Failing tests:** `export_inline_json_matches_the_document_and_round_trips_through_add_file` (export to a temp file, `ADDFILE`-path import creates a file card whose value equals); `the_inline_menu_offers_export_and_the_file_menu_does_not`.
- [ ] **Step 2-4:** run, implement, run suite. **Step 5: Commit** `inline Json: Export JSON... writes the document to a file`.

---

### Task 10: Undo for Detach

**Files:** Modify `undo.rs` (`UndoKind::Detach`, `replay_detach`), `node_ops.rs` (`detach_json` records it), `session_sync/remote.rs` (extract `rebuild_json_node(id)` from the Json arm of `insert_materialized` if none exists), tests in `undo/tests.rs`.

**Interfaces:**
- Produces: `UndoKind::Detach { node: NodeId, path: String, body: Tree }` (`body` = the BODY tree the detach wrote; a version number would not do, since `tree_version` ticks and never repeats, so an undone later edit leaves it different). Undo: only when the card is inline and its mirror tree equals `body`; tombstone BODY (`set_body(node, Tree::new())`), restore `PATH`, rebuild the file card from the file. Redo: `detach_json(node)` again.

- [ ] **Step 1: Failing tests:** `undoing_a_detach_restores_the_file_card_and_its_path`, `redoing_it_detaches_again`, `an_undo_is_skipped_when_the_body_changed_since` (a direct `edit_prop` on BODY with no undo step in between), all in `undo/tests.rs` with `json_file_card`.
- [ ] **Step 2: Read** the Json arm of `insert_materialized`; extract the file-card build into `rebuild_json_node` only if it is not already a function (the restore path must stay byte-for-byte equivalent: `cargo test -p manifold fetched` passes before and after the extraction).
- [ ] **Step 3-4:** implement, run the three tests and the suite. **Step 5: Commit** `undo: Detach to inline is one undo step`.

---

### Task 11: `PROPS` text in the global index

**Files:** Modify `search/sync.rs` (`index_print`, `index_text`), tests in `search/sync.rs` tests.

**Interfaces:**
- Produces: `App::props_text(&self, id: NodeId) -> String`: the card's `PROPS` scalar values (`Str`, `Int`, `Float`, `Num`, `Bool` as text; `Null`, `Ref`, empties skipped), the `labels` subtree skipped, joined by spaces, cut at 4096 bytes on a char boundary. `index_print` hashes `Mirror::tree_version(id, PROPS)`; `index_text` appends `props_text`.

- [ ] **Step 1: Failing tests:** `a_property_value_is_found_by_the_global_search_and_reindexed_on_change`, `a_labels_membership_stamp_is_not_indexed` (a labelled card's `Int(ms)` digits do not match), `props_text_is_cut_at_4_kib_on_a_char_boundary` (a 6 KiB multibyte value).
- [ ] **Step 2-4:** run, implement (`PROPS` has no UI in 4b; tests write it with `edit_prop`), run suite. **Step 5: Commit** `search: PROPS scalar text joins the card's index document (4 KiB cap)`.

---

### Task 12: Docs, parked and roadmap

**Files:** Modify `README.md`, `PARKED.md`, `ROADMAP.md`, the spec, this plan.

- [ ] **Step 1:** README: replace "Inline Json cards, read side" with the full description (editing gestures, the verb lines `JSONEDIT`, `EXPORTJSON`, undo, chips). Spec phase 4: "4b done <date>"; spec status `done` only if phases 1 to 4 are all done.
- [ ] **Step 2:** `PARKED.md`: delete the four closed entries; keep a short `decided against` line for element-id keying; add: "Create a Ref from the UI (a card picker)", "Drag-reorder rows of an inline array", "View overrides are not part of undo steps", "A remote structural change still prunes view overrides (`retain_resolvable`)", "A skipped undo gives no notice" (replay skips a step another replica touched, silently). Each with trigger and evidence per VERSIONING.md.
- [ ] **Step 3:** `ROADMAP.md`: strand A done; the line moves to the changelog at the bump (user's call). This plan `status: done`.
- [ ] **Step 4:** grep docs for the old wording (`4b open`, `not undoable`, `keyed by element id`, `read side`) and fix or list what is stale.
- [ ] **Step 5: Commit** `docs: properties 4b shipped; parked entries closed or restated`.

---

## Self-review

Spec coverage (section 4 "Editing", 2.2, 1.3): edit scalar, retype, add key, delete, insert, reorder: Tasks 4, 7; one undo step each: Task 4; Export: Task 9; Ref chips: Task 8; `PROPS` index text: Task 11; the four parked entries: Tasks 2 (doubling), 3 (tombstone), 5 (view overrides), 10 (Detach undo). Not in scope and parked in Task 12: Ref creation UI, drag-reorder, `BODY` global-index text (spec: parked).

Placeholder scan: Tasks 6 to 11 give interfaces, test names with their assertions and the exact insertion points rather than full code, because the UI tasks cannot be written blind of egui layout; Tasks 1 to 5 carry code or exact algorithms. No "TBD". Type consistency: `JsonOp`, `PointerMap`, `RowEdit`, `Chip`, `det_id`, `value_to_tree(_at)` are spelled the same wherever used.

Stress-test rounds (3, each against the code, not the plan's own text):

1. Round 1 found 11 defects: a clumsy `between` test line; `value_to_tree_at` lacked a base depth (so `MAX_DEPTH` would restart at a subtree); `Remove` of an element must tombstone the `Id(e)` path, not the pointer's `.../v` path, and "the last one" needed a defined count; a Remove of an array element needs two composed pointer maps; the headless verb could not express the root pointer (`root` word added); Detach's undo guard by `tree_version` can never hold (the version ticks and is never reused, so it differs after any undone later edit; now a tree-equality guard); `handle_undo_keys` is gated by `graph_keys_free`, which is false on a JSON level, so Ctrl+Z would never reach an edit (new guard and test); a double click also reports `clicked`, so the Begin push is additive to the existing branch; Review Focus 3 named the wrong task; `JsonPayload` constructors all change with `refs`; a chip click must leave the level before `reveal_node`.
2. Round 2 found 3: the move test relied on an unverified `writes` API (now asserts the recorded step); test helpers are private to `session_sync/tests` (widen, don't copy); the D3 predicate's URL clause was unverified (checked: `place_node` carries the source URL in `field::URL`, so a pending-fetch card has a URL and stays a file card).
3. Round 3 found 3: a no-op op must record no step and still return `Ok`; "Add key"/"Add element" open their editor after the op (the refresh clears an open edit); Task 12 held a conditional ("only if the executor finds users hit it") that is a placeholder, now a firm entry.

A fourth pass over the final text found nothing material.

Architecture check: no dispatcher grew a branch. The one new dispatcher is `edit_json`'s `match JsonOp`, which replaces seven would-be writers. `refresh_inline` replaces three hand-rolled swaps (detach, new, remote). What would remove more: `UndoKind::Revert` and `Fold` into register edits (already parked), and a drag-reorder that would make Move a pointer gesture instead of a menu row.
