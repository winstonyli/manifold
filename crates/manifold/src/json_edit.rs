//! Edits to an inline Json card's document: one op enum and one writer (plan 4b D1). An op is resolved against the
//! BODY tree (`body::pointer_path`), written through `edit_prop`, recorded as one `UndoKind::Props` step, and
//! shown by `refresh_inline`.

use serde_json::Value;

use crate::json_view::{child_pointer, last_key, parent_pointer, pointer_tokens};
use crate::store::body::{
    self, between, elements, initial_order, pointer_path, tree_to_value, value_to_tree_at, Element, INLINE_MAX_LEAVES,
};
use crate::store::model::{field, NodeId};
use crate::store::tree::{subtree, Leaf, Seg, Tree, TreePath};
use crate::undo::{PropEdit, UndoKind};
use crate::App;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsonOp {
    /// The entry at `pointer` becomes `value` (a scalar edit, or a retype when the kind changes).
    Set { pointer: String, value: Value },
    /// The member at `pointer` is renamed to `to`; refused when `to` exists.
    Rename { pointer: String, to: String },
    /// Member `key` = `value` joins the object at `pointer`.
    AddKey { pointer: String, key: String, value: Value },
    /// `value` joins the array at `pointer` at `index` (`index == len` appends).
    Insert { pointer: String, index: usize, value: Value },
    /// The member or element at `pointer` goes; the last one leaves `{}` or `[]`.
    Remove { pointer: String },
    /// The element at `pointer` moves to index `to` of its array.
    Move { pointer: String, to: usize },
}

impl JsonOp {
    fn label(&self) -> &'static str {
        match self {
            JsonOp::Set { .. } => "Edit value",
            JsonOp::Rename { .. } => "Rename key",
            JsonOp::AddKey { .. } => "Add key",
            JsonOp::Insert { .. } => "Add element",
            JsonOp::Remove { .. } => "Delete",
            JsonOp::Move { .. } => "Move element",
        }
    }
}

/// What a row edit changes: the member's key, or its scalar value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum EditWhat {
    Key,
    Value,
}

/// The text box open over a row (`JsonLevel::edit`). `original` is the entry's value when the box opened: a commit or
/// a refresh that finds something else there cancels the edit.
pub(crate) struct RowEdit {
    pub(crate) pointer: String,
    pub(crate) on: EditWhat,
    pub(crate) buffer: String,
    pub(crate) original: Value,
    /// Where the entry sits in the BODY tree (element ids, not indexes): an array shift that leaves an equal value at
    /// `pointer` still cancels the edit.
    pub(crate) anchor: Option<TreePath>,
    /// The box has not yet taken the keyboard focus (and the text selection) it opens with.
    pub(crate) focus: bool,
}

/// The six JSON kinds the Type menu offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum JsonType {
    String,
    Number,
    Boolean,
    Null,
    Object,
    Array,
}

impl JsonType {
    pub(crate) const ALL: [JsonType; 6] =
        [JsonType::String, JsonType::Number, JsonType::Boolean, JsonType::Null, JsonType::Object, JsonType::Array];

    pub(crate) fn label(self) -> &'static str {
        match self {
            JsonType::String => "Text",
            JsonType::Number => "Number",
            JsonType::Boolean => "Bool",
            JsonType::Null => "Null",
            JsonType::Object => "{}",
            JsonType::Array => "[]",
        }
    }
}

/// `text` as the new value of a scalar whose old value was `original`: a string takes it verbatim, any other scalar
/// reads it as a JSON scalar.
pub(crate) fn parse_edit(original: &Value, text: &str) -> Result<Value, String> {
    if original.is_string() {
        return Ok(Value::String(text.to_owned()));
    }
    match serde_json::from_str::<Value>(text.trim()) {
        Ok(value) if !value.is_object() && !value.is_array() => Ok(value),
        _ => Err("Not a JSON value".to_owned()),
    }
}

