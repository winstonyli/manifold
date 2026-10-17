---
status: done
---

# `manifold://` Custom URL Scheme Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Register and serve a real `manifold://` URL scheme in the WebView2 engine-host, starting with one page, `manifold://welcome`, containing a real feature-tour of the app.

**Architecture:** All new code lives in one new file, `crates/manifold-engine-webview2/src/scheme.rs`, matching this crate's existing one-file-per-concern layout (`audio.rs`, `download.rs`, `find.rs`, `nav_state.rs`, `popup.rs`). It exposes two functions: one used by `create_environment()` to build environment options with the `"manifold"` scheme registered (`ICoreWebView2CustomSchemeRegistration` via `CreateCoreWebView2EnvironmentWithOptions`), and one, `scheme::register(&environment, &webview)`, called at webview setup time (alongside the existing `nav_state::register_handlers`/`find::register`/etc. calls) that adds a `WebResourceRequested` handler serving the embedded page content. `manifold.exe` (the host) needs zero changes — confirmed in the design spec that `manifold://welcome` already flows through `resolve_navigation_url`/`looks_like_url` unchanged, since any string containing `"://"` is already treated as a literal URL.

**Tech Stack:** Rust, `webview2-com = "0.39.1"` (this workspace's pinned version — every API surface referenced below was verified by reading that exact crate version's source directly, not assumed from documentation), `windows = "0.62"` (the `Win32_UI_Shell` feature this needs for `SHCreateMemStream` is already enabled in this crate's `Cargo.toml`, so no dependency changes are needed).

**Spec:** `docs/superpowers/specs/2026-09-19-manifold-scheme-design.md`

## Global Constraints

- WebView2 only. No changes to `manifold-engine-cef`/`manifold-engine-servo`/`manifold-engine-webkit`, and no changes to `manifold`/`ipc-protocol`.
- Static, compiled-in pages only (`include_str!`) — no new IPC message, no dynamic content from the host process.
- No new `Cargo.toml` dependency or feature changes (everything needed is already available).
- This project has no unit-test framework and no headless debug-command hook into WebView2-internal rendering — verification for both tasks below is a real, non-headless launch (per the spec's own Testing section), not the usual headless `DebugCommand` protocol.

---

### Task 1: Register the `manifold://` scheme and serve a minimal placeholder for `manifold://welcome`

**Files:**
- Create: `crates/manifold-engine-webview2/src/scheme.rs`
- Modify: `crates/manifold-engine-webview2/src/main.rs` (add `mod scheme;`, modify `create_environment()` at line ~372, add a `scheme::register(&environment, &webview)` call alongside the existing `nav_state`/`find`/`audio`/`download` registration calls at ~line 126-141)

**Interfaces:**
- Produces: `scheme::environment_options() -> windows_core::Result<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2EnvironmentOptions>` — used only by `create_environment()`.
- Produces: `scheme::register(environment: &ICoreWebView2Environment, webview: &ICoreWebView2) -> windows_core::Result<()>` — used only at webview setup time, same call shape as `find::register`/`download::register` (both already take `&webview` or `&environment, &webview`).

- [ ] **Step 1: Create `scheme.rs` with the environment-options builder and the page-serving registration**

Create `crates/manifold-engine-webview2/src/scheme.rs`:

```rust
//! The `manifold://` custom URL scheme -- registered at environment
//! creation via `environment_options()`, then served via `register()`'s
//! `WebResourceRequested` handler. Both are called from `main.rs` (see
//! `create_environment`/the webview-setup block) since scheme registration
//! is genuinely split across two different WebView2 lifecycle points: the
//! scheme name itself must be known before the environment is created, but
//! actually serving requests needs a live `ICoreWebView2` to attach the
//! event handler to. See the design doc
//! (`docs/superpowers/specs/2026-09-19-manifold-scheme-design.md`) for why
//! this is a real custom scheme (not an `https://` virtual host) and why
//! pages are compiled in rather than supplied dynamically by `manifold.exe`.

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Environment, ICoreWebView2CustomSchemeRegistration,
    ICoreWebView2EnvironmentOptions, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
};
use webview2_com::{CoreWebView2CustomSchemeRegistration, CoreWebView2EnvironmentOptions, WebResourceRequestedEventHandler};
use windows::core::HSTRING;
use windows::Win32::UI::Shell::SHCreateMemStream;

/// This scheme's name, without the `://` -- matches the design spec's own
/// `manifold://` naming. `HasAuthorityComponent` (set below) means a URL
/// like `manifold://welcome` parses with `host = "welcome"`, the same
/// shape `http`/`https` URLs already have.
const SCHEME_NAME: &str = "manifold";

/// Placeholder content for Task 1 -- proves the whole pipe (environment
/// registration -> WebResourceRequested -> a real page rendering) works
/// end to end before Task 2 replaces this with the real, designed
/// feature-tour content.
const WELCOME_HTML_PLACEHOLDER: &str = "<!doctype html><html><body style=\"background:#1E1F24;color:#E6E7EB;font-family:sans-serif;padding:2em;\"><h1>manifold://welcome</h1><p>placeholder -- Task 2 replaces this.</p></body></html>";

/// Builds the `ICoreWebView2EnvironmentOptions` that `create_environment`
/// passes to `CreateCoreWebView2EnvironmentWithOptions`, with the
/// `"manifold"` custom scheme registered on it. Called once, before the
/// environment exists.
pub fn environment_options() -> windows_core::Result<ICoreWebView2EnvironmentOptions> {
    let scheme_registration = CoreWebView2CustomSchemeRegistration::new(SCHEME_NAME.to_string());
    unsafe {
        scheme_registration.set_has_authority_component(true);
        // Local, static content -- no network trust boundary to actually
        // secure, but this avoids WebView2 flagging the page as insecure/
        // mixed content the way a real custom scheme without this flag
        // otherwise would.
        scheme_registration.set_treat_as_secure(true);
    }
    let scheme_registration: ICoreWebView2CustomSchemeRegistration = scheme_registration.into();

    let options = CoreWebView2EnvironmentOptions::default();
    unsafe {
        options.set_scheme_registrations(vec![Some(scheme_registration)]);
    }
    Ok(options.into())
}

/// Attaches the `WebResourceRequested` handler that actually serves
/// `manifold://` pages. Called once per tab, right after the webview is
/// created -- same call shape as `find::register`/`download::register`
/// elsewhere in this crate. Best-effort: if this fails (e.g. an older
/// WebView2 Runtime missing the interface Task 1's environment-options
/// path needs), `manifold://` pages simply won't render for this tab,
/// same soft-degradation posture `find::register`'s own caller already
/// gives its own best-effort registration.
pub fn register(
    environment: &ICoreWebView2Environment,
    webview: &ICoreWebView2,
) -> windows_core::Result<()> {
    unsafe {
        webview.AddWebResourceRequestedFilter(
            &HSTRING::from(format!("{SCHEME_NAME}://*")),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;
    }

    let environment = environment.clone();
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else { return Ok(()) };
        let Ok(request) = (unsafe { args.Request() }) else { return Ok(()) };
        let mut uri = windows::core::PWSTR::null();
        if unsafe { request.Uri(&mut uri) }.is_err() {
            return Ok(());
        }
        let uri = unsafe { crate::nav_state::take_pwstr(uri) };

        let page = page_name(&uri).and_then(page_content);
        let (status, reason, body) = match page {
            Some(html) => (200, "OK", html),
            None => (404, "Not Found", "<!doctype html><body>Not found</body></html>"),
        };
        let Some(stream) = (unsafe { SHCreateMemStream(Some(body.as_bytes())) }) else {
            return Ok(());
        };
        if let Ok(response) = unsafe {
            environment.CreateWebResourceResponse(
                &stream,
                status,
                &HSTRING::from(reason),
                &HSTRING::from("Content-Type: text/html"),
            )
        } {
            let _ = unsafe { args.SetResponse(&response) };
        }
        Ok(())
    }));
    let mut token = Default::default();
    unsafe {
        webview.add_WebResourceRequested(&handler, &mut token)?;
    }
    Ok(())
}

