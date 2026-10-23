//! Per-engine presentation: labels, colors, tiers, pros/cons and the engine
//! picker popup.

use super::*;
use crate::view_install::ViewStatus;

pub(crate) fn view_label(kind: ViewKind) -> &'static str {
    kind.desc().label
}

/// The rendering engine `kind` embeds and which other hosts share it, from the family tags: "Blink engine, shared
/// with Blink (CEF)", or "WebKit engine (only host)".
pub(crate) fn engine_line(kind: ViewKind) -> String {
    use crate::caps::Tag;
    let Some(&(family, name)) = Tag::FAMILIES.iter().find(|(tag, _)| kind.has(*tag)) else { return String::new() };
    let others: Vec<&str> = ViewKind::ALL.into_iter().filter(|k| *k != kind && k.has(family)).map(view_label).collect();
    if others.is_empty() {
        format!("{name} engine (only host)")
    } else {
        format!("{name} engine, shared with {}", others.join(", "))
    }
}

/// What the disk holds of `kind`'s downloadable parts, for the headless `VIEWSTATE` and `EXPORTSTATE`:
/// "host ready", and for CEF also "runtime missing" (Ready, Missing or Stale each).
pub(crate) fn on_disk_text(kind: ViewKind, disk: crate::view_install::OnDisk) -> String {
    use crate::view_install::RuntimeOnDisk;
    let word = |part: RuntimeOnDisk| match part {
        RuntimeOnDisk::Ready => "ready",
        RuntimeOnDisk::Missing => "missing",
        RuntimeOnDisk::Stale => "stale",
    };
    let mut text = format!("host {}", word(disk.host));
    if crate::view_install::has_runtime(kind) {
        text.push_str(&format!(", runtime {}", word(disk.runtime)));
    }
    text
}

/// How an engine state's label is colored: weak text, or the danger red for a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Weak,
    Danger,
}

impl Tone {
    pub(crate) fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            Tone::Weak => crate::theme::palette(visuals).text_weak,
            Tone::Danger => crate::theme::danger(visuals),
        }
    }
}

/// `bytes` in decimal megabytes ("172 MB"), the unit the CEF CDN lists its builds in.
pub(crate) fn megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes as f64 / 1e6).round())
}

/// The one-line state every engine list shows, and its tone. `Installed` says so here; the pickers and
/// cards leave it out (an installed engine has no marker beyond its name), Settings shows it.
pub(crate) fn view_status_label(state: &ViewStatus) -> (String, Tone) {
    match state {
        ViewStatus::Installed => ("Installed".into(), Tone::Weak),
        ViewStatus::NotInstalled { size } => (format!("Not installed \u{b7} {}", megabytes(*size)), Tone::Weak),
        ViewStatus::Installing { received, total } if *total > 0 => {
            let percent = (*received as f64 / *total as f64 * 100.0).floor().clamp(0.0, 100.0);
            (format!("Installing {percent}%"), Tone::Weak)
        }
        ViewStatus::Installing { .. } => ("Installing".into(), Tone::Weak),
        ViewStatus::UpdateNeeded { size } => (format!("Update needed \u{b7} {}", megabytes(*size)), Tone::Weak),
        ViewStatus::Failed(reason) => (format!("Failed: {reason}"), Tone::Danger),
        ViewStatus::Custom(path) => (format!("Custom: {}", path.display()), Tone::Weak),
        ViewStatus::Unavailable(reason) => (format!("Unavailable: {reason}"), Tone::Weak),
    }
}

/// What choosing an engine in a picker does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickOutcome {
    /// Use it.
    Switch(ViewKind),
    /// Not usable yet: ask to install it (the confirm names the size). Never a silent switch.
    AskInstall(ViewKind),
    /// Its install is already running; nothing to switch to yet.
    AlreadyInstalling(ViewKind),
    /// This platform cannot run it (no pin, wrong OS): the click only explains, with the reason.
    Unavailable(ViewKind, String),
}

impl PickOutcome {
    /// The toast an `Unavailable` pick shows: the engine and the same reason its state line gives.
    pub(crate) fn unavailable_toast(kind: ViewKind, reason: &str) -> String {
        format!("{} is unavailable: {reason}", view_label(kind))
    }
}