/// The value an entry of kind `kind` takes when retyped from `from`, keeping what carries over.
pub(crate) fn type_default(kind: JsonType, from: &Value) -> Value {
    match kind {
        JsonType::String => Value::String(match from {
            Value::String(s) => s.clone(),
            Value::Number(_) | Value::Bool(_) => from.to_string(),
            _ => String::new(),
        }),
        JsonType::Number => match from {
            Value::Number(_) => from.clone(),
            Value::String(s) => {
                serde_json::from_str::<serde_json::Number>(s.trim()).map_or(Value::from(0), Value::Number)
            }
            _ => Value::from(0),
        },
        JsonType::Boolean => Value::Bool(*from == Value::Bool(true) || from.as_str() == Some("true")),
        JsonType::Null => Value::Null,
        JsonType::Object => Value::Object(Default::default()),
        JsonType::Array => Value::Array(Vec::new()),
    }
}

/// The structure rows of an entry's context menu, in order; `Type` stands for the row of six kinds. A file level has
/// none (it edits nothing).
pub(crate) fn json_menu_rows(inline: bool, value: &Value, is_root: bool, parent_is_array: bool) -> Vec<&'static str> {
    if !inline {
        return Vec::new();
    }
    let mut rows = Vec::new();
    match value {
        Value::Object(_) => rows.push("Add key"),
        Value::Array(_) => rows.push("Add element"),
        _ => {}
    }
    if parent_is_array {
        rows.extend(["Insert above", "Insert below", "Move up", "Move down"]);
    }
    rows.push("Type");
    if !is_root {
        rows.push("Delete");
    }
    rows
}

/// The op a structure row of the menu (`json_menu_rows`) runs on the entry at `pointer`, with the edit to open once
/// it landed (the new entry's key or value). `None` for a row that does nothing here: Move up at the first element.
pub(crate) fn menu_op(row: &str, doc: &Value, pointer: &str) -> Option<(JsonOp, Option<(String, EditWhat)>)> {
    let value = doc.pointer(pointer)?;
    let parent = parent_pointer(pointer).unwrap_or("");
    let index = last_key(pointer).and_then(|k| k.parse::<usize>().ok());
    let siblings = doc.pointer(parent).and_then(Value::as_array).map_or(0, Vec::len);
    let insert = |at: usize| {
        let op = JsonOp::Insert { pointer: parent.to_owned(), index: at, value: Value::Null };
        Some((op, Some((child_pointer(parent, &at.to_string()), EditWhat::Value))))
    };
    match (row, value) {
        ("Add key", Value::Object(map)) => {
            let key = std::iter::once("new".to_owned())
                .chain((2..).map(|n| format!("new{n}")))
                .find(|k| !map.contains_key(k))?;
            let follow = (child_pointer(pointer, &key), EditWhat::Key);
            Some((JsonOp::AddKey { pointer: pointer.to_owned(), key, value: Value::Null }, Some(follow)))
        }
        ("Add element", Value::Array(items)) => {
            let at = items.len();
            let op = JsonOp::Insert { pointer: pointer.to_owned(), index: at, value: Value::Null };
            Some((op, Some((child_pointer(pointer, &at.to_string()), EditWhat::Value))))
        }
        ("Insert above", _) => insert(index?),
        ("Insert below", _) => insert(index? + 1),
        ("Move up", _) => Some((JsonOp::Move { pointer: pointer.to_owned(), to: index?.checked_sub(1)? }, None)),
        ("Move down", _) => {
            Some((JsonOp::Move { pointer: pointer.to_owned(), to: index.filter(|i| i + 1 < siblings)? + 1 }, None))
        }
        ("Delete", _) => Some((JsonOp::Remove { pointer: pointer.to_owned() }, None)),
        _ => None,
    }
}

/// How one structural edit moved the entries a view, selection or outline row names by JSON pointer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PointerMap {
    /// The entry at the pointer and everything under it is gone.
    Drop(String),
    /// Elements of the array at `parent` from index `from` on moved by `delta`.
    Shift { parent: String, from: usize, delta: isize },
    /// The element at `from` of the array at `parent` moved to `to`; those between slid one place.
    Permute { parent: String, from: usize, to: usize },
    /// The entry at `old` and everything under it is now at `new`.
    Prefix { old: String, new: String },
}