/// Extracts the host component from a `manifold://<host>...` URI (`None`
/// if it doesn't even match the scheme) -- same
/// `split(['/', '?', '#']).next()` shape `crates/manifold/src/suggestions.rs`'s
/// own `looks_like_url` already uses to isolate a URL's host part.
fn page_name(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix(&format!("{SCHEME_NAME}://"))?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or(rest))
}

/// The static page router. One `match` arm per known page -- no registry
/// abstraction for what is, for now, a single page (see the design spec's
/// own reasoning for why this is deliberately not over-built).
fn page_content(page: &str) -> Option<&'static str> {
    match page {
        "welcome" => Some(WELCOME_HTML_PLACEHOLDER),
        _ => None,
    }
}
```

- [ ] **Step 2: Check whether `take_pwstr` is `pub` or `pub(crate)` in `nav_state.rs`, and adjust visibility if needed**

`scheme.rs` calls `crate::nav_state::take_pwstr` (the same PWSTR-to-String helper `nav_state.rs`'s own `NavigationStartingEventHandler` closure already uses — see `crates/manifold-engine-webview2/src/nav_state.rs` around its `NavigationStartingEventHandler::create` call). Open `nav_state.rs`, find the `fn take_pwstr` definition, and if it is not already `pub(crate)` or `pub`, change its visibility to `pub(crate)` (do not make it `pub` at the crate's public API boundary if this crate has one — `pub(crate)` is enough for a sibling module in the same crate).

- [ ] **Step 3: Wire `scheme.rs` into `main.rs`**

In `crates/manifold-engine-webview2/src/main.rs`, find the `mod` declarations (~line 9-16):

```rust
mod audio;
mod capture;
mod download;
mod find;
mod input;
mod nav_state;
mod popup;
mod window;
```

Replace with:

```rust
mod audio;
mod capture;
mod download;
mod find;
mod input;
mod nav_state;
mod popup;
mod scheme;
mod window;
```

Find `create_environment` (~line 372-389):

```rust
fn create_environment() -> webview2_com::Result<ICoreWebView2Environment> {
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(|handler| unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::CreateCoreWebView2Environment(&handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            tx.send(environment.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send environment over mpsc channel");
            Ok(())
        }),
    )?;
    Ok(webview2_com::wait_with_pump(rx)??)
}
```

Replace with:

```rust
fn create_environment() -> webview2_com::Result<ICoreWebView2Environment> {
    let options = scheme::environment_options().map_err(webview2_com::Error::WindowsError)?;
    let (tx, rx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            webview2_com::Microsoft::Web::WebView2::Win32::CreateCoreWebView2EnvironmentWithOptions(
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR::null(),
                &options,
                &handler,
            )
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            tx.send(environment.ok_or_else(|| {
                windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
            }))
            .expect("send environment over mpsc channel");
            Ok(())
        }),
    )?;
    Ok(webview2_com::wait_with_pump(rx)??)
}
```

Find the existing best-effort registration calls (~line 138-141):

```rust
    let find_state = find::register(&environment, &webview).ok();
    let pending_audio_playing = audio::register(&webview).ok();
    let pending_context_menu: input::PendingContextMenu = Default::default();
    let pending_download = download::register(&webview).ok();
