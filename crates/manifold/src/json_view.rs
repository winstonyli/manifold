//! The JSON level's pure model (spec
//! `docs/superpowers/specs/2026-09-27-json-node-rework-design.md`): the
//! saved view overrides (`VIEW`), JSON Pointers and accessor paths, the
//! text rows show, labels, which cards are visible, the tree layout,
//! search and the outline's rows. Nothing here touches `App`.

use std::collections::{BTreeMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A card's width in world units (100% zoom).
pub(crate) const CARD_WIDTH: f32 = 220.0;
pub(crate) const HEADER_HEIGHT: f32 = 26.0;
/// The Json card preview's header: two lines, as card header F's 40 (here world units).
pub(crate) const PREVIEW_HEADER_HEIGHT: f32 = 40.0;
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
/// Root rows the Json card shows at its parent's level.
pub(crate) const PREVIEW_ROWS: usize = 4;

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

/// One card on a JSON level: the root, or a non-empty container whose
/// row is visible.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Card {
    pub(crate) pointer: String,
    /// The parent card's index and which of its rows leads here.
    pub(crate) parent: Option<(usize, usize)>,
    pub(crate) depth: usize,
    pub(crate) collapsed: bool,
    /// Entry rows shown: none when collapsed, else the revealed pages.
    pub(crate) rows: usize,
    /// Entries past the revealed pages, shown as one `+ N more` row.
    pub(crate) more: usize,
}

impl Card {
    pub(crate) fn height(&self) -> f32 {
        HEADER_HEIGHT + ROW_HEIGHT * (self.rows + usize::from(self.more > 0)) as f32
    }

    pub(crate) fn size(&self) -> egui::Vec2 {
        egui::vec2(CARD_WIDTH, self.height())
    }
}

/// The cards a value opens with (spec "Default expansion"): breadth-first
/// from the root, each card expanded while the visible count stays within
/// `EXPANSION_BUDGET` and it is shallower than `EXPANSION_DEPTH`. The root
/// is always expanded. Depends on the value alone (plan ruling 5).
pub(crate) fn default_expanded(root: &Value) -> HashSet<String> {
    let mut expanded = HashSet::new();
    let mut visible = 1;
    let mut queue = VecDeque::from([(String::new(), root, 0usize)]);
    while let Some((pointer, value, depth)) = queue.pop_front() {
        let children: Vec<(String, &Value)> = entries(value)
            .take(PAGE_SIZE)
            .filter(|(_, child)| is_card(child))
            .map(|(key, child)| (child_pointer(&pointer, &key), child))
            .collect();
        let is_root = pointer.is_empty();
        if !is_root && (depth >= EXPANSION_DEPTH || visible + children.len() > EXPANSION_BUDGET) {
            continue;
        }
        visible += children.len();
        expanded.insert(pointer);
        queue.extend(children.into_iter().map(|(p, child)| (p, child, depth + 1)));
    }
    expanded
}

/// The saved override if there is one, else the default.
pub(crate) fn is_collapsed(view: &JsonView, expanded: &HashSet<String>, pointer: &str) -> bool {
    view.get(pointer).and_then(|o| o.collapsed).unwrap_or(!expanded.contains(pointer))
}

/// Every visible card, breadth-first from the root (so a parent always
/// comes before its children, and siblings in row order).
pub(crate) fn visible_cards(root: &Value, view: &JsonView, expanded: &HashSet<String>) -> Vec<Card> {
    let mut cards = Vec::new();
    let mut queue = VecDeque::from([(String::new(), root, None, 0usize)]);
    while let Some((pointer, value, parent, depth)) = queue.pop_front() {
        let collapsed = is_collapsed(view, expanded, &pointer);
        let len = entry_count(value);
        let rows = if collapsed { 0 } else { len.min(view.pages(&pointer) * PAGE_SIZE) };
        let index = cards.len();
        for (row, (key, child)) in entries(value).take(rows).enumerate() {
            if is_card(child) {
                queue.push_back((child_pointer(&pointer, &key), child, Some((index, row)), depth + 1));
            }
        }
        let more = if collapsed { 0 } else { len - rows };
        cards.push(Card { pointer, parent, depth, collapsed, rows, more });
    }
    cards
}