/// Where in `tokens` an array index under `parent` sits, and that index.
fn index_under(tokens: &[String], parent: &str) -> Option<(usize, usize)> {
    let parent = pointer_tokens(parent);
    if !tokens.starts_with(&parent) {
        return None;
    }
    Some((parent.len(), tokens.get(parent.len())?.parse().ok()?))
}

fn join(tokens: &[String]) -> String {
    tokens.iter().fold(String::new(), |p, t| child_pointer(&p, t))
}

impl PointerMap {
    fn apply(&self, pointer: &str) -> Option<String> {
        let mut tokens = pointer_tokens(pointer);
        match self {
            PointerMap::Drop(gone) => (!tokens.starts_with(&pointer_tokens(gone))).then(|| pointer.to_owned()),
            PointerMap::Shift { parent, from, delta } => {
                if let Some((at, i)) = index_under(&tokens, parent).filter(|(_, i)| i >= from) {
                    tokens[at] = i.checked_add_signed(*delta)?.to_string();
                }
                Some(join(&tokens))
            }
            PointerMap::Permute { parent, from, to } => {
                if let Some((at, i)) = index_under(&tokens, parent) {
                    let moved = match i {
                        i if i == *from => *to,
                        i if from < to && i > *from && i <= *to => i - 1,
                        i if to < from && i >= *to && i < *from => i + 1,
                        i => i,
                    };
                    tokens[at] = moved.to_string();
                }
                Some(join(&tokens))
            }
            PointerMap::Prefix { old, new } => {
                let old = pointer_tokens(old);
                if !tokens.starts_with(&old) {
                    return Some(pointer.to_owned());
                }
                let mut moved = pointer_tokens(new);
                moved.extend(tokens.drain(old.len()..));
                Some(join(&moved))
            }
        }
    }
}

/// Where `pointer` went after `maps`, applied in order; `None` when an edit removed it.
pub(crate) fn remap_pointer(maps: &[PointerMap], pointer: &str) -> Option<String> {
    maps.iter().try_fold(pointer.to_owned(), |p, map| map.apply(&p))
}

/// What `op` did to pointers, given the document it was planned against.
fn pointer_maps(op: &JsonOp, doc: &Value) -> Vec<PointerMap> {
    match op {
        JsonOp::Set { .. } | JsonOp::AddKey { .. } => Vec::new(),
        JsonOp::Insert { pointer, index, .. } => {
            vec![PointerMap::Shift { parent: pointer.clone(), from: *index, delta: 1 }]
        }
        JsonOp::Remove { pointer } => {
            let mut maps = vec![PointerMap::Drop(pointer.clone())];
            if let Ok((parent, token)) = split(pointer) {
                if let (Some(Value::Array(_)), Ok(i)) = (doc.pointer(parent), token.parse::<usize>()) {
                    maps.push(PointerMap::Shift { parent: parent.to_owned(), from: i + 1, delta: -1 });
                }
            }
            maps
        }
        JsonOp::Move { pointer, to } => match split(pointer) {
            Ok((parent, token)) => token
                .parse()
                .map(|from| PointerMap::Permute { parent: parent.to_owned(), from, to: *to })
                .into_iter()
                .collect(),
            Err(_) => Vec::new(),
        },
        JsonOp::Rename { pointer, to } => match split(pointer) {
            Ok((parent, _)) => vec![PointerMap::Prefix { old: pointer.clone(), new: child_pointer(parent, to) }],
            Err(_) => Vec::new(),
        },
    }
}

/// The first word of `text` and the trimmed rest.
fn word(text: &str) -> (&str, &str) {
    let (head, rest) = text.trim().split_once(char::is_whitespace).unwrap_or((text.trim(), ""));
    (head, rest.trim())
}

/// `text`, or an error naming `what` when it is empty.
fn need<'a>(text: &'a str, what: &str) -> Result<&'a str, String> {
    (!text.is_empty()).then_some(text).ok_or_else(|| format!("missing {what}"))
}

