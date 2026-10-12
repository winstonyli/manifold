//! The app's look: palette, egui style and fonts, shared popup/dialog
//! frames, the toolbar panel and chip widgets, and the dark title bar.

use super::*;

/// Tab view's chrome is a single top `Panel` of this exact height, in egui
/// points — kept as a constant rather than read back from egui's layout so
/// the compositor's content viewport (see `gpu::GpuState::render`'s
/// `content_top_px`) always agrees with where egui actually painted,
/// without a frame of lag. Was two stacked bars (a tab-identity strip above
/// a separate navigation toolbar) until they were merged into one row —
/// the identity info (favicon/spinner, engine dot) now lives inline with
/// navigation controls in the same bar, and the separate page-title label
/// was dropped as redundant with the address bar's URL.
pub(crate) const CHROME_HEIGHT_POINTS: f32 = 38.0;

/// Fixed tag-color choices a group or tab card's color swatch cycles
/// through (see `next_tag_color`) -- vermillion/orange/yellow/bluish
/// green/sky blue/reddish purple, chosen for mutual distinguishability
/// against this app's dark chrome rather than any particular meaning.
/// Used via `Mirror::color`, for both `Tab` and
/// `Group`. These are the Okabe-Ito colorblind-safe palette values
/// (Okabe & Ito, 2008) -- every swatch here is rendered as a plain
/// filled circle with no secondary encoding (shape, letter, pattern) to
/// distinguish them, confirmed live: the original arbitrary red/orange/
/// yellow/green/cyan/purple choice had several genuinely confusable
/// pairs (red/orange, green/cyan) for common colorblindness types.
/// Swapping the six RGB values keeps the same visual role per slot and
/// needs no rendering changes at all.
pub(crate) const TAG_COLOR_PALETTE: [egui::Color32; 6] = [
    egui::Color32::from_rgb(213, 94, 0),
    egui::Color32::from_rgb(230, 159, 0),
    egui::Color32::from_rgb(240, 228, 66),
    egui::Color32::from_rgb(0, 158, 115),
    egui::Color32::from_rgb(86, 180, 233),
    egui::Color32::from_rgb(204, 121, 167),
];

/// Background, border and padding shared by every menu and dropdown
/// (right-click menus, toolbar lists, the address-bar suggestions).
pub(crate) fn popup_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::default()
        .fill(TOOLBAR_FILL)
        .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
        .corner_radius(8u8)
        .inner_margin(egui::Margin::same(4))
}

/// One key as a keycap: `text` in a small bordered box. egui has no such widget, only
/// `Button::shortcut_text` (plain weak text at a button's right edge, used for menu rows).
pub(crate) fn kbd(ui: &mut egui::Ui, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
        .corner_radius(3u8)
        .inner_margin(egui::Margin::symmetric(4, 1))
        .show(ui, |ui| ui.label(egui::RichText::new(text).monospace().small()))
        .response
}

/// A 30x17 on/off switch (the mockup's `.tog`): the knob eases between the ends. Click or Space toggles it;
/// the caller reads `clicked()` and flips its own state. `label` is the screen-reader name.
pub(crate) fn toggle(ui: &mut egui::Ui, on: bool, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 17.0), egui::Sense::click());
    let t = ui.ctx().animate_bool_responsive(response.id, on);
    let off_fill = TOOLBAR_FILL.gamma_multiply(1.6);
    let fill = lerp_color32(off_fill, ACCENT, t);
    let knob = lerp_color32(ui.visuals().weak_text_color(), BG, t);
    let radius = rect.height() / 2.0;
    ui.painter().rect_filled(rect, radius, fill);
    let knob_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
    ui.painter().circle_filled(egui::pos2(knob_x, rect.center().y), 6.5, knob);
    focus_ring(ui, &response);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    response
}

/// A settings row: `label` (and a weak `hint` under it) on the left, `control` at the right edge.
pub(crate) fn setting_row(ui: &mut egui::Ui, label: &str, hint: Option<&str>, control: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width((ui.available_width() - 60.0).max(120.0));
            ui.label(label);
            if let Some(hint) = hint {
                ui.add(egui::Label::new(egui::RichText::new(hint).small().weak()).wrap());
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
    });
}

/// A tooltip that also names the widget for a screen reader: icon buttons otherwise announce their glyph.
pub(crate) trait NamedTip {
    fn named_tip(self, text: impl Into<String>) -> Self;
}

impl NamedTip for egui::Response {
    fn named_tip(self, text: impl Into<String>) -> Self {
        let text = text.into();
        let enabled = self.enabled();
        self.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text.clone()));
        self.on_hover_text(text)
    }
}

/// Paints `keys` as keycaps right-aligned at `right_center`, with no layout: for a hint drawn over a field.
pub(crate) fn paint_keycaps(ui: &egui::Ui, right_center: egui::Pos2, keys: &[&str]) {
    let font = egui::FontId::new(11.0, bound_family(ui.ctx(), control_family()));
    let weak = egui::Color32::from_rgb(150, 144, 130);
    let mut right = right_center.x;
    for key in keys.iter().rev() {
        let galley = ui.painter().layout_no_wrap((*key).to_string(), font.clone(), weak);
        let size = galley.size() + egui::vec2(8.0, 2.0);
        let rect = egui::Rect::from_min_size(egui::pos2(right - size.x, right_center.y - size.y / 2.0 - 1.0), size);
        ui.painter().rect(
            rect,
            3.0,
            egui::Color32::from_rgb(48, 45, 38),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(62, 59, 52)),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, weak);
        right = rect.min.x - 3.0;
    }
}

