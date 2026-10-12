//! Per-engine presentation: labels, colors, tiers, pros/cons and the engine
//! picker popup.

use super::*;
use crate::engine_install::EngineState;

pub(crate) fn engine_label(kind: EngineKind) -> &'static str {
    kind.desc().label
}

/// What the disk holds of `kind`'s downloadable parts, for the headless `ENGINESTATE` and `EXPORTSTATE`:
/// "host ready", and for CEF also "runtime missing" (Ready, Missing or Stale each).
pub(crate) fn on_disk_text(kind: EngineKind, disk: crate::engine_install::OnDisk) -> String {
    use crate::engine_install::RuntimeOnDisk;
    let word = |part: RuntimeOnDisk| match part {
        RuntimeOnDisk::Ready => "ready",
        RuntimeOnDisk::Missing => "missing",
        RuntimeOnDisk::Stale => "stale",
    };
    let mut text = format!("host {}", word(disk.host));
    if crate::engine_install::has_runtime(kind) {
        text.push_str(&format!(", runtime {}", word(disk.runtime)));
    }
    text
}

/// How an engine state's label is colored: weak text, or `DANGER_COLOR` for a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Weak,
    Danger,
}

impl Tone {
    pub(crate) fn color(self) -> egui::Color32 {
        match self {
            Tone::Weak => TEXT_WEAK,
            Tone::Danger => DANGER_COLOR,
        }
    }
}

/// `bytes` in decimal megabytes ("172 MB"), the unit the CEF CDN lists its builds in.
pub(crate) fn megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes as f64 / 1e6).round())
}

/// The one-line state every engine list shows, and its tone. `Installed` says so here; the pickers and
/// cards leave it out (an installed engine has no marker beyond its name), Settings shows it.
pub(crate) fn engine_state_label(state: &EngineState) -> (String, Tone) {
    match state {
        EngineState::Installed => ("Installed".into(), Tone::Weak),
        EngineState::NotInstalled { size } => (format!("Not installed \u{b7} {}", megabytes(*size)), Tone::Weak),
        EngineState::Installing { received, total } if *total > 0 => {
            let percent = (*received as f64 / *total as f64 * 100.0).floor().clamp(0.0, 100.0);
            (format!("Installing {percent}%"), Tone::Weak)
        }
        EngineState::Installing { .. } => ("Installing".into(), Tone::Weak),
        EngineState::UpdateNeeded { size } => (format!("Update needed \u{b7} {}", megabytes(*size)), Tone::Weak),
        EngineState::Failed(reason) => (format!("Failed: {reason}"), Tone::Danger),
        EngineState::Custom(path) => (format!("Custom: {}", path.display()), Tone::Weak),
        EngineState::Unavailable(reason) => (format!("Unavailable: {reason}"), Tone::Weak),
    }
}

/// What choosing an engine in a picker does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickOutcome {
    /// Use it.
    Switch(EngineKind),
    /// Not usable yet: ask to install it (the confirm names the size). Never a silent switch.
    AskInstall(EngineKind),
    /// Its install is already running; nothing to switch to yet.
    AlreadyInstalling(EngineKind),
    /// This platform cannot run it (no pin, wrong OS): the click only explains, with the reason.
    Unavailable(EngineKind, String),
}

impl PickOutcome {
    /// The toast an `Unavailable` pick shows: the engine and the same reason its state line gives.
    pub(crate) fn unavailable_toast(kind: EngineKind, reason: &str) -> String {
        format!("{} is unavailable: {reason}", engine_label(kind))
    }
}

pub(crate) fn pick_outcome(kind: EngineKind, state: &EngineState) -> PickOutcome {
    match state {
        EngineState::Installed | EngineState::Custom(_) => PickOutcome::Switch(kind),
        EngineState::Installing { .. } => PickOutcome::AlreadyInstalling(kind),
        EngineState::Unavailable(reason) => PickOutcome::Unavailable(kind, reason.clone()),
        EngineState::NotInstalled { .. } | EngineState::UpdateNeeded { .. } | EngineState::Failed(_) => {
            PickOutcome::AskInstall(kind)
        }
    }
}

/// A pending confirm before a large or destructive engine action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineConfirm {
    Download(EngineKind),
    Remove(EngineKind),
}

