---
status: done
---

# `manifold://` Custom URL Scheme — Design

## Motivation

This app's own launch experience today is a single blank tab pointed at
`https://example.com` — nothing about it teaches the graph-based mental
model that's the whole point of the app. The eventual goal (a separate,
later piece of work) is a first-launch "demo session" that actually shows
off groups/notes/multi-engine/JSON-explosion by example. That demo needs
somewhere to point a tab that doesn't depend on network access on first
launch. This spec covers only the prerequisite: a `manifold://` internal
URL scheme, proven out with one real page (`manifold://welcome`). The demo
session itself is out of scope here.

## Scope decisions (confirmed during brainstorming)

- **WebView2 only, for now.** This project already carries uneven
  per-engine maturity (Servo's build is deliberately deferred, WebKit needs
  a separate WSL2 build to even verify) — `manifold://` ships for the
  default engine first; CEF/Servo/WebKit are explicit follow-up work, not
  part of this spec. Navigating a `manifold://` URL on one of those three
  today will simply fail to load (no scheme registered there) — an
  accepted, disclosed limitation, not a bug to guard against here.
  (Update 2026-10-02: the first-launch demo, whose tabs are all `manifold://` pages, now always opens in WebView2 whatever the default engine, and where WebView2 is unavailable (Linux, macOS) a first launch opens a normal root tab instead of a demo whose pages would fail to load.)
