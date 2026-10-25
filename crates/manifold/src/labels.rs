//! Labels: stored `Kind::Label` nodes a card can carry by name and colour (`PROPS/labels`). A label is mirror and
//! store state, never a graph card, like a link (`links.rs`).

/// A label's synced definition, decoded from its mirrored fields (`Mirror::label_def`).
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code, reason = "read by the label lifecycle (Task 3)")]
pub(crate) struct LabelDef {
    pub(crate) name: String,
    pub(crate) color: Option<egui::Color32>,
}
