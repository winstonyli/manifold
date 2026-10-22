//! `CardId`: the 16-byte `Copy` key of a card on the canvas. A stored node's card is its `NodeId` as is; every other
//! card (a `VirtualSource`'s, a JSON level's) is a source tag plus a 63-bit key chosen by that source. The two never
//! collide: a node's `counter` counts up from zero per launch, so its top bit is clear, and a non-node id sets it.

use std::fmt;

use crate::store::model::{ActorId, NodeId};

/// The bit of `lo` that marks a non-node card.
const NON_NODE: u64 = 1 << 63;

/// The name JSON-level cards use as their source (`json_level.rs`); no `VirtualSource` may take it.
pub(crate) const JSON_SOURCE: &str = "json";

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub(crate) struct CardId {
    hi: u64,
    lo: u64,
}

/// FNV-1a over `parts`, with a 0xFF byte (never in UTF-8) after each so `["a", "bc"]` and `["ab", "c"]` differ.
pub(crate) const fn fnv64(parts: &[&str]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut p = 0;
    while p < parts.len() {
        let bytes = parts[p].as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            hash = (hash ^ bytes[i] as u64).wrapping_mul(0x0000_0100_0000_01b3);
            i += 1;
        }
        hash = (hash ^ 0xFF).wrapping_mul(0x0000_0100_0000_01b3);
        p += 1;
    }
    hash
}

impl CardId {
    /// A non-node card of `source` (a `VirtualSource::name`, or [`JSON_SOURCE`]) with the source's own `key`, whose
    /// top bit is dropped.
    pub(crate) const fn virt(source: &str, key: u64) -> CardId {
        CardId { hi: fnv64(&[source]), lo: key & !NON_NODE | NON_NODE }
    }

    /// The stored node this card shows; `None` for a non-node card.
    /// The card of a stored node named by its id text. The stores key by text until the mirror is re-keyed
    /// (plan Task 5), so this is the one parse on the way from a store key to a card.
    pub(crate) fn stored(id: &str) -> CardId {
        id.parse::<NodeId>().map(CardId::from).unwrap_or_else(|e| panic!("a stored node id is a NodeId: {id:?}: {e}"))
    }

    /// The egui widget id of this card's frame: what keyboard focus and hit tests name.
    pub(crate) fn widget(self) -> egui::Id {
        egui::Id::new(("graph_card", self))
    }

    /// The egui widget id of a note card's text editor.
    pub(crate) fn editor(self) -> egui::Id {
        egui::Id::new(("note_edit", self))
    }

    pub(crate) const fn node(self) -> Option<NodeId> {
        if self.lo & NON_NODE != 0 {
            None
        } else {
            Some(NodeId { actor: ActorId(self.hi), counter: self.lo })
        }
    }

    pub(crate) const fn is_virtual(self) -> bool {
        self.lo & NON_NODE != 0
    }

    /// The hash of the source's name, for a non-node card.
    pub(crate) const fn source_tag(self) -> Option<u64> {
        if self.is_virtual() {
            Some(self.hi)
        } else {
            None
        }
    }

    /// The source's own key, for a non-node card.
    pub(crate) const fn key(self) -> Option<u64> {
        if self.is_virtual() {
            Some(self.lo & !NON_NODE)
        } else {
            None
        }
    }
}

impl From<NodeId> for CardId {
    fn from(node: NodeId) -> CardId {
        debug_assert!(node.counter & NON_NODE == 0, "a node counter reached 2^63: {node}");
        CardId { hi: node.actor.0, lo: node.counter }
    }
}

impl fmt::Display for CardId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node() {
            Some(node) => node.fmt(f),
            None => write!(f, "virtual:{:016x}:{:x}", self.hi, self.lo & !NON_NODE),
        }
    }
}

/// Debug exports print a card id as its text, which `FromStr` reads back.
impl serde::Serialize for CardId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl std::str::FromStr for CardId {
    type Err = String;