/// A key combination as keycaps in the current row: "Ctrl+Shift+C" is three, and "A / B"
/// is two combinations around a weak slash.
pub(crate) fn kbd_keys(ui: &mut egui::Ui, keys: &str) {
    ui.spacing_mut().item_spacing.x = 3.0;
    for (i, alternative) in keys.split(" / ").enumerate() {
        if i > 0 {
            ui.label(egui::RichText::new("/").weak());
        }
        for key in alternative.split('+').filter(|k| !k.is_empty()) {
            kbd(ui, key);
        }
    }
}

/// A floating, non-interactive bar of "keycaps + what they do" pairs.
pub(crate) fn key_hint_strip<'a>(
    ctx: &egui::Context,
    area: egui::Area,
    hints: impl Iterator<Item = (&'a str, &'a str)>,
) {
    area.order(egui::Order::Foreground).interactable(false).show(ctx, |ui| {
        popup_frame(ui).show(ui, |ui| {
            control_font(ui);
            ui.horizontal(|ui| {
                for (keys, what) in hints {
                    kbd_keys(ui, keys);
                    ui.label(egui::RichText::new(what).small().weak());
                    ui.add_space(4.0);
                }
            });
        });
    });
}

/// An outline on a keyboard-focused widget. egui shows focus only as the widget's
/// "active" look, which is invisible on a button whose fill is overridden or that
/// has no frame; this paints inside the widget's rect, so it moves nothing.
pub(crate) fn focus_ring(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(response.rect, 2.0, egui::Stroke::new(1.5, TEXT_PRIMARY), egui::StrokeKind::Inside);
    }
}

/// Adds `button` with no border in any state, at the same size in every state.
/// Not `Button::stroke(NONE)`: egui sizes a button's padding for the stroke its
/// *state's visuals* have (padding minus the stroke width) and only then applies
/// the override, so a button whose hovered visuals have a 1 px stroke but whose
/// override has none is 2 px narrower hovered than at rest, and every widget
/// after it in the row shifts. Zeroing the strokes in the visuals keeps the
/// padding the same in every state.
pub(crate) fn add_borderless(ui: &mut egui::Ui, enabled: bool, button: egui::Button) -> egui::Response {
    let none = egui::Stroke::NONE;
    let w = &mut ui.visuals_mut().widgets;
    let saved =
        [w.noninteractive.bg_stroke, w.inactive.bg_stroke, w.hovered.bg_stroke, w.active.bg_stroke, w.open.bg_stroke];
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.bg_stroke = none;
    }
    let response = ui.add_enabled(enabled, button);
    focus_ring(ui, &response);
    let w = &mut ui.visuals_mut().widgets;
    for (state, stroke) in
        [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open].into_iter().zip(saved)
    {
        state.bg_stroke = stroke;
    }
    response
}

/// Runs `add` with no border on widgets at rest. A button drawn only when hovered
/// (`selectable_label`, `toggle_value`, `Button::selectable`) has its frame
/// stripped at rest, so it is laid out without its stroke but padded for it
/// (padding minus the stroke width); the hovered frame then adds the stroke back
/// and the widget grows by twice the stroke width, pushing what follows. With no
/// resting stroke the padding is the full amount in both states.
pub(crate) fn rest_borderless<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let saved = ui.visuals().widgets.inactive.bg_stroke;
    ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::NONE;
    let result = add(ui);
    ui.visuals_mut().widgets.inactive.bg_stroke = saved;
    result
}

/// `ui.selectable_label` that does not change size on hover; see [`rest_borderless`].
pub(crate) fn selectable_label(ui: &mut egui::Ui, selected: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    rest_borderless(ui, |ui| ui.selectable_label(selected, text))
}

/// The one solid-gold button a toolbar has: its main action.
pub(crate) fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding.x = 11.0;
        add_borderless(
            ui,
            true,
            egui::Button::new(
                egui::RichText::new(label)
                    .font(egui::FontId::new(14.0, bound_family(ui.ctx(), control_bold_family())))
                    .color(BG),
            )
            .fill(ACCENT),
        )
    })
    .inner
}

/// Compact rows for a menu or dropdown, applied inside its `popup_frame`:
/// no gap between rows, tighter padding, and no resting fill, so a row shows
/// only its text until hovered or highlighted, then fills the full width.
/// The global spacing is airier on purpose, which suits dialogs and the
/// settings page but makes a menu of plain buttons look padded.
pub(crate) fn menu_item_style(ui: &mut egui::Ui) {
    control_font(ui);
    let style = ui.style_mut();
    style.spacing.item_spacing.y = 0.0;
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    // 26 px rows with 5 px hover corners, as in the mockup's menu.
    style.spacing.interact_size.y = 26.0;
    for w in
        [&mut style.visuals.widgets.hovered, &mut style.visuals.widgets.active, &mut style.visuals.widgets.inactive]
    {
        w.corner_radius = egui::CornerRadius::same(5);
    }
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
}

/// `popup_frame` for centered dialogs (quick switcher, shortcuts help,
/// import confirm, JSON viewer): rounder, with more padding.
pub(crate) fn dialog_frame(ui: &egui::Ui) -> egui::Frame {
    popup_frame(ui).corner_radius(6u8).inner_margin(egui::Margin::same(14))
}

