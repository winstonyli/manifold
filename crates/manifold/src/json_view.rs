//! The JSON level's pure model (spec
//! `docs/superpowers/specs/2026-09-27-json-node-rework-design.md`): the
//! saved view overrides (`VIEW`), JSON Pointers and accessor paths, the
//! text rows show, labels, which cards are visible, the tree layout,
//! search and the outline's rows. Nothing here touches `App`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A card's width in world units (100% zoom).
pub(crate) const CARD_WIDTH: f32 = 220.0;
pub(crate) const HEADER_HEIGHT: f32 = 26.0;
/// The Json card preview's header: two lines, as card header F's 40 (here world units).
pub(crate) const PREVIEW_HEADER_HEIGHT: f32 = crate::card::TAB_HEADER_POINTS;
/// The Json card preview's width: the other cards' (`GRAPH_NODE_SIZE`), not a level card's `CARD_WIDTH`.
pub(crate) const PREVIEW_CARD_WIDTH: f32 = 160.0;
pub(crate) const ROW_HEIGHT: f32 = 22.0;
/// Between depth columns.
pub(crate) const COLUMN_GAP: f32 = 60.0;
/// Between sibling subtrees.
pub(crate) const SIBLING_GAP: f32 = 16.0;
/// Rows per page; a `+ N more` row reveals the next page.
pub(crate) const PAGE_SIZE: usize = 20;
/// A reveal this many pages past a card's window grows the window; farther, it moves it.
pub(crate) const REVEAL_GROW_PAGES: usize = 5;
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
    /// The first page shown (H1b): a far reveal moves the window here instead of paging every row in; `+ N
    /// earlier` grows it up. 0 when unset, so a view saved before it reads unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first: Option<u32>,
}

impl CardOverride {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The keys `CardOverride` reads; any other key of a stored override is kept as it was (`Unread::extra`).
const CARD_OVERRIDE_KEYS: [&str; 5] = ["pos", "collapsed", "color", "pages", "first"];

/// What a view's stored text held that this build can't read (M5), kept so an edit here writes it back untouched
/// (`JsonView::to_field`) rather than wiping it on every replica: whole entries that don't read as a `CardOverride`
/// (a newer build changed a field's shape), the unknown keys of entries that do, and text that isn't an object of
/// overrides at all, which no edit may overwrite (`JsonView::is_opaque`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Unread {
    entries: BTreeMap<String, Value>,
    extra: BTreeMap<String, serde_json::Map<String, Value>>,
    opaque: Option<String>,
}

/// A Json node's view overrides by JSON Pointer: the `VIEW` field (14), and what this build couldn't read of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct JsonView(pub(crate) BTreeMap<String, CardOverride>, pub(crate) Unread);

/// No overrides: what a node without a view (`Mirror::view`'s `None`) lays out with.
pub(crate) static NO_VIEW: JsonView =
    JsonView(BTreeMap::new(), Unread { entries: BTreeMap::new(), extra: BTreeMap::new(), opaque: None });

/// One pointer's entry as `to_field` writes it: an unreadable one as it was, or an override with its unknown keys
/// after its own. Serialising `CardOverride` itself (not through `serde_json::to_value`, which widens an `f32` to
/// `f64`) keeps today's text.
#[derive(Serialize)]
#[serde(untagged)]
enum Written<'a> {
    Unread(&'a Value),
    Card(WrittenCard<'a>),
}

/// An override and its unknown keys, flattened into one object. (Not `Card`: that is this file's laid-out card.)
#[derive(Serialize)]
struct WrittenCard<'a> {
    #[serde(flatten)]
    known: Option<&'a CardOverride>,
    #[serde(flatten)]
    extra: Option<&'a serde_json::Map<String, Value>>,
}

