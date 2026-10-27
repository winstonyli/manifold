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

/// Fixed tag-color choices for a label's colour (see `free_tag_color`) -- vermillion/orange/yellow/bluish
/// green/sky blue/reddish purple, chosen for mutual distinguishability
/// against this app's dark chrome rather than any particular meaning.
/// Used via `Mirror::color` for `Label` and jar nodes. These are the Okabe-Ito colorblind-safe palette values
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

/// `TAG_COLOR_PALETTE` for the light theme, entry for entry: the same hues, with the three too pale for a white card
/// (orange-yellow, yellow, sky blue) deepened so every swatch keeps 3:1 against it. A tag is stored as its
/// `TAG_COLOR_PALETTE` entry in either theme; `tag_shown` picks what is painted.
const LIGHT_TAG_COLOR_PALETTE: [egui::Color32; 6] = [
    TAG_COLOR_PALETTE[0],
    egui::Color32::from_rgb(170, 105, 0),
    egui::Color32::from_rgb(140, 122, 0),
    TAG_COLOR_PALETTE[3],
    egui::Color32::from_rgb(0, 114, 178),
    TAG_COLOR_PALETTE[5],
];

/// The color to paint for a stored tag color: its entry of the theme's table (a color outside the palette is
/// painted as stored).
pub(crate) fn tag_shown(stored: egui::Color32, dark: bool) -> egui::Color32 {
    match TAG_COLOR_PALETTE.iter().position(|&p| p == stored) {
        Some(i) if !dark => LIGHT_TAG_COLOR_PALETTE[i],
        _ => stored,
    }
}

/// Background, border and padding shared by every menu and dropdown
/// (right-click menus, toolbar lists, the address-bar suggestions).
pub(crate) fn popup_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::default()
        .fill(ui.visuals().window_fill)
        .stroke(egui::Stroke::new(1.0, ui.visuals().weak_text_color()))
        .corner_radius(RADIUS)
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
    let t = animate(ui.ctx(), response.id, on, egui::emath::easing::cubic_out);
    let off_fill = ui.visuals().window_fill.gamma_multiply(1.6);
    let fill = lerp_color32(off_fill, accent(ui.visuals()), t);
    let knob = lerp_color32(ui.visuals().weak_text_color(), ui.visuals().panel_fill, t);
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

/// A tooltip that also names the widget for a screen reader (icon buttons otherwise announce their glyph) and for the
/// gate, which finds the control by name in the state export's `controls` instead of by pixel.
pub(crate) trait NamedTip {
    fn named_tip(self, text: impl Into<String>) -> Self;
}

impl NamedTip for egui::Response {
    fn named_tip(self, text: impl Into<String>) -> Self {
        let text = text.into();
        let enabled = self.enabled();
        self.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text.clone()));
        crate::identity::note_control(&self.ctx, self.layer_id, &text, self.rect);
        self.on_hover_text(text)
    }
}

/// Paints `keys` as keycaps right-aligned at `right_center`, with no layout: for a hint drawn over a field.
pub(crate) fn paint_keycaps(ui: &egui::Ui, right_center: egui::Pos2, keys: &[&str]) {
    let font = egui::FontId::new(11.0, egui::FontFamily::Proportional);
    let visuals = ui.visuals();
    let (weak, fill, border) =
        (palette(visuals).text_weak, visuals.widgets.inactive.bg_fill, visuals.widgets.hovered.bg_fill);
    let mut right = right_center.x;
    for key in keys.iter().rev() {
        let galley = ui.painter().layout_no_wrap((*key).to_string(), font.clone(), weak);
        let size = galley.size() + egui::vec2(8.0, 2.0);
        let rect = egui::Rect::from_min_size(egui::pos2(right - size.x, right_center.y - size.y / 2.0 - 1.0), size);
        ui.painter().rect(rect, 3.0, fill, egui::Stroke::new(1.0, border), egui::StrokeKind::Inside);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, weak);
        right = rect.min.x - 3.0;
    }
}

/// A key combination as keycaps in the current row: "Ctrl+Shift+C" is three, and "A / B"
/// is two combinations around a weak slash.
pub(crate) fn kbd_keys(ui: &mut egui::Ui, keys: &str) {
    // Scoped: the tight keycap gap must not reach what the caller lays out next.
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = KEY_GAP;
        for (i, alternative) in keys.split(" / ").enumerate() {
            if i > 0 {
                ui.label(egui::RichText::new("/").weak());
            }
            for key in alternative.split('+').filter(|k| !k.is_empty()) {
                kbd(ui, key);
            }
        }
    });
}