/// One click on a card's color swatch advances to the next entry of `TAG_COLOR_PALETTE`, wrapping back to `None`
/// (the plain default border) after the last: `None` -> the first entry -> ... -> the last -> `None`.
pub(crate) fn next_tag_color(current: Option<egui::Color32>) -> Option<egui::Color32> {
    match current.and_then(|c| TAG_COLOR_PALETTE.iter().position(|&p| p == c)) {
        None => Some(TAG_COLOR_PALETTE[0]),
        Some(i) if i + 1 < TAG_COLOR_PALETTE.len() => Some(TAG_COLOR_PALETTE[i + 1]),
        Some(_) => None,
    }
}

/// Fill for the toolbar/chrome bars (Tab view's merged single bar, Graph
/// view's "+ New Tab" bar, the settings popup) — distinct from the
/// graph/content area below.
pub(crate) const TOOLBAR_FILL: egui::Color32 = egui::Color32::from_rgb(29, 26, 20);

/// A single accent color used throughout — selection highlights, hyperlinks,
/// the focused-tab-strip fill, the focused graph node's border — so the
/// whole chrome reads as one coherent theme rather than egui's flat default
/// dark gray-on-gray. A muted gold rather than the cooler blue this used to
/// be, to sit with the rest of the warm/editorial palette below instead of
/// against it.
pub(crate) const ACCENT: egui::Color32 = egui::Color32::from_rgb(215, 162, 74);

/// The one severity-red used everywhere this app needs to say "warning" or
/// "destructive": a group card's overloaded border, every close/eject
/// button's hover fill, error text, and the muted-audio icon all reference
/// this single constant rather than each re-deriving the same RGB triple --
/// so it reads unambiguously as one meaning across the whole chrome, and a
/// future shade change only has one place to edit. A warm rust rather than a
/// pure red, so it still reads unambiguously as "danger" without clashing
/// with the rest of this palette the way a stop-sign red would.
pub(crate) const DANGER_COLOR: egui::Color32 = egui::Color32::from_rgb(196, 87, 58);

/// The sky-blue entry of `TAG_COLOR_PALETTE`: the one cool hue in this otherwise warm theme.
pub(crate) const TAG_SKY_BLUE: egui::Color32 = TAG_COLOR_PALETTE[4];

/// A drag that a release would refuse (keeping what it holds): `TAG_SKY_BLUE` pulled a little toward `BG`, a hue far
/// from `ACCENT` (a valid drop) and `DANGER_COLOR` (a deletion), so the outcomes differ by hue, not brightness.
pub(crate) fn refused_color() -> egui::Color32 {
    lerp_color32(TAG_SKY_BLUE, BG, 0.2)
}

/// A sticky note's card fill -- a warm, light color deliberately unlike
/// every other card's dark `extreme_bg_color`/`TOOLBAR_FILL` tone, so a note
/// reads as "a physical note" at a glance rather than another chrome-toned
/// panel. The text drawn on it (`NotePayload`'s editable body) is dark to
/// stay legible against this light fill, the one card in this app that
/// isn't light-text-on-dark.
pub(crate) const NOTE_CARD_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 236, 148);

/// The window/content-area base — behind every panel, the darkest layer.
/// Distinct from `TOOLBAR_FILL` on purpose: egui's own default has exactly
/// one dark gray for everything (panels, buttons, the app background all
/// read as the same flat plane), which is the single biggest reason this
/// app used to look like an unstyled egui demo rather than real chrome.
/// This + `TOOLBAR_FILL` + `SURFACE_RAISED` form a three-step depth ladder
/// (background < panel < raised widget) that egui's stock palette doesn't
/// have at all.
pub(crate) const BG: egui::Color32 = egui::Color32::from_rgb(23, 20, 15);

/// A button/field's own resting fill, one step up from `TOOLBAR_FILL` — the
/// panel it sits on. Also used for the graph card's engine-picker `Frame`.
/// Deliberately a wide jump from `TOOLBAR_FILL`, not a subtle one — the
/// point is a button reads as its own raised chip at a glance, not just
/// under close pixel inspection.
pub(crate) const SURFACE_RAISED: egui::Color32 = egui::Color32::from_rgb(41, 36, 27);

/// Hover state, one step up again — every interactive widget in this app's
/// toolbar/popups visibly lightens on hover, which stock egui's near-
/// identical inactive/hovered grays don't make obvious at a glance.
pub(crate) const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(58, 50, 37);

pub(crate) const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(233, 226, 211);

pub(crate) const TEXT_WEAK: egui::Color32 = egui::Color32::from_rgb(168, 154, 120);