- **A real custom scheme (`manifold://welcome`), not an `https://` virtual
  host.** WebView2 supports both: `SetVirtualHostNameToFolderMapping` is
  simpler but produces an `https://`-look-alike hostname, not a literal
  custom scheme. `ICoreWebView2EnvironmentOptions4::
  SetCustomSchemeRegistrations` + a `WebResourceRequested` handler is the
  documented, supported path to an actual `manifold://` scheme, and the
  `webview2-com` crate this project already depends on wraps it (see
  [WebResourceRequested-CustomScheme.md](https://github.com/MicrosoftEdge/WebView2Feedback/blob/main/specs/WebResourceRequested-CustomScheme.md)).
- **A static, compiled-in page set**, not a dynamic/IPC-driven one. Pages
  are authored as HTML files under `manifold-engine-webview2`'s own
  `assets/` directory and embedded via `include_str!` at compile time —
  the same convention `manifold`'s own window icon/font already use.
  `manifold.exe` (the host) needs zero new IPC messages and zero awareness
  of the scheme: a `manifold://` URL flows through the exact same
  `HostToEngine::Navigate { url, .. }` message and the exact same
  `resolve_navigation_url`/`looks_like_url` address-bar logic every other
  URL already does (confirmed: `looks_like_url` treats any string
  containing `"://"` as a URL, not a search query, with no scheme
  allowlist). Adding, editing, or removing a `manifold://` page only ever
  touches `manifold-engine-webview2`.
- **One real page for this spec: `manifold://welcome`.** A short feature
  tour (see Content below), not a placeholder. No page-registry
  abstraction is introduced for a single page — the router is a plain
  `match` over the request's host component, naturally extensible later
  without needing to build out infrastructure ahead of need.

## Architecture

Two touch points, both inside `crates/manifold-engine-webview2/src/main.rs`:

1. **Environment creation** (`create_environment`, currently calling the
   plain `CreateCoreWebView2Environment`) switches to
   `CreateCoreWebView2EnvironmentWithOptions`, passing environment options
   configured with one `CoreWebView2CustomSchemeRegistration` for the
   scheme name `"manifold"`:
   - `HasAuthorityComponent: true` — so `manifold://welcome` parses with
     `host = "welcome"`, matching how `http`/`https` URLs already parse
     (`scheme://host/path`), rather than an opaque `scheme:opaque-data`
     shape.
   - `TreatAsSecure: true` — so the page isn't flagged as insecure/mixed
     content by the engine (it's fully local, static content; there's
     nothing to secure against, but WebView2 still applies its own
     content-security posture based on this flag).
2. **After the `ICoreWebView2` is created** (same file, right after the
   controller/webview setup this crate already has), register a
   `WebResourceRequested` event handler, filtered to `manifold://*` via
   `AddWebResourceRequestedFilter`. The handler:
   - Parses the request's URI, extracts the host component.
   - Matches it against the small, static set of embedded pages (for this
     spec: just `"welcome"`).
   - Returns a `CoreWebView2WebResourceResponse` — 200 with the matching
     page's bytes and a `Content-Type: text/html` header for a known page,
     or 404 for anything else.

Exact `webview2-com` type/method names (`CoreWebView2EnvironmentOptions4`
vs. a differently-versioned interface, the exact `WebResourceRequested`
handler signature, how `CoreWebView2WebResourceResponse` gets constructed
from raw bytes) are an implementation-plan concern, not a design one — the
plan that follows this spec should verify them directly against the
`webview2-com = "0.39.1"` version this workspace is pinned to (its own
`sample.rs` in the [wravery/webview2-rs](https://github.com/wravery/webview2-rs)
repo is the reference implementation for this exact pattern) rather than
assuming names from documentation for a possibly-different interface
version.

One specific unknown left for the plan to confirm empirically rather than
assume: `CoreWebView2CustomSchemeRegistration` also exposes an
`AllowedOrigins` list, documented as controlling which origins may issue
requests *with* the custom scheme (cross-origin fetch/XHR *into*
`manifold://` from another page) — this should have no bearing on a plain
top-level navigation (typing the URL, or `Navigate{url}`) actually
loading, which is all this spec needs. The plan should verify a top-level
navigation to `manifold://welcome` works with `AllowedOrigins` left at its
default/empty value before adding any origin-allowlist logic that isn't
needed.

## Content: `manifold://welcome`

A single static HTML file, `crates/manifold-engine-webview2/assets/
welcome.html`, inline `<style>` only (no external CSS/JS/fonts, matching
the offline-first requirement this whole feature exists to satisfy). Dark
theme matching the app's own chrome: background `#1E1F24` (`TOOLBAR_FILL`),
accent `#58A6FF` (`ACCENT`), primary text `#E6E7EB` (`TEXT_PRIMARY`), weak/
secondary text `#9497A1` (`TEXT_WEAK`) — the same four constants
`crates/manifold/src/main.rs` already defines for its own egui chrome.

Sections, in order:

1. **Header.** "Manifold" + tagline: "Your tabs, as a graph — not a strip."
2. **Graph, not a strip.** Tabs are nodes you can drag, group, and
   connect — not a shrinking row of icons. Opening a link from a tab draws
   an edge to its parent, so your browsing history is a visible tree, not
   a stack you have to remember.
3. **Groups, nested as deep as you need.** Drag tabs into a group to
   collapse them into one card; drag a group onto another to merge them.
   Color-tag anything for at-a-glance organization.
4. **Four engines, one browser.** Every tab can run on Servo, CEF, WebKit,
   or WebView2 — pick per tab, or open "Split" to compare the same page
   across two engines side by side.
5. **Not just tabs.** Notes, images, PDFs, and JSON files live on the same
   graph as your tabs — drag one into a group with related tabs instead of
   hunting through a separate downloads list.
6. **Try it: expand a JSON file into the graph.** Right-click any JSON
   node → "Expand as Graph" to explode its structure into real, draggable,
   nested nodes instead of a flat popup viewer.

Simple card-per-section layout (each section its own bordered/rounded
block, matching the app's own card visual language used for graph nodes),
laid out in a single scrollable column — no interactivity, no JS: this
page is read-only content, not a mini web-app.

## Testing

- Build-verify `manifold-engine-webview2` with zero warnings.
- Live-verify: launch the real (non-headless) app, navigate a tab to
  `manifold://welcome` via the address bar, confirm the page renders with
  the correct content and styling, and confirm typing it works exactly
  like typing any other URL (no special-casing needed on the `manifold`
  host side, per the architecture above). Also confirm an unknown page
  (`manifold://nonexistent`) shows a 404 rather than crashing or hanging.
- Headless verification is not expected to be meaningful here (this is
  entirely a WebView2-internal rendering path with no `manifold.exe`-side
  logic to drive via debug commands) — this is one of the rare cases in
  this project where a real, non-headless launch is the only verification
  that actually exercises the change.