/// Gap between keycaps, and between a hint row's items.
const KEY_GAP: f32 = 3.0;

/// One row of "keycaps + what they do" pairs, with the tight hint-row spacing.
pub(crate) fn key_hint_row<'a>(ui: &mut egui::Ui, hints: impl Iterator<Item = (&'a str, &'a str)>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = KEY_GAP;
        for (keys, what) in hints {
            kbd_keys(ui, keys);
            ui.label(egui::RichText::new(what).small().weak());
            ui.add_space(4.0);
        }
    });
}

/// A floating, non-interactive bar of "keycaps + what they do" pairs.
pub(crate) fn key_hint_strip<'a>(
    ctx: &egui::Context,
    area: egui::Area,
    hints: impl Iterator<Item = (&'a str, &'a str)>,
) {
    area.order(egui::Order::Foreground).interactable(false).show(ctx, |ui| {
        popup_frame(ui).show(ui, |ui| {
            key_hint_row(ui, hints);
        });
    });
}

/// `color` as `[r, g, b]`.
pub(crate) fn rgb(color: egui::Color32) -> [u8; 3] {
    [color.r(), color.g(), color.b()]
}

/// An outline on a keyboard-focused widget. egui shows focus only as the widget's
/// "active" look, which is invisible on a button whose fill is overridden or that
/// has no frame; this paints inside the widget's rect, so it moves nothing.
pub(crate) fn focus_ring(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            2.0,
            egui::Stroke::new(1.5, palette(ui.visuals()).text),
            egui::StrokeKind::Inside,
        );
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
    let fill = accent(ui.visuals());
    filled_button(ui, label, fill)
}

/// A dialog's main button when it destroys something: the same solid fill in the danger color, so the colour says
/// which kind of action it is (a gold "Delete" would read as the safe default).
pub(crate) fn danger_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let fill = danger(ui.visuals());
    filled_button(ui, label, fill)
}

fn filled_button(ui: &mut egui::Ui, label: &str, fill: egui::Color32) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding.x = 11.0;
        let ink = palette(ui.visuals()).on_accent;
        add_borderless(
            ui,
            true,
            egui::Button::new(
                egui::RichText::new(label)
                    .font(egui::FontId::new(14.0, bound_family(ui.ctx(), bold_family())))
                    .color(ink),
            )
            .fill(fill),
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
    let style = ui.style_mut();
    style.spacing.item_spacing.y = 0.0;
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    // 26 px rows with 5 px hover corners, as in the mockup's menu.
    style.spacing.interact_size.y = 26.0;
    for w in
        [&mut style.visuals.widgets.hovered, &mut style.visuals.widgets.active, &mut style.visuals.widgets.inactive]
    {
        w.corner_radius = egui::CornerRadius::same(RADIUS_ITEM);
    }
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
}

/// Space above and below a menu's divider line.
pub(crate) const MENU_SEPARATOR_PAD: f32 = 3.0;

/// The divider between groups of rows in a menu: one rule, padded alike in every menu.
pub(crate) fn menu_separator(ui: &mut egui::Ui) {
    ui.add_space(MENU_SEPARATOR_PAD);
    ui.separator();
    ui.add_space(MENU_SEPARATOR_PAD);
}

/// `popup_frame` for one-row floating bars (find bar, toasts): a little more air than a menu, less than a dialog.
pub(crate) fn bar_frame(ui: &egui::Ui) -> egui::Frame {
    popup_frame(ui).inner_margin(egui::Margin::symmetric(10, 6))
}

/// `popup_frame` for centered dialogs (quick switcher, shortcuts help,
/// import confirm, JSON viewer): rounder, with more padding.
pub(crate) fn dialog_frame(ui: &egui::Ui) -> egui::Frame {
    popup_frame(ui).corner_radius(6u8).inner_margin(egui::Margin::same(14))
}