```

Replace with:

```rust
    let find_state = find::register(&environment, &webview).ok();
    let pending_audio_playing = audio::register(&webview).ok();
    let pending_context_menu: input::PendingContextMenu = Default::default();
    let pending_download = download::register(&webview).ok();
    let _ = scheme::register(&environment, &webview);
```

- [ ] **Step 4: Build**

```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold-engine-webview2 2>&1 | tail -60
```

This is genuinely unfamiliar COM/FFI territory even though every API used above was verified by reading `webview2-com = "0.39.1"`'s actual source (not assumed from docs) — expect to spend real time here on type mismatches the borrow/type checker catches that weren't visible from reading the source alone (e.g. exactly which `Param<T>` impls exist for `&T` vs `T`, whether `.into()` needs an explicit target type annotation at each conversion). Iterate until it's a clean build. If a specific API genuinely doesn't exist or doesn't behave as this step describes, stop and report BLOCKED with what you found — don't guess further into a workaround.

Confirm zero warnings too:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold-engine-webview2 2>&1 | grep -i warning
```

- [ ] **Step 5: Build the whole workspace's `manifold` binary too (it depends on the engine-host being buildable, and this is what you'll actually launch to test)**

```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold -p manifold-engine-webview2 2>&1 | tail -60
```

- [ ] **Step 6: Live-verify with a real, non-headless launch**

This project has no headless hook into WebView2-internal rendering, so this genuinely needs the real app, not the usual headless `DebugCommand` protocol. Back up `%APPDATA%/manifold/session.json`/`settings.json` first (same convention as every other test in this project), write a session with one tab already pointed at `manifold://welcome`:

