//! Maps today's kind enums to `(mime, tags)`. Plan 2 deletes the enums and this file with them.

use super::pattern::{Tag, TagSet};
use crate::session_sync::Fields;
use crate::store::model::{field, FieldValue, Kind};

pub(crate) const TAB: &str = "application/vnd.manifold.tab";
pub(crate) const GROUP: &str = "application/vnd.manifold.group";
pub(crate) const JSON_LEAF: &str = "application/vnd.manifold.json-leaf";
pub(crate) const TEXT_MARKDOWN: &str = "text/markdown";

/// A stored node's type and tags, as `App::shared_nodes` sees it: a note with no (or only blank) text is `Blank`.
pub(crate) fn store_node(kind: Kind, fields: &Fields) -> (&'static str, TagSet) {
    let none = TagSet::EMPTY;
    match kind {
        Kind::Tab => (TAB, none),
        Kind::Group => (GROUP, none),
        Kind::Note => {
            let blank = !matches!(fields.get(&field::TEXT), Some(FieldValue::Str(text)) if !text.trim().is_empty());
            (TEXT_MARKDOWN, if blank { TagSet::of(&[Tag::Blank]) } else { none })
        }
        Kind::Image => ("image/png", none),
        Kind::Pdf => ("application/pdf", none),
        Kind::Json => ("application/json", none),
        Kind::JsonLeaf => (JSON_LEAF, none),
        Kind::Download => ("application/octet-stream", none),
        Kind::Jar => ("application/vnd.manifold.jar", none),
        Kind::Unknown(_) => ("application/x-manifold-unknown", none),
    }
}
