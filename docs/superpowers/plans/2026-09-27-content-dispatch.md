# Content Dispatch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pick a node's kind from its content's MIME type through one table, whatever way the content arrived: a page a tab loaded, the file picker, a dropped file, or `ADDFILE`.

**Architecture:** A new host-only module `content.rs` holds the table (`kind_of`), the per-kind byte checks (`passes`), the local-file sniffer (`sniff`) and `file://` helpers. `App::insert_content` is the single insert path. `redirect_fetch.rs` grows from "images only, saved to `%TEMP%`" into "any table kind, saved to `profile::content_dir()`". Image, Pdf, Json and Note nodes keep their source URL in the store's existing `url` field, which enables Refresh, Open as tab, and refetching a missing file.

**Tech Stack:** Rust. Uses `image` 0.25 (`guess_format`, `load_from_memory`), `serde_json`, `ureq` 3, and winit 0.30 (`WindowEvent::DroppedFile`). All of these are existing dependencies; this plan adds no new crates.

**Spec:** `docs/superpowers/specs/2026-09-27-content-dispatch-design.md`

## Global Constraints

**Branch and checks**
- Work on `dev`; never touch `main`.
- Run `cargo fmt --all` before each commit.
- `cargo clippy -p manifold -p ipc-protocol --all-targets -- -D warnings` must pass.
- Never commit with `--no-verify`.
- Never add an `#[allow]` without a comment saying why.

**Tests and the profile folder**
- No test may read or write the real `%APPDATA%\manifold`.
- `profile::content_dir()` returns `None` under `cfg(test)`, like `favicon_cache_dir()`.
- Tests that need files use `store::testutil::temp_dir` and pass the path in.

**Build and scripts**
- Build and test with `-j 12`, for example `cargo test -p manifold -j 12 <filter>`.
- Headless runs go only through `scripts/drive.sh`, `scripts/headless-smoke-test.sh`, `scripts/two-instance-test.sh` and `scripts/scale-test.sh`. Run them through `rtk proxy bash scripts/<name>.sh`, or their output is filtered away.
- Rebuild with `cargo build -p manifold -j 12` before any script run. The scripts run the existing exe.
- Run Python as `python -P script.py < /dev/null`. Never pipe a script into `python -`.

**Commits**
- End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

**Content rules (from the spec)**
- Table:
  - `image/*` → Image
  - `application/pdf` → Pdf
  - `application/json` and `*/*+json` → Json
  - `text/plain` → Note
  - `text/html` and `application/xhtml+xml` → Tab
  - anything else → web: left to the engine's own download; local file: a finished Download card
- Checks:
  - an image must decode
  - a PDF must start with `%PDF-`
  - JSON must parse
  - a note must be UTF-8 and at most 1 MB (`1_000_000` bytes)
- A web page that fails its check stays a tab. A local file that fails is refused with a toast.
- Fetched files are named `<node id>.<ext>` in `profile::content_dir()` (`%APPDATA%\manifold\content`).
- The URL goes in store field `url` (5) on Image, Pdf, Json and Note. No new field number.
- Refresh applies to Image, Pdf and Json, never Note. Open as tab applies to any node with a URL.

## Rulings made while planning (spec gaps)

Each ruling is listed as decision — why — cost if wrong.

1. **The `path` field becomes explicit** (`FieldDef.explicit: true`).
   - Why: a replica that refetches a missing file saves it under its own content dir. As a diffed field, the new path would be written back to the store, and replicas with different profile paths would overwrite each other on every save. `path` never changes after creation for any kind today, so making it explicit changes nothing else. It matches the op-log spec's note that paths are machine-local.
   - Cost if wrong: small. One line of `FIELDS` changes, plus a sentence in the op-log spec.
2. **Web text becomes a Note holding the text; no file is saved.**
   - Why: a note stores its text in the store, Refresh is not offered for notes, so a file would be dead weight.
3. **"Open as tab" skips conversion through `App::skip_convert: HashSet<String>` (tab ids), not a `TabPayload` field.**
   - Why: the tab's node is inserted asynchronously after its spawn. A set keyed by the id we allocate avoids threading a flag through `TabSpawn` and `TabCarry`.
   - When an id leaves the set: the tab's first `RedirectableContent` or `JsonBody` report consumes it. A real URL change (where `json_body` is cleared) removes it. So does tab destroy.
4. **The JSON race.** When both `JsonBody` and a `text/plain` `RedirectableContent` arrive for one load, `JsonBody` converts synchronously and removes the tab. The text fetch then finds no tab and does nothing (the existing `still_current` check).
5. **Pruning keeps a content-dir file when either holds:**
   - its file name is the file name of a live node's path or a Recently Closed entry's `path` field, or
   - its stem is a live node id.
   - Why: a replica's refetched file is named by id while its stored path names another machine's file.
6. **Drop position.** winit 0.30 sends no `CursorMoved` during an OLE drag, so `DroppedFile` reads the real cursor through `GetCursorPos` + `ScreenToClient`.
   - `ADDFILE <path> x y` goes through the same placement code with the point given, so headless tests cover the placement. The OLE part itself is live-tested only.

## File Structure

**New file**
- `crates/manifold/src/content.rs`: the MIME table, `passes`, `sniff`, the file-extension choice, `file://` ↔ path, and `prune`. Pure functions, unit-tested.

**Modified files**
- `crates/manifold/src/profile.rs`: add `content_dir()`.
- `crates/manifold/src/store/model.rs`: make `path` explicit.
- `crates/manifold/src/node.rs`:
  - add `url: Option<String>` to `ImagePayload`, `PdfPayload`, `JsonPayload` and `NotePayload`
  - add `GraphNode::source_url`/`source_url_mut`
- `crates/manifold/src/session_sync.rs`:
  - `kind_fields` gains URL for Image, Pdf, Json and Note
  - payload ↔ fields mapping carries the URL
  - materialize resolves a missing file
- `crates/manifold/src/redirect_fetch.rs`: rework the redirect fetcher to handle every table kind (see Task 4).
- `crates/manifold/src/main.rs`:
  - `insert_content`, `redirect_tab_to_content`, `spawn_tab_at`
  - `add_file_node` dispatch
  - menu items, Refresh, Open as tab
  - `skip_convert`
  - prune at launch
- `crates/manifold/src/event_loop.rs`:
  - convert on `RedirectableContent` and `JsonBody`
  - the fetch drain
  - `DroppedFile`
  - `ADDFILE` handling
- `crates/manifold/src/debug_control.rs`: `ADDFILE` replaces `ADDIMAGE`, `ADDPDF` and `ADDJSON`.
- `crates/manifold/src/popups.rs`: remove `save_json_body_as_node` and its button.
- `crates/manifold/src/graph_ui.rs`: nothing expected. The missing-file click already routes to `retry_missing_file`.
- `crates/ipc-protocol/src/messages.rs`: doc comments on `RedirectableContent` and `JsonBody` only. No wire change.
- `Cargo.toml` of manifold: add `"Win32_UI_WindowsAndMessaging"` and `"Win32_Graphics_Gdi"` to the `windows-sys` features (Task 7).
- `scripts/headless-smoke-test.sh` (`ADDIMAGE` → `ADDFILE`, new checks), `README.md`, the spec's status line, and the op-log spec's `path` row.

---

### Task 1: `content.rs` (the table, checks, sniffer and helpers)

**Files:**
- Create: `crates/manifold/src/content.rs`
- Modify: `crates/manifold/src/main.rs` (add `mod content;` next to `mod redirect_fetch;`)

**Interfaces:**
- Produces:
  - `pub(crate) enum ContentKind { Image, Pdf, Json, Note, Page, Other }`, deriving `Clone, Copy, Debug, PartialEq, Eq`
  - `pub(crate) const NOTE_MAX_BYTES: usize = 1_000_000;`
  - `pub(crate) const SNIFF_BYTES: usize = NOTE_MAX_BYTES + 1;`
  - `pub(crate) fn kind_of(mime: &str) -> ContentKind`
  - `pub(crate) fn passes(kind: ContentKind, bytes: &[u8]) -> bool`
  - `pub(crate) fn sniff(head: &[u8], file_name: &str) -> &'static str`
  - `pub(crate) fn extension(kind: ContentKind, bytes: &[u8]) -> &'static str`
  - `pub(crate) fn file_url_path(url: &str) -> Option<std::path::PathBuf>`
  - `pub(crate) fn path_file_url(path: &std::path::Path) -> String`
  - `pub(crate) fn read_head(path: &std::path::Path) -> std::io::Result<Vec<u8>>` (reads at most `SNIFF_BYTES`)
  - `pub(crate) fn prune(dir: &std::path::Path, keep: impl Fn(&str) -> bool) -> usize` (deletes files whose name `keep` rejects; returns how many)