/// Flips `pointer`'s collapse, storing an override only where it differs
/// from the default.
pub(crate) fn toggle(view: &mut JsonView, expanded: &HashSet<String>, pointer: &str) {
    let collapsed = !is_collapsed(view, expanded, pointer);
    let default = !expanded.contains(pointer);
    view.entry(pointer).collapsed = (collapsed != default).then_some(collapsed);
    view.0.retain(|_, o| !o.is_empty());
}

/// The `+ N more` row: one more page.
pub(crate) fn show_more(view: &mut JsonView, pointer: &str) {
    let pages = view.pages(pointer) + 1;
    view.entry(pointer).pages = Some(pages as u32);
}

/// Makes `pointer` visible (spec "Outline"): every ancestor expanded and
/// paged far enough to show the next step down. `true` if `view` changed.
pub(crate) fn reveal(root: &Value, view: &mut JsonView, expanded: &HashSet<String>, pointer: &str) -> bool {
    let before = view.clone();
    let mut at = root;
    let mut ancestor = String::new();
    for token in pointer_tokens(pointer) {
        let Some(index) = entry_index(at, &token) else { break };
        if is_collapsed(view, expanded, &ancestor) {
            toggle(view, expanded, &ancestor);
        }
        if index >= view.pages(&ancestor) * PAGE_SIZE {
            view.entry(&ancestor).pages = Some((index / PAGE_SIZE + 1) as u32);
        }
        at = if at.is_array() { &at[index] } else { &at[token.as_str()] };
        ancestor = child_pointer(&ancestor, &token);
    }
    *view != before
}

/// Each card's top-left (spec "Tree layout"): one column per depth; each
/// subtree a band as tall as the larger of its card and its children's
/// bands plus `SIBLING_GAP`s; a parent centred on its children's span.
/// The root is at the origin, and a saved `pos` replaces one card's only.
pub(crate) fn tree_layout(cards: &[Card], view: &JsonView) -> Vec<egui::Pos2> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); cards.len()];
    for (i, card) in cards.iter().enumerate() {
        if let Some((parent, _)) = card.parent {
            children[parent].push(i);
        }
    }
    let span = |band: &[f32], kids: &[usize]| {
        kids.iter().map(|&k| band[k]).sum::<f32>() + SIBLING_GAP * kids.len().saturating_sub(1) as f32
    };
    // Children come after their parent (`visible_cards` is breadth-first),
    // so walking backwards sizes every child before its parent.
    let mut band = vec![0.0; cards.len()];
    for i in (0..cards.len()).rev() {
        band[i] = cards[i].height().max(span(&band, &children[i]));
    }
    let mut top = vec![0.0; cards.len()];
    let mut pos = vec![egui::Pos2::ZERO; cards.len()];
    for i in 0..cards.len() {
        let mut y = top[i] + (band[i] - span(&band, &children[i])) / 2.0;
        for &k in &children[i] {
            top[k] = y;
            y += band[k] + SIBLING_GAP;
        }
        let x = cards[i].depth as f32 * (CARD_WIDTH + COLUMN_GAP);
        pos[i] = egui::pos2(x, top[i] + (band[i] - cards[i].height()) / 2.0);
    }
    let origin = pos.first().map_or(0.0, |p| p.y);
    for (card, p) in cards.iter().zip(&mut pos) {
        p.y -= origin;
        if let Some([x, y]) = view.get(&card.pointer).and_then(|o| o.pos) {
            *p = egui::pos2(x, y);
        }
    }
    pos
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SearchResult {
    /// Matching entries' pointers, in document order.
    pub(crate) matches: Vec<String>,
    /// The same pointers, for highlighting.
    pub(crate) hits: HashSet<String>,
    /// The matches and every ancestor of one.
    pub(crate) keep: HashSet<String>,
}

