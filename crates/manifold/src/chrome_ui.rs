//! Toasts and the per-frame UI entry points of `App`.

use super::*;
use crate::store::model::NodeId;

impl App {
    /// Queues `message` behind whatever toast is showing. `build_save_toast`
    /// takes it off the queue when it first draws it, and its
    /// `SAVE_TOAST_DURATION` starts then, not here: a toast queued during
    /// startup (an unreadable settings/session file) used to spend part of
    /// its time before the first frame.
    pub(crate) fn queue_toast(&mut self, message: String) {
        self.toasts.save_toast_queue.push_back(message);
    }

    /// A spawn-failure toast: held a moment so failures with one cause (engine and `error`) read as one message, counted
    /// by distinct intent (`url`, engine, `group`; `failure_toasts`).
    pub(crate) fn queue_failure_toast(
        &mut self,
        kind: ViewKind,
        url: &str,
        group: Option<NodeId>,
        error: &str,
        message: String,
    ) {
        let intent =
            format!("{url}\u{1f}{}\u{1f}{}", view_label(kind), group.map(|g| g.to_string()).unwrap_or_default());
        self.toasts.failure_notices.push(std::time::Instant::now(), view_label(kind), error, &intent, message);
    }

    /// `url` short enough for a toast: its first `TOAST_URL_CHARS` characters and an ellipsis. A toast sizes to its
    /// text, and a `data:` URL can be megabytes.
    pub(crate) fn toast_url(url: &str) -> String {
        const TOAST_URL_CHARS: usize = 80;
        match url.char_indices().nth(TOAST_URL_CHARS) {
            Some((at, _)) => format!("{}\u{2026}", &url[..at]),
            None => url.to_string(),
        }
    }

