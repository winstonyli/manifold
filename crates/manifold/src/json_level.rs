//! A Json node's level (spec "The JSON level"): its record cards laid out
//! as a tree, the selection, the outline's state and search. Present
//! exactly while `current_group` names a file-backed Json node; never in
//! `self.nodes`, so nothing here can reach the store but the node's own
//! `view` field.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use super::*;
use crate::json_view::{self, Card, JsonView, OutlineRow, SearchResult};

pub(crate) struct JsonLevel {
    /// The Json node shown.
    pub(crate) node_id: String,
    /// The node's value when the level was built; a Refresh rebuilds it.
    pub(crate) value: Arc<serde_json::Value>,
    /// Nodes in `value`, for choosing a worker search.
    pub(crate) nodes: usize,
    /// `json_view::default_expanded`, fixed for this value.
    pub(crate) expanded: HashSet<String>,
    pub(crate) cards: Vec<Card>,
    /// World rects, one per card.
    pub(crate) rects: Vec<egui::Rect>,
    /// Card index by pointer.
    pub(crate) index: HashMap<String, usize>,
    /// The selected card's or row's pointer.
    pub(crate) selected: Option<String>,
    /// The view changed from elsewhere (another replica); relaid out on
    /// the next sync.
    pub(crate) stale: bool,
    pub(crate) outline: Outline,
    pub(crate) search: Search,
    /// A context menu: the pointer it acts on and where it opened (screen).
    pub(crate) menu: Option<(String, egui::Pos2)>,
}

/// The outline's own state (spec "Outline"): not saved, separate from
/// the canvas's collapse.
#[derive(Default)]
pub(crate) struct Outline {
    pub(crate) open: HashSet<String>,
    pub(crate) rows: Vec<OutlineRow>,
    /// `open` or the search changed; `rows` is rebuilt before the next draw.
    pub(crate) dirty: bool,
    /// Scroll this entry's row into view on the next draw; resolved to a
    /// row there, after any rebuild.
    pub(crate) scroll_to: Option<String>,
}

#[derive(Default)]
pub(crate) struct Search {
    pub(crate) query: String,
    pub(crate) result: Option<SearchResult>,
    /// The result's hits as rows (`json_view::hit_rows_by_parent`): sorted entry indices per parent card. Set with
    /// `result` (`set_result`), so a frame never scans a card's rows for hits.
    pub(crate) hit_rows: HashMap<String, Vec<usize>>,
    /// The match Enter or ↓ is on.
    pub(crate) current: usize,
    /// A worker's search of a big value, with the query it ran.
    pub(crate) pending: Option<(String, mpsc::Receiver<SearchResult>)>,
    /// Ctrl+F: focus the search box on the next draw.
    pub(crate) focus: bool,
    /// Bumped per query; a worker whose number is stale stops (search_with).
    pub(crate) generation: Arc<AtomicU64>,
}

impl Drop for Search {
    /// Leaving the level stops its worker too, not only a newer query.
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

impl Search {
    /// Sets the result and the hit rows built from it, together.
    pub(crate) fn set_result(&mut self, root: &serde_json::Value, result: Option<SearchResult>) {
        self.hit_rows = result.as_ref().map(|r| json_view::hit_rows_by_parent(root, &r.hits)).unwrap_or_default();
        self.result = result;
    }
}

/// Something the user did on a JSON level, applied once drawing is done.
pub(crate) enum JsonEdit {
    Select(String),
    Toggle(String),
    More(String),
    /// The `+ N earlier` row.
    Earlier(String),
    /// A dragged card's new top-left.
    Moved(String, egui::Pos2),
    Relayout,
    /// A card header's colour, `0x00RRGGBB` (plan ruling 4), or none.
    Color(String, Option<u32>),
}

/// What a copy takes from an entry (spec "Copy").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyWhat {
    Value,
    Key,
    Path,
}

impl JsonLevel {
    fn new(node_id: String, value: Arc<serde_json::Value>, view: &JsonView) -> Self {
        let expanded = json_view::default_expanded(&value);
        let mut level = Self {
            node_id,
            nodes: json_view::count_nodes(&value),
            value,
            expanded,
            cards: Vec::new(),
            rects: Vec::new(),
            index: HashMap::new(),
            selected: None,
            stale: false,
            outline: Outline { open: HashSet::from([String::new()]), dirty: true, ..Outline::default() },
            search: Search::default(),
            menu: None,
        };
        level.relayout(view);
        level
    }

