//! The capability row table: which rows apply to a node, layered by rank (spec section 2).

use super::pattern::{Pattern, Tag, TagSet};

/// One row. A `None` field says nothing, so a lower-ranked row's value stands.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pattern: Pattern,
    requires: TagSet,
    restorable: Option<bool>,
    draw_layer: Option<u8>,
}

impl Row {
    pub(crate) fn new(pattern: &str) -> Row {
        Row { pattern: Pattern::parse(pattern), requires: TagSet::EMPTY, restorable: None, draw_layer: None }
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
}

/// How many draw layers the builtin rows use: every `Caps::draw_layer` is below this, so a level scan has this many
/// buckets.
pub(crate) const DRAW_LAYERS: usize = 8;

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
        }
        caps
    }
}

/// What today's `worth_reopening` decided per kind: only a completed download, a jar, a blank note and an unknown kind
/// are not recorded when closed.
pub(crate) fn builtin_rows() -> Vec<Row> {
    use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP, TYPE_JSON_LEAF, TYPE_NOTE, TYPE_TAB};
    vec![
        Row::new("*/*").restorable(false).draw_layer(0),
        Row::new(TYPE_TAB).restorable(true).draw_layer(1), // a sleeping tab; a loaded one draws under it
        // Outranks the row above for a loaded tab: same pattern, more `requires` tags (`Row::rank`).
        Row::new(TYPE_TAB).requires(&[Tag::Loaded]).draw_layer(0),
        Row::new(TYPE_GROUP).restorable(true).draw_layer(7),
        Row::new("image/*").restorable(true).draw_layer(4),
        Row::new("application/pdf").restorable(true).draw_layer(5),
        Row::new("application/json").restorable(true).draw_layer(6),
        Row::new(TYPE_JSON_LEAF).restorable(true).draw_layer(6),
        Row::new(TYPE_NOTE).restorable(true).draw_layer(3),
        Row::new(TYPE_NOTE).requires(&[Tag::Blank]).restorable(false),
        Row::new(TYPE_DOWNLOAD).draw_layer(2), // a download
    ]
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
            ("application/vnd.manifold.json-leaf", TagSet::EMPTY),
            ("text/markdown", TagSet::of(&[Tag::Blank])),
            ("application/octet-stream", TagSet::EMPTY),
            ("video/mp4", TagSet::EMPTY),
        ] {
            let want = forwards.caps(mime, tags);
            assert_eq!(backwards.caps(mime, tags), want, "{mime} {tags:?}: rank, not registration order");
            assert_eq!(forwards.caps(&mime.to_ascii_uppercase(), tags), want, "{mime}: case");
        }
        assert_eq!(forwards.caps("IMAGE/PNG", TagSet::EMPTY), Caps { restorable: true, draw_layer: 4 });
        // Equal rank keeps registration order in both tables: the later `application/vnd.a*` twin wins.
        let ranks: Vec<(u8, u32)> = forwards.rows.iter().map(Row::rank).collect();
        assert!(ranks.windows(2).all(|pair| pair[0] <= pair[1]), "{ranks:?}");
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