- [ ] **Step 1: Write the module with its tests.** Write the tests first and the bodies as `todo!()`, so Step 2 fails for real.

```rust
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
        ContentKind::Json => serde_json::from_slice::<serde_json::de::IgnoredAny>(strip_bom(bytes)).is_ok(),
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
        let parses = whole && serde_json::from_str::<serde_json::de::IgnoredAny>(body).is_ok();
        // Too big to parse here: trust the extension; the insert parses it.
        if parses || (!whole && ext.as_deref() == Some("json")) {
            return "application/json";
        }
    }
    let starts = |prefix: &str| body.as_bytes().get(..prefix.len()).is_some_and(|b| b.eq_ignore_ascii_case(prefix.as_bytes()));
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

/// Deletes each file in `dir` whose name `keep` rejects; returns how many.
/// A missing `dir` deletes nothing.
pub(crate) fn prune(dir: &Path, keep: impl Fn(&str) -> bool) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| !keep(&e.file_name().to_string_lossy()))
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
        assert_eq!(prune(&dir, |name| name != "old.bin"), 1);
        assert!(dir.join("1-2.png").exists() && !dir.join("old.bin").exists());
        assert_eq!(prune(&dir.join("missing"), |_| false), 0);
    }
}
```

Check `store::testutil::temp_dir`'s real signature with `grep -n "pub fn temp_dir" crates/manifold/src/store/testutil.rs` and adapt the two calls if it differs. Also confirm that `image::ImageFormat::to_mime_type` and `extensions_str` exist in image 0.25; both do in 0.25.x.

- [ ] **Step 2: Run the tests; they fail.**
  - Run: `cargo test -p manifold -j 12 content::tests`
  - Expected: FAIL, because the bodies are `todo!()` panics.

- [ ] **Step 3: Replace each `todo!()` with the bodies above.**

- [ ] **Step 4: Run the tests; they pass.**
  - Run: `cargo test -p manifold -j 12 content::tests`
  - Expected: 7 passed.
  - Also run `cargo clippy -p manifold --all-targets -j 12 -- -D warnings`.
  - Until Task 3 uses them, the module's items trigger `dead_code`. Put `#![cfg_attr(not(test), allow(dead_code))] // Used from Task 3 on; removed there.` at the top of the file, and delete it in Task 3.

- [ ] **Step 5: Commit.**

```bash
cargo fmt --all
git add crates/manifold/src/content.rs crates/manifold/src/main.rs
git commit -m "content: one MIME table, checks and a local-file sniffer

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Source URL on content nodes; `content_dir`; explicit `path`

**Files:**
- Modify:
  - `crates/manifold/src/profile.rs`
  - `crates/manifold/src/store/model.rs` (`FIELDS` row for `path`)
  - `crates/manifold/src/node.rs` (the four payloads, plus the `source_url` accessors)
  - `crates/manifold/src/session_sync.rs`: `kind_fields` (~line 37), `shared_nodes` (~315–339), the field-change apply (~448–451), materialize (~482–500)
  - every construction site of the four payloads (the compiler lists them)
  - `docs/superpowers/specs/2026-09-25-session-op-log-design.md`: the `url` and `path` rows of the field table (~135, 139)

**Interfaces:**
- Produces:
  - `profile::content_dir() -> Option<PathBuf>`
  - a `url: Option<String>` field on `ImagePayload`, `PdfPayload`, `JsonPayload` and `NotePayload`
  - `GraphNode::source_url(&self) -> Option<&str>`
  - `GraphNode::source_url_mut(&mut self) -> Option<&mut Option<String>>`
  - both accessors are `None` for every other kind, and for JSON leaves (`key.is_some()`)

- [ ] **Step 1: Write the failing tests.**
  - In `profile.rs` `tests_have_no_profile_dir`, add `assert_eq!(super::content_dir(), None);`.
  - In `session_sync.rs` tests, add a round-trip test. Model it on the nearest existing `shared_nodes` → `restore_session` test, for example the one near line 1876 that calls `assert!(app.restore_session())`; reuse its setup helpers.

```rust
#[test]
fn content_nodes_keep_their_source_url() {
    // A note made from a web page keeps its URL through save and restore.
    let mut app = /* the same test-App-with-store setup the neighbouring restore tests use */;
    let id = app.alloc_node_id();
    let payload = NotePayload { text: "hi".into(), editing: false, url: Some("https://x.test/a.txt".into()) };
    app.insert_placeholder_node(id.clone(), egui::Vec2::ZERO, NodePayload::Note(payload));
    let shared = app.shared_nodes();
    let fields = &shared[&id.parse().unwrap()].fields;
    assert_eq!(fields.get(&field::URL), Some(&FieldValue::Str("https://x.test/a.txt".into())));
    assert!(app.save_session());
    let mut restored = /* reopen the same store the way the neighbouring tests do */;
    assert!(restored.restore_session());
    assert_eq!(restored.nodes[&id].source_url(), Some("https://x.test/a.txt"));
}

#[test]
fn path_is_explicit() {
    assert!(crate::store::model::field_def(field::PATH).unwrap().explicit);
}
```

Replace the two `/* ... */` placeholders with the setup code the nearest existing restore test uses. Copy it; don't invent a helper.

- [ ] **Step 2: Run the tests; they fail.**
  - Run: `cargo test -p manifold -j 12 content_nodes_keep_their_source_url path_is_explicit tests_have_no_profile_dir`
  - Expected: compile error (`url` is not a field of `NotePayload`; `content_dir` doesn't exist).

- [ ] **Step 3: Implement.**

`profile.rs`, after `favicon_cache_dir` (also add `content` to the module doc's list):

```rust
/// Where content fetched from the web lives: each file is named
/// `<node id>.<ext>` (see `content`). `None` like `dir()`.
pub fn content_dir() -> Option<PathBuf> {
    Some(dir()?.join("content"))
}
```

`store/model.rs`: change the `path` row to `explicit: true`, and add a comment above it:

```rust
    // Machine-local: a replica that refetches a missing file saves it under
    // its own content dir, and that path must not be written back.
```

`node.rs`: add this field to each of the four payload structs:

```rust
    /// Where this content came from on the web, when it did (see
    /// `content`): Refresh and Open as tab use it, and a missing file is
    /// fetched again from it. `None` for a local file.
    pub(crate) url: Option<String>,