    /// Recomputes the visible cards and their rects from `view`.
    pub(crate) fn relayout(&mut self, view: &JsonView) {
        self.cards = json_view::visible_cards(&self.value, view, &self.expanded);
        let positions = json_view::tree_layout(&self.cards, view);
        self.rects =
            self.cards.iter().zip(positions).map(|(card, pos)| egui::Rect::from_min_size(pos, card.size())).collect();
        self.index = self.cards.iter().enumerate().map(|(i, card)| (card.pointer.clone(), i)).collect();
    }

    /// A card's level-item id, `<json node id>#<pointer>` (spec "Model").
    pub(crate) fn card_id(&self, card: usize) -> String {
        format!("{}#{}", self.node_id, self.cards[card].pointer)
    }

    /// The pointer of the entry row under `world` on card `card`, or
    /// `None` on its header or its `+ N more` row.
    pub(crate) fn row_at(&self, card: usize, world: egui::Pos2) -> Option<String> {
        let rect = self.rects.get(card)?;
        let offset = world.y - rect.top() - json_view::HEADER_HEIGHT;
        if offset < 0.0 || !rect.contains(world) {
            return None;
        }
        let row = (offset / json_view::ROW_HEIGHT) as usize;
        let c = &self.cards[card];
        let row = row.checked_sub(c.lead())?; // the `+ N earlier` row (`?`, not `let … else`: clippy::question_mark)
        let pointer = &c.pointer;
        let value = self.value.pointer(pointer)?;
        let (key, _) = json_view::entries_from(value, c.skip).take(c.rows).nth(row)?;
        Some(json_view::child_pointer(pointer, &key))
    }

    /// Where the entry at `pointer` is drawn (world): its own card if it
    /// has one, else its row on its parent's card; `None` if neither shows.
    pub(crate) fn entry_rect(&self, pointer: &str) -> Option<egui::Rect> {
        if let Some(&i) = self.index.get(pointer) {
            return Some(self.rects[i]);
        }
        let parent = json_view::parent_pointer(pointer)?;
        let card = *self.index.get(parent)?;
        let row = json_view::entry_index(self.value.pointer(parent)?, &json_view::last_key(pointer)?)?;
        let c = &self.cards[card];
        if row < c.skip || row >= c.skip + c.rows {
            return None;
        }
        let rect = self.rects[card];
        let top = rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (row - c.skip + c.lead()) as f32;
        Some(egui::Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(rect.width(), json_view::ROW_HEIGHT)))
    }

    /// The topmost card under `world`: later cards draw on top.
    pub(crate) fn card_at(&self, world: egui::Pos2) -> Option<usize> {
        self.rects.iter().rposition(|rect| rect.contains(world))
    }

    /// Whether `world` is on card `card`'s `+ N more` row.
    pub(crate) fn more_row_at(&self, card: usize, world: egui::Pos2) -> bool {
        let (Some(rect), Some(c)) = (self.rects.get(card), self.cards.get(card)) else { return false };
        let top = rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (c.lead() + c.rows) as f32;
        c.more > 0 && rect.contains(world) && world.y >= top
    }

    /// Whether `world` is on card `card`'s `+ N earlier` row.
    pub(crate) fn earlier_row_at(&self, card: usize, world: egui::Pos2) -> bool {
        let (Some(rect), Some(c)) = (self.rects.get(card), self.cards.get(card)) else { return false };
        let top = rect.top() + json_view::HEADER_HEIGHT;
        c.skip > 0 && rect.contains(world) && world.y >= top && world.y < top + json_view::ROW_HEIGHT
    }
}

impl App {
    /// The Json node `current_group` names, if it does.
    pub(crate) fn current_json_node(&self) -> Option<&JsonPayload> {
        let node = self.nodes.get(self.current_group.as_ref()?)?;
        node.as_json()
    }

    /// Leaves a JSON level for the Json node's group, before placing a new
    /// node the user should see land (a drop, an added file, an import, a
    /// reopen): the level shows none of its group's nodes. Based on
    /// `current_group`, like `insertion_group`, so it holds before the
    /// first frame too.
    pub(crate) fn leave_json_level(&mut self) {
        if self.current_json_node().is_some() {
            self.switch_group_level(self.insertion_group());
        }
    }