/// A `JSONEDIT` line: `set <ptr> <json>`, `add <ptr> <key> <json>`, `insert <ptr> <index> <json>`, `remove <ptr>`,
/// `rename <ptr> <to>`, `move <ptr> <to>`. `root` stands for the root pointer; `<json>` is the rest of the line.
pub(crate) fn parse_json_edit(line: &str) -> Result<JsonOp, String> {
    let (verb, rest) = word(line);
    let (pointer, rest) = word(rest);
    if pointer.is_empty() {
        return Err("missing a pointer".into());
    }
    let pointer = if pointer == "root" { String::new() } else { pointer.to_owned() };
    let json = |text: &str| serde_json::from_str::<Value>(need(text, "a JSON value")?).map_err(|e| e.to_string());
    let index = |text: &str| need(text, "an index")?.parse::<usize>().map_err(|e| e.to_string());
    match verb {
        "set" => Ok(JsonOp::Set { pointer, value: json(rest)? }),
        "add" => {
            let (key, value) = word(rest);
            Ok(JsonOp::AddKey { pointer, key: need(key, "a key")?.to_owned(), value: json(value)? })
        }
        "insert" => {
            let (at, value) = word(rest);
            Ok(JsonOp::Insert { pointer, index: index(at)?, value: json(value)? })
        }
        "remove" => Ok(JsonOp::Remove { pointer }),
        "rename" => Ok(JsonOp::Rename { pointer, to: need(rest, "a new key")?.to_owned() }),
        "move" => Ok(JsonOp::Move { pointer, to: index(rest)? }),
        other => Err(format!("unknown edit {other:?}")),
    }
}

const GONE: &str = "That entry is gone";
pub(crate) const EDIT_CHANGED: &str = "That entry changed; edit cancelled";

/// One write: `path` and everything under it become `leaves` (none: deleted).
type Step = (TreePath, Vec<(TreePath, Leaf)>);

fn field_key(name: &str) -> Seg {
    Seg::Key(name.to_owned())
}

fn resolve(tree: &Tree, pointer: &str) -> Result<TreePath, String> {
    if pointer.is_empty() {
        return Ok(body::root());
    }
    pointer_path(tree, pointer).ok_or_else(|| GONE.to_owned())
}

/// The parent pointer and the last token of `pointer`; the root has neither.
fn split(pointer: &str) -> Result<(&str, String), String> {
    let parent = parent_pointer(pointer).ok_or("The document itself can't be deleted or moved")?;
    Ok((parent, pointer_tokens(pointer).pop().unwrap_or_default()))
}

/// The step that sets element `e`'s order key.
fn reorder(e: &Element, order: String) -> Step {
    let at = e.at.child(&field_key("o"));
    (at.clone(), vec![(at, Leaf::Str(order))])
}

impl App {
    /// Applies `op` to inline card `id`: one write set, one undo step, one refresh. Refuses (with a toast, writing
    /// nothing) a file or error card, an entry that no longer resolves, a duplicate key and a document past
    /// `INLINE_MAX_LEAVES` or `MAX_DEPTH`. An op that changes nothing records no step.
    pub(crate) fn edit_json(&mut self, id: NodeId, op: JsonOp) -> Result<(), String> {
        match self.write_json_op(id, &op) {
            Ok((edits, maps)) => {
                if !edits.is_empty() {
                    self.record_undo(op.label().to_owned(), UndoKind::Props(edits));
                    self.remap_view_state(id, &maps);
                    self.refresh_inline(id);
                }
                Ok(())
            }
            Err(message) => {
                self.queue_toast(message.clone());
                Err(message)
            }
        }
    }

