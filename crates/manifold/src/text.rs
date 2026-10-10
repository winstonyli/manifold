/// Case-insensitive substring match against any of `haystacks` -- shared
/// by the Graph toolbar's own filter (via `graph_filter_dims` below) and
/// the History/Bookmarks popup filter boxes, so the three don't each
/// reimplement the same `to_lowercase().contains()` check slightly
/// differently. An empty `filter` always matches (nothing typed yet, so
/// nothing should be filtered out).
pub(crate) fn filter_matches(filter: &str, haystacks: &[&str]) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    let filter = filter.to_lowercase();
    haystacks.iter().any(|h| h.to_lowercase().contains(&filter))
}

/// `1234` -> `"1.2 KB"`, binary (1024-based) units -- matches Windows
/// Explorer's own convention for a "how big is this file" readout, which is
/// the nearest existing reference point a download progress display has in
/// this app.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The bare filename portion of a download's full path, for display --
/// shared by the graph card's own label and the "Downloads (N)" toolbar
/// dropdown so the two never show it differently. Falls back to the whole
/// path if it has no separable filename component (shouldn't normally
/// happen for a real downloaded file's path).
pub(crate) fn download_filename(path: &str) -> &str {
    std::path::Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path)
}

/// Cap on a truncation-style cache's size (`App::truncate_cache`, `App::favicon_origin_cache`) -- past this it is
/// cleared outright rather than evicted incrementally. No LRU bookkeeping for what should, in steady state, only ever
/// hold a small multiple of "how many labeled cards are visible at once"; a full clear is cheap insurance against
/// unbounded growth if that assumption is ever wrong (e.g. many distinct titles cycling through over a long session).
pub(crate) const TRUNCATE_CACHE_CAP: usize = 2000;

/// Explorer's `/select,` argument for `path`, quoted: Explorer splits an
/// unquoted argument at commas, so `a,b.pdf` opened the wrong folder.
#[cfg(windows)]
pub(crate) fn explorer_select_arg(path: &str) -> String {
    format!("/select,\"{path}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn explorer_select_quotes_the_path() {
        assert_eq!(explorer_select_arg(r"C:\d\a,b.pdf"), r#"/select,"C:\d\a,b.pdf""#);
    }
}
