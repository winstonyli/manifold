//! The capability row table: which rows apply to a node, layered by rank (spec section 2).

use super::pattern::{Pattern, Tag, TagSet};

/// One row. A `None` field says nothing, so a lower-ranked row's value stands.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pattern: Pattern,
    requires: TagSet,
    restorable: Option<bool>,
    draw_layer: Option<u8>,
    selectable: Option<bool>,
    drag_source: Option<bool>,
    drop_target: Option<bool>,
    menu: Option<bool>,
    stored_controls: Option<bool>,
    framed: Option<bool>,
    delete_verb: Option<&'static str>,
    click: Option<Click>,
    file: Option<bool>,
    menu_area: Option<&'static str>,
}

impl Row {
    pub(crate) fn new(pattern: &str) -> Row {
        Row {
            pattern: Pattern::parse(pattern),
            requires: TagSet::EMPTY,
            restorable: None,
            draw_layer: None,
            selectable: None,
            drag_source: None,
            drop_target: None,
            menu: None,
            stored_controls: None,
            framed: None,
            delete_verb: None,
            click: None,
            file: None,
            menu_area: None,
        }
    }

    pub(crate) fn selectable(mut self, on: bool) -> Row {
        self.selectable = Some(on);
        self
    }

    pub(crate) fn drag_source(mut self, on: bool) -> Row {
        self.drag_source = Some(on);
        self
    }

    pub(crate) fn drop_target(mut self, on: bool) -> Row {
        self.drop_target = Some(on);
        self
    }

    pub(crate) fn menu(mut self, on: bool) -> Row {
        self.menu = Some(on);
        self
    }

    pub(crate) fn stored_controls(mut self, on: bool) -> Row {
        self.stored_controls = Some(on);
        self
    }

    pub(crate) fn framed(mut self, on: bool) -> Row {
        self.framed = Some(on);
        self
    }

    pub(crate) fn delete_verb(mut self, verb: &'static str) -> Row {
        self.delete_verb = Some(verb);
        self
    }

    pub(crate) fn click(mut self, click: Click) -> Row {
        self.click = Some(click);
        self
    }

    pub(crate) fn file(mut self, on: bool) -> Row {
        self.file = Some(on);
        self
    }

    pub(crate) fn menu_area(mut self, area: &'static str) -> Row {
        self.menu_area = Some(area);
        self
    }

    pub(crate) fn requires(mut self, tags: &[Tag]) -> Row {
        self.requires = TagSet::of(tags);
        self
    }

    pub(crate) fn restorable(mut self, restorable: bool) -> Row {
        self.restorable = Some(restorable);
        self
    }

    pub(crate) fn draw_layer(mut self, layer: u8) -> Row {
        self.draw_layer = Some(layer);
        self
    }

    fn rank(&self) -> (u8, u32) {
        (self.pattern.rank(), self.requires.len())
    }
}

/// A node's merged capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Caps {
    /// Closing it is recorded, so Recently Closed can reopen it.
    pub(crate) restorable: bool,
    /// Where its card draws in Graph view: 0 first, so a higher layer draws, and takes the pointer, on top. Below
    /// `DRAW_LAYERS`.
    pub(crate) draw_layer: u8,
    /// May be in the rectangle selection.
    pub(crate) selectable: bool,
    /// May be picked up.
    pub(crate) drag_source: bool,
    /// A dragged card may land on it.
    pub(crate) drop_target: bool,
    /// Has a node menu.
    pub(crate) menu: bool,
    /// Pin, Close and Eject act on a stored node.
    pub(crate) stored_controls: bool,
    /// Counts toward the level-of-detail count (how many cards are on screen); the trail strip's pages, which are
    /// not cards in the graph, turn it off.
    pub(crate) framed: bool,
    /// What Del does to it, as the first word of its delete menu row ("close", "remove", "delete").
    pub(crate) delete_verb: &'static str,
    /// What a double-click (or Enter) does where it is a `NodeAction`; `None` leaves the click to the caller.
    pub(crate) click: Option<Click>,
    /// A file on disk: its card is matched by filename, and its menu is Refresh, Open as Tab, Copy URL, then the
    /// file rows.
    pub(crate) file: bool,
    /// The egui area id of its node menu.
    pub(crate) menu_area: &'static str,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps {
            restorable: false,
            draw_layer: 0,
            selectable: false,
            drag_source: false,
            drop_target: false,
            menu: false,
            stored_controls: false,
            framed: false,
            delete_verb: "delete",
            click: None,
            file: false,
            menu_area: "node_context_menu",
        }
    }
}

