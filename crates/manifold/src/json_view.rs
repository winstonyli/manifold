// Used from Task 5 on; removed in Task 10. Not gated to `not(test)`: under
// `--all-targets` the test binary compiles with `cfg(test)` crate-wide, so a
// `cfg_attr(not(test), ...)` gate never applies there and the still-unused
// constants (only some are exercised by this file's own unit tests) trip
// `-D warnings` in the test target too.
#![allow(dead_code)]
//! The JSON level's pure model (spec
//! `docs/superpowers/specs/2026-09-27-json-node-rework-design.md`): the
//! saved view overrides (`VIEW`), JSON Pointers and accessor paths, the
//! text rows show, labels, which cards are visible, the tree layout,
//! search and the outline's rows. Nothing here touches `App`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A card's width in world units (100% zoom).
pub(crate) const CARD_WIDTH: f32 = 220.0;
pub(crate) const HEADER_HEIGHT: f32 = 26.0;
pub(crate) const ROW_HEIGHT: f32 = 22.0;
/// Between depth columns.
pub(crate) const COLUMN_GAP: f32 = 60.0;
/// Between sibling subtrees.
pub(crate) const SIBLING_GAP: f32 = 16.0;
/// Rows per page; a `+ N more` row reveals the next page.
pub(crate) const PAGE_SIZE: usize = 20;
/// Default expansion stops at about this many visible cards...
pub(crate) const EXPANSION_BUDGET: usize = 150;
/// ...and cards this deep start collapsed.
pub(crate) const EXPANSION_DEPTH: usize = 3;
/// Keys whose scalar value names an object, most preferred first.
pub(crate) const LABEL_KEYS: [&str; 5] = ["name", "title", "label", "id", "key"];
/// The longest string the label rule's fallback takes.
pub(crate) const LABEL_MAX_CHARS: usize = 40;
/// The longest inline preview that labels an array.
pub(crate) const PREVIEW_MAX_CHARS: usize = 30;
/// A string row shows at most this many characters; the tooltip has more.
pub(crate) const DISPLAY_MAX_CHARS: usize = 200;
/// A value with more nodes than this is searched on a worker thread.
pub(crate) const WORKER_SEARCH_NODES: usize = 100_000;

/// What the user changed about one card (spec "Saved view state"). Only
/// set fields are stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CardOverride {
    /// Top-left, in level coordinates (the root card at the origin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pos: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) collapsed: Option<bool>,
    /// `0x00RRGGBB`, as `session_sync::color_value` stores a node's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<u32>,
    /// Pages of rows revealed; one when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pages: Option<u32>,
}

impl CardOverride {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A Json node's view overrides by JSON Pointer: the `VIEW` field (14).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct JsonView(pub(crate) BTreeMap<String, CardOverride>);

impl JsonView {
    /// The stored field's text; empty or unreadable text is no overrides.
    pub(crate) fn from_field(text: &str) -> Self {
        if text.is_empty() {
            return Self::default();
        }
        serde_json::from_str(text).map(Self).unwrap_or_default()
    }

    /// Compact JSON of the non-empty overrides, or `""` when there are none.
    pub(crate) fn to_field(&self) -> String {
        let kept: BTreeMap<&String, &CardOverride> = self.0.iter().filter(|(_, o)| !o.is_empty()).collect();
        if kept.is_empty() {
            return String::new();
        }
        serde_json::to_string(&kept).unwrap_or_default()
    }

    pub(crate) fn get(&self, pointer: &str) -> Option<&CardOverride> {
        self.0.get(pointer)
    }

    pub(crate) fn entry(&mut self, pointer: &str) -> &mut CardOverride {
        self.0.entry(pointer.to_string()).or_default()
    }

    /// Pages revealed for the card at `pointer`, at least one.
    pub(crate) fn pages(&self, pointer: &str) -> usize {
        self.get(pointer).and_then(|o| o.pages).map_or(1, |pages| (pages as usize).max(1))
    }

    /// Drops overrides whose pointer no longer resolves in `root` (after
    /// a Refresh), and empty ones.
    pub(crate) fn retain_resolvable(&mut self, root: &Value) {
        self.0.retain(|pointer, o| !o.is_empty() && root.pointer(pointer).is_some());
    }

