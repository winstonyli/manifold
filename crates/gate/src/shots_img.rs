//! The image steps of `gate chrome-shots`: the contact sheet (was scripts/shots_sheet.py) and the hover report (was
//! scripts/hover_diff.py), both on the `image` crate instead of Pillow.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use image::imageops::{self, FilterType};
use image::RgbImage;

/// The screenshot or image at `path` as RGB, an error naming the path if it cannot be read.
pub(crate) fn load(path: &Path) -> io::Result<RgbImage> {
    Ok(image::open(path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?.to_rgb8())
}

/// Crops each view's PNG in `out` to its top `crop_px` rows into `out/crop/`, and tiles all of them, three to a row
/// at 640 px wide, into `out/sheet.png`. Returns the line to print.
pub fn write_sheet(out: &Path, crop_px: u32, views: &[String]) -> io::Result<String> {
    fs::create_dir_all(out.join("crop"))?;
    let mut images = Vec::new();
    for view in views {
        let path = out.join(format!("{view}.png"));
        if !path.exists() {
            continue;
        }
        let im = load(&path)?;
        imageops::crop_imm(&im, 0, 0, im.width(), crop_px.min(im.height()))
            .to_image()
            .save(out.join("crop").join(format!("{view}.png")))
            .map_err(io::Error::other)?;
        images.push(im);
    }
    let Some(first) = images.first() else {
        return Ok("no views shot".to_string());
    };
    let (cols, tile_w) = (3u32, 640u32);
    let tile_h = first.height() * tile_w / first.width();
    let rows = (images.len() as u32).div_ceil(cols);
    let mut sheet = RgbImage::from_pixel(cols * tile_w, rows * tile_h, image::Rgb([30, 30, 30]));
    for (i, im) in images.iter().enumerate() {
        let tile = imageops::resize(im, tile_w, tile_h, FilterType::Triangle);
        imageops::replace(&mut sheet, &tile, i64::from(i as u32 % cols * tile_w), i64::from(i as u32 / cols * tile_h));
    }
    sheet.save(out.join("sheet.png")).map_err(io::Error::other)?;
    Ok(format!("{} views: {}", images.len(), out.display()))
}

/// The lines (columns of a top band, or rows of a left strip) where `im` differs from `rest` by more than 12 in
/// luma. `band` is the band's height, or in `column` mode the strip's width starting at column `x0`.
fn changed_lines(im: &RgbImage, rest: &RgbImage, band: u32, column: bool, x0: u32) -> BTreeSet<u32> {
    let (w, h) = (im.width().min(rest.width()), im.height().min(rest.height()));
    let differs = |x: u32, y: u32| {
        let (a, b) = (im.get_pixel(x, y).0, rest.get_pixel(x, y).0);
        let d = |i: usize| u32::from(a[i].abs_diff(b[i]));
        (299 * d(0) + 587 * d(1) + 114 * d(2)) / 1000 > 12
    };
    if column {
        (0..h).filter(|&y| (x0..(x0 + band).min(w)).any(|x| differs(x, y))).collect()
    } else {
        (0..w).filter(|&x| (0..band.min(h)).any(|y| differs(x, y))).collect()
    }
}

/// hover_diff.py's report from the changed lines of each hover (`position -> lines`): lines that change in most
/// hovers are the app itself (a clock, a blinking cursor) and are dropped; per hover the span of changed lines and the
/// number of separate runs (a gap over 6 starts a new one). A hover should repaint one widget: one run.
fn report_lines(mut changed: BTreeMap<u32, BTreeSet<u32>>) -> Vec<String> {
    if changed.len() >= 5 {
        let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
        for lines in changed.values() {
            for &l in lines {
                *counts.entry(l).or_default() += 1;
            }
        }
        let limit = 0.6 * changed.len() as f64;
        let noise: BTreeSet<u32> = counts.into_iter().filter(|&(_, n)| n as f64 > limit).map(|(l, _)| l).collect();
        for lines in changed.values_mut() {
            lines.retain(|l| !noise.contains(l));
        }
    }
    let mut out = Vec::new();
    let mut bad = 0;
    for (pos, lines) in &changed {
        let lines: Vec<u32> = lines.iter().copied().collect();
        let (Some(&first), Some(&last)) = (lines.first(), lines.last()) else {
            out.push(format!("x={pos:4} no change"));
            continue;
        };
        let runs = 1 + lines.windows(2).filter(|p| p[1] - p[0] > 6).count();
        let flag = if runs > 1 { "  <-- several runs" } else { "" };
        bad += usize::from(runs > 1);
        out.push(format!("x={pos:4} changed {first}..{last} (w={}) runs={runs}{flag}", last - first + 1));
    }
    out.push(format!("{} hovers, {bad} with several runs", changed.len()));
    out
}

/// `dir` holds rest.png and x<NNNN>.png (or, in `column` mode, y<NNNN>.png) shot by the hover scans. Compares the top
/// `band` rows of each against rest (in `column` mode a strip `band` wide from column `x0`).
pub fn hover_report(dir: &Path, band: u32, column: bool, x0: u32) -> io::Result<Vec<String>> {
    let rest = load(&dir.join("rest.png"))?;
    let prefix = if column { 'y' } else { 'x' };
    let mut changed = BTreeMap::new();
    for entry in fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(n) =
            name.strip_prefix(prefix).and_then(|r| r.strip_suffix(".png")).and_then(|d| d.parse::<u32>().ok())
        else {
            continue;
        };
        changed.insert(n, changed_lines(&load(&entry.path())?, &rest, band, column, x0));
    }
    Ok(report_lines(changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[u32]) -> BTreeSet<u32> {
        v.iter().copied().collect()
    }

    #[test]
    fn runs_spans_and_flags() {
        let report = report_lines(BTreeMap::from([(4, set(&[10, 11, 12])), (18, set(&[10, 30])), (32, set(&[]))]));
        assert_eq!(report[0], "x=   4 changed 10..12 (w=3) runs=1");
        assert_eq!(report[1], "x=  18 changed 10..30 (w=21) runs=2  <-- several runs");
        assert_eq!(report[2], "x=  32 no change");
        assert_eq!(report[3], "3 hovers, 1 with several runs");
    }

    #[test]
    fn lines_changing_in_most_hovers_are_noise() {
        // Line 0 changes in all five hovers (the app's own blink); line 40+i is each hover's own.
        let changed = (0..5).map(|i| (i, set(&[0, 40 + i * 20]))).collect();
        let report = report_lines(changed);
        assert!(report[..5].iter().all(|l| l.ends_with("runs=1")), "{report:?}");
        assert!(report[0].contains("changed 40..40"), "{report:?}");
    }

    #[test]
    fn a_changed_pixel_marks_its_column_or_row() {
        let rest = RgbImage::from_pixel(20, 20, image::Rgb([0, 0, 0]));
        let mut im = rest.clone();
        im.put_pixel(7, 3, image::Rgb([255, 255, 255]));
        im.put_pixel(15, 18, image::Rgb([255, 255, 255]));
        assert_eq!(changed_lines(&im, &rest, 10, false, 0), set(&[7]));
        assert_eq!(changed_lines(&im, &rest, 10, true, 0), set(&[3]));
        assert_eq!(changed_lines(&im, &rest, 10, true, 10), set(&[18]));
    }
}
