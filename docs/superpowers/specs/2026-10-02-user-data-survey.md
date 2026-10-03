# User data (cookies, local/session storage): survey, 2026-10-02

A survey, not a design. Nothing here is decided. Source reads are of this repo and the vendored crates; the WebView2 row is
from the WebView2 API as I know it and is **not verified in this repo**.

## Today

| Engine | Where page data lives | Persists across restart | Shared between tabs |
|---|---|---|---|
| WebView2 | one user-data folder next to `manifold-engine-webview2.exe` (null folder passed, README "Open: WebView2's user-data folder") | yes | yes |
| CEF | per-host throwaway `root_cache_path` under `<profile>\cef-cache`, `cache_path` empty | no | no (one process per tab, Chromium's process singleton) |
| WebKit | `WebContext::default()`: WebKitGTK's default data manager, XDG dirs, outside the profile (not checked on disk) | yes (default context is persistent) | yes (one context per host process, so per tab? not checked) |
| Servo | `ServoBuilder::default()`: `opts.config_dir` is `None`, so client and cache storage use a tempdir; cookies not checked | no (storage); cookies unknown | no |

manifold's own files (session graph, bookmarks, history, favicons, settings) are in `%APPDATA%\manifold` and do not hold page
state. Page state is dropped on restore and on an engine swap (README, known v1 limitation). There is no "clear browsing
data" UI.

## What each engine offers

- **WebView2:** the user-data folder is a creation parameter, so it can point into the profile. An InPrivate environment option
  and a profile `ClearBrowsingData` call exist in newer runtimes (to verify before relying on them).
- **CEF:** a persistent profile needs `cache_path`, and one browser process may use a given root. Options: keep per-tab
  throwaway (today); one CEF host for all CEF tabs with a persistent root (loses one-process-per-tab); or several
  `RequestContext`s with their own `cache_path` inside one process (same loss).
- **WebKitGTK:** `WebContext::new_ephemeral()` for memory-only, or a `WebsiteDataManager` with explicit base dirs inside the
  profile for persistent; `WebsiteDataManager` can also clear data by type.
- **Servo 0.5.0:** `Opts::config_dir` and `temporary_storage` choose where storage goes (set `config_dir` into the profile
  for persistence).

## Stances to choose between

1. **Persist everywhere, in the profile.** Cookie logins survive restarts on all engines. Costs: CEF needs a shared host
   (one process for all CEF tabs) or accepts that each tab has its own login; every engine's folder must be routed into
   `%APPDATA%\manifold` and cleared by one action.
2. **Ephemeral everywhere.** Matches CEF today and the "graph shape only" session model; simple privacy story. Costs: no
   logins ever, a real usability hit for a daily browser.
3. **Persist where cheap, say so.** WebView2 and WebKit persist (routed into the profile), CEF and Servo do not; document
   per-engine behaviour and show it in the engine picker. Least work, most surprising.

## Common to every stance

- Route WebView2's folder (and WebKit's, if persistent) into `profile::dir()`; tests must not touch the real one.
- A "Clear browsing data" action (per engine, refused or deferred while a tab of that engine is live).
- Say in Settings what each engine keeps.

## Unchecked

WebKit's on-disk location and per-tab sharing; Servo's cookie storage; the WebView2 InPrivate and ClearBrowsingData APIs.
