//! Truncating a title or path to a width, cached by width bucket.

use super::*;

/// `text` cut to `max_width` screen points at the start ("…\\Trips\\a.jpg"), so a path's file name
/// stays visible. Shares `truncate_to_width`'s cache.
pub(crate) fn truncate_start_to_width(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
) -> String {
    truncate_core(cache, ctx, text, font_id, max_width, true)
}

/// `truncate_core`'s cache key: a hash of the text (the text itself is in
/// the value, compared on a hit, so a lookup allocates nothing), the font
/// size as bits (`f32` isn't `Hash`/`Eq`), the width bucket, and which end
/// is cut. Every call site builds its `FontId` with `FontId::proportional`,
/// so the family is not in the key. See `truncate_to_width` for why the
/// width is bucketed rather than exact.
pub(crate) type TruncateCacheKey = (u64, u32, i32, bool);

/// Width granularity `truncate_to_width` bucket its cache key to -- see
/// that function's own doc comment. A truncation result only actually
/// changes at the specific width where one more character's glyph would
/// fit, so nearby widths overwhelmingly share the same correct answer;
/// this just needs to be small enough that a bucket rarely straddles one
/// of those boundaries. Flooring (not rounding) to this bucket size
/// means the width actually used for the search is always <= the real
/// available width, so the visible failure mode of a stale/coarse bucket
/// is "very occasionally one character more truncated than strictly
/// necessary," never overflow past the card's own edge.
const TRUNCATE_WIDTH_BUCKET: f32 = 4.0;

/// Shortens `text` to the longest prefix (plus an ellipsis) that lays out
/// no wider than `max_width` at `font_id`, or returns it unchanged if it
/// already fits. Needed because graph-view node labels are now painted at
/// a fixed screen-space font size regardless of zoom (see
/// `build_graph_ui`'s `overlay_painter`/`project`) — at low zoom a node's
/// card shrinks well below the label's natural width, and unlike a normal
/// egui `Label` widget (which can `.truncate()` itself during layout),
/// text painted directly via `Painter::text` has no such built-in limit.
///
/// Memoized on `cache` -- each search step needs a real text-shaping call
/// (`ctx.fonts_mut(...).layout_no_wrap`), and the overwhelming common
/// case frame to frame is "this exact label, this exact width, again",
/// since neither a card's title nor its on-screen width usually change
/// between two consecutive frames. **Found on a general (not just
/// large-graph) performance audit**: this ran unconditionally, every
/// frame, for every visible labeled card, regardless of graph size.
///
/// **`max_width` is bucketed, not used exactly, for both the cache key
/// and the search bound.** Every call site derives it from
/// `zoom_scale`, which changes continuously (as a raw float) on every
/// frame during an active pan/zoom gesture -- keying on the exact width
/// meant this cache hit approximately never while the user was actually
/// zooming, the one time avoiding repeated font-shaping calls matters
/// most. Found live, following up on the general audit above.
pub(crate) fn truncate_to_width(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
) -> String {
    truncate_core(cache, ctx, text, font_id, max_width, false)
}

/// `text` cut to fit `max_width` with "…" at the cut: at the end, or at the
/// start when `from_start` (a path keeps its file name). Cuts fall on
/// grapheme boundaries, so a ZWJ emoji or a combining mark is never split,
/// and the width measured is the width drawn. A binary search over
/// grapheme boundaries rather than a shrink-by-one loop, since
/// `layout_no_wrap` (one text-shaping pass) is the expensive part and a
/// title could in principle be long. Memoized on `cache`.
fn truncate_core(
    cache: &mut HashMap<TruncateCacheKey, (String, String)>,
    ctx: &egui::Context,
    text: &str,
    font_id: &egui::FontId,
    max_width: f32,
    from_start: bool,
) -> String {
    use std::hash::{Hash, Hasher};
    use unicode_segmentation::UnicodeSegmentation;
    let bucketed_width = (max_width / TRUNCATE_WIDTH_BUCKET).floor() * TRUNCATE_WIDTH_BUCKET;
    let width_bucket = (max_width / TRUNCATE_WIDTH_BUCKET).floor() as i32;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let key: TruncateCacheKey = (hasher.finish(), font_id.size.to_bits(), width_bucket, from_start);
    if let Some((cached_text, cut)) = cache.get(&key) {
        if cached_text == text {
            return cut.clone();
        }
    }
    let width_of = |s: &str| -> f32 {
        ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font_id.clone(), egui::Color32::WHITE)).size().x
    };
    let result = if bucketed_width <= 0.0 {
        String::new()
    } else if width_of(text) <= bucketed_width {
        text.to_owned()
    } else {
        // Byte offsets of every grapheme boundary, both ends included.
        let mut bounds: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        bounds.push(text.len());
        let candidate = |k: usize| {
            if from_start {
                format!("\u{2026}{}", &text[bounds[bounds.len() - 1 - k]..])
            } else {
                format!("{}\u{2026}", &text[..bounds[k]])
            }
        };
        // The most graphemes `k` kept that still fit.
        let (mut lo, mut hi) = (0usize, bounds.len() - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if width_of(&candidate(mid)) <= bucketed_width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        candidate(lo)
    };
    if cache.len() >= TRUNCATE_CACHE_CAP {
        cache.clear();
    }
    cache.insert(key, (text.to_owned(), result.clone()));
    result
}
