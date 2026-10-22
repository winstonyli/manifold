//! A card's identity: the type it is stored under and the tags its runtime adds (spec 2026-10-04-node-model-design.md,
//! section 1). The key into the capability rows (`caps::registry`) and the edge table (`caps::edges`); the card
//! classifier enum it replaced went in 2b-6.

use super::*;
use crate::caps::{edges, registry, Caps, Tag, TagSet, CONTAINS};
use crate::store::model::NodeId;
use crate::store::model::{Kind, TYPE_UNKNOWN};
#[cfg(test)]
use crate::store::model::{TYPE_DOWNLOAD, TYPE_GROUP, TYPE_IMAGE, TYPE_JAR, TYPE_JSON, TYPE_NOTE, TYPE_PDF, TYPE_TAB};

/// The row string a stored type reads as, for `caps::registry` and `caps::edges` (spec decision 7): a known kind's
/// own type string (case ignored; any `image/*` is the image row), `TYPE_UNKNOWN` for a type this build doesn't
/// know, and "" for no type (an id the mirror holds no `type` for). Only a `*/*` row matches "" (a string without a
/// slash reads as `Pattern::Any`'s), so an id with no type gets exactly the abilities `*/*` grants every card.
pub(crate) fn canonical_type(raw: &str) -> &'static str {
    if raw.is_empty() {
        return "";
    }
    Kind::from_type(raw).type_str().unwrap_or(TYPE_UNKNOWN)
}

/// A card's type and tags, as `caps::registry().caps(ty, tags)` takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CardKind {
    pub(crate) ty: &'static str,
    pub(crate) tags: TagSet,
}

impl CardKind {
    /// A tab with a running engine (`Tag::Loaded`), as against a sleeping, waking or failed one.
    pub(crate) fn loaded(self) -> bool {
        self.tags.contains_all(TagSet::of(&[Tag::Loaded]))
    }

    /// Its capabilities (`caps::registry`).
    pub(crate) fn caps(self) -> Caps {
        registry().caps(self.ty, self.tags)
    }
}

/// Whether nodes can sit inside a card of type `ty`: the target of a `contains` edge (a group).
pub(crate) fn is_container(ty: &str) -> bool {
    edges().target_accepts(CONTAINS, ty)
}

/// Whether a `member` card may be put inside a `group` card; argument order is (member, group), source then target.
pub(crate) fn can_contain(member: CardKind, group: CardKind) -> bool {
    edges().can_connect(CONTAINS, member.ty, group.ty)
}

impl App {
    /// The identity of card `id`: its stored type as its row reads it (`canonical_type`), plus `Tag::Loaded` while a
    /// tab has an engine. `None` when `id` has no node.
    pub(crate) fn card_kind(&self, id: NodeId) -> Option<CardKind> {
        let node = self.nodes.get(&id)?;
        let ty = canonical_type(self.doc.mirror.ty(id));
        let tags = if node.runtime.as_tab().is_some() { TagSet::of(&[Tag::Loaded]) } else { TagSet::EMPTY };
        Some(CardKind { ty, tags })
    }

    /// Whether node `id` is a container (a group): `is_container` of its type. `false` when `id` has no node.
    pub(crate) fn is_container(&self, id: NodeId) -> bool {
        self.card_kind(id).is_some_and(|kind| is_container(kind.ty))
    }
}

