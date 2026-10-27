//! The popups and menus on screen (`App::menus`): which anchored popup is open, the fading menus and their state.

use super::*;
use crate::store::model::NodeId;

/// A minimal right-click context menu: just "Open in new tab" for a link,
/// positioned where the click landed — see `manifold_view_protocol::ViewToHost::
/// ContextMenuLink`'s doc comment for why this is the one action
/// implemented rather than a full copy/paste/inspect menu. `x`/`y` are
/// already in window (not content-area) point space — see where this is
/// constructed in `drain_tab_events`.
#[derive(Clone)]
pub(crate) struct ContextMenuState {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) href: String,
}

/// A popup driven by `open` (`Some` while showing) that fades out instead of vanishing the instant `open` goes back
/// to `None`: the position and content it needs during the fade-out would otherwise disappear the same frame it
/// "closes". `cache` remembers the last real value.
#[derive(Debug)]
pub(crate) struct Fading<T> {
    pub(crate) open: Option<T>,
    cache: Option<T>,
}

impl<T> Default for Fading<T> {
    fn default() -> Self {
        Fading { open: None, cache: None }
    }
}

impl<T: Clone> Fading<T> {
    /// `id` keys the fade animation (unique per popup). Polled every frame regardless of `open`, the same
    /// "continuous polling" the toolbar dropdowns' own fade needs: calling `ctx.animate_bool` only while `open` is
    /// `Some` would leave real elapsed time untracked while it is `None`, and the next open would jump straight to
    /// visible instead of easing in. Returns `None` once the fade has finished (nothing left to draw), else the
    /// value (`open`'s own while it is `Some`, otherwise the frozen cache) and the fade (0.0-1.0).
    pub(crate) fn state(&mut self, ctx: &egui::Context, id: egui::Id) -> Option<(T, f32)> {
        let fade = ctx.animate_bool(id, self.open.is_some());
        if let Some(value) = &self.open {
            self.cache = Some(value.clone());
        }
        if fade <= 0.0 {
            return None;
        }
        self.cache.clone().map(|value| (value, fade))
    }
}

/// The popups anchored below a toolbar or Settings button. At most one is
/// open (`App::open_popup`), so opening one closes whichever was open,
/// whether a button, a headless click or a shortcut opened it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum AnchoredPopup {
    RecentlyClosed,
    Bookmarks,
    History,
    Downloads,
    TabActions,
    DefaultViewKind,
    CurrentViewKind,
    SplitView,
    /// The Back chip's branch list (trail spec decision 8).
    BackBranches,
    ForwardBranches,
}

/// Opens `which`, or closes it if it's the one open.
pub(crate) fn toggle_popup(open: &mut Option<AnchoredPopup>, which: AnchoredPopup) {
    *open = if *open == Some(which) { None } else { Some(which) };
}

/// Closes `which` if it's the one open. A popup still fading out runs its
/// own close checks (a click outside it, say), and those mustn't close the
/// popup that replaced it.
pub(crate) fn close_popup(open: &mut Option<AnchoredPopup>, which: AnchoredPopup) {
    if *open == Some(which) {
        *open = None;
    }
}

/// A node's right-click menu: where it is open, the page showing instead of its items, and the copy that outlives it
/// while it fades. The page belongs to the menu that set it: opening a menu starts on its items.
#[derive(Default)]
pub(crate) struct NodeMenu {
    /// `(node_id, screen_pos)` while the menu is open.
    pub(crate) open: Option<(NodeId, egui::Pos2)>,
    /// The page showing instead of the items (`MenuPage`). Left as it is when the menu closes, so the fade-out keeps
    /// the page the click was on; `open` drops it, and so does the end of the fade (`App::build_node_menu`).
    pub(crate) page: Option<crate::node_menu::MenuPage>,
    /// `open` with the card's frozen `NodeMenuView`, set each frame the menu is open, so `build_node_menu` keeps
    /// drawing it (fading out, not vanishing) for a few frames after `open` itself goes back to `None`.
    pub(crate) fade: Fading<(NodeId, egui::Pos2, NodeMenuView)>,
}

impl NodeMenu {
    /// Opens the menu on `id` at `pos`, on its items.
    pub(crate) fn open(&mut self, id: NodeId, pos: egui::Pos2) {
        self.open = Some((id, pos));
        self.page = None;
    }

    /// Closes the menu; its page stays for the fade-out.
    pub(crate) fn close(&mut self) {
        self.open = None;
    }

    /// The open menu's page, `None` for its items; ignored when no menu is open.
    pub(crate) fn show_page(&mut self, page: Option<crate::node_menu::MenuPage>) {
        if self.open.is_some() {
            self.page = page;
        }
    }

    /// Drops the menu `id` has open: it is gone.
    pub(crate) fn forget(&mut self, id: NodeId) {
        if self.open.as_ref().is_some_and(|(n, _)| *n == id) {
            self.open = None;
            self.page = None;
        }
    }
}

