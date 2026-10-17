---
status: done
---

# Cookie jars: design, 2026-10-02

Status: built (v1), 2026-10-03: one default jar, WebView2 and CEF only (plan `docs/superpowers/plans/2026-10-02-cookie-jar-v1.md`,

> **Superseded in part by `2026-10-13-jars-as-cards-design.md` (built 2026-10-14):** jars are cards on the graph, the Default is a card, and one `jar` edge replaces the per-tab field and the group-default snapshot.
final review fixes in `.superpowers/sdd/2026-10-02-cookie-jar-v1/`). Decisions from the user on 2026-10-02: **persistent per
jar**; **cookies only for v1**.

**WebView2 tabs now run InPrivate, so WebView2 no longer persists localStorage, IndexedDB, cache or permissions across
restarts; only cookies persist, through the jar.** Before v1 they kept all of that in the shared user-data folder. Sites that
keep a login in localStorage (many single-page apps keep a token there) now sign the user out on every restart and on
every WebView2 sleep and wake (each host has its own in-memory profile, so nothing survives its host).
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

Built (v1): a single default jar, `Jar { cookies }` in `<profile>\jars\default.json`, used by every WebView2 and CEF tab.
The per-tab `jar` field, named jars and group inheritance below are **not built yet**; they are the design for later.

- `Jar { id, name, cookies: Vec<Cookie> }`, stored in the profile (`<profile>\jars\<id>.json`).
- `Cookie` is the neutral shape the spike used: `name, value, domain, path, expires: Option<f64 unix>, secure, http_only,
  same_site: Strict | Lax | None | Unspecified`.
- A tab node carries `jar: Option<JarId>`; a group may set a jar that its tabs inherit **at creation** (snapshot, not live
  inheritance). `None` means the default jar. Jars appear in the graph as nodes later; v1 only needs the field.

## Flow

As built:

1. **Tab start:** WebView2 and CEF hosts are started with `ENGINE_AWAIT_COOKIES=1`. Manifold sends the jar's cookies
   (`SetCookies`, a JSON array of neutral cookies) as its first message once the host connects, and the host waits up to
   5 s for it before its first navigation. The snapshot is read at connect time, so it is at most one pull old. Servo and
   WebKit are neither seeded nor pulled.
2. **During use:** the engine's own store is the working copy. Manifold pulls it back (`GetCookies`, answered by `Cookies`)
   every 30 s for every live WebView2 and CEF tab, before a sleep or an engine swap (which wait up to 2 s for the reply),
   and once at quit (waiting up to 1 s). There is no pull on tab close or on navigation.
3. **Merge**, per tab, against two sets manifold keeps for that tab: its *baseline* (the cookies it was seeded with,
   updated by each of its pulls) and the keys it has itself *reported* in a pull (empty at spawn: a seed is not proof the
   engine holds a cookie). A pulled cookie equal to the tab's baseline is unchanged and ignored, so an idle tab's stale copy
   cannot overwrite a value another tab changed since; one that differs or is new replaces the jar's (last change wins per
   `(name, domain, path)`; an already-expired one deletes it). Expiries within 1 ms count as equal, against the baseline
   and against the jar's own copy, so an engine's float conversion of the same expiry is never a change. A key the tab reported before and no longer has is a
   deletion, so a logout propagates; a seeded key the tab never reported is never deleted by it, so a seed the engine
   rejected is not a logout. Both sets are cleared when the tab's host closes or is replaced.
4. **A host answers only a complete read.** A failed, errored or unparsable read gets no reply (manifold's pull times out
   after 10 s), never `[]`, which would read as a logout of everything. CEF cannot tell an empty store from a slow one (its
   visitor is never called for zero cookies, and a visit before the store initialises is queued): after 2 s without a
   callback it answers `[]` only if that host was never seeded with cookies and has never reported any, and otherwise
   stays silent. WebView2's `Network.getAllCookies` answers nothing on a CDP error or a result without a `cookies` array.
5. **Seeding fidelity.** CEF: a host-only cookie (no leading dot) is set with an empty `domain` and its host in the URL
   (https if `secure`, with the cookie's path), because Chromium prefixes a dot to any non-empty domain and rejects a
   `__Host-` cookie that has one; a dotted domain is passed as is. CEF reads a host-only cookie back by its bare host, the
   neutral form (checked by `scripts/jar-probe.sh` on `jar.localhost`, 2026-10-03). CEF's expiry (`Basetime`, microseconds
   since 1601, past 2^53) is shifted to and from Unix time in integer microseconds, so a microsecond-resolution expiry
   comes back bit for bit (an `f64` shift changed ~90% of `Max-Age` expiries). WebView2: cookies with no name or domain
   are left out of the `Network.setCookies` batch, and a failed batch is retried one cookie at a time.

The design's `about:blank` start page for engines that cannot be seeded before navigation was not needed: both seeded
engines wait for the seed instead.

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

Cookie files are credentials. The design: store them under the profile with user-only file permissions and protect them at
rest with the OS store (DPAPI on Windows; the platform keyring elsewhere). v1 builds the Windows half only (below). Never
log values (the spike's dump printed real ones; the host must filter and the debug commands must not exist outside debug
builds). A jar can be exported only by an explicit user action.

