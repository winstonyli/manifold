use super::*;
use crate::card_kind::fixtures::{kinds, loaded_tab, plain, sleeping_tab};

/// Fix batch B3: a chip on a narrow card gives up, in turn, the end of its name, its name, then the old jar's dot
/// and arrow; it never runs past `max`.
#[test]
fn a_jar_chip_shrinks_to_fit_its_card() {
    let fit = |max, lead, name| fit_jar_chip(max, lead, name, 0.0);
    let whole = JAR_CHIP_PAD + 2.0 * JAR_CHIP_R + JAR_CHIP_NAME_GAP + 30.0 + JAR_CHIP_NAME_END;
    assert_eq!(fit(whole, 0.0, 30.0), Some(ChipFit { lead: false, name: Some(30.0), marker: false }));
    assert_eq!(fit(whole - 10.0, 0.0, 30.0), Some(ChipFit { lead: false, name: Some(20.0), marker: false }));
    assert_eq!(fit(whole + 20.0, 20.0, 30.0), Some(ChipFit { lead: true, name: Some(30.0), marker: false }));
    assert_eq!(fit(whole + 5.0, 20.0, 30.0), Some(ChipFit { lead: true, name: Some(15.0), marker: false }));
    assert_eq!(fit(40.0, 20.0, 30.0), Some(ChipFit { lead: true, name: None, marker: false }));
    assert_eq!(fit(30.0, 20.0, 30.0), Some(ChipFit { lead: false, name: None, marker: false }));
    assert_eq!(fit(10.0, 20.0, 30.0), None);
    // A name already narrower than the minimum is kept whole when it fits.
    assert_eq!(fit(whole - 20.0, 0.0, 10.0), Some(ChipFit { lead: false, name: Some(10.0), marker: false }));
}

/// Styled jar label markers: a chip whose label has a marker keeps all of the marker and ellipsizes only the name;
/// with no room for the marker and a short name, it falls back to the dots as one without a marker does.
#[test]
fn a_jar_chip_keeps_its_marker_while_the_name_shrinks() {
    let (name, marker) = (30.0, 20.0);
    let whole =
        JAR_CHIP_PAD + 2.0 * JAR_CHIP_R + JAR_CHIP_NAME_GAP + name + JAR_CHIP_MARKER_GAP + marker + JAR_CHIP_NAME_END;
    let fit = |max, lead| fit_jar_chip(max, lead, name, marker);
    let shown = |lead, name| Some(ChipFit { lead, name: Some(name), marker: true });
    assert_eq!(fit(whole, 0.0), shown(false, 30.0));
    assert_eq!(fit(whole - 10.0, 0.0), shown(false, 20.0), "the name gives way, the marker stays whole");
    assert_eq!(fit(whole - 16.0, 0.0), shown(false, JAR_CHIP_MIN_NAME));
    assert_eq!(fit(whole + 15.0, 20.0), shown(true, 25.0));
    let dots = Some(ChipFit { lead: false, name: None, marker: false });
    assert_eq!(fit(whole - 17.0, 0.0), dots, "no room for a short name and the marker: just the dots");
    assert_eq!(fit(30.0, 20.0), dots);
    assert_eq!(fit(10.0, 20.0), None);
}

/// The painted chip shows the marker as text of its own, whole, next to an ellipsized name.
#[test]
fn a_painted_jar_chip_draws_the_marker_whole() {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    let chip = crate::jar_defs::JarChip {
        name: crate::jar_defs::JarLabel {
            name: "A rather long jar name".into(),
            marker: Some(crate::jar_defs::JarMarker::Fragment("3f2a".into())),
        },
        color: egui::Color32::RED,
        following: false,
        missing: false,
        will_switch: false,
        can_switch_now: false,
        was_color: egui::Color32::GRAY,
        view_ignores: false,
        tooltip: None,
    };
    for (max, full) in [(300.0, true), (90.0, false), (20.0, false)] {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::FontId::proportional(11.0);
            let painted = paint_jar_chip(
                ui.painter(),
                egui::Pos2::ZERO,
                max,
                &chip,
                font,
                egui::Color32::WHITE,
                egui::Color32::YELLOW,
            );
            assert!(painted.is_some_and(|r| r.width() <= max), "{max}: {painted:?}");
        });
        output.textures_delta.clear();
        let galleys: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.clone()),
                _ => None,
            })
            .collect();
        let texts: Vec<String> = galleys.iter().map(|g| g.text().to_string()).collect();
        if max < 30.0 {
            assert!(texts.is_empty(), "{max}: just the dot: {texts:?}");
            continue;
        }
        assert_eq!(texts.len(), 2, "{max}: name and marker: {texts:?}");
        assert_eq!((texts[0].as_str(), texts[1].as_str()), ("A rather long jar name", "#3f2a"), "{max}");
        assert!(!galleys[1].elided, "{max}: the marker is whole");
        assert_eq!(galleys[0].elided, !full, "{max}: the name is ellipsized only when narrow");
    }
}