impl JsonView {
    /// The stored field's text; empty text is no overrides. Each entry is read on its own, and what this build
    /// can't read is kept (`Unread`).
    pub(crate) fn from_field(text: &str) -> Self {
        if text.is_empty() {
            return Self::default();
        }
        let Ok(raw) = serde_json::from_str::<BTreeMap<String, Value>>(text) else {
            eprintln!("json view: the stored view is not an object of overrides; kept as is, edits to it refused");
            return JsonView(BTreeMap::new(), Unread { opaque: Some(text.to_owned()), ..Unread::default() });
        };
        let mut view = Self::default();
        for (pointer, value) in raw {
            let read = match &value {
                Value::Object(object) => CardOverride::deserialize(&value).ok().map(|known| {
                    let extra: serde_json::Map<String, Value> = object
                        .iter()
                        .filter(|(key, _)| !CARD_OVERRIDE_KEYS.contains(&key.as_str()))
                        .map(|(key, v)| (key.clone(), v.clone()))
                        .collect();
                    (known, extra)
                }),
                _ => None,
            };
            match read {
                Some((known, extra)) => {
                    if !known.is_empty() {
                        view.0.insert(pointer.clone(), known);
                    }
                    if !extra.is_empty() {
                        view.1.extra.insert(pointer, extra);
                    }
                }
                None => {
                    view.1.entries.insert(pointer, value);
                }
            }
        }
        view
    }

    /// Compact JSON of the non-empty overrides and of what this build couldn't read, written back as it was, or `""`
    /// when there is nothing. Opaque text comes back unchanged.
    pub(crate) fn to_field(&self) -> String {
        if let Some(text) = &self.1.opaque {
            return text.clone();
        }
        let mut out: BTreeMap<&str, Written> =
            self.1.entries.iter().map(|(pointer, value)| (pointer.as_str(), Written::Unread(value))).collect();
        let pointers: BTreeSet<&str> = self
            .0
            .iter()
            .filter(|(_, o)| !o.is_empty())
            .map(|(pointer, _)| pointer.as_str())
            .chain(self.1.extra.keys().map(String::as_str))
            .collect();
        for pointer in pointers {
            let known = self.0.get(pointer).filter(|o| !o.is_empty());
            out.insert(pointer, Written::Card(WrittenCard { known, extra: self.1.extra.get(pointer) }));
        }
        if out.is_empty() {
            return String::new();
        }
        serde_json::to_string(&out).unwrap_or_default()
    }

    /// No overrides and nothing unread: `Mirror::set` stores no view for it.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty() && self.1 == Unread::default()
    }

    /// Text that isn't an object of overrides (a newer format): laid out as no overrides, written back unchanged,
    /// and never edited here (`edit_json_view`).
    pub(crate) fn is_opaque(&self) -> bool {
        self.1.opaque.is_some()
    }

    pub(crate) fn get(&self, pointer: &str) -> Option<&CardOverride> {
        self.0.get(pointer)
    }

    /// The override to edit at `pointer`. An unreadable entry there goes: the user's edit replaces that card's state.
    pub(crate) fn entry(&mut self, pointer: &str) -> &mut CardOverride {
        self.1.entries.remove(pointer);
        self.0.entry(pointer.to_string()).or_default()
    }

    /// Pages revealed for the card at `pointer`, at least one.
    pub(crate) fn pages(&self, pointer: &str) -> usize {
        self.get(pointer).and_then(|o| o.pages).map_or(1, |pages| (pages as usize).max(1))
    }

    /// The first page shown for the card at `pointer`; 0 when unset.
    pub(crate) fn first(&self, pointer: &str) -> usize {
        self.get(pointer).and_then(|o| o.first).map_or(0, |first| first as usize)
    }

    /// Drops overrides whose pointer no longer resolves in `root` (after a Refresh), and empty ones; unread entries
    /// and keys go by the same rule. Opaque text is left alone.
    pub(crate) fn retain_resolvable(&mut self, root: &Value) {
        self.0.retain(|pointer, o| !o.is_empty() && root.pointer(pointer).is_some());
        self.1.entries.retain(|pointer, _| root.pointer(pointer).is_some());
        self.1.extra.retain(|pointer, _| root.pointer(pointer).is_some());
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
    entries_from(value, 0)
}