/// A card's double-click where it is one action (`card::click_action` maps it to a `NodeAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Click {
    /// Opens it into Tab or Item view.
    Open,
    /// Reveals its file.
    Reveal,
}

/// How many draw layers the builtin rows use: every `Caps::draw_layer` is below this, so a level scan has this many
/// buckets.
pub(crate) const DRAW_LAYERS: usize = 9;

#[derive(Debug, Default)]
pub(crate) struct Registry {
    /// Kept in rank order, lowest first, equal ranks in registration order (`register`), so `caps` is one fold.
    rows: Vec<Row>,
}

impl Registry {
    pub(crate) fn new() -> Registry {
        Registry::default()
    }

    /// Adds a row. The same pattern and required tags twice is an error.
    pub(crate) fn register(&mut self, row: Row) -> Result<(), String> {
        if self.rows.iter().any(|r| r.pattern == row.pattern && r.requires == row.requires) {
            return Err(format!("capability row {:?} with tags {:?} is already registered", row.pattern, row.requires));
        }
        // After every row of the same or a lower rank: a stable sort, done once here rather than per lookup.
        let at = self.rows.iter().rposition(|r| r.rank() <= row.rank()).map_or(0, |i| i + 1);
        self.rows.insert(at, row);
        Ok(())
    }

    /// The rows matching `mime` whose required tags `tags` holds, applied from lowest rank to highest (equal rank:
    /// registration order), each overriding the fields it sets. Per card per frame (`App::level_scan`): no
    /// allocation, the rows are already in rank order.
    pub(crate) fn caps(&self, mime: &str, tags: TagSet) -> Caps {
        let matching = self.rows.iter().filter(|r| tags.contains_all(r.requires) && r.pattern.matches(mime));
        let mut caps = Caps::default();
        for row in matching {
            if let Some(restorable) = row.restorable {
                caps.restorable = restorable;
            }
            if let Some(layer) = row.draw_layer {
                caps.draw_layer = layer;
            }
            if let Some(v) = row.selectable {
                caps.selectable = v;
            }
            if let Some(v) = row.drag_source {
                caps.drag_source = v;
            }
            if let Some(v) = row.drop_target {
                caps.drop_target = v;
            }
            if let Some(v) = row.menu {
                caps.menu = v;
            }
            if let Some(v) = row.framed {
                caps.framed = v;
            }
            if let Some(v) = row.stored_controls {
                caps.stored_controls = v;
            }
            if let Some(v) = row.delete_verb {
                caps.delete_verb = v;
            }
            if let Some(v) = row.click {
                caps.click = Some(v);
            }
            if let Some(v) = row.file {
                caps.file = v;
            }
            if let Some(v) = row.menu_area {
                caps.menu_area = v;
            }
        }
        caps
    }
}

/// What `worth_reopening` decides per type: only a completed download and a blank note are not recorded when closed
/// (a type this build doesn't know comes back as a shell, spec decision 4). Also the draw layer of each type
/// (`draw_layer`). A string no row matches gets the `Caps` default; `card_kind::canonical_type` maps every stored
/// type to a row first.
pub(crate) fn builtin_rows() -> Vec<Row> {
    use crate::store::model::{
        TYPE_DOWNLOAD, TYPE_GROUP, TYPE_JAR, TYPE_JSON_LEVEL, TYPE_NOTE, TYPE_TAB, TYPE_TRAIL_PAGE, TYPE_UNKNOWN,
        TYPE_VIRTUAL_PREFIX,
    };
    vec![
        // Every stored kind can do everything; a virtual kind's own row, below, turns off what it cannot.
        Row::new("*/*")
            .selectable(true)
            .drag_source(true)
            .drop_target(true)
            .menu(true)
            .stored_controls(true)
            .framed(true),
        // A virtual kind starts with nothing and draws on top (layer 8); a source's exact row grants what it needs.
        Row::new(&format!("{TYPE_VIRTUAL_PREFIX}*"))
            .selectable(false)
            .drag_source(false)
            .drop_target(false)
            .menu(false)
            .stored_controls(false)
            .draw_layer(8),
        Row::new(TYPE_TRAIL_PAGE).drag_source(true).menu(true).framed(false), // dragged out to open as a card; a long trail must not tip the level of detail
        // A JSON level's card (`draw_json_level` draws it and handles its input): the shared chrome leaves it alone.
        Row::new(TYPE_JSON_LEVEL)
            .selectable(false)
            .drag_source(false)
            .drop_target(false)
            .menu(false)
            .stored_controls(false),
        Row::new(TYPE_TAB).restorable(true).draw_layer(1).delete_verb("close").click(Click::Open), // a sleeping tab; a loaded one draws under it
        // Outranks the row above for a loaded tab: same pattern, more `requires` tags (`Row::rank`).
        Row::new(TYPE_TAB).requires(&[Tag::Loaded]).draw_layer(0),
        Row::new(TYPE_GROUP).restorable(true).draw_layer(7).delete_verb("close"),
        Row::new("image/*")
            .restorable(true)
            .draw_layer(4)
            .file(true)
            .click(Click::Open)
            .menu_area("image_context_menu"),
        Row::new("application/pdf")
            .restorable(true)
            .draw_layer(5)
            .file(true)
            .click(Click::Reveal)
            .menu_area("pdf_context_menu"),
        Row::new("application/json").restorable(true).draw_layer(6).file(true).menu_area("json_context_menu"),
        Row::new(TYPE_NOTE).restorable(true).draw_layer(3),
        Row::new(TYPE_JAR).restorable(true).draw_layer(3).delete_verb("close"), // a jar card, drawn with notes
        Row::new(TYPE_UNKNOWN).restorable(true).draw_layer(3), // a type this build doesn't know, drawn with notes
        Row::new(TYPE_NOTE).requires(&[Tag::Blank]).restorable(false),
        Row::new(TYPE_DOWNLOAD).restorable(false).draw_layer(2).delete_verb("remove").click(Click::Reveal), // a download
    ]
    .into_iter()
    .chain(test_rows())
    .collect()
}

