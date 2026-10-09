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
    /// The file-backed Json node `current_group` names, if it does.
    pub(crate) fn current_json_node(&self) -> Option<&JsonPayload> {
        let node = self.nodes.get(self.current_group.as_ref()?)?;
        node.as_json().filter(|json| !json.leaf)
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
            CopyWhat::Path if self.json_accessor_paths => json_view::accessor_path(&level.value, pointer),
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
    /// search's matches.
    pub(crate) fn json_level_state(&self) -> serde_json::Value {
        let Some(level) = &self.json_level else { return serde_json::Value::Null };
        let cards: Vec<serde_json::Value> = level
            .cards
            .iter()
            .zip(&level.rects)
            .map(|(card, rect)| {
                serde_json::json!({
                    "pointer": card.pointer,
                    "pos": [rect.min.x, rect.min.y],
                    "collapsed": card.collapsed,
                    "rows": card.rows,
                    "skip": card.skip,
                    "more": card.more,
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
mod tests {
    use super::*;
    use crate::store::model::Kind;

    // Real node ids: `snapshots()` skips any id that doesn't parse as
    // a `NodeId`, which would make the store test pass vacuously.
    const OUTER: &str = "0000000000000001-1";
    const J: &str = "0000000000000001-2";

    /// An App with a Json node `J` inside group `OUTER`, loaded from a temp
    /// file holding `text`. `name` keeps each test's file its own, since
    /// tests run in parallel in one process.
    fn app_with_json(name: &str, text: &str) -> (App, String) {
        let dir = crate::store::testutil::temp_dir(&format!("json_level-{name}"));
        let path = dir.join("level.json");
        std::fs::write(&path, text).unwrap();
        let mut app = App::default();
        app.place_group(Some(OUTER.into()), "outer", Placement { group: None, pos: egui::Vec2::ZERO });
        app.insert_json_node_with_id(
            J.into(),
            path.to_string_lossy().into_owned(),
            egui::Vec2::ZERO,
            Some(OUTER.into()),
        )
        .unwrap();
        (app, J.into())
    }

    /// Wave 2 (H1a): a search's hit rows are built with its result, per parent card, and go with it.
    #[test]
    fn a_search_builds_its_hit_rows_per_card() {
        let (mut app, j) = app_with_json("hit-rows", r#"{"a": {"b": 1}, "c": [1, 2]}"#);
        app.switch_group_level(Some(j));
        app.set_json_search("1".into());
        let rows = |app: &App| app.json_level.as_ref().unwrap().search.hit_rows.clone();
        assert_eq!(rows(&app), HashMap::from([("/a".to_string(), vec![0]), ("/c".to_string(), vec![0])]));
        app.set_json_search(String::new());
        assert!(rows(&app).is_empty());
    }

    /// Wave 2 (H1a): Tab and Shift+Tab walk the JSON cards through real frames, and a focused card panned out of view
    /// keeps its focus. Pins the focus reads before they move out of the per-card loop.
    #[test]
    fn tab_walks_the_json_cards_and_focus_survives_a_pan() {
        let (mut app, j) = app_with_json("tab-walk", r#"{"a": {"x": 1}, "b": {"y": 2}, "c": {"z": 3}}"#);
        app.switch_group_level(Some(j));
        let ctx = egui::Context::default();
        crate::theme::apply_fonts(&ctx);
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_graph_ui(ui));
            output.textures_delta.clear();
        };
        let tab = |shift: bool| egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: Some(egui::Key::Tab),
            pressed: true,
            repeat: false,
            modifiers: if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE },
        };
        let card = |app: &App, i: usize| {
            let level = app.json_level.as_ref().unwrap();
            egui::Id::new(("json_card", &level.node_id, &level.cards[i].pointer))
        };
        for _ in 0..3 {
            frame(&mut app, vec![]);
        }
        assert_eq!(app.json_level.as_ref().unwrap().cards.len(), 4, "root, a, b, c");
        ctx.memory_mut(|m| m.request_focus(card(&app, 0)));
        frame(&mut app, vec![]);
        let focused = || ctx.memory(|m| m.focused());
        assert_eq!(focused(), Some(card(&app, 0)));
        // An idle frame follows each key: egui applies a card's Tab lock filter only once it has held focus for a
        // frame (`set_focus_lock_filter` checks `had_focus_last_frame`), so a key in the very next frame would also
        // run egui's own Tab move and skip a card. That consecutive-frame skip is today's behavior, not pinned here.
        frame(&mut app, vec![tab(false)]);
        assert_eq!(focused(), Some(card(&app, 1)), "Tab: the next card");
        frame(&mut app, vec![]);
        assert_eq!(focused(), Some(card(&app, 1)), "an idle frame keeps it");
        frame(&mut app, vec![tab(false)]);
        assert_eq!(focused(), Some(card(&app, 2)));
        frame(&mut app, vec![]);
        frame(&mut app, vec![tab(true)]);
        assert_eq!(focused(), Some(card(&app, 1)), "Shift+Tab: the previous card");
        frame(&mut app, vec![]);
        assert_eq!(focused(), Some(card(&app, 1)));
        app.camera.jump_to(egui::Rect::from_min_size(egui::pos2(50_000.0, 50_000.0), egui::vec2(800.0, 600.0)));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        assert_eq!(focused(), Some(card(&app, 1)), "a focused card panned out of view keeps its focus");
    }

    #[test]
    fn entering_builds_the_level_and_leaving_drops_it() {
        let (mut app, j) = app_with_json("enter", r#"{"a": {"b": 1}, "c": [1, 2]}"#);
        app.switch_group_level(Some(j.clone()));
        let level = app.json_level.as_ref().expect("entering a Json node builds its level");
        assert_eq!(level.node_id, j);
        let ids: Vec<String> = app.level_items().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, [format!("{J}#"), format!("{J}#/a"), format!("{J}#/c")]);
        assert!(app.level_items().iter().all(|i| i.kind == LevelItemKind::JsonCard));
        assert_eq!(app.level_edges().len(), 2);
        assert!(app.level_edges().iter().all(|e| e.link.is_none()), "no JSON-level line is a link");
        app.switch_group_level(Some(OUTER.into()));
        assert!(app.json_level.is_none());
    }

    /// Links: a JSON level's cards are not stored nodes, so no node menu there offers the Links section, and entering
    /// one ends pick-target mode.
    #[test]
    fn a_json_level_hides_the_links_section_and_ends_pick_target_mode() {
        let (mut app, j) = app_with_json("links", r#"{"a": 1}"#);
        let note = app.alloc_node_id();
        app.insert_note_at(note.clone(), None);
        app.edit_group(&note, Some(OUTER.into()));
        app.switch_group_level(Some(OUTER.into()));
        assert!(!app.node_menu_view(&note).unwrap().edges.is_empty(), "the Json card is a target at OUTER's level");
        app.apply_node_action(&egui::Context::default(), &note, crate::NodeAction::Connect(crate::caps::LINK));
        app.switch_group_level(Some(j));
        assert!(app.connecting.is_none());
        assert!(app.node_menu_view(&note).unwrap().edges.is_empty());
    }

    /// Review C-B11: screen<->world agree with the outline open.
    #[test]
    fn screen_to_world_inverts_world_to_screen_with_the_outline_open() {
        let (mut app, j) = app_with_json("inverse", r#"{"a": 1}"#);
        app.switch_group_level(Some(j));
        app.json_outline_visible = true;
        app.json_outline_width = 300.0;
        let world = egui::Rect::from_min_size(egui::pos2(40.0, 60.0), egui::vec2(10.0, 10.0));
        let screen = app.graph_world_rect_to_screen(world);
        let back = app.graph_screen_point_to_world(screen.min);
        assert!((back - world.min.to_vec2()).length() < 0.01, "{back:?}");
    }

    #[test]
    fn placement_leaves_a_json_level_and_uses_the_outer_group() {
        let (mut app, j) = app_with_json("placement", "[1]");
        app.switch_group_level(Some(j));
        let at = app.placement("n", None);
        assert_eq!(at.group.as_deref(), Some(OUTER));
        assert_eq!(app.current_group.as_deref(), Some(OUTER), "the level was left");
        assert!(app.json_level.is_none());
        assert_eq!(at.pos, GRAPH_WORLD_CENTER + graph_layout::spawn_jitter("n"), "no pos: jittered spawn");
        let explicit = egui::vec2(3.0, 4.0);
        assert_eq!(app.placement("n", Some(explicit)).pos, explicit, "an explicit pos wins");
    }

    #[test]
    fn a_new_note_or_group_from_a_json_level_leaves_it_and_lands_in_the_outer_group() {
        let (mut app, j) = app_with_json("create_note_group", "[1]");
        for create in [App::new_note_shortcut, App::new_group_shortcut] {
            app.switch_group_level(Some(j.clone()));
            let before = app.node_order.len();
            create(&mut app);
            assert_eq!(app.node_order.len(), before + 1);
            assert!(app.json_level.is_none(), "the level was left");
            assert_eq!(app.mirror.group(app.node_order.last().unwrap()), Some(OUTER));
        }
    }

    #[test]
    fn a_new_tab_from_a_json_level_leaves_it_and_spawns_in_the_outer_group() {
        let (mut app, j) = app_with_json("new_tab", "[1]");
        app.switch_group_level(Some(j));
        app.new_tab_shortcut();
        assert!(app.json_level.is_none(), "the level was left");
        let (spawn, _) = app.test_spawns.last().expect("a tab spawn was issued");
        assert_eq!(spawn.group.as_deref(), Some(OUTER));
    }

    #[test]
    fn place_node_inserts_at_the_placement_and_returns_the_id() {
        let (mut app, _) = app_with_json("place_node", "[1]");
        let note = || Runtime::Note(NotePayload { text: String::new(), editing: false });
        let at = Placement { group: Some(OUTER.into()), pos: egui::vec2(1.0, 2.0) };
        let id = app.place_node(None, Kind::Note, note(), at);
        assert_eq!(
            (app.mirror.group(&id), app.layout_pos(&id), app.mirror.parent(&id).is_none()),
            (Some(OUTER), egui::vec2(1.0, 2.0), true)
        );
        assert_eq!(app.mirror.pos(&id), Some(egui::vec2(1.0, 2.0)), "the document pos is the placement too");
        assert_eq!(app.node_order.last(), Some(&id));
        let at = Placement { group: None, pos: egui::Vec2::ZERO };
        assert_eq!(app.place_node(Some("fixed".into()), Kind::Note, note(), at), "fixed", "a given id is kept");
    }

    #[test]
    fn new_nodes_land_beside_the_json_node_not_inside_it() {
        let (mut app, j) = app_with_json("insertion", "[1]");
        app.current_group = Some(j.clone()); // before any frame syncs the level
        assert_eq!(app.insertion_group(), Some(OUTER.to_string()));
        app.current_group = Some(OUTER.into());
        assert_eq!(app.insertion_group(), Some(OUTER.to_string()));
    }

    #[test]
    fn the_store_sees_nothing_of_the_level_but_view() {
        let (mut app, j) = app_with_json("store", r#"{"a": {"b": [1, 2, 3]}, "c": [1, 2]}"#);
        // (id, kind, group, fields without VIEW) for every stored node, as the mirror snapshot has it.
        let stored = |app: &App| {
            crate::session_sync::tests::snapshots(app)
                .into_iter()
                .map(|(id, mut s)| {
                    s.fields.remove(&field::VIEW);
                    (id, s.kind, s.group, s.fields)
                })
                .collect::<Vec<_>>()
        };
        let before = stored(&app);
        assert_eq!(before.len(), 2, "both nodes are stored, so the comparison means something");
        app.switch_group_level(Some(j.clone()));
        // Selecting first: selecting reveals, which would undo the collapse.
        app.apply_json_edit(JsonEdit::Select("/a/b".into()));
        app.apply_json_edit(JsonEdit::Toggle("/a".into()));
        app.apply_json_edit(JsonEdit::More("/c".into()));
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(500.0, 40.0)));
        app.apply_json_edit(JsonEdit::Relayout);
        assert_eq!(stored(&app), before);
        // The toggle is still there after Relayout; the drag is not.
        let view = app.mirror.view(&j).expect("a view");
        assert_eq!(view.get("/a").and_then(|o| o.collapsed), Some(true));
        assert_eq!(view.get("/a").and_then(|o| o.pos), None);
        app.switch_group_level(Some(OUTER.into()));
        assert_eq!(stored(&app), before);
    }

    #[test]
    fn a_drag_saves_only_that_cards_position() {
        let (mut app, j) = app_with_json("drag", r#"{"a": {"x": 1}, "b": {"y": 2}}"#);
        app.switch_group_level(Some(j.clone()));
        let b_before = app.json_level.as_ref().unwrap().rects[2];
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(700.0, 10.0)));
        let level = app.json_level.as_ref().unwrap();
        assert_eq!(level.rects[1].min, egui::pos2(700.0, 10.0));
        assert_eq!(level.rects[2], b_before);
    }

    #[test]
    fn rows_are_hit_by_their_world_position() {
        let (mut app, j) = app_with_json("rows", r#"{"a": 1, "b": {"c": 2}}"#);
        app.switch_group_level(Some(j));
        let level = app.json_level.as_ref().unwrap();
        let root = level.rects[0];
        let row = |i: f32| {
            egui::pos2(root.center().x, root.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (i + 0.5))
        };
        assert_eq!(level.row_at(0, row(0.0)), Some("/a".into()));
        assert_eq!(level.row_at(0, row(1.0)), Some("/b".into()));
        assert_eq!(
            level.row_at(0, egui::pos2(root.center().x, root.top() + 5.0)),
            None,
            "the header is the card, not a row"
        );
    }

    /// Wave 2 (H1b): a window past the first page puts `+ N earlier` under the header and every entry row one
    /// lower; hit-testing and `entry_rect` follow it.
    #[test]
    fn a_moved_window_is_hit_below_its_earlier_row() {
        let mut view = JsonView::default();
        view.entry("").first = Some(2);
        let level = JsonLevel::new(J.into(), Arc::new(serde_json::json!((0..100).collect::<Vec<_>>())), &view);
        let (card, rect) = (&level.cards[0], level.rects[0]);
        assert_eq!((card.skip, card.rows, card.more, card.lead()), (40, 20, 40, 1));
        let row = |i: usize| {
            egui::pos2(
                rect.center().x,
                rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * (i as f32 + 0.5),
            )
        };
        assert!(level.earlier_row_at(0, row(0)) && !level.earlier_row_at(0, row(1)));
        assert_eq!(level.row_at(0, row(0)), None, "the earlier row is not an entry");
        assert_eq!(level.row_at(0, row(1)).as_deref(), Some("/40"));
        assert_eq!(level.row_at(0, row(20)).as_deref(), Some("/59"));
        assert_eq!(level.row_at(0, row(21)), None, "the more row is not an entry");
        assert!(level.more_row_at(0, row(21)) && !level.more_row_at(0, row(20)));
        let top = |p: &str| level.entry_rect(p).map(|r| r.top());
        assert_eq!(top("/40"), Some(rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT));
        assert_eq!(top("/59"), Some(rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * 20.0));
        assert_eq!((top("/39"), top("/60")), (None, None), "outside the window");
    }

    #[test]
    fn copy_texts_follow_the_selection_and_the_path_form() {
        let (mut app, j) = app_with_json("copy", r#"{"users": [{"name": "Ada", "a/b": 1}]}"#);
        app.switch_group_level(Some(j));
        let copy = |app: &App, p: &str, what| app.json_copy_text(p, what);
        assert_eq!(copy(&app, "/users/0/name", CopyWhat::Value).as_deref(), Some("Ada"));
        assert_eq!(
            copy(&app, "/users/0", CopyWhat::Value).as_deref(),
            Some("{\n  \"name\": \"Ada\",\n  \"a/b\": 1\n}")
        );
        assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Key).as_deref(), Some("a/b"));
        assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Path).as_deref(), Some("/users/0/a~1b"));
        app.json_accessor_paths = true;
        assert_eq!(copy(&app, "/users/0/a~1b", CopyWhat::Path).as_deref(), Some(r#"users[0]["a/b"]"#));
        assert_eq!(copy(&app, "/nope", CopyWhat::Value), None);
    }

    #[test]
    fn the_topmost_card_under_a_point_is_hit() {
        let (mut app, j) = app_with_json("card_at", r#"{"a": {"x": 1}}"#);
        app.switch_group_level(Some(j));
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(0.0, 0.0))); // onto the root
        let level = app.json_level.as_ref().unwrap();
        assert_eq!(level.card_at(egui::pos2(5.0, 5.0)), Some(1), "later cards draw on top");
        assert_eq!(level.card_at(egui::pos2(-50.0, -50.0)), None);
    }

    #[test]
    fn a_remotely_deleted_json_node_leaves_its_level() {
        let (mut app, j) = app_with_json("remote_delete", r#"{"a": [1]}"#);
        app.switch_group_level(Some(j.clone()));
        // What `remote_changes` reports when another replica deletes J.
        app.apply_remote(vec![(j.parse().unwrap(), None)]);
        assert!(!app.nodes.contains_key(&j));
        assert_eq!(app.current_group.as_deref(), Some(OUTER), "the view leaves the removed level");
        assert!(app.json_level.is_none());
        assert_eq!(app.insertion_group().as_deref(), Some(OUTER), "new nodes land in a group that exists");
    }

    #[test]
    fn a_failed_refresh_leaves_the_level_for_the_error_card() {
        let dir = crate::store::testutil::temp_dir("json_level-failed_refresh");
        let mut app = App::default();
        app.place_group(Some(OUTER.into()), "outer", Placement { group: None, pos: egui::Vec2::ZERO });
        let body = crate::redirect_fetch::save(&dir, J, crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
        let source = ContentSource::Web { url: "https://x.test/j".into(), body };
        app.insert_content(source, J.into(), egui::Vec2::ZERO, Some(OUTER.into())).unwrap();
        app.switch_group_level(Some(J.into()));
        assert!(app.json_level.is_some());
        let bad = WebBody::File {
            kind: crate::content::ContentKind::Json,
            path: dir.join("missing.json").to_string_lossy().into_owned(),
        };
        app.reload_fetched_node(J, bad);
        assert!(app.nodes[J].as_json().unwrap().error.is_some(), "the refresh failed");
        // Still the Json node's own group before any frame syncs the level.
        assert_eq!(app.insertion_group().as_deref(), Some(OUTER));
        app.sync_json_level(); // the start of the next frame
        assert_eq!(app.current_group.as_deref(), Some(OUTER), "no level is built from the placeholder");
        assert!(app.json_level.is_none());
    }

    #[test]
    fn a_failed_reload_keeps_the_view() {
        let dir = crate::store::testutil::temp_dir("json_level-failed_reload_view");
        let mut app = App::default();
        let body =
            crate::redirect_fetch::save(&dir, J, crate::content::ContentKind::Json, br#"{"a": {"b": 1}}"#.to_vec())
                .unwrap();
        let source = ContentSource::Web { url: "https://x.test/j".into(), body };
        app.insert_content(source, J.into(), egui::Vec2::ZERO, None).unwrap();
        app.switch_group_level(Some(J.into()));
        app.apply_json_edit(JsonEdit::Toggle("/a".into()));
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(500.0, 40.0)));
        let before = app.mirror.view(J).expect("a view").to_field();
        assert_ne!(before, "", "the view has overrides to lose");
        let bad = WebBody::File {
            kind: crate::content::ContentKind::Json,
            path: dir.join("missing.json").to_string_lossy().into_owned(),
        };
        app.reload_fetched_node(J, bad);
        let json = app.nodes[J].as_json().unwrap();
        assert!(json.error.is_some(), "the reload failed");
        assert_eq!(app.mirror.view(J).expect("a view").to_field(), before, "the error card keeps the view for Retry");
    }

    /// Ruling 8: a reload removes the node and inserts it again, but the view lives in the mirror, which keeps it;
    /// only overrides the new value can't resolve are dropped.
    #[test]
    fn a_reload_keeps_the_view() {
        let dir = crate::store::testutil::temp_dir("json_level-reload_view");
        let mut app = App::default();
        let first = br#"{"a": {"b": 1}, "c": {"d": 2}}"#.to_vec();
        let body = crate::redirect_fetch::save(&dir, J, crate::content::ContentKind::Json, first).unwrap();
        let source = ContentSource::Web { url: "https://x.test/j".into(), body };
        app.insert_content(source, J.into(), egui::Vec2::ZERO, None).unwrap();
        app.switch_group_level(Some(J.into()));
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(500.0, 40.0)));
        app.apply_json_edit(JsonEdit::Moved("/c".into(), egui::pos2(900.0, 40.0)));
        let second = br#"{"a": {"b": 3}}"#.to_vec();
        let body = crate::redirect_fetch::save(&dir, J, crate::content::ContentKind::Json, second).unwrap();
        app.reload_fetched_node(J, body);
        assert!(app.nodes[J].as_json().unwrap().error.is_none(), "the reload worked");
        let view = app.mirror.view(J).expect("the view survived the reload");
        assert_eq!(view.get("/a").and_then(|o| o.pos), Some([500.0, 40.0]));
        assert!(view.get("/c").is_none(), "an override the new value can't resolve is dropped");
    }

    #[test]
    fn a_refetch_placeholder_is_not_entered_and_keeps_its_view() {
        let dir = crate::store::testutil::temp_dir("json_level-placeholder");
        let mut app = App::default();
        app.place_group(Some(OUTER.into()), "outer", Placement { group: None, pos: egui::Vec2::ZERO });
        // What `materialize` inserts for a missing file with a known URL, its view already the mirror's.
        let view = JsonView::from_field(r#"{"/a":{"collapsed":true}}"#);
        app.edit_view(J, &view.to_field());
        let payload = JsonPayload {
            path: dir.join("not-fetched-yet.json").to_string_lossy().into_owned(),
            value: Default::default(),
            leaf: false,
            error: None,
        };
        app.insert_placeholder_node(J.into(), egui::Vec2::ZERO, Kind::Json, Runtime::Json(payload));
        let url = crate::store::model::FieldValue::Str("https://x.test/j".into());
        app.mirror.set(J, crate::store::model::field::URL, url);
        app.edit_group(J, Some(OUTER.into()));
        assert!(!app.nodes[J].as_json().unwrap().enterable());
        app.switch_group_level(Some(J.into()));
        assert_eq!(app.current_group.as_deref(), Some(OUTER), "entering leaves for the placeholder's group");
        assert!(app.json_level.is_none());
        assert_eq!(app.mirror.view(J), Some(&view));
    }

    #[test]
    fn a_drop_from_a_json_level_leaves_it_and_lands_where_the_user_sees_it() {
        let (mut app, j) = app_with_json("create", r#"{"a": 1}"#);
        app.view_mode = ViewMode::Graph;
        app.switch_group_level(Some(j.clone()));
        // A drop leaves the level first, then lands where the user sees it.
        let file = crate::store::testutil::temp_dir("json_level-create-drop").join("dropped.txt");
        std::fs::write(&file, b"x").unwrap();
        app.drop_file(file, egui::pos2(300.0, 300.0));
        assert_eq!(app.current_group.as_deref(), Some(OUTER));
        assert!(app.json_level.is_none());
        let new: Vec<_> = app.nodes.iter().filter(|(_, n)| n.as_note().is_some()).map(|(id, _)| id).collect();
        assert_eq!(new.len(), 1, "the dropped text file is a note");
        assert_eq!(app.mirror.group(new[0]), Some(OUTER));
    }

    #[test]
    fn selecting_a_row_already_open_in_the_outline_does_not_rebuild_it() {
        let (mut app, j) = app_with_json("outline_rebuild", r#"{"a": {"b": 1}, "c": {"d": 2}}"#);
        app.switch_group_level(Some(j));
        app.poll_json_search();
        let rows = |app: &App| {
            app.json_level.as_ref().unwrap().outline.rows.iter().map(|r| r.pointer.clone()).collect::<Vec<_>>()
        };
        assert_eq!(rows(&app), ["", "/a", "/c"]);
        let built = app.json_level.as_ref().unwrap().outline.rows.as_ptr();
        app.apply_json_edit(JsonEdit::Select("/c".into()));
        let outline = &app.json_level.as_ref().unwrap().outline;
        assert!(!outline.dirty, "the root was already open");
        assert_eq!(outline.scroll_to.as_deref(), Some("/c"));
        app.poll_json_search();
        assert_eq!(app.json_level.as_ref().unwrap().outline.rows.as_ptr(), built, "the same rows, not rebuilt");
        // Selecting inside a closed container opens it: a rebuild.
        app.apply_json_edit(JsonEdit::Select("/a/b".into()));
        assert!(app.json_level.as_ref().unwrap().outline.dirty);
        app.poll_json_search();
        assert_eq!(rows(&app), ["", "/a", "/a/b", "/c"]);
    }

    #[test]
    fn selecting_a_hidden_entry_reveals_it_and_saves_the_reveal() {
        let users: Vec<_> = (0..45).map(|i| serde_json::json!({"name": format!("u{i}")})).collect();
        let (mut app, j) = app_with_json("reveal", &serde_json::json!({ "users": users }).to_string());
        app.switch_group_level(Some(j.clone()));
        app.apply_json_edit(JsonEdit::Toggle("/users".into()));
        app.apply_json_edit(JsonEdit::Select("/users/33/name".into()));
        let level = app.json_level.as_ref().unwrap();
        assert!(level.index.contains_key("/users/33"), "the entry's card is on the canvas");
        assert_eq!(level.selected.as_deref(), Some("/users/33/name"));
        for open in ["", "/users", "/users/33"] {
            assert!(level.outline.open.contains(open), "the outline opens {open:?}");
        }
        let view = app.mirror.view(&j).expect("a view");
        assert_eq!(view.pages("/users"), 2, "the reveal is saved like a manual one");
    }

    #[test]
    fn selecting_a_pointer_that_does_not_resolve_is_ignored() {
        let (mut app, j) = app_with_json("select_missing", r#"{"a": {"b": 1}}"#);
        app.switch_group_level(Some(j));
        app.apply_json_edit(JsonEdit::Select("/a/b".into()));
        app.apply_json_edit(JsonEdit::Select("/a/nope".into()));
        assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/a/b"));
    }

    #[test]
    fn selecting_pans_only_to_an_entry_out_of_view() {
        let users: Vec<_> = (0..45).map(|i| serde_json::json!({"name": format!("u{i}")})).collect();
        let (mut app, j) = app_with_json("pan", &serde_json::json!({ "a": 1, "users": users }).to_string());
        app.switch_group_level(Some(j));
        let level = app.json_level.as_ref().unwrap();
        let root = level.rects[level.index[""]];
        let row = level.entry_rect("/a").unwrap();
        assert!(root.contains_rect(row) && row.height() == json_view::ROW_HEIGHT, "a row on its parent's card");
        app.camera.jump_to(root.expand(10.0));
        app.apply_json_edit(JsonEdit::Select(String::new()));
        app.apply_json_edit(JsonEdit::Select("/a".into()));
        assert!(app.camera.tween.is_none(), "a visible entry leaves the camera where it was");
        app.apply_json_edit(JsonEdit::Select("/users/40/name".into()));
        let to = app.camera.tween.as_ref().expect("an off-screen entry pans").to;
        let target = app.json_level.as_ref().unwrap().entry_rect("/users/40/name").unwrap();
        assert!(to.contains_rect(target), "the camera heads to where the entry is in view");
    }

    #[test]
    fn keys_walk_the_outline_and_expand_cards() {
        let (mut app, j) = app_with_json("keys", r#"{"a": {"x": 1}, "b": 2}"#);
        app.switch_group_level(Some(j));
        let selected = |app: &App| app.json_level.as_ref().unwrap().selected.clone();
        app.step_json_selection(1);
        assert_eq!(selected(&app).as_deref(), Some(""), "nothing selected: the first row");
        app.step_json_selection(1);
        assert_eq!(selected(&app).as_deref(), Some("/a"));
        // /a is closed in the outline, so Down skips its child; Right opens it.
        app.open_json_selection(true);
        app.step_json_selection(1);
        assert_eq!(selected(&app).as_deref(), Some("/a/x"));
        app.step_json_selection(1);
        app.step_json_selection(1);
        assert_eq!(selected(&app).as_deref(), Some("/b"), "clamped at the last row");
        app.step_json_selection(-1);
        app.step_json_selection(-1);
        assert_eq!(selected(&app).as_deref(), Some("/a"));
        // Left closes the open row, then selects the parent.
        app.open_json_selection(false);
        assert_eq!(selected(&app).as_deref(), Some("/a"));
        app.open_json_selection(false);
        assert_eq!(selected(&app).as_deref(), Some(""));
        // Enter toggles the selected container's card.
        let collapsed = |app: &App| {
            json_view::is_collapsed(
                app.mirror.view(J).unwrap_or(&json_view::NO_VIEW),
                &app.json_level.as_ref().unwrap().expanded,
                "/a",
            )
        };
        let before = collapsed(&app);
        app.apply_json_edit(JsonEdit::Select("/a".into()));
        app.toggle_json_selection();
        assert_ne!(collapsed(&app), before);
    }

    #[test]
    fn search_steps_through_matches_and_filters_the_outline() {
        let (mut app, j) = app_with_json("search", r#"{"a": {"name": "x1"}, "b": {"name": "x2"}, "c": 3}"#);
        app.switch_group_level(Some(j));
        app.set_json_search("X".into());
        app.poll_json_search();
        let rows = |app: &App| {
            app.json_level.as_ref().unwrap().outline.rows.iter().map(|r| r.pointer.clone()).collect::<Vec<_>>()
        };
        assert_eq!(rows(&app), ["", "/a", "/a/name", "/b", "/b/name"]);
        app.step_json_match();
        assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/a/name"));
        app.step_json_match();
        assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/b/name"));
        app.step_json_match();
        assert_eq!(app.json_level.as_ref().unwrap().selected.as_deref(), Some("/a/name"), "wraps around");
        app.set_json_search(String::new());
        app.poll_json_search();
        // The two selections opened /a and /b in the outline.
        assert_eq!(rows(&app), ["", "/a", "/a/name", "/b", "/b/name", "/c"]);
    }

    #[test]
    fn a_big_value_is_searched_on_a_worker() {
        let big: Vec<u32> = (0..json_view::WORKER_SEARCH_NODES as u32 + 1).collect();
        let (mut app, j) = app_with_json("worker", &serde_json::to_string(&big).unwrap());
        app.switch_group_level(Some(j));
        app.set_json_search("99999".into());
        assert!(app.json_level.as_ref().unwrap().search.pending.is_some());
        for _ in 0..1000 {
            app.poll_json_search();
            if app.json_level.as_ref().unwrap().search.result.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let result = app.json_level.as_ref().unwrap().search.result.clone().expect("the worker answers");
        assert_eq!(result.matches, ["/99999"]);
    }

    /// Leaving the level marks its worker's search stale (`Drop for Search`), so `search_with` stops at its next
    /// check rather than walking the rest of a big value.
    #[test]
    fn leaving_the_level_stops_its_worker() {
        let items: Vec<String> = (0..json_view::WORKER_SEARCH_NODES + 10).map(|i| format!("v{i}")).collect();
        let (mut app, j) = app_with_json("leave", &serde_json::to_string(&items).unwrap());
        app.switch_group_level(Some(j));
        app.set_json_search("v".into());
        let level = app.json_level.as_ref().unwrap();
        let (value, generation) = (level.value.clone(), level.search.generation.clone());
        let started = generation.load(Ordering::Relaxed);
        app.json_level = None;
        let checks = std::cell::Cell::new(0);
        let stale = || {
            checks.set(checks.get() + 1);
            generation.load(Ordering::Relaxed) != started
        };
        assert!(json_view::search_with(&value, "v", &stale).is_none(), "a worker of the left level stops");
        assert_eq!(checks.get(), 1, "at its first check");
    }

    /// Review D-B6: while a worker searches, the previous query's result is gone.
    #[test]
    fn a_pending_search_hides_the_previous_result() {
        let items: Vec<String> = (0..json_view::WORKER_SEARCH_NODES + 10).map(|i| format!("v{i}")).collect();
        let (mut app, j) = app_with_json("pending", &serde_json::to_string(&items).unwrap());
        app.switch_group_level(Some(j));
        app.set_json_search("v1".into());
        // Let v1's worker answer first, so there is an old result to go stale.
        for _ in 0..1000 {
            app.poll_json_search();
            if app.json_level.as_ref().unwrap().search.result.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.json_level.as_ref().unwrap().search.result.is_some(), "v1's worker answers");
        app.set_json_search("v2".into());
        let level = app.json_level.as_ref().unwrap();
        assert!(
            level.search.pending.is_none() || level.search.result.is_none(),
            "no stale result beside a pending search"
        );
    }

    /// Escape in a text field (the search box) ends the edit only; egui drops the field's focus before any
    /// widget runs, so only `shortcuts::text_field_has_keys` still knows the key was the field's.
    #[test]
    fn escape_in_a_text_field_keeps_the_json_selection() {
        let (mut app, j) = app_with_json("escape", r#"{"a": 1}"#);
        app.switch_group_level(Some(j));
        app.apply_json_edit(JsonEdit::Select("/a".into()));
        let ctx = egui::Context::default();
        let field = egui::Id::new("json_search_field");
        let mut text = String::new();
        let mut frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 400.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                // Read first, as `build_graph_ui` does.
                let typing = crate::shortcuts::text_field_has_keys(ui.ctx());
                ui.add(egui::TextEdit::singleline(&mut text).id(field));
                app.handle_graph_keys(ui, typing);
            });
            output.textures_delta.clear();
        };
        ctx.memory_mut(|m| m.request_focus(field));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: Some(egui::Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![escape.clone()]);
        let selected = |app: &App| app.json_level.as_ref().unwrap().selected.clone();
        assert_eq!(selected(&app).as_deref(), Some("/a"), "Escape ended the edit only");
        frame(&mut app, vec![escape]);
        assert_eq!(selected(&app), None, "with no field focused, Escape clears the selection");
    }

    /// M5: a view in a format this build can't read (a newer build's) is never overwritten by an edit here, which
    /// would replace it on every replica.
    #[test]
    fn an_unreadable_view_is_never_overwritten() {
        use crate::store::model::{field, FieldValue};
        let (mut app, j) = app_with_json("opaque-view", r#"{"a": {"b": 1}, "c": [1, 2]}"#);
        app.edit_view(&j, "[1]");
        app.switch_group_level(Some(j.clone()));
        app.apply_json_edit(JsonEdit::Toggle("/a".into()));
        // A live drag moves the card's rect frame by frame (`json_level_ui`); the refused drop must put it back.
        let a = app.json_level.as_ref().unwrap().index["/a"];
        let laid = app.json_level.as_ref().unwrap().rects[a];
        let dropped = laid.translate(egui::vec2(300.0, 0.0));
        app.json_level.as_mut().unwrap().rects[a] = dropped;
        app.apply_json_edit(JsonEdit::Moved("/a".into(), dropped.min));
        assert_eq!(app.mirror.get(&j, field::VIEW), FieldValue::Str("[1]".into()));
        assert!(app.mirror.view(&j).is_some_and(|view| view.is_opaque()));
        assert_eq!(app.json_level.as_ref().unwrap().rects[a], laid, "the refused drag snaps back to the layout");
    }
}