/// `entries` from entry `start` on. An array starts there directly; an object steps past `start` entries
/// (`serde_json`'s map iterator has no fast `nth`).
pub(crate) fn entries_from(value: &Value, start: usize) -> Box<dyn Iterator<Item = (String, &Value)> + '_> {
    match value {
        Value::Object(map) => Box::new(map.iter().skip(start).map(|(k, v)| (k.clone(), v))),
        Value::Array(items) => {
            let start = start.min(items.len());
            Box::new(items[start..].iter().enumerate().map(move |(i, v)| ((start + i).to_string(), v)))
        }
        _ => Box::new(std::iter::empty()),
    }
}

/// The rows of a card whose first row's top is `rows_top` that intersect `visible` (world), as a range of its
/// `rows` rows. `Rect::EVERYTHING` gives every row: float-to-int casts saturate.
pub(crate) fn visible_rows(rows_top: f32, rows: usize, visible: egui::Rect) -> std::ops::Range<usize> {
    let first = ((visible.top() - rows_top) / ROW_HEIGHT).floor().max(0.0) as usize;
    let end = ((visible.bottom() - rows_top) / ROW_HEIGHT).ceil().max(0.0) as usize;
    first.min(rows)..end.min(rows)
}

/// The entry index of `pointer` on the card at `card` (`value` is the card's value), when `pointer` is a canonical
/// child of it (its own pointer as `child_pointer` writes it) within `window`.
pub(crate) fn row_of(value: &Value, card: &str, pointer: &str, window: std::ops::Range<usize>) -> Option<usize> {
    let token = pointer.strip_prefix(card)?.strip_prefix('/')?;
    if token.contains('/') {
        return None;
    }
    let key = token.replace("~1", "/").replace("~0", "~");
    let row = entry_index(value, &key).filter(|row| window.contains(row))?;
    let canonical = if value.is_array() { row.to_string() } else { key };
    (child_pointer(card, &canonical) == pointer).then_some(row)
}

/// Each search hit's row on its parent's card: sorted entry indices per parent pointer. A hit that is not a
/// canonical child pointer (none from `search_with` is) or that names no entry is left out; the root has no row.
pub(crate) fn hit_rows_by_parent(root: &Value, hits: &HashSet<String>) -> HashMap<String, Vec<usize>> {
    let mut keys: HashMap<&str, HashSet<String>> = HashMap::new();
    for hit in hits {
        let (Some(parent), Some(key)) = (parent_pointer(hit), last_key(hit)) else { continue };
        if child_pointer(parent, &key) == *hit {
            keys.entry(parent).or_default().insert(key);
        }
    }
    keys.into_iter()
        .filter_map(|(parent, keys)| {
            let mut rows: Vec<usize> = match root.pointer(parent)? {
                Value::Object(map) => {
                    map.keys().enumerate().filter(|(_, k)| keys.contains(*k)).map(|(i, _)| i).collect()
                }
                Value::Array(items) => keys
                    .iter()
                    .filter_map(|k| k.parse::<usize>().ok().filter(|&i| i < items.len() && i.to_string() == *k))
                    .collect(),
                _ => return None,
            };
            rows.sort_unstable();
            Some((parent.to_string(), rows))
        })
        .collect()
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
    /// Entries before the window: shown as one `+ N earlier` row when over 0.
    pub(crate) skip: usize,
    /// Entry rows shown: none when collapsed, else the window's pages.
    pub(crate) rows: usize,
    /// Entries past the revealed pages, shown as one `+ N more` row.
    pub(crate) more: usize,
}

impl Card {
    /// The `+ N earlier` row: 1 when the window starts past the first entry.
    pub(crate) fn lead(&self) -> usize {
        usize::from(self.skip > 0)
    }