/// Fix batch B4: the dots the card chip and Tab view share show a waiting switch as the old jar's dot, an arrow,
/// then the new jar's; one dot otherwise. Tab view drew only the new jar's dot.
#[test]
fn the_jar_dots_show_a_waiting_switch() {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    for will_switch in [false, true] {
        let chip = crate::jar_defs::JarChip {
            name: "Home".into(),
            color: egui::Color32::RED,
            following: false,
            missing: false,
            will_switch,
            can_switch_now: true,
            was_color: egui::Color32::GRAY,
            view_ignores: false,
            tooltip: None,
        };
        let mut end = 0.0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let arrow = jar_chip_arrow(ui.painter(), &chip, egui::FontId::proportional(11.0), egui::Color32::WHITE);
            assert_eq!(arrow.is_some(), will_switch);
            let width = jar_dots_width(arrow.as_deref());
            end = paint_jar_dots(ui.painter(), 0.0, 8.0, &chip, arrow, egui::Color32::WHITE, egui::Color32::YELLOW);
            assert_eq!(end, width, "jar_dots_width matches what is painted");
        });
        output.textures_delta.clear();
        let circles: Vec<egui::Color32> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Circle(c) => Some(c.fill),
                _ => None,
            })
            .collect();
        let texts = output.shapes.iter().filter(|s| matches!(s.shape, egui::Shape::Text(_))).count();
        if will_switch {
            assert_eq!((circles, texts), (vec![egui::Color32::GRAY, egui::Color32::RED], 1));
        } else {
            assert_eq!((circles, texts), (vec![egui::Color32::RED], 0));
        }
    }
}

/// A painted chip's rect is remembered per card: a point on it hits, one off it or on another card does not, and a
/// chip that stops being painted stops being hit.
#[test]
fn a_painted_jar_chip_can_be_hit_until_it_stops_being_painted() {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    let chip = crate::jar_defs::JarChip {
        name: crate::jar_defs::JarLabel { name: "Work".into(), marker: None },
        color: egui::Color32::RED,
        following: false,
        missing: false,
        will_switch: false,
        can_switch_now: false,
        was_color: egui::Color32::GRAY,
        view_ignores: false,
        tooltip: None,
    };
    let frame = |paint: bool| {
        let mut overlays: crate::graph_ui::DeferredOverlays = Vec::new();
        if paint {
            push_jar_chip(
                &mut overlays,
                "card",
                egui::pos2(10.0, 10.0),
                egui::Vec2::ZERO,
                300.0,
                chip.clone(),
                egui::Color32::YELLOW,
            );
        }
        let mut overlays = Some(overlays);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for overlay in overlays.take().unwrap_or_default() {
                overlay(ui.painter(), &|p| p);
            }
        });
        output.textures_delta.clear();
    };
    frame(true);
    let on = egui::pos2(10.0 + JAR_CHIP_PAD + JAR_CHIP_R, 10.0 + JAR_CHIP_HEIGHT / 2.0);
    assert!(on_jar_chip(&ctx, "card", on));
    assert!(!on_jar_chip(&ctx, "card", egui::pos2(10.0, 10.0 + JAR_CHIP_HEIGHT + 20.0)), "below it");
    assert!(!on_jar_chip(&ctx, "other", on), "another card's chip");
    frame(false);
    frame(false);
    assert!(!on_jar_chip(&ctx, "card", on), "no longer painted");
}

/// The painted scrim honours `fit_jar_chip`, with the long name ellipsized rather than overflowing.
#[test]
fn a_painted_jar_chip_stays_inside_its_width() {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    let chip = crate::jar_defs::JarChip {
        name: "A rather long jar name".into(),
        color: egui::Color32::RED,
        following: false,
        missing: false,
        will_switch: true,
        can_switch_now: true,
        was_color: egui::Color32::GRAY,
        view_ignores: false,
        tooltip: None,
    };
    for max in [200.0, 80.0, 40.0, 20.0, 10.0] {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let font = egui::FontId::proportional(11.0);
            let painted = paint_jar_chip(
                ui.painter(),
                egui::Pos2::ZERO,
                max,
                &chip,
                font,
                egui::Color32::WHITE,
                egui::Color32::YELLOW,
            );
            assert!(painted.is_none_or(|r| r.width() <= max), "{max}: {painted:?}");
            assert_eq!(painted.is_none(), max < 20.0, "{max}");
        });
        output.textures_delta.clear();
    }
}

