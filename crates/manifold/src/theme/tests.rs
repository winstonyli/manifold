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
/// from its right.
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

/// Headless mode's caret stays drawn in both themes, so two shots of a focused text box match.
#[test]
fn a_steady_caret_does_not_blink_in_either_theme() {
    let ctx = egui::Context::default();
    apply_style(&ctx);
    steady_caret(&ctx);
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        assert!(!ctx.style_of(theme).visuals.text_cursor.blink, "{theme:?}");
    }
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