    /// Moves the saved view's overrides, the open level's selection and its outline rows to where `maps` put them.
    /// Runs before `refresh_inline`, whose prune drops whatever no longer resolves.
    fn remap_view_state(&mut self, id: NodeId, maps: &[PointerMap]) {
        let f = |p: &str| remap_pointer(maps, p);
        let mut view = self.doc.mirror.view(id).filter(|v| !v.is_opaque()).cloned();
        if let Some(view) = view.as_mut() {
            view.remap(&f);
        }
        if let Some(view) = view.filter(|v| Some(v) != self.doc.mirror.view(id)) {
            self.edit_view(id, &view.to_field());
        }
        if let Some(level) = self.json_level.as_mut().filter(|l| l.node_id == id) {
            level.selected = level.selected.take().and_then(|p| f(&p));
            level.outline.open = level.outline.open.drain().filter_map(|p| f(&p)).collect();
        }
    }

    /// Where `pointer` sits in card `id`'s BODY tree (`body::pointer_path`); `None` when it holds nothing.
    pub(crate) fn entry_anchor(&self, id: NodeId, pointer: &str) -> Option<TreePath> {
        pointer_path(self.doc.mirror.tree(id, field::BODY)?, pointer)
    }

    /// Opens the context menu over the entry at `pointer` and selects it.
    pub(crate) fn open_json_menu(&mut self, pointer: String, pos: egui::Pos2) {
        let Some(id) = self.json_level.as_ref().map(|l| l.node_id) else { return };
        let anchor = self.entry_anchor(id, &pointer);
        if let Some(level) = self.json_level.as_mut() {
            level.selected = Some(pointer.clone());
            level.menu = Some((pointer, pos, anchor));
        }
    }

    /// Opens a text box over the entry at `pointer` on the open level: its key, or its scalar value. Nothing opens on
    /// a file level, a container's value, an array element's key or the root's.
    pub(crate) fn begin_json_edit(&mut self, pointer: String, on: EditWhat) {
        let Some(level) = self.json_level.as_ref().filter(|l| self.json_editable(l.node_id)) else { return };
        let Some(value) = level.value.pointer(&pointer) else { return };
        // A Ref row is a link, not text: its value is replaced by deleting or retyping the entry, not by typing.
        if on == EditWhat::Value && level.refs.contains_key(&pointer) {
            return;
        }
        let in_object = parent_pointer(&pointer).and_then(|p| level.value.pointer(p)).is_some_and(Value::is_object);
        let text = match on {
            EditWhat::Key if in_object => last_key(&pointer),
            EditWhat::Value => crate::json_view::scalar_text(value),
            EditWhat::Key => None,
        };
        let Some(buffer) = text else { return };
        let anchor = self.entry_anchor(level.node_id, &pointer);
        let edit = RowEdit { pointer, on, buffer, original: value.clone(), anchor, focus: true };
        if let Some(level) = self.json_level.as_mut() {
            level.edit = Some(edit);
        }
    }

    /// Writes the open text box as one edit and closes it. A box whose entry changed since it opened is dropped
    /// with a toast; one whose text isn't a JSON value stays open with a toast.
    pub(crate) fn commit_json_edit(&mut self) {
        let Some(level) = self.json_level.as_mut() else { return };
        let Some(edit) = level.edit.take() else { return };
        let id = level.node_id;
        if level.value.pointer(&edit.pointer) != Some(&edit.original)
            || self.entry_anchor(id, &edit.pointer) != edit.anchor
        {
            return self.queue_toast(EDIT_CHANGED.to_owned());
        }
        let op = match edit.on {
            EditWhat::Value => match parse_edit(&edit.original, &edit.buffer) {
                Ok(value) => JsonOp::Set { pointer: edit.pointer.clone(), value },
                Err(message) => {
                    self.queue_toast(message);
                    if let Some(level) = self.json_level.as_mut() {
                        level.edit = Some(edit);
                    }
                    return;
                }
            },
            EditWhat::Key => JsonOp::Rename { pointer: edit.pointer.clone(), to: edit.buffer.clone() },
        };
        match self.edit_json(id, op) {
            Ok(()) => {
                self.save_session();
            }
            // A refused rename (the name is taken) keeps the box and the typed text, as a bad value does.
            Err(_) if edit.on == EditWhat::Key => {
                if let Some(level) = self.json_level.as_mut() {
                    level.edit = Some(edit);
                }
            }
            Err(_) => {}
        }
    }