    /// Parses exactly what `Display` prints.
    fn from_str(s: &str) -> Result<CardId, String> {
        let bad = || format!("not a card id: {s:?}");
        let Some(rest) = s.strip_prefix("virtual:") else {
            return s.parse::<NodeId>().map(CardId::from);
        };
        let (tag, key) = rest.split_once(':').ok_or_else(bad)?;
        let hex = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_hexdigit());
        if tag.len() != 16 || !hex(tag) || !hex(key) {
            return Err(bad());
        }
        let key = u64::from_str_radix(key, 16).map_err(|_| bad())?;
        if key & NON_NODE != 0 {
            return Err(bad());
        }
        Ok(CardId { hi: u64::from_str_radix(tag, 16).map_err(|_| bad())?, lo: key | NON_NODE })
    }
}

/// Panics when two sources share a tag, or one takes [`JSON_SOURCE`]'s name. Sources are fixed at construction, so
/// only tests register them (`App::register_virtual_source`) and check the default list.
#[cfg(test)]
pub(crate) fn assert_distinct_tags(names: &[&str]) {
    for (i, a) in names.iter().enumerate() {
        assert!(*a != JSON_SOURCE, "{JSON_SOURCE:?} is reserved for JSON-level cards");
        for b in &names[i + 1..] {
            assert!(fnv64(&[a]) != fnv64(&[b]), "virtual sources {a:?} and {b:?} share a tag");
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// The `n`th node of a fixed test actor.
    pub(crate) fn node(n: u64) -> NodeId {
        NodeId { actor: ActorId(1), counter: n }
    }

    pub(crate) fn card(n: u64) -> CardId {
        node(n).into()
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{card, node};
    use super::*;

    #[test]
    fn a_node_id_round_trips_through_card_id_and_text() {
        let n = NodeId { actor: ActorId(0xdead_beef_0000_0001), counter: 42 };
        let id = CardId::from(n);
        assert_eq!(id.node(), Some(n));
        assert!(!id.is_virtual());
        assert_eq!(id.to_string(), n.to_string());
        assert_eq!(id.to_string().parse::<CardId>(), Ok(id));
    }

    #[test]
    fn a_virtual_id_is_not_a_node_and_round_trips() {
        let id = CardId::virt("trail", 7);
        assert_eq!(id.node(), None);
        assert!(id.is_virtual());
        assert_eq!(id.key(), Some(7));
        assert_eq!(id.source_tag(), Some(fnv64(&["trail"])));
        assert_eq!(id.to_string().parse::<CardId>(), Ok(id));
        assert!(id.to_string().starts_with("virtual:"));
    }

    #[test]
    fn a_virtual_key_loses_only_its_top_bit() {
        let id = CardId::virt("x", u64::MAX);
        assert_eq!(id.key(), Some(u64::MAX >> 1));
        assert_eq!(id.to_string().parse::<CardId>(), Ok(id));
    }

    #[test]
    fn two_sources_and_two_keys_give_four_distinct_ids() {
        let ids = [CardId::virt("a", 1), CardId::virt("a", 2), CardId::virt("b", 1), CardId::virt("b", 2)];
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn a_virtual_id_never_equals_a_node_id() {
        assert_ne!(CardId::virt("a", 1), card(1));
        assert_ne!(CardId::virt("a", 0), CardId::from(node(0)));
    }

    #[test]
    fn a_card_id_is_sixteen_bytes() {
        assert_eq!(size_of::<CardId>(), 16);
    }

    #[test]
    fn fnv64_separates_its_parts() {
        assert_ne!(fnv64(&["a", "bc"]), fnv64(&["ab", "c"]));
        assert_ne!(fnv64(&["a", "b"]), fnv64(&["ab"]));
    }

    #[test]
    fn text_that_is_no_card_id_is_rejected() {
        for s in ["", "n", "virtual:", "virtual:1:2", "virtual:0123456789abcdef", "virtual:0123456789abcdef:zz"] {
            assert!(s.parse::<CardId>().is_err(), "{s:?}");
        }
        // A key with the non-node bit set is not what Display prints.
        assert!("virtual:0123456789abcdef:ffffffffffffffff".parse::<CardId>().is_err());
    }

    #[test]
    #[should_panic(expected = "reserved for JSON-level cards")]
    fn the_json_name_is_reserved() {
        assert_distinct_tags(&["trail", "json"]);
    }

    #[test]
    fn distinct_names_pass_the_tag_check() {
        assert_distinct_tags(&["trail", "fake", "far"]);
    }
}
