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
    movable: Option<bool>,
    drop_target: Option<bool>,
    menu: Option<bool>,
    linkable: Option<bool>,
    stored_controls: Option<bool>,
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
            movable: None,
            drop_target: None,
            menu: None,
            linkable: None,
            stored_controls: None,
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

    pub(crate) fn movable(mut self, on: bool) -> Row {
        self.movable = Some(on);
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

    pub(crate) fn linkable(mut self, on: bool) -> Row {
        self.linkable = Some(on);
        self
    }

    pub(crate) fn stored_controls(mut self, on: bool) -> Row {
        self.stored_controls = Some(on);
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
    /// A drag repositions it and stores the position.
    pub(crate) movable: bool,
    /// A dragged card may land on it.
    pub(crate) drop_target: bool,
    /// Has a node menu.
    pub(crate) menu: bool,
    /// Shows link handles.
    pub(crate) linkable: bool,
    /// Swatch, Pin, Close and Eject act on a stored node.
    pub(crate) stored_controls: bool,
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
            if let Some(v) = row.movable {
                caps.movable = v;
            }
            if let Some(v) = row.drop_target {
                caps.drop_target = v;
            }
            if let Some(v) = row.menu {
                caps.menu = v;
            }
            if let Some(v) = row.linkable {
                caps.linkable = v;
            }
            if let Some(v) = row.stored_controls {
                caps.stored_controls = v;
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
        TYPE_DOWNLOAD, TYPE_GROUP, TYPE_JAR, TYPE_NOTE, TYPE_TAB, TYPE_UNKNOWN, TYPE_VIRTUAL_PREFIX,
    };
    vec![
        // Every stored kind can do everything; a virtual kind's own row, below, turns off what it cannot.
        Row::new("*/*")
            .selectable(true)
            .drag_source(true)
            .movable(true)
            .drop_target(true)
            .menu(true)
            .linkable(true)
            .stored_controls(true),
        // A virtual kind starts with nothing and draws on top (layer 8); a source's exact row grants what it needs.
        Row::new(&format!("{TYPE_VIRTUAL_PREFIX}*"))
            .selectable(false)
            .drag_source(false)
            .movable(false)
            .drop_target(false)
            .menu(false)
            .linkable(false)
            .stored_controls(false)
            .draw_layer(8),
        Row::new(TYPE_TAB).restorable(true).draw_layer(1), // a sleeping tab; a loaded one draws under it
        // Outranks the row above for a loaded tab: same pattern, more `requires` tags (`Row::rank`).
        Row::new(TYPE_TAB).requires(&[Tag::Loaded]).draw_layer(0),
        Row::new(TYPE_GROUP).restorable(true).draw_layer(7),
        Row::new("image/*").restorable(true).draw_layer(4),
        Row::new("application/pdf").restorable(true).draw_layer(5),
        Row::new("application/json").restorable(true).draw_layer(6),
        Row::new(TYPE_NOTE).restorable(true).draw_layer(3),
        Row::new(TYPE_JAR).restorable(true).draw_layer(3), // a jar card, drawn with notes
        Row::new(TYPE_UNKNOWN).restorable(true).draw_layer(3), // a type this build doesn't know, drawn with notes
        Row::new(TYPE_NOTE).requires(&[Tag::Blank]).restorable(false),
        Row::new(TYPE_DOWNLOAD).restorable(false).draw_layer(2), // a download
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
        fake("fake-all")
            .selectable(true)
            .drag_source(true)
            .movable(true)
            .drop_target(true)
            .menu(true)
            .linkable(true)
            .stored_controls(true),
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
                caps.selectable
                    && caps.drag_source
                    && caps.movable
                    && caps.drop_target
                    && caps.menu
                    && caps.linkable
                    && caps.stored_controls,
                "{ty}: {caps:?}"
            );
        }
    }

    #[test]
    fn a_virtual_kind_has_no_ability_until_its_own_row_grants_one() {
        let registry = registry(builtin_rows());
        let bare = registry.caps("application/vnd.manifold.virtual.test", TagSet::EMPTY);
        assert!(!bare.selectable && !bare.drag_source && !bare.movable && !bare.drop_target);
        assert!(!bare.menu && !bare.linkable && !bare.stored_controls);
        assert_eq!(bare.draw_layer, 8, "a virtual card draws above every stored kind");
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