    /// Makes `json_level` match `current_group`: built on entering a Json
    /// node, relaid out when marked stale, dropped on leaving. Runs at the
    /// start of `build_graph_ui` and in `switch_group_level`.
    pub(crate) fn sync_json_level(&mut self) {
        let Some(json) = self.current_json_node() else {
            self.json_level = None;
            return;
        };
        // A Json node whose file failed to load, or is still being
        // refetched, has no level (plan ruling 10, `JsonPayload::enterable`);
        // a failed Refresh while viewing it, or a restart after its file
        // moved, leaves to its group, where its card shows the error and
        // Retry. Its group is never a Json node, so this recurses once.
        if !json.enterable() {
            let group = self.group_of(&self.current_group);
            self.switch_group_level(group);
            return;
        }
        // Looked up again through the field, not `current_json_node`, so
        // `self.json_level` can be borrowed mutably alongside it.
        let id = self.current_group.clone().unwrap_or_default();
        let Some(json) = self.nodes.get(&id).and_then(|n| n.as_json()) else { return };
        let view = self.mirror.view(&id).unwrap_or(&json_view::NO_VIEW);
        match &mut self.json_level {
            Some(level) if level.node_id == id => {
                if level.stale {
                    level.stale = false;
                    level.relayout(view);
                }
            }
            _ => self.json_level = Some(JsonLevel::new(id, json.value.clone(), view)),
        }
        // Here, not in the outline's build, so a worker's answer lands
        // while the outline is hidden (headless `JSONSEARCH`).
        self.receive_json_search();
    }

    /// Applies `edit` to the shown node's view, drops overrides that no
    /// longer resolve, writes it (`edit_view`), relays the level out and
    /// saves. Only while the node holds a parsed value: pruning against an
    /// error or refetch placeholder's `Null` would wipe the synced view.
    pub(crate) fn edit_json_view(&mut self, edit: impl FnOnce(&mut JsonView, &serde_json::Value, &HashSet<String>)) {
        let Some(level) = self.json_level.as_ref() else { return };
        if !self.nodes.get(&level.node_id).and_then(|n| n.as_json()).is_some_and(|json| json.enterable()) {
            return;
        }
        let id = level.node_id.clone();
        let mut view = self.mirror.view(&id).cloned().unwrap_or_default();
        if view.is_opaque() {
            // M5: a newer build's format, written back untouched; an edit here would replace it on every replica.
            // Lay out from the unchanged view so a refused drag doesn't leave its card where it was dropped.
            if let Some(level) = self.json_level.as_mut() {
                level.relayout(&view);
            }
            self.queue_toast("This JSON view was saved by a newer manifold; changes to it aren't saved".into());
            return;
        }
        edit(&mut view, &level.value, &level.expanded);
        view.retain_resolvable(&level.value);
        self.edit_view(&id, &view.to_field());
        if let Some(level) = self.json_level.as_mut() {
            level.relayout(&view);
        }
        self.save_session();
    }

    pub(crate) fn apply_json_edit(&mut self, edit: JsonEdit) {
        match edit {
            JsonEdit::Select(pointer) => {
                // A pointer that doesn't resolve (a stale headless or
                // outline pointer) changes nothing.
                let Some(level) = self.json_level.as_mut().filter(|l| l.value.pointer(&pointer).is_some()) else {
                    return;
                };
                level.selected = Some(pointer.clone());
                self.reveal_json(&pointer);
            }
            JsonEdit::Toggle(pointer) => {
                self.edit_json_view(|view, _, expanded| json_view::toggle(view, expanded, &pointer))
            }
            JsonEdit::More(pointer) => self.edit_json_view(|view, _, _| json_view::show_more(view, &pointer)),
            JsonEdit::Earlier(pointer) => {
                self.edit_json_view(|view, value, _| json_view::show_earlier(value, view, &pointer))
            }
            JsonEdit::Moved(pointer, top_left) => {
                self.edit_json_view(|view, _, _| view.entry(&pointer).pos = Some([top_left.x, top_left.y]))
            }
            JsonEdit::Relayout => self.edit_json_view(|view, _, _| view.clear_positions()),
            JsonEdit::Color(pointer, color) => self.edit_json_view(|view, _, _| view.entry(&pointer).color = color),
        }
    }

