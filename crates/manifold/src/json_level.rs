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
// `open`, `dirty` and `scroll_to` are read once Task 8 draws the outline.
#[allow(dead_code)]
pub(crate) struct Outline {
    pub(crate) open: HashSet<String>,
    pub(crate) rows: Vec<OutlineRow>,
    /// `open` or the search changed; `rows` is rebuilt before the next draw.
    pub(crate) dirty: bool,
    /// Scroll this row into view on the next draw.
    pub(crate) scroll_to: Option<usize>,
}

#[derive(Default)]
pub(crate) struct Search {
    pub(crate) query: String,
    pub(crate) result: Option<SearchResult>,
    /// The match Enter or ↓ is on.
    pub(crate) current: usize,
    /// A worker's search of a big value, with the query it ran.
    // Read once Task 8 runs big searches on a worker.
    #[allow(dead_code)]
    pub(crate) pending: Option<(String, mpsc::Receiver<SearchResult>)>,
    /// Ctrl+F: focus the search box on the next draw.
    // Set by Ctrl+F (Task 7); read once Task 8 draws the search box.
    #[allow(dead_code)]
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

    /// Makes `json_level` match `current_group`: built on entering a Json
    /// node, relaid out when marked stale, dropped on leaving. Runs at the
    /// start of `build_graph_ui` and in `switch_group_level`.
    pub(crate) fn sync_json_level(&mut self) {
        let Some(json) = self.current_json_node() else {
            self.json_level = None;
            return;
        };
        // A Json node whose file failed to load has no level (plan ruling
        // 10); a failed Refresh while viewing it, or a restart after its
        // file moved, leaves to its group, where its card shows the error
        // and Retry. Its group is never a Json node, so this recurses once.
        if json.error.is_some() {
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
    }

    /// Applies `edit` to the shown node's view, drops overrides that no
    /// longer resolve, relays the level out and saves (`VIEW` is diffed).
    pub(crate) fn edit_json_view(&mut self, edit: impl FnOnce(&mut JsonView, &serde_json::Value, &HashSet<String>)) {
        let Some(level) = self.json_level.as_mut() else { return };
        let Some(NodePayload::Json(json)) = self.nodes.get_mut(&level.node_id).map(|n| &mut n.payload) else {
            return;
        };
        edit(&mut json.view, &level.value, &level.expanded);
        json.view.retain_resolvable(&level.value);
        level.relayout(&json.view);
        self.save_session();
    }

    pub(crate) fn apply_json_edit(&mut self, edit: JsonEdit) {
        match edit {
            JsonEdit::Select(pointer) => {
                if let Some(level) = self.json_level.as_mut() {
                    level.selected = Some(pointer);
                }
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

    /// Runs a search on the current JSON level (Task 8 adds the worker
    /// and the outline).
    pub(crate) fn set_json_search(&mut self, query: String) {
        if let Some(level) = self.json_level.as_mut() {
            level.search.result = (!query.is_empty()).then(|| json_view::search(&level.value, &query));
            level.search.query = query;
            level.search.current = 0;
        }
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
        app.apply_json_edit(JsonEdit::Toggle("/a".into()));
        app.apply_json_edit(JsonEdit::More("/c".into()));
        app.apply_json_edit(JsonEdit::Moved("/a".into(), egui::pos2(500.0, 40.0)));
        app.apply_json_edit(JsonEdit::Select("/a/b".into()));
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
    fn expanding_the_viewed_json_node_follows_it_to_its_group() {
        let (mut app, j) = app_with_json("expand", r#"{"a": 1}"#);
        app.switch_group_level(Some(j.clone()));
        app.expand_json_as_graph(&j);
        let root = app.current_group.clone().expect("the view is on a level");
        assert_ne!(root, j);
        assert!(app.nodes[&root].as_group().is_some_and(|g| g.from_json), "the view is the expanded root group");
        assert!(app.json_level.is_none());
    }
}
