//! The app's look: palette, egui style and fonts, shared popup/dialog
//! frames, the toolbar panel and chip widgets, and the dark title bar.

use super::*;

/// Fixed tag-color choices a group or tab card's color swatch cycles
/// through (see `next_tag_color`) -- vermillion/orange/yellow/bluish
/// green/sky blue/reddish purple, chosen for mutual distinguishability
/// against this app's dark chrome rather than any particular meaning.
/// Used via the now-shared `GraphNode::color`, for both `Tab` and
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
        .corner_radius(4u8)
        .inner_margin(egui::Margin::same(4))
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
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
}

/// `popup_frame` for centered dialogs (quick switcher, shortcuts help,
/// import confirm, JSON viewer): rounder, with more padding.
pub(crate) fn dialog_frame(ui: &egui::Ui) -> egui::Frame {
    popup_frame(ui).corner_radius(6u8).inner_margin(egui::Margin::same(14))
}

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
pub(crate) fn apply_fonts(ctx: &egui::Context) {
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
    ctx.set_fonts(fonts);
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
/// the hover glow.
pub(crate) fn lerp_color32(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp_u8 = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgba_unmultiplied(
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
        .frame(egui::Frame::default().fill(TOOLBAR_FILL).inner_margin(egui::Margin::symmetric(8, 3)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, content);
        });
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
    let response = ui.add(egui::Button::new(egui::RichText::new(label.into()).color(fg)).fill(bg));
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
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(icon.into()).color(fg)).fill(bg).stroke(egui::Stroke::NONE),
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
    let chip_bg_slot = ui.painter().add(egui::Shape::Noop);
    let (lead_rect, _) = ui.allocate_exact_size(egui::Vec2::new(1.0, 0.0), egui::Sense::hover());
    ui.spacing_mut().item_spacing.x = 2.0;
    let content_rect = content(ui);
    let (trail_rect, _) = ui.allocate_exact_size(egui::Vec2::new(1.0, 0.0), egui::Sense::hover());
    let rect = lead_rect.union(content_rect).union(trail_rect);
    // Content that renders nothing can leave `ui.cursor()`'s fallback
    // infinite in one axis (`.max.x` for left-to-right layouts, `.min.x`
    // for right-to-left) -- skip painting rather than filling an
    // unbounded rect.
    if rect.is_finite() {
        ui.painter().set(chip_bg_slot, egui::Shape::rect_filled(rect, 6.0, TOOLBAR_FILL.gamma_multiply(1.6)));
    }
}
