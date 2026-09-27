//! What node a piece of content becomes, picked from its MIME type: the
//! one table every way content arrives goes through (a page a tab loaded,
//! the file picker, a dropped file, `ADDFILE`). See
//! `docs/superpowers/specs/2026-09-27-content-dispatch-design.md`.
//!
//! Local files have no Content-Type, so `sniff` reads their first bytes.
//! Hand-written rather than a crate: the crates surveyed know hundreds of
//! binary formats that all land in `Other` here, and the checks below
//! must decode or parse the bytes anyway (see the spec's "Sniffing local
//! files").

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::de::IgnoredAny;

/// A note holds at most this much text; longer text isn't converted.
pub(crate) const NOTE_MAX_BYTES: usize = 1_000_000;
/// How much of a local file `sniff` reads: one byte past a note's limit,
/// so a head this long means the file is too big to be a note.
pub(crate) const SNIFF_BYTES: usize = NOTE_MAX_BYTES + 1;

/// What content becomes. `Page` is a tab; `Other` is the engine's own
/// download (web) or a finished Download card (local file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentKind {
    Image,
    Pdf,
    Json,
    Note,
    Page,
    Other,
}

/// The table. Parameters (`; charset=utf-8`) and case are ignored.
pub(crate) fn kind_of(mime: &str) -> ContentKind {
    let essence = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match essence.as_str() {
        m if m.starts_with("image/") => ContentKind::Image,
        "application/pdf" => ContentKind::Pdf,
        "application/json" => ContentKind::Json,
        m if m.ends_with("+json") => ContentKind::Json,
        "text/plain" => ContentKind::Note,
        "text/html" | "application/xhtml+xml" => ContentKind::Page,
        _ => ContentKind::Other,
    }
}

/// Whether `bytes` really are `kind`: a server's header, or a file's
/// sniffed type, isn't trusted on its own.
pub(crate) fn passes(kind: ContentKind, bytes: &[u8]) -> bool {
    match kind {
        ContentKind::Image => image::load_from_memory(bytes).is_ok(),
        ContentKind::Pdf => bytes.starts_with(b"%PDF-"),
        ContentKind::Json => serde_json::from_slice::<IgnoredAny>(strip_bom(bytes)).is_ok(),
        ContentKind::Note => bytes.len() <= NOTE_MAX_BYTES && std::str::from_utf8(bytes).is_ok(),
        ContentKind::Page | ContentKind::Other => true,
    }
}

/// A local file's MIME type from its first bytes (`head`, at most
/// `SNIFF_BYTES`). The extension only breaks ties between text types:
/// `.json` text that doesn't parse is plain text, `.html` text without a
/// doctype is still HTML.
pub(crate) fn sniff(head: &[u8], file_name: &str) -> &'static str {
    if let Ok(format) = image::guess_format(head) {
        return format.to_mime_type();
    }
    if head.starts_with(b"%PDF-") {
        return "application/pdf";
    }
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        // A head cut mid-character is still text.
        Err(e) if e.error_len().is_none() && head.len() == SNIFF_BYTES => {
            std::str::from_utf8(&head[..e.valid_up_to()]).expect("valid up to here")
        }
        Err(_) => return "application/octet-stream",
    };
    let ext = Path::new(file_name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let body = text.trim_start_matches('\u{feff}').trim_start();
    if body.starts_with('{') || body.starts_with('[') {
        let whole = head.len() < SNIFF_BYTES;
        let parses = whole && serde_json::from_str::<IgnoredAny>(body).is_ok();
        // Too big to parse here: trust the extension; the insert parses it.
        if parses || (!whole && ext.as_deref() == Some("json")) {
            return "application/json";
        }
    }
    let starts =
        |prefix: &str| body.as_bytes().get(..prefix.len()).is_some_and(|b| b.eq_ignore_ascii_case(prefix.as_bytes()));
    if starts("<!doctype html") || starts("<html") || matches!(ext.as_deref(), Some("html" | "htm" | "xhtml")) {
        return "text/html";
    }
    "text/plain"
}

/// The extension a fetched file of `kind` is saved with.
pub(crate) fn extension(kind: ContentKind, bytes: &[u8]) -> &'static str {
    match kind {
        ContentKind::Image => {
            image::guess_format(bytes).ok().and_then(|f| f.extensions_str().first().copied()).unwrap_or("img")
        }
        ContentKind::Pdf => "pdf",
        ContentKind::Json => "json",
        ContentKind::Note => "txt",
        ContentKind::Page => "html",
        ContentKind::Other => "bin",
    }
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
}

