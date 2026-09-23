# Unified Toolbar Architecture Design

## Goal

Replace the Graph and Tab toolbars' current ad hoc, independently-styled
button rows with one shared composition model and one shared visual
system (Option A: segmented chips), so:

- Actions read as grouped clusters instead of a flat row of buttons.
- Both toolbars look like one coherent system, not two unrelated ones.
- Adding full-screen support for another node kind later (Note, Image,
  Pdf, Json) is a single, well-defined extension point, not another
  toolbar redesign.

## Non-goals (this pass)

- Building full-screen/maximized views for Note, Image, Pdf, or folding
  the existing JSON floating viewer into this system. Today only Tab
  nodes can enter `ViewMode::Tab` (confirmed: `ViewMode` is
  `{Graph, Tab, Settings}`, `main.rs:1442`; `focus_tab` is the only path
  into `ViewMode::Tab` and always targets a Tab node). This design adds
  the extension point; it does not use it for anything beyond Tab.
- Renaming `ViewMode::Tab` or otherwise generalizing the view-mode enum.
  Unnecessary churn for a Graph+Tab-only pass — the toolbar composition
  layer, not the enum name, is where the architecture lives.
- Making the Universal chip and the Tab-only `.unwrap()` in `build_tab_ui`
  robust to a future non-Tab kind entering a maximized view. In the
  current implementation `render_universal_chip` is called from inside
  Tab's own per-kind content (`render_tab_toolbar_content`), not
  unconditionally before per-kind dispatch, and `build_tab_ui` unwraps
  the focused node as a Tab payload before that dispatch even runs. Both
  are safe under this pass's own Non-goal (only Tab enters `ViewMode::Tab`
  today) and are commented at their call sites in `main.rs`, but a real
  per-kind toolbar for another kind needs to address both before it can
  rely on the Universal chip or the address-bar-text sync path. Both were
  resolved by the Image Item View pass
  (`2026-09-23-image-item-view-design.md`): `render_item_toolbar` now
  renders the Universal chip as a real layer-1 element before per-kind
  dispatch, and the `.unwrap()` became an `if let`.

## Composition model

Every toolbar is built from up to three layers, outer to inner:

1. **Universal chip** — Help + Settings. Present in every toolbar,
   including Graph's. Already implemented as shared methods
   (`build_help_button`, `build_settings_button`) called from both
   toolbars today; this pass wraps that existing call pair in the new
   chip visual via one shared `render_universal_chip(app, ui)` function,
   rather than each toolbar independently laying it out.
2. **Item-view shared chip** — "back to Graph." Present in every
   non-Graph (i.e. currently: Tab-view) toolbar. Extracted from today's
   Tab-view-specific `ui.button("Graph")` into `render_return_to_graph`
   so a future kind entering a maximized view gets it automatically.
3. **Per-kind content** — whatever the focused node's kind contributes:
   today, only Tab (nav cluster, address bar, identity slot, page-actions
   chip). Every other kind gets nothing yet, but the dispatch point
   exists and returns an empty result for them.

The Graph toolbar's own content (+ New Tab, Groups chip, Reset View,
Filter, selection actions) is not "per-kind" — it belongs to the Graph
*view* itself, not a focused node, so it stays as Graph-toolbar-local
code that also calls the shared `render_universal_chip`.

## Dispatch mechanism: match now, trait-shaped for a painless upgrade

Investigating the real `build_tab_ui` body (`main.rs:8361-8954`) found the
Tab toolbar's content needs far more than a payload reference plus one
string: it reads/writes `self.suggestion_fetcher`, `self.dropdown_
highlighted`, `self.tab_actions_open`, `self.split_tab`, `self.split_
engine_picker_open`, `self.json_viewer_node`, `self.bookmarks`, `self.
pending_focus_address_bar`, `self.address_bar_focused`, `self.open_tab_
suggestions`, and calls `self.build_help_button` (itself `&mut self`) —
essentially the whole `App`, not a narrow slice. A per-kind function
that only receives `&TabPayload` plus a small ctx struct can't express
this without that struct growing to cover nearly all of `App`'s
surface, at which point it buys nothing over just taking `&mut self`.

The accurate design: per-kind content functions are `&mut self` methods,
dispatched via a cheap **owned classifier computed first**, so the
classifier's borrow of `self.nodes` ends before the method call needs
`&mut self` — exactly the discipline `close_node`'s existing `MemberKind`
match already uses (`main.rs:7375`: match on a `Copy`/owned enum derived
from `&NodePayload`, not on the payload reference itself, specifically
so the immutable borrow ends before the match arms run `&mut self`
cleanup).

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemNodeKind { Tab, Download, Group, Note, Image, Pdf, Json }

fn item_node_kind(payload: &NodePayload) -> ItemNodeKind {
    match payload {
        NodePayload::Tab(_) => ItemNodeKind::Tab,
        NodePayload::Download(_) => ItemNodeKind::Download,
        NodePayload::Group(_) => ItemNodeKind::Group,
        NodePayload::Note(_) => ItemNodeKind::Note,
        NodePayload::Image(_) => ItemNodeKind::Image,
        NodePayload::Pdf(_) => ItemNodeKind::Pdf,
        NodePayload::Json(_) => ItemNodeKind::Json,
    }
}