pub(crate) fn pick_outcome(kind: ViewKind, state: &ViewStatus) -> PickOutcome {
    match state {
        ViewStatus::Installed | ViewStatus::Custom(_) => PickOutcome::Switch(kind),
        ViewStatus::Installing { .. } => PickOutcome::AlreadyInstalling(kind),
        ViewStatus::Unavailable(reason) => PickOutcome::Unavailable(kind, reason.clone()),
        ViewStatus::NotInstalled { .. } | ViewStatus::UpdateNeeded { .. } | ViewStatus::Failed(_) => {
            PickOutcome::AskInstall(kind)
        }
    }
}

/// A pending confirm before a large or destructive engine action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewConfirm {
    Download(ViewKind),
    Remove(ViewKind),
}

/// The state line of an engine card or Settings row: its label, plus a progress bar while installing.
pub(crate) fn view_status_line(ui: &mut egui::Ui, state: &ViewStatus) {
    let (text, tone) = view_status_label(state);
    ui.label(egui::RichText::new(text).small().color(tone.color(ui.visuals())));
    if let ViewStatus::Installing { received, total } = state {
        install_progress_bar(ui, *received, *total);
    }
}

/// The download card's progress look (`build_downloads_popup`): a bar with bytes received over total.
pub(crate) fn install_progress_bar(ui: &mut egui::Ui, received: u64, total: u64) {
    let fraction = if total > 0 { (received as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
    ui.add(
        egui::ProgressBar::new(fraction)
            .text(format!("{} / {}", megabytes(received), megabytes(total)))
            // Not INFINITY: in the Settings column that is the whole window, not the 480-point page.
            .desired_width(ui.available_width().min(320.0)),
    );
}

/// The engine's identifying color (`ViewDesc::color`): the small dot/badge wherever a tab's engine matters at a
/// glance, the tab strip and the graph node's engine badge.
pub(crate) fn view_color(kind: ViewKind) -> egui::Color32 {
    kind.desc().color
}

/// A small filled circle identifying a tab's engine at a glance — used
/// inline in a `ui.horizontal` layout (the tab strip, the engine picker's
/// own dropdown rows), so it allocates its own space rather than being a
/// raw `painter()` call positioned by hand.
pub(crate) fn view_dot(ui: &mut egui::Ui, kind: ViewKind, diameter: f32) {
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), diameter / 2.0, view_color(kind));
}

/// Where each engine actually stands in this specific app, not a general
/// verdict on the upstream project -- "Experimental" for Servo is about
/// *this app's own* software-rendering-only integration and Servo's
/// still-incomplete standards coverage, not a claim about Servo itself.
/// Ordered `Recommended` first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ViewTier {
    Recommended,
    Experimental,
    Limited,
}

impl ViewTier {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ViewTier::Recommended => "Recommended",
            ViewTier::Experimental => "Experimental",
            ViewTier::Limited => "Limited support",
        }
    }

    /// A muted, tier-coded badge color -- deliberately not reusing
    /// the accent and danger colors (both already mean something else
    /// elsewhere: "focused" and "destructive") or any `view_color`
    /// (that axis is "which engine," this one is "how ready is it").
    pub(crate) fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            ViewTier::Recommended => crate::theme::palette(visuals).success,
            ViewTier::Experimental => crate::theme::accent(visuals),
            ViewTier::Limited => crate::theme::danger(visuals),
        }
    }
}

