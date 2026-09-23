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

```rust
struct ItemToolbarCtx<'a> {
    address_bar_text: &'a mut String,
    // future per-kind mutable state (e.g. an image zoom level) goes here
}

#[derive(Default)]
struct ItemToolbarActions {
    reload_requested: bool,
    home_requested: bool,
    go_back: bool,
    go_forward: bool,
    navigate: Option<String>,
    bookmark_toggled: bool,
    split_requested: bool,
    close_split_requested: bool,
    view_json_requested: bool,
    // extended as needed
}

fn render_item_toolbar_content(
    payload: &NodePayload,
    ui: &mut egui::Ui,
    ctx: &mut ItemToolbarCtx,
) -> ItemToolbarActions {
    match payload {
        NodePayload::Tab(t) => { /* today's Tab toolbar body, reading `t` + `ctx`, returning actions */ }
        _ => ItemToolbarActions::default(),
    }
}
```

The caller (`build_tab_ui`) builds `ctx` from disjoint `self` fields
(`&mut self.address_bar_text`, etc.) and applies the returned
`ItemToolbarActions` to `self` afterward — the same
extract-then-mutate-after discipline the current code already uses
(e.g. `main.rs:8409`, `main.rs:8640`), just packaged as a return value
instead of ad hoc locals.

**Why match, not a trait, for this pass:** a trait scoped to the payload
struct (`impl ItemToolbarContent for TabPayload`) is technically sound —
the earlier concern that it fights egui's borrow patterns was wrong,
since the trait's `Self` would be the small payload struct, not the
whole `App`, and `ItemToolbarCtx` threads through the same disjoint-field
borrows the match version uses. But a trait's only real benefit here is
code locality (each kind's toolbar logic next to that kind's other
methods), and in this single 13k-line `main.rs` with no per-kind
modules, that locality argument doesn't hold — a Tab impl and a future
Note impl would end up thousands of lines apart, harder to compare than
consecutive match arms. Both approaches get identical compile-time
safety (Rust's match exhaustiveness applies either way).

**Migration path, if this file is later split into per-kind modules:**
because the match arms already destructure to exactly
`(t: &TabPayload, ui, ctx) -> ItemToolbarActions` — a trait method's
exact input/output shape — moving to a trait is a mechanical cut: each
arm's body becomes `impl ItemToolbarContent for TabPayload { fn
toolbar_content(&self, ui, ctx) -> ItemToolbarActions { <body> } }`, and
the dispatcher collapses to one-line delegate calls per arm. No
signature or logic changes required.

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
  `render_item_toolbar_content`, `ItemToolbarCtx`, `ItemToolbarActions`.
- `build_tab_ui` (`main.rs:8361`): replace inline nav/address/trailing-
  cluster code with calls to the four new functions above; apply the
  returned `ItemToolbarActions` the same way existing flags
  (`reload_requested`, etc.) are applied today.
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