Implemented (Task 6): the jar file is `MJR1` followed by the DPAPI blob (current user, `CRYPTPROTECT_UI_FORBIDDEN`, no
entropy). Without entropy any process of the same Windows user can decrypt it; the accepted v1 threat model is other users
and offline copies. A legacy plaintext file still loads and is rewritten protected on the next save. A file that cannot be
read (decrypt failure, truncation, bad JSON) is renamed to `default.json.unreadable-<unix seconds>[-<n>]` at load, never
over an earlier one; only the 5 newest are kept, since each holds credentials. If it cannot be read at all or cannot be
moved aside, the jar is locked for the session: cookies work in memory but nothing is saved, so the file is never
overwritten. A save writes `default.json.<pid>.tmp` and renames it. Load deletes a temp file whose process is gone (a
crashed save, a full copy of the jar) and leaves a live process's alone, since it may be another instance's save in
progress.

v1 protects the file for the user only through DPAPI, on Windows; there the file itself gets the profile folder's ACL. On
Linux and macOS `MJR1` names the format only: the content is plain JSON, not protected, in a file created with mode 0600.
A file moved across OSes (or users, or machines) is not readable and is set aside as above. Platform keyring protection is
the follow-up, not built.

Two manifold instances on one profile each load the jar once and save their whole jar on every change, so they overwrite
each other's jar file (last save wins). That is not supported in v1; a re-read and merge under a lock before each save, as
the session store does, is the fix if it is needed.

## Decided

- The default jar starts empty. The old WebView2 folder is not imported (decided 2026-10-02).

## WebView2 isolation (spiked 2026-10-02)

`create_environment` passes a null user-data folder (the shared one next to the exe) and creates tabs with
`CreateCoreWebView2CompositionController`. The vendored bindings (`webview2-com-sys` 0.39) offer two ways out:

- **InPrivate (chosen):** `ICoreWebView2ControllerOptions::SetIsInPrivateModeEnabled(true)`, passed through
  `CreateCoreWebView2CompositionControllerWithOptions` (Environment10). No cookie touches disk (a named profile still
  writes caches; below) and the tab starts with an empty jar, so only the jar's cookies are ever present and the user's real shared folder stops being involved. Manifold seeds
  from the jar and pulls back as in the flow above.
- Per-jar user-data folder: a path argument to `CreateCoreWebView2EnvironmentWithOptions`. Works on older runtimes but
  puts a second, WebView2-managed store of the same cookies on disk, and one folder cannot be shared by differently
  configured environments.

Spiked on `spike/cookie-bridge` (7b6666b and its successor; `scripts/spike/inprivate-seed.sh`): an InPrivate
controller created with `CreateCoreWebView2CompositionControllerWithOptions` held 2 cookies in total (the seeded ones)
where the shared-folder controller held 20 real ones. `Network.setCookies`, awaited before the first `Navigate`, made the
very first request carry `sid` (HttpOnly, SameSite=Lax) and `pref` (Strict, session), and `Network.getAllCookies` read them
back with attributes. Still unverified: the minimum WebView2 runtime that has the controller options (the existing code
already degrades when Environment15 is missing, `main.rs:224`; do the same, and fall back to the per-jar folder).
Side effect to know: the spike also showed the non-InPrivate path writing to the real shared folder, which is why the
spike deleted its two test cookies afterwards.

**One profile per live host (built, 2026-10-03).** All WebView2 hosts share one browser process (one user-data folder),
and InPrivate controllers of one profile share one in-memory cookie store: the two-tab run of `scripts/jar-probe.sh`
showed a second tab, seeded with an empty jar, sending the first tab's login. So each host also calls
`ICoreWebView2ControllerOptions::SetProfileName` with a name no other live host has, still InPrivate; the same run then
reports the tabs do not share.

The name comes from a pool (built 2026-10-03, replacing a name per host pid): manifold hands each new host the lowest
free slot, `manifold_0` to `manifold_7`, in `ENGINE_WEBVIEW2_PROFILE` (`process_manager::claim_webview2_slot`), and frees
it when the host is dropped, only after the host process has exited and its profile folder's deletion has been tried. A
lock file per slot (`<user-data folder>\manifold-profile-<n>.lock`, held open without sharing) keeps two manifold
instances on one user-data folder from taking the same slot. More concurrent WebView2 tabs than the pool get names
beyond it (`manifold_8`, ...), and the log says so. A slot does not wait for its folder to be deleted: the shared browser
process keeps a closed profile's folder open for as long as it lives, and waiting made the ten-tab probe climb past the
pool. Reusing the name is still safe because the cookies are InPrivate, in memory, and go with the host: in the reuse
run of `scripts/jar-probe.sh`, tab A logs in and closes, tab B then gets the same name in the same live browser and is
not signed in; in the fresh run a name reused after a browser restart is not signed in either.
Reusing a slot as soon as its host exits was tried and rejected (2026-10-04, ceeace8, reverted): the cookies live in
the shared browser process, and a host's exit does not prove the browser has dropped its InPrivate profile, so a Switch
Jar Now could hand the next jar the old jar's sign-ins. A slot is reused only after its folder's deletion is tried.

