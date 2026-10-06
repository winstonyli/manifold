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
        self.rows.push(row);
        Ok(())
    }

    /// The rows matching `mime` whose required tags `tags` holds, applied from lowest rank to highest (equal rank:
    /// registration order), each overriding the fields it sets.
    pub(crate) fn caps(&self, mime: &str, tags: TagSet) -> Caps {
        let mut matching: Vec<&Row> =
            self.rows.iter().filter(|r| r.pattern.matches(mime) && tags.contains_all(r.requires)).collect();
        matching.sort_by_key(|r| r.rank()); // stable: ties keep registration order
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
