//! Maps today's kind enums to `(mime, tags)`. Plan 2 deletes the enums and this file with them.

use super::edges::CONTAINS;
use super::pattern::{Tag, TagSet};
use crate::node::ItemNodeKind;
use crate::session_sync::Fields;
use crate::store::model::{
    field, FieldValue, Kind, TYPE_DOWNLOAD, TYPE_GROUP, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB,
};

/// A stored node's type and tags, as `App::shared_nodes` sees it: a note with no (or only blank) text is `Blank`.
pub(crate) fn store_node(kind: Kind, fields: &Fields) -> (&'static str, TagSet) {
    let none = TagSet::EMPTY;
    match kind {
        Kind::Note => {
            let blank = !matches!(fields.get(&field::TEXT), Some(FieldValue::Str(text)) if !text.trim().is_empty());
            (TYPE_NOTE, if blank { TagSet::of(&[Tag::Blank]) } else { none })
        }
        kind => (kind.type_str().unwrap_or("application/x-manifold-unknown"), none),
    }
}

/// A card's type and tags. A loaded tab is `Loaded`; a sleeping one is the same type without it.
pub(crate) fn item_node(kind: ItemNodeKind) -> (&'static str, TagSet) {
    match kind {
        ItemNodeKind::Tab => (TYPE_TAB, TagSet::of(&[Tag::Loaded])),
        ItemNodeKind::Unloaded => (TYPE_TAB, TagSet::EMPTY),
        ItemNodeKind::Group => (TYPE_GROUP, TagSet::EMPTY),
        ItemNodeKind::Note => (TYPE_NOTE, TagSet::EMPTY),
        ItemNodeKind::Image => ("image/png", TagSet::EMPTY),
        ItemNodeKind::Pdf => (TYPE_PDF, TagSet::EMPTY),
        ItemNodeKind::Json => (TYPE_JSON, TagSet::EMPTY),
        ItemNodeKind::Download => (TYPE_DOWNLOAD, TagSet::EMPTY),
    }
}

/// Whether nodes can sit inside this kind of card (a group).
pub(crate) fn is_container(kind: ItemNodeKind) -> bool {
    super::edges().target_accepts(CONTAINS, item_node(kind).0)
}

/// `is_container` for a stored node's kind.
pub(crate) fn is_container_store(kind: Kind) -> bool {
    super::edges().target_accepts(CONTAINS, store_node(kind, &Fields::new()).0)
}

/// Whether a `member` card may be put inside a `group` card; argument order is (member, group), source then target.
pub(crate) fn can_contain(member: ItemNodeKind, group: ItemNodeKind) -> bool {
    super::edges().can_connect(CONTAINS, item_node(member).0, item_node(group).0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [ItemNodeKind; 8] = [
        ItemNodeKind::Tab,
        ItemNodeKind::Unloaded,
        ItemNodeKind::Download,
        ItemNodeKind::Group,
        ItemNodeKind::Note,
        ItemNodeKind::Image,
        ItemNodeKind::Pdf,
        ItemNodeKind::Json,
    ];

    #[test]
    fn only_a_group_card_is_a_container() {
        for kind in KINDS {
            assert_eq!(is_container(kind), kind == ItemNodeKind::Group, "{kind:?}");
        }
        for kind in [Kind::Tab, Kind::Note, Kind::Jar, Kind::JsonLeaf, Kind::Unknown] {
            assert!(!is_container_store(kind), "{kind:?}");
        }
        assert!(is_container_store(Kind::Group));
    }

    #[test]
    fn every_card_kind_can_go_in_a_group_and_nothing_goes_in_anything_else() {
        for member in KINDS {
            for onto in KINDS {
                assert_eq!(can_contain(member, onto), onto == ItemNodeKind::Group, "{member:?} into {onto:?}");
            }
        }
    }

    #[test]
    fn a_loaded_tab_is_tagged_and_a_sleeping_one_is_not() {
        assert_eq!(item_node(ItemNodeKind::Tab).1, TagSet::of(&[Tag::Loaded]));
        assert_eq!(item_node(ItemNodeKind::Unloaded).1, TagSet::EMPTY);
        assert_eq!(item_node(ItemNodeKind::Tab).0, item_node(ItemNodeKind::Unloaded).0);
    }
}