/// The first `TAG_COLOR_PALETTE` entry that none of `used` has, else the entry the `used.len()`th wraps to: what a new
/// jar or label takes, so the first few differ.
pub(crate) fn free_tag_color(used: &[Option<egui::Color32>]) -> egui::Color32 {
    let has = |c: &egui::Color32| used.iter().any(|u| u.unwrap_or(TAG_COLOR_PALETTE[0]) == *c);
    TAG_COLOR_PALETTE
        .iter()
        .copied()
        .find(|c| !has(c))
        .unwrap_or(TAG_COLOR_PALETTE[used.len() % TAG_COLOR_PALETTE.len()])
}

/// A `manifold_pages::palette` entry as an egui color: the palette is defined once, there, and shared with the
/// `manifold://` pages.
const fn color([r, g, b]: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(r, g, b)
}

/// Corner radii: the base (widgets, windows, menus), a menu or dropdown row, and a toolbar chip.
pub(crate) const RADIUS: u8 = 8;
pub(crate) const RADIUS_ITEM: u8 = 6;
pub(crate) const RADIUS_CHIP: u8 = 10;
/// A graph card's corner radius, in graph units (multiply by `inv_zoom` for screen space).
pub(crate) const CARD_RADIUS: f32 = 12.0;

/// Text on a dark scrim over a card (the title strip, an image's caption, the jar chip): near white. The scrims are
/// dark in both themes, so this does not change with the theme.
pub(crate) const SCRIM_TEXT: egui::Color32 = egui::Color32::from_gray(240);
/// The weaker line of `SCRIM_TEXT` (a tab card's source).
pub(crate) const SCRIM_TEXT_WEAK: egui::Color32 = egui::Color32::from_gray(190);

/// The yellow entry of `TAG_COLOR_PALETTE`: the one hue that is far from both the blue accent and the red danger.
pub(crate) const TAG_YELLOW: egui::Color32 = TAG_COLOR_PALETTE[2];

/// A drag that a release would refuse (keeping what it holds): the theme's yellow tag color, a hue far from the accent (a valid drop) and the danger red (a deletion), so the outcomes differ by hue, not
/// brightness.
pub(crate) fn refused_color(visuals: &egui::Visuals) -> egui::Color32 {
    tag_shown(TAG_YELLOW, visuals.dark_mode)
}

/// A sticky note's card fill -- a warm, light color deliberately unlike
/// every other card's dark `extreme_bg_color`/`window_fill` tone, so a note
/// reads as "a physical note" at a glance rather than another chrome-toned
/// panel. The text drawn on it (`NotePayload`'s editable body) is dark to
/// stay legible against this light fill, the one card in this app that
/// isn't light-text-on-dark.
pub(crate) const NOTE_CARD_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 236, 148);

/// What `Visuals` has no exact slot for, per theme (`palette` picks by `Visuals::dark_mode`). Text lives here, not in
/// `Visuals`: `Visuals::text_color()` is the weak tan and `weak_text_color()` is derived from it, so mapping these
/// onto egui's slots would change every plain label and every `weak_text_color()` caller.
pub(crate) struct Palette {
    pub text: egui::Color32,
    pub text_weak: egui::Color32,
    /// Text on a solid accent or danger fill (`filled_button`, the selection): the dark ground on the light accent,
    /// white on the light theme's deep one.
    pub on_accent: egui::Color32,
    /// A darker rust than `danger`, for a solid fill that carries `text` (the graph close button's hover).
    pub danger_fill: egui::Color32,
    pub success: egui::Color32,
    /// The window and canvas ground: the clear color behind everything the app paints.
    pub canvas: egui::Color32,
    /// Peak alpha of a card's soft shadow.
    pub shadow: u8,
}

/// Ink on a tag color: it sits on the tag, not on the theme, so it does not change with the theme.
pub(crate) const INK_DARK: egui::Color32 = egui::Color32::from_rgb(23, 20, 15);
pub(crate) const INK_LIGHT: egui::Color32 = egui::Color32::from_rgb(233, 226, 211);

static DARK_PALETTE: Palette = Palette {
    text: color(manifold_pages::palette::DARK.text_primary),
    text_weak: color(manifold_pages::palette::DARK.text_weak),
    on_accent: color(manifold_pages::palette::DARK.bg),
    danger_fill: egui::Color32::from_rgb(150, 52, 38),
    success: egui::Color32::from_rgb(150, 190, 90),
    canvas: color(manifold_pages::palette::DARK.bg),
    shadow: 70,
};