/// The popups and menus on screen (`App::menus`), each fading menu with the copy that outlives it.
#[derive(Default)]
pub(crate) struct MenuPopups {
    /// The right-click context menu of a node's card -- one for every kind, so at most one menu is ever open by
    /// construction (`NodeMenu`). Right-click is otherwise the rectangle-select gesture over the graph panel (see that
    /// gesture's own doc comment on `handle_rect_select`); a press landing on any card opens this instead of starting a
    /// marquee. Drawn by `App::build_node_menu`, which dispatches on the node's kind.
    pub(crate) node_menu: NodeMenu,
    /// The right-click menu of a trail strip page: its virtual card id and the screen point (`App::build_trail_menu`).
    pub(crate) trail_menu: Option<(crate::card_id::CardId, egui::Pos2)>,
    /// The Back/Forward branch popup's camera and which popup it belongs to: recentred every time the popup opens
    /// (`popup_centres_cursor`), kept while it stays open.
    pub(crate) trail_canvas: Option<(AnchoredPopup, crate::graph_ui::GraphCamera)>,
    /// The minimal right-click context menu, if currently showing — see
    /// `ContextMenuState`'s own doc comment.
    pub(crate) context_menu: Fading<ContextMenuState>,
    /// The `Ctrl+K` omnibar popup, if open; its state is `App::omni_popup` (see `App::build_omni_popup`).
    pub(crate) omni_popup: Fading<()>,
    /// Which popup anchored below a toolbar or Settings button is open, if
    /// any; one at a time. See `AnchoredPopup`, `toggle_popup` and
    /// `close_popup`.
    pub(crate) open_popup: Option<AnchoredPopup>,
    /// `F1` / headless `TOGGLESHORTCUTSHELP` -- whether the keyboard-
    /// shortcut reference popup is showing. See `App::build_shortcuts_help`.
    pub(crate) shortcuts_help_open: bool,
}

impl App {
    /// `which`'s anchor rect and fade (0.0-1.0), or `None` once it has
    /// faded out or while its button isn't drawn. Call it every frame,
    /// open or not: egui's animation manager needs continuous polling to
    /// track real elapsed time, and polling only while open made a
    /// re-open jump straight to visible instead of easing in.
    pub(crate) fn anchored_popup_fade(
        &self,
        ctx: &egui::Context,
        which: AnchoredPopup,
        anchor: Option<egui::Rect>,
    ) -> Option<(egui::Rect, f32)> {
        let open = self.menus.open_popup == Some(which);
        let fade = ctx.animate_bool(egui::Id::new(("anchored_popup_fade", which)), open);
        if fade > 0.0 {
            anchor.map(|rect| (rect, fade))
        } else {
            None
        }
    }

    /// Whether a popup, menu-like surface or confirm owns the keyboard, so the page behind must not act on a press
    /// that closes it (Escape) or on its own shortcuts.
    pub(crate) fn modal_open(&self) -> bool {
        self.popup_open()
            || self.confirm_open()
            || self.menus.node_menu.open.is_some()
            || self.menus.shortcuts_help_open
    }

    pub(crate) fn popup_opener_key(which: AnchoredPopup) -> egui::Id {
        egui::Id::new(("popup_opener", which))
    }

    /// The settings page's button showing `kind`, opening engine picker `which`, labelled in the engine's color.
    pub(crate) fn view_opener_button(
        &mut self,
        ui: &mut egui::Ui,
        which: AnchoredPopup,
        kind: ViewKind,
    ) -> egui::Response {
        let unavailable = matches!(self.view_status(kind), crate::view_install::ViewStatus::Unavailable(_));
        let color = if unavailable { palette(ui.visuals()).text_weak } else { view_color(kind) };
        // The caret says it opens a list (the `▼` glyph is missing from the font; Phosphor's draws).
        let label = format!("{} {}", view_label(kind), egui_phosphor::light::CARET_DOWN);
        let response = ui.button(egui::RichText::new(label).color(color));
        self.popup_opener_clicked(ui, which, &response);
        response
    }

    /// The button that opens popup `which`: toggles it when clicked and remembers itself, so
    /// `build_ui` can hand focus back to it when the popup closes from the keyboard. Returns
    /// whether the popup was just opened.
    pub(crate) fn popup_opener_clicked(
        &mut self,
        ui: &egui::Ui,
        which: AnchoredPopup,
        button: &egui::Response,
    ) -> bool {
        ui.data_mut(|d| d.insert_temp(Self::popup_opener_key(which), button.id));
        if button.clicked() {
            toggle_popup(&mut self.menus.open_popup, which);
        }
        button.clicked() && self.menus.open_popup == Some(which)
    }

    /// Whether an anchored popup is open: Escape closes it (`build_ui`), so the settings page and
    /// Tab view must not also act on the same press.
    pub(crate) fn popup_open(&self) -> bool {
        self.menus.open_popup.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shortcut reference owns the keyboard like a popup: the page behind must not act on the Escape that closes it.
    #[test]
    fn the_shortcuts_help_owns_the_keyboard() {
        let mut app = App::default();
        app.menus.shortcuts_help_open = true;
        assert!(app.modal_open());
    }

    #[test]
    fn opening_a_popup_replaces_the_open_one() {
        let mut open = None;
        toggle_popup(&mut open, AnchoredPopup::Bookmarks);
        assert_eq!(open, Some(AnchoredPopup::Bookmarks));
        toggle_popup(&mut open, AnchoredPopup::History);
        assert_eq!(open, Some(AnchoredPopup::History));
        toggle_popup(&mut open, AnchoredPopup::History);
        assert_eq!(open, None);
    }

    #[test]
    fn a_fading_popup_cannot_close_its_replacement() {
        let mut open = Some(AnchoredPopup::History);
        // Bookmarks is still fading out and sees the click outside it.
        close_popup(&mut open, AnchoredPopup::Bookmarks);
        assert_eq!(open, Some(AnchoredPopup::History));
        close_popup(&mut open, AnchoredPopup::History);
        assert_eq!(open, None);
    }
}