/// The card kinds the tests' tables run over.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// A card of type `ty` with no tags: every kind but the live tab.
    pub(crate) fn plain(ty: &'static str) -> CardKind {
        CardKind { ty, tags: TagSet::EMPTY }
    }

    /// A tab with a running engine.
    pub(crate) fn loaded_tab() -> CardKind {
        CardKind { ty: TYPE_TAB, tags: TagSet::of(&[Tag::Loaded]) }
    }

    /// A sleeping, waking or failed tab.
    pub(crate) fn sleeping_tab() -> CardKind {
        plain(TYPE_TAB)
    }

    /// Every card kind, in draw order: each `TYPES` entry a node can show and the unknown-type row
    /// (`kinds_cover_every_node_type` pins that), plus the loaded tab, which shares the sleeping tab's type.
    pub(crate) fn kinds() -> [CardKind; 10] {
        [
            loaded_tab(),
            sleeping_tab(),
            plain(TYPE_DOWNLOAD),
            plain(TYPE_NOTE),
            plain(TYPE_JAR),
            plain(TYPE_UNKNOWN),
            plain(TYPE_IMAGE),
            plain(TYPE_PDF),
            plain(TYPE_JSON),
            plain(TYPE_GROUP),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{kinds, loaded_tab, plain, sleeping_tab};
    use super::*;
    use crate::caps::DRAW_LAYERS;
    use crate::card_id::testutil::node;
    use crate::node::{
        DownloadPayload, ImagePayload, JsonPayload, NotePayload, PdfPayload, UnloadedState, UnloadedTabPayload,
    };
    use crate::process_manager::{ViewKind, ViewTab};
    use crate::store::model::FieldValue;

    /// The fixture lists one card per node type (a link is never a card) and the loaded tab, in draw order.
    #[test]
    fn kinds_cover_every_node_type() {
        use crate::store::model::{TYPES, TYPE_LINK};
        let mut types: Vec<&str> = TYPES.iter().map(|t| t.1).filter(|&ty| ty != TYPE_LINK).collect();
        types.push(TYPE_UNKNOWN);
        let listed: Vec<&str> = kinds().iter().filter(|kind| !kind.loaded()).map(|kind| kind.ty).collect();
        for ty in &types {
            assert!(listed.contains(ty), "{ty} is not in kinds()");
        }
        assert_eq!(listed.len(), types.len(), "{listed:?}");
        assert_eq!(kinds()[0], loaded_tab());
        let layers: Vec<u8> = kinds().iter().map(|kind| kind.caps().draw_layer).collect();
        assert!(layers.windows(2).all(|pair| pair[0] <= pair[1]), "{layers:?}");
    }

    /// Every constructible payload with the type it is stored under and the `CardKind` it must read as.
    fn payloads() -> Vec<(Kind, Runtime, CardKind)> {
        let (tab, _peer) = ViewTab::fake(ViewKind::Servo);
        let live = Runtime::Tab(crate::node::TabPayload {
            tab,
            url: "https://a/".into(),
            title: String::new(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_playing: false,
            muted: false,
            zoom_factor: 1.0,
            swap_target: None,
        });
        let sleeping = Runtime::Unloaded(UnloadedTabPayload {
            url: "https://s/".into(),
            title: String::new(),
            muted: false,
            zoom_factor: 1.0,
            state: UnloadedState::Sleeping,
        });
        let download = Runtime::Download(DownloadPayload {
            path: "f.bin".into(),
            bytes_received: 0,
            total_bytes: None,
            completed: false,
            failed: false,
        });
        let json =
            Runtime::Json(JsonPayload { path: "a.json".into(), value: std::sync::Arc::new(1.into()), error: None });
        let image = Runtime::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None });
        vec![
            (Kind::Tab, live, loaded_tab()),
            (Kind::Tab, sleeping, plain(TYPE_TAB)),
            (Kind::Download, download, plain(TYPE_DOWNLOAD)),
            (Kind::Note, Runtime::Note(NotePayload { text: "t".into(), editing: false }), plain(TYPE_NOTE)),
            (Kind::Image, image, plain(TYPE_IMAGE)),
            (Kind::Pdf, Runtime::Pdf(PdfPayload { path: "d.pdf".into(), outcome: None }), plain(TYPE_PDF)),
            (Kind::Json, json, plain(TYPE_JSON)),
            (Kind::Jar, Runtime::Jar, plain(TYPE_JAR)),
            (Kind::Unknown, Runtime::Unknown, plain(TYPE_UNKNOWN)),
            (Kind::Group, Runtime::None, plain(TYPE_GROUP)),
        ]
    }

    #[test]
    fn card_kind_reads_the_type_and_tags_loaded_for_every_constructible_payload() {
        let mut app = App::default();
        for (i, (kind, payload, want)) in payloads().into_iter().enumerate() {
            let id = node(i as u64 + 1);
            let live = payload.as_tab().is_some();
            app.test_node(id, kind, payload);
            if kind == Kind::Unknown {
                let future = FieldValue::Str("application/x-from-the-future".into());
                app.doc.mirror.init_field(id, crate::store::model::field::TYPE, future);
            }
            let got = app.card_kind(id).unwrap();
            assert_eq!(got, want, "{kind:?}");
            assert_eq!(got.loaded(), live, "{kind:?}");
        }
        assert_eq!(app.card_kind(node(9999)), None);
    }

    /// A type this build doesn't know reads as the sentinel row: a restorable card drawn with notes and jars, not a
    /// container.
    #[test]
    fn an_unknown_stored_type_reads_as_the_unknown_row() {
        use crate::store::model::{field, FieldValue};
        let mut app = App::default();
        app.test_node(node(500), Kind::Unknown, Runtime::Unknown);
        app.doc.mirror.init_field(node(500), field::TYPE, FieldValue::Str("application/x-from-the-future".into()));
        let kind = app.card_kind(node(500)).unwrap();
        assert_eq!(kind.ty, TYPE_UNKNOWN);
        assert!(kind.caps().restorable);
        assert_eq!(kind.caps().draw_layer, 3);
        assert!(!is_container(kind.ty));
        assert!(!app.is_container(node(500)) && !app.is_container(node(9999)));
    }

    /// One mapping from a stored type to its row (spec decision 7): a known kind's own string (any image, any case),
    /// the sentinel for anything else, and "" (no row) for no type.
    #[test]
    fn canonical_type_maps_known_kinds_to_their_row_and_the_rest_to_the_sentinel() {
        use crate::store::model::TYPE_LINK;
        assert_eq!(canonical_type("Text/Markdown"), TYPE_NOTE);
        assert_eq!(canonical_type("image/jpeg"), TYPE_IMAGE);
        assert_eq!(canonical_type(TYPE_TAB), TYPE_TAB);
        assert_eq!(canonical_type(TYPE_LINK), TYPE_LINK);
        assert_eq!(canonical_type("application/x-from-the-future"), TYPE_UNKNOWN);
        assert_eq!(canonical_type("application/vnd.manifold.json-leaf"), TYPE_UNKNOWN, "a retired type");
        assert_eq!(canonical_type(TYPE_UNKNOWN), TYPE_UNKNOWN);
        assert_eq!(canonical_type(""), "");
    }

    /// The layers in the order Graph view always drew the kinds: a later one draws on top.
    #[test]
    fn draw_layer_follows_the_graph_draw_order() {
        let order = [
            (loaded_tab(), 0),
            (plain(TYPE_TAB), 1),
            (plain(TYPE_DOWNLOAD), 2),
            (plain(TYPE_NOTE), 3),
            (plain(TYPE_JAR), 3),
            (plain(TYPE_UNKNOWN), 3),
            (plain(TYPE_IMAGE), 4),
            (plain(TYPE_PDF), 5),
            (plain(TYPE_JSON), 6),
            (plain(TYPE_GROUP), 7),
        ];
        for (kind, layer) in order {
            assert_eq!(kind.caps().draw_layer, layer, "{kind:?}");
            assert!(usize::from(layer) < DRAW_LAYERS);
        }
    }

    #[test]
    fn only_a_group_card_is_a_container() {
        for kind in kinds() {
            assert_eq!(is_container(kind.ty), kind.ty == TYPE_GROUP, "{kind:?}");
        }
        // The node form reads the mirror's type, not the runtime: a group has none.
        let mut app = App::default();
        app.test_node(node(501), Kind::Group, Runtime::None);
        app.test_node(node(502), Kind::Note, Runtime::Note(NotePayload { text: String::new(), editing: false }));
        assert!(app.is_container(node(501)) && !app.is_container(node(502)));
    }

    #[test]
    fn every_card_kind_can_go_in_a_group_and_nothing_goes_in_anything_else() {
        for member in kinds() {
            for onto in kinds() {
                assert_eq!(can_contain(member, onto), onto.ty == TYPE_GROUP, "{member:?} into {onto:?}");
            }
        }
    }

    /// The old Tab/Unloaded pair is one type, told apart by `Loaded`.
    #[test]
    fn a_live_and_a_sleeping_tab_share_a_type_and_differ_in_loaded() {
        assert_eq!(loaded_tab().ty, sleeping_tab().ty);
        assert!(loaded_tab().loaded() && !sleeping_tab().loaded());
    }
}