static LIGHT_PALETTE: Palette = Palette {
    text: color(manifold_pages::palette::LIGHT.text_primary),
    text_weak: color(manifold_pages::palette::LIGHT.text_weak),
    on_accent: color(manifold_pages::palette::LIGHT.surface_raised),
    danger_fill: egui::Color32::from_rgb(230, 125, 105),
    success: egui::Color32::from_rgb(38, 112, 42),
    canvas: color(manifold_pages::palette::LIGHT.bg),
    shadow: 28,
};

pub(crate) fn palette(visuals: &egui::Visuals) -> &'static Palette {
    if visuals.dark_mode {
        &DARK_PALETTE
    } else {
        &LIGHT_PALETTE
    }
}

/// The accent: the selection fill, which `fill_style` also uses for hyperlinks and the active widget.
pub(crate) fn accent(visuals: &egui::Visuals) -> egui::Color32 {
    visuals.selection.bg_fill
}

/// `ctx.animate_bool_with_easing`, and the one place the app eases a value: a pass that draws one between its ends
/// is recorded, so `WAITIDLE` (`App::debug_idle`, through `eased_last_pass`) waits for fades and hover easing to
/// finish. Ease through this, not egui's `animate_bool*`, or `WAITIDLE` won't see it.
pub(crate) fn animate(ctx: &egui::Context, id: egui::Id, on: bool, easing: fn(f32) -> f32) -> f32 {
    let t = ctx.animate_bool_with_easing(id, on, easing);
    if t > 0.0 && t < 1.0 {
        // `cumulative_pass_nr` counts completed passes, so during a pass it is this pass's index.
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(easing_pass_id(), pass));
    }
    t
}

/// Whether the last completed pass drew an `animate` value between its ends.
pub(crate) fn eased_last_pass(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<u64>(easing_pass_id())).is_some_and(|pass| pass + 1 == ctx.cumulative_pass_nr())
}

fn easing_pass_id() -> egui::Id {
    egui::Id::new("theme_easing_pass")
}

/// The one severity-red, for text, icons and borders.
pub(crate) fn danger(visuals: &egui::Visuals) -> egui::Color32 {
    visuals.error_fg_color
}

/// Fills egui's dark and light style slots (`style_mut_of` keeps one `Style` per theme), so `set_theme` switches the
/// whole chrome. Each slot starts from egui's own preset for its theme, so every field the table below does not name
/// is coherent for that theme, then gets the spacing, shape and the `manifold_pages::palette` colors: the
/// bg, toolbar, raised and hover depth ladder and the accent through selection, hyperlink,
/// focus and the active widget. Which slot shows is `apply_theme_choice`'s.
pub(crate) fn apply_style(ctx: &egui::Context) {
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        ctx.style_mut_of(theme, |style| fill_style(style, dark));
    }
}

/// The theme setting as egui's preference: `System` follows winit's theme events.
pub(crate) fn apply_theme_choice(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(match choice {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
    });
}

/// True on the first call and whenever `now` differs from the last one seen: the frame loop's cue to re-apply what
/// follows the theme (title bar, `manifold://` pages).
pub(crate) fn theme_changed(last: &mut Option<egui::Theme>, now: egui::Theme) -> bool {
    last.replace(now) != Some(now)
}