/// Name rules: a chip draws a hidden character in its jar's name as a placeholder, in the marker style.
#[test]
fn a_jar_chip_shows_a_hidden_character_as_a_placeholder() {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    let chip = crate::jar_defs::JarChip {
        name: crate::jar_defs::JarLabel { name: "Wo\u{200B}rk".into(), marker: None },
        color: egui::Color32::RED,
        following: false,
        missing: false,
        will_switch: false,
        can_switch_now: false,
        was_color: egui::Color32::GRAY,
        view_ignores: false,
        tooltip: None,
    };
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let font = egui::FontId::proportional(11.0);
        paint_jar_chip(ui.painter(), egui::Pos2::ZERO, 300.0, &chip, font, egui::Color32::WHITE, egui::Color32::YELLOW);
    });
    output.textures_delta.clear();
    let galleys: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some(t.galley.clone()),
            _ => None,
        })
        .collect();
    let name = galleys.iter().find(|g| g.text() == "Wo⟦200B⟧rk");
    let name = name.unwrap_or_else(|| panic!("{:?}", galleys.iter().map(|g| g.text()).collect::<Vec<_>>()));
    let hidden = name.job.sections.iter().find(|s| s.format.font_id.family == egui::FontFamily::Monospace);
    assert_eq!(hidden.expect("a monospace placeholder").format.font_id.size, 11.0 * crate::jar_defs::MARKER_SCALE);
}

/// Name rules: a group card's header draws its stored name through the display rule (placeholders, collapsed
/// spaces, the fallback); another card's title is drawn as before.
#[test]
fn a_group_header_draws_its_name_by_the_display_rule() {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    let mut app = App::default();
    for (name, user_name, want) in [
        ("Re\u{200B}ading", true, "Re⟦200B⟧ading"),
        ("  Two\u{00A0}\u{00A0}spaces ", true, "Two spaces"),
        ("", true, "Untitled"),
        ("Re\u{200B}ading", false, "Re\u{200B}ading"),
    ] {
        let segs = crate::names::display_name(name, crate::names::UNTITLED);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut overlays: DeferredOverlays = Vec::new();
            let header = CardHeader {
                glyph: Some(egui_phosphor::light::FOLDER),
                glyph_color: None,
                icon_image: None,
                name: if user_name { String::new() } else { name.into() },
                user_name: user_name.then_some(&segs[..]),
                source: "3 items".into(),
                source_is_path: false,
                scrim: false,
                right_margin: 0.0,
            };
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
            app.paint_card_header(ui, &mut overlays, "n", rect, 1.0, 1.0, header);
            for overlay in overlays {
                overlay(ui.painter(), &|p| p);
            }
        });
        output.textures_delta.clear();
        let galleys: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.clone()),
                _ => None,
            })
            .collect();
        let title = galleys.iter().find(|g| g.text() == want);
        let title =
            title.unwrap_or_else(|| panic!("{want}: {:?}", galleys.iter().map(|g| g.text()).collect::<Vec<_>>()));
        let mono = title.job.sections.iter().any(|s| s.format.font_id.family == egui::FontFamily::Monospace);
        assert_eq!(mono, want.contains('⟦'), "{want}");
    }
}

/// A header with no glyph (a live tab's: the card draws its own favicon or engine dot) draws only its title and
/// source line, left-aligned at `HEADER_TEXT_X` and centred on `HEADER_TITLE_Y` and `HEADER_SOURCE_Y`, where the
/// tab card drew them by hand before it went through `CardHeader`.
#[test]
fn a_header_without_a_glyph_draws_title_and_source_at_the_header_lines() {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    let mut app = App::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let mut overlays: DeferredOverlays = Vec::new();
        let header = CardHeader {
            glyph: None,
            glyph_color: None,
            icon_image: None,
            name: "Title".into(),
            user_name: None,
            source: "example.com".into(),
            source_is_path: false,
            scrim: false,
            right_margin: 0.0,
        };
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
        assert!(app.paint_card_header(ui, &mut overlays, "n", rect, 1.0, 1.0, header));
        for overlay in overlays {
            overlay(ui.painter(), &|p| p);
        }
    });
    output.textures_delta.clear();
    let texts: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()))),
            _ => None,
        })
        .collect();
    let names: Vec<_> = texts.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(names, ["Title", "example.com"]);
    for ((_, at), y) in texts.iter().zip([HEADER_TITLE_Y, HEADER_SOURCE_Y]) {
        assert!((at.left() - HEADER_TEXT_X).abs() <= 0.5, "{at:?}");
        assert!((at.center().y - y).abs() <= 0.5, "{at:?} vs {y}");
    }
}

