//! A card's identity: the type it is stored under and the tags its runtime adds (spec 2026-10-04-node-model-design.md,
//! section 1). The key into the capability rows (`caps::registry`) and the edge table (`caps::edges`), replacing
//! `ItemNodeKind` as 2b-6 moves its readers.

use super::*;
use crate::caps::{edges, registry, Caps, Tag, TagSet, CONTAINS};
use crate::store::model::{
    Kind, TYPE_DOWNLOAD, TYPE_GROUP, TYPE_IMAGE, TYPE_JSON, TYPE_JSON_LEAF, TYPE_NOTE, TYPE_PDF, TYPE_TAB,
};

/// The row string for a type this build doesn't know: only the `*/*` row matches it.
const UNKNOWN_TYPE: &str = "*/*";

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

/// The name EXPORTSTATE's `cards[].kind` gave this card before 2b-6 (`ItemNodeKind`'s `Debug` spelling): a live tab
/// is `Tab`, any other tab `Unloaded`, a JSON leaf `Json`. The probe scripts still read these; Task 5 replaces them.
pub(crate) fn legacy_kind_name(kind: CardKind) -> &'static str {
    match kind.ty {
        TYPE_TAB if kind.loaded() => "Tab",
        TYPE_TAB => "Unloaded",
        TYPE_DOWNLOAD => "Download",
        TYPE_GROUP => "Group",
        TYPE_NOTE => "Note",
        TYPE_IMAGE => "Image",
        TYPE_PDF => "Pdf",
        TYPE_JSON | TYPE_JSON_LEAF => "Json",
        _ => "Unknown",
    }
}

impl App {
    /// The identity of card `id`: its stored type (`Mirror::ty`, canonicalised: any `image/*` is the image row), plus
    /// `Tag::Loaded` while a tab has an engine. `None` when `id` has no node.
    pub(crate) fn card_kind(&self, id: &str) -> Option<CardKind> {
        let node = self.nodes.get(id)?;
        let ty = Kind::from_type(self.mirror.ty(id)).type_str().unwrap_or(UNKNOWN_TYPE);
        let tags = if node.payload.as_tab().is_some() { TagSet::of(&[Tag::Loaded]) } else { TagSet::EMPTY };
        Some(CardKind { ty, tags })
    }
}