/// Case-insensitive substring search over object keys and scalar values
/// (spec "Search", plan ruling 9), across the whole value.
pub(crate) fn search(root: &Value, query: &str) -> SearchResult {
    let mut result = SearchResult::default();
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return result;
    }
    // Depth-first, children pushed in reverse so they pop in document order.
    let mut stack: Vec<(String, Option<&str>, &Value)> = vec![(String::new(), None, root)];
    while let Some((pointer, key, value)) = stack.pop() {
        let key_hit = key.is_some_and(|k| k.to_lowercase().contains(&needle));
        let value_hit = scalar_text(value).is_some_and(|t| t.to_lowercase().contains(&needle));
        if key_hit || value_hit {
            for (i, _) in pointer.match_indices('/') {
                result.keep.insert(pointer[..i].to_string());
            }
            result.keep.insert(pointer.clone());
            result.hits.insert(pointer.clone());
            result.matches.push(pointer.clone());
        }
        let children: Vec<(String, Option<&str>, &Value)> = match value {
            Value::Object(map) => map.iter().map(|(k, v)| (child_pointer(&pointer, k), Some(k.as_str()), v)).collect(),
            Value::Array(items) => {
                items.iter().enumerate().map(|(i, v)| (child_pointer(&pointer, &i.to_string()), None, v)).collect()
            }
            _ => Vec::new(),
        };
        stack.extend(children.into_iter().rev());
    }
    result
}

/// Nodes in `value`, itself included: decides whether search runs on a
/// worker (`WORKER_SEARCH_NODES`). serde_json's parser caps nesting at
/// 128, so the recursion is bounded.
pub(crate) fn count_nodes(value: &Value) -> usize {
    1 + match value {
        Value::Object(map) => map.values().map(count_nodes).sum(),
        Value::Array(items) => items.iter().map(count_nodes).sum(),
        _ => 0,
    }
}

/// One outline row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OutlineRow {
    pub(crate) pointer: String,
    pub(crate) depth: usize,
}

/// The outline flattened (plan ruling 8): the root, then every entry of
/// each open container. With a search (`keep`), exactly the kept rows,
/// all open.
pub(crate) fn outline_rows(root: &Value, open: &HashSet<String>, keep: Option<&HashSet<String>>) -> Vec<OutlineRow> {
    let mut rows = Vec::new();
    let mut stack = vec![(String::new(), root, 0usize)];
    while let Some((pointer, value, depth)) = stack.pop() {
        if keep.is_some_and(|keep| !keep.contains(&pointer)) {
            continue;
        }
        let is_open = keep.is_some() || open.contains(&pointer);
        if is_open {
            let children: Vec<_> = entries(value).map(|(k, v)| (child_pointer(&pointer, &k), v, depth + 1)).collect();
            stack.extend(children.into_iter().rev());
        }
        rows.push(OutlineRow { pointer, depth });
    }
    rows
}

/// A card's header title: the file name for the root, `key` for an
/// object's child, `parent[index]` for an array's.
pub(crate) fn card_title(root: &Value, pointer: &str, root_name: &str) -> String {
    let (Some(parent), Some(key)) = (parent_pointer(pointer), last_key(pointer)) else {
        return root_name.to_string();
    };
    if root.pointer(parent).is_some_and(Value::is_array) {
        format!("{}[{key}]", card_title(root, parent, root_name))
    } else {
        key
    }
}