/// Applied once at startup (see `Default for App`). Bumps corner rounding
/// and spacing up from egui's fairly tight defaults (which read as "plain
/// tool window" rather than "app chrome"), replaces egui's stock flat-gray
/// widget palette with the `BG`/`TOOLBAR_FILL`/`SURFACE_RAISED`/
/// `SURFACE_HOVER` depth ladder above, and threads `ACCENT` through
/// selection/hyperlink/focus/active-widget colors so the whole chrome reads
/// as one deliberately designed theme rather than an unstyled egui app.
pub(crate) fn apply_style(ctx: &egui::Context) {
    // `all_styles_mut` (not `style`/`set_style`, egui 0.36 keeps separate
    // dark/light styles internally) applies the same tweaks regardless of
    // which theme the OS/window ends up reporting — this app's chrome is
    // dark-only in practice, but there's no "just style whatever's active"
    // entry point, so set both identically. **Found live**: this used to
    // only tweak specific widget-state fields on top of whichever preset
    // (`Visuals::dark()`/`Visuals::light()`) egui already had loaded for
    // that style slot — under a light OS theme, every field this function
    // doesn't explicitly touch (heading/`.strong()` text color among them)
    // kept the light preset's own near-black default, invisible against
    // this app's always-dark backgrounds (`BG`/`TOOLBAR_FILL`/etc. are
    // forced dark regardless of OS theme). Resetting to `Visuals::dark()`
    // first, for both style slots, means every untouched field starts from
    // a coherent dark-appropriate default before the cosmetic overrides
    // below apply on top.
    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        // Airier than egui's own defaults on purpose -- the editorial
        // direction this app's chrome now takes wants real breathing room in
        // dialogs/menus/settings. The one place this is deliberately *not*
        // wanted -- the graph view's own toolbar, already packed with icon
        // buttons and genuinely screen-space-constrained -- pushes narrower
        // values back onto its own `Ui` locally (see wherever that toolbar
        // is drawn) rather than fighting this global default.
        style.spacing.item_spacing = egui::vec2(10.0, 7.0);
        style.spacing.button_padding = egui::vec2(13.0, 7.0);

        let corner_radius: egui::CornerRadius = 2u8.into();
        style.visuals.widgets.noninteractive.corner_radius = corner_radius;
        style.visuals.widgets.noninteractive.bg_fill = TOOLBAR_FILL;
        style.visuals.widgets.noninteractive.weak_bg_fill = TOOLBAR_FILL;
        style.visuals.widgets.noninteractive.fg_stroke.color = TEXT_WEAK;

        style.visuals.widgets.inactive.corner_radius = corner_radius;
        style.visuals.widgets.inactive.bg_fill = SURFACE_RAISED;
        style.visuals.widgets.inactive.weak_bg_fill = SURFACE_RAISED;
        style.visuals.widgets.inactive.fg_stroke.color = TEXT_PRIMARY;
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, SURFACE_HOVER);

        style.visuals.widgets.hovered.corner_radius = corner_radius;
        style.visuals.widgets.hovered.bg_fill = SURFACE_HOVER;
        style.visuals.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
        style.visuals.widgets.hovered.fg_stroke.color = TEXT_PRIMARY;
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);

        style.visuals.widgets.active.corner_radius = corner_radius;
        style.visuals.widgets.active.bg_fill = ACCENT;
        style.visuals.widgets.active.weak_bg_fill = ACCENT;
        style.visuals.widgets.active.fg_stroke.color = egui::Color32::BLACK;
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);

        style.visuals.widgets.open.corner_radius = corner_radius;
        style.visuals.widgets.open.bg_fill = SURFACE_RAISED;
        style.visuals.widgets.open.weak_bg_fill = SURFACE_RAISED;
        style.visuals.widgets.open.fg_stroke.color = TEXT_PRIMARY;
        style.visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, ACCENT);

        style.visuals.window_corner_radius = 2u8.into();
        style.visuals.menu_corner_radius = 2u8.into();
        style.visuals.window_fill = TOOLBAR_FILL;
        style.visuals.panel_fill = BG;
        style.visuals.extreme_bg_color = BG;
        style.visuals.faint_bg_color = SURFACE_RAISED;
        style.visuals.window_stroke = egui::Stroke::new(1.0, SURFACE_RAISED);

        style.visuals.selection.bg_fill = ACCENT;
        // Warm cream (`TEXT_PRIMARY`), not pure white -- white read cold and
        // slightly out of place against a gold selection fill once the rest
        // of this palette warmed up.
        style.visuals.selection.stroke.color = TEXT_PRIMARY;
        style.visuals.hyperlink_color = ACCENT;
        style.visuals.override_text_color = None;
    });
}

/// The named font family `heading_text` requests for dialog titles and
/// settings-section headers -- kept separate from `FontFamily::Proportional`
/// (body text) so the two can be genuinely different typefaces rather than
/// the same face at a bigger size. Both are static single-weight instances
/// generated from Google's variable-only source files via `fonttools
/// varLib.instancer` (neither Literata nor Newsreader ships a static build
/// upstream any more, and egui/ab_glyph has no variable-axis support of its
/// own -- it would otherwise just rasterize whatever the font's default
/// `fvar` instance happens to be, ignoring any weight/opsz this app wants).
pub(crate) fn heading_family() -> egui::FontFamily {
    egui::FontFamily::Name("heading".into())
}

/// Familjen Grotesk Regular for controls: buttons, menu rows, hints. Same
/// static-instance story as `heading_family`; falls back to the body chain.
pub(crate) fn control_family() -> egui::FontFamily {
    egui::FontFamily::Name("control".into())
}

/// Makes widgets laid out in `ui` (buttons, menu rows, text fields) use Familjen Grotesk at body size.
/// egui 0.36 buttons resolve their font from `TextStyle::Body`, not `TextStyle::Button`, so the family is
/// set per `Ui` rather than through `text_styles`.
pub(crate) fn control_font(ui: &mut egui::Ui) {
    let family = bound_family(ui.ctx(), control_family());
    ui.style_mut().override_font_id = Some(egui::FontId::new(13.0, family));
}

/// `family` if the context's fonts define it, else the body family. Asking egui for an unbound named family
/// panics, which would otherwise take down any frame run before `apply_fonts` takes effect (headless tests).
fn bound_family(ctx: &egui::Context, family: egui::FontFamily) -> egui::FontFamily {
    if ctx.fonts(|f| f.families().contains(&family)) {
        family
    } else {
        egui::FontFamily::Proportional
    }
}

