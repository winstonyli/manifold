//! A card's identity: the type it is stored under and the tags its runtime adds (spec 2026-10-04-node-model-design.md,
//! section 1). The key into the capability rows (`caps::registry`), replacing `ItemNodeKind` as 2b-6 moves its readers.

use super::*;
use crate::caps::{Tag, TagSet};
use crate::store::model::Kind;

/// The row string for a type this build doesn't know: only the `*/*` row matches it.
const UNKNOWN_TYPE: &str = "*/*";

/// A card's type and tags, as `caps::registry().caps(ty, tags)` takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CardKind {
    pub(crate) ty: &'static str,
    pub(crate) tags: TagSet,
}

impl App {
    /// The identity of card `id`: its stored type (`Mirror::ty`, canonicalised: any `image/*` is the image row), plus
    /// `Tag::Loaded` while a tab has an engine. `None` when `id` has no node.
    // Read from Task 2 on (the capability readers replace `ItemNodeKind`).
    #[allow(dead_code)]
    pub(crate) fn card_kind(&self, id: &str) -> Option<CardKind> {
        let node = self.nodes.get(id)?;
        let ty = Kind::from_type(self.mirror.ty(id)).type_str().unwrap_or(UNKNOWN_TYPE);
        let tags = if node.payload.as_tab().is_some() { TagSet::of(&[Tag::Loaded]) } else { TagSet::EMPTY };
        Some(CardKind { ty, tags })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{bridge, registry};
    use crate::node::{
        item_node_kind, DownloadPayload, GroupPayload, ImagePayload, ItemNodeKind, JsonPayload, NotePayload,
        PdfPayload, UnloadedState, UnloadedTabPayload, GRAPH_DRAW_ORDER,
    };
    use crate::process_manager::{EngineKind, EngineTab};

    fn payloads() -> Vec<(Kind, NodePayload)> {
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
        vec![
            (Kind::Tab, live),
            (Kind::Tab, sleeping),
            (Kind::Download, download),
            (Kind::Note, NodePayload::Note(NotePayload { text: "t".into(), editing: false })),
            (Kind::Image, NodePayload::Image(ImagePayload { path: "i.png".into(), width: 1, height: 1, error: None })),
            (Kind::Pdf, NodePayload::Pdf(PdfPayload { path: "d.pdf".into(), outcome: None })),
            (Kind::Json, json(false)),
            (Kind::JsonLeaf, json(true)),
            (Kind::Group, NodePayload::Group(GroupPayload::default())),
        ]
    }

    #[test]
    fn card_kind_matches_the_bridge_for_every_constructible_payload() {
        let mut app = App::default();
        for (i, (kind, payload)) in payloads().into_iter().enumerate() {
            let id = format!("n{i}");
            let item = item_node_kind(&payload);
            let loaded = payload.as_tab().is_some();
            app.test_node(&id, kind, payload);
            let got = app.card_kind(&id).unwrap();
            assert_eq!(got.tags.contains_all(TagSet::of(&[Tag::Loaded])), loaded, "{kind:?}");
            if kind != Kind::JsonLeaf {
                let (ty, tags) = bridge::item_node(item);
                assert_eq!(got, CardKind { ty, tags }, "{kind:?}");
            } else {
                assert_eq!(got.ty, crate::store::model::TYPE_JSON_LEAF);
            }
        }
        assert_eq!(app.card_kind("missing"), None);
    }

    #[test]
    fn an_unknown_stored_type_falls_back_to_the_catch_all_row() {
        let mut app = App::default();
        app.test_node("u", Kind::Unknown, NodePayload::Group(GroupPayload::default()));
        assert_eq!(app.card_kind("u").unwrap().ty, UNKNOWN_TYPE);
    }

    #[test]
    fn draw_layer_follows_graph_draw_order() {
        for (index, kind) in GRAPH_DRAW_ORDER.into_iter().enumerate() {
            let (ty, tags) = bridge::item_node(kind);
            assert_eq!(usize::from(registry().caps(ty, tags).draw_layer), index, "{kind:?}");
        }
        assert_eq!(GRAPH_DRAW_ORDER[7], ItemNodeKind::Group);
    }
}