/// Name rules: a group card's hover names the hidden characters in its stored name; a clean name has none. Its
/// header draws the same segments the hover came from.
#[test]
fn a_group_card_tooltip_names_a_hidden_character() {
    let ctx = egui::Context::default();
    crate::theme::apply_fonts(&ctx);
    let mut app = App::default();
    let g = app.alloc_node_id();
    app.insert_group_at(g.clone(), None);
    for (name, want, title) in [
        ("Re\u{200B}ading", Some("Hidden characters: U+200B zero width space"), "Re⟦200B⟧ading"),
        ("Reading", None, "Reading"),
    ] {
        app.mirror.set(&g, crate::store::model::field::NAME, crate::store::model::FieldValue::Str(name.into()));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut overlays: DeferredOverlays = Vec::new();
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 200.0));
            let view = test_view(plain(TYPE_GROUP));
            let mut actions = Vec::new();
            let drop_preview = None;
            let members = HashMap::new();
            let mut pass = CardPass {
                deferred_overlays: &mut overlays,
                actions: &mut actions,
                inv_zoom: 1.0,
                zoom_scale: 1.0,
                drop_preview: &drop_preview,
                pick_targets: &[],
                connect_target: None,
                connect_ring: None,
                show_order: false,
                order: &[],
                any_focused: false,
                members: &members,
                viewport: egui::Rect::EVERYTHING,
                pan_target: None,
            };
            let body = app.paint_group_body(ui, &mut pass, &g, &view, rect);
            assert_eq!(body.tooltip.as_deref(), want, "{name:?}");
            for overlay in overlays {
                overlay(ui.painter(), &|p| p);
            }
        });
        output.textures_delta.clear();
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == title), "{title}: {texts:?}");
    }
}

/// Header F is on every card kind: each kind's body painter reaches the shared header (or, for
/// the tab card and the JSON preview, the code that draws the same 40-point one). Source-text
/// check, like `profile::tests`, so a new kind or a rewrite that drops its header fails here
/// rather than only in a screenshot.
#[test]
fn every_card_body_draws_a_header() {
    let src = include_str!("../card.rs");
    let production = &src[..src.find("mod tests;").unwrap()];
    let markers = ["paint_card_header(", "paint_missing_file_card(", "tab_has_header("];
    for name in ["unloaded_tab", "download", "note", "image", "pdf", "json", "group", "tab", "jar", "unknown"] {
        let start = production.find(&format!("fn paint_{name}_body(")).unwrap_or_else(|| panic!("no {name} body"));
        let rest = &production[start + 1..];
        let body = &rest[..rest
            .find(
                "
    pub(crate) fn ",
            )
            .unwrap_or(rest.len())];
        // The JSON preview's header is drawn by `json_level_ui::paint_json_content`, which
        // `CardLook::preview` switches to F.
        let json_preview = name == "json" && body.contains("TAB_HEADER_POINTS");
        assert!(json_preview || markers.iter().any(|m| body.contains(m)), "paint_{name}_body draws no card header");
    }
}

/// A `CardView` for `kind` with nothing set, for tests to override.
fn test_view(kind: CardKind) -> CardView {
    CardView {
        kind,
        color: None,
        in_group: false,
        overloaded: false,
        members: Vec::new(),
        focused: false,
        pinned: false,
        muted: false,
        audio_playing: false,
        selected: false,
        closable: true,
    }
}

#[test]
fn card_border_resolves_focus_tag_overload_idle_and_hover() {
    // Opaque colors, so `lerp_color32` lands exactly on either end.
    let style = CardStyle {
        border_idle: egui::Color32::GRAY,
        border_hover: egui::Color32::WHITE,
        ..card_style(plain(TYPE_NOTE), &egui::Visuals::dark())
    };
    let red = egui::Color32::RED;
    let view = |color| CardView { color, ..test_view(plain(TYPE_NOTE)) };
    // At rest: the tag color, else the kind's idle color.
    assert_eq!(card_border(&view(None), &style, 0.0), (1.0, egui::Color32::GRAY));
    assert_eq!(card_border(&view(Some(red)), &style, 0.0), (1.0, red));
    // An overloaded group rests on DANGER_COLOR, unless tagged.
    let overloaded = |color| CardView { color, overloaded: true, ..test_view(plain(TYPE_GROUP)) };
    assert_eq!(card_border(&overloaded(None), &style, 0.0), (1.0, DANGER_COLOR));
    assert_eq!(card_border(&overloaded(Some(red)), &style, 0.0), (1.0, red));
    // Fully hovered: the hover color at 1.5, whatever the resting color.
    assert_eq!(card_border(&view(Some(red)), &style, 1.0), (1.5, egui::Color32::WHITE));
    // The open tab: ACCENT at 2.5 over everything, not eased.
    let focused = CardView { color: Some(red), focused: true, ..test_view(loaded_tab()) };
    assert_eq!(card_border(&focused, &style, 0.0), (2.5, ACCENT));
    assert_eq!(card_border(&focused, &style, 1.0), (2.5, ACCENT));
}