/// Familjen Grotesk SemiBold, for the primary button.
pub(crate) fn control_bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("control_bold".into())
}

/// Replaces egui's bundled default (`Ubuntu-Light`) with Literata for body
/// text, inserted ahead of it in the proportional family rather than
/// instead of it — a glyph outside Literata's own coverage still falls back
/// to the bundled default (and from there to egui's bundled emoji/symbol
/// fonts) instead of tofu-boxing. Picked over the OS's own Segoe UI for one
/// concrete reason: `egui`/`ab_glyph` needs the actual font bytes at build
/// time, and there's no equivalent of a browser's "ask the OS for this
/// family" for a native GPU-rendered glyph atlas — bundling a real file was
/// required either way, so the choice came down to which file, not whether
/// to bundle one. Literata is Google's own "built for reading apps" text
/// face (tuned for legibility at small sizes on low-DPI screens), instanced
/// here at a text-appropriate `opsz`, not IBM Plex Serif's old plain
/// Regular-only cut, which read a little soft for UI chrome once compared
/// side by side. Newsreader (also Google's, from the same reading-app
/// design effort, so the pairing is deliberate) supplies just the separate
/// `heading_family()` above, instanced Bold and at a display-sized `opsz`,
/// for the handful of dialog titles/section headers that want more
/// contrast than body text at any weight gives for free.
pub(crate) fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Literata".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/Literata-Regular.ttf"))),
    );
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "Literata".to_owned());
    }
    fonts.font_data.insert(
        "Newsreader".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/Newsreader-Bold.ttf"))),
    );
    // Falls back to the same Literata/default chain as body text for any
    // glyph Newsreader's own Latin-focused coverage doesn't have (a heading
    // is still just text -- it shouldn't tofu-box on a character body text
    // would have rendered fine).
    let mut heading_fallback = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    heading_fallback.insert(0, "Newsreader".to_owned());
    fonts.families.insert(heading_family(), heading_fallback);
    // A real icon font, not a symbol glyph gamble -- Literata's own doc
    // comment above notes a theoretical fallback chain to egui's bundled
    // emoji/symbol font, but this project has repeatedly confirmed live
    // that several such glyphs still render as empty tofu boxes regardless
    // (see the pin/mute/audio-indicator dots, all painter-drawn shapes
    // specifically to dodge this). Phosphor's own codepoints live in a
    // private-use range no other font in this chain defines, so there's no
    // coverage ambiguity: every `egui_phosphor::light::*` constant used
    // in this app's chrome is guaranteed to resolve here. `Light` (not
    // `Regular`) to sit better against the now-serif-forward, less heavy
    // typography around it.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Light);
    // After the icon font is added, so these chains inherit its glyphs: an icon button in a
    // `control_font` Ui looks its glyph up in this family.
    for (name, family, bytes) in [
        ("FamiljenGrotesk", control_family(), include_bytes!("../assets/fonts/FamiljenGrotesk-Regular.ttf").as_slice()),
        (
            "FamiljenGrotesk-SemiBold",
            control_bold_family(),
            include_bytes!("../assets/fonts/FamiljenGrotesk-SemiBold.ttf").as_slice(),
        ),
    ] {
        // Familjen's line box sits its caps ~0.075 em below centre; lift the glyphs so labels look centred.
        let tweak = egui::FontTweak { y_offset_factor: -0.06, ..Default::default() };
        fonts.font_data.insert(name.to_owned(), std::sync::Arc::new(egui::FontData::from_static(bytes).tweak(tweak)));
        let mut chain = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
        chain.insert(0, name.to_owned());
        fonts.families.insert(family, chain);
    }
    fonts
}

/// Installs `font_definitions` in `ctx`.
pub(crate) fn apply_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

/// A dialog title or settings-section header — `heading_family()` (Newsreader
/// Bold) at a size that reads as a real step up from body text, in
/// `TEXT_PRIMARY`. Every such title in this app's chrome goes through this
/// one helper rather than each popup separately picking a size/weight, so
/// they stay visually identical to each other by construction.
pub(crate) fn heading_text(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).font(egui::FontId::new(17.0, heading_family())).color(TEXT_PRIMARY)
}