fn fill_style(style: &mut egui::Style, dark: bool) {
    style.visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    if !dark {
        // egui's light orange is 2.7:1 on the paper canvas; the jar chip and its dots need 3:1.
        style.visuals.warn_fg_color = egui::Color32::from_rgb(176, 84, 0);
    }
    // Airier than egui's own defaults on purpose -- the editorial
    // direction this app's chrome now takes wants real breathing room in
    // dialogs/menus/settings. The one place this is deliberately *not*
    // wanted -- the graph view's own toolbar, already packed with icon
    // buttons and genuinely screen-space-constrained -- pushes narrower
    // values back onto its own `Ui` locally (see wherever that toolbar
    // is drawn) rather than fighting this global default.
    style.spacing.item_spacing = egui::vec2(10.0, 7.0);
    style.spacing.button_padding = egui::vec2(13.0, 7.0);

    let c = manifold_pages::palette::colors(dark);
    let (bg, toolbar, raised, hover) =
        (color(c.bg), color(c.toolbar_fill), color(c.surface_raised), color(c.surface_hover));
    let (outline, text, weak) = (color(c.outline), color(c.text_primary), color(c.text_weak));
    let (accent, danger) = (color(c.accent), color(c.danger));

    let corner_radius: egui::CornerRadius = RADIUS.into();
    style.visuals.widgets.noninteractive.corner_radius = corner_radius;
    style.visuals.widgets.noninteractive.bg_fill = toolbar;
    style.visuals.widgets.noninteractive.weak_bg_fill = toolbar;
    style.visuals.widgets.noninteractive.fg_stroke.color = weak;

    style.visuals.widgets.inactive.corner_radius = corner_radius;
    style.visuals.widgets.inactive.bg_fill = raised;
    style.visuals.widgets.inactive.weak_bg_fill = raised;
    style.visuals.widgets.inactive.fg_stroke.color = text;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, hover);

    style.visuals.widgets.hovered.corner_radius = corner_radius;
    style.visuals.widgets.hovered.bg_fill = hover;
    style.visuals.widgets.hovered.weak_bg_fill = hover;
    style.visuals.widgets.hovered.fg_stroke.color = text;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, outline);

    // Focused, pressed or just clicked. egui gives the active ink to widgets that paint no fill too (a clickable label,
    // a checkbox's text, `RichText::strong`, `Spinner`), so it stays the primary text on the hover fill, and the
    // accent outline marks the state. Ink on a solid accent fill is `Palette::on_accent`.
    style.visuals.widgets.active.corner_radius = corner_radius;
    style.visuals.widgets.active.bg_fill = hover;
    style.visuals.widgets.active.weak_bg_fill = hover;
    style.visuals.widgets.active.fg_stroke.color = text;
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);

    style.visuals.widgets.open.corner_radius = corner_radius;
    style.visuals.widgets.open.bg_fill = raised;
    style.visuals.widgets.open.weak_bg_fill = raised;
    style.visuals.widgets.open.fg_stroke.color = text;
    style.visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, accent);

    style.visuals.window_corner_radius = RADIUS.into();
    style.visuals.menu_corner_radius = RADIUS.into();
    style.visuals.window_fill = toolbar;
    style.visuals.panel_fill = bg;
    style.visuals.extreme_bg_color = bg;
    style.visuals.faint_bg_color = raised;
    style.visuals.window_stroke = egui::Stroke::new(1.0, raised);

    style.visuals.selection.bg_fill = accent;
    // Text drawn on the selection fill (a selected row, a toggle).
    style.visuals.selection.stroke.color = if dark { DARK_PALETTE.on_accent } else { LIGHT_PALETTE.on_accent };
    style.visuals.hyperlink_color = accent;
    style.visuals.override_text_color = None;
    style.visuals.error_fg_color = danger;
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

/// Familjen Grotesk SemiBold: the primary button and every title (`heading_text`).
pub(crate) fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("control_bold".into())
}

/// Familjen Grotesk is the app's one family: Regular for all text (inserted ahead of egui's bundled default in the
/// proportional family, so a glyph outside its Latin coverage falls back to the default and from there to egui's
/// emoji and symbol fonts), SemiBold as `bold_family()` for titles and the primary button. The icon font (Phosphor
/// Light) is added before both so their chains carry its glyphs. Familjen's line box sits its caps ~0.075 em below
/// centre; the tweak lifts the glyphs so labels look centred. Both files are static instances of Google's
/// variable-only sources (`fonttools varLib.instancer`): egui/ab_glyph has no variable-axis support.
pub(crate) fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Light);
    for (name, bytes) in [
        ("FamiljenGrotesk", include_bytes!("../assets/fonts/FamiljenGrotesk-Regular.ttf").as_slice()),
        ("FamiljenGrotesk-SemiBold", include_bytes!("../assets/fonts/FamiljenGrotesk-SemiBold.ttf").as_slice()),
    ] {
        let tweak = egui::FontTweak { y_offset_factor: -0.06, ..Default::default() };
        fonts.font_data.insert(name.to_owned(), std::sync::Arc::new(egui::FontData::from_static(bytes).tweak(tweak)));
    }
    let proportional = fonts.families.get_mut(&egui::FontFamily::Proportional).expect("egui defines it");
    proportional.insert(0, "FamiljenGrotesk".to_owned());
    let mut bold = proportional.clone();
    bold[0] = "FamiljenGrotesk-SemiBold".to_owned();
    fonts.families.insert(bold_family(), bold);
    fonts
}

/// Installs `font_definitions` in `ctx`.
pub(crate) fn apply_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