```bash
cd "/c/Users/Winston Li/Documents/GitHub/manifold"
cp "$APPDATA/manifold/session.json" "$APPDATA/manifold/session.json.bak"
cp "$APPDATA/manifold/settings.json" "$APPDATA/manifold/settings.json.bak"
cat > "$APPDATA/manifold/session.json" <<'EOF'
{
  "tabs": [{"id": "tab-0", "parent": null, "url": "manifold://welcome", "engine": "webview2", "pos_x": 0.0, "pos_y": 0.0}],
  "notes": [], "images": [], "pdfs": [], "jsons": [],
  "focused": "tab-0", "view_mode_tab": true,
  "next_tab_id": 1, "next_note_id": 0, "next_image_id": 0, "next_pdf_id": 0, "next_json_id": 0
}
EOF
taskkill //F //IM manifold.exe //T 2>/dev/null
taskkill //F //IM manifold-engine-webview2.exe //T 2>/dev/null
```

Then launch the REAL (non-headless) app: `./target/debug/manifold.exe` (no `--headless` flag). Take a real screenshot of the actual on-screen window (via the `System.Drawing`/`System.Windows.Forms` PowerShell screen-capture approach already used earlier in this project's own history for verifying the taskbar icon, since this needs to see actual WebView2-rendered pixels, not this app's own internal `SCREENSHOT` debug command which captures its own render target and may not apply the same way here — check whichever capture approach actually gets you real on-screen pixels). Confirm the placeholder page ("manifold://welcome" heading, "placeholder -- Task 2 replaces this.") renders. Also manually type `manifold://nonexistent` into the address bar and confirm a 404 page renders instead of a generic browser error page (proving the router/404 path works, not just the happy path).

Quit the app, restore the session/settings backups, kill stray processes:
```bash
mv "$APPDATA/manifold/session.json.bak" "$APPDATA/manifold/session.json"
mv "$APPDATA/manifold/settings.json.bak" "$APPDATA/manifold/settings.json"
taskkill //F //IM manifold.exe //T 2>/dev/null
taskkill //F //IM manifold-engine-webview2.exe //T 2>/dev/null
```

If real on-screen screenshot capture proves unworkable after a reasonable effort, report DONE_WITH_CONCERNS rather than spending excessive time on it — state plainly what you tried and fall back to describing what you directly observed (e.g. via `read_console_messages`-equivalent logging you add temporarily, or the tab's title-bar text if `manifold://welcome`'s `<title>` is set to something distinctive) as the next-best evidence.

- [ ] **Step 7: Commit**

```bash
git add crates/manifold-engine-webview2/src/scheme.rs crates/manifold-engine-webview2/src/main.rs crates/manifold-engine-webview2/src/nav_state.rs
git commit -m "$(cat <<'EOF'
Register manifold:// custom scheme, serve a placeholder welcome page

CoreWebView2CustomSchemeRegistration + WebResourceRequested, per the
design doc -- a real custom scheme, not an https virtual host. manifold.exe
needs no changes: manifold://welcome already flows through the existing
resolve_navigation_url/looks_like_url address-bar logic unchanged. One
placeholder page for now; a later commit replaces it with real content.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Replace the placeholder with the real `manifold://welcome` feature-tour content

**Files:**
- Modify: `crates/manifold-engine-webview2/src/scheme.rs` (replace `WELCOME_HTML_PLACEHOLDER` with real content, loaded via `include_str!` from a new asset file)
- Create: `crates/manifold-engine-webview2/assets/welcome.html`

**Interfaces:**
- Consumes: `page_content`/`page_name` from Task 1, unchanged in shape — only the HTML string itself changes.

- [ ] **Step 1: Write the real welcome page content**

Create `crates/manifold-engine-webview2/assets/welcome.html`:

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Welcome to Manifold</title>
<style>
  :root {
    color-scheme: dark;
  }
  body {
    margin: 0;
    background: #1E1F24;
    color: #E6E7EB;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    padding: 3em 2em;
    max-width: 720px;
    margin: 0 auto;
  }
  h1 {
    color: #58A6FF;
    font-size: 2em;
    margin-bottom: 0.1em;
  }
  .tagline {
    color: #9497A1;
    font-size: 1.1em;
    margin-top: 0;
    margin-bottom: 2em;
  }
  .card {
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 8px;
    padding: 1.2em 1.5em;
    margin-bottom: 1em;
  }
  .card h2 {
    color: #E6E7EB;
    font-size: 1.15em;
    margin-top: 0;
    margin-bottom: 0.4em;
  }
  .card p {
    color: #9497A1;
    margin: 0;
    line-height: 1.5;
  }