    /// Relayout: every saved position goes, the rest stays.
    pub(crate) fn clear_positions(&mut self) {
        for o in self.0.values_mut() {
            o.pos = None;
        }
        self.0.retain(|_, o| !o.is_empty());
    }
}

/// `parent`'s child `key`, escaping `~` and `/` (RFC 6901).
pub(crate) fn child_pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// `pointer`'s unescaped reference tokens; none for the root (`""`).
pub(crate) fn pointer_tokens(pointer: &str) -> Vec<String> {
    pointer.split('/').skip(1).map(|t| t.replace("~1", "/").replace("~0", "~")).collect()
}

/// The pointer one level up; `None` for the root.
pub(crate) fn parent_pointer(pointer: &str) -> Option<&str> {
    pointer.rfind('/').map(|i| &pointer[..i])
}

/// The last reference token, unescaped; `None` for the root.
pub(crate) fn last_key(pointer: &str) -> Option<String> {
    pointer_tokens(pointer).pop()
}

/// `pointer` as a script would write it: `users[0].address`, with
/// `["…"]` for keys that aren't identifiers. The root is `""`.
pub(crate) fn accessor_path(root: &Value, pointer: &str) -> String {
    let mut out = String::new();
    let mut at = Some(root);
    for token in pointer_tokens(pointer) {
        match at {
            Some(Value::Array(items)) => {
                out.push_str(&format!("[{token}]"));
                at = token.parse::<usize>().ok().and_then(|i| items.get(i));
            }
            _ => {
                if is_identifier(&token) {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(&token);
                } else {
                    out.push_str(&format!("[{}]", Value::String(token.clone())));
                }
                at = at.and_then(|v| v.get(&token));
            }
        }
    }
    out
}

fn is_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// A scalar's plain text, strings unquoted; `None` for a container.
pub(crate) fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

/// Copy value (spec "Copy"): a scalar's plain text, or a container's
/// pretty-printed JSON.
pub(crate) fn copy_value_text(value: &Value) -> String {
    scalar_text(value).unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default())
}

/// How a row shows `value`: JSON text (strings quoted and escaped, cut at
/// `DISPLAY_MAX_CHARS` with `…`), and `{n}` / `[n]` for a container.
pub(crate) fn display_text(value: &Value) -> String {
    match value {
        Value::Object(map) => format!("{{{}}}", map.len()),
        Value::Array(items) => format!("[{}]", items.len()),
        Value::String(s) if s.chars().count() > DISPLAY_MAX_CHARS => {
            format!("{}…", Value::String(s.chars().take(DISPLAY_MAX_CHARS).collect()))
        }
        scalar => scalar.to_string(),
    }
}

/// The label rule (spec "Cards"), as display text.
pub(crate) fn label(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            let preferred =
                LABEL_KEYS.iter().filter_map(|key| map.get(*key)).find(|v| !v.is_null() && scalar_text(v).is_some());
            let fallback = || map.values().find(|v| v.as_str().is_some_and(|s| s.chars().count() <= LABEL_MAX_CHARS));
            preferred.or_else(fallback).map(display_text)
        }
        Value::Array(items) => array_preview(items),
        _ => None,
    }
}

/// `[a, b]` when every element is a scalar and it fits `PREVIEW_MAX_CHARS`.
fn array_preview(items: &[Value]) -> Option<String> {
    let mut text = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if item.is_array() || item.is_object() {
            return None;
        }
        if i > 0 {
            text.push_str(", ");
        }
        text.push_str(&display_text(item));
        if text.chars().count() > PREVIEW_MAX_CHARS {
            return None;
        }
    }
    text.push(']');
    (text.chars().count() <= PREVIEW_MAX_CHARS).then_some(text)
}

pub(crate) fn entry_count(value: &Value) -> usize {
    match value {
        Value::Object(map) => map.len(),
        Value::Array(items) => items.len(),
        _ => 0,
    }
}

/// A container's entries in order, array indices as keys.
pub(crate) fn entries(value: &Value) -> Box<dyn Iterator<Item = (String, &Value)> + '_> {
    match value {
        Value::Object(map) => Box::new(map.iter().map(|(k, v)| (k.clone(), v))),
        Value::Array(items) => Box::new(items.iter().enumerate().map(|(i, v)| (i.to_string(), v))),
        _ => Box::new(std::iter::empty()),
    }
}

/// Whether a child gets its own card: non-empty containers only (plan
/// ruling 6); `{}` and `[]` show inline.
pub(crate) fn is_card(value: &Value) -> bool {
    entry_count(value) > 0
}

