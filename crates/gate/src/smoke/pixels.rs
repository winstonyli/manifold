//! Smoke's checks of a screenshot's pixels.

use std::path::Path;

use super::{floats, rect_of};
use crate::state::{round_i, State};

/// Whether the card `id`'s border, sampled at the middle of each of its four edges, is within 40 (RGB distance) of
/// `want` at any of them (another card can cover one edge: the group's left edge was once hidden by a tab card).
/// Pixels off the image are skipped; a card or border with no pixel on the image fails.
pub(super) fn border_near(s: &State, png: &Path, id: &str, want: [u8; 3]) -> bool {
    let Some(r) = rect_of(s, id) else { return false };
    let Ok(img) = crate::shots_img::load(png) else { return false };
    let (mid_x, mid_y) = (round_i((r[0] + r[2]) / 2.0), round_i((r[1] + r[3]) / 2.0));
    let (left, top) = (round_i(r[0]), round_i(r[1]));
    let (right, bottom) = (round_i(r[2]), round_i(r[3]));
    let mut near = f64::INFINITY;
    for (cx, cy, step) in
        [(left, mid_y, (1, 0)), (right - 3, mid_y, (1, 0)), (mid_x, top, (0, 1)), (mid_x, bottom - 3, (0, 1))]
    {
        for k in -1..3 {
            let (x, y) = (cx + k * step.0, cy + k * step.1);
            let (Ok(px), Ok(py)) = (u32::try_from(x), u32::try_from(y)) else { continue };
            if px >= img.width() || py >= img.height() {
                continue;
            }
            let p = img.get_pixel(px, py).0;
            let d2: i64 = p.iter().zip(want).map(|(a, b)| (i64::from(*a) - i64::from(b)).pow(2)).sum();
            near = near.min((d2 as f64).sqrt());
        }
    }
    near < 40.0
}

/// How many pixels inside the popups showing in `s` are within 12 (RGB distance) of `want`.
pub(super) fn fill_pixels(s: &State, png: &Path, want: [u8; 3]) -> usize {
    let Ok(img) = crate::shots_img::load(png) else { return 0 };
    let rects: Vec<Vec<f64>> =
        s.0["popups"].as_array().map(|l| l.iter().filter_map(|p| floats(&p["rect"], 4)).collect()).unwrap_or_default();
    let mut n = 0;
    for r in rects {
        for y in r[1].max(0.0) as u32..(r[3].max(0.0) as u32).min(img.height()) {
            for x in r[0].max(0.0) as u32..(r[2].max(0.0) as u32).min(img.width()) {
                let p = img.get_pixel(x, y).0;
                let d2: i64 = p.iter().zip(want).map(|(a, b)| (i64::from(*a) - i64::from(b)).pow(2)).sum();
                n += usize::from(d2 < 144);
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn state(json: &str) -> State {
        State::parse(json).unwrap()
    }

    #[test]
    fn border_pixels_near_the_card_edge() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::create_dir_all(&dir).unwrap();
        let png = dir.join("b.png");
        let mut img = image::RgbImage::from_pixel(50, 50, image::Rgb([10, 10, 10]));
        img.put_pixel(20, 25, image::Rgb([200, 90, 60])); // 1 px stroke at the left edge, mid-height
        img.save(&png).unwrap();
        let s =
            state(r#"{"cards":[{"id":"a","rect":[21.0,10.0,40.0,40.0]},{"id":"far","rect":[45.0,10.0,60.0,40.0]}]}"#);
        // Pixels x 20..=23 at y 25 are checked: 20 is the stroke, within 40 of (196, 87, 58).
        assert!(border_near(&s, &png, "a", [196, 87, 58]));
        assert!(!border_near(&s, &png, "a", [215, 162, 74]));
        assert!(!border_near(&s, &png, "nope", [196, 87, 58]));
        // A card wholly past the image has no pixel to sample and fails; one partly past it is judged by the pixels
        // that are there.
        assert!(!border_near(
            &state(r#"{"cards":[{"id":"o","rect":[60.0,10.0,70.0,40.0]}]}"#),
            &png,
            "o",
            [10, 10, 10]
        ));
        assert!(border_near(&s, &png, "far", [10, 10, 10]));
    }
}