/// The local path a `file:///` URL names, percent-decoded. `None` for any
/// other URL, including a `file://server/share` one.
pub(crate) fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file:///").or_else(|| url.strip_prefix("file://localhost/"))?;
    let rest = rest.split(['?', '#']).next()?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut it = rest.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex = [it.next()?, it.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    Some(PathBuf::from(if cfg!(windows) { path.replace('/', "\\") } else { format!("/{path}") }))
}

/// `path` as a `file:///` URL, the inverse of `file_url_path`.
pub(crate) fn path_file_url(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from("file:///");
    for b in path.trim_start_matches('/').bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => url.push(b as char),
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

/// The first `SNIFF_BYTES` of `path`.
pub(crate) fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::new();
    std::fs::File::open(path)?.take(SNIFF_BYTES as u64).read_to_end(&mut head)?;
    Ok(head)
}

/// Whether `path` is exactly `<dir>/<id>.<anything>` -- the shape
/// `redirect_fetch::save` always names a content file. Structural, not a
/// prefix check: `Path::starts_with` doesn't resolve `..`, so e.g.
/// `dir/../../x` would pass a naive `old.starts_with(dir)` and then get
/// deleted as if it were `x` -- comparing `path.parent()` to `dir`
/// component-wise instead rejects that, since a literal `..` component
/// never equals a real directory name.
// `prune`'s own `keep` closure and the app's stem rule for a synced
// node's local copy (`App::content_keep`) could plausibly share this
// (same "does this filename belong to a live id" question), but the keep
// rule is set-based -- checking membership in a set of ids, not parsing
// one specific path -- so it doesn't reuse this structural check.
pub(crate) fn owned_content_file(dir: &Path, id: &str, path: &Path) -> bool {
    path.parent() == Some(dir) && path.file_stem().is_some_and(|stem| stem == id)
}

