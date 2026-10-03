# Cookie jars: design, 2026-10-02

Status: proposed, not built. Decisions from the user on 2026-10-02: **persistent per jar**; **cookies only for v1**.
Background: `2026-10-02-user-data-survey.md` (what each engine does today). Evidence: spike branch `spike/cookie-bridge`
(commit 7b6666b, throwaway, not for merge).

## Goal

A *jar* is a named cookie store that manifold owns. A tab uses exactly one jar, whatever engine renders it, so a login made
in one engine is present in another, and a swap between engines keeps the user signed in.

## What the spike showed (WebView2 to CEF)

- WebView2 exports its whole jar through CDP `Network.getAllCookies`: name, value, domain, path, expires, session,
  secure, httpOnly, sameSite.
- CEF accepts them through `CookieManager::set_cookie`. The first request after injection carried `sid` and `pref`.
  HttpOnly held (`document.cookie` omitted `sid`, the server still received it). SameSite was mapped but not read back.
- The target tab must be live; CEF takes cookies only once its host runs.
- WebView2's one persistent folder holds the user's real cookies, so any export must be filtered by jar.
- Not tested: Servo, WebKit, mid-session changes.

## Model

- `Jar { id, name, cookies: Vec<Cookie> }`, stored in the profile (`<profile>\jars\<id>.json`).
- `Cookie` is the neutral shape the spike used: `name, value, domain, path, expires: Option<f64 unix>, secure, http_only,
  same_site: Strict | Lax | None | Unspecified`.
- A tab node carries `jar: Option<JarId>`; a group may set a jar that its tabs inherit **at creation** (snapshot, not live
  inheritance). `None` means the default jar. Jars appear in the graph as nodes later; v1 only needs the field.

## Flow

1. **Tab start:** manifold sends the jar's cookies to the host (`SetCookies`) before the first navigation.
2. **During use:** the engine's own jar is the working copy. Manifold syncs back (`GetCookies`) on tab close, on engine
   swap, on sleep, and on a timer while a tab is live.
3. **Merge:** last write wins per `(name, domain, path)`; a cookie missing from an engine's export is deleted from the jar
   only if it was present at the last sync (a logout must propagate).
4. Engines that cannot be seeded before navigation get a start page of `about:blank`, seed, then navigate.

## Engine mapping

| Engine | Get | Set | Notes |
|---|---|---|---|
| WebView2 | CDP `Network.getAllCookies` (proved) | CDP `Network.setCookies` or `CookieManager.AddOrUpdateCookie` | the shared persistent folder must stop being the source of truth: use InPrivate or a per-jar folder so only jar cookies are present |
| CEF | `visit_all_cookies` | `set_cookie` (proved) | per-tab throwaway cache stays; the jar is the persistence |
| WebKit | `CookieManager::cookies(uri)` (feature `v2_20`; one URI per call, no whole-jar dump) | `add_cookie` (async) | read in the vendored crates, not run. `soup3::Cookie` has `is_http_only` and `same_site_policy`, so the attributes fit. Both calls are async and need the glib main context |
| Servo | `SiteDataManager::cookies_for_url(url, CookieSource::HTTP)` (also `_async`) | `set_cookie_for_url(url, cookie, cb)` (always `CookieSource::HTTP`, so HttpOnly can be set) | read in the vendored crates, not run. Uses the `cookie` crate type (http_only, same_site, expires). One URL per call, no whole-jar dump |

WebKit and Servo return cookies per URL, but both also list which domains hold cookies, so a whole-jar dump is a
two-step read: list domains, then fetch each as a URL. WebKit: raw `webkit_cookie_manager_get_domains_with_cookies`, wrapped
as `CookieManager::domains_with_cookies`. Servo: `SiteDataManager::site_data(StorageType::Cookies)` (from the net layer's
`ListCookies`; names are sites, eTLD+1). Both read in the vendored sources, not run. Gap that remains: the per-URL fetch
filters by the URL's path and scheme, so a cookie scoped to `/admin`, or `Secure` when asked over `http`, is returned only
if that URL is asked for. Ask `https://<domain>/` and `http://<domain>/`, and add the paths of the tab's history; a
path-scoped cookie never visited is missed. Neither engine exposes a raw "all cookies" call (checked: no such function in
`webkit2gtk-sys` or in `servo-net-traits`' `CoreResourceMsg`).

## Security

Cookie files are credentials. v1 stores them under the profile with user-only file permissions and protects them at rest
with the OS store (DPAPI on Windows; the platform keyring elsewhere). Never log values (the spike's dump printed real
ones; the host must filter and the debug commands must not exist outside debug builds). A jar can be exported only by an
explicit user action.

## Decided

- The default jar starts empty. The old WebView2 folder is not imported (decided 2026-10-02).

## WebView2 isolation (read, not run)

`create_environment` passes a null user-data folder (the shared one next to the exe) and creates tabs with
`CreateCoreWebView2CompositionController`. The vendored bindings (`webview2-com-sys` 0.39) offer two ways out:

- **InPrivate (chosen):** `ICoreWebView2ControllerOptions::SetIsInPrivateModeEnabled(true)`, passed through
  `CreateCoreWebView2CompositionControllerWithOptions` (Environment10). Nothing touches disk and the tab starts with an empty
  jar, so only the jar's cookies are ever present and the user's real shared folder stops being involved. Manifold seeds
  from the jar and pulls back as in the flow above.
- Per-jar user-data folder: a path argument to `CreateCoreWebView2EnvironmentWithOptions`. Works on older runtimes but
  puts a second, WebView2-managed store of the same cookies on disk, and one folder cannot be shared by differently
  configured environments.

To verify before building: the minimum WebView2 runtime that supports the controller options (the existing code already
degrades when Environment15 is missing, `main.rs:224`; do the same, and fall back to the per-jar folder), and that
InPrivate controllers accept `Network.setCookies` before the first navigation.

## Out of scope for v1

localStorage, sessionStorage, IndexedDB, cache, permissions, a jar picker UI, jar nodes drawn in the graph, a "clear
browsing data" command. Storage has no verified engine-agnostic read or write path.

## Open questions

- Sync cadence versus cost for the timer, and whether a pull on every navigation commit is cheaper than a timer.
- Whether missing never-visited path-scoped cookies on WebKit and Servo is acceptable (it is a small gap).