```

Add the accessors to `impl GraphNode`:

```rust
    /// The web source of an Image, Pdf, Json (not a leaf) or Note node.
    pub(crate) fn source_url(&self) -> Option<&str> {
        match &self.payload {
            NodePayload::Image(p) => p.url.as_deref(),
            NodePayload::Pdf(p) => p.url.as_deref(),
            NodePayload::Json(p) if p.key.is_none() => p.url.as_deref(),
            NodePayload::Note(p) => p.url.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn source_url_mut(&mut self) -> Option<&mut Option<String>> {
        match &mut self.payload {
            NodePayload::Image(p) => Some(&mut p.url),
            NodePayload::Pdf(p) => Some(&mut p.url),
            NodePayload::Json(p) if p.key.is_none() => Some(&mut p.url),
            NodePayload::Note(p) => Some(&mut p.url),
            _ => None,
        }
    }
```

At every construction site the compiler reports, add `url: None`. The web-sourced sites come in later tasks.

`session_sync.rs`, `kind_fields`:

```rust
        Kind::Note => &[POS, COLOR, URL, TEXT],
        Kind::Image | Kind::Pdf | Kind::Json => &[POS, COLOR, URL, PATH],
        Kind::Download => &[POS, COLOR, PATH],
```

In `shared_nodes`, in the Note, Image, Pdf and Json (`None` key) arms, insert the URL. Absent is the empty string (`str_of` reads a missing field as `""`):

```rust
                    fields.insert(field::URL, str(n.url.as_deref().unwrap_or("")));
```

Check that `FieldValue::absent(FieldType::Str)` is `FieldValue::Str(String::new())`. If it isn't, insert `FieldValue::absent(FieldType::Str)` when `url` is `None`.

In the field-change apply, a changed URL updates `url` for those four kinds:

```rust
                NodePayload::Note(n) => {
                    if changed(field::TEXT) {
                        n.text = str_of(fields, field::TEXT);
                    }
                    if changed(field::URL) {
                        n.url = url_of(fields);
                    }
                }
                NodePayload::Image(_) | NodePayload::Pdf(_) | NodePayload::Json(_) if changed(field::URL) => {
                    if let Some(url) = node.source_url_mut() { *url = url_of(fields); }
                }
                // Paths, keys and values never change after creation.
                NodePayload::Download(_) | NodePayload::Image(_) | NodePayload::Pdf(_) | NodePayload::Json(_) => {}
```

Adjust the borrow: the match is on `&mut node.payload`. If `node` isn't nameable there, set the field inside each arm the way the Note arm does. Add this helper next to `str_of`:

```rust
fn url_of(fields: &Fields) -> Option<String> {
    Some(str_of(fields, field::URL)).filter(|u| !u.is_empty())
}
```

In materialize, after the `match kind { ... }` block where group and color are set, also set the URL:

```rust
        if let Some(node) = self.nodes.get_mut(&id) {
            node.group = group;
            node.color = color;
            if let Some(url) = node.source_url_mut() {
                *url = url_of(fields);
            }
        }
```

Op-log spec field table:
- `url` row: Kinds becomes `Tab, Image, Pdf, Json, Note`. Append to Notes: "On Image, Pdf, Json and Note it is the web source, set once at creation (content dispatch spec)."
- `path` row: Emitted becomes `explicit`. Append: "Only creation sets it: a replica that refetches a missing file keeps its own path (content dispatch spec)."

- [ ] **Step 4: Run the tests; they pass.**
  - Run: `cargo test -p manifold -j 12`
  - Expected: all pass. Among them are `profile::tests` (no new `"APPDATA"` mention, since `content_dir` builds on `dir()`) and the `debug_assert` in `shared_nodes` that checks the fields equal `kind_fields`.

- [ ] **Step 5: Commit.**

```bash
cargo fmt --all
git add -A crates/manifold/src docs/superpowers/specs/2026-09-25-session-op-log-design.md
git commit -m "Content nodes keep their web source in the url field

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `insert_content` for local files: picker, `ADDFILE`, notes, pages, other files

**Files:**
- Modify:
  - `crates/manifold/src/main.rs`: new `ContentSource`, `insert_content`, `insert_note_with_text`, `spawn_tab_at`; rewrite `add_file_node` (~4455); delete `insert_image_node`, `insert_pdf_node` and `insert_json_node`, which become unused
  - `crates/manifold/src/debug_control.rs` (~419–421 help, ~657–660 parse)
  - `crates/manifold/src/event_loop.rs` (~886–899)
  - `scripts/headless-smoke-test.sh` (line ~423)
  - `crates/manifold/src/content.rs`: remove the `dead_code` allow

**Interfaces:**
- Consumes:
  - from Task 1: `content::{kind_of, sniff, read_head, path_file_url, ContentKind, NOTE_MAX_BYTES, SNIFF_BYTES}`
  - from Task 2: `url` on the payloads, `source_url_mut`
- Produces:

```rust
/// Content on its way to becoming a node (`App::insert_content`).
pub(crate) enum ContentSource {
    /// A file on this machine. Sniffed; the node points at it where it is.
    Local(std::path::PathBuf),
    /// Checked content from `url` (see `redirect_fetch`).
    Web { url: String, body: WebBody },
}

pub(crate) enum WebBody {
    /// Saved in the content dir as `<id>.<ext>`; `id` is the node's id.
    File { kind: content::ContentKind, path: String },
    /// A note's text.
    Text(String),
}
```

- `fn insert_content(&mut self, source: ContentSource, id: String, pos: egui::Vec2, group: Option<String>) -> Result<(), String>`
  - On `Err`, the failure has already been toasted.
  - Does not save; callers call `save_session`, as the insert functions already work.
  - `ContentKind::Page` spawns a tab and returns `Ok`.
- `fn spawn_tab_at(&mut self, url: &str, pos: egui::Vec2, group: Option<String>) -> String` (the tab id)
- `fn insert_note_with_text(&mut self, id: String, text: String, url: Option<String>, pos: egui::Vec2, group: Option<String>)`
- `DebugCommand::AddFile(String, Option<(f32, f32)>)`, replacing `AddImage`, `AddPdf` and `AddJson`. The point is in window points and is used by Task 7. In this task, `None` means "jittered around world center at the current level", as the three old commands behaved.

- [ ] **Step 1: Write the failing tests** in `main.rs`'s test module. Model the App setup on the existing insert tests; grep `insert_json_node_with_id` in tests. `App::default()` has `gpu: None`, so an image insert skips the upload.

```rust
#[test]
fn insert_content_dispatches_local_files_by_content() {
    let dir = crate::store::testutil::temp_dir("insert_content");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(3, 2).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let files = [
        ("pic.pdf", png.into_inner()), // lying extension: still an image
        ("doc", b"%PDF-1.4\n".to_vec()),
        ("data.txt", br#"{"a": 1}"#.to_vec()),
        ("notes.md", b"# hi".to_vec()),
        ("blob.bin", vec![0, 1, 2, 255]),
    ];
    let mut app = App::default();
    let mut kinds = Vec::new();
    for (name, bytes) in files {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let id = app.alloc_node_id();
        app.insert_content(ContentSource::Local(path), id.clone(), egui::Vec2::ZERO, None).unwrap();
        kinds.push(match &app.nodes[&id].payload {
            NodePayload::Image(p) => format!("image {}x{}", p.width, p.height),
            NodePayload::Pdf(_) => "pdf".into(),
            NodePayload::Json(p) => format!("json {}", p.value),
            NodePayload::Note(p) => format!("note {}", p.text),
            NodePayload::Download(p) => format!("download {} done={}", p.bytes_received, p.completed),
            _ => "other".into(),
        });
        assert_eq!(app.nodes[&id].source_url(), None);
    }
    assert_eq!(kinds, ["image 3x2", "pdf", r#"json {"a":1}"#, "note # hi", "download 4 done=true"]);
}

#[test]
fn insert_content_refuses_a_failing_local_file() {
    let dir = crate::store::testutil::temp_dir("insert_content_refuse");
    let big = dir.join("big.txt");
    std::fs::write(&big, vec![b'a'; crate::content::NOTE_MAX_BYTES + 1]).unwrap();
    let broken = dir.join("broken.png");
    std::fs::write(&broken, b"\x89PNG\r\n\x1a\n not really").unwrap();
    let mut app = App::default();
    for path in [big, broken, dir.join("missing.txt")] {
        let id = app.alloc_node_id();
        assert!(app.insert_content(ContentSource::Local(path), id.clone(), egui::Vec2::ZERO, None).is_err());
        assert!(!app.nodes.contains_key(&id));
    }
    assert_eq!(app.save_toast_queue.len() + usize::from(app.save_toast.is_some()), 3, "each refusal toasts");
}
```

Adapt the toast assertion to however the existing tests count toasts; grep `save_toast_queue` in tests.

- [ ] **Step 2: Run the tests; they fail.**
  - Run: `cargo test -p manifold -j 12 insert_content`
  - Expected: compile error (`ContentSource` and `insert_content` don't exist).

- [ ] **Step 3: Implement.**

```rust
    /// The one way content becomes a node (see `content`): a picked,
    /// dropped or `ADDFILE` file, or a page a tab converted. `id`, `pos`
    /// and `group` place it. A failure is toasted here.
    fn insert_content(
        &mut self,
        source: ContentSource,
        id: String,
        pos: egui::Vec2,
        group: Option<String>,
    ) -> Result<(), String> {
        use content::ContentKind as K;
        let (url, kind, path) = match source {
            ContentSource::Web { url, body: WebBody::Text(text) } => {
                self.insert_note_with_text(id, text, Some(url), pos, group);
                return Ok(());
            }
            ContentSource::Web { url, body: WebBody::File { kind, path } } => (Some(url), kind, path),
            ContentSource::Local(path) => {
                let name = crate::json_file::file_name(&path);
                let head = content::read_head(&path).map_err(|err| {
                    let err = format!("Couldn't open {name}: {err}");
                    self.queue_toast(err.clone());
                    err
                })?;
                let kind = content::kind_of(content::sniff(&head, &name));
                let path_str = path.to_string_lossy().into_owned();
                match kind {
                    K::Note if head.len() > content::NOTE_MAX_BYTES => {
                        let err = format!("Couldn't open {name}: too long for a note (over 1 MB)");
                        self.queue_toast(err.clone());
                        return Err(err);
                    }
                    K::Note => {
                        // `sniff` said text and the head is the whole file.
                        let text = String::from_utf8(head).map_err(|e| e.to_string())?;
                        self.insert_note_with_text(id, text, None, pos, group);
                        return Ok(());
                    }
                    K::Page => {
                        self.spawn_tab_at(&content::path_file_url(&path), pos, group);
                        return Ok(());
                    }
                    K::Other => {
                        let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
                        let payload = DownloadPayload {
                            path: path_str,
                            bytes_received: bytes,
                            total_bytes: Some(bytes),
                            completed: true,
                        };
                        self.insert_placeholder_node(id.clone(), pos, NodePayload::Download(payload));
                        if let Some(node) = self.nodes.get_mut(&id) {
                            node.group = group;
                        }
                        return Ok(());
                    }
                    _ => (None, kind, path_str),
                }
            }
        };
        match kind {
            K::Image => self.insert_image_node_with_id(id.clone(), path, pos, group)?,
            K::Pdf => self.insert_pdf_node_with_id(id.clone(), path, pos, group),
            K::Json => self.insert_json_node_with_id(id.clone(), path, pos, group)?,
            // A web page never reaches here as a file (see `redirect_fetch`).
            K::Note | K::Page | K::Other => return Err(format!("{kind:?} is not a file kind")),
        }
        if let Some(slot) = self.nodes.get_mut(&id).and_then(|n| n.source_url_mut()) {
            *slot = url;
        }
        Ok(())
    }

    fn insert_note_with_text(
        &mut self,
        id: String,
        text: String,
        url: Option<String>,
        pos: egui::Vec2,
        group: Option<String>,
    ) {
        let payload = NotePayload { text, editing: false, url };
        self.insert_placeholder_node(id.clone(), pos, NodePayload::Note(payload));
        if let Some(node) = self.nodes.get_mut(&id) {
            node.group = group;
        }
    }

    /// A new tab at `url`, placed at `pos` in `group`. Returns its id.
    fn spawn_tab_at(&mut self, url: &str, pos: egui::Vec2, group: Option<String>) -> String {
        let id = self.alloc_node_id();
        let (kind, zoom) = (self.settings.default_starting_engine, self.settings.zoom_factor);
        let spawn = TabSpawn::new(id.clone(), url.to_string(), kind, pos, SpawnPurpose::New, zoom);
        self.spawn_tab_async(TabSpawn { group, ..spawn });
        id
    }
```

Before finalizing `spawn_tab_at`, read `spawn_new_tab_with_kind` (main.rs ~3378) to its end. If it does anything after computing its position besides `spawn_tab_async` (bookkeeping, `save_session`), do the same here.

Check whether `insert_placeholder_node` already takes a group. If it does, pass `group` and drop the `get_mut`. Also check `json_file::file_name`'s parameter type (it takes `&Path` at the existing call sites).

Rewrite `add_file_node`:
- keep its doc, updated to say it dispatches through `insert_content`
- keep its `pos: Option<egui::Vec2>` meaning: `None` → jitter around world center
- widen the filter: keep the named filter, and add `add_filter("All files", &["*"])`

```rust
    fn add_file_node(&mut self, pos: Option<egui::Vec2>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Image, PDF, JSON or text", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "pdf", "json", "txt", "md", "html", "htm"])
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        if self.add_file_at(path, pos).is_ok() {
            self.save_session();
        }
    }

    /// `path` as a new node at `pos`, or jittered around world center, at
    /// the current level. The picker and `ADDFILE`.
    fn add_file_at(&mut self, path: std::path::PathBuf, pos: Option<egui::Vec2>) -> Result<(), String> {
        let id = self.alloc_node_id();
        let pos = pos.unwrap_or_else(|| GRAPH_WORLD_CENTER + graph_layout::spawn_jitter(&id));
        let group = self.current_group.clone();
        self.insert_content(ContentSource::Local(path), id, pos, group)
    }
```

Delete `insert_image_node`, `insert_pdf_node` and `insert_json_node`. Their only callers were `add_file_node` and the three debug commands.

`debug_control.rs`:
- Replace the `AddImage`, `AddPdf` and `AddJson` variants with `AddFile(String, Option<(f32, f32)>)`.
- Parse `ADDFILE <path>` and `ADDFILE <path> <x> <y>`. The path may contain spaces, so peel two trailing numbers off the end only when both parse as `f32`.
- Help line: `ADDFILE <path> [x y]  adds a file as the node its content makes (image, PDF, JSON, note, tab, or a download card); x y is a window point, like a drop there`.
- Add a parser unit test next to the existing parse tests:

```rust
    assert_eq!(parse("ADDFILE C:\\a b\\c.png"), Some(DebugCommand::AddFile("C:\\a b\\c.png".into(), None)));
    assert_eq!(parse("ADDFILE C:\\a b\\c.png 10 20.5"), Some(DebugCommand::AddFile("C:\\a b\\c.png".into(), Some((10.0, 20.5)))));
```

Use whatever the existing parse tests call instead of `parse`, and match how they compare (`matches!` if `DebugCommand` has no `PartialEq`).

`event_loop.rs`: replace the three arms with:

```rust
            DebugCommand::AddFile(path, None) => {
                if self.add_file_at(path.into(), None).is_ok() {
                    self.save_session();
                }
            }
            // Placed like a drop at that window point (Task 7).
            DebugCommand::AddFile(path, Some(_point)) => {
                if self.add_file_at(path.into(), None).is_ok() {
                    self.save_session();
                }
            }
```

(Task 7 replaces the second arm.)

`scripts/headless-smoke-test.sh` line ~423: `ADDIMAGE` → `ADDFILE`, unchanged otherwise.

- [ ] **Step 4: Run the tests and the smoke test; they pass.**
  - Run: `cargo test -p manifold -j 12`
  - Expected: all pass.
  - Then run `cargo build -p manifold -j 12 && rtk proxy bash scripts/headless-smoke-test.sh`.
  - Expected: 29/29.

- [ ] **Step 5: Commit.**

```bash
cargo fmt --all
git add -A crates/manifold/src scripts/headless-smoke-test.sh
git commit -m "One insert path for files: sniffed, dispatched through the MIME table; ADDFILE

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Web conversion for every table kind

**Files:**
- Modify:
  - `crates/manifold/src/redirect_fetch.rs`: rewrite
  - `crates/manifold/src/main.rs`: `redirect_tab_to_image` → `redirect_tab_to_content`; `redirect_fetcher` construction (~1538); the `use` at line 38
  - `crates/manifold/src/event_loop.rs`: the drain (~1115), `JsonBody` (~1284), `RedirectableContent` (~1294)
  - `crates/manifold/src/popups.rs`: delete `save_json_body_as_node` (~1188) and its button in `build_json_viewer`, then update that function's doc
  - `crates/ipc-protocol/src/messages.rs`: the `RedirectableContent` and `JsonBody` doc comments

**Interfaces:**
- Consumes:
  - `content::{kind_of, passes, extension, file_url_path, ContentKind}`
  - `ContentSource`, `WebBody`, `insert_content` (Task 3)
- Produces, in `redirect_fetch`:

```rust
/// What a finished fetch is for.
pub enum FetchTarget {
    /// Convert this tab, if it still shows `url`.
    Tab(String),
    /// Reload this node's file (Refresh, or a missing file; Task 5/6).
    Node(String),
}

pub struct FetchJob {
    pub target: FetchTarget,
    /// The node the content becomes: names the saved file.
    pub node_id: String,
    pub url: String,
    pub kind: crate::content::ContentKind,
}

pub struct FetchResult {
    pub job: FetchJob,
    /// `None`: the fetch failed or the bytes failed their check.
    pub body: Option<crate::WebBody>,
}

impl RedirectFetcher {
    /// `dir` is `profile::content_dir()`; `None` (tests) fails every fetch.
    pub fn new(dir: Option<std::path::PathBuf>) -> Self;
    pub fn request(&self, job: FetchJob);
    pub fn tick(&self) -> Vec<FetchResult>;
}

/// Fetches (or, for `file://`, reads) `url`, checks it is `kind`, and
/// saves it as `<node_id>.<ext>` in `dir`. A note's text comes back
/// instead of being saved. Blocking: `request` runs it on a thread.
pub fn fetch(dir: &std::path::Path, node_id: &str, url: &str, kind: ContentKind) -> Option<crate::WebBody>;

/// Checks `bytes` are `kind` and saves them (or returns a note's text).
pub fn save(dir: &std::path::Path, node_id: &str, kind: ContentKind, bytes: Vec<u8>) -> Option<crate::WebBody>;
```

- `App::redirect_tab_to_content(&mut self, tab_id: &str, node_id: String, url: String, body: WebBody)`

If `WebBody` lives in `main.rs`, refer to it from `redirect_fetch` as `crate::WebBody`. Make it `pub(crate)`; the module is private to the crate.

- [ ] **Step 1: Write the failing tests.** Every one is network-free; `file://` reads from disk.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{path_file_url, ContentKind};
    use crate::WebBody;

    #[test]
    fn saves_checked_content_by_node_id() {
        let dir = crate::store::testutil::temp_dir("redirect_save");
        match save(&dir, "7-1", ContentKind::Json, br#"{"a": 1}"#.to_vec()) {
            Some(WebBody::File { kind: ContentKind::Json, path }) => {
                assert_eq!(std::path::Path::new(&path), dir.join("7-1.json"));
                assert_eq!(std::fs::read(&path).unwrap(), br#"{"a": 1}"#);
            }
            _ => panic!("expected a saved JSON file"),
        }
        assert!(save(&dir, "7-2", ContentKind::Json, b"{broken".to_vec()).is_none());
        assert!(!dir.join("7-2.json").exists());
        assert!(matches!(save(&dir, "7-3", ContentKind::Note, b"hi".to_vec()), Some(WebBody::Text(t)) if t == "hi"));
        assert!(!dir.join("7-3.txt").exists(), "a note's text isn't saved");
    }

    #[test]
    fn file_urls_are_read_from_disk() {
        let dir = crate::store::testutil::temp_dir("redirect_file_url");
        let src = dir.join("in put.pdf");
        std::fs::write(&src, b"%PDF-1.4\n").unwrap();
        let out = dir.join("out");
        let body = fetch(&out, "9-1", &path_file_url(&src), ContentKind::Pdf);
        assert!(matches!(body, Some(WebBody::File { kind: ContentKind::Pdf, .. })));
        assert!(out.join("9-1.pdf").exists(), "the content dir is created on demand");
    }
}
```

In `main.rs` tests, add a conversion test. It needs a tab node without an engine; grep existing tests for how they build a `Tab` payload node (for example the `remove_from_group` test's `node` helper, ~5540). If no tab-shaped test node is constructible without an engine process, test `redirect_tab_to_content` on an `Unloaded` node instead, and relax the tab lookup to `payload.saved_tab()`.

```rust
#[test]
fn a_converted_tab_keeps_its_place_and_url() {
    let dir = crate::store::testutil::temp_dir("redirect_convert");
    let mut app = App::default();
    let tab = /* insert a tab-like node at pos (5, 6) in group Some("g"), with a child node whose parent is it */;
    let body = crate::redirect_fetch::save(&dir, "n", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.redirect_tab_to_content(&tab, "n".into(), "https://x.test/a.json".into(), body);
    assert!(!app.nodes.contains_key(&tab));
    let node = &app.nodes["n"];
    assert_eq!((node.pos, node.group.as_deref()), (egui::vec2(5.0, 6.0), Some("g")));
    assert_eq!(node.source_url(), Some("https://x.test/a.json"));
    assert!(app.nodes.values().all(|n| n.parent.as_deref() != Some(tab.as_str())), "children moved up");
}
```

Replace the placeholder comment with real setup from the neighbouring tests.

- [ ] **Step 2: Run the tests; they fail.**
  - Run: `cargo test -p manifold -j 12 redirect`
  - Expected: compile errors (`save`, `fetch`, `redirect_tab_to_content` don't exist).

- [ ] **Step 3: Implement.**

`redirect_fetch.rs`: rewrite the module doc to describe the new scope. The kind comes from `content::kind_of`; bytes are checked with `content::passes`; files are saved to the content dir as `<node id>.<ext>`; `file://` is read from disk. Remove the JSON-is-opt-in paragraph. Body:

```rust
use crate::content::{self, ContentKind};
use crate::WebBody;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

pub fn fetch(dir: &Path, node_id: &str, url: &str, kind: ContentKind) -> Option<WebBody> {
    let bytes = match content::file_url_path(url) {
        Some(path) => std::fs::read(path).ok()?,
        None => {
            let response = ureq::get(url).config().timeout_global(Some(Duration::from_secs(15))).build().call().ok()?;
            response.into_body().with_config().limit(100_000_000).read_to_vec().ok()?
        }
    };
    save(dir, node_id, kind, bytes)
}

pub fn save(dir: &Path, node_id: &str, kind: ContentKind, bytes: Vec<u8>) -> Option<WebBody> {
    if !content::passes(kind, &bytes) {
        return None;
    }
    if kind == ContentKind::Note {
        return String::from_utf8(bytes).ok().map(WebBody::Text);
    }
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{node_id}.{}", content::extension(kind, &bytes)));
    std::fs::write(&path, &bytes).ok()?;
    Some(WebBody::File { kind, path: path.to_string_lossy().into_owned() })
}
```

`RedirectFetcher` keeps its channel shape. It stores `dir: Option<PathBuf>`, and `request` spawns a thread that runs `dir.as_deref().and_then(|d| fetch(d, &job.node_id, &job.url, job.kind))`, then sends a `FetchResult`. Construct it with `RedirectFetcher::new(crate::profile::content_dir())`.

`main.rs`:
- Rename `redirect_tab_to_image` to `redirect_tab_to_content(tab_id, node_id, url, body)`.
- Keep its body up to the id allocation. Then:

```rust
        // Failure is already toasted; the tab it replaces is gone either way.
        let _ = self.insert_content(ContentSource::Web { url, body }, node_id, pos, group);
        self.save_session();
```

- Update its doc: "Converts a tab whose page is content the table maps to a node…". Also remove `skip_convert`'s entry for the tab here, once Task 5 adds it.

`event_loop.rs`, `RedirectableContent`:

```rust
            EngineToHost::RedirectableContent { content_type } => {
                use crate::content::ContentKind as K;
                let url = self.nodes.get(tab_id).and_then(|n| n.as_tab()).map(|t| t.url.clone());
                let kind = crate::content::kind_of(&content_type);
                if let (Some(url), K::Image | K::Pdf | K::Json | K::Note) = (url, kind) {
                    let node_id = self.alloc_node_id();
                    let target = FetchTarget::Tab(tab_id.to_string());
                    self.redirect_fetcher.request(FetchJob { target, node_id, url, kind });
                }
            }
```

`JsonBody`: keep setting `tab.json_body` (so "View as JSON" still works on a tab kept by Open as tab), then convert right away:

```rust
            EngineToHost::JsonBody { body } => {
                let url = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()).map(|tab| {
                    tab.json_body = Some(body.clone());
                    tab.url.clone()
                });
                if let (Some(url), Some(dir)) = (url, crate::profile::content_dir()) {
                    let node_id = self.alloc_node_id();
                    if let Some(web) = crate::redirect_fetch::save(&dir, &node_id, crate::content::ContentKind::Json, body.into_bytes()) {
                        self.redirect_tab_to_content(tab_id, node_id, url, web);
                    }
                }
            }
```

This writes the file on the UI thread. JSON bodies are the engine's in-memory page text, already sent over IPC, so that's acceptable.

Drain:

```rust
        for FetchResult { job, body } in self.redirect_fetcher.tick() {
            let Some(body) = body else { continue };
            match job.target {
                // Still the same tab showing the same url (see above).
                FetchTarget::Tab(tab_id) => {
                    let still_current =
                        self.nodes.get(&tab_id).and_then(|n| n.as_tab()).is_some_and(|t| t.url == job.url);
                    if still_current {
                        self.redirect_tab_to_content(&tab_id, job.node_id, job.url, body);
                    }
                }
                FetchTarget::Node(id) => self.reload_fetched_node(&id, body), // Task 5
            }
        }
```

In this task, have `FetchTarget::Node(_) => {}` with no comment. Task 5 fills it.

`popups.rs`: delete `save_json_body_as_node` and the "Save as Node" button. Update `build_json_viewer`'s doc to drop the opt-in paragraph.

`messages.rs`: in the `RedirectableContent` doc, replace everything from "classifies `content_type` against a small, explicit allowlist" through the end with:

> "…classifies `content_type` with `content::kind_of` (the host's one MIME table), refetches the url, checks the bytes and, when they pass, replaces the tab with the node the table names at the same position. Sent by all four engines for any content type but `application/json`, which comes as `JsonBody` instead and converts from that body."

In the `JsonBody` doc, note that the host now converts the tab to a Json node from this body.

- [ ] **Step 4: Run the tests; they pass.**
  - Run: `cargo test -p manifold -j 12`, then `cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings`.
  - Expected: all pass, clippy clean.

- [ ] **Step 5: Live probe with `drive.sh`.**
  - Write a command file with the Write tool. It opens `file:///` URLs of a PDF and a JSON file placed under `target/contentprobe/`, then `EXPORTSTATE`.
  - Check that each tab became a Pdf or Json card whose state shows the URL. If `EXPORTSTATE` does not carry payload URLs, use `EXPORTSESSION` and look for the `url` field on those nodes.
  - Check which engines convert `file://`: each engine's `RedirectableContent` detection must fire for `file://` pages. Record the result per engine in the report. An engine that doesn't is a finding, not a blocker.

- [ ] **Step 6: Commit.**

```bash
cargo fmt --all
git add -A crates/manifold/src crates/ipc-protocol/src/messages.rs
git commit -m "Web content converts through the MIME table; JSON pages convert automatically

Save as Node is gone: a JSON page is already a node. Fetched files live in
the profile's content dir, named by node id.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Refresh and Open as tab

**Files:**
- Modify: `crates/manifold/src/main.rs`:
  - `NodeAction` (~140): add `Refresh` and `OpenAsTab`
  - `NodeMenuView` (~390): add `has_url`
  - `node_menu_config` (~307)
  - the menu tests (~5421–5535)
  - the action dispatch (~4985)
  - new `refresh_node`, `reload_fetched_node`, `open_as_tab`
  - new `skip_convert: HashSet<String>` on `App`
- Modify: `crates/manifold/src/event_loop.rs`:
  - consume `skip_convert` in `RedirectableContent` and `JsonBody`
  - clear it on a real URL change (~1202) and on tab destroy
  - the drain's `FetchTarget::Node` arm

**Interfaces:**
- Consumes: `FetchJob`, `FetchTarget::Node` and `WebBody` (Task 4); `source_url` (Task 2); `spawn_tab_at` (Task 3)
- Produces:
  - `fn refresh_node(&mut self, id: &str)`: requests a `FetchTarget::Node` fetch for a node with a URL
  - `fn reload_fetched_node(&mut self, id: &str, body: WebBody)`: swaps the node's file and reloads it in place, keeping the same id, position, group, color and order
  - `fn open_as_tab(&mut self, id: &str)`

Menu order for Image, Pdf and Json with a URL: `Refresh | Open as Tab | Open Containing Folder | Copy Path | [Expand as Graph] | <colors> …`. For Note with a URL: `Open as Tab | Copy Text | <colors> …`.

- [ ] **Step 1: Write the failing tests.**
  - Add `has_url: false` to every `NodeMenuView` literal in the tests.
  - Add to `node_menu_configs`:

```rust
        let with_url = |kind| NodeMenuView { kind, has_url: true, ..view_of(kind) };
        assert_eq!(
            menu_of(&with_url(ItemNodeKind::Image), false).2,
            "Refresh | Open as Tab | Open Containing Folder | Copy Path | <colors> | --- | Delete"
        );
        assert_eq!(
            menu_of(&with_url(ItemNodeKind::Json), false).2,
            "Refresh | Open as Tab | Open Containing Folder | Copy Path | Expand as Graph | <colors> | --- | Delete"
        );
        assert_eq!(menu_of(&with_url(ItemNodeKind::Note), false).2, "Open as Tab | Copy Text | <colors> | --- | Delete");
```

`view_of` is a small helper building the default view the `menu` helper builds. Extract it from `menu` so both use it.

```rust
#[test]
fn a_refetched_file_reloads_the_node_in_place() {
    let dir = crate::store::testutil::temp_dir("refresh_reload");
    let mut app = App::default();
    let first = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[1]".to_vec()).unwrap();
    app.insert_content(ContentSource::Web { url: "https://x.test/j".into(), body: first }, "r".into(), egui::vec2(1.0, 2.0), None).unwrap();
    app.nodes.get_mut("r").unwrap().color = Some(egui::Color32::RED);
    let order = app.node_order.clone();
    let second = crate::redirect_fetch::save(&dir, "r", crate::content::ContentKind::Json, b"[2]".to_vec()).unwrap();
    app.reload_fetched_node("r", second);
    let node = &app.nodes["r"];
    assert_eq!(node.as_json().unwrap().value, serde_json::json!([2]));
    assert_eq!((node.pos, node.color, node.source_url()), (egui::vec2(1.0, 2.0), Some(egui::Color32::RED), Some("https://x.test/j")));
    assert_eq!(app.node_order, order);
}
```

- [ ] **Step 2: Run the tests; they fail.**
  - Run: `cargo test -p manifold -j 12 node_menu a_refetched_file`
  - Expected: compile errors.

- [ ] **Step 3: Implement.**

`node_menu_config`: build the URL items first, and put them in front of each kind's items:

```rust
    let url_items = |refresh: bool| {
        let mut items = Vec::new();
        if view.has_url && refresh {
            items.push(Button("Refresh", A::Refresh));
        }
        if view.has_url {
            items.push(Button("Open as Tab", A::OpenAsTab));
        }
        items
    };
```

- Image, Pdf, Json: `let mut items = url_items(true); items.extend(file_items()); …`
- Note: `url_items(false)` followed by Copy Text.
- Fill `has_url` wherever `NodeMenuView` is built from a node: `node.source_url().is_some()`.

Action dispatch:

```rust
            NodeAction::Refresh => self.refresh_node(id),
            NodeAction::OpenAsTab => self.open_as_tab(id),
```

The new `App` methods:

```rust
    /// Fetches a node's source URL again; `reload_fetched_node` swaps it
    /// in once it passes its check. Refresh, and a missing file.
    fn refresh_node(&mut self, id: &str) {
        use content::ContentKind as K;
        let Some(node) = self.nodes.get(id) else { return };
        let kind = match &node.payload {
            NodePayload::Image(_) => K::Image,
            NodePayload::Pdf(_) => K::Pdf,
            NodePayload::Json(j) if j.key.is_none() => K::Json,
            _ => return,
        };
        let Some(url) = node.source_url().map(str::to_string) else { return };
        let job = FetchJob { target: FetchTarget::Node(id.to_string()), node_id: id.to_string(), url, kind };
        self.redirect_fetcher.request(job);
    }

    /// Reloads node `id` from freshly fetched content: same id, position,
    /// group, color and draw order.
    fn reload_fetched_node(&mut self, id: &str, body: WebBody) {
        let Some(node) = self.nodes.get(id) else { return };
        let WebBody::File { .. } = &body else { return };
        let (pos, group, parent, color) = (node.pos, node.group.clone(), node.parent.clone(), node.color);
        let Some(url) = node.source_url().map(str::to_string) else { return };
        let at = self.node_order.iter().position(|n| n == id);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_image(id);
        }
        self.nodes.remove(id);
        self.node_order.retain(|n| n != id);
        if self.insert_content(ContentSource::Web { url, body }, id.to_string(), pos, group).is_ok() {
            if let Some(node) = self.nodes.get_mut(id) {
                (node.parent, node.color) = (parent, color);
            }
            if let (Some(at), Some(last)) = (at, self.node_order.pop()) {
                self.node_order.insert(at.min(self.node_order.len()), last);
            }
            self.save_session();
        }
    }
```

Before writing `reload_fetched_node`:
- Read `retry_missing_file` (~4069) to its end; it solves the same "reinsert in place" problem. Match how it restores order. If it frees the image texture differently (`free_image` vs something else), follow it.
- If an insert fails after the node was removed, the node is lost. Do what `retry_missing_file` does on failure. It keeps a placeholder, so mirror that: on `Err`, insert a placeholder of the same kind carrying the error, with `url` set.

```rust
    /// A tab at the node's source URL beside it, which stays a tab on its
    /// first load (`skip_convert`), e.g. to read a PDF past page 1.
    fn open_as_tab(&mut self, id: &str) {
        let Some(node) = self.nodes.get(id) else { return };
        let Some(url) = node.source_url().map(str::to_string) else { return };
        let (pos, group) = (node.pos + egui::vec2(40.0, 40.0), node.group.clone());
        let tab = self.spawn_tab_at(&url, pos, group);
        self.skip_convert.insert(tab);
    }
```

Add the field to `App`, with its `Default`:

```rust
    /// Tabs opened by "Open as tab": their first `RedirectableContent` or
    /// `JsonBody` report doesn't convert them. Left on a real url change
    /// or when the tab goes.
    skip_convert: HashSet<String>,
```

In `event_loop.rs`, at the top of both the `RedirectableContent` and `JsonBody` arms, after `JsonBody` has stored `json_body`:

```rust
                if self.skip_convert.remove(tab_id) {
                    return; // or `continue`/skip, matching the surrounding control flow
                }
```

Match the handler's actual control flow: if these arms sit inside a `match` in a function, wrap the conversion in `if !self.skip_convert.remove(tab_id) { … }` instead of returning.

- On a real URL change, where `node.json_body = None` is set (~1202): add `self.skip_convert.remove(tab_id);`.
- In `destroy_tab`: add `self.skip_convert.remove(id);`.

Drain arm: `FetchTarget::Node(id) => self.reload_fetched_node(&id, body),`

- [ ] **Step 4: Run the tests; they pass.**
  - Run: `cargo test -p manifold -j 12`, then clippy.
  - Expected: all pass.

- [ ] **Step 5: Live probe with `drive.sh`.**
  - Using the Task 4 probe files: after a `file://` PDF converts, right-click the card, choose Open as Tab, wait, then `EXPORTSTATE`.
  - Expected: a new tab at the `file://` URL that is still a tab.
  - Refresh: edit the source JSON file on disk (a `!python` step), Refresh the Json card, and check the card's value changed.

- [ ] **Step 6: Commit.**

```bash
cargo fmt --all
git add -A crates/manifold/src
git commit -m "Refresh and Open as Tab for nodes made from web content

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Missing files refetch; pruning at launch

**Files:**
- Modify:
  - `crates/manifold/src/session_sync.rs`: materialize for Image, Pdf and Json (~486–500)
  - `crates/manifold/src/main.rs`: `retry_missing_file` (~4069); a new `resolve_content_path`; a new `prune_content_dir`; call it at startup
  - `crates/manifold/src/event_loop.rs`: call prune after `restore_session` (~62)

**Interfaces:**
- Consumes: `refresh_node` (Task 5), `content::prune` (Task 1), `profile::content_dir` (Task 2)
- Produces:
  - `fn resolve_content_path(dir: Option<&Path>, id: &str, stored: &str) -> String` (a free function):
    - returns `stored` when that file exists
    - else the first `dir/<id>.*` that exists
    - else `stored`
  - `fn content_keep_names(&self) -> HashSet<String>`: file names to keep (ruling 5)

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn a_missing_content_file_resolves_to_this_machines_copy() {
    let dir = crate::store::testutil::temp_dir("resolve_content");
    std::fs::write(dir.join("5-9.png"), b"x").unwrap();
    let elsewhere = r"Z:\nowhere\5-9.png";
    assert_eq!(resolve_content_path(Some(&dir), "5-9", elsewhere), dir.join("5-9.png").to_string_lossy());
    assert_eq!(resolve_content_path(Some(&dir), "5-8", elsewhere), elsewhere);
    assert_eq!(resolve_content_path(None, "5-9", elsewhere), elsewhere);
}

#[test]
fn pruning_keeps_files_live_or_recently_closed_nodes_name() {
    let dir = crate::store::testutil::temp_dir("prune_content");
    let mut app = App::default();
    // A live node by path, a live node by id (a refetched copy), a closed
    // entry by its path field, and an orphan.
    for name in ["a.png", "1-7.json", "c.pdf", "orphan.png"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    /* insert an Image placeholder whose path is dir/a.png; a Json placeholder with id "1-7" and path Z:\\x.json;
       push a ClosedEntry whose doc has one node with fields {"path": dir/c.pdf} */
    let keep = app.content_keep_names();
    assert_eq!(crate::content::prune(&dir, |name| keep.contains(name)), 1);
    assert!(!dir.join("orphan.png").exists());
}
```

Fill in the placeholder comment with real inserts via `insert_placeholder_node` and a `ClosedEntry { doc: JsonDoc { nodes: vec![JsonNode { id, kind: "pdf".into(), fields }], local: None } }`, pushed to `app.closed` (check the field name).

- [ ] **Step 2: Run the tests; they fail.**
  - Run: `cargo test -p manifold -j 12 resolve_content pruning_keeps`
  - Expected: compile errors.

- [ ] **Step 3: Implement.**

```rust
/// Where node `id`'s file is on this machine: `stored` if it exists, else
/// a copy this machine fetched into the content dir (named by id, see
/// `content`), else `stored` (it shows as missing).
fn resolve_content_path(dir: Option<&std::path::Path>, id: &str, stored: &str) -> String {
    if std::path::Path::new(stored).exists() {
        return stored.to_string();
    }
    let prefix = format!("{id}.");
    dir.and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map_or_else(|| stored.to_string(), |e| e.path().to_string_lossy().into_owned())
}
```

In materialize, for Image, Pdf and Json:
- Resolve the path first: `let path = resolve_content_path(crate::profile::content_dir().as_deref(), &id.to_string(), &str_of(fields, field::PATH));`.
- After the node exists with its URL set (Task 2's block), if the resolved path still doesn't exist and the node has a URL, call `self.refresh_node(&id)`. The placeholder card shows until the fetch lands.
- Pdf: `insert_pdf_node_with_id` renders asynchronously and reports a missing file as `Some(Err(_))` later. Call `refresh_node` in the same case, so the successful fetch replaces the error card.

`retry_missing_file`: at the top, after the Unloaded case:

```rust
        if self.nodes.get(id).is_some_and(|n| n.source_url().is_some()) {
            self.refresh_node(id);
            return;
        }
```

`content_keep_names`:

```rust
    /// Content-dir file names something still uses: live nodes' files,
    /// files named by a live node's id (this machine's copy of a synced
    /// node), and Recently Closed entries' files.
    fn content_keep_names(&self) -> HashSet<String> {
        let name = |path: &str| std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned());
        let mut keep: HashSet<String> = HashSet::new();
        for (id, node) in &self.nodes {
            let path = match &node.payload {
                NodePayload::Image(p) => Some(&p.path),
                NodePayload::Pdf(p) => Some(&p.path),
                NodePayload::Json(p) => Some(&p.path),
                NodePayload::Download(p) => Some(&p.path),
                _ => None,
            };
            keep.extend(path.and_then(|p| name(p)));
            keep.insert(id.clone()); // matched by stem below
        }
        for entry in &self.closed {
            for node in &entry.doc.nodes {
                keep.extend(node.fields.get("path").and_then(|v| v.as_str()).and_then(name));
            }
        }
        keep
    }
```

`prune` gets the closure `|file| keep.contains(file) || file.split_once('.').is_some_and(|(stem, _)| keep.contains(stem))`. Build the closure in `prune_content_dir` so the test above exercises the same rule: move the stem rule into `content_keep_names`' caller, and have the test call `prune_content_dir(&dir)`, or share one `fn content_keeps(&self) -> impl Fn(&str) -> bool`. Pick one and use it in both places.

```rust
    /// Deletes content-dir files nothing uses (spec "Where fetched files
    /// live"), on a thread. Called once at launch, after restore.
    fn prune_content_dir(&self) {
        let Some(dir) = profile::content_dir() else { return };
        let keep = self.content_keep_names();
        std::thread::spawn(move || {
            let n = content::prune(&dir, |file| {
                keep.contains(file) || file.split_once('.').is_some_and(|(stem, _)| keep.contains(stem))
            });
            if n > 0 {
                eprintln!("content: pruned {n} unused file(s)");
            }
        });
    }
```

`event_loop.rs` ~62: after the `if !self.restore_session() { … }` block, add `self.prune_content_dir();`.

- [ ] **Step 4: Run the tests; they pass.**
  - Run: `cargo test -p manifold -j 12`, then clippy.
  - Expected: all pass.

- [ ] **Step 5: Live probe with `drive.sh`.** It isolates the profile, so the content dir is the probe's own.
  1. Convert a `file://` JSON as in Task 4.
  2. QUIT.
  3. Delete `<probe profile>\manifold\content\<id>.json` with a `!python` step. The drive script prints its profile dir; if it doesn't, read `scripts/drive.sh` for where it points `APPDATA`.
  4. Relaunch with the same profile.
  5. Check that the card comes back loaded and the file exists again.
  6. Also drop an orphan `junk.png` into the content dir before relaunch, and check it's gone after.

- [ ] **Step 6: Commit.**

```bash
cargo fmt --all
git add -A crates/manifold/src
git commit -m "Missing web-sourced files refetch; unused content files are pruned at launch

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Drag and drop

**Files:**
- Modify:
  - `crates/manifold/Cargo.toml`: `windows-sys` features gain `"Win32_UI_WindowsAndMessaging"` and `"Win32_Graphics_Gdi"`, if not already enabled
  - `crates/manifold/src/event_loop.rs`: the `WindowEvent::DroppedFile` arm (near `CursorMoved`, ~185); the `AddFile(path, Some(point))` arm
  - `crates/manifold/src/main.rs`: `drop_files_at`, `drop_batch`
  - `graph_ui.rs`: only if a public screen → world helper has to be exposed

**Interfaces:**
- Consumes: `add_file_at` and `insert_content` (Task 3)
- Produces: `fn drop_file(&mut self, path: PathBuf, point: egui::Pos2)`. `point` is in window points. Files dropped in one frame fan out: `drop_batch: u32` counts drops since the last frame and is reset in `render_frame`.

Placement:
- **Graph view:** the world position under `point`. Use the same screen → world transform the graph uses for a click. Grep `graph_ui.rs` for where a click position becomes a world position (e.g. the empty-canvas menu's `pending_new_node` position) and call that. Add `egui::vec2(24.0, 24.0) * drop_batch as f32` to the result. Group: `current_group`.
- **Tab view:** beside the focused tab: `focused.pos + egui::vec2(240.0, 0.0) + egui::vec2(24.0, 24.0) * drop_batch`, in the focused tab's group. With no focused tab, jitter around world center at the current level.

- [ ] **Step 1: Write the failing test.** It covers the placement math only; the OLE drop itself is live-only.

```rust
#[test]
fn several_drops_in_a_frame_fan_out() {
    let dir = crate::store::testutil::temp_dir("drop_fan");
    let mut app = App::default();
    let mut ids = Vec::new();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), b"x").unwrap();
        let before: HashSet<String> = app.nodes.keys().cloned().collect();
        app.drop_file(dir.join(name), egui::pos2(300.0, 300.0));
        ids.extend(app.nodes.keys().filter(|k| !before.contains(*k)).cloned());
    }
    let (a, b) = (app.nodes[&ids[0]].pos, app.nodes[&ids[1]].pos);
    assert_eq!(b - a, egui::vec2(24.0, 24.0));
}
```

If `App::default()` has no camera rect to map a point through, `drop_file`'s Graph branch must still produce finite positions. Assert the difference only, as above.

- [ ] **Step 2: Run the test; it fails.**
  - Run: `cargo test -p manifold -j 12 several_drops`
  - Expected: compile error.

- [ ] **Step 3: Implement.**

`event_loop.rs`, in `window_event`:

```rust
            // The page never receives the drop (spec "Drag and drop").
            // winit 0.30 sends no CursorMoved during an OLE drag, so the
            // drop point is read from the OS.
            WindowEvent::DroppedFile(path) => {
                let point = self.cursor_point().unwrap_or(self.last_pointer_point());
                self.drop_file(path, point);
                self.save_session();
                // request a redraw the way neighbouring arms do
            }
```

`cursor_point` (Windows only; `None` elsewhere). Physical pixels → points by dividing by `self.ppp()`:

```rust
    #[cfg(windows)]
    fn cursor_point(&self) -> Option<egui::Pos2> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::{Foundation::POINT, Graphics::Gdi::ScreenToClient, UI::WindowsAndMessaging::GetCursorPos};
        let RawWindowHandle::Win32(handle) = self.window.as_ref()?.window_handle().ok()?.as_raw() else { return None };
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: plain Win32 calls on our own window handle and a local POINT.
        let ok = unsafe { GetCursorPos(&mut p) != 0 && ScreenToClient(handle.hwnd.get() as _, &mut p) != 0 };
        let ppp = self.ppp() as f32;
        ok.then(|| egui::pos2(p.x as f32 / ppp, p.y as f32 / ppp))
    }
    #[cfg(not(windows))]
    fn cursor_point(&self) -> Option<egui::Pos2> { None }