/// Where the entry `token` sits among `value`'s entries.
pub(crate) fn entry_index(value: &Value, token: &str) -> Option<usize> {
    match value {
        Value::Object(map) => map.keys().position(|k| k == token),
        Value::Array(items) => token.parse::<usize>().ok().filter(|&i| i < items.len()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn view_round_trips_and_stores_nothing_when_empty() {
        assert_eq!(JsonView::default().to_field(), "");
        assert_eq!(JsonView::from_field(""), JsonView::default());
        assert_eq!(JsonView::from_field("not json"), JsonView::default());

        let mut view = JsonView::default();
        view.entry("/users/0").pos = Some([520.0, 20.0]);
        view.entry("/users/0").collapsed = Some(true);
        view.entry("/users").pages = Some(2);
        view.entry("/empty"); // an override with nothing set is not stored
        let text = view.to_field();
        assert_eq!(text, r#"{"/users":{"pages":2},"/users/0":{"pos":[520.0,20.0],"collapsed":true}}"#);
        let back = JsonView::from_field(&text);
        assert_eq!(back.get("/users/0"), view.get("/users/0"));
        assert_eq!(back.pages("/users"), 2);
        assert_eq!(back.pages("/elsewhere"), 1);
    }

    #[test]
    fn stale_pointers_and_positions_are_dropped() {
        let root = json!({"a": [1, 2], "b": {"c": 3}});
        let mut view = JsonView::default();
        view.entry("/a").pages = Some(2);
        view.entry("/a/5").collapsed = Some(true);
        view.entry("/gone").pos = Some([1.0, 2.0]);
        view.entry("").pos = Some([0.0, 0.0]);
        view.retain_resolvable(&root);
        assert_eq!(view.0.keys().collect::<Vec<_>>(), ["", "/a"]);
        view.clear_positions();
        assert_eq!(view.0.keys().collect::<Vec<_>>(), ["/a"]);
    }

    #[test]
    fn pointers_escape_tilde_and_slash() {
        assert_eq!(child_pointer("", "users"), "/users");
        assert_eq!(child_pointer("/a", "x/y~z"), "/a/x~1y~0z");
        assert_eq!(pointer_tokens("/a/x~1y~0z"), ["a", "x/y~z"]);
        assert!(pointer_tokens("").is_empty());
        assert_eq!(parent_pointer("/a/b"), Some("/a"));
        assert_eq!(parent_pointer("/a"), Some(""));
        assert_eq!(parent_pointer(""), None);
        assert_eq!(last_key("/a/x~1y"), Some("x/y".to_string()));
        assert_eq!(last_key(""), None);
    }

    #[test]
    fn accessor_paths_quote_what_isnt_an_identifier() {
        let root = json!({"users": [{"address": {"city": "London"}}], "a b": {"he\"y": 1}, "x/y": [[5]]});
        assert_eq!(accessor_path(&root, "/users/0/address"), "users[0].address");
        assert_eq!(accessor_path(&root, "/a b/he\"y"), r#"["a b"]["he\"y"]"#);
        assert_eq!(accessor_path(&root, "/x~1y/0/0"), r#"["x/y"][0][0]"#);
        assert_eq!(accessor_path(&root, ""), "");
        assert_eq!(accessor_path(&json!([[1]]), "/0/0"), "[0][0]");
    }

    #[test]
    fn copy_and_display_text() {
        assert_eq!(copy_value_text(&json!("plain")), "plain");
        assert_eq!(copy_value_text(&json!(1.5)), "1.5");
        assert_eq!(copy_value_text(&json!(null)), "null");
        assert_eq!(copy_value_text(&json!({"a": 1})), "{\n  \"a\": 1\n}");
        assert_eq!(display_text(&json!("say \"hi\"")), r#""say \"hi\"""#);
        assert_eq!(display_text(&json!({"a": 1, "b": 2})), "{2}");
        assert_eq!(display_text(&json!([1, 2, 3])), "[3]");
        assert_eq!(display_text(&json!(true)), "true");
        let long = "x".repeat(DISPLAY_MAX_CHARS + 5);
        assert_eq!(display_text(&json!(long)).chars().count(), DISPLAY_MAX_CHARS + 3);
        // two quotes and "…"
    }

    #[test]
    fn labels_follow_the_spec_rule() {
        assert_eq!(label(&json!({"id": 7, "name": "Ada"})), Some("\"Ada\"".into()));
        assert_eq!(label(&json!({"key": "k", "title": "T"})), Some("\"T\"".into()));
        assert_eq!(label(&json!({"label": "L", "id": 1})), Some("\"L\"".into()));
        assert_eq!(label(&json!({"id": 1, "key": "k"})), Some("1".into()));
        assert_eq!(label(&json!({"key": "k"})), Some("\"k\"".into()));
        // Null and containers never label; the fallback takes the first short string.
        assert_eq!(label(&json!({"name": null, "id": {"x": 1}, "note": "short"})), Some("\"short\"".into()));
        assert_eq!(label(&json!({"long": "y".repeat(41), "n": 1})), None);
        assert_eq!(label(&json!({"s": "y".repeat(40)})), Some(format!("\"{}\"", "y".repeat(40))));
        assert_eq!(label(&json!([-0.12, 51.5])), Some("[-0.12, 51.5]".into()));
        assert_eq!(label(&json!([[1], 2])), None);
        assert_eq!(label(&json!(["a very long string indeed", "and more"])), None);
        assert_eq!(label(&json!([])), Some("[]".into()));
        assert_eq!(label(&json!("scalar")), None);
    }

    #[test]
    fn entries_in_file_order_with_indices_for_arrays() {
        let object = json!({"z": 1, "a": 2});
        let keys: Vec<String> = entries(&object).map(|(k, _)| k).collect();
        assert_eq!(keys, ["z", "a"]); // preserve_order: file order, not sorted
        let array = json!(["x", "y"]);
        assert_eq!(entries(&array).map(|(k, _)| k).collect::<Vec<_>>(), ["0", "1"]);
        assert_eq!(entry_index(&object, "a"), Some(1));
        assert_eq!(entry_index(&array, "1"), Some(1));
        assert_eq!(entry_index(&array, "2"), None);
        assert!(is_card(&json!({"a": 1})) && is_card(&json!([0])));
        assert!(!is_card(&json!({})) && !is_card(&json!([])) && !is_card(&json!(3)));
    }
}
