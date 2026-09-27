# Content dispatch: one MIME table for every way content arrives

Status: approved design, 2026-09-27. Not yet implemented.

## Goal

A node's kind is picked from its content's MIME type, the same way no
matter how the content arrived: a page a tab navigated to, a file from
the picker, a file dropped on the window, or a headless command. Web
content converts automatically; nothing is opt-in any more (JSON's "Save
as Node" was). Converting keeps the source URL, so a node loses nothing
the tab offered.

## Today

- Web: every engine reports a finished page's `document.contentType`
  (`EngineToHost::RedirectableContent`, or `JsonBody` for
  `application/json`). `redirect_fetch::classify` maps only `image/*`;
  the host refetches the URL, checks the bytes decode, saves them to
  `%TEMP%\manifold-redirects\<tab id>.bin` and replaces the tab with an
  Image node (`redirect_tab_to_image`). JSON stays a tab with an opt-in
  "Save as Node" (`build_json_viewer`).
- Local files: `add_file_node` dispatches on the picked file's extension.
  Headless `ADDIMAGE`/`ADDPDF`/`ADDJSON` skip dispatch entirely.
- No OS drag-and-drop.
- Image/Pdf/Json nodes keep only a file path. Temp files are never
  cleaned up, and Windows may clear `%TEMP%` under them.

## The table

`content.rs` (new) owns one mapping from MIME type to what to make:

| MIME type | Makes |
|---|---|
| `image/*` | Image node |
| `application/pdf` | Pdf node |
| `application/json`, `*/*+json` | Json node |
| `text/plain` | Note (the text) |
| `text/html`, `application/xhtml+xml` | Tab (a local file opens at its `file://` URL; a web page stays the tab it is) |
| anything else | web: left to the engine's own download flow (unchanged); local file: a finished Download card for the file |

`ContentKind` names these outcomes. Adding a kind is one row here plus
its insert function.

Each outcome has a check the bytes must pass, or the content is not
converted (a web page stays a tab; a local file is refused with a toast):
an image must decode, a PDF must start with `%PDF-`, JSON must parse, a
note must be UTF-8 and at most 1 MB.

### Sniffing local files

A local file has no Content-Type, and its extension can lie or be
missing, so `content::sniff(bytes, file_name) -> mime` reads the first
bytes: `image::guess_format` for images, `%PDF-`, a JSON parse (only when
the text starts with `{` or `[`), `<!doctype html`/`<html` for HTML,
valid UTF-8 for plain text, otherwise `application/octet-stream`. The
extension breaks ties only between text types (`.md` and `.txt` are
plain text; `.json` that fails to parse is plain text, not refused).

Hand-written rather than a crate (surveyed 2026-09-27): `infer` and
`file-format` know hundreds of binary formats, all of which land in
"anything else" here, and `infer` doesn't do text, HTML or JSON at all.
`tree_magic_mini` needs freedesktop's database (absent on Windows) or its
GPL-licensed embedded copy. `mime_guess` is extension-only. The checks
above must decode or parse the bytes anyway, and `image` and
`serde_json` are already dependencies. Web pages need no sniffing: the
engine reports the type it already settled on.

## One insert path

`App::insert_content(source, pos, group)` takes a `ContentSource`:

- `Local(path)`: read, sniff, dispatch. The node points at the file where
  it is (no copy), as picked files do today.
- `Web { url, mime, bytes }`: dispatch on `mime`, check the bytes, save
  them under the profile (below), and make the node with `url` set.

Every entry point calls it:

- **Web conversion.** `RedirectableContent` classifies through the table.
  Images, PDFs, `+json` and text are fetched again by the host
  (`redirect_fetch`, as images are today), with no cookies: content
  behind a login fails its check and stays a tab. A `file://` URL is
  read from disk instead of fetched. `application/json`
  uses the body the engine already sends (`JsonBody`), so a logged-in
  API converts, with no second request. The tab is replaced as images
  are today (`redirect_tab_to_*` becomes one function): same position,
  parent and group; its children move to its parent; not added to
  Recently Closed.
- **File picker.** `add_file_node` keeps its filter list but dispatches
  through `insert_content` instead of the extension match.
- **Drag and drop (new).** `WindowEvent::DroppedFile`. In Graph view the
  files land at the pointer, fanned out slightly when several drop at
  once. In Tab view they land beside the focused tab. The page never
  receives the drop.
- **Headless.** One `ADDFILE <path> [x y]` replaces `ADDIMAGE`, `ADDPDF`
  and `ADDJSON` (scripts updated).

"Save as Node" goes away: a JSON page is already a node.

## Keeping the source

The store's existing `url` field (5) joins the fields of Image, Pdf,
Json and Note (`kind_fields`). No new field number; an older build
ignores the extra field on those kinds. A node made from the web has it
set; one from a local file leaves it empty.

A node with a URL gets two menu items:

- **Refresh** (Image, Pdf, Json): fetch the URL again, run the check,
  replace the file, reload the node. A Note doesn't get it: refreshing
  would overwrite edits.
- **Open as tab**: a tab at the URL beside the node, e.g. to read a PDF
  past page 1 in the engine's viewer. That tab skips conversion for its
  first load only (a flag on the tab, cleared once that load reports);
  navigating it away and back converts as usual.

## Where fetched files live

`profile::content_dir()` (`%APPDATA%\manifold\content`, `None` under
test like `favicon_cache_dir`) replaces `%TEMP%\manifold-redirects`.
Files are named `<node id>.<ext>` from the table.

- **Missing file, URL known:** refetched instead of showing an error. This
  also fills in a node synced from another replica, whose path was never
  on this machine.
- **Missing file, no URL:** today's missing-file card with Retry.
- **Pruning:** at launch, on a thread, delete files in the content dir
  that no live node and no Recently Closed entry names.

Existing nodes pointing into `%TEMP%\manifold-redirects` keep working
until that file goes; they have no URL, so they can't refetch.

## Out of scope

- Engines sending response bytes (would make logged-in images and PDFs
  convert; can follow without changing this design).
- Letting a web page receive file drops (upload fields).
- PDF page navigation inside the node.
- A dedicated File kind for other local files (a Download card serves).

## Testing

- `content` unit tests: the table (each row, `+json`, parameters like
  `; charset=`), `sniff` on each magic number, a lying extension, a
  missing extension, `.md`, broken JSON, binary junk.
- `insert_content` tests per kind with `temp_dir` files, including the
  check failing (tab kept / toast).
- Pruning test: referenced files kept, unreferenced ones deleted.
- Headless: `ADDFILE` for each kind in the smoke test; a drive.sh probe
  loading a local `file://` PDF and JSON page in a tab and checking it
  converts with `url` set (no network needed).
- Open as tab: the new tab stays a tab on its first load.