```

Check how the codebase already gets the HWND (grep `RawWindowHandle::Win32`) and reuse that pattern. `last_pointer_point` is whatever field `CursorMoved` updates (~185); read it to name it.

`drop_file` in `main.rs`: compute the position as above, then `self.add_file_at(path, Some(pos))`. Use `insert_content` directly when the group differs from `current_group`, as in Tab view. Increment `drop_batch`. In `render_frame`, reset `self.drop_batch = 0`.

`ADDFILE` with a point:

```rust
            DebugCommand::AddFile(path, Some((x, y))) => {
                self.drop_file(path.into(), egui::pos2(x, y));
                self.save_session();
            }
```

Check the window: winit enables drag-and-drop on Windows by default (`WindowAttributes::with_drag_and_drop(true)`). Grep window creation to make sure nothing disables it.

- [ ] **Step 4: Run the tests, and add a smoke check; they pass.**
  - Run: `cargo test -p manifold -j 12`, then clippy.
  - Add a smoke test step that writes a `.txt` file and a `.json` file under the test's temp dir, sends `ADDFILE <txt> 400 300` and `ADDFILE <json> 400 300`, and checks with `check_state`:
    - a Note and a Json card exist
    - their screen centres are within 60 px of (400, 300)
    - they differ by the fan-out
  - Use the query helper's `xy`.
  - Run: `cargo build -p manifold -j 12 && rtk proxy bash scripts/headless-smoke-test.sh`
  - Expected: all checks pass, now more than 29.

- [ ] **Step 5: Live check (manual; ask the user).** Dropping a real file from Explorer onto the window can't be scripted. Ask the user to:
  1. drop a PNG onto Graph view
  2. drop two files at once
  3. drop a file onto Tab view

  Then report what they saw.

- [ ] **Step 6: Commit.**

```bash
cargo fmt --all
git add -A crates/manifold scripts/headless-smoke-test.sh
git commit -m "Files dropped on the window become nodes where they land

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Smoke coverage per kind, docs, spec outcome

