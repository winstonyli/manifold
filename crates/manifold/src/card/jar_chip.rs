//! The jar chip on a tab card: its fit, painting, hit rect and the dots it shows.

use super::*;

/// A jar chip's height in screen points.
pub(super) const JAR_CHIP_HEIGHT: f32 = 16.0;

/// A jar chip's dot radius, and its padding: before the first dot, after the name (or the last dot, with no name).
pub(super) const JAR_CHIP_R: f32 = 4.0;

pub(super) const JAR_CHIP_PAD: f32 = 5.0;

pub(super) const JAR_CHIP_NAME_END: f32 = 6.0;

/// The gap between the jar's dot and its name.
pub(super) const JAR_CHIP_NAME_GAP: f32 = 4.0;

/// The chip's black scrim, so its light name reads on a white page. The frame blends in linear light, which makes a
/// scrim look far thinner than its alpha suggests: measured on a white thumbnail (2026-10-03), alpha 210 came out
/// sRGB grey 123 (about 3.7:1 against the name), so it is 240 (about grey 69, 8:1).
const JAR_CHIP_SCRIM_ALPHA: u8 = 240;

/// Narrower than this, a chip's name (about one letter and "…") is left out rather than squeezed.
pub(super) const JAR_CHIP_MIN_NAME: f32 = 14.0;

/// The gap between a jar's name and its marker (`JarLabel::marker_text`).
pub(super) const JAR_CHIP_MARKER_GAP: f32 = 4.0;

/// What of a jar chip fits `max` screen points (`fit_jar_chip`): whether the old jar's dot and arrow are drawn
/// (`lead`), the width the name gets (`None`: no name), and whether the marker is drawn (always whole, with the name).
#[derive(Debug, PartialEq)]
pub(super) struct ChipFit {
    pub(super) lead: bool,
    pub(super) name: Option<f32>,
    pub(super) marker: bool,
}

/// Fits a chip whose lead is `lead`, name `name` and marker `marker` points wide (0: no marker) into `max`: whole if it
/// fits, else the name ellipsized (never the marker), else just the dots, else just the jar's dot (a waiting switch's
/// cue goes last but one), else nothing.
pub(super) fn fit_jar_chip(max: f32, lead: f32, name: f32, marker: f32) -> Option<ChipFit> {
    let dots = |lead: f32| JAR_CHIP_PAD + lead + 2.0 * JAR_CHIP_R;
    let tail = if marker > 0.0 { JAR_CHIP_MARKER_GAP + marker } else { 0.0 };
    let room = max - dots(lead) - JAR_CHIP_NAME_GAP - tail - JAR_CHIP_NAME_END;
    if room >= name.min(JAR_CHIP_MIN_NAME) {
        Some(ChipFit { lead: lead > 0.0, name: Some(name.min(room)), marker: marker > 0.0 })
    } else if dots(lead) + JAR_CHIP_PAD <= max {
        Some(ChipFit { lead: lead > 0.0, name: None, marker: false })
    } else if dots(0.0) + JAR_CHIP_PAD <= max {
        Some(ChipFit { lead: false, name: None, marker: false })
    } else {
        None
    }
}