/// The state line of an engine card or Settings row: its label, plus a progress bar while installing.
pub(crate) fn engine_state_line(ui: &mut egui::Ui, state: &EngineState) {
    let (text, tone) = engine_state_label(state);
    ui.label(egui::RichText::new(text).small().color(tone.color()));
    if let EngineState::Installing { received, total } = state {
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

/// A distinct color per engine (loosely nodding to each project's own
/// branding — Servo orange, Chromium/Blink blue, Safari/WebKit teal) used
/// as a small identifying dot/badge wherever a tab's engine matters at a
/// glance: the tab strip, the graph node's engine badge. Deliberately
/// separate from `ACCENT` (which means "focused"/"selected", a different
/// axis of meaning) so the two never get confused for one another.
pub(crate) fn engine_color(kind: EngineKind) -> egui::Color32 {
    match kind {
        EngineKind::Servo => egui::Color32::from_rgb(255, 138, 80),
        EngineKind::Cef => egui::Color32::from_rgb(66, 133, 244),
        EngineKind::Webkit => egui::Color32::from_rgb(26, 188, 156),
        // Windows/Edge blue — distinct enough from CEF's Chromium blue to
        // tell the two apart at a glance despite both being Blink under
        // the hood.
        EngineKind::Webview2 => egui::Color32::from_rgb(0, 120, 212),
    }
}

/// A small filled circle identifying a tab's engine at a glance — used
/// inline in a `ui.horizontal` layout (the tab strip, the engine picker's
/// own dropdown rows), so it allocates its own space rather than being a
/// raw `painter()` call positioned by hand.
pub(crate) fn engine_dot(ui: &mut egui::Ui, kind: EngineKind, diameter: f32) {
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), diameter / 2.0, engine_color(kind));
}

/// Where each engine actually stands in this specific app, not a general
/// verdict on the upstream project -- "Experimental" for Servo is about
/// *this app's own* software-rendering-only integration and Servo's
/// still-incomplete standards coverage, not a claim about Servo itself.
/// Ordered `Tier::One` first for `engine_picker_order` below.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EngineTier {
    Recommended,
    Experimental,
    Limited,
}

impl EngineTier {
    pub(crate) fn label(self) -> &'static str {
        match self {
            EngineTier::Recommended => "Recommended",
            EngineTier::Experimental => "Experimental",
            EngineTier::Limited => "Limited support",
        }
    }

    /// A muted, tier-coded badge color -- deliberately not reusing
    /// `ACCENT`/`DANGER_COLOR` (both already mean something else
    /// elsewhere: "focused" and "destructive") or any `engine_color`
    /// (that axis is "which engine," this one is "how ready is it").
    pub(crate) fn color(self) -> egui::Color32 {
        match self {
            EngineTier::Recommended => egui::Color32::from_rgb(150, 190, 90),
            EngineTier::Experimental => egui::Color32::from_rgb(215, 162, 74),
            EngineTier::Limited => egui::Color32::from_rgb(196, 87, 58),
        }
    }
}

pub(crate) fn engine_tier(kind: EngineKind) -> EngineTier {
    match kind {
        EngineKind::Cef | EngineKind::Webview2 => EngineTier::Recommended,
        EngineKind::Servo => EngineTier::Experimental,
        EngineKind::Webkit => EngineTier::Limited,
    }
}

/// Short, specific claims only -- see `engine_cons` for the matching
/// tradeoff each one of these doesn't mention.
pub(crate) fn engine_pros(kind: EngineKind) -> &'static [&'static str] {
    match kind {
        EngineKind::Cef => &[
            "Full Chromium/Blink compatibility -- broadest real-world site support of the four",
            "Mature, widely embedded elsewhere -- not this app's own experiment",
        ],
        EngineKind::Webview2 => &[
            "Also Blink-based -- same site compatibility as CEF",
            "Uses the OS's own Edge runtime -- no separate browser binary for this app to ship or update",
        ],
        EngineKind::Servo => &[
            "Modern, from-scratch Rust engine -- same language as this app itself",
            "No legacy multi-decade codebase to work around, unlike the other three",
        ],
        EngineKind::Webkit => &[
            "The only non-Blink, non-Servo engine here -- real engine diversity for cross-engine testing",
            "Catches Safari/WebKit-specific site behavior the other three can't show you",
        ],
    }
}

