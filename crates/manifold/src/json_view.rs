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

/// What a view's stored text held that this build can't read, kept so an edit here writes it back untouched
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

    /// Renames every override's pointer by `f` (`None`: the entry is gone and its override goes). Opaque text is left
    /// alone.
    pub(crate) fn remap(&mut self, f: &dyn Fn(&str) -> Option<String>) {
        if self.is_opaque() {
            return;
        }
        fn rekey<V>(map: &mut BTreeMap<String, V>, f: &dyn Fn(&str) -> Option<String>) {
            *map = std::mem::take(map).into_iter().filter_map(|(p, v)| Some((f(&p)?, v))).collect();
        }
        rekey(&mut self.0, f);
        rekey(&mut self.1.entries, f);
        rekey(&mut self.1.extra, f);
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

/// Whether a child gets its own card: non-empty containers only;
/// `{}` and `[]` show inline.
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
/// is always expanded. Depends on the value alone, so expanding one card never collapses another.
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
/// The algorithm is `tree_layout::tree_layout` (children come after their
/// parent: `visible_cards` is breadth-first).
pub(crate) fn tree_layout(cards: &[Card], view: &JsonView) -> Vec<egui::Pos2> {
    let nodes: Vec<crate::tree_layout::TreeNode> = cards
        .iter()
        .map(|card| crate::tree_layout::TreeNode { parent: card.parent.map(|(p, _)| p), size: card.size() })
        .collect();
    let mut pos = crate::tree_layout::tree_layout(&nodes, COLUMN_GAP, SIBLING_GAP);
    for (card, p) in cards.iter().zip(&mut pos) {
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
/// (not array indices), across the whole value.
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

/// The outline flattened: the root, then every entry of
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
mod tests;