/// A dialog title or settings-section header — `bold_family()` at a size that reads as a real step up from body text, in
/// the primary text color. Every such title in this app's chrome goes through this
/// one helper rather than each popup separately picking a size/weight, so
/// they stay visually identical to each other by construction.
pub(crate) fn heading_text(visuals: &egui::Visuals, text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).font(egui::FontId::new(17.0, bold_family())).color(palette(visuals).text)
}

/// The native window's theme for the setting (`None` follows the system). winit owns the title bar and re-applies it
/// on system changes, so a fixed choice is set here rather than through DWM, which winit would reset. A full
/// custom-drawn title bar was considered and deferred: it would mean reimplementing drag, minimize, maximize and
/// close hit-testing.
pub(crate) fn window_theme(choice: ThemeChoice) -> Option<winit::window::Theme> {
    match choice {
        ThemeChoice::System => None,
        ThemeChoice::Light => Some(winit::window::Theme::Light),
        ThemeChoice::Dark => Some(winit::window::Theme::Dark),
    }
}

/// `color` at `alpha` (0-255), its own hue unchanged: a wash or a ring in the card's or the accent's color.
pub(crate) fn with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Per-channel linear interpolation between two colors -- `egui`/`epaint`
/// has no built-in `Color32::lerp`. Used to ease a card's border color
/// between its resting and hovered tint instead of snapping instantly,
/// matching the same eased feel `animate` with `cubic_out` already gives
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
        .frame(egui::Frame::default().fill(ui.visuals().window_fill).inner_margin(egui::Margin::symmetric(8, 5)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
            // Buttons and fields are 28 px tall (the mockup's control height).
            ui.spacing_mut().interact_size.y = 28.0;
            // Soft-filled fields and borderless buttons: the raised fill
            // alone separates them from the bar (approved toolbar mockup).
            ui.visuals_mut().extreme_bg_color = ui.visuals().widgets.inactive.bg_fill;
            ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::NONE;
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, content);
        });
}

/// A single-line text field's shape, for every field drawn through `toolbar_field`: the control height (28 px, as
/// buttons) with the text centered in it, and an inset that keeps the first letter off the rounded end (egui's own
/// default margin is (4, 2) and top-aligned text).
pub(crate) fn field_edit(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    field_edit_inset(text, 8, 8)
}

/// `field_edit` with its own left and right text inset, for a field that draws something inside its ends (the address
/// bar's favicon and keycaps).
pub(crate) fn field_edit_inset(text: &mut dyn egui::TextBuffer, left: i8, right: i8) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .margin(egui::Margin { left, right, top: 4, bottom: 4 })
        .min_size(egui::vec2(0.0, 28.0))
        .vertical_align(egui::Align::Center)
}

/// Runs `add` with the approved mockup's field look: a soft raised fill and a hairline hover-step border
/// with `RADIUS_CHIP` corners, so a text field reads as an inset box rather than bare text on the bar.
pub(crate) fn toolbar_field<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let visuals = ui.visuals_mut();
        visuals.extreme_bg_color = visuals.widgets.inactive.bg_fill;
        let border = egui::Stroke::new(1.0, visuals.widgets.hovered.bg_fill);
        for w in [&mut visuals.widgets.inactive, &mut visuals.widgets.hovered] {
            w.bg_stroke = border;
            w.corner_radius = egui::CornerRadius::same(RADIUS_CHIP);
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
    let hover_t = animate(ui.ctx(), id, was_hovered, egui::emath::easing::cubic_out);
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
    let hover_t = animate(ui.ctx(), id, was_hovered, egui::emath::easing::cubic_out);
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
        ui.painter().rect_filled(rect, 0.0, ui.visuals().widgets.hovered.bg_fill);
    }
    let chip_bg_slot = ui.painter().add(egui::Shape::Noop);
    // 2 px of chip padding around 26 px buttons: a 30 px chip in a 28 px row.
    // The spacing that follows a widget is the one in force when it is laid out, so the lead padding is laid out
    // with none (else the first button sits `item_spacing` further in than the last is from the right edge), and the
    // trailing padding with the row's own, which is the gap to whatever follows the chip.
    let (outer_spacing, outer_interact_y) = (ui.spacing().item_spacing.x, ui.spacing().interact_size.y);
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.spacing_mut().interact_size.y = 26.0;
    let (lead_rect, _) = ui.allocate_exact_size(egui::Vec2::new(2.0, 0.0), egui::Sense::hover());
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
        let fill = ui.visuals().window_fill.gamma_multiply(1.6);
        ui.painter().set(chip_bg_slot, egui::Shape::rect_filled(rect, 8.0, fill));
    }
}