/// Forces the native (OS-drawn, undecorated-in-headless-mode-only -- see
/// `resumed`'s own window setup) titlebar dark, independent of the current
/// Windows theme setting. Without this, a system in light mode draws a
/// bright titlebar directly above this app's now warm-dark chrome -- a
/// visible seam undermining the rest of this theming pass for the one strip
/// of the window this app doesn't control. A full custom-drawn titlebar
/// (matching the palette exactly, not just light/dark) was considered and
/// deliberately deferred -- it would mean reimplementing drag/minimize/
/// maximize/close hit-testing, a real feature addition, not a theme tweak.
/// `DWMWA_USE_IMMERSIVE_DARK_MODE` (20) is undocumented pre-Windows 11 but
/// has shipped stably since a Windows 10 20H1 update; failure is silently
/// ignored (`DwmSetWindowAttribute`'s return value discarded) the same way
/// this app already treats titlebar theming as cosmetic, never fatal.
#[cfg(target_os = "windows")]
pub(crate) fn apply_dark_titlebar(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else { return };
    let hwnd: windows_sys::Win32::Foundation::HWND = win32.hwnd.get() as *mut core::ffi::c_void;
    let enabled: windows_sys::Win32::Foundation::BOOL = 1;
    unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows_sys::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &enabled as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<windows_sys::Win32::Foundation::BOOL>() as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn apply_dark_titlebar(_window: &Window) {}

/// Per-channel linear interpolation between two colors -- `egui`/`epaint`
/// has no built-in `Color32::lerp`. Used to ease a card's border color
/// between its resting and hovered tint instead of snapping instantly,
/// matching the same eased feel `animate_bool_responsive` already gives
/// the hover glow. Interpolates the premultiplied channels `Color32` stores, so easing in from `TRANSPARENT` fades
/// the target's own color in rather than passing through a darker tint of it.
pub(crate) fn lerp_color32(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp_u8 = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgba_premultiplied(
        lerp_u8(a.r(), b.r()),
        lerp_u8(a.g(), b.g()),
        lerp_u8(a.b(), b.b()),
        lerp_u8(a.a(), b.a()),
    )
}

/// Every fixed-height toolbar-style row in this app (Tab view's own
/// address-bar row, Graph view's own row, the Settings page's back-button
/// row, and the tab-respawn placeholder) goes through this one function
/// now, instead of each hand-rolling the same `Panel::top(id).exact_size
/// (CHROME_HEIGHT_POINTS).frame(...)` shell separately. That duplication
/// was never just repetition -- three of these four had already silently
/// drifted out of sync with each other at least once: the Graph toolbar
/// got a compact-spacing override during a later palette pass (`apply_
/// style` bumped the app-wide default spacing airier, which this fixed
/// row height was never resized for), but the Tab toolbar and the
/// Settings page's own back-button row didn't get the same override --
/// both later overflowed the fixed row height for it, found live on two
/// separate occasions, each requiring its own investigation to notice.
/// A single shared constructor makes that whole *class* of bug
/// structurally impossible to reintroduce one toolbar at a time again:
/// every future toolbar automatically gets the same compact spacing this
/// row height needs, and the same horizontal-scroll overflow safety (see
/// the Graph toolbar's own former comment on this, now here instead) --
/// without a fresh author needing to already know why, or remembering to
/// copy it from whichever toolbar they happened to look at first.
///
/// `content` still owns its own internal layout (`horizontal_centered`
/// vs. `with_layout(.., Align::Min)`, etc.) -- callers differ there for
/// reasons specific to each row (see the Tab toolbar's own `Align::Min`
/// doc comment), so this only ever provides the shared *shell* around it.
pub(crate) fn toolbar_panel(id: &'static str, ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Panel::top(id)
        .exact_size(CHROME_HEIGHT_POINTS)
        .frame(egui::Frame::default().fill(TOOLBAR_FILL).inner_margin(egui::Margin::symmetric(8, 5)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
            // Buttons and fields are 28 px tall (the mockup's control height).
            ui.spacing_mut().interact_size.y = 28.0;
            control_font(ui);
            // Soft-filled fields and borderless buttons: the raised fill
            // alone separates them from the bar (approved toolbar mockup).
            ui.visuals_mut().extreme_bg_color = SURFACE_RAISED;
            ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::NONE;
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, content);
        });
}

/// Runs `add` with the approved mockup's field look: a soft fill (white at 6%) and a hairline border
/// (white at 14%) with 6 px corners, so a text field reads as an inset box rather than bare text on the bar.
pub(crate) fn toolbar_field<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        visuals.extreme_bg_color = egui::Color32::from_rgb(43, 40, 34);
        let border = egui::Stroke::new(1.0, egui::Color32::from_rgb(62, 59, 52));
        for w in [&mut visuals.widgets.inactive, &mut visuals.widgets.hovered] {
            w.bg_stroke = border;
            w.corner_radius = egui::CornerRadius::same(6);
        }
        add(ui)
    })
    .inner
}

/// A toolbar icon button whose hover fill/text color eases in/out instead
/// of snapping -- egui's stock `Button` picks its resting/hovered colors
/// fresh every frame with no built-in transition of its own (same reason
/// the graph cards needed their own `hover_t` easing earlier this
/// session). `id` must be unique per button *concept*, not per call site --
/// the same id reused across, say, the Tab and Graph toolbars' own gear
/// buttons is fine, since only one of those toolbars is ever showing in a
/// given frame to begin with. One frame of hover-state lag: this frame's
/// color comes from *last* frame's hover (stashed in `ctx` memory), since
/// a button's fill has to be picked before `ui.add` reveals whether the
/// pointer is actually over its not-yet-laid-out rect -- imperceptible at
/// any real frame rate, same "acceptable one-frame lag" the zoom-%
/// toolbar readout and the dot-grid background already accept elsewhere.
/// Deliberately scoped to every *icon-only* toolbar button (Reload/Home/
/// Screenshot/Print-to-PDF/Settings/Keyboard-Shortcuts/Settings-page-Back/
/// Close-Split/Import-Group/Ungroup-All/Collapse-Expand-Groups) rather than
/// every `ui.button` call in the app -- see README.md's own "Toolbar
/// icon-button hover easing" paragraph for why a full sweep
/// wasn't attempted.
pub(crate) fn icon_button_eased(ui: &mut egui::Ui, id: egui::Id, label: impl Into<String>) -> egui::Response {
    let was_hovered: bool = ui.ctx().memory(|m| m.data.get_temp(id).unwrap_or(false));
    let hover_t = ui.ctx().animate_bool_responsive(id, was_hovered);
    let bg =
        lerp_color32(ui.visuals().widgets.inactive.weak_bg_fill, ui.visuals().widgets.hovered.weak_bg_fill, hover_t);
    let fg = lerp_color32(
        ui.visuals().widgets.inactive.fg_stroke.color,
        ui.visuals().widgets.hovered.fg_stroke.color,
        hover_t,
    );
    let response =
        ui.add(egui::Button::new(egui::RichText::new(label.into()).color(fg)).fill(bg).min_size(egui::vec2(32.0, 0.0)));
    ui.ctx().memory_mut(|m| m.data.insert_temp(id, response.hovered()));
    response
}