/// Matching tradeoff for each `engine_pros` claim -- specific to how this
/// app integrates each engine today, not a permanent verdict (Servo's own
/// software-rendering limitation here, for instance, is this app's current
/// `SoftwareRenderingContext` choice, not something inherent to Servo).
pub(crate) fn engine_cons(kind: EngineKind) -> &'static [&'static str] {
    match kind {
        EngineKind::Cef => &["Heaviest process footprint of the four"],
        EngineKind::Webview2 => &[
            "Requires the WebView2 Runtime to be installed (usually already is, on Windows 10 1803+/11)",
            "Tied to whatever Edge version is installed system-wide, not pinned by this app",
        ],
        EngineKind::Servo => &[
            "Runs via software (CPU) rendering only in this app today -- slower than the other three",
            "Younger engine -- some sites may render incorrectly or not at all yet",
        ],
        EngineKind::Webkit => &[
            "No native Windows WebKit exists -- runs inside a WSL2 Ubuntu VM, relayed over TCP \
             (no shared memory, higher latency)",
            "Slowest to start (a cold WSL2 boot can take up to 45s); needs WSL2 + Ubuntu + several Linux \
             packages preinstalled",
        ],
    }
}

/// Display order for the engine picker: best-supported first, so a first-
/// time chooser sees the safe defaults before the tradeoffs.
pub(crate) const ENGINE_PICKER_ORDER: [EngineKind; 4] =
    [EngineKind::Cef, EngineKind::Webview2, EngineKind::Servo, EngineKind::Webkit];