/// The Json card's size at its parent's level (spec "The card at the
/// parent level"): a header, up to `PREVIEW_ROWS` root rows and a
/// `+ N more` row; one value row for a scalar or empty root.
pub(crate) fn preview_size(value: &Value) -> egui::Vec2 {
    let len = entry_count(value);
    let rows = if len == 0 { 1 } else { len.min(PREVIEW_ROWS) + usize::from(len > PREVIEW_ROWS) };
    egui::vec2(CARD_WIDTH, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT * rows as f32)
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

    fn users(n: usize) -> Value {
        json!({
            "page": 1,
            "users": (0..n).map(|i| json!({"id": i, "name": format!("u{i}"), "roles": ["a", "b"]})).collect::<Vec<_>>(),
            "meta": {"bbox": [[-0.12, 51.5], [0.33, 51.7]], "empty": {}},
        })
    }

    fn pointers(cards: &[Card]) -> Vec<&str> {
        cards.iter().map(|c| c.pointer.as_str()).collect()
    }

    #[test]
    fn cards_page_objects_and_arrays_and_nest_arrays() {
        let root = users(45);
        let expanded = default_expanded(&root);
        let cards = visible_cards(&root, &JsonView::default(), &expanded);
        let users_card = cards.iter().find(|c| c.pointer == "/users").unwrap();
        assert_eq!((users_card.rows, users_card.more), (20, 25));
        // Only the first page's elements have cards; `{}` gets none.
        assert!(cards.iter().any(|c| c.pointer == "/users/19"));
        assert!(!cards.iter().any(|c| c.pointer == "/users/20"));
        assert!(!cards.iter().any(|c| c.pointer == "/meta/empty"));
        // A nested array is a card whose rows are arrays with cards of their own.
        assert!(cards.iter().any(|c| c.pointer == "/meta/bbox/0"));

        let mut view = JsonView::default();
        show_more(&mut view, "/users");
        let cards = visible_cards(&root, &view, &expanded);
        let users_card = cards.iter().find(|c| c.pointer == "/users").unwrap();
        assert_eq!((users_card.rows, users_card.more), (40, 5));
        assert_eq!(users_card.height(), HEADER_HEIGHT + ROW_HEIGHT * 41.0);

        let big_object: Value = (0..30).map(|i| (format!("k{i}"), json!(i))).collect::<serde_json::Map<_, _>>().into();
        let cards = visible_cards(&big_object, &JsonView::default(), &default_expanded(&big_object));
        assert_eq!((cards[0].rows, cards[0].more), (20, 10));
    }

    #[test]
    fn default_expansion_stops_at_the_depth_and_the_budget() {
        let deep = json!({"a": {"b": {"c": {"d": {"e": 1}}}}});
        let expanded = default_expanded(&deep);
        let cards = visible_cards(&deep, &JsonView::default(), &expanded);
        assert_eq!(pointers(&cards), ["", "/a", "/a/b", "/a/b/c"]);
        assert!(!cards[2].collapsed && cards[3].collapsed); // depth 3 starts collapsed
        assert_eq!(cards[3].height(), HEADER_HEIGHT);

        // 20 users of 1 + 1 cards each fit; the budget stops before 150.
        let root = users(20);
        let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
        assert!(cards.len() <= EXPANSION_BUDGET, "{} cards", cards.len());
        let root =
            json!((0..20).map(|_| json!((0..20).map(|_| json!({"x": [1]})).collect::<Vec<_>>())).collect::<Vec<_>>());
        let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
        assert!(cards.len() <= EXPANSION_BUDGET + PAGE_SIZE, "{} cards", cards.len());
    }

    #[test]
    fn scalar_and_empty_roots_are_one_header() {
        for root in [json!(3), json!("s"), json!({}), json!([])] {
            let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
            assert_eq!(pointers(&cards), [""]);
            assert_eq!((cards[0].rows, cards[0].more), (0, 0));
        }
    }

    #[test]
    fn toggling_stores_only_a_difference_from_the_default() {
        let root = users(2);
        let expanded = default_expanded(&root);
        let mut view = JsonView::default();
        toggle(&mut view, &expanded, "/users");
        assert_eq!(view.get("/users").unwrap().collapsed, Some(true));
        assert!(is_collapsed(&view, &expanded, "/users"));
        toggle(&mut view, &expanded, "/users");
        assert_eq!(view.to_field(), "");
        // Expanding one card never collapses another (plan ruling 5):
        // `/meta/bbox/0` (depth 3, collapsed by default) holds only
        // scalars, so exactly one fewer card is collapsed afterwards.
        let collapsed = |view: &JsonView| visible_cards(&root, view, &expanded).iter().filter(|c| c.collapsed).count();
        let before = collapsed(&view);
        toggle(&mut view, &expanded, "/meta/bbox/0");
        assert_eq!(collapsed(&view), before - 1);
    }

    #[test]
    fn reveal_expands_ancestors_and_pages_to_the_target() {
        let root = users(45);
        let expanded = default_expanded(&root);
        let mut view = JsonView::default();
        toggle(&mut view, &expanded, "/users");
        assert!(reveal(&root, &mut view, &expanded, "/users/33/roles/1"));
        assert!(!is_collapsed(&view, &expanded, "/users"));
        assert_eq!(view.pages("/users"), 2);
        let cards = visible_cards(&root, &view, &expanded);
        assert!(cards.iter().any(|c| c.pointer == "/users/33/roles"));
        assert!(!reveal(&root, &mut view, &expanded, "/users/33/roles/1"), "already visible");
    }

    #[test]
    fn layout_is_columns_by_depth_with_parents_centred_and_overrides_local() {
        let root = json!({"a": {"x": 1}, "b": {"y": 2, "z": 3}});
        let expanded = default_expanded(&root);
        let cards = visible_cards(&root, &JsonView::default(), &expanded);
        let pos = tree_layout(&cards, &JsonView::default());
        assert_eq!(pos, tree_layout(&cards, &JsonView::default()), "deterministic");
        assert_eq!(pos[0], egui::pos2(0.0, 0.0), "root at the origin");
        let (a, b) = (1, 2);
        assert_eq!(pos[a].x, CARD_WIDTH + COLUMN_GAP);
        assert_eq!(pos[b].y, pos[a].y + cards[a].height() + SIBLING_GAP);
        let span_mid = (pos[a].y + pos[b].y + cards[b].height()) / 2.0;
        assert!((pos[0].y + cards[0].height() / 2.0 - span_mid).abs() < 0.01, "parent centred on its children");

        let mut view = JsonView::default();
        view.entry("/a").pos = Some([900.0, 900.0]);
        let moved = tree_layout(&cards, &view);
        assert_eq!(moved[a], egui::pos2(900.0, 900.0));
        assert_eq!((moved[0], moved[b]), (pos[0], pos[b]), "only that card moves");
    }

    #[test]
    fn search_matches_keys_and_scalars_in_hidden_pages_and_keeps_ancestors() {
        let root = users(45);
        let result = search(&root, "U33");
        assert_eq!(result.matches, ["/users/33/name"]);
        assert!(result.hits.contains("/users/33/name"));
        for kept in ["", "/users", "/users/33", "/users/33/name"] {
            assert!(result.keep.contains(kept), "{kept}");
        }
        assert_eq!(search(&root, "bbox").matches, ["/meta/bbox"]);
        assert!(search(&root, "").matches.is_empty());
        // Array indices don't match (plan ruling 9).
        assert!(search(&json!(["x"]), "0").matches.is_empty());
        assert_eq!(count_nodes(&json!({"a": [1, 2]})), 4);
    }

    #[test]
    fn outline_rows_follow_open_containers_or_the_search() {
        let root = json!({"a": {"x": 1}, "b": [true]});
        let open = HashSet::from([String::new()]);
        let rows: Vec<_> = outline_rows(&root, &open, None).into_iter().map(|r| (r.pointer, r.depth)).collect();
        assert_eq!(rows, [("".into(), 0), ("/a".into(), 1), ("/b".into(), 1)]);
        let open = HashSet::from([String::new(), "/a".to_string()]);
        let rows: Vec<_> = outline_rows(&root, &open, None).into_iter().map(|r| r.pointer).collect();
        assert_eq!(rows, ["", "/a", "/a/x", "/b"]);
        let found = search(&root, "true");
        let rows: Vec<_> =
            outline_rows(&root, &HashSet::new(), Some(&found.keep)).into_iter().map(|r| r.pointer).collect();
        assert_eq!(rows, ["", "/b", "/b/0"]);
    }

    #[test]
    fn titles_and_preview_sizes() {
        let root = users(3);
        assert_eq!(card_title(&root, "", "users.json"), "users.json");
        assert_eq!(card_title(&root, "/users/0", "users.json"), "users[0]");
        assert_eq!(card_title(&root, "/meta/bbox/1", "users.json"), "bbox[1]");
        assert_eq!(card_title(&root, "/meta", "users.json"), "meta");
        assert_eq!(preview_size(&root), egui::vec2(CARD_WIDTH, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT * 3.0));
        let wide = json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5});
        assert_eq!(preview_size(&wide).y, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT * 5.0); // 4 rows and "+ 1 more"
        assert_eq!(preview_size(&json!(7)).y, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT);
    }
}
