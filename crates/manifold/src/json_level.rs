//! A Json node's level (spec "The JSON level"): its record cards laid out
//! as a tree, the selection, the outline's state and search. Present
//! exactly while `current_group` names a file-backed Json node; never in
//! `self.nodes`, so nothing here can reach the store but the node's own
//! `view` field.

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
    /// The match Enter or ↓ is on.
    pub(crate) current: usize,
    /// A worker's search of a big value, with the query it ran.
    pub(crate) pending: Option<(String, mpsc::Receiver<SearchResult>)>,
    /// Ctrl+F: focus the search box on the next draw.
    pub(crate) focus: bool,
}

/// Something the user did on a JSON level, applied once drawing is done.
pub(crate) enum JsonEdit {
    Select(String),
    Toggle(String),
    More(String),
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
    fn new(node_id: String, value: serde_json::Value, view: &JsonView) -> Self {
        let expanded = json_view::default_expanded(&value);
        let mut level = Self {
            node_id,
            nodes: json_view::count_nodes(&value),
            value: Arc::new(value),
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
        let pointer = &self.cards[card].pointer;
        let value = self.value.pointer(pointer)?;
        let (key, _) = json_view::entries(value).take(self.cards[card].rows).nth(row)?;
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
        if row >= self.cards[card].rows {
            return None;
        }
        let rect = self.rects[card];
        let top = rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * row as f32;
        Some(egui::Rect::from_min_size(egui::pos2(rect.left(), top), egui::vec2(rect.width(), json_view::ROW_HEIGHT)))
    }

    /// The topmost card under `world`: later cards draw on top.
    pub(crate) fn card_at(&self, world: egui::Pos2) -> Option<usize> {
        self.rects.iter().rposition(|rect| rect.contains(world))
    }

    /// Whether `world` is on card `card`'s `+ N more` row.
    pub(crate) fn more_row_at(&self, card: usize, world: egui::Pos2) -> bool {
        let (Some(rect), Some(c)) = (self.rects.get(card), self.cards.get(card)) else { return false };
        let top = rect.top() + json_view::HEADER_HEIGHT + json_view::ROW_HEIGHT * c.rows as f32;
        c.more > 0 && rect.contains(world) && world.y >= top
    }
}

impl App {
    /// The file-backed Json node `current_group` names, if it does.
    pub(crate) fn current_json_node(&self) -> Option<&JsonPayload> {
        let node = self.nodes.get(self.current_group.as_ref()?)?;
        node.as_json().filter(|json| json.key.is_none())
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
            let group = self.current_group.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.group.clone());
            self.switch_group_level(group);
            return;
        }
        // Looked up again through the field, not `current_json_node`, so
        // `self.json_level` can be borrowed mutably alongside it.
        let id = self.current_group.clone().unwrap_or_default();
        let Some(json) = self.nodes.get(&id).and_then(|n| n.as_json()) else { return };
        match &mut self.json_level {
            Some(level) if level.node_id == id => {
                if level.stale {
                    level.stale = false;
                    level.relayout(&json.view);
                }
            }
            _ => self.json_level = Some(JsonLevel::new(id, json.value.clone(), &json.view)),
        }
        // Here, not in the outline's build, so a worker's answer lands
        // while the outline is hidden (headless `JSONSEARCH`).
        self.receive_json_search();
    }

    /// Applies `edit` to the shown node's view, drops overrides that no
    /// longer resolve, relays the level out and saves (`VIEW` is diffed).
    /// Only while the node holds a parsed value: pruning against an error
    /// or refetch placeholder's `Null` would wipe the synced view.
    pub(crate) fn edit_json_view(&mut self, edit: impl FnOnce(&mut JsonView, &serde_json::Value, &HashSet<String>)) {
        let Some(level) = self.json_level.as_mut() else { return };
        let Some(NodePayload::Json(json)) = self.nodes.get_mut(&level.node_id).map(|n| &mut n.payload) else {
            return;
        };
        if !json.enterable() {
            return;
        }
        edit(&mut json.view, &level.value, &level.expanded);
        json.view.retain_resolvable(&level.value);
        level.relayout(&json.view);
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
        if query.is_empty() {
            level.search.result = None;
        } else if level.nodes > json_view::WORKER_SEARCH_NODES {
            let (tx, rx) = mpsc::channel();
            let (value, q) = (level.value.clone(), query.clone());
            std::thread::spawn(move || {
                // The level may be gone by now; a closed channel is fine.
                let _ = tx.send(json_view::search(&value, &q));
            });
            level.search.pending = Some((query.clone(), rx));
        } else {
            level.search.result = Some(json_view::search(&level.value, &query));
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
                        level.search.result = Some(result);
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

    /// Selecting from anywhere (spec "Outline"): the entry's card is
    /// revealed on the canvas (saved, like a manual expand), the camera
    /// pans to it, and the outline opens down to it.
    pub(crate) fn reveal_json(&mut self, pointer: &str) {
        // A dry run first, so selecting an already-visible entry doesn't save.
        let changed = self.json_level.as_ref().zip(self.current_json_node()).is_some_and(|(level, json)| {
            let mut view = json.view.clone();
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

    // Real node ids: `shared_nodes()` skips any id that doesn't parse as
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
        app.insert_placeholder_node(
            OUTER.into(),
            egui::Vec2::ZERO,
            NodePayload::Group(GroupPayload { name: "outer".into(), from_json: false }),
        );
        app.insert_json_node_with_id(
            J.into(),
            path.to_string_lossy().into_owned(),
            egui::Vec2::ZERO,
            Some(OUTER.into()),
        )
        .unwrap();
        (app, J.into())
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
        app.switch_group_level(Some(OUTER.into()));
        assert!(app.json_level.is_none());
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
            let made = &app.nodes[app.node_order.last().unwrap()];
            assert_eq!(made.group.as_deref(), Some(OUTER));
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
        let note = || NodePayload::Note(NotePayload { text: String::new(), editing: false, url: None });
        let at = Placement { group: Some(OUTER.into()), pos: egui::vec2(1.0, 2.0) };
        let id = app.place_node(None, note(), at);
        let node = &app.nodes[&id];
        assert_eq!((node.group.as_deref(), node.pos, node.parent.is_none()), (Some(OUTER), egui::vec2(1.0, 2.0), true));
        assert_eq!(app.node_order.last(), Some(&id));
        let at = Placement { group: None, pos: egui::Vec2::ZERO };
        assert_eq!(app.place_node(Some("fixed".into()), note(), at), "fixed", "a given id is kept");
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
        // (id, kind, group, fields without VIEW) for every stored node.
        let stored = |app: &App| {
            app.shared_nodes()
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
        let view = &app.nodes[&j].as_json().unwrap().view;
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
        app.insert_placeholder_node(
            OUTER.into(),
            egui::Vec2::ZERO,
            NodePayload::Group(GroupPayload { name: "outer".into(), from_json: false }),
        );
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
        let before = app.nodes[J].as_json().unwrap().view.to_field();
        assert_ne!(before, "", "the view has overrides to lose");
        let bad = WebBody::File {
            kind: crate::content::ContentKind::Json,
            path: dir.join("missing.json").to_string_lossy().into_owned(),
        };
        app.reload_fetched_node(J, bad);
        let json = app.nodes[J].as_json().unwrap();
        assert!(json.error.is_some(), "the reload failed");
        assert_eq!(json.view.to_field(), before, "the error card keeps the view for Retry");
    }

    #[test]
    fn a_refetch_placeholder_is_not_entered_and_keeps_its_view() {
        let dir = crate::store::testutil::temp_dir("json_level-placeholder");
        let mut app = App::default();
        app.insert_placeholder_node(
            OUTER.into(),
            egui::Vec2::ZERO,
            NodePayload::Group(GroupPayload { name: "outer".into(), from_json: false }),
        );
        // What `materialize` inserts for a missing file with a known URL.
        let view = JsonView::from_field(r#"{"/a":{"collapsed":true}}"#);
        let payload = JsonPayload {
            path: dir.join("not-fetched-yet.json").to_string_lossy().into_owned(),
            value: serde_json::Value::Null,
            key: None,
            error: None,
            url: Some("https://x.test/j".into()),
            view: view.clone(),
        };
        app.insert_placeholder_node(J.into(), egui::Vec2::ZERO, NodePayload::Json(payload));
        app.nodes.get_mut(J).unwrap().group = Some(OUTER.into());
        assert!(!app.nodes[J].as_json().unwrap().enterable());
        app.switch_group_level(Some(J.into()));
        assert_eq!(app.current_group.as_deref(), Some(OUTER), "entering leaves for the placeholder's group");
        assert!(app.json_level.is_none());
        assert_eq!(app.nodes[J].as_json().unwrap().view, view);
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
        let new: Vec<_> = app.nodes.values().filter(|n| n.as_note().is_some()).collect();
        assert_eq!(new.len(), 1, "the dropped text file is a note");
        assert_eq!(new[0].group.as_deref(), Some(OUTER));
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
        let view = &app.nodes[&j].as_json().unwrap().view;
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
}