/// One engine's own card inside `build_engine_picker_popup` -- name, color
/// dot, tier badge, then its pros/cons side by side. Returns whether this
/// card's response (the whole card is the hit target, not just a button
/// within it, so picking an engine doesn't require precision-clicking a
/// small label). Keyboard focus, arrows and Enter are the popup's job.
pub(crate) fn draw_engine_card(
    ui: &mut egui::Ui,
    kind: EngineKind,
    is_selected: bool,
    state: &EngineState,
) -> egui::Response {
    // An engine this platform cannot run is dimmed to weak text and only explains itself when clicked.
    let dim = matches!(state, EngineState::Unavailable(_));
    let frame_response = egui::Frame::default()
        .fill(if is_selected { SURFACE_HOVER } else { SURFACE_RAISED })
        .corner_radius(2u8)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            // The popup's full width, not just as wide as the card's text.
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                engine_dot(ui, kind, 10.0);
                ui.label(egui::RichText::new(engine_label(kind)).color(if dim { TEXT_WEAK } else { TEXT_PRIMARY }));
                if is_selected {
                    ui.label(egui_phosphor::light::CHECK_CIRCLE);
                }
                let tier = engine_tier(kind);
                ui.label(egui::RichText::new(tier.label()).small().color(if dim { TEXT_WEAK } else { tier.color() }));
            });
            if *state != EngineState::Installed {
                engine_state_line(ui, state);
            }
            ui.add_space(4.0);
            for pro in engine_pros(kind) {
                let text = egui::RichText::new(format!("+ {pro}")).small();
                ui.label(if dim { text.color(TEXT_WEAK) } else { text });
            }
            for con in engine_cons(kind) {
                ui.label(egui::RichText::new(format!("\u{2212} {con}")).small().color(TEXT_WEAK));
            }
        });
    // Interacted on *after* the frame's content is laid out, using its own
    // resulting rect -- interacting on `ui.max_rect()` before any content
    // exists gives a hit region sized to whatever was allocated so far
    // (near-zero at that point), not the card's real, final size. Found
    // live: clicking squarely on a card's own visible pros/cons text did
    // nothing, since only a sliver at the very top was ever actually
    // clickable.
    let id = ui.id().with(("engine_card", kind as u8));
    if dim {
        // `Sense::CLICK`, not `click()`: still clickable (to explain), but not focusable, so the keyboard
        // skips it; the default cursor says it does nothing. (`add_enabled` would swallow the click.)
        return ui.interact(frame_response.response.rect, id, egui::Sense::CLICK);
    }
    ui.interact(frame_response.response.rect, id, egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The shared engine-picker popup -- see `draw_engine_card`'s own doc
/// comment for each card's content. Used by all three places this app lets
/// you choose an engine (Settings' "Default starting engine," Settings'
/// "Current Tab" swap, and the Tab toolbar's "Split" picker), each through
/// `App::build_anchored_engine_picker`, replacing what used to be three
/// separate plain `egui::ComboBox`es with no tier/tradeoff information at
/// all. Returns the popup's own drawn rect (for the caller's
/// `clicked_outside_popup` check) and whichever engine was clicked this
/// frame, if any. `interactable` is whether the popup is open, same as
/// `build_list_popup_frame`'s.
pub(crate) fn build_engine_picker_popup(
    ctx: &egui::Context,
    id: &str,
    anchor_rect: egui::Rect,
    selected: Option<EngineKind>,
    state_of: &dyn Fn(EngineKind) -> EngineState,
    fade: f32,
    interactable: bool,
) -> (egui::Rect, Option<EngineKind>) {
    let mut picked = None;
    // Keyboard: the selected card is focused when the popup opens, Up/Down move between
    // cards, Enter picks (a focused click widget clicks on Enter), and Tab stays inside.
    let was_open_id = egui::Id::new((id, "was_open"));
    let just_opened = interactable && !ctx.data(|d| d.get_temp::<bool>(was_open_id).unwrap_or(false));
    ctx.data_mut(|d| d.insert_temp(was_open_id, interactable));
    let area_rect = egui::Area::new(egui::Id::new(id))
        .fixed_pos(anchor_rect.left_bottom())
        .order(egui::Order::Foreground)
        .interactable(interactable)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            popup_frame(ui).show(ui, |ui| {
                control_font(ui);
                ui.set_width(340.0);
                // Same "cap height, let it scroll" shape the other
                // toolbar list popups (Bookmarks/History/Downloads)
                // already use -- four cards this rich run well past a
                // short window's own height otherwise, with the last
                // one (WebKit, usually) silently unreachable past the
                // window's bottom edge (confirmed live).
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    let mut cards = Vec::new();
                    // Unavailable cards are not focusable, so the keyboard skips them: the popup opens on
                    // the selected card, or the first available one when the selection is not usable.
                    let usable = |kind: EngineKind| !matches!(state_of(kind), EngineState::Unavailable(_));
                    let focus_on = selected
                        .filter(|kind| usable(*kind))
                        .or_else(|| ENGINE_PICKER_ORDER.into_iter().find(|kind| usable(*kind)))
                        .unwrap_or(ENGINE_PICKER_ORDER[0]);
                    for (i, kind) in ENGINE_PICKER_ORDER.into_iter().enumerate() {
                        if i > 0 {
                            ui.add_space(6.0);
                        }
                        let card = draw_engine_card(ui, kind, selected == Some(kind), &state_of(kind));
                        if just_opened && focus_on == kind {
                            card.request_focus();
                        }
                        if card.has_focus() {
                            focus_ring(ui, &card);
                            card.scroll_to_me(None);
                            ui.memory_mut(|m| {
                                m.set_focus_lock_filter(
                                    card.id,
                                    egui::EventFilter { tab: true, vertical_arrows: true, ..Default::default() },
                                )
                            });
                        }
                        if card.clicked() {
                            picked = Some(kind);
                        }
                        if usable(kind) {
                            cards.push(card.id);
                        }
                        if i > 0 {
                            ui.add_space(2.0);
                        }
                    }
                    if let Some(at) = cards.iter().position(|id| ui.memory(|m| m.has_focus(*id))) {
                        let (down, up) = ui.input_mut(|i| {
                            (
                                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                            )
                        });
                        // Down is +1, Up is -1 (as len - 1, mod len).
                        let step = usize::from(down) + (cards.len() - 1) * usize::from(up);
                        if step != 0 {
                            ui.memory_mut(|m| m.request_focus(cards[(at + step) % cards.len()]));
                        }
                    }
                });
            });
        })
        .response
        .rect;
    (area_rect, picked)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn frame(ctx: &egui::Context, selected: EngineKind, events: Vec<egui::Event>) -> Option<EngineKind> {
        frame_with(ctx, selected, events, &|_| EngineState::Installed)
    }

    fn frame_with(
        ctx: &egui::Context,
        selected: EngineKind,
        events: Vec<egui::Event>,
        state_of: &dyn Fn(EngineKind) -> EngineState,
    ) -> Option<EngineKind> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 900.0))),
            events,
            ..Default::default()
        };
        let mut picked = None;
        let mut output = ctx.run_ui(input, |ui| {
            let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(60.0, 20.0));
            picked = build_engine_picker_popup(ui.ctx(), "test_picker", anchor, Some(selected), state_of, 1.0, true).1;
        });
        output.textures_delta.clear(); // nothing uploads the font atlas here
        picked
    }

    #[test]
    fn state_labels_cover_all_six_states() {
        let label = |s| engine_state_label(&s);
        assert_eq!(label(EngineState::Installed), ("Installed".to_string(), Tone::Weak));
        assert_eq!(
            label(EngineState::NotInstalled { size: 171_619_649 }),
            ("Not installed \u{b7} 172 MB".to_string(), Tone::Weak)
        );
        assert_eq!(
            label(EngineState::Installing { received: 34, total: 100 }),
            ("Installing 34%".to_string(), Tone::Weak)
        );
        assert_eq!(label(EngineState::Installing { received: 0, total: 0 }).0, "Installing");
        assert_eq!(
            label(EngineState::UpdateNeeded { size: 171_619_649 }),
            ("Update needed \u{b7} 172 MB".to_string(), Tone::Weak)
        );
        assert_eq!(
            label(EngineState::Failed("download failed: x".into())),
            ("Failed: download failed: x".to_string(), Tone::Danger)
        );
        assert_eq!(
            label(EngineState::Custom(std::path::PathBuf::from("D:/cef"))),
            ("Custom: D:/cef".to_string(), Tone::Weak)
        );
    }

    #[test]
    fn the_disk_text_names_the_parts_the_engine_has() {
        use crate::engine_install::{OnDisk, RuntimeOnDisk};
        let disk = OnDisk { host: RuntimeOnDisk::Ready, runtime: RuntimeOnDisk::Stale };
        assert_eq!(on_disk_text(EngineKind::Servo, disk), "host ready");
        assert_eq!(on_disk_text(EngineKind::Cef, disk), "host ready, runtime stale");
        assert_eq!(
            on_disk_text(EngineKind::Cef, OnDisk::both(RuntimeOnDisk::Missing)),
            "host missing, runtime missing"
        );
    }

    /// An engine that isn't usable asks to install; it never switches silently.
    #[test]
    fn picking_an_unavailable_engine_asks_to_install() {
        let kind = EngineKind::Cef;
        let ask = PickOutcome::AskInstall(kind);
        assert_eq!(pick_outcome(kind, &EngineState::Installed), PickOutcome::Switch(kind));
        assert_eq!(pick_outcome(kind, &EngineState::Custom("D:/cef".into())), PickOutcome::Switch(kind));
        assert_eq!(pick_outcome(kind, &EngineState::NotInstalled { size: 1 }), ask);
        assert_eq!(pick_outcome(kind, &EngineState::UpdateNeeded { size: 1 }), ask);
        assert_eq!(pick_outcome(kind, &EngineState::Failed("x".into())), ask);
        assert_eq!(
            pick_outcome(kind, &EngineState::Installing { received: 1, total: 2 }),
            PickOutcome::AlreadyInstalling(kind)
        );
    }

    /// The picker opens focused on the selected card; Down moves to the next one and Enter picks it.
    #[test]
    fn arrows_and_enter_pick_an_engine_from_the_keyboard() {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let selected = ENGINE_PICKER_ORDER[1];
        let next = ENGINE_PICKER_ORDER[2];
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
        let skipped = ENGINE_PICKER_ORDER[2];
        let state_of = |kind: EngineKind| {
            if kind == skipped {
                EngineState::Unavailable("needs Windows".into())
            } else {
                EngineState::Installed
            }
        };
        let (selected, after) = (ENGINE_PICKER_ORDER[1], ENGINE_PICKER_ORDER[3]);
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
        assert_eq!(frame_with(&ctx, skipped, vec![key(egui::Key::Enter)], &state_of), Some(ENGINE_PICKER_ORDER[0]));
    }

    #[test]
    fn an_unavailable_pick_carries_its_reason() {
        let state = EngineState::Unavailable("needs Windows".into());
        let outcome = pick_outcome(EngineKind::Webview2, &state);
        assert_eq!(outcome, PickOutcome::Unavailable(EngineKind::Webview2, "needs Windows".into()));
        assert_eq!(
            PickOutcome::unavailable_toast(EngineKind::Webview2, "needs Windows"),
            "WebView2 is unavailable: needs Windows"
        );
    }

    /// Tab must not leave the popup: Enter after it still picks the focused card.
    #[test]
    fn tab_keeps_focus_on_the_card() {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let selected = ENGINE_PICKER_ORDER[1];
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
        let state = EngineState::Installing { received: 0, total: 10 };
        for _ in 0..2 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                egui::Area::new(egui::Id::new("bar")).show(ui.ctx(), |ui| engine_state_line(ui, &state));
            });
            output.textures_delta.clear(); // nothing uploads the font atlas here
        }
    }
}