    /// A brief bottom-left toast, for `self.toasts.save_toast` — cleared once
    /// `SAVE_TOAST_DURATION` elapses. No dismiss interaction (unlike the
    /// context menu): it just times out, and clicks pass through it to the
    /// card or page underneath. Fades in over its first
    /// `TOAST_FADE_IN` and out over its last `TOAST_FADE_OUT` rather than
    /// popping on and cutting off instantly -- driven directly off
    /// `shown_at.elapsed()` (this app's winit loop runs `ControlFlow::Poll`,
    /// so it's already redrawing every frame regardless; no extra
    /// `request_repaint` needed for a time-based fade to animate smoothly),
    /// not `ctx.animate_bool` -- unlike the toggle-bool dropdown popups,
    /// this already has a real wall-clock lifetime to derive opacity from
    /// directly. Despite the field's own name, the message is the caller's
    /// full text now, not just a path this function used to prepend
    /// "Saved " to -- every original "Saved <path>" call site now includes
    /// that itself, freeing this up for unrelated short-lived messages too
    /// (e.g. a crash/respawn notice, see `ViewToHost` handling below).
    fn build_save_toast(&mut self, ui: &mut egui::Ui) {
        const TOAST_FADE_IN: std::time::Duration = std::time::Duration::from_millis(150);
        const TOAST_FADE_OUT: std::time::Duration = std::time::Duration::from_millis(400);
        for message in self.toasts.failure_notices.flush(std::time::Instant::now()) {
            self.queue_toast(message);
        }
        if self.toasts.failure_notices.is_pending() {
            ui.ctx().request_repaint_after(failure_toasts::WINDOW);
        }
        if self.toasts.save_toast.is_none() {
            self.toasts.save_toast =
                self.toasts.save_toast_queue.pop_front().map(|message| (message, std::time::Instant::now()));
        }
        let Some((message, shown_at)) = &self.toasts.save_toast else { return };
        let elapsed = shown_at.elapsed();
        if elapsed >= SAVE_TOAST_DURATION {
            if self.toasts.undo.as_ref().is_some_and(|(shown, _, _)| shown == message) {
                self.toasts.undo = None;
            }
            self.toasts.save_toast = None;
            return;
        }
        ui.ctx().request_repaint();
        let fade_in = (elapsed.as_secs_f32() / TOAST_FADE_IN.as_secs_f32()).min(1.0);
        let remaining = SAVE_TOAST_DURATION.saturating_sub(elapsed);
        let fade_out = (remaining.as_secs_f32() / TOAST_FADE_OUT.as_secs_f32()).min(1.0);
        let opacity = fade_in.min(fade_out);
        let message = message.clone();
        // A toast that can be undone takes clicks on its Undo button; any other lets them through to what is under it.
        let undoable = self.toasts.undo.as_ref().is_some_and(|(shown, _, _)| *shown == message);
        let mut undo_clicked = false;
        egui::Area::new(egui::Id::new("save_toast"))
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
            .order(egui::Order::Foreground)
            .interactable(undoable)
            .show(ui.ctx(), |ui| {
                ui.set_opacity(opacity);
                // Same ACCENT-tinted glow the graph cards use, tying the
                // toast into that visual language -- but this Frame
                // auto-sizes to its own label, so the rect `card_glow_
                // shapes` needs isn't known until *after* `.show()` below
                // lays it out. Reserve two placeholder shapes now (glow is
                // always two layers) so they still paint *behind* the
                // frame by paint order, then fill them in for real once
                // `response.rect` exists -- see `card_glow_shapes`'s own
                // doc comment for why this indirection, unlike graph
                // cards, is needed here specifically.
                let glow_slots = [ui.painter().add(egui::Shape::Noop), ui.painter().add(egui::Shape::Noop)];
                let response = bar_frame(ui).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(message);
                        if let Some((_, _, redo)) = self.toasts.undo.as_ref().filter(|_| undoable) {
                            undo_clicked = ui.button(if *redo { "Redo" } else { "Undo" }).clicked();
                        }
                    });
                });
                let shapes = card_glow_shapes(
                    response.response.rect,
                    8.0,
                    accent(ui.visuals()),
                    1.0,
                    1.0,
                    palette(ui.visuals()).shadow,
                    0.0,
                );
                for (slot, shape) in glow_slots.into_iter().zip(shapes) {
                    ui.painter().set(slot, shape);
                }
            });
        if undo_clicked {
            if let Some((_, serial, redo)) = self.toasts.undo.take() {
                self.step_history(Some(serial), redo);
            }
            self.toasts.save_toast = None;
        }
    }

    /// One frame of UI. An open anchored popup takes Escape (the views skip theirs while one is
    /// open, `popup_open`), and when it closes from the keyboard focus goes back to its opening button.
    pub(crate) fn build_ui(&mut self, ui: &mut egui::Ui) {
        crate::identity::begin_icon_frame(ui.ctx());
        self.view_ui.view_status_cache.borrow_mut().clear();
        let open_at_start = self.menus.open_popup;
        // An open page menu takes the Escape first (`build_trail_menu`); the popup under it stays.
        let escape = open_at_start.is_some()
            && self.menus.trail_menu.is_none()
            && ui.input(|i| i.key_pressed(egui::Key::Escape));
        self.build_ui_inner(ui);
        // A page menu no host drew this frame (the view changed under it) must not wait for the next host to appear.
        let drawn = ui.ctx().data(|d| d.get_temp::<u64>(egui::Id::new("trail_menu_frame")));
        if drawn != Some(ui.ctx().cumulative_frame_nr()) {
            self.menus.trail_menu = None;
        }
        if escape && self.menus.open_popup == open_at_start {
            self.menus.open_popup = None;
        }
        if let Some(which) = open_at_start.filter(|_| self.menus.open_popup.is_none()) {
            if !ui.input(|i| i.pointer.any_click()) {
                if let Some(button) = ui.data(|d| d.get_temp::<egui::Id>(App::popup_opener_key(which))) {
                    ui.memory_mut(|m| m.request_focus(button));
                }
            }
        }
    }

    pub(crate) fn build_ui_inner(&mut self, ui: &mut egui::Ui) {
        self.handle_global_shortcuts(ui);
        // The address bar is built only in Tab view, so leaving it by any
        // path ends the edit, as Escape does. Otherwise the dropdown and
        // typed text came back over whichever tab was shown next.
        if !self.showing_tab_view() && self.address_bar.editing() {
            self.address_bar.dismiss();
            self.address_bar.focused = false;
        }
        // Likewise a drag ends when its card stops being drawn (Graph view left, or the card left the level by
        // another replica's move): its release would never be seen, and the next one would save a stale pos.
        self.end_stale_drag(self.transition.is_some() || self.view_mode == ViewMode::Graph);
        // And pick-target mode, whose targets are drawn only there.
        self.end_stale_connect(self.view_mode == ViewMode::Graph);
        // And the rectangle selection, which only ever holds the tabs drawn on this level.
        self.end_stale_selection();
        // Every view, so a toast queued in Graph view (a crash/respawn
        // notice, an unreadable session file) is seen where it happened.
        self.build_save_toast(ui);
        // A `Transition` (either direction) always renders Graph view's
        // node layout as the backdrop — it's the one screen both a
        // shrinking-away Tab view and an expanding-toward Tab view have a
        // coherent shared position in (the target/source node). The actual
        // animated content quad itself is drawn separately, in
        // `render_frame` — see `transition_content_rects_points`.
        if self.transition.is_some() {
            self.build_graph_ui(ui);
            // A Tab's transition quad is a GPU pane (`content_panes_pts`);
            // an Image's is painted here, above the graph backdrop.
            if let Some((tex_id, rect)) = self.transition.as_ref().and_then(|t| self.image_transition_frame(t)) {
                // Clipped to the area below the toolbar `build_graph_ui`
                // (the backdrop, drawn just above) just painted -- at the
                // ToGraph end's t≈0, `rect` starts at the zoomed-in Image
                // view's own rect, which can reach above
                // `CHROME_HEIGHT_POINTS` into the toolbar row; this is a
                // plain `Order::Foreground` layer with no clip of its own,
                // so without this it painted over the toolbar for a frame.
                // Same height as both the Graph and Item toolbars
                // (`GRAPH_TOOLBAR_HEIGHT_POINTS == CHROME_HEIGHT_POINTS`),
                // so this is correct at either end of the animation.
                let screen = self.screen_size_points();
                let clip_rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, CHROME_HEIGHT_POINTS),
                    egui::vec2(screen.x, (screen.y - CHROME_HEIGHT_POINTS).max(0.0)),
                );
                ui.ctx()
                    .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("image_transition")))
                    .with_clip_rect(clip_rect)
                    .image(tex_id, rect, crate::card::FULL_UV, egui::Color32::WHITE);
            }
            return;
        }
        match self.view_mode {
            ViewMode::Item => self.build_item_ui(ui),
            ViewMode::Graph => self.build_graph_ui(ui),
            ViewMode::Settings => self.build_settings_page(ui),
        }
        self.build_quick_switcher(ui);
        self.build_confirm_popup(ui);
    }

    /// The "Keyboard Shortcuts (F1)" half of `render_universal_chip`.
    /// Returns its own rect, which the shortcuts-help popup anchors to.
    fn build_help_button(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let help_response =
            chip_icon_button(ui, egui::Id::new("icon_btn_shortcuts_help"), egui_phosphor::light::QUESTION)
                .named_tip("Keyboard Shortcuts (F1)");
        if help_response.clicked() {
            self.menus.shortcuts_help_open = !self.menus.shortcuts_help_open;
        }
        help_response.rect
    }

    /// The Settings-gear half of `render_universal_chip`.
    fn build_settings_button(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let response = chip_icon_button(ui, egui::Id::new("icon_btn_settings_gear"), egui_phosphor::light::GEAR)
            .named_tip("Settings");
        if response.clicked() {
            self.toggle_settings_page();
        }
        response.rect
    }

    /// Help + Settings, present in every toolbar (see the design spec's
    /// "universal chip" layer -- layer 1, drawn frame-level by
    /// `render_item_toolbar` for every Item-view kind, not just Tab).
    /// Returns the same rect `build_help_button` already returns -- the
    /// shortcuts-help popup anchors to that specific button, not to the
    /// chip as a whole, so callers that need it (`ItemToolbarFrame`'s own
    /// `shortcuts_help_button_rect`) keep working unchanged.
    ///
    /// Reads "Help, Settings" left to right whether `ui` is a left-to-right
    /// row (the Graph toolbar) or a right-to-left child (the Item toolbar's
    /// trailing cluster) -- see `in_reading_order`. Drawing both in one
    /// fixed call order once silently reversed them in the right-to-left
    /// caller (confirmed live: Tab and Graph view screenshotted side by side
    /// disagreed).
    pub(crate) fn render_universal_chip(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let mut help_rect = egui::Rect::NOTHING;
        toolbar_chip(ui, |ui| {
            let (help, settings) = in_reading_order(ui, self, Self::build_help_button, Self::build_settings_button);
            help_rect = help;
            help.union(settings)
        });
        help_rect
    }
}