    pub(crate) fn height(&self) -> f32 {
        HEADER_HEIGHT + ROW_HEIGHT * (self.lead() + self.rows + usize::from(self.more > 0)) as f32
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
        // The window starts on a page boundary, at the last page at most.
        let skip = if collapsed {
            0
        } else {
            (view.first(&pointer) * PAGE_SIZE).min(len.saturating_sub(1) / PAGE_SIZE * PAGE_SIZE)
        };
        let rows = if collapsed { 0 } else { (len - skip).min(view.pages(&pointer) * PAGE_SIZE) };
        let lead = usize::from(skip > 0);
        let index = cards.len();
        for (row, (key, child)) in entries_from(value, skip).take(rows).enumerate() {
            if is_card(child) {
                // The row as drawn: below the `+ N earlier` row when there is one.
                queue.push_back((child_pointer(&pointer, &key), child, Some((index, lead + row)), depth + 1));
            }
        }
        let more = if collapsed { 0 } else { len - skip - rows };
        cards.push(Card { pointer, parent, depth, collapsed, skip, rows, more });
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

/// The `+ N earlier` row: one more page above the window, so it grows up.
pub(crate) fn show_earlier(root: &Value, view: &mut JsonView, pointer: &str) {
    let len = root.pointer(pointer).map_or(0, entry_count);
    let first = view.first(pointer).min(len.saturating_sub(1) / PAGE_SIZE);
    if first == 0 {
        return;
    }
    let pages = view.pages(pointer) + 1;
    let o = view.entry(pointer);
    o.first = (first > 1).then(|| (first - 1) as u32);
    o.pages = Some(pages as u32);
}

/// Makes `pointer` visible (spec "Outline"): every ancestor expanded and its window holding the next step down
/// (grown when within REVEAL_GROW_PAGES pages, moved otherwise). `true` if `view` changed.
pub(crate) fn reveal(root: &Value, view: &mut JsonView, expanded: &HashSet<String>, pointer: &str) -> bool {
    let before = view.clone();
    let mut at = root;
    let mut ancestor = String::new();
    for token in pointer_tokens(pointer) {
        let Some(index) = entry_index(at, &token) else { break };
        if is_collapsed(view, expanded, &ancestor) {
            toggle(view, expanded, &ancestor);
        }
        let (page, first, pages) = (index / PAGE_SIZE, view.first(&ancestor), view.pages(&ancestor));
        let window = if page < first && first - page <= REVEAL_GROW_PAGES {
            Some((page, pages + (first - page))) // just above: grow up
        } else if page >= first + pages && page < first + pages + REVEAL_GROW_PAGES {
            Some((first, page - first + 1)) // just below: grow down, as before
        } else if page < first || page >= first + pages {
            Some((page, 1)) // far: move the window to it
        } else {
            None
        };
        if let Some((first, pages)) = window {
            let o = view.entry(&ancestor);
            o.first = (first > 0).then_some(first as u32);
            o.pages = (pages > 1).then_some(pages as u32);
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
    /// The search stopped at MAX_MATCHES; there are more.
    pub(crate) truncated: bool,
}

/// The most matches a search collects; the status line says when it stopped.
pub(crate) const MAX_MATCHES: usize = 10_000;

/// Case-insensitive substring search over object keys and scalar values
/// (spec "Search", plan ruling 9), across the whole value.
pub(crate) fn search(root: &Value, query: &str) -> SearchResult {
    search_with(root, query, &|| false).expect("never cancelled")
}

/// One open container's children in `search_with`'s walk.
enum Children<'a> {
    Object(serde_json::map::Iter<'a>),
    Array(std::iter::Enumerate<std::slice::Iter<'a, Value>>),
}

/// Appends `key` to `path` as one pointer token, escaping `~` and `/` as `child_pointer` does.
fn push_token(path: &mut String, key: &str) {
    path.push('/');
    for c in key.chars() {
        match c {
            '~' => path.push_str("~0"),
            '/' => path.push_str("~1"),
            c => path.push(c),
        }
    }
}

/// Whether `text` contains `needle` (already lowercased), ignoring case as `str::to_lowercase` does. When both
/// are ASCII, a byte compare with no allocation. Otherwise `to_lowercase().contains`, as before: a non-ASCII text
/// can lowercase to ASCII (the Kelvin sign is `k`), and lowercasing can change a text's length.
fn contains_lower(text: &str, needle: &str) -> bool {
    if text.is_ascii() && needle.is_ascii() {
        let (text, needle) = (text.as_bytes(), needle.as_bytes());
        needle.is_empty() || text.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
    } else {
        text.to_lowercase().contains(needle)
    }
}

/// `search`, checking `cancelled` every 4096 nodes: `None` once it says so
/// (a newer query superseded this worker's).
pub(crate) fn search_with(root: &Value, query: &str, cancelled: &dyn Fn() -> bool) -> Option<SearchResult> {
    let mut result = SearchResult::default();
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return Some(result);
    }
    use std::fmt::Write;
    // Depth-first in document order with one path: a token is pushed going down and cut off coming back up
    // (`stack` holds each open container's children and its path length).
    let mut path = String::new();
    let mut number = String::new();
    let mut stack: Vec<(Children, usize)> = Vec::new();
    let mut node: Option<(Option<&str>, &Value)> = Some((None, root));
    let mut visited = 0usize;
    while let Some((key, value)) = node.take() {
        visited += 1;
        if visited.is_multiple_of(4096) && cancelled() {
            return None;
        }
        let key_hit = key.is_some_and(|k| contains_lower(k, &needle));
        let value_hit = match value {
            Value::String(s) => contains_lower(s, &needle),
            Value::Number(n) => {
                number.clear();
                let _ = write!(number, "{n}");
                contains_lower(&number, &needle)
            }
            Value::Bool(b) => contains_lower(if *b { "true" } else { "false" }, &needle),
            Value::Null => contains_lower("null", &needle),
            Value::Array(_) | Value::Object(_) => false,
        };
        if key_hit || value_hit {
            if result.matches.len() == MAX_MATCHES {
                result.truncated = true; // a match past the cap: there are more than it shows
                break;
            }
            for (i, _) in path.match_indices('/') {
                result.keep.insert(path[..i].to_string());
            }
            result.keep.insert(path.clone());
            result.hits.insert(path.clone());
            result.matches.push(path.clone());
        }
        match value {
            Value::Object(map) => stack.push((Children::Object(map.iter()), path.len())),
            Value::Array(items) => stack.push((Children::Array(items.iter().enumerate()), path.len())),
            _ => {}
        }
        // The next node: the next child of the innermost container that has one left.
        while let Some((children, len)) = stack.last_mut() {
            path.truncate(*len);
            let next = match children {
                Children::Object(iter) => iter.next().map(|(k, v)| {
                    push_token(&mut path, k);
                    (Some(k.as_str()), v)
                }),
                Children::Array(iter) => iter.next().map(|(i, v)| {
                    path.push('/');
                    let _ = write!(path, "{i}");
                    (None, v)
                }),
            };
            if next.is_some() {
                node = next;
                break;
            }
            stack.pop();
        }
    }
    Some(result)
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
    egui::vec2(PREVIEW_CARD_WIDTH, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT * rows as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Wave 2 (H1b): revealing the last of 20 000 objects moves the array's window to it: one page of rows, so one
    /// page of cards.
    #[test]
    fn reveal_bounds_rows() {
        let root = json!({"items": (0..20_000).map(|n| json!({"n": n})).collect::<Vec<_>>()});
        let expanded = default_expanded(&root);
        let mut view = JsonView::default();
        assert!(reveal(&root, &mut view, &expanded, "/items/19999"));
        let cards = visible_cards(&root, &view, &expanded);
        let items = cards.iter().find(|c| c.pointer == "/items").expect("the array's card");
        assert_eq!((cards.len(), items.rows, items.skip, items.more), (22, 20, 19_980, 0));
    }

    /// Wave 2 (H1b): a view saved before `first` existed reads, lays out and writes back exactly as before.
    #[test]
    fn an_old_view_round_trips_without_first() {
        let text = r#"{"/items":{"collapsed":false,"pages":3}}"#;
        let view = JsonView::from_field(text);
        assert_eq!(view.first("/items"), 0);
        assert_eq!(view.pages("/items"), 3);
        assert_eq!(view.to_field(), text);
    }

    /// Wave 2 (H1b): a reveal near the window grows it as before; a far one moves it; `+ N earlier` grows it up.
    #[test]
    fn reveal_moves_the_window_and_earlier_grows_it_up() {
        let root = json!({"items": (0..1000).collect::<Vec<_>>()});
        let expanded = default_expanded(&root);
        let mut view = JsonView::default();
        reveal(&root, &mut view, &expanded, "/items/90"); // page 4: within 5 of the window's end
        assert_eq!((view.first("/items"), view.pages("/items")), (0, 5));
        reveal(&root, &mut view, &expanded, "/items/900"); // page 45: far
        assert_eq!((view.first("/items"), view.pages("/items")), (45, 1));
        show_earlier(&root, &mut view, "/items");
        assert_eq!((view.first("/items"), view.pages("/items")), (44, 2));
        let items = visible_cards(&root, &view, &expanded).into_iter().find(|c| c.pointer == "/items").unwrap();
        assert_eq!((items.skip, items.rows, items.more, items.lead()), (880, 40, 80, 1));
    }

    #[test]
    fn view_round_trips_and_stores_nothing_when_empty() {
        assert_eq!(JsonView::default().to_field(), "");
        assert_eq!(JsonView::from_field(""), JsonView::default());
        assert!(JsonView::from_field("not json").is_opaque(), "kept, not read as no overrides");

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

    /// M5: one entry this build can't read (a newer build's shape) costs neither the others nor itself.
    #[test]
    fn an_entry_this_build_cannot_read_survives_an_edit() {
        let mut view = JsonView::from_field(r#"{"/a":{"pages":{"offset":0,"pages":2}},"/b":{"collapsed":true}}"#);
        assert_eq!(view.get("/b").and_then(|o| o.collapsed), Some(true), "the readable entry still loads");
        view.entry("/c").collapsed = Some(true);
        let back: Value = serde_json::from_str(&view.to_field()).unwrap();
        assert_eq!(back["/a"]["pages"]["offset"], 0, "written back as it was");
        assert_eq!(back["/b"]["collapsed"], true);
        assert_eq!(back["/c"]["collapsed"], true);
    }

    /// M5: an override's unknown keys, a non-object entry and text that isn't an object of overrides are all
    /// written back; positions keep their short f32 form.
    #[test]
    fn unknown_keys_and_unreadable_text_are_written_back() {
        let mut view = JsonView::from_field(r#"{"/a":{"collapsed":true,"width":3},"/x":[]}"#);
        assert_eq!(view.get("/a").and_then(|o| o.collapsed), Some(true));
        view.entry("/a").collapsed = None;
        view.entry("/b").pos = Some([0.1, 0.2]);
        assert_eq!(view.to_field(), r#"{"/a":{"width":3},"/b":{"pos":[0.1,0.2]},"/x":[]}"#);
        let opaque = JsonView::from_field("[1]");
        assert!(opaque.is_opaque() && !opaque.is_empty());
        assert_eq!(opaque.to_field(), "[1]");
        assert!(JsonView::from_field("").is_empty());
    }

    /// The keys `from_field` treats as known are exactly the ones `CardOverride` writes.
    #[test]
    fn card_override_keys_are_the_fields_it_reads() {
        let full = CardOverride {
            pos: Some([1.0, 2.0]),
            collapsed: Some(true),
            color: Some(1),
            pages: Some(2),
            first: Some(3),
        };
        let Value::Object(object) = serde_json::to_value(&full).unwrap() else { panic!("an object") };
        assert_eq!(object.keys().map(String::as_str).collect::<Vec<_>>(), CARD_OVERRIDE_KEYS);
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
    fn visible_rows_cover_the_rows_the_rect_touches() {
        let r = |top, bottom| egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(220.0, bottom));
        assert_eq!(visible_rows(26.0, 25, r(137.0, 245.0)), 5..10);
        assert_eq!(visible_rows(26.0, 25, egui::Rect::EVERYTHING), 0..25, "the preview: every row");
        assert_eq!(visible_rows(26.0, 25, r(-500.0, 0.0)), 0..0, "above the rows");
        assert_eq!(visible_rows(26.0, 25, r(5000.0, 6000.0)), 25..25, "below them");
    }

    #[test]
    fn row_of_and_hit_rows_take_canonical_children_only() {
        let root = json!({"a~b": {"x/y": 1, "z": 2}, "arr": [10, 20, 30]});
        assert_eq!(row_of(&root["a~b"], "/a~0b", "/a~0b/x~1y", 0..2), Some(0));
        assert_eq!(row_of(&root["a~b"], "/a~0b", "/a~0b/z", 0..1), None, "outside the window");
        assert_eq!(row_of(&root["arr"], "/arr", "/arr/2", 0..3), Some(2));
        assert_eq!(row_of(&root["arr"], "/arr", "/arr/02", 0..3), None, "not canonical");
        assert_eq!(row_of(&root["arr"], "/arr", "/arr", 0..3), None, "the card itself");
        assert_eq!(row_of(&root, "", "/arr/2", 0..2), None, "a grandchild");
        let hits = HashSet::from(["/a~0b/z", "/arr/0", "/arr/2", "/arr/02"].map(String::from));
        let rows = hit_rows_by_parent(&root, &hits);
        assert_eq!(rows.get("/a~0b"), Some(&vec![1]));
        assert_eq!(rows.get("/arr"), Some(&vec![0, 2]));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn entries_from_starts_at_an_entry() {
        let root = json!({"o": {"a": 1, "b": 2, "c": 3}, "arr": [10, 20, 30]});
        let keys = |v: &Value, start| entries_from(v, start).map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(keys(&root["o"], 1), ["b", "c"]);
        assert_eq!(keys(&root["arr"], 2), ["2"]);
        assert!(keys(&root["arr"], 9).is_empty());
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

    /// Review D-R7: results stop at MAX_MATCHES, and a cancelled search returns nothing.
    #[test]
    fn search_is_capped_and_cancellable() {
        let big = serde_json::Value::Array((0..MAX_MATCHES + 50).map(|i| serde_json::json!(format!("x{i}"))).collect());
        let result = search(&big, "x");
        assert_eq!(result.matches.len(), MAX_MATCHES);
        assert!(result.truncated);
        assert!(search_with(&big, "x", &|| true).is_none());
        assert!(!search(&serde_json::json!({"a": "x"}), "x").truncated);
        let exact = serde_json::Value::Array((0..MAX_MATCHES).map(|i| serde_json::json!(format!("x{i}"))).collect());
        let result = search(&exact, "x");
        assert_eq!(result.matches.len(), MAX_MATCHES);
        assert!(!result.truncated, "exactly MAX_MATCHES: nothing was left out");
    }

    /// Today's `search_with`, kept as M1's reference (wave 2).
    fn reference_search(root: &Value, query: &str, cancelled: &dyn Fn() -> bool) -> Option<SearchResult> {
        let mut result = SearchResult::default();
        let needle = query.to_lowercase();
        if needle.is_empty() {
            return Some(result);
        }
        // Depth-first, children pushed in reverse so they pop in document order.
        let mut stack: Vec<(String, Option<&str>, &Value)> = vec![(String::new(), None, root)];
        let mut visited = 0usize;
        while let Some((pointer, key, value)) = stack.pop() {
            visited += 1;
            if visited.is_multiple_of(4096) && cancelled() {
                return None;
            }
            let key_hit = key.is_some_and(|k| k.to_lowercase().contains(&needle));
            let value_hit = scalar_text(value).is_some_and(|t| t.to_lowercase().contains(&needle));
            if key_hit || value_hit {
                if result.matches.len() == MAX_MATCHES {
                    result.truncated = true; // a match past the cap: there are more than it shows
                    break;
                }
                for (i, _) in pointer.match_indices('/') {
                    result.keep.insert(pointer[..i].to_string());
                }
                result.keep.insert(pointer.clone());
                result.hits.insert(pointer.clone());
                result.matches.push(pointer.clone());
            }
            let children: Vec<(String, Option<&str>, &Value)> = match value {
                Value::Object(map) => {
                    map.iter().map(|(k, v)| (child_pointer(&pointer, k), Some(k.as_str()), v)).collect()
                }
                Value::Array(items) => {
                    items.iter().enumerate().map(|(i, v)| (child_pointer(&pointer, &i.to_string()), None, v)).collect()
                }
                _ => Vec::new(),
            };
            stack.extend(children.into_iter().rev());
        }
        Some(result)
    }

    /// Wave 2 (M1): the search's whole result equals today's, for queries chosen to hit its edge cases (case,
    /// Unicode lowercasing, escaped keys, numbers, booleans, null, the root), at the match cap, and its cancel
    /// checks come at the same visits.
    #[test]
    fn search_matches_reference() {
        let root = json!({
            "Name": "Kelvin \u{212A} and \u{130}stanbul",
            "kayak": "plain ascii",
            "a/b": {"c~d": "slash and tilde", "\u{DF}": "strasse ss"},
            "items": [1, 2.5, -3e10, true, false, null, "Item 9", {"item 9": ["x"]}],
            "~": "/",
            "": "empty key",
            "nested": {"deep": {"deeper": ["zzz?", 12_345_678_901_234_567_890u64]}},
        });
        let queries = [
            "",
            "a",
            "A",
            "k",
            "\u{212A}",
            "i\u{307}",
            "\u{130}",
            "\u{DF}",
            "ss",
            "~",
            "/",
            "~1",
            "c~d",
            "2.5",
            "-3",
            "e10",
            "true",
            "fal",
            "null",
            "item 9",
            "ITEM",
            "zzz",
            "nomatch",
            "1234567890",
            "empty",
        ];
        for q in queries {
            assert_eq!(search_with(&root, q, &|| false), reference_search(&root, q, &|| false), "{q:?}");
        }
        assert_eq!(
            search_with(&json!("root text"), "TEXT", &|| false),
            reference_search(&json!("root text"), "TEXT", &|| false)
        );

        let many = Value::Array((0..10_005).map(|i| json!(format!("m{i}"))).collect());
        let new = search(&many, "m");
        assert!(new.truncated && new.matches.len() == MAX_MATCHES);
        assert_eq!(Some(new), reference_search(&many, "m", &|| false));
        type SearchFn = fn(&Value, &str, &dyn Fn() -> bool) -> Option<SearchResult>;
        let checks = |f: SearchFn, stop_at: usize| {
            let n = std::cell::Cell::new(0);
            let result = f(&many, "m", &|| {
                n.set(n.get() + 1);
                n.get() >= stop_at
            });
            (result, n.get())
        };
        assert_eq!(checks(search_with, usize::MAX), checks(reference_search, usize::MAX), "the same cancel checks");
        assert_eq!(checks(search_with, 2), (None, 2), "a cancel stops it");
        assert_eq!(checks(reference_search, 2), (None, 2));
    }

    /// Wave 2 (M1): a UI-thread search (just under `WORKER_SEARCH_NODES`) of 14 000 items of 7 nodes, with many
    /// matches and with none. Timing, not correctness, so ignored:
    /// `cargo test --release -j 8 -p manifold --bins json_search_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn json_search_cost() {
        use crate::bench::{report, timed};
        let root = Value::Array(
            (0..14_000)
                .map(|n| json!({"id": n, "name": format!("item {n}"), "score": n as f64 * 0.37, "tags": ["a", "b"]}))
                .collect(),
        );
        assert!(count_nodes(&root) < WORKER_SEARCH_NODES, "{} nodes: searched on the UI thread", count_nodes(&root));
        for q in ["item 9", "zzz"] {
            report(&format!("search {q:?}, 14 000 items"), timed(20, || drop(search(&root, q))));
        }
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
        assert_eq!(preview_size(&root), egui::vec2(PREVIEW_CARD_WIDTH, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT * 3.0));
        let wide = json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5});
        assert_eq!(preview_size(&wide).y, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT * 5.0); // 4 rows and "+ 1 more"
        assert_eq!(preview_size(&json!(7)).y, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT);
    }
}