#[derive(Default)]
struct ItemToolbarActions {
    go_back: bool,
    go_forward: bool,
    reload_requested: bool,
    home_requested: bool,
    enter_pressed: bool,
    toggle_bookmark_requested: bool,
    close_split_requested: bool,
    tab_actions_button_rect: Option<egui::Rect>,
    split_engine_button_rect: Option<egui::Rect>,
    address_bar_rect: Option<egui::Rect>,
    shortcuts_help_button_rect: Option<egui::Rect>,
}

impl App {
    fn render_item_toolbar_content(&mut self, focused_id: &str, ui: &mut egui::Ui) -> ItemToolbarActions {
        let kind = match self.nodes.get(focused_id) {
            Some(node) => item_node_kind(&node.payload),
            None => return ItemToolbarActions::default(),
        };
        match kind {
            ItemNodeKind::Tab => self.render_tab_toolbar_content(focused_id, ui),
            ItemNodeKind::Download
            | ItemNodeKind::Group
            | ItemNodeKind::Note
            | ItemNodeKind::Image
            | ItemNodeKind::Pdf
            | ItemNodeKind::Json => ItemToolbarActions::default(),
        }
    }
}
```

`render_tab_toolbar_content` wraps *only* what's inside today's
`ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
...})` block (the nav cluster through the trailing cluster and address
bar — `main.rs:8447-8681`), unchanged in body, returning the struct
above instead of leaving `let mut` locals behind. Everything currently
around that closure in `build_tab_ui` (escape handling, the suggestion
dropdown `Area`, applying `go_back`/`go_forward`/etc. to `self`) stays
exactly where it is — it just reads `actions.field` instead of the old
bare locals. `navigate: Option<String>` is NOT part of the struct: it's
computed after the closure today from `enter_pressed` and the
suggestion list (`main.rs:8781-8786`), so only `enter_pressed` needs to
cross the boundary.

**Why match, not a trait, for this pass:** the trait objection about
egui's borrow patterns doesn't actually block a *correctly-scoped*
trait (`Self = TabPayload`, cross-cutting state threaded through an
explicit ctx) — but as the investigation above shows, correctly scoping
it here would require that ctx to carry nearly all of `App`, which is
no longer a meaningful ctx boundary. A trait's remaining benefit is code
locality (each kind's logic next to that kind's other methods), and in
this single 13k-line `main.rs` with no per-kind modules, that argument
doesn't hold — a Tab impl and a future Note impl would end up thousands
of lines apart, harder to compare than consecutive match arms. Both
approaches get identical compile-time safety: `item_node_kind`'s match
is exhaustive over `NodePayload` either way, so adding an 8th variant
without updating it is a compile error regardless of which mechanism
consumes the classifier.

**Migration path, if this file is later split into per-kind modules:**
because `render_tab_toolbar_content` already has the shape `fn(&mut
self, focused_id, ui) -> ItemToolbarActions` with all of its state
access self-contained, moving it to a trait method on `TabPayload`
would still require solving the same "needs nearly all of `App`"
problem — so the realistic migration isn't "cut the match arm into a
trait impl" as originally assumed, it's "decide which pieces of `App`
state each kind's toolbar genuinely needs and carve those into a
per-kind-relevant struct" — a real design task of its own at that
point, not a mechanical one. Flagged here so a future reader doesn't
assume it's free.

## Visual system

Extract the chip-painting idiom from the toolbar-chip-wip prototype
(reserve a `Shape::Noop`, lay out the chip's children as direct siblings
of the row's other widgets — never inside a nested `ui.horizontal`, which
was the root cause of that prototype's vertical-alignment bug — then
backfill the reserved shape with the union of the children's rects) into
one reusable helper:

```rust
fn toolbar_chip(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> ...) -> ...
```

Every chip in both toolbars — Graph's Groups chip, Tab's nav chip, Tab's
page-actions chip, and the new shared Universal chip — goes through this
one helper, so the vertical-alignment fix and the chip fill/radius live
in exactly one place and can't drift apart between toolbars.

## Concrete refactor inventory

- New: `toolbar_chip`, `render_universal_chip`, `render_return_to_graph`,
  `ItemNodeKind`, `item_node_kind`, `ItemToolbarActions`,
  `App::render_item_toolbar_content`, `App::render_tab_toolbar_content`.
- `build_tab_ui` (`main.rs:8361`): the leading `if ui.button("Graph")
  .clicked() { back_to_graph = true; }` moves out into
  `render_return_to_graph`; the rest of the `ui.with_layout(...)` block
  (`main.rs:8447-8681`) moves into `render_tab_toolbar_content` verbatim
  aside from returning `ItemToolbarActions` instead of leaving locals;
  everything outside that block (escape/suggestion/mutation handling)
  stays put, reading the returned struct's fields.
- Graph toolbar (`toolbar_panel("graph_toolbar", ...)`, `main.rs:9059`):
  replace its own inline Help/Settings calls with `render_universal_chip`;
  wrap its Groups cluster (Import Group / Ungroup All / Collapse-Expand)
  in `toolbar_chip`.
- The chip-icon-button styling (`chip_icon_button`, transparent-fill
  real-frame button) from `toolbar-chip-wip` carries over unchanged as
  the icon style used inside `toolbar_chip`.

## Testing

Same headless-screenshot + pixel-measurement technique used throughout
the earlier chip prototype work: isolated `%APPDATA%`, `SCREENSHOT`
debug command, Python/PIL edge-sampling to confirm chip edges pixel-match
their un-chipped neighbors across both toolbars.