/// The caps bridge's `item_node`, for the test tables still written over `ItemNodeKind` (Task 3 rewrites them).
#[cfg(test)]
impl From<ItemNodeKind> for CardKind {
    fn from(kind: ItemNodeKind) -> CardKind {
        let (ty, tags) = match kind {
            ItemNodeKind::Tab => (TYPE_TAB, TagSet::of(&[Tag::Loaded])),
            ItemNodeKind::Unloaded => (TYPE_TAB, TagSet::EMPTY),
            ItemNodeKind::Download => (TYPE_DOWNLOAD, TagSet::EMPTY),
            ItemNodeKind::Group => (TYPE_GROUP, TagSet::EMPTY),
            ItemNodeKind::Note => (TYPE_NOTE, TagSet::EMPTY),
            ItemNodeKind::Image => (TYPE_IMAGE, TagSet::EMPTY),
            ItemNodeKind::Pdf => (TYPE_PDF, TagSet::EMPTY),
            ItemNodeKind::Json => (TYPE_JSON, TagSet::EMPTY),
        };
        CardKind { ty, tags }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::DRAW_LAYERS;
    use crate::node::{
        DownloadPayload, GroupPayload, ImagePayload, JsonPayload, NotePayload, PdfPayload, UnloadedState,
        UnloadedTabPayload,
    };
    use crate::process_manager::{EngineKind, EngineTab};

    fn plain(ty: &'static str) -> CardKind {
        CardKind { ty, tags: TagSet::EMPTY }
    }

    fn loaded_tab() -> CardKind {
        CardKind { ty: TYPE_TAB, tags: TagSet::of(&[Tag::Loaded]) }
    }

    /// Every card kind the old `ItemNodeKind` named, as `CardKind`s: the bridge tests' `KINDS` list.
    fn kinds() -> [CardKind; 8] {
        [
            loaded_tab(),
            plain(TYPE_TAB),
            plain(TYPE_DOWNLOAD),
            plain(TYPE_GROUP),
            plain(TYPE_NOTE),
            plain(TYPE_IMAGE),
            plain(TYPE_PDF),
            plain(TYPE_JSON),
        ]
    }

    /// Every constructible payload with the type it is stored under and the `CardKind` it must read as.
    fn payloads() -> Vec<(Kind, NodePayload, CardKind)> {
        let (tab, _peer) = EngineTab::fake(EngineKind::Servo);
        let live = NodePayload::Tab(crate::node::TabPayload {
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
        let sleeping = NodePayload::Unloaded(UnloadedTabPayload {
            url: "https://s/".into(),
            title: String::new(),
            muted: false,
            zoom_factor: 1.0,
            state: UnloadedState::Sleeping,
        });
        let download = NodePayload::Download(DownloadPayload {
            path: "f.bin".into(),
            bytes_received: 0,
            total_bytes: None,
            completed: false,
            failed: false,
        });
        let json = |leaf| {
            NodePayload::Json(JsonPayload {
                path: "a.json".into(),
                value: std::sync::Arc::new(1.into()),
                leaf,
                error: None,
            })
        };
        let image = NodePayload::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None });
        vec![
            (Kind::Tab, live, loaded_tab()),
            (Kind::Tab, sleeping, plain(TYPE_TAB)),
            (Kind::Download, download, plain(TYPE_DOWNLOAD)),
            (Kind::Note, NodePayload::Note(NotePayload { text: "t".into(), editing: false }), plain(TYPE_NOTE)),
            (Kind::Image, image, plain(TYPE_IMAGE)),
            (Kind::Pdf, NodePayload::Pdf(PdfPayload { path: "d.pdf".into(), outcome: None }), plain(TYPE_PDF)),
            (Kind::Json, json(false), plain(TYPE_JSON)),
            (Kind::JsonLeaf, json(true), plain(TYPE_JSON_LEAF)),
            (Kind::Group, NodePayload::Group(GroupPayload::default()), plain(TYPE_GROUP)),
        ]
    }

    #[test]
    fn card_kind_reads_the_type_and_tags_loaded_for_every_constructible_payload() {
        let mut app = App::default();
        for (i, (kind, payload, want)) in payloads().into_iter().enumerate() {
            let id = format!("n{i}");
            let live = payload.as_tab().is_some();
            app.test_node(&id, kind, payload);
            let got = app.card_kind(&id).unwrap();
            assert_eq!(got, want, "{kind:?}");
            assert_eq!(got.loaded(), live, "{kind:?}");
        }
        assert_eq!(app.card_kind("missing"), None);
    }

    #[test]
    fn an_unknown_stored_type_falls_back_to_the_catch_all_row() {
        let mut app = App::default();
        app.test_node("u", Kind::Unknown, NodePayload::Group(GroupPayload::default()));
        let kind = app.card_kind("u").unwrap();
        assert_eq!(kind.ty, UNKNOWN_TYPE);
        assert!(!kind.caps().restorable);
        assert_eq!(kind.caps().draw_layer, 0);
        assert!(!is_container(kind.ty));
    }

    /// The layers in the order `GRAPH_DRAW_ORDER` drew the kinds: a later one draws on top.
    #[test]
    fn draw_layer_follows_the_graph_draw_order() {
        let order = [
            (loaded_tab(), 0),
            (plain(TYPE_TAB), 1),
            (plain(TYPE_DOWNLOAD), 2),
            (plain(TYPE_NOTE), 3),
            (plain(TYPE_IMAGE), 4),
            (plain(TYPE_PDF), 5),
            (plain(TYPE_JSON), 6),
            (plain(TYPE_JSON_LEAF), 6),
            (plain(TYPE_GROUP), 7),
        ];
        for (kind, layer) in order {
            assert_eq!(kind.caps().draw_layer, layer, "{kind:?}");
            assert!(usize::from(layer) < DRAW_LAYERS);
        }
        // A leaf drew as a Json card (`ItemNodeKind::Json` covered both types).
        assert_eq!(plain(TYPE_JSON_LEAF).caps().draw_layer, plain(TYPE_JSON).caps().draw_layer);
    }

    #[test]
    fn only_a_group_card_is_a_container() {
        for kind in kinds() {
            assert_eq!(is_container(kind.ty), kind.ty == TYPE_GROUP, "{kind:?}");
        }
    }

    #[test]
    fn every_card_kind_can_go_in_a_group_and_nothing_goes_in_anything_else() {
        for member in kinds() {
            for onto in kinds() {
                assert_eq!(can_contain(member, onto), onto.ty == TYPE_GROUP, "{member:?} into {onto:?}");
            }
        }
    }

    /// EXPORTSTATE's `kind` strings are unchanged until Task 5: each old kind's `Debug` name, a leaf's `Json`.
    #[test]
    fn legacy_kind_names_spell_the_old_enum() {
        for kind in GRAPH_DRAW_ORDER {
            assert_eq!(legacy_kind_name(kind.into()), format!("{kind:?}"));
        }
        assert_eq!(legacy_kind_name(plain(TYPE_JSON_LEAF)), "Json");
        assert_eq!(legacy_kind_name(plain(UNKNOWN_TYPE)), "Unknown");
        // The test bridge keeps the old pair: one type, told apart by `Loaded`.
        let (tab, sleeping) = (CardKind::from(ItemNodeKind::Tab), CardKind::from(ItemNodeKind::Unloaded));
        assert_eq!(tab.ty, sleeping.ty);
        assert!(tab.loaded() && !sleeping.loaded());
    }
}