/// Deletes each file in `dir` whose name `keep` rejects and whose modified
/// time is before `snapshot`; returns how many. A missing `dir` deletes
/// nothing.
///
/// `snapshot` should be taken when `keep`'s own name set was built, before
/// this runs (typically on a thread, some time later): several instances
/// can share one profile, and another instance's freshly fetched file,
/// whose node this instance hasn't learned about yet, must survive a sweep
/// built from a keep set that predates it. A file modified at or after
/// `snapshot` is never a candidate, whatever `keep` says.
pub(crate) fn prune(dir: &Path, snapshot: std::time::SystemTime, keep: impl Fn(&str) -> bool) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| !keep(&e.file_name().to_string_lossy()))
        .filter(|e| e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < snapshot))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(2, 2).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn the_table() {
        use ContentKind::*;
        for (mime, kind) in [
            ("image/png", Image),
            ("IMAGE/JPEG", Image),
            ("application/pdf", Pdf),
            ("application/json", Json),
            ("application/json; charset=utf-8", Json),
            ("application/geo+json", Json),
            ("text/plain", Note),
            ("text/plain;charset=UTF-8", Note),
            ("text/html", Page),
            ("application/xhtml+xml", Page),
            ("application/zip", Other),
            ("", Other),
        ] {
            assert_eq!(kind_of(mime), kind, "{mime}");
        }
    }

    #[test]
    fn checks() {
        use ContentKind::*;
        assert!(passes(Image, &png()));
        assert!(!passes(Image, b"not a picture"));
        assert!(passes(Pdf, b"%PDF-1.7\n..."));
        assert!(!passes(Pdf, b"<html>"));
        assert!(passes(Json, b"\xef\xbb\xbf{\"a\": [1, 2]}"));
        assert!(!passes(Json, b"{\"a\": "));
        assert!(passes(Note, "h\u{e9}llo".as_bytes()));
        assert!(!passes(Note, b"\xff\xfe"));
        assert!(!passes(Note, &vec![b'a'; NOTE_MAX_BYTES + 1]));
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(&png(), "x.png"), "image/png");
        // Content wins over a lying or missing extension.
        assert_eq!(sniff(&png(), "x.pdf"), "image/png");
        assert_eq!(sniff(&png(), "noext"), "image/png");
        assert_eq!(sniff(b"%PDF-1.4", "x.txt"), "application/pdf");
        assert_eq!(sniff(b"  {\"a\": 1}", "data"), "application/json");
        assert_eq!(sniff(b"[1, 2", "broken.json"), "text/plain");
        assert_eq!(sniff(b"<!DOCTYPE html><p>hi", "page"), "text/html");
        assert_eq!(sniff(b"<p>fragment</p>", "page.htm"), "text/html");
        assert_eq!(sniff(b"# Title\n", "notes.md"), "text/plain");
        assert_eq!(sniff(b"", "empty.txt"), "text/plain");
        assert_eq!(sniff(b"\x00\x01\x02\xff", "blob"), "application/octet-stream");
        // Too big to parse: the extension decides.
        let mut big = b"[".to_vec();
        big.resize(SNIFF_BYTES, b' ');
        assert_eq!(sniff(&big, "big.json"), "application/json");
        assert_eq!(sniff(&big, "big.txt"), "text/plain");
    }

    #[test]
    fn extensions() {
        assert_eq!(extension(ContentKind::Image, &png()), "png");
        assert_eq!(extension(ContentKind::Pdf, b""), "pdf");
        assert_eq!(extension(ContentKind::Json, b""), "json");
    }

    #[test]
    fn file_urls_round_trip() {
        let path = if cfg!(windows) { PathBuf::from(r"C:\Users\A B\x#1%.pdf") } else { PathBuf::from("/a b/x#1%.pdf") };
        let url = path_file_url(&path);
        assert!(!url.contains(' ') && !url.contains('#'), "{url}");
        assert_eq!(file_url_path(&url), Some(path));
        assert_eq!(file_url_path("https://example.com/x.pdf"), None);
        assert_eq!(file_url_path("file://server/share/x"), None);
    }

    #[test]
    fn read_head_stops_at_the_sniff_limit() {
        let dir = crate::store::testutil::temp_dir("content_head");
        let path = dir.join("big.txt");
        std::fs::write(&path, vec![b'a'; SNIFF_BYTES + 10]).unwrap();
        assert_eq!(read_head(&path).unwrap().len(), SNIFF_BYTES);
    }

    #[test]
    fn prune_keeps_what_keep_accepts() {
        let dir = crate::store::testutil::temp_dir("content_prune");
        for name in ["1-2.png", "3-4.json", "old.bin"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let snapshot = std::time::SystemTime::now();
        assert_eq!(prune(&dir, snapshot, |name| name != "old.bin"), 1);
        assert!(dir.join("1-2.png").exists() && !dir.join("old.bin").exists());
        assert_eq!(prune(&dir.join("missing"), snapshot, |_| false), 0);
    }

    #[test]
    fn prune_skips_a_file_modified_at_or_after_the_snapshot() {
        // Simulates another instance's fetch landing after this instance's
        // keep set was built but before the sweep runs: `new.bin` isn't in
        // `keep` either, but its mtime is at/after `snapshot`, so it must
        // survive regardless.
        let dir = crate::store::testutil::temp_dir("content_prune_race");
        let old = dir.join("old.bin");
        std::fs::write(&old, b"x").unwrap();
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::OpenOptions::new().write(true).open(&old).unwrap().set_modified(old_time).unwrap();

        let snapshot = std::time::SystemTime::now();
        let new = dir.join("new.bin");
        std::fs::write(&new, b"x").unwrap();
        std::fs::OpenOptions::new().write(true).open(&new).unwrap().set_modified(snapshot).unwrap();

        assert_eq!(prune(&dir, snapshot, |_| false), 1);
        assert!(!old.exists(), "older than the snapshot and unkept: pruned");
        assert!(new.exists(), "modified at the snapshot moment: kept despite being unkept");
    }

    #[test]
    fn owned_content_file_is_a_structural_check_not_a_prefix_one() {
        let dir = crate::store::testutil::temp_dir("owned_content_file");
        let id = "abc123";
        assert!(owned_content_file(&dir, id, &dir.join(format!("{id}.png"))));
        // `starts_with` alone would accept this -- `..` doesn't resolve to
        // the parent directory it names until the filesystem sees it, so a
        // naive prefix check would delete `id.png` two levels up instead.
        assert!(!owned_content_file(&dir, id, &dir.join("..").join(format!("{id}.png"))));
        assert!(!owned_content_file(&dir, id, &dir.join("sub").join(format!("{id}.png"))));
        assert!(!owned_content_file(&dir, id, &dir.join("other.png")));
        assert!(!owned_content_file(&dir, id, std::path::Path::new("/somewhere/else/abc123.png")));
    }
}