/// The virtual-card tests' kinds (`virtual_source::fake`): `fake-none` is the prefix row alone, `fake-all` has every
/// ability, and each other grants one. `caps::registry()` is a `OnceLock`, so a test cannot add a row itself.
#[cfg(test)]
fn test_rows() -> [Row; 5] {
    let fake = |name: &str| Row::new(&format!("{}{name}", crate::store::model::TYPE_VIRTUAL_PREFIX));
    [
        fake("fake-all").selectable(true).drag_source(true).drop_target(true).menu(true).stored_controls(true),
        fake("fake-selectable").selectable(true),
        fake("fake-drag").drag_source(true),
        fake("fake-drop").drop_target(true),
        fake("fake-menu").menu(true),
    ]
}

#[cfg(not(test))]
fn test_rows() -> [Row; 0] {
    []
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(rows: Vec<Row>) -> Registry {
        let mut registry = Registry::new();
        for row in rows {
            registry.register(row).unwrap();
        }
        registry
    }

    #[test]
    fn every_builtin_draw_layer_fits_the_bucket_count() {
        for row in builtin_rows() {
            assert!(row.draw_layer.is_none_or(|layer| usize::from(layer) < DRAW_LAYERS), "{row:?}");
        }
    }

    #[test]
    fn every_builtin_stored_kind_has_every_ability() {
        use crate::store::model::{
            TYPE_DOWNLOAD, TYPE_GROUP, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB, TYPE_UNKNOWN,
        };
        let registry = registry(builtin_rows());
        for ty in
            [TYPE_TAB, TYPE_GROUP, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_DOWNLOAD, TYPE_UNKNOWN, "image/png"]
        {
            let caps = registry.caps(ty, TagSet::EMPTY);
            assert!(
                caps.selectable && caps.drag_source && caps.drop_target && caps.menu && caps.stored_controls,
                "{ty}: {caps:?}"
            );
        }
    }

    #[test]
    fn a_virtual_kind_has_no_ability_until_its_own_row_grants_one() {
        let registry = registry(builtin_rows());
        let bare = registry.caps("application/vnd.manifold.virtual.test", TagSet::EMPTY);
        assert!(!bare.selectable && !bare.drag_source && !bare.drop_target);
        assert!(!bare.menu && !bare.stored_controls);
        assert_eq!(bare.draw_layer, 8, "a virtual card draws above every stored kind");
    }

    #[test]
    fn a_trail_page_can_be_dragged_and_has_a_menu_and_nothing_else() {
        let caps = registry(builtin_rows()).caps(crate::store::model::TYPE_TRAIL_PAGE, TagSet::EMPTY);
        assert!(caps.drag_source && caps.menu);
        assert!(!caps.selectable && !caps.drop_target && !caps.stored_controls);
        assert_eq!(caps.draw_layer, 8);
    }

    #[test]
    fn nothing_matching_gives_the_defaults() {
        assert!(!Registry::new().caps("text/markdown", TagSet::EMPTY).restorable);
    }

    #[test]
    fn a_higher_pattern_overrides_a_lower_one_per_field() {
        let r = registry(vec![
            Row::new("*/*").restorable(false),
            Row::new("image/*").restorable(true),
            Row::new("image/gif").restorable(false),
        ]);
        assert!(r.caps("image/png", TagSet::EMPTY).restorable);
        assert!(!r.caps("image/gif", TagSet::EMPTY).restorable);
        assert!(!r.caps("video/mp4", TagSet::EMPTY).restorable);
    }

    #[test]
    fn registration_order_does_not_matter_across_ranks() {
        let high_first = registry(vec![Row::new("image/gif").restorable(true), Row::new("*/*").restorable(false)]);
        assert!(high_first.caps("image/gif", TagSet::EMPTY).restorable);
    }

    #[test]
    fn the_same_pattern_and_tags_twice_is_an_error() {
        let mut r = Registry::new();
        r.register(Row::new("image/*")).unwrap();
        assert!(r.register(Row::new("IMAGE/*")).is_err());
        r.register(Row::new("image/*").requires(&[Tag::Blank])).unwrap();
    }

    #[test]
    fn at_equal_rank_the_later_registration_wins() {
        // Both are subtype-prefix patterns (rank 3) and both match `application/vnd.abc`.
        let r = registry(vec![
            Row::new("application/vnd.a*").restorable(true),
            Row::new("application/vnd.ab*").restorable(false),
        ]);
        assert!(!r.caps("application/vnd.abc", TagSet::EMPTY).restorable);
    }

    /// Rows registered in any order layer by rank, equal ranks by registration order, and a mixed-case lookup
    /// reads the same caps as a lower-case one: the builtin table, registered forwards and backwards.
    #[test]
    fn layering_is_the_same_whatever_the_registration_order_and_the_input_case() {
        let forwards = registry(builtin_rows());
        let mut reversed = builtin_rows();
        reversed.reverse();
        let backwards = registry(reversed);
        let loaded = TagSet::of(&[Tag::Loaded]);
        for (mime, tags) in [
            ("application/vnd.manifold.tab", TagSet::EMPTY),
            ("application/vnd.manifold.tab", loaded),
            ("image/png", TagSet::EMPTY),
            ("image/png", loaded),
            ("application/json", TagSet::EMPTY),
            ("text/markdown", TagSet::of(&[Tag::Blank])),
            ("application/octet-stream", TagSet::EMPTY),
            ("video/mp4", TagSet::EMPTY),
        ] {
            let want = forwards.caps(mime, tags);
            assert_eq!(backwards.caps(mime, tags), want, "{mime} {tags:?}: rank, not registration order");
            assert_eq!(forwards.caps(&mime.to_ascii_uppercase(), tags), want, "{mime}: case");
        }
        let png = forwards.caps("IMAGE/PNG", TagSet::EMPTY);
        assert_eq!((png.restorable, png.draw_layer), (true, 4));
        // Rows are in rank order after registration.
        let ranks: Vec<(u8, u32)> = forwards.rows.iter().map(Row::rank).collect();
        assert!(ranks.windows(2).all(|pair| pair[0] <= pair[1]), "{ranks:?}");
        // Equal rank keeps registration order: a row registered after an equal-rank twin is applied after it, an
        // out-of-order insert included.
        let mut r = registry(vec![Row::new("*/*"), Row::new("application/vnd.ab*").restorable(false)]);
        r.register(Row::new("application/vnd.a*").restorable(true)).unwrap();
        assert!(r.caps("application/vnd.abc", TagSet::EMPTY).restorable, "registered last, applied last");
    }

    #[test]
    fn a_required_tag_makes_a_row_apply_only_with_it_and_outrank_its_twin() {
        let r = registry(vec![
            Row::new("text/markdown").restorable(true),
            Row::new("text/markdown").requires(&[Tag::Blank]).restorable(false),
        ]);
        assert!(r.caps("text/markdown", TagSet::EMPTY).restorable);
        assert!(!r.caps("text/markdown", TagSet::of(&[Tag::Blank])).restorable);
    }

    #[test]
    fn a_tag_does_not_outrank_a_more_specific_pattern() {
        let r = registry(vec![
            Row::new("image/*").requires(&[Tag::Loaded]).restorable(false),
            Row::new("image/png").restorable(true),
        ]);
        assert!(r.caps("image/png", TagSet::of(&[Tag::Loaded])).restorable);
    }
}