    /// Whether `id` is a card `edit_json` accepts: inline, and not an error card.
    pub(crate) fn json_editable(&self, id: NodeId) -> bool {
        self.doc.mirror.has(id)
            && self.nodes.get(&id).and_then(|n| n.as_json()).is_some_and(|p| p.is_inline() && p.error.is_none())
    }

    /// Inline Json card `id`'s document as plain JSON (a `Ref` as `{"$ref": "<id>"}`); `None` for any other card.
    pub(crate) fn export_inline_json(&self, id: NodeId) -> Option<Value> {
        let inline = self.nodes.get(&id).and_then(|n| n.as_json()).is_some_and(|p| p.is_inline());
        inline.then(|| self.doc.mirror.tree(id, field::BODY).map_or(Value::Null, tree_to_value))
    }

    fn write_json_op(&mut self, id: NodeId, op: &JsonOp) -> Result<(Vec<PropEdit>, Vec<PointerMap>), String> {
        if !self.json_editable(id) {
            return Err("Only an inline JSON card can be edited".to_owned());
        }
        let tree = self.doc.mirror.tree(id, field::BODY).cloned().unwrap_or_default();
        let doc = tree_to_value(&tree);
        let steps = self.plan_json_op(op, &tree, &doc)?;
        let removed: usize = steps.iter().map(|(path, _)| subtree(&tree, path).len()).sum();
        let added: usize = steps.iter().map(|(_, leaves)| leaves.len()).sum();
        if (tree.len() + added).saturating_sub(removed) >= INLINE_MAX_LEAVES {
            return Err(format!("More than {INLINE_MAX_LEAVES} values in one document"));
        }
        let mut edits = Vec::new();
        for (path, leaves) in steps {
            let now = self.doc.mirror.tree(id, field::BODY).map(|t| subtree(t, &path)).unwrap_or_default();
            if now == leaves {
                continue;
            }
            if let [(only, leaf)] = leaves.as_slice() {
                if *only == path {
                    edits.extend(self.edit_prop(id, field::BODY, path, Some(leaf.clone())));
                    continue;
                }
            }
            edits.extend(self.edit_prop(id, field::BODY, path, None));
            for (leaf_path, leaf) in leaves {
                edits.extend(self.edit_prop(id, field::BODY, leaf_path, Some(leaf)));
            }
        }
        Ok((edits, pointer_maps(op, &doc)))
    }

    /// `value` as the leaves of the subtree at `at` (the entry at `pointer`), minting element ids.
    fn json_leaves(&mut self, value: &Value, at: &TreePath, pointer: &str) -> Result<Vec<(TreePath, Leaf)>, String> {
        let depth = pointer_tokens(pointer).len() + 1;
        let mut mint = |_: &str| self.alloc_node_id();
        Ok(value_to_tree_at(value, at, pointer, depth, &mut mint)?.into_iter().collect())
    }

