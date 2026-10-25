//! Edits to an inline Json card's document: one op enum and one writer (plan 4b D1). An op is resolved against the
//! BODY tree (`body::pointer_path`), written through `edit_prop`, recorded as one `UndoKind::Props` step, and
//! shown by `refresh_inline`.

use serde_json::Value;

use crate::json_view::{child_pointer, parent_pointer, pointer_tokens};
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

const GONE: &str = "That entry is gone";

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
            Ok(edits) => {
                if !edits.is_empty() {
                    self.record_undo(op.label().to_owned(), UndoKind::Props(edits));
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

    fn write_json_op(&mut self, id: NodeId, op: &JsonOp) -> Result<Vec<PropEdit>, String> {
        let editable =
            self.nodes.get(&id).and_then(|n| n.as_json()).is_some_and(|p| p.is_inline() && p.error.is_none());
        if !editable || !self.doc.mirror.has(id) {
            return Err("Only an inline JSON card can be edited".to_owned());
        }
        let tree = self.doc.mirror.tree(id, field::BODY).cloned().unwrap_or_default();
        let steps = self.plan_json_op(op, &tree)?;
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
        Ok(edits)
    }

    /// `value` as the leaves of the subtree at `at` (the entry at `pointer`), minting element ids.
    fn json_leaves(&mut self, value: &Value, at: &TreePath, pointer: &str) -> Result<Vec<(TreePath, Leaf)>, String> {
        let depth = pointer_tokens(pointer).len() + 1;
        let mut mint = |_: &str| self.alloc_node_id();
        Ok(value_to_tree_at(value, at, pointer, depth, &mut mint)?.into_iter().collect())
    }

    fn plan_json_op(&mut self, op: &JsonOp, tree: &Tree) -> Result<Vec<Step>, String> {
        let doc = tree_to_value(tree);
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