**Files:**
- Modify:
  - `scripts/headless-smoke-test.sh`
  - `README.md`
  - `docs/superpowers/specs/2026-09-27-content-dispatch-design.md` (status line plus an Outcome section)

- [ ] **Step 1: Smoke checks for `ADDFILE` of each kind.**
  - Next to the existing `ADDFILE` image step, add PDF, JSON, `.md` note, `.html` (becomes a tab, `file://` URL) and `.bin` (a Download card).
  - Check each through `EXPORTSTATE` card kinds with `check_state`, using the existing helper style.
  - Generate the fixtures in the test's temp dir with `printf`. Don't commit binary fixtures; the PNG is the existing icon.
  - Run: `cargo build -p manifold -j 12 && rtk proxy bash scripts/headless-smoke-test.sh`
  - Expected: all pass. Run it 3 times, and report the CPU load alongside.

- [ ] **Step 2: README.**
  - Update the `ADDIMAGE`/`ADDPDF`/`ADDJSON` command docs to `ADDFILE <path> [x y]`, in the debug-command list.
  - Add a changelog-style entry in the README's existing style, like the neighbouring entries. It should cover:
    - the one MIME table
    - automatic conversion of JSON, PDF and text pages
    - Save as Node removed
    - drag and drop
    - Refresh and Open as Tab
    - the content dir and pruning
    - `path` made explicit
  - Leave historical entries that mention `ADDIMAGE` as they are; they describe the past.