    fn plan_json_op(&mut self, op: &JsonOp, tree: &Tree, doc: &Value) -> Result<Vec<Step>, String> {
        match op {
            JsonOp::Set { pointer, value } => {
                let path = resolve(tree, pointer)?;
                let leaves = self.json_leaves(value, &path, pointer)?;
                Ok(vec![(path, leaves)])
            }
            JsonOp::AddKey { pointer, key, value } => {
                let path = resolve(tree, pointer)?;
                let Some(Value::Object(members)) = doc.pointer(pointer) else { return Err("Not an object".into()) };
                if members.contains_key(key) {
                    return Err(format!("There is already a key \"{key}\""));
                }
                let at = path.child(&field_key(key));
                let leaves = self.json_leaves(value, &at, &child_pointer(pointer, key))?;
                Ok(vec![(at, leaves)])
            }
            JsonOp::Insert { pointer, index, value } => {
                let path = resolve(tree, pointer)?;
                if !matches!(doc.pointer(pointer), Some(Value::Array(_))) {
                    return Err("Not an array".into());
                }
                let els = elements(tree, &path);
                if *index > els.len() {
                    return Err(GONE.into());
                }
                let el = path.child(&Seg::Id(self.alloc_node_id()));
                let prev = index.checked_sub(1).map(|i| els[i].order.as_str());
                let next = els.get(*index).map(|e| e.order.as_str());
                let mut steps = Vec::new();
                let order = between(prev, next).unwrap_or_else(|| {
                    for (i, e) in els.iter().enumerate() {
                        let new = initial_order(if i < *index { i } else { i + 1 });
                        if e.order != new {
                            steps.push(reorder(e, new));
                        }
                    }
                    initial_order(*index)
                });
                let child = child_pointer(pointer, &index.to_string());
                let mut leaves = self.json_leaves(value, &el.child(&field_key("v")), &child)?;
                leaves.push((el.child(&field_key("o")), Leaf::Str(order)));
                steps.push((el, leaves));
                Ok(steps)
            }
            JsonOp::Remove { pointer } => {
                let (parent_pointer, token) = split(pointer)?;
                let path = resolve(tree, pointer)?;
                let parent = resolve(tree, parent_pointer)?;
                match doc.pointer(parent_pointer) {
                    Some(Value::Array(items)) if items.len() == 1 => {
                        Ok(vec![(parent.clone(), vec![(parent, Leaf::EmptyArr)])])
                    }
                    Some(Value::Array(_)) => {
                        let els = elements(tree, &parent);
                        let el = token.parse::<usize>().ok().and_then(|i| els.get(i)).ok_or(GONE)?;
                        Ok(vec![(el.at.clone(), Vec::new())])
                    }
                    Some(Value::Object(members)) if members.len() == 1 => {
                        Ok(vec![(parent.clone(), vec![(parent, Leaf::EmptyObj)])])
                    }
                    Some(Value::Object(_)) => Ok(vec![(path, Vec::new())]),
                    _ => Err(GONE.into()),
                }
            }
            JsonOp::Move { pointer, to } => {
                let (parent_pointer, token) = split(pointer)?;
                let parent = resolve(tree, parent_pointer)?;
                if !matches!(doc.pointer(parent_pointer), Some(Value::Array(_))) {
                    return Err("Only array elements move".into());
                }
                let mut rest = elements(tree, &parent);
                let from = token.parse::<usize>().ok().filter(|&i| i < rest.len()).ok_or(GONE)?;
                if *to >= rest.len() {
                    return Err(GONE.into());
                }
                if from == *to {
                    return Ok(Vec::new());
                }
                let moving = rest.remove(from);
                let key =
                    between(to.checked_sub(1).map(|i| rest[i].order.as_str()), rest.get(*to).map(|e| e.order.as_str()));
                Ok(match key {
                    Some(order) => vec![reorder(&moving, order)],
                    None => {
                        rest.insert(*to, moving);
                        let renumber = |(j, e): (usize, &Element)| {
                            (e.order != initial_order(j)).then(|| reorder(e, initial_order(j)))
                        };
                        rest.iter().enumerate().filter_map(renumber).collect()
                    }
                })
            }
            JsonOp::Rename { pointer, to } => {
                let (parent_pointer, key) = split(pointer)?;
                let Some(Value::Object(members)) = doc.pointer(parent_pointer) else {
                    return Err("Only object keys can be renamed".into());
                };
                if *to == key {
                    return Ok(Vec::new());
                }
                if members.contains_key(to) {
                    return Err(format!("There is already a key \"{to}\""));
                }
                let old = resolve(tree, pointer)?;
                let new = resolve(tree, parent_pointer)?.child(&field_key(to));
                let depth = old.segments().len();
                let moved = subtree(tree, &old)
                    .into_iter()
                    .map(|(p, leaf)| (p.segments().iter().skip(depth).fold(new.clone(), |at, seg| at.child(seg)), leaf))
                    .collect();
                Ok(vec![(old, Vec::new()), (new, moved)])
            }
        }
    }
}

#[cfg(test)]
mod tests;