/// A jar chip with its top-left at `at` (screen), at most `max_width` wide (`fit_jar_chip`): a dot (outlined when
/// following the group, `warn`-coloured for a missing jar), the name and its marker on a dark scrim, after the old
/// jar's dot and an arrow while the tab waits to switch. The marker is styled as everywhere else
/// (`JarLabel::marker_format`), in `text` dimmed by the theme's weak text alpha: the scrim is always near-black, so
/// the panel's own weak colour would not suit it. Its hover text goes through `CardBody::tooltip`. Returns the scrim
/// drawn.
pub(crate) fn paint_jar_chip(
    painter: &egui::Painter,
    at: egui::Pos2,
    max_width: f32,
    chip: &crate::jar_defs::JarChip,
    font: egui::FontId,
    text: egui::Color32,
    warn: egui::Color32,
) -> Option<egui::Rect> {
    const R: f32 = JAR_CHIP_R;
    let cy = at.y + JAR_CHIP_HEIGHT / 2.0;
    let arrow = jar_chip_arrow(painter, chip, font.clone(), text);
    let weak = text.gamma_multiply(painter.ctx().global_style().visuals.weak_text_alpha);
    let (segs, name_format) = (chip.name.display(), chip.name.name_format(font.clone()));
    let name_job = |wrap: Option<f32>| {
        let mut job = egui::text::LayoutJob::default();
        crate::names::append_name(&mut job, &segs, &name_format, weak);
        if let Some(room) = wrap {
            job.wrap = egui::text::TextWrapping {
                max_width: room,
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
        }
        painter.layout_job(job)
    };
    let name = name_job(None);
    let marker = chip.name.marker_text().map(|m| {
        painter.layout_job(egui::text::LayoutJob::single_section(
            m,
            crate::jar_defs::JarLabel::marker_format(font.size, weak),
        ))
    });
    let lead = jar_dots_width(arrow.as_deref()) - 2.0 * R;
    let fit = fit_jar_chip(max_width, lead, name.size().x, marker.as_ref().map_or(0.0, |m| m.size().x))?;
    // name rule: a fitted width, not text
    let name = fit.name.map(|room| if room >= name.size().x { name } else { name_job(Some(room)) });
    let marker = marker.filter(|_| fit.marker);
    let lead = if fit.lead { lead } else { 0.0 };
    let tail = marker.as_ref().map_or(0.0, |m| JAR_CHIP_MARKER_GAP + m.size().x);
    let width = match &name {
        Some(name) => JAR_CHIP_PAD + lead + 2.0 * R + JAR_CHIP_NAME_GAP + name.size().x + tail + JAR_CHIP_NAME_END,
        None => JAR_CHIP_PAD + lead + 2.0 * R + JAR_CHIP_PAD,
    };
    let scrim = egui::Rect::from_min_size(at, egui::vec2(width, JAR_CHIP_HEIGHT));
    painter.rect_filled(scrim, JAR_CHIP_HEIGHT / 2.0, egui::Color32::from_black_alpha(JAR_CHIP_SCRIM_ALPHA));
    let x = paint_jar_dots(painter, at.x + JAR_CHIP_PAD, cy, chip, arrow.filter(|_| fit.lead), text, warn);
    if let Some(name) = name {
        let x = x + JAR_CHIP_NAME_GAP;
        let size = name.size();
        painter.galley(egui::pos2(x, cy - size.y / 2.0), name, text);
        if let Some(marker) = marker {
            let m = marker.size();
            painter.galley(egui::pos2(x + size.x + JAR_CHIP_MARKER_GAP, cy - m.y / 2.0), marker, text);
        }
    }
    Some(scrim)
}

/// The arrow between a waiting switch's two dots (`paint_jar_dots`); `None` unless the tab waits to switch.
pub(crate) fn jar_chip_arrow(
    painter: &egui::Painter,
    chip: &crate::jar_defs::JarChip,
    font: egui::FontId,
    text: egui::Color32,
) -> Option<std::sync::Arc<egui::Galley>> {
    chip.will_switch.then(|| painter.layout_no_wrap(egui_phosphor::light::ARROW_RIGHT.into(), font, text))
}

/// How wide `paint_jar_dots` paints, with `arrow` (`jar_chip_arrow`) or without.
pub(crate) fn jar_dots_width(arrow: Option<&egui::Galley>) -> f32 {
    arrow.map_or(0.0, |a| 2.0 * JAR_CHIP_R + 3.0 + a.size().x + 3.0) + 2.0 * JAR_CHIP_R
}

/// A jar's dots from `x` (screen) on the line `cy`, shared by the card chip and Tab view: with `arrow`, the jar the
/// live tab still runs on (`was_color`) and the arrow first; then its jar's dot, outlined when following the group and
/// `warn`-coloured for a missing jar. Returns the x after the last dot.
pub(crate) fn paint_jar_dots(
    painter: &egui::Painter,
    mut x: f32,
    cy: f32,
    chip: &crate::jar_defs::JarChip,
    arrow: Option<std::sync::Arc<egui::Galley>>,
    text: egui::Color32,
    warn: egui::Color32,
) -> f32 {
    const R: f32 = JAR_CHIP_R;
    if let Some(arrow) = arrow {
        painter.circle_filled(egui::pos2(x + R, cy), R, chip.was_color);
        x += 2.0 * R + 3.0;
        let size = arrow.size();
        painter.galley(egui::pos2(x, cy - size.y / 2.0), arrow, text);
        x += size.x + 3.0;
    }
    let color = if chip.missing { warn } else { chip.color };
    if chip.following {
        painter.circle_stroke(egui::pos2(x + R, cy), R - 0.75, egui::Stroke::new(1.5, color));
    } else {
        painter.circle_filled(egui::pos2(x + R, cy), R, color);
    }
    x + 2.0 * R
}

/// Queues tab `chip` at world point `world` (the card's corner it hangs from), moved by `offset` screen points, at
/// most `max_width` screen points wide: the card's width less the chip's inset on both sides.
pub(super) fn push_jar_chip(
    deferred_overlays: &mut DeferredOverlays,
    id: &str,
    world: egui::Pos2,
    offset: egui::Vec2,
    max_width: f32,
    chip: crate::jar_defs::JarChip,
    warn: egui::Color32,
) {
    let key = jar_chip_key(id);
    deferred_overlays.push(Box::new(move |overlay_painter, project| {
        let font = egui::FontId::proportional(11.0);
        let text = crate::theme::SCRIM_TEXT;
        if let Some(scrim) =
            paint_jar_chip(overlay_painter, project(world) + offset, max_width, &chip, font, text, warn)
        {
            let ctx = overlay_painter.ctx();
            let frame = ctx.cumulative_frame_nr();
            ctx.data_mut(|d| d.insert_temp(key, (frame, scrim)));
        }
    }));
}

fn jar_chip_key(id: &str) -> egui::Id {
    egui::Id::new(("jar_chip_rect", id))
}

/// Whether screen point `at` is on card `id`'s jar chip, as last painted (this frame or the one before: a chip no
/// longer drawn is not hit).
pub(crate) fn on_jar_chip(ctx: &egui::Context, id: &str, at: egui::Pos2) -> bool {
    let now = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<(u64, egui::Rect)>(jar_chip_key(id)))
        .is_some_and(|(frame, scrim)| frame + 1 >= now && scrim.contains(at))
}