- [ ] **Step 3: Spec.**
  - Change the status to `Status: implemented, 2026-09-27 (plan docs/superpowers/plans/2026-09-27-content-dispatch.md).`
  - Add an `## Outcome` section listing the planning rulings, what was live-verified per engine (from Tasks 4 and 5), and anything left open.

- [ ] **Step 4: Final checks.**
  - Run: `cargo fmt --all --check && cargo clippy -p manifold -p ipc-protocol --all-targets -j 12 -- -D warnings && cargo test -p manifold -j 12`
  - Expected: clean, all pass.

- [ ] **Step 5: Commit.**

```bash
git add README.md scripts/headless-smoke-test.sh docs/superpowers/specs/2026-09-27-content-dispatch-design.md
git commit -m "Content dispatch: smoke coverage per kind, docs, spec outcome

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage.** Each spec section and the task that covers it:

| Spec section | Covered by |
|---|---|
| Table | T1 |
| Checks | T1; used by T3 and T4 |
| Sniffing | T1 |
| One insert path | T3 |
| Web conversion | T4 (incl. `file://`, `JsonBody`, cookie-less refetch, tab replaced as before) |
| File picker | T3 |
| Drag and drop | T7 |
| Headless `ADDFILE` | T3, T7 |
| Save as Node removed | T4 |
| URL field | T2 |
| Refresh / Open as tab | T5 |
| Content dir | T2, T4 |
| Missing file with URL | T6 |
| Missing file without URL | unchanged, T6 keeps the path |
| Pruning | T6 |
| Old `%TEMP%` nodes keep working | nothing touches them |
| Testing section | T1, T3, T4, T6, T7, T8 |

**Gaps closed by rulings:** the path ping-pong across replicas (ruling 1) and the drop position (ruling 6).

**Known soft spots**, which the implementer verifies in the steps named:
- `spawn_tab_at` against `spawn_new_tab_with_kind`'s tail (T3)
- `reload_fetched_node`'s failure path against `retry_missing_file` (T5)
- test App setup, copied from neighbouring tests (T2–T6)
- whether each engine reports `RedirectableContent` for `file://` pages (T4 probe)