#[test]
fn file_header_text_prefers_the_url_over_the_path() {
    // A path in this platform's own form (a backslash is not a separator off Windows).
    let path = if cfg!(windows) { r"C:\pics\a.jpg" } else { "/pics/a.jpg" };
    assert_eq!(file_header_text(path, None), ("a.jpg".to_owned(), path.to_owned(), true));
    let url = "https://x.test/a.jpg";
    assert_eq!(file_header_text(path, Some(url)), ("a.jpg".to_owned(), url.to_owned(), false));
}

#[test]
fn cover_uv_crops_the_longer_axis_and_never_stretches() {
    let uv = |src, w, h| cover_uv(src, egui::vec2(w, h));
    // Same shape: the whole texture.
    assert_eq!(uv((256, 192), 128.0, 96.0), FULL_UV);
    // Wider source: sides cropped evenly.
    let wide = uv((200, 100), 100.0, 100.0);
    assert_eq!((wide.min.x, wide.max.x, wide.min.y, wide.max.y), (0.25, 0.75, 0.0, 1.0));
    // Taller source: bottom cropped, top kept.
    let tall = uv((100, 200), 100.0, 100.0);
    assert_eq!((tall.min.y, tall.max.y, tall.min.x, tall.max.x), (0.0, 0.5, 0.0, 1.0));
    // A degenerate size falls back to the whole texture.
    assert_eq!(uv((0, 0), 10.0, 10.0).max, egui::pos2(1.0, 1.0));
}

#[test]
fn tab_thumb_rect_leaves_room_for_the_header_only_on_tall_cards() {
    let card = egui::Rect::from_center_size(egui::Pos2::ZERO, GRAPH_NODE_SIZE);
    // Zoom 1: 110 points tall, so the header (40 points) takes the top.
    let thumb = tab_thumb_rect(card, 1.0);
    assert_eq!(thumb.top(), card.top() + TAB_HEADER_POINTS);
    assert_eq!(thumb.bottom(), card.bottom() - crate::GRAPH_THUMB_MARGIN);
    // Zoom 0.5: 55 points tall, so the plain inset thumbnail and the strip.
    assert!(!tab_has_header(card, 2.0));
    assert_eq!(tab_thumb_rect(card, 2.0), card.shrink(crate::GRAPH_THUMB_MARGIN));
}

#[test]
fn card_controls_list_each_cards_buttons_in_pointer_order() {
    use CardControl::*;
    for kind in kinds().into_iter().filter(|k| k.ty != TYPE_TAB) {
        assert_eq!(card_controls(&test_view(kind)), vec![Swatch, Pin, Close], "{kind:?}");
        let grouped = CardView { in_group: true, ..test_view(kind) };
        assert_eq!(card_controls(&grouped), vec![Swatch, Pin, Eject, Close], "{kind:?}");
    }
    // Close is the one close on every kind, a group included (Dissolve is a menu row, not the card's close).
    assert_eq!(Close.action(), NodeAction::Delete);
    // Every card shows its pin; an unloaded tab never shows the audio
    // indicator -- it has no engine to be playing or muted.
    assert_eq!(card_controls(&test_view(sleeping_tab())), vec![Swatch, Pin, Close]);
    assert_eq!(
        card_controls(&CardView { audio_playing: true, muted: true, ..test_view(sleeping_tab()) }),
        vec![Swatch, Pin, Close]
    );
    // A tab always shows its pin; its audio indicator while playing or
    // muted. Close still registers last: it overlaps the indicator.
    let tab = |f: fn(&mut CardView)| {
        let mut view = test_view(loaded_tab());
        f(&mut view);
        card_controls(&view)
    };
    assert_eq!(tab(|_| {}), vec![Swatch, Pin, Close]);
    assert_eq!(tab(|v| v.pinned = true), vec![Swatch, Pin, Close]);
    assert_eq!(tab(|v| v.audio_playing = true), vec![Swatch, Pin, Audio, Close]);
    assert_eq!(tab(|v| v.muted = true), vec![Swatch, Pin, Audio, Close]);
    assert_eq!(
        tab(|v| {
            v.muted = true;
            v.in_group = true;
        }),
        vec![Swatch, Pin, Audio, Eject, Close]
    );
    assert_eq!(Pin.action(), NodeAction::TogglePin);
    assert_eq!(Audio.action(), NodeAction::ToggleMute);
    assert!(!Pin.hover_only() && !Audio.hover_only());
    assert_eq!(Eject.action(), NodeAction::RemoveFromGroup);
    assert_eq!(Swatch.action(), NodeAction::CycleColor);
    assert!(Close.hover_only() && Eject.hover_only() && !Swatch.hover_only());
}