Costs seen on runtime 154.0.4258.53:
- No extra processes (two tabs: one browser process, one more renderer, as before).
- A named InPrivate profile still writes a directory, `EBWebView\WV2Profile_manifold_<n>` (~1.8 MB: caches, an empty
  `Network\Cookies` database; no cookie rows). Manifold deletes it when it drops the host (as it does a CEF cache dir,
  with retries) or when it next takes the slot, and sweeps leftovers older than a minute at startup
  (`process_manager::sweep_webview2_profiles`, only that prefix). The live browser holds closed profiles' folders, and a
  browser starting up recreates the last-used profile's folder, so after a run up to the pool's worth of folders
  remain until the next start.
- WebView2 registers every named profile in the folder's `Local State` (`profile.info_cache`, ~1.1 KB each, plus
  `profile.profiles_order`) and never forgets it. `ICoreWebView2Profile8::Delete` on `Shutdown` did not remove the
  entries (tried: manifold kills the host right after sending `Shutdown`, and the browser dies with it before it writes
  `Local State`). With the pool, the entries stop at the most WebView2 tabs ever open at once (at least the pool's 8):
  the probe's ten tabs opened and closed in turn used 3 names and added none.

## Out of scope for v1

localStorage, sessionStorage, IndexedDB, cache, permissions, a jar picker UI, jar nodes drawn in the graph, a "clear
browsing data" command. Storage has no verified engine-agnostic read or write path.

## Open questions

What was built answers some of these; each says so.

- Sync cadence versus cost: built as a 30 s timer plus a pull before sleep and swap (2 s wait) and at quit (1 s wait). Whether
  a pull on every navigation commit is cheaper than a timer is still open.
- Whether missing never-visited path-scoped cookies on WebKit and Servo is acceptable (it is a small gap).
- No delta broadcast in v1: a running tab, CEF or WebView2, does not receive cookies another tab added later; it gets them
  when it is next respawned. Each tab has its own cookie store: WebView2 tabs did share one until each host got its own
  profile (WebView2 isolation, above; decided 2026-10-03 over treating all WebView2 tabs as one participant).
- With no delta broadcast, a logout in one tab is undone by another tab that still holds the cookie only if that tab
  changes it again; an unchanged copy (expiries within 1 ms) does not overwrite the jar (the per-tab baseline). A rotation
  in one tab likewise survives an idle tab's stale copy.
- WebView2's `Local State` grew by about 1 KB per WebView2 host ever started (one named profile each, never
  unregistered). Answered by the profile-name pool (WebView2 isolation, above): it now grows only when more WebView2 tabs
  are open at once than ever before, past 8. Names made before the pool (`manifold_<pid>`) stay in an existing
  `Local State`; nothing removes them. Still open: a host killed right after `Shutdown` never lets a profile `Delete`
  land, which a host exiting on its own would allow.
- A tab closed within 30 s of its last cookie change loses that change: there is no pull on close (only every 30 s, before a
  sleep or swap, and at quit), and never from a host that is gone.
- Pulls are best effort. An unanswered `GetCookies` times out after 10 s and blocks nothing, but a reply that arrives late
  is still merged. After 3 unanswered pulls in a row a tab is not asked again until it respawns, so a WebView2 tab whose CDP
  read keeps failing stops contributing its logins. A WebView2 host that fell back to the shared profile (InPrivate
  unavailable) never answers `GetCookies` and is not seeded, so the jar never absorbs the user's real shared-profile cookies.
- A CEF host that was seeded but whose store has since emptied (every cookie deleted) never reports the deletions: CEF does
  not call the visitor for an empty store, and a seeded host stays silent rather than answer `[]` (Flow 4).
- **Known gap: such a silent CEF tab can be retired for good.** A host counts as seeded whenever the jar was non-empty,
  even if every cookie was rejected or already expired (load keeps expired cookies and the seed sends them). Its store is
  then empty, every pull is silent, and after 3 silent pulls (about 90 s) it is never asked again: not on the timer,
  before a sleep or swap, or at quit. A login the user makes later in that tab never reaches the jar. Fixes: keep asking a
  retired tab at a low rate and always at sleep, swap and quit, or do not count silence from a host that has never
  reported anything.
- Session cookies (`expires: null`) are saved and re-seeded on every restart until a tab reports them gone, unlike a
  browser, which drops them when it closes. A design choice for v1, not yet reviewed with the user.
- Manifold and its hosts ship together: the IPC messages are not versioned, and a host built from another commit (such as
  the WebKit host built separately in WSL) must be rebuilt with manifold.
- Saves (DPAPI, a write and a rename) run on the UI thread for every pull that changed the jar. Cheap now; move them off the
  frame path or debounce them if the number of tabs grows.
- The implementation plan is historical: it still shows earlier signatures (`jar_json: Option<&str>`,
  `apply_pulled_cookies(json)`), unticked steps, and commit titles ("seed from the jar file") from the abandoned file-seeding
  design. Seeding is over IPC.