</style>
</head>
<body>
  <h1>Manifold</h1>
  <p class="tagline">Your tabs, as a graph — not a strip.</p>

  <div class="card">
    <h2>Graph, not a strip</h2>
    <p>Tabs are nodes you can drag, group, and connect — not a shrinking row
    of icons. Opening a link from a tab draws an edge to its parent, so your
    browsing history is a visible tree, not a stack you have to remember.</p>
  </div>

  <div class="card">
    <h2>Groups, nested as deep as you need</h2>
    <p>Drag tabs into a group to collapse them into one card; drag a group
    onto another to merge them. Color-tag anything for at-a-glance
    organization.</p>
  </div>

  <div class="card">
    <h2>Four engines, one browser</h2>
    <p>Every tab can run on Servo, CEF, WebKit, or WebView2 — pick per tab,
    or open "Split" to compare the same page across two engines side by
    side.</p>
  </div>

  <div class="card">
    <h2>Not just tabs</h2>
    <p>Notes, images, PDFs, and JSON files live on the same graph as your
    tabs — drag one into a group with related tabs instead of hunting
    through a separate downloads list.</p>
  </div>

  <div class="card">
    <h2>Try it: expand a JSON file into the graph</h2>
    <p>Right-click any JSON node → "Expand as Graph" to explode its
    structure into real, draggable, nested nodes instead of a flat popup
    viewer.</p>
  </div>
</body>
</html>
```

- [ ] **Step 2: Embed it at compile time**

In `crates/manifold-engine-webview2/src/scheme.rs`, find:

```rust
/// Placeholder content for Task 1 -- proves the whole pipe (environment
/// registration -> WebResourceRequested -> a real page rendering) works
/// end to end before Task 2 replaces this with the real, designed
/// feature-tour content.
const WELCOME_HTML_PLACEHOLDER: &str = "<!doctype html><html><body style=\"background:#1E1F24;color:#E6E7EB;font-family:sans-serif;padding:2em;\"><h1>manifold://welcome</h1><p>placeholder -- Task 2 replaces this.</p></body></html>";
```

Replace with:

```rust
/// The real `manifold://welcome` page -- a feature tour, embedded at
/// compile time (same `include_str!`/`include_bytes!` convention this
/// crate's own icon/font assets already use elsewhere in this workspace).
const WELCOME_HTML: &str = include_str!("../assets/welcome.html");
```

Find the `page_content` function's match arm:

```rust
        "welcome" => Some(WELCOME_HTML_PLACEHOLDER),
```

Replace with:

```rust
        "welcome" => Some(WELCOME_HTML),
```

- [ ] **Step 3: Build**

```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold-engine-webview2 -p manifold 2>&1 | tail -60
```
Then confirm zero warnings:
```bash
PATH=$(echo "$PATH" | tr ':' '\n' | grep -v -F "depot_tools" | tr '\n' ':') rtk proxy cargo build -p manifold-engine-webview2 -p manifold 2>&1 | grep -i warning
```

- [ ] **Step 4: Live-verify with a real, non-headless launch**

Same procedure as Task 1's Step 6 (backup session/settings, write a session with a tab at `manifold://welcome`, launch the real non-headless app, screenshot). Confirm: the real feature-tour content renders (all 5 cards, correct copy), the styling matches the app's own dark chrome (background/accent/text colors), and the page is legible at the default window size (no obvious overflow/clipping). Restore session/settings backups and kill stray processes afterward, same as Task 1.

- [ ] **Step 5: Commit**

```bash
git add crates/manifold-engine-webview2/src/scheme.rs crates/manifold-engine-webview2/assets/welcome.html
git commit -m "$(cat <<'EOF'
Write the real manifold://welcome feature-tour content

Replaces Task 1's placeholder with the designed page: graph view,
nested groups, multi-engine + Split compare, non-tab node kinds, and
a callout for "Expand as Graph" -- matching the design doc's own
content outline and this app's dark chrome palette.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

## Post-implementation: update README

After both tasks are committed, add one README.md entry (matching this project's own established changelog convention) describing the `manifold://` scheme, why a real custom scheme was chosen over an `https://` virtual host, the WebView2-only scope, and how it was live-verified (a real non-headless launch, since this is one of the rare changes with no headless verification path). Controller-owned wrap-up, same as the JSON "Expand as Graph" plan's own README step — not its own task.