/// Pins each `CardControl`'s corner/offset anchor and size, at
/// inv_zoom 1.0 and 2.0, as a regression check on `CardControl::rect`.
#[test]
fn card_control_rects_sit_on_their_corners() {
    use CardControl::*;
    let card = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 80.0));
    for inv_zoom in [1.0_f32, 2.0] {
        assert_eq!(
            Swatch.rect(card, inv_zoom),
            egui::Rect::from_center_size(card.right_bottom(), egui::Vec2::splat(16.0) * inv_zoom)
        );
        assert_eq!(
            Pin.rect(card, inv_zoom),
            egui::Rect::from_center_size(card.left_bottom(), egui::Vec2::splat(16.0) * inv_zoom)
        );
        assert_eq!(
            Audio.rect(card, inv_zoom),
            egui::Rect::from_center_size(
                card.right_top() + egui::vec2(-8.0, 8.0) * inv_zoom,
                egui::Vec2::splat(16.0) * inv_zoom
            )
        );
        assert_eq!(
            Eject.rect(card, inv_zoom),
            egui::Rect::from_center_size(card.left_top(), egui::Vec2::splat(18.0) * inv_zoom)
        );
        assert_eq!(
            Close.rect(card, inv_zoom),
            egui::Rect::from_center_size(card.right_top(), egui::Vec2::splat(18.0) * inv_zoom)
        );
    }
}

/// A PDF card's click reveals its file, as a download's does; the other kinds keep theirs.
#[test]
fn click_action_gives_a_pdf_the_download_click() {
    assert_eq!(click_action(plain(TYPE_PDF)), Some(NodeAction::Reveal));
    assert_eq!(click_action(plain(TYPE_PDF)), click_action(plain(TYPE_DOWNLOAD)));
    for kind in [loaded_tab(), sleeping_tab(), plain(TYPE_IMAGE)] {
        assert_eq!(click_action(kind), Some(NodeAction::Open), "{kind:?}");
    }
    for kind in [plain(TYPE_NOTE), plain(TYPE_GROUP), plain(TYPE_JSON)] {
        assert_eq!(click_action(kind), None, "{kind:?}: handled by the caller");
    }
}

#[test]
fn drop_action_matches_the_drop_table() {
    let card = |kind: CardKind| DropTarget::Card(kind);
    let (tab, sleeping, group) = (loaded_tab(), sleeping_tab(), plain(TYPE_GROUP));
    for kind in kinds() {
        // The back button only exists inside a group.
        assert_eq!(drop_action(kind, DropTarget::BackButton, true), Some(DropEffect::Eject), "{kind:?}");
        assert_eq!(drop_action(kind, DropTarget::BackButton, false), None, "{kind:?}");
        let onto_group = if kind.ty == TYPE_GROUP { DropEffect::Merge } else { DropEffect::Join };
        for inside_group in [false, true] {
            assert_eq!(drop_action(kind, card(group), inside_group), Some(onto_group), "{kind:?}");
        }
    }
    // An unloaded card is a tab on both sides of the drop.
    for (dragged, target) in [(tab, tab), (tab, sleeping), (sleeping, tab), (sleeping, sleeping)] {
        assert_eq!(drop_action(dragged, card(target), false), Some(DropEffect::GroupWith), "{dragged:?} -> {target:?}");
        assert_eq!(drop_action(dragged, card(target), true), Some(DropEffect::Reparent), "{dragged:?} -> {target:?}");
    }
    // Only a tab drops onto a tab, and nothing drops onto the file kinds.
    assert_eq!(drop_action(plain(TYPE_NOTE), card(tab), false), None);
    assert_eq!(drop_action(group, card(tab), true), None);
    assert_eq!(drop_action(tab, card(plain(TYPE_NOTE)), false), None);
    assert_eq!(drop_action(plain(TYPE_IMAGE), card(plain(TYPE_JSON)), true), None);
}