#[cfg(test)]
pub(crate) mod testutil;

#[cfg(test)]
mod tests {
    use super::*;
    use testutil::contrast;

    /// Every foreground role on every surface it is drawn on keeps its contrast, in each theme: 4.5:1 (WCAG AA) for
    /// text, 3:1 for the hover outline (a UI boundary). A new role goes in this table with the surfaces it sits on.
    #[test]
    fn colour_roles_keep_their_contrast_on_their_surfaces() {
        let mut failures = Vec::new();
        for dark in [true, false] {
            let t = manifold_pages::palette::colors(dark);
            let visuals = testutil::ctx(dark).global_style().visuals.clone();
            let p = palette(&visuals);
            let [bg, toolbar, raised, hover] = [t.bg, t.toolbar_fill, t.surface_raised, t.surface_hover].map(color);
            let [accent, danger, outline] = [t.accent, t.danger, t.outline].map(color);
            let active = visuals.widgets.active;
            let surfaces = [bg, toolbar, raised, hover];
            // A focused or pressed widget may paint its fill or none (a clickable label, a checkbox's text, `strong`).
            let under_active = [bg, toolbar, raised, hover, active.bg_fill, active.weak_bg_fill];
            // (role, color, the surfaces it is drawn on, the ratio it needs). Accent and danger text is drawn on the page
            // and raised surfaces only (a hovered row shows weak or primary text).
            let roles: [(&str, egui::Color32, &[egui::Color32], f32); 9] = [
                ("text", p.text, &surfaces, 4.5),
                ("text_weak", p.text_weak, &surfaces, 4.5),
                ("accent", accent, &[bg, toolbar, raised], 4.5),
                ("danger", danger, &[bg, toolbar, raised], 4.5),
                ("success", p.success, &[bg, toolbar, raised], 4.5),
                ("outline", outline, &[hover], 3.0),
                ("active text", active.fg_stroke.color, &under_active, 4.5),
                // Text on a solid fill: `filled_button` and the selection set `on_accent`; the close hover sets primary.
                ("on_accent", p.on_accent, &[accent, danger], 4.5),
                ("text on danger_fill", p.text, &[p.danger_fill], 4.5),
            ];
            for (name, color, surfaces, needed) in roles {
                for &surface in surfaces {
                    let got = contrast(color, surface);
                    if got < needed {
                        failures.push(format!("dark={dark}: {name} on {surface:?} is {got:.2}, needs {needed}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// `eased_last_pass` is true after a pass that drew a value between its ends, and false once it has settled.
    #[test]
    fn an_easing_value_marks_its_pass_until_it_settles() {
        let ctx = testutil::ctx(true);
        let id = egui::Id::new("easing_probe");
        let pass = |on: bool, time: f64| {
            let input = egui::RawInput { time: Some(time), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                animate(ui.ctx(), id, on, egui::emath::easing::linear);
            });
            output.textures_delta.clear(); // nothing uploads the font atlas here
            eased_last_pass(&ctx)
        };
        assert!(!pass(false, 0.0), "a value seen for the first time starts settled");
        assert!(pass(true, 0.01), "just switched on, it is between its ends");
        assert!(!pass(true, 10.0), "settled at 1");
    }

    #[test]
    fn card_and_canvas_differ_in_both_themes() {
        for dark in [true, false] {
            let c = manifold_pages::palette::colors(dark);
            assert_ne!(c.surface_raised, c.bg, "dark={dark}");
        }
    }

    #[test]
    fn every_tag_color_has_3_to_1_on_its_themes_card() {
        for dark in [true, false] {
            let card = color(manifold_pages::palette::colors(dark).surface_raised);
            for c in TAG_COLOR_PALETTE.map(|c| tag_shown(c, dark)) {
                assert!(contrast(c, card) >= 3.0, "{c:?} on the {} card", if dark { "dark" } else { "light" });
            }
        }
    }

    #[test]
    fn selection_ink_and_the_refused_color_read_in_both_themes() {
        for dark in [true, false] {
            let v = testutil::ctx(dark).global_style().visuals.clone();
            assert!(contrast(v.selection.stroke.color, v.selection.bg_fill) >= 4.5, "selection ink, dark={dark}");
            assert!(contrast(refused_color(&v), v.panel_fill) >= 3.0, "refused on the canvas, dark={dark}");
            let toolbar = color(manifold_pages::palette::colors(dark).toolbar_fill);
            for surface in [v.panel_fill, toolbar] {
                assert!(contrast(v.warn_fg_color, surface) >= 3.0, "warn (jar chip) on {surface:?}, dark={dark}");
            }
        }
    }

    #[test]
    fn theme_changed_reports_first_and_each_switch() {
        let mut last = None;
        let calls = [egui::Theme::Dark, egui::Theme::Dark, egui::Theme::Light, egui::Theme::Dark];
        assert_eq!(calls.map(|t| theme_changed(&mut last, t)), [true, false, true, true]);
    }

    #[test]
    fn palette_matches_the_visuals_theme() {
        for dark in [true, false] {
            let visuals = testutil::ctx(dark).global_style().visuals.clone();
            let t = manifold_pages::palette::colors(dark);
            assert_eq!(palette(&visuals).text, color(t.text_primary), "dark={dark}");
            assert_eq!(palette(&visuals).canvas, color(t.bg), "dark={dark}");
            assert_eq!(accent(&visuals), color(t.accent), "dark={dark}");
        }
    }

    #[test]
    fn palette_follows_visuals_dark_mode() {
        let ctx = testutil::ctx(true);
        let v = ctx.global_style().visuals.clone();
        assert_eq!(palette(&v).text, color(manifold_pages::palette::DARK.text_primary));
        assert_eq!(accent(&v), color(manifold_pages::palette::DARK.accent));
        assert_eq!(danger(&v), color(manifold_pages::palette::DARK.danger));
    }

    #[test]
    fn apply_style_sets_selection_and_error_colors() {
        let ctx = testutil::ctx(true);
        let v = ctx.global_style().visuals.clone();
        assert_eq!(v.selection.bg_fill, color(manifold_pages::palette::DARK.accent));
        assert_eq!(v.error_fg_color, color(manifold_pages::palette::DARK.danger));
    }

    /// Light text keeps AA contrast on the solid danger fill the graph close button shows on hover.
    #[test]
    fn danger_fill_carries_primary_text() {
        let p = &DARK_PALETTE;
        assert!(contrast(p.text, p.danger_fill) >= 4.5, "{}", contrast(p.text, p.danger_fill));
    }

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

    /// A button cluster is padded evenly: the first button sits as far from the chip's left edge as the last does
    /// from its right (the lead padding used to be followed by a full `item_spacing`).
    #[test]
    fn a_toolbar_chip_pads_both_sides_alike() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 100.0));
        let mut gaps = (0.0, 0.0);
        let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            ui.horizontal(|ui| {
                let start = ui.cursor().min.x;
                let (mut first, mut last) = (0.0, 0.0);
                toolbar_chip(ui, |ui| {
                    let a = chip_icon_button(ui, egui::Id::new("a"), "a").rect;
                    let b = chip_icon_button(ui, egui::Id::new("b"), "b").rect;
                    (first, last) = (a.min.x, b.max.x);
                    a.union(b)
                });
                let after = ui.cursor().min.x - ui.spacing().item_spacing.x;
                gaps = (first - start, after - last);
            });
        });
        out.textures_delta.clear();
        assert_eq!(gaps, (2.0, 2.0), "left and right padding of the chip");
    }

    /// A menu divider takes the same height wherever it sits, whatever spacing the menu has, and a keycap row
    /// leaves the caller's item spacing alone.
    #[test]
    fn menu_separator_height_is_fixed_and_kbd_keys_do_not_leak_spacing() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 100.0));
        let (mut height, mut before, mut after) = (0.0, 0.0, 0.0);
        let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            menu_item_style(ui);
            let top = ui.cursor().min.y;
            menu_separator(ui);
            height = ui.cursor().min.y - top;
            before = ui.spacing().item_spacing.x;
            kbd_keys(ui, "Ctrl+A");
            after = ui.spacing().item_spacing.x;
        });
        out.textures_delta.clear();
        assert_eq!(height, 2.0 * MENU_SEPARATOR_PAD + 6.0, "pad + rule + pad");
        assert_eq!(after, before, "kbd_keys leaves the caller's spacing alone");
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