pub(crate) fn chip_icon_button(ui: &mut egui::Ui, id: egui::Id, icon: impl Into<String>) -> egui::Response {
    chip_icon_button_enabled(ui, id, icon, true)
}

/// Same as `chip_icon_button`, but for icons that need a disabled state
/// inside a chip (e.g. nav back/forward, dimmed when there's no history) --
/// `chip_icon_button` alone can't express `ui.add_enabled`'s disabled
/// styling since it always calls `ui.add`.
///
/// Same hover-eased feedback as `icon_button_eased`, but with no visible
/// border ever (idle or hovered) and a transparent idle fill -- for icons
/// already sitting inside a shared background chip (see `toolbar_chip`),
/// where `icon_button_eased`'s own always-drawn box reads as a border
/// nested inside the chip's own border. Deliberately still a real,
/// fully-framed `egui::Button` (not `.frame(false)`, which shrinks the
/// widget's own allocated size well below a normal button's and was
/// found live to make a chip noticeably shorter than its "+ New Tab"/
/// "Reset View" neighbors) -- keeping the frame but making it invisible
/// at rest matches every sibling button's sizing exactly while still
/// hiding the box until the icon is hovered, at which point a soft fill
/// (no stroke) confirms the hit target.
pub(crate) fn chip_icon_button_enabled(
    ui: &mut egui::Ui,
    id: egui::Id,
    icon: impl Into<String>,
    enabled: bool,
) -> egui::Response {
    let was_hovered: bool = ui.ctx().memory(|m| m.data.get_temp(id).unwrap_or(false));
    let hover_t = ui.ctx().animate_bool_responsive(id, was_hovered);
    let bg = lerp_color32(egui::Color32::TRANSPARENT, ui.visuals().widgets.hovered.weak_bg_fill, hover_t);
    let fg = lerp_color32(
        ui.visuals().widgets.inactive.fg_stroke.color,
        ui.visuals().widgets.hovered.fg_stroke.color,
        hover_t,
    );
    let response = add_borderless(
        ui,
        enabled,
        egui::Button::new(egui::RichText::new(icon.into()).color(fg)).fill(bg).min_size(egui::vec2(32.0, 26.0)),
    );
    ui.ctx().memory_mut(|m| m.data.insert_temp(id, response.hovered()));
    response
}