#[test]
fn node_card_size_matches_each_kinds_card() {
    let download = Runtime::Download(DownloadPayload {
        path: "f.bin".into(),
        bytes_received: 0,
        total_bytes: None,
        completed: false,
        failed: false,
    });
    assert_eq!(node_card_size(&download), DOWNLOAD_NODE_SIZE);
    assert_eq!(node_card_size(&Runtime::Note(NotePayload { text: String::new(), editing: false })), NOTE_NODE_SIZE);
    assert_eq!(node_card_size(&Runtime::None), GROUP_NODE_SIZE, "a group has no runtime");
    assert_eq!(node_card_size(&Runtime::Unknown), UNKNOWN_NODE_SIZE);
    let json = |value: serde_json::Value, error: Option<&str>| {
        Runtime::Json(JsonPayload { path: "a.json".into(), value: value.into(), error: error.map(Into::into) })
    };
    let five = serde_json::json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5});
    assert_eq!(node_card_size(&json(five.clone(), None)), crate::json_view::preview_size(&five));
    assert_eq!(node_card_size(&json(serde_json::json!({}), None)).x, crate::json_view::PREVIEW_CARD_WIDTH);
    // An error card keeps the small size.
    assert_eq!(node_card_size(&json(five, Some("missing"))), JSON_NODE_SIZE);
    assert_eq!(
        node_card_size(&Runtime::Unloaded(UnloadedTabPayload {
            url: "https://example.org/".into(),
            title: String::new(),
            muted: false,
            zoom_factor: 1.5,
            state: UnloadedState::Failed("no binary".into()),
        })),
        GRAPH_NODE_SIZE
    );

    // Image: a small picture keeps its own size; an oversized one is
    // scaled down to fit `IMAGE_NODE_MAX_SIZE`, aspect ratio kept.
    let image = |width, height| Runtime::Image(ImagePayload { path: "p.png".into(), width, height, error: None });
    assert_eq!(node_card_size(&image(40, 20)), egui::vec2(40.0, 20.0));
    let big = node_card_size(&image(100_000, 50_000));
    assert!(big.x <= IMAGE_NODE_MAX_SIZE.x && big.y <= IMAGE_NODE_MAX_SIZE.y);
    assert!((big.x / big.y - 2.0).abs() < 1e-3);

    // Pdf: pending and failed use the fixed pending box; rendered uses
    // the page's own size through the same image fitting.
    let pdf = |outcome| Runtime::Pdf(PdfPayload { path: "d.pdf".into(), outcome });
    assert_eq!(node_card_size(&pdf(None)), PDF_PENDING_NODE_SIZE);
    assert_eq!(node_card_size(&pdf(Some(Err("bad".into())))), PDF_PENDING_NODE_SIZE);
    assert_eq!(
        node_card_size(&pdf(Some(Ok(PdfOutcome { width: 40, height: 20, page_count: 3 })))),
        egui::vec2(40.0, 20.0)
    );
    assert_eq!(node_card_size(&pdf(Some(Ok(PdfOutcome { width: 100_000, height: 50_000, page_count: 1 })))), big);
}

#[test]
fn card_filter_text_matches_each_kinds_filter_input() {
    // A note or a group matches on its own text (`App::own_text`), passed in.
    fn filter(runtime: &Runtime) -> Vec<&str> {
        card_filter_text(runtime, "buy milk")
    }
    let download = Runtime::Download(DownloadPayload {
        path: "C:/dl/f.bin".into(),
        bytes_received: 0,
        total_bytes: None,
        completed: true,
        failed: false,
    });
    // Download filters on its full path; the file kinds on the filename only.
    assert_eq!(filter(&download), ["C:/dl/f.bin"]);
    assert_eq!(filter(&Runtime::Note(NotePayload { text: String::new(), editing: false })), ["buy milk"]);
    assert_eq!(
        filter(&Runtime::Image(ImagePayload { path: "C:/pics/cat.png".into(), width: 1, height: 1, error: None })),
        ["cat.png"]
    );
    assert_eq!(filter(&Runtime::Pdf(PdfPayload { path: "C:/docs/paper.pdf".into(), outcome: None })), ["paper.pdf"]);
    assert!(filter(&Runtime::Unknown).is_empty(), "matched by its stored name, which the caller pushes");
    assert_eq!(
        filter(&Runtime::Json(JsonPayload {
            path: "C:/data/x.json".into(),
            value: std::sync::Arc::new(serde_json::json!(1)),
            error: None,
        })),
        ["x.json"]
    );
    assert_eq!(filter(&Runtime::None), ["buy milk"], "a group (no runtime) matches on its name");
    // Like a live tab: title and url, so the Graph filter can match a
    // sleeping card by its saved title, not just its url.
    assert_eq!(
        filter(&Runtime::Unloaded(UnloadedTabPayload {
            url: "https://example.org/".into(),
            title: "Example".into(),
            muted: false,
            zoom_factor: 1.5,
            state: UnloadedState::Failed("no binary".into()),
        })),
        ["Example", "https://example.org/"]
    );
    assert_eq!(
        filter(&Runtime::Unloaded(UnloadedTabPayload {
            url: "https://example.org/".into(),
            title: String::new(),
            muted: false,
            zoom_factor: 1.5,
            state: UnloadedState::Failed("no binary".into()),
        })),
        ["", "https://example.org/"]
    );
}