/// One engine's own card inside `build_view_picker_popup` -- name, color
/// dot, tier badge, then its pros/cons side by side. Returns whether this
/// card's response (the whole card is the hit target, not just a button
/// within it, so picking an engine doesn't require precision-clicking a
/// small label). Keyboard focus, arrows and Enter are the popup's job.
pub(crate) fn draw_view_card(
    ui: &mut egui::Ui,
    kind: ViewKind,
    is_selected: bool,
    state: &ViewStatus,
) -> egui::Response {
    // An engine this platform cannot run is dimmed to weak text and only explains itself when clicked.
    let dim = matches!(state, ViewStatus::Unavailable(_));
    let frame_response = egui::Frame::default()
        .fill(if is_selected { ui.visuals().widgets.hovered.bg_fill } else { ui.visuals().widgets.inactive.bg_fill })
        .corner_radius(crate::theme::RADIUS)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            // The popup's full width, not just as wide as the card's text.
            ui.set_min_width(ui.available_width());
            let palette = crate::theme::palette(ui.visuals());
            ui.horizontal(|ui| {
                view_dot(ui, kind, 10.0);
                ui.label(egui::RichText::new(view_label(kind)).color(if dim {
                    palette.text_weak
                } else {
                    palette.text
                }));
                if is_selected {
                    ui.label(egui_phosphor::light::CHECK_CIRCLE);
                }
                let tier = kind.desc().tier;
                ui.label(egui::RichText::new(tier.label()).small().color(if dim {
                    palette.text_weak
                } else {
                    tier.color(ui.visuals())
                }));
            });
            if *state != ViewStatus::Installed {
                view_status_line(ui, state);
            }
            ui.label(egui::RichText::new(engine_line(kind)).small().color(palette.text_weak));
            ui.add_space(4.0);
            for pro in kind.desc().pros {
                let text = egui::RichText::new(format!("+ {pro}")).small();
                ui.label(if dim { text.color(palette.text_weak) } else { text });
            }
            for con in kind.desc().cons {
                ui.label(egui::RichText::new(format!("\u{2212} {con}")).small().color(palette.text_weak));
            }
        });
    // Interacted on *after* the frame's content is laid out, using its own
    // resulting rect -- interacting on `ui.max_rect()` before any content
    // exists gives a hit region sized to whatever was allocated so far
    // (near-zero at that point), not the card's real, final size. Found
    // live: clicking squarely on a card's own visible pros/cons text did
    // nothing, since only a sliver at the very top was ever actually
    // clickable.
    let id = ui.id().with(("view_card", kind as u8));
    if dim {
        // `Sense::CLICK`, not `click()`: still clickable (to explain), but not focusable, so the keyboard
        // skips it; the default cursor says it does nothing. (`add_enabled` would swallow the click.)
        return ui.interact(frame_response.response.rect, id, egui::Sense::CLICK);
    }
    ui.interact(frame_response.response.rect, id, egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The shared engine-picker popup -- see `draw_view_card`'s own doc
/// comment for each card's content. Used by all three places this app lets
/// you choose an engine (Settings' "Default starting engine," Settings'
/// "Current Tab" swap, and the Tab toolbar's "Split" picker), each through
/// `App::build_anchored_view_picker`, replacing what used to be three
/// separate plain `egui::ComboBox`es with no tier/tradeoff information at
/// all. Returns the popup's own drawn rect (for the caller's
/// `clicked_outside_popup` check) and whichever engine was clicked this
/// frame, if any. `interactable` is whether the popup is open, same as
/// `build_list_popup_frame`'s.
pub(crate) fn build_view_picker_popup(
    ctx: &egui::Context,
    id: &str,
    anchor_rect: egui::Rect,
    selected: Option<ViewKind>,
    state_of: &dyn Fn(ViewKind) -> ViewStatus,
    fade: f32,
    interactable: bool,
) -> (egui::Rect, Option<ViewKind>) {
    let mut picked = None;
    // Keyboard (`list_row_nav`): the selected card is focused when the popup opens, Up/Down move between
    // cards, Enter picks (a focused click widget clicks on Enter), and Tab stays inside.
    let area_rect = egui::Area::new(egui::Id::new(id))
        .fixed_pos(anchor_rect.left_bottom())
        .order(egui::Order::Foreground)
        .interactable(interactable)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            popup_frame(ui).show(ui, |ui| {
                ui.set_width(340.0);
                // Same "cap height, let it scroll" shape the other
                // toolbar list popups (Bookmarks/History/Downloads)
                // already use -- four cards this rich run well past a
                // short window's own height otherwise, with the last
                // one (WebKit, usually) silently unreachable past the
                // window's bottom edge (confirmed live).
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    // Unavailable cards are not focusable, so the keyboard skips them: the popup opens on
                    // the selected card, or the first available one when the selection is not usable.
                    let usable = |kind: ViewKind| !matches!(state_of(kind), ViewStatus::Unavailable(_));
                    let focus_on = selected
                        .filter(|kind| usable(*kind))
                        .or_else(|| ViewKind::picker_order().into_iter().find(|kind| usable(*kind)))
                        .unwrap_or(ViewKind::picker_order()[0]);
                    for (i, kind) in ViewKind::picker_order().into_iter().enumerate() {
                        if i > 0 {
                            ui.add_space(6.0);
                        }
                        let card = draw_view_card(ui, kind, selected == Some(kind), &state_of(kind));
                        if usable(kind) {
                            nav_row(ui, &card);
                            if focus_on == kind {
                                nav_row_opens_on(ui, &card);
                            }
                        }
                        if card.clicked() {
                            picked = Some(kind);
                        }
                        if i > 0 {
                            ui.add_space(2.0);
                        }
                    }
                });
                list_row_nav(ui, interactable);
            });
        })
        .response
        .rect;
    (area_rect, picked)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_engine_line_names_the_family_and_who_shares_it() {
        assert_eq!(super::engine_line(ViewKind::Cef), "Blink engine, shared with WebView2");
        assert_eq!(super::engine_line(ViewKind::Webview2), "Blink engine, shared with Blink (CEF)");
        assert_eq!(super::engine_line(ViewKind::Webkit), "WebKit engine (only host)");
        assert_eq!(super::engine_line(ViewKind::Servo), "Servo engine (only host)");
    }

    use super::*;

    #[test]
    fn tier_and_tone_colors_come_from_the_visuals() {
        let visuals = crate::theme::testutil::ctx(true).global_style().visuals.clone();
        assert_eq!(ViewTier::Recommended.color(&visuals), crate::theme::palette(&visuals).success);
        assert_eq!(ViewTier::Experimental.color(&visuals), crate::theme::accent(&visuals));
        assert_eq!(Tone::Danger.color(&visuals), crate::theme::danger(&visuals));
        assert_eq!(Tone::Weak.color(&visuals), crate::theme::palette(&visuals).text_weak);
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Runs one frame of the picker (open, `selected` highlighted) with `events`, returning its pick.
    fn frame(ctx: &egui::Context, selected: ViewKind, events: Vec<egui::Event>) -> Option<ViewKind> {
        frame_with(ctx, selected, events, &|_| ViewStatus::Installed)
    }

    fn frame_with(
        ctx: &egui::Context,
        selected: ViewKind,
        events: Vec<egui::Event>,
        state_of: &dyn Fn(ViewKind) -> ViewStatus,
    ) -> Option<ViewKind> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 900.0))),
            events,
            ..Default::default()
        };
        let mut picked = None;
        let mut output = ctx.run_ui(input, |ui| {
            let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(60.0, 20.0));
            picked = build_view_picker_popup(ui.ctx(), "test_picker", anchor, Some(selected), state_of, 1.0, true).1;
        });
        output.textures_delta.clear(); // nothing uploads the font atlas here
        picked
    }

    #[test]
    fn state_labels_cover_all_six_states() {
        let label = |s| view_status_label(&s);
        assert_eq!(label(ViewStatus::Installed), ("Installed".to_string(), Tone::Weak));
        assert_eq!(
            label(ViewStatus::NotInstalled { size: 171_619_649 }),
            ("Not installed \u{b7} 172 MB".to_string(), Tone::Weak)
        );
        assert_eq!(
            label(ViewStatus::Installing { received: 34, total: 100 }),
            ("Installing 34%".to_string(), Tone::Weak)
        );
        assert_eq!(label(ViewStatus::Installing { received: 0, total: 0 }).0, "Installing");
        assert_eq!(
            label(ViewStatus::UpdateNeeded { size: 171_619_649 }),
            ("Update needed \u{b7} 172 MB".to_string(), Tone::Weak)
        );
        assert_eq!(
            label(ViewStatus::Failed("download failed: x".into())),
            ("Failed: download failed: x".to_string(), Tone::Danger)
        );
        assert_eq!(
            label(ViewStatus::Custom(std::path::PathBuf::from("D:/cef"))),
            ("Custom: D:/cef".to_string(), Tone::Weak)
        );
    }

    #[test]
    fn the_disk_text_names_the_parts_the_view_has() {
        use crate::view_install::{OnDisk, RuntimeOnDisk};
        let disk = OnDisk { host: RuntimeOnDisk::Ready, runtime: RuntimeOnDisk::Stale };
        assert_eq!(on_disk_text(ViewKind::Servo, disk), "host ready");
        assert_eq!(on_disk_text(ViewKind::Cef, disk), "host ready, runtime stale");
        assert_eq!(on_disk_text(ViewKind::Cef, OnDisk::both(RuntimeOnDisk::Missing)), "host missing, runtime missing");
    }

    /// An engine that isn't usable asks to install; it never switches silently.
    #[test]
    fn picking_an_unavailable_view_asks_to_install() {
        let kind = ViewKind::Cef;
        let ask = PickOutcome::AskInstall(kind);
        assert_eq!(pick_outcome(kind, &ViewStatus::Installed), PickOutcome::Switch(kind));
        assert_eq!(pick_outcome(kind, &ViewStatus::Custom("D:/cef".into())), PickOutcome::Switch(kind));
        assert_eq!(pick_outcome(kind, &ViewStatus::NotInstalled { size: 1 }), ask);
        assert_eq!(pick_outcome(kind, &ViewStatus::UpdateNeeded { size: 1 }), ask);
        assert_eq!(pick_outcome(kind, &ViewStatus::Failed("x".into())), ask);
        assert_eq!(
            pick_outcome(kind, &ViewStatus::Installing { received: 1, total: 2 }),
            PickOutcome::AlreadyInstalling(kind)
        );
    }

    /// The picker opens focused on the selected card; Down moves to the next one and Enter picks it.
    #[test]
    fn arrows_and_enter_pick_a_view_from_the_keyboard() {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let selected = ViewKind::picker_order()[1];
        let next = ViewKind::picker_order()[2];
        for _ in 0..3 {
            assert_eq!(frame(&ctx, selected, vec![]), None);
        }
        assert_eq!(frame(&ctx, selected, vec![key(egui::Key::ArrowDown)]), None);
        assert_eq!(frame(&ctx, selected, vec![]), None);
        assert_eq!(frame(&ctx, selected, vec![key(egui::Key::Enter)]), Some(next));
    }

    /// An unavailable card is skipped by Down (and by the opening focus), so Enter never lands on it.
    #[test]
    fn the_keyboard_skips_unavailable_cards() {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let skipped = ViewKind::picker_order()[2];
        let state_of = |kind: ViewKind| {
            if kind == skipped {
                ViewStatus::Unavailable("needs Windows".into())
            } else {
                ViewStatus::Installed
            }
        };
        let (selected, after) = (ViewKind::picker_order()[1], ViewKind::picker_order()[3]);
        for _ in 0..3 {
            assert_eq!(frame_with(&ctx, selected, vec![], &state_of), None);
        }
        assert_eq!(frame_with(&ctx, selected, vec![key(egui::Key::ArrowDown)], &state_of), None);
        assert_eq!(frame_with(&ctx, selected, vec![], &state_of), None);
        assert_eq!(frame_with(&ctx, selected, vec![key(egui::Key::Enter)], &state_of), Some(after));
        // Selection on an unavailable engine: the popup opens on the first usable card instead.
        let ctx = egui::Context::default();
        apply_style(&ctx);
        for _ in 0..3 {
            frame_with(&ctx, skipped, vec![], &state_of);
        }
        assert_eq!(
            frame_with(&ctx, skipped, vec![key(egui::Key::Enter)], &state_of),
            Some(ViewKind::picker_order()[0])
        );
    }

    #[test]
    fn an_unavailable_pick_carries_its_reason() {
        let state = ViewStatus::Unavailable("needs Windows".into());
        let outcome = pick_outcome(ViewKind::Webview2, &state);
        assert_eq!(outcome, PickOutcome::Unavailable(ViewKind::Webview2, "needs Windows".into()));
        assert_eq!(
            PickOutcome::unavailable_toast(ViewKind::Webview2, "needs Windows"),
            "WebView2 is unavailable: needs Windows"
        );
    }

    /// Tab must not leave the popup: Enter after it still picks the focused card.
    #[test]
    fn tab_keeps_focus_on_the_card() {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let selected = ViewKind::picker_order()[1];
        for _ in 0..3 {
            frame(&ctx, selected, vec![]);
        }
        frame(&ctx, selected, vec![key(egui::Key::Tab)]);
        frame(&ctx, selected, vec![]);
        assert_eq!(frame(&ctx, selected, vec![key(egui::Key::Enter)]), Some(selected));
    }

    #[test]
    fn the_install_bar_has_a_finite_width_in_a_popup_that_sizes_itself() {
        // An infinite bar made the picker's auto-sized popup NaN-wide, which egui panics on.
        let ctx = egui::Context::default();
        let state = ViewStatus::Installing { received: 0, total: 10 };
        for _ in 0..2 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                egui::Area::new(egui::Id::new("bar")).show(ui.ctx(), |ui| view_status_line(ui, &state));
            });
            output.textures_delta.clear(); // nothing uploads the font atlas here
        }
    }
}