    /// What a copy puts on the clipboard for the entry at `pointer`
    /// (spec "Copy"); `None` if it doesn't resolve.
    pub(crate) fn json_copy_text(&self, pointer: &str, what: CopyWhat) -> Option<String> {
        let level = self.json_level.as_ref()?;
        let value = level.value.pointer(pointer)?;
        Some(match what {
            CopyWhat::Value => json_view::copy_value_text(value),
            CopyWhat::Key => json_view::last_key(pointer).unwrap_or_default(),
            CopyWhat::Path if self.json_ui.json_accessor_paths => json_view::accessor_path(&level.value, pointer),
            CopyWhat::Path => pointer.to_string(),
        })
    }

    /// Runs a search (spec "Search"): on the UI thread for a small value,
    /// on a worker above `WORKER_SEARCH_NODES`. An empty query ends it.
    pub(crate) fn set_json_search(&mut self, query: String) {
        let Some(level) = self.json_level.as_mut() else { return };
        level.search.current = 0;
        level.search.pending = None;
        level.outline.dirty = true;
        let generation = level.search.generation.fetch_add(1, Ordering::Relaxed) + 1;
        level.search.set_result(&level.value, None); // the old query's matches mustn't show or step while this one runs
        if !query.is_empty() {
            if level.nodes > json_view::WORKER_SEARCH_NODES {
                let (tx, rx) = mpsc::channel();
                let (value, q, current) = (level.value.clone(), query.clone(), level.search.generation.clone());
                std::thread::spawn(move || {
                    let stale = || current.load(Ordering::Relaxed) != generation;
                    // A cancelled worker drops `tx`: `receive_json_search`
                    // clears `pending` on `Disconnected`.
                    if let Some(result) = json_view::search_with(&value, &q, &stale) {
                        let _ = tx.send(result); // the level may be gone; a closed channel is fine
                    }
                });
                level.search.pending = Some((query.clone(), rx));
            } else {
                let result = json_view::search(&level.value, &query);
                level.search.set_result(&level.value, Some(result));
            }
        }
        level.search.query = query;
        self.poll_json_search();
    }

    /// Takes a worker's answer if it has one, and rebuilds the outline's
    /// rows if they're stale. Runs each frame before the outline draws.
    pub(crate) fn poll_json_search(&mut self) {
        self.receive_json_search();
        let Some(level) = self.json_level.as_mut() else { return };
        if level.outline.dirty {
            let keep = level.search.result.as_ref().map(|r| &r.keep);
            level.outline.rows = json_view::outline_rows(&level.value, &level.outline.open, keep);
            level.outline.dirty = false;
        }
    }