/// A CEF runtime with an older marker (Update needed) is named as needing an update, not as missing.
#[test]
fn a_failed_card_names_an_outdated_view_as_needing_an_update() {
    use crate::view_install::ViewStatus;
    assert_eq!(missing_view_phrases(&ViewStatus::UpdateNeeded { size: 1 }), ("needs an update", "needs an update"));
    assert_eq!(missing_view_phrases(&ViewStatus::NotInstalled { size: 1 }), ("is not installed", "not installed"));
    assert_eq!(missing_view_phrases(&ViewStatus::Failed("x".into())), ("is not installed", "not installed"));
}

#[test]
fn a_waking_cards_tooltip_differs_from_a_sleeping_ones() {
    assert_eq!(sleeping_or_waking_tooltip(&UnloadedState::Sleeping), "Sleeping — click to load");
    assert_eq!(sleeping_or_waking_tooltip(&UnloadedState::Waking), "Loading…");
}

/// Review C-B3: leaving a level ends a note edit there.
#[test]
fn leaving_the_level_ends_a_note_edit() {
    let mut app = App::default();
    app.new_note_shortcut();
    let id = app.node_order[0].clone();
    app.nodes.get_mut(&id).unwrap().as_note_mut().unwrap().editing = true;
    app.switch_group_level(None);
    assert!(!app.nodes[&id].as_note().unwrap().editing);
}

/// Review C-B8: a group's members come in creation order, every time.
#[test]
fn group_members_follow_node_order() {
    let mut app = App::default();
    let g = app.alloc_node_id();
    app.insert_group_at(g.clone(), None);
    let mut made = Vec::new();
    for _ in 0..20 {
        app.new_note_shortcut();
        let id = app.node_order.last().unwrap().clone();
        app.edit_group(&id, Some(g.clone()));
        made.push(id);
    }
    assert_eq!(app.group_members()[&g], made);
    let entry = app.level_items().into_iter().find(|e| e.id == g).unwrap();
    assert_eq!(app.card_view(&entry, &app.group_members()).unwrap().members, made);
}

fn with_fonts(f: impl FnOnce(&egui::Context)) {
    let ctx = egui::Context::default();
    // `run_ui` takes an `FnMut`; the test body runs once.
    let mut f = Some(f);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        if let Some(f) = f.take() {
            f(ui.ctx());
        }
    });
    // The font atlas upload is never applied here; dropping it unapplied is a debug assertion.
    output.textures_delta.clear();
}

#[test]
fn a_cut_from_the_start_keeps_a_true_suffix_on_grapheme_boundaries() {
    use unicode_segmentation::UnicodeSegmentation;
    with_fonts(|ctx| {
        let mut cache = HashMap::new();
        let font = egui::FontId::proportional(14.0);
        let text = format!("{}e\u{301}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}tail.jpg", "dir\\".repeat(30));
        let cut = truncate_start_to_width(&mut cache, ctx, &text, &font, 80.0);
        let kept = cut.strip_prefix('\u{2026}').expect("cut at the start");
        assert!(text.ends_with(kept), "a true suffix, not a re-reversed one: {cut}");
        let start = text.len() - kept.len();
        assert!(text.grapheme_indices(true).any(|(i, _)| i == start), "starts on a grapheme boundary");
    });
}

#[test]
fn a_cut_from_the_end_never_splits_a_grapheme() {
    use unicode_segmentation::UnicodeSegmentation;
    with_fonts(|ctx| {
        let mut cache = HashMap::new();
        let font = egui::FontId::proportional(14.0);
        // A ZWJ family, and a base letter with a combining mark; every width, so some cut lands
        // inside a sequence if the search counts chars.
        for text in ["\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".repeat(40), "e\u{301}".repeat(60)] {
            for width in (8..=200).step_by(4) {
                let cut = truncate_to_width(&mut cache, ctx, &text, &font, width as f32);
                let kept = cut.strip_suffix('\u{2026}').expect("cut at the end");
                assert!(
                    text.grapheme_indices(true).any(|(i, _)| i == kept.len()) || kept.is_empty(),
                    "width {width}: {kept:?}"
                );
            }
        }
    });
}

#[test]
fn a_repeated_cut_is_one_cache_entry() {
    with_fonts(|ctx| {
        let mut cache = HashMap::new();
        let font = egui::FontId::proportional(14.0);
        let a = truncate_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
        let b = truncate_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
        assert_eq!((a, cache.len()), (b, 1));
        truncate_start_to_width(&mut cache, ctx, "a long title that will not fit", &font, 40.0);
        assert_eq!(cache.len(), 2, "start and end cuts are cached apart");
    });
}