/// Paints a raised chip background behind `content` -- e.g. a cluster of
/// `chip_icon_button`s that should read as one grouped control instead of
/// a flat row of buttons. Uses egui's "reserve then backfill" idiom
/// (`Shape::Noop` now, `Painter::set` once the real size is known) since
/// the chip's own size depends on content not yet laid out.
///
/// `content` MUST run directly against the `ui` it's given -- never
/// inside its own nested `ui.horizontal`/`ui.new_child`. A nested Ui
/// becomes an independently-sized layout block that the OUTER row then
/// centers (or top-aligns) using its own possibly-smaller reported
/// height, which is exactly what caused a confirmed-live ~5px vertical
/// misalignment in the `toolbar-chip-wip` prototype: the outer row
/// centered the nested block within its own taller row height, and no
/// amount of forcing the nested block's *height* fixed it, because the
/// bug was the block's *existence*, not its size. `content` must instead
/// call the SAME `ui` for every widget (making them direct siblings of
/// whatever sits before/after the chip in the row) and return the union
/// of its own widgets' rects -- exact by construction, since each
/// widget's own returned `Response::rect` is real.
pub(crate) fn toolbar_chip(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> egui::Rect) {
    // A hairline between this chip and whatever came before it in the row.
    if ui.min_rect().width() > 1.0 {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, SURFACE_HOVER);
    }
    let chip_bg_slot = ui.painter().add(egui::Shape::Noop);
    // 2 px of chip padding around 26 px buttons: a 30 px chip in a 28 px row.
    let (lead_rect, _) = ui.allocate_exact_size(egui::Vec2::new(2.0, 0.0), egui::Sense::hover());
    let (outer_spacing, outer_interact_y) = (ui.spacing().item_spacing.x, ui.spacing().interact_size.y);
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.spacing_mut().interact_size.y = 26.0;
    let content_rect = content(ui);
    ui.spacing_mut().item_spacing.x = outer_spacing;
    ui.spacing_mut().interact_size.y = outer_interact_y;
    let (trail_rect, _) = ui.allocate_exact_size(egui::Vec2::new(2.0, 0.0), egui::Sense::hover());
    let rect = lead_rect.union(content_rect).union(trail_rect).expand2(egui::vec2(0.0, 2.0));
    // Content that renders nothing can leave `ui.cursor()`'s fallback
    // infinite in one axis (`.max.x` for left-to-right layouts, `.min.x`
    // for right-to-left) -- skip painting rather than filling an
    // unbounded rect.
    if rect.is_finite() {
        ui.painter().set(chip_bg_slot, egui::Shape::rect_filled(rect, 8.0, TOOLBAR_FILL.gamma_multiply(1.6)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Easing in from transparent keeps the target's color and only raises its opacity: a lerp of the unmultiplied
    /// channels from transparent black dipped through a darker tint than either end (the chip buttons' hover flash).
    #[test]
    fn lerp_from_transparent_keeps_the_targets_color() {
        let hover = egui::Color32::from_rgb(58, 50, 37);
        for t in [0.2, 0.5, 0.8, 1.0] {
            let [r, g, b, a] = lerp_color32(egui::Color32::TRANSPARENT, hover, t).to_srgba_unmultiplied();
            assert!(r.abs_diff(58) <= 3 && g.abs_diff(50) <= 3 && b.abs_diff(37) <= 3, "t={t}: {r},{g},{b}");
            assert_eq!(a, (255.0 * t).round() as u8, "t={t}");
        }
        assert_eq!(lerp_color32(egui::Color32::TRANSPARENT, hover, 0.0), egui::Color32::TRANSPARENT);
        let (a, b) = (egui::Color32::from_rgb(10, 20, 30), egui::Color32::from_rgb(110, 120, 130));
        assert_eq!(lerp_color32(a, b, 0.5), egui::Color32::from_rgb(60, 70, 80));
    }

    /// A click on a `toggle` reports `clicked()`; the toggle is 30x17 and draws no text.
    #[test]
    fn toggle_reports_clicks() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        let frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut hit = (false, egui::Vec2::ZERO);
            let mut out = ctx.run_ui(input, |ui| {
                let r = toggle(ui, false, "Test");
                hit = (r.clicked(), r.rect.size());
            });
            out.textures_delta.clear();
            hit
        };
        let at = egui::pos2(15.0, 8.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let (_, size) = frame(vec![egui::Event::PointerMoved(at)]);
        assert_eq!(size, egui::vec2(30.0, 17.0));
        frame(vec![button(true)]);
        let (clicked, _) = frame(vec![button(false)]);
        assert!(clicked);
    }

    /// An icon button given `named_tip` is announced by its name, not its glyph.
    #[test]
    fn named_tip_labels_the_button_for_screen_readers() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            ui.add(egui::Button::new(egui_phosphor::light::HOUSE)).named_tip("Go to home page");
        });
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update.expect("accesskit enabled");
        assert!(update.nodes.iter().any(|(_, n)| n.label() == Some("Go to home page")), "no node carries the name");
    }

    /// A button in a `control_font` Ui is laid out in Familjen Grotesk, which is narrower than Literata
    /// at the same size; a plain Ui keeps Literata.
    #[test]
    fn control_font_switches_button_family() {
        let ctx = egui::Context::default();
        apply_fonts(&ctx);
        apply_style(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0))),
            ..Default::default()
        };
        // Fonts set with `set_fonts` take effect on the next frame.
        ctx.run_ui(input.clone(), |_| {}).textures_delta.clear();
        let (mut plain, mut control) = (0.0, 0.0);
        let mut out = ctx.run_ui(input, |ui| {
            plain = ui.add(egui::Button::new("Developer Tools")).rect.width();
            control_font(ui);
            control = ui.add(egui::Button::new("Developer Tools")).rect.width();
        });
        out.textures_delta.clear();
        assert!(control < plain - 5.0, "control {control} should be narrower than plain {plain}");
    }

    /// The size of the widget `add` lays out, after a few frames with the mouse over it or not
    /// (a widget's state comes from the previous frame, so one frame is not enough).
    fn size_of(hover: bool, add: &dyn Fn(&mut egui::Ui) -> egui::Response) -> egui::Vec2 {
        let ctx = egui::Context::default();
        apply_style(&ctx);
        let mut size = egui::Vec2::ZERO;
        let mut hovered = false;
        for _ in 0..4 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 200.0))),
                ..Default::default()
            };
            if hover {
                input.events.push(egui::Event::PointerMoved(egui::pos2(12.0, 10.0)));
            }
            let mut output = ctx.run_ui(input, |ui| {
                let response = add(ui);
                size = response.rect.size();
                hovered = response.hovered();
            });
            output.textures_delta.clear(); // nothing uploads the font atlas here
        }
        assert_eq!(hovered, hover, "the pointer should be over the widget exactly when hovering (size {size:?})");
        size
    }

    /// Hovering must not resize a button: it would shift everything after it in the row.
    #[test]
    fn hovering_a_widget_does_not_resize_it() {
        // The toolbar's spacing and unframed resting buttons, as `toolbar_panel` sets them.
        fn in_toolbar(ui: &mut egui::Ui) {
            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
            ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::NONE;
        }
        type Case = (&'static str, Box<dyn Fn(&mut egui::Ui) -> egui::Response>);
        let cases: Vec<Case> = vec![
            ("primary_button", Box::new(|ui| primary_button(ui, "+ New Tab"))),
            (
                "chip_icon_button",
                Box::new(|ui| {
                    in_toolbar(ui);
                    chip_icon_button_enabled(ui, egui::Id::new("icon"), "X", true)
                }),
            ),
            (
                "primary_button in the toolbar",
                Box::new(|ui| {
                    in_toolbar(ui);
                    primary_button(ui, "+ New Tab")
                }),
            ),
            ("button", Box::new(|ui| ui.button("Save"))),
            ("selectable_label", Box::new(|ui| selectable_label(ui, false, "Mute"))),
            ("selected selectable_label", Box::new(|ui| selectable_label(ui, true, "Mute"))),
        ];
        for (name, add) in cases {
            let rest = size_of(false, &*add);
            let hover = size_of(true, &*add);
            assert_eq!(rest, hover, "{name}: {rest:?} at rest, {hover:?} hovered");
        }
    }
}