    /// Takes a worker's answer if it has one, marking the outline stale.
    /// Cheap (a `try_recv`), so it runs every frame from `sync_json_level`.
    fn receive_json_search(&mut self) {
        let Some(level) = self.json_level.as_mut() else { return };
        if let Some((query, rx)) = &level.search.pending {
            match rx.try_recv() {
                Ok(result) => {
                    if *query == level.search.query {
                        level.search.set_result(&level.value, Some(result));
                    }
                    level.search.pending = None;
                    level.outline.dirty = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => level.search.pending = None,
            }
        }
    }

    /// Enter or ↓ in the search box: selects the next match, wrapping.
    pub(crate) fn step_json_match(&mut self) {
        let Some(level) = self.json_level.as_mut() else { return };
        let Some(result) = level.search.result.as_ref().filter(|r| !r.matches.is_empty()) else { return };
        let pointer = result.matches[level.search.current % result.matches.len()].clone();
        level.search.current = (level.search.current + 1) % result.matches.len();
        self.apply_json_edit(JsonEdit::Select(pointer));
    }

    /// Up/Down on a JSON level: selects the previous/next outline row (the first when nothing is
    /// selected), clamped at the ends. Selecting reveals and pans, as a click does.
    pub(crate) fn step_json_selection(&mut self, delta: isize) {
        self.poll_json_search();
        let Some(level) = self.json_level.as_ref() else { return };
        let rows = &level.outline.rows;
        let at = level.selected.as_ref().and_then(|s| rows.iter().position(|r| &r.pointer == s));
        let next = match at {
            Some(i) => i.saturating_add_signed(delta).min(rows.len().saturating_sub(1)),
            None => 0,
        };
        if let Some(row) = rows.get(next) {
            self.apply_json_edit(JsonEdit::Select(row.pointer.clone()));
        }
    }

    /// Enter on a JSON level: expands or collapses the selected container's card.
    pub(crate) fn toggle_json_selection(&mut self) {
        let Some(level) = self.json_level.as_ref() else { return };
        let Some(pointer) = level.selected.clone() else { return };
        if level.value.pointer(&pointer).is_some_and(|v| json_view::entry_count(v) > 0) {
            self.apply_json_edit(JsonEdit::Toggle(pointer));
        }
    }

    /// Right/Left on a JSON level: opens / closes the selected row in the outline; Left on a row
    /// that is already closed selects its parent.
    pub(crate) fn open_json_selection(&mut self, open: bool) {
        let Some(level) = self.json_level.as_mut() else { return };
        let Some(pointer) = level.selected.clone() else { return };
        let is_container = level.value.pointer(&pointer).is_some_and(|v| json_view::entry_count(v) > 0);
        if open {
            if is_container && level.outline.open.insert(pointer) {
                level.outline.dirty = true;
            }
        } else if level.outline.open.remove(&pointer) {
            level.outline.dirty = true;
        } else if let Some(parent) = json_view::parent_pointer(&pointer).map(str::to_string) {
            self.apply_json_edit(JsonEdit::Select(parent));
        }
    }

    /// Selecting from anywhere (spec "Outline"): the entry's card is
    /// revealed on the canvas (saved, like a manual expand), the camera
    /// pans to it, and the outline opens down to it.
    pub(crate) fn reveal_json(&mut self, pointer: &str) {
        // A dry run first, so selecting an already-visible entry doesn't save.
        let changed = self.json_level.as_ref().filter(|_| self.current_json_node().is_some()).is_some_and(|level| {
            let mut view = self.mirror.view(&level.node_id).cloned().unwrap_or_default();
            json_view::reveal(&level.value, &mut view, &level.expanded, pointer)
        });
        if changed {
            self.edit_json_view(|view, value, expanded| {
                json_view::reveal(value, view, expanded, pointer);
            });
        }
        let Some(level) = self.json_level.as_mut() else { return };
        // Pan only if the entry isn't already fully in view, judged where
        // the camera is headed, so clicking what's on screen never moves it.
        let view = self.camera.tween.as_ref().map_or(self.camera.rect, |t| t.to);
        if let Some(target) = level.entry_rect(pointer).filter(|r| !view.contains_rect(*r)) {
            self.camera.ease_to(egui::Rect::from_center_size(target.center(), view.size()));
        }
        // The rows are rebuilt only if this opened something (spec: "rebuilt
        // only when its expansion or the search changes"); a search shows
        // every kept row open regardless.
        let mut ancestor = String::new();
        let mut opened = level.outline.open.insert(String::new());
        for token in json_view::pointer_tokens(json_view::parent_pointer(pointer).unwrap_or("")) {
            ancestor = json_view::child_pointer(&ancestor, &token);
            opened |= level.outline.open.insert(ancestor.clone());
        }
        if opened && level.search.result.is_none() {
            level.outline.dirty = true;
        }
        level.outline.scroll_to = Some(pointer.to_string());
    }

    /// `EXPORTSTATE`'s `json_level` (spec "Headless"): the root id, the
    /// visible cards' pointers and positions, the selection and the
    /// search's matches. A card whose window has moved (`skip` > 0) also gives
    /// `earlier`, the screen point of its `+ N earlier` row (else null).
    pub(crate) fn json_level_state(&self) -> serde_json::Value {
        let Some(level) = &self.json_level else { return serde_json::Value::Null };
        let cards: Vec<serde_json::Value> = level
            .cards
            .iter()
            .zip(&level.rects)
            .map(|(card, rect)| {
                // Where a click lands on the `+ N earlier` row (window points), for a script to aim at.
                let earlier = (card.skip > 0).then(|| {
                    let row = egui::Rect::from_min_size(
                        egui::pos2(rect.left(), rect.top() + json_view::HEADER_HEIGHT),
                        egui::vec2(rect.width(), json_view::ROW_HEIGHT),
                    );
                    let c = self.graph_world_rect_to_screen(row).center();
                    [c.x, c.y]
                });
                serde_json::json!({
                    "pointer": card.pointer,
                    "pos": [rect.min.x, rect.min.y],
                    "collapsed": card.collapsed,
                    "rows": card.rows,
                    "skip": card.skip,
                    "more": card.more,
                    "earlier": earlier,
                })
            })
            .collect();
        serde_json::json!({
            "root": level.node_id,
            "cards": cards,
            "selected": level.selected,
            "matches": level.search.result.as_ref().map(|r| r.matches.clone()).unwrap_or_default(),
            "outline_rows": level.outline.rows.len(),
        })
    }
}

#[cfg(test)]
mod tests;
