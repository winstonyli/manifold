# Content dispatch: one MIME table for every way content arrives

Status: implemented, 2026-09-27 (plan docs/superpowers/plans/2026-09-27-content-dispatch.md).

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
  that no live node and no Recently Closed entry names. Files a named
  session names are kept too, and so is anything modified in the last
  7 days; exports outside the profile can't be tracked.

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

## Outcome

Built across eight tasks (commits `2fdfdc7..2152826` on `dev`, this task
adds the last commit). Rulings made along the way, decision only:

Planning rulings:

1. `path` is an explicit field: a replica that refetches a missing file
   saves it under its own content dir, and an implicit field would write
   that machine-local path back into the shared store.
2. Web text converts into a Note holding the text directly; no file is
   saved for it, since Refresh isn't offered for notes anyway.
3. "Open as tab" tracks skipped conversion by tab id in `App::skip_convert`,
   not a field on the tab payload, since the tab's node doesn't exist yet
   at spawn time. An id stays set until a real url change after the first load settles, or
   the tab closes; every content report meanwhile is skipped.
4. When both `JsonBody` and a text redirect arrive for one load, the JSON
   path converts and removes the tab first; the text path's existing
   `still_current` check quietly no-ops on the now-missing tab.
5. Pruning keeps a content-dir file whose name matches a live node's path
   (or a Recently Closed entry's), or whose stem matches a live node id —
   the id form is what lets a replica's own refetch of another machine's
   file survive.
6. A dropped file's position comes from `GetCursorPos`/`ScreenToClient`,
   not `CursorMoved` (winit sends none during an OLE drag); `ADDFILE <path>
   x y` reuses the same placement code so headless tests cover it.

Per-task rulings:

- Task 6's file-keep rule lives in one shared function so pruning and its
  test can't drift apart.
- `mod content;` sits alphabetically among `main.rs`'s other `mod` lines,
  not next to `mod redirect_fetch` as the plan's looser wording suggested.
- Two early commits (Tasks 2 and 5) carry the subagent harness's own
  "Claude Sonnet 5" trailer instead of "Claude Opus 5.5"; left as-is
  rather than rewriting already-made history, since it still names the
  model that wrote the code.
- Images decode by content (`image::load_from_memory`), not by
  `image::open`'s extension-based guess, so a lying extension can't hide
  a real image from the missing-file/retry path.
- Converting a tab reuses `remove_tab` (the same primitive a close or a
  remote deletion uses) instead of hand-copying `redirect_tab_to_image`'s
  old body, so a converted tab clears every guard (dragging, hovered,
  focus, split, pending respawn) a real close already clears.
- `skip_convert` stays set until a real url change after the first load
  settles, or the tab closes; every content report meanwhile is
  skipped. Remote loads use it too, so only the instance that navigated
  a shared tab converts it.
- A PDF that fails to render is never refreshed automatically; the
  brief's mandated removal of that retry loop is covered instead by the
  existing missing-file and restore paths.
- A missing file that still has a URL is refetched silently — no
  "Couldn't open" toast and no failed render first — inserting the
  placeholder directly and refreshing it.
- Pruning skips any file modified at or after its keep-set snapshot, so a
  file another running replica just fetched can't be deleted out from
  under it.
- The live Explorer drag-and-drop check (Task 7's Step 5) is left for the
  user to try, not attempted by any task's own implementer.
- The smoke test asserts each dropped card lands within 60 px of the
  point it was dropped at; it only asserts two same-frame drops fan out
  beside each other when they actually land in one rendered frame, and
  back-to-back headless `ADDFILE`s never reliably do, so that assertion
  was dropped from the smoke test and is covered by its unit test
  instead.

Live-verified per engine (Tasks 4 and 5, via `scripts/drive.sh` seeding a
`file://` tab): WebView2 and CEF both convert JSON, PDF, PNG and plain
text pages into the right node kind, keeping the source URL. On
WebView2, Refresh re-fetches and swaps a Json node's stored bytes in
place, and Open as Tab adds a second, unconverted tab beside the
original node rather than replacing it. Task 6's refetch-on-missing-file
and pruning were live-verified together: deleting a Json node's content
file and dropping an unrelated file into the content dir, then
restarting, refetched the missing file byte-identical and removed the
unrelated one. Task 7's headless `ADDFILE` drops were verified through
the smoke test (this task extends it to every kind); the real OLE
drag-and-drop path was not.

A final fix wave made pruning conservative (named sessions and a 7-day
grace kept, nothing pruned after a store recovery, a Refresh keeps an old
file another node names), gave failed conversions and refetches the
missing-file card, made two instances converting one shared tab add one
node (`scripts/two-instance-test.sh` scenario 5), capped `file://` reads,
saved fetches atomically, and added UNC `file://` paths, a BOM-tolerant
JSON load and a smoke step for engine-side conversion.

Left open:

- Servo and WebKit conversion over `file://` is untested — their
  engine-hosts didn't spawn in this checkout.
- `skip_convert` isn't persisted, so an Open-as-Tab tab converts back to
  a card on its first load after a restart.
- The live Explorer drag-and-drop still needs the user's own check.
- Content files named only by an export outside the profile can't be
  tracked, so pruning may delete them after the grace period.
- Reopening a closed node re-ids it, so a replica's `<old id>.<ext>`
  content file is orphaned and refetched rather than reused (no data
  loss).
