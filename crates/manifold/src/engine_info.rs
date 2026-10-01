//! Per-engine presentation: labels, colors, tiers, pros/cons and the engine
//! picker popup.

use super::*;

pub(crate) fn engine_label(kind: EngineKind) -> &'static str {
    kind.desc().label
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
            "No native Windows WebKit exists -- runs inside a WSL2 Ubuntu VM, relayed over TCP (no shared memory, higher latency)",
            "Slowest to start (a cold WSL2 boot can take up to 45s); needs WSL2 + Ubuntu + several Linux packages preinstalled",
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
pub(crate) fn draw_engine_card(ui: &mut egui::Ui, kind: EngineKind, is_selected: bool) -> egui::Response {
    let frame_response = egui::Frame::default()
        .fill(if is_selected { SURFACE_HOVER } else { SURFACE_RAISED })
        .corner_radius(2u8)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            // The popup's full width, not just as wide as the card's text.
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                engine_dot(ui, kind, 10.0);
                ui.label(egui::RichText::new(engine_label(kind)).color(TEXT_PRIMARY));
                if is_selected {
                    ui.label(egui_phosphor::light::CHECK_CIRCLE);
                }
                let tier = engine_tier(kind);
                ui.label(egui::RichText::new(tier.label()).small().color(tier.color()));
            });
            ui.add_space(4.0);
            for pro in engine_pros(kind) {
                ui.label(egui::RichText::new(format!("+ {pro}")).small());
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
    ui.interact(frame_response.response.rect, ui.id().with(("engine_card", kind as u8)), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
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
                ui.set_width(340.0);
                // Same "cap height, let it scroll" shape the other
                // toolbar list popups (Bookmarks/History/Downloads)
                // already use -- four cards this rich run well past a
                // short window's own height otherwise, with the last
                // one (WebKit, usually) silently unreachable past the
                // window's bottom edge (confirmed live).
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    let mut cards = Vec::new();
                    for (i, kind) in ENGINE_PICKER_ORDER.into_iter().enumerate() {
                        if i > 0 {
                            ui.add_space(6.0);
                        }
                        let card = draw_engine_card(ui, kind, selected == Some(kind));
                        if just_opened && selected.unwrap_or(ENGINE_PICKER_ORDER[0]) == kind {
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
                        cards.push(card.id);
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
