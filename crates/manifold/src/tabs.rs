//! Tab lifecycle on App: spawn, respawn, focus, hide, split, leave to graph, destroy and remove.

use super::*;
use ipc_protocol::PageState;
use sleep_state::{PendingSleep, SavedScroll, SleepWhy, Verdict};

/// Tab spawn and respawn bookkeeping (`App::spawns`): spawns in flight and what waits on them, the per-tab state
/// that outlives a spawn (`host_loads`, `navigated_from`, `skip_convert`, `tab_crashes`), and the session generation
/// that tells a result from a replaced session.
#[derive(Default)]
pub(crate) struct SpawnTracker {
    /// Tab spawns started by `spawn_tab_async` whose `TabSpawned` result
    /// hasn't been handled yet. Read by the headless `WAITTABS` command,
    /// whose waiting senders sit in `tabs_spawned_waiters` until this
    /// reaches zero.
    pub(crate) tab_spawns_in_flight: usize,

    /// Ids whose tab spawn is in flight (every `SpawnPurpose`), each with
    /// the tab it was started as (`session_sync::spawn_snapshot`). No live
    /// node yet (contract "In-flight spawns"); a save creates one the store
    /// doesn't hold yet (a new, imported or reopened tab) from this snapshot
    /// (`snapshot`), so a quit before it lands doesn't lose it. A respawn
    /// queues no delete, so the store keeps the tab meanwhile.
    pub(crate) spawning: HashMap<String, session_sync::Snapshot>,

    /// Other replicas' latest change to a node in `spawning`, applied when
    /// its spawn lands (`None`: deleted). The spawn itself carries only
    /// what it was started with.
    pub(crate) remote_in_flight: HashMap<String, Option<session_sync::RemoteNode>>,

    /// Tabs loading a url the host gave them, with when each was last seen
    /// loading; their url changes aren't shared meanwhile
    /// (`App::shares_url_change`).
    pub(crate) host_loads: HashMap<String, std::time::Instant>,

    /// Tabs `navigate_tab` just moved on, with the url each left, when, and
    /// whether the engine has reported loading since (`App::reported_url`).
    pub(crate) navigated_from: HashMap<String, (String, std::time::Instant, bool)>,

    pub(crate) tabs_spawned_waiters: Vec<std::sync::mpsc::Sender<()>>,

    /// Bumped by `replace_running_session`. A tab spawn carries the value it
    /// started under, and `handle_tab_spawn_result` drops a result from an
    /// older one: it belongs to the replaced session, and inserting it would
    /// add a stray tab to the new scene, so a spawn from a replaced session
    /// never lands in it.
    pub(crate) session_generation: u64,

    /// Recent engine crash times per tab id (`tabs::crashes_too_often`): a
    /// tab crashing too often is left as a failed card, not respawned.
    pub(crate) tab_crashes: HashMap<String, Vec<std::time::Instant>>,

    /// The tab id `respawn_tab` was called for *while it was the focused
    /// tab* — i.e. an engine swap or crash recovery is currently respawning
    /// the tab the user is looking at. `build_tab_view_ui`'s existing "focused
    /// tab missing from `self.nodes`" guard (a pre-existing one-frame safety
    /// net, from before spawning was async) would otherwise bounce the user
    /// to Graph view for the tab's *entire* respawn duration — imperceptible
    /// when spawning was synchronous, but a visible multi-second flicker
    /// back to Graph and then back to Tab view now that a WebKit respawn can
    /// take many seconds. Set in `respawn_tab`, cleared in
    /// `handle_tab_spawn_result` once that same id's `Respawn` outcome
    /// (`Ok` or `Err`) arrives.
    pub(crate) pending_respawn_focus: Option<String>,

    /// Unloaded tabs' cards whose `wake_tab` spawn is in flight. Unlike a
    /// crash respawn, the card is still on the graph meanwhile, so it can
    /// be moved or closed: the outcome takes the card's placement as it is
    /// now, and is dropped if the card is gone. Set in `wake_tab`, cleared
    /// by that outcome in `handle_tab_spawn_result`.
    pub(crate) wakes_in_flight: HashSet<String>,

    /// Tabs that stay tabs whatever they load, each with whether that load
    /// has settled (reported `loading == false`): one opened by "Open as
    /// tab", and one loading another replica's url (`materialize`), which
    /// that replica converts itself. Every `RedirectableContent`/`JsonBody`
    /// report is skipped while a tab is in here. It stays until a real url
    /// change after the load settles (an HTTP redirect during it doesn't
    /// count; see `skip_convert_after`) or the tab closes (`remove_tab`).
    /// Not persisted (spec "Left open").
    pub(crate) skip_convert: HashMap<String, bool>,
}

impl SpawnTracker {
    /// Drops `id`, a node being removed for good, from the per-tab state that is not about a spawn in flight.
    /// `spawning`, `remote_in_flight`, `wakes_in_flight` and `host_loads` are not here: the spawn lifecycle owns them.
    pub(crate) fn forget(&mut self, id: &str) {
        self.skip_convert.remove(id);
        self.navigated_from.remove(id);
        self.tab_crashes.remove(id);
        if self.pending_respawn_focus.as_deref() == Some(id) {
            self.pending_respawn_focus = None;
        }
    }

    /// A session replace: drops the replaced session's spawns in flight and bumps `session_generation`, so their
    /// results are dropped when they land. The headless wait counters, `skip_convert` and `tab_crashes` stay
    /// (`forget` takes the per-tab ones, id by id).
    pub(crate) fn reset(&mut self) {
        self.wakes_in_flight.clear();
        self.spawning.clear();
        self.host_loads.clear();
        self.navigated_from.clear();
        self.remote_in_flight.clear();
        self.session_generation += 1;
    }
}

/// Which call site triggered a tab spawn — `handle_tab_spawn_result` uses
/// this to know what to do once the (now-async) `process_manager::spawn_tab`
/// call finishes, mirroring what `spawn_root_tab`/`spawn_new_tab`/
/// `respawn_tab` used to do inline before the spawn was made async.
pub(crate) enum SpawnPurpose {
    /// The very first tab at startup: becomes focused, switches to Tab view.
    Root,
    /// Graph view's "+ New Tab" (or a page-triggered new tab): spawned
    /// invisible, no focus change.
    New,
    /// Engine swap finishing, or crash recovery: visibility/focus depend on
    /// whether `tab_id` is still the focused tab by the time the spawn
    /// actually completes (checked at handling time, not capture time).
    Respawn,
    /// Startup session restore (see `App::restore_session`) — one of
    /// potentially many concurrent spawns kicked off together, so (like
    /// `Respawn`) visibility/focus is decided at completion time against
    /// whatever `self.focused` was set to before any of them were issued,
    /// not capture time.
    Restore,
    /// `App::start_split`'s secondary pane -- spawned invisible like `New`,
    /// but once it completes it becomes `self.split_tab` and turns visible
    /// (a split's secondary pane is always live, unlike an ordinary
    /// background tab) rather than gaining focus. See `App::split_tab`'s
    /// own doc comment.
    Split,
    /// `App::wake_tab`: an unloaded tab's engine starting (a sleeping tab
    /// opened, a failed one retried). Lands like `Respawn`, decided at
    /// completion against `self.focused`, and is a host-started load
    /// (`note_host_load`), like a restore.
    Wake,
}

/// A pending `process_manager::spawn_tab` call's outcome, carried back to
/// the main/event-loop thread via `EventLoopProxy` — see `App::spawn_tab_async`
/// for why this exists: spawning WebKit's engine-host blocks on a cold WSL2
/// boot + Xvfb + WebKitGTK init (confirmed live to take several real seconds
/// on first use, occasionally much longer — see `process_manager::
/// spawn_webkit_tab`'s own doc comment), which would otherwise freeze the
/// whole app (redraw, input, headless stdin) for that entire duration if run
/// on this thread.
pub(crate) struct TabSpawnOutcome {
    pub(crate) spawn: TabSpawn,
    /// `App::session_generation` when the spawn started -- see that field.
    pub(crate) generation: u64,
    pub(crate) result: Result<EngineTab, process_manager::SpawnError>,
    /// The cookies the host was seeded with, as its starting `jar::TabCookies` (empty for hosts that are not).
    pub(crate) seeded: jar::TabCookies,
    /// The jar the host was seeded from, resolved when the spawn started: its pulls merge into this one.
    pub(crate) jar: jar::JarKey,
    /// That jar's clear count when the seed was read (`jar::Snapshots::seed`); `None` if it never was.
    pub(crate) seed_clears: Option<u64>,
}

/// What `spawn_tab_async` needs to start a tab and, once it's up, to
/// insert its graph node: `TabSpawn::new` for a new tab (no parent or
/// group, nothing carried), `TabSpawn::respawn` for one that
/// already has a node.
pub(crate) struct TabSpawn {
    pub(crate) id: String,
    pub(crate) url: String,
    pub(crate) kind: EngineKind,
    pub(crate) parent: Option<String>,
    pub(crate) group: Option<String>,
    /// The jar a new tab starts with, which it lands with (`Carried::jar`). A tab the mirror already holds (a
    /// respawn or wake) uses the mirror's instead, which a cascade may change while it is in flight.
    pub(crate) jar: jar::JarChoice,
    /// Where a new tab lands: its layout entry and its document pos start there (2b-4 Ruling 3). A tab already
    /// placed (a respawn, a wake) keeps both (`App::layout` outlives the respawn, `Mirror::seed` never overwrites).
    pub(crate) pos: egui::Vec2,
    pub(crate) purpose: SpawnPurpose,
    pub(crate) carry: TabCarry,
    /// The engine to respawn with if this spawn fails: a swap's old
    /// engine, whose process `finish_swap` has already had to drop.
    pub(crate) fallback_kind: Option<EngineKind>,
}

/// What a tab keeps across a respawn (engine swap, crash recovery) or a
/// wake. A new tab starts unpinned, untagged, at the default zoom, with no
/// title or mute of its own yet. A new attribute that must survive a
/// respawn or wake is one more field here.
pub(crate) struct TabCarry {
    /// See `Mirror::pinned`; seeds the mirror when the tab's id has no entry yet.
    pub(crate) pinned: bool,
    /// See `Mirror::color`; seeds the mirror when the tab's id has no entry yet.
    pub(crate) color: Option<egui::Color32>,
    /// See `TabPayload::zoom_factor`. A new tab takes
    /// `Settings::zoom_factor`, having no value of its own yet.
    pub(crate) zoom_factor: f64,
    /// See `TabPayload::title`: shown until the page reports its own.
    pub(crate) title: String,
    /// See `TabPayload::muted`. Sent to the new engine on landing.
    pub(crate) muted: bool,
}

impl TabSpawn {
    pub(crate) fn new(
        id: String,
        url: String,
        kind: EngineKind,
        pos: egui::Vec2,
        purpose: SpawnPurpose,
        zoom_factor: f64,
    ) -> Self {
        TabSpawn {
            id,
            url,
            kind,
            parent: None,
            group: None,
            jar: Default::default(),
            pos,
            purpose,
            carry: TabCarry { pinned: false, color: None, zoom_factor, title: String::new(), muted: false },
            fallback_kind: None,
        }
    }

    /// Tab `node` (`id`) respawned as `kind` at `url`, keeping its `TabCarry` (`carried` is the mirror's); its place
    /// in the graph is its `App::layout` entry, which the respawn leaves alone. `None` if `node` isn't a tab.
    pub(crate) fn respawn(id: &str, node: &GraphNode, carried: Carried, kind: EngineKind, url: String) -> Option<Self> {
        let tab = node.as_tab()?;
        Some(TabSpawn {
            id: id.to_string(),
            url,
            kind,
            parent: carried.parent.clone(),
            group: carried.group.clone(),
            jar: carried.jar,
            pos: carried.pos.unwrap_or_default(),
            purpose: SpawnPurpose::Respawn,
            carry: TabCarry {
                pinned: carried.pinned,
                color: carried.color,
                zoom_factor: tab.zoom_factor,
                title: tab.title.clone(),
                muted: tab.muted,
            },
            fallback_kind: None,
        })
    }
}

/// An engine answer parked until the tab's cookies arrive (`App::hold_for_cookies`).
pub(crate) enum Held {
    PageState(PageState),
    Snapshot(EngineKind, String),
}

/// How long a sleep or swap waits for the host's `Cookies` reply before going ahead without it.
const COOKIE_HOLD: std::time::Duration = std::time::Duration::from_secs(2);

/// How long quitting waits for the tabs' last `Cookies` replies (`App::final_cookie_pull`).
const QUIT_PULL_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// A `GetCookies` unanswered this long is given up on, so the next pull may ask again.
pub(crate) const JAR_PULL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The longest a jar a pull changed waits for its pull round to end before it is saved anyway (`flush_dirty_jars`).
pub(crate) const JAR_FLUSH_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

/// A tab whose last `MAX_SILENT_PULLS` pulls all went unanswered is given up on: its host cannot answer (the WebView2
/// shared-profile fallback, a CDP error), so it is no longer asked and nothing waits on it.
const MAX_SILENT_PULLS: u8 = 3;

/// Records a new `GetCookies` to `id` and returns whether to send it: not while one is pending, and not once the host
/// has stayed silent `MAX_SILENT_PULLS` times in a row (`silent` counts the unanswered ones; a reply resets it).
pub(crate) fn start_pull(
    pulls: &mut HashMap<String, std::time::Instant>,
    silent: &mut HashMap<String, u8>,
    id: &str,
    now: std::time::Instant,
) -> bool {
    if silent.get(id).is_some_and(|n| *n >= MAX_SILENT_PULLS) || pull_pending(pulls, id, now) {
        return false;
    }
    if pulls.contains_key(id) {
        // The previous pull timed out unanswered.
        let n = silent.entry(id.to_string()).or_default();
        *n += 1;
        if *n >= MAX_SILENT_PULLS {
            pulls.remove(id);
            return false;
        }
    }
    pulls.insert(id.to_string(), now);
    true
}

/// Whether `id` has a `GetCookies` sent less than `JAR_PULL_TIMEOUT` ago and not yet answered.
pub(crate) fn pull_pending(pulls: &HashMap<String, std::time::Instant>, id: &str, now: std::time::Instant) -> bool {
    pulls.get(id).is_some_and(|sent| now.saturating_duration_since(*sent) < JAR_PULL_TIMEOUT)
}

/// A tab whose engine crashes this many times within `CRASH_WINDOW` is left
/// as a failed card instead of respawning again.
pub(crate) const CRASH_LIMIT: usize = 3;
pub(crate) const CRASH_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// Records a crash at `now` in `times` (one tab's) and says whether that tab
/// has now crashed `CRASH_LIMIT` times within `CRASH_WINDOW`.
pub(crate) fn crashes_too_often(times: &mut Vec<std::time::Instant>, now: std::time::Instant) -> bool {
    times.retain(|&t| now.duration_since(t) < CRASH_WINDOW);
    times.push(now);
    times.len() >= CRASH_LIMIT
}

/// Why `leave_failed_card` left a failed card, which picks its toast.
pub(crate) enum LeftClosed {
    /// `AfterCrash::GiveUp`: it crashed too often.
    KeepsCrashing,
    /// `AfterCrash::CannotStart`: the host could not start; the toast says why.
    CannotStart,
    /// "End Engine" on a tab that stopped responding.
    Ended,
}

/// What to do about a tab whose engine reported `Crashed { reason }` at `now`.
#[derive(Debug, PartialEq)]
pub(crate) enum AfterCrash<'a> {
    Restart,
    /// Leave a failed card: the host speaks another protocol version, or keeps crashing (`crashes_too_often`).
    GiveUp,
    /// Leave a failed card naming why: the host could not start at all (its reason began with
    /// `ipc_protocol::HOST_FATAL`), so a respawn would fail the same way.
    CannotStart(&'a str),
}

/// Decides for `Crashed { reason }` at `now`; `times` is the tab's recent crashes, recorded only when it counts.
pub(crate) fn after_crash<'a>(
    reason: &'a str,
    times: &mut Vec<std::time::Instant>,
    now: std::time::Instant,
) -> AfterCrash<'a> {
    if let Some(why) = reason.strip_prefix(ipc_protocol::HOST_FATAL) {
        return AfterCrash::CannotStart(why.trim());
    }
    if reason.starts_with(ipc_protocol::PROTOCOL_MISMATCH) || crashes_too_often(times, now) {
        return AfterCrash::GiveUp;
    }
    AfterCrash::Restart
}

/// `tab` as an unloaded card in `state`, keeping what a reopen needs. Its engine is the mirror's: the one a swap in
/// flight was switching to, if any (a sleep can answer before the swap's snapshot), which it wakes into.
fn unloaded_from(tab: &TabPayload, state: UnloadedState) -> UnloadedTabPayload {
    UnloadedTabPayload {
        url: tab.url.clone(),
        title: tab.title.clone(),
        muted: tab.muted,
        zoom_factor: tab.zoom_factor,
        state,
    }
}

impl App {
    /// Spawns the very first tab at startup: no parent, becomes focused,
    /// starts in Tab view (preserves "opens straight into a usable
    /// browser" from before M5 — Graph view is reached explicitly).
    pub(crate) fn spawn_root_tab(&mut self) {
        let id = self.alloc_node_id();
        let home_page = self.settings.home_page.clone();
        let kind = self.settings.default_starting_engine;
        let zoom = self.settings.zoom_factor;
        self.spawn_tab_async(TabSpawn::new(id, home_page, kind, egui::Vec2::ZERO, SpawnPurpose::Root, zoom));
    }

    /// Runs `process_manager::spawn_tab` on a background thread and posts
    /// its result back via `event_proxy` as `UserEvent::TabSpawned` — see
    /// `TabSpawnOutcome`'s doc comment for why this can't run inline on the
    /// main/event-loop thread. `handle_tab_spawn_result` does what the old
    /// synchronous `Ok`/`Err` branches used to do, once the result actually
    /// arrives.
    pub(crate) fn spawn_tab_async(&mut self, spawn: TabSpawn) {
        self.spawns.tab_spawns_in_flight += 1;
        // Shared at the pos, url, engine and jar the mirror holds for it (a respawn's `spawn.url` is the live one); a
        // new tab has no entry yet and shares the ones it starts with.
        let shared_url = Some(self.mirror.url(&spawn.id)).filter(|url| !url.is_empty());
        let has = self.mirror.has(&spawn.id);
        let pos = if has { self.mirror.pos(&spawn.id) } else { Some(spawn.pos) };
        let choice = if has { self.mirror.jar_choice(&spawn.id) } else { spawn.jar };
        let engine = if has { self.mirror.engine_code(&spawn.id) } else { session_sync::engine_code(spawn.kind) };
        let url = shared_url.unwrap_or(&spawn.url);
        let snapshot = session_sync::spawn_snapshot(&spawn, pos, url, engine, choice);
        self.spawns.spawning.insert(spawn.id.clone(), snapshot);
        // A new tab is created from that snapshot by the next save (Ruling 3); a respawn's id is already stored.
        self.queue_create(&spawn.id);
        let generation = self.spawns.session_generation;
        // Resolved and loaded before the test early return, so tests see the load happen.
        let (jar, _) = self.resolved_jar(&choice);
        self.ensure_jar_loaded(jar);
        #[cfg(test)]
        if self.event_proxy.is_none() {
            self.test_spawns.push((spawn, generation));
            return;
        }
        let proxy = self.event_proxy.clone().expect("event_proxy set in main before any tab spawn");
        let wake = crate::event_loop::engine_wake(std::sync::Arc::clone(&self.pacing.engine_wake), proxy.clone());
        let custom_cef = self.custom_cef_dir();
        let snapshots = self.cookies.jar_snapshots.clone();
        std::thread::spawn(move || {
            // Read when the host has connected (the host starts seconds after the spawn): remember its keys, and the
            // jar's clear count then, so a clear before it lands is caught (`note_spawned_jar`).
            let seeded = std::cell::RefCell::new((crate::jar::TabCookies::default(), None));
            let seed = || {
                let (cookies, clears) = match snapshots.lock() {
                    Ok(s) => {
                        let (cookies, clears) = s.seed(jar);
                        (cookies, Some(clears))
                    }
                    Err(_) => (Vec::new(), None),
                };
                *seeded.borrow_mut() = (crate::jar::TabCookies::seeded(&cookies), clears);
                serde_json::to_string(&cookies).unwrap_or_else(|_| "[]".to_owned())
            };
            let result =
                process_manager::spawn_tab(spawn.kind, &spawn.id, &spawn.url, custom_cef.as_deref(), &seed, wake);
            let (seeded, seed_clears) = seeded.into_inner();
            let _ = proxy.send_event(UserEvent::TabSpawned(Box::new(TabSpawnOutcome {
                spawn,
                generation,
                result,
                seeded,
                jar,
                seed_clears,
            })));
        });
    }

    /// Folds tab `tab_id`'s cookie dump into the jar its host was spawned with, and marks that jar dirty when it
    /// changed (`flush_dirty_jars` saves it) (spec: "Its pulls merge into that jar and save only that jar's file").
    /// Merged against that tab's own `jar_tabs` entry (`jar::Jar::merge`). `false` for a dump that does not parse
    /// (logged), a tab with no `jar_tabs` entry (closed or replaced since it was asked; deviation 5), a named jar
    /// deleted while it ran, or a host whose jar was cleared under it (`jar_reseed`): it still holds the old cookies,
    /// and they must not refill the cleared jar.
    pub(crate) fn apply_pulled_cookies(&mut self, tab_id: &str, json: &str) -> bool {
        if self.cookies.jar_reseed.contains(tab_id) {
            return false;
        }
        let pulled = match crate::jar::from_json(json) {
            Ok(pulled) => pulled,
            Err(err) => {
                // All or nothing, never per cookie: a cookie missing from a pull is a logout (`jar::Jar::merge`), so
                // skipping one that doesn't parse would delete a live login. Logged, as the reason this tab's
                // logins stop reaching its jar.
                eprintln!("jar: tab {tab_id}'s cookie dump didn't parse ({err}); its jar is not updated");
                return false;
            }
        };
        let Some((key, _)) = self.cookies.jar_tabs.get(tab_id) else { return false };
        let key = *key;
        if matches!(key, crate::jar::JarKey::Named(id) if !self.mirror.jar_defs().contains_key(&id)) {
            return false;
        }
        self.ensure_jar_loaded(key);
        let (Some(jar), Some((_, tab))) = (self.jars.get_mut(&key), self.cookies.jar_tabs.get_mut(tab_id)) else {
            return false;
        };
        let changed = jar.merge(pulled, tab);
        if changed {
            let jar = jar.clone();
            if let Ok(mut snapshots) = self.cookies.jar_snapshots.lock() {
                snapshots.insert(key, jar);
            }
            // Saved once the round's other replies are in (`flush_dirty_jars`): fifteen tabs that saw one rotating
            // cookie write the file once, not fifteen times.
            self.cookies.jar_dirty.entry(key).or_insert_with(std::time::Instant::now);
        }
        changed
    }

    /// Saves each dirty jar (`jar_dirty`) once its pull round is over, or once it has waited `JAR_FLUSH_AFTER`;
    /// `None` (quit) saves every one. The round is over when no tab has a pull pending (`pull_pending`): one that
    /// timed out unanswered keeps its `jar_pulls` entry until that tab's next pull, so emptiness is not the test. A
    /// named jar deleted meanwhile is skipped, as `apply_pulled_cookies` skips one.
    pub(crate) fn flush_dirty_jars(&mut self, now: Option<std::time::Instant>) {
        if self.cookies.jar_dirty.is_empty() {
            return;
        }
        let round_over = now
            .is_none_or(|now| !self.cookies.jar_pulls.keys().any(|id| pull_pending(&self.cookies.jar_pulls, id, now)));
        let due: Vec<crate::jar::JarKey> = self
            .cookies
            .jar_dirty
            .iter()
            .filter(|(_, since)| {
                round_over || now.is_some_and(|now| now.saturating_duration_since(**since) >= JAR_FLUSH_AFTER)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in due {
            if matches!(key, crate::jar::JarKey::Named(id) if !self.mirror.jar_defs().contains_key(&id)) {
                self.cookies.jar_dirty.remove(&key);
                continue;
            }
            self.save_jar(key);
        }
    }

    /// Asks one WebView2 or CEF tab for its cookies; the answer arrives as `EngineToHost::Cookies`. Best effort:
    /// a host may never answer, so nothing waits on it, and a pull still pending (under `JAR_PULL_TIMEOUT`) is not
    /// stacked.
    pub(crate) fn pull_cookies(&mut self, id: &str) {
        let Some(node) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) else { return };
        if matches!(node.tab.kind, EngineKind::Webview2 | EngineKind::Cef)
            && start_pull(&mut self.cookies.jar_pulls, &mut self.cookies.jar_silent, id, std::time::Instant::now())
        {
            node.tab.send(&HostToEngine::GetCookies);
        }
    }

    /// Parks `held` until tab `id`'s pending `Cookies` reply (or `COOKIE_HOLD`), so the host is not dropped before its
    /// last cookies arrive. Hands it back when no pull is pending: act on it now.
    pub(crate) fn hold_for_cookies(&mut self, id: &str, held: Held) -> Option<Held> {
        let now = std::time::Instant::now();
        if !pull_pending(&self.cookies.jar_pulls, id, now) {
            return Some(held);
        }
        // The latest request wins: a second hold (a sleep then a swap) cancels the first, which may be stale.
        if let Some((_, old)) = self.cookies.cookie_holds.remove(id) {
            self.cancel_held(id, old);
        }
        self.cookies.cookie_holds.insert(id.to_string(), (now + COOKIE_HOLD, held));
        None
    }

    /// Drops a held answer without acting on it, leaving no half-started request behind: a cancelled sleep's pending
    /// entry goes too (else its timeout would sleep the tab anyway). A swap's `swap_target` was already taken when its
    /// snapshot arrived, so there is nothing to undo.
    fn cancel_held(&mut self, id: &str, held: Held) {
        if let Held::PageState(_) = held {
            self.pending_sleeps.remove(id);
        }
    }

    /// Forgets everything cookie-related about tab `id`'s host (it closed, or a new host replaces it): its `jar_tabs`
    /// entry, pull bookkeeping, its reseed mark and any held answer, which is cancelled, not run against whatever host
    /// comes next.
    pub(crate) fn clear_cookie_state(&mut self, id: &str) {
        if let Some((_, held)) = self.cookies.forget(id) {
            self.cancel_held(id, held);
        }
    }

    /// A new host for tab `id` landed (`handle_tab_spawn_result`): forgets the old host's cookie state, then records
    /// the jar this one was spawned with and its seed. `seed_clears` is that jar's clear count when the seed was read
    /// (`None`: never seeded). A Clear Cookies since then, while the spawn or wake was in flight, found no `jar_tabs`
    /// entry to mark; a host seeded with cookies it cleared is marked now, like the hosts running at the clear, so it
    /// switches and its pulls cannot refill the jar (final review I3). Marked after the wipe, which would drop it.
    pub(crate) fn note_spawned_jar(
        &mut self,
        id: &str,
        jar: crate::jar::JarKey,
        seeded: crate::jar::TabCookies,
        seed_clears: Option<u64>,
    ) {
        self.clear_cookie_state(id);
        let clears = self.cookies.jar_snapshots.lock().ok().map(|s| s.clears(jar));
        let stale = seed_clears.is_some() && clears.is_some() && seed_clears != clears;
        if stale && !seeded.baseline.is_empty() {
            self.cookies.jar_reseed.insert(id.to_string());
        }
        self.cookies.jar_tabs.insert(id.to_string(), (jar, seeded));
    }

    /// Acts on what `hold_for_cookies` parked for `id`, if anything.
    pub(crate) fn release_cookie_hold(&mut self, id: &str) {
        if let Some((_, held)) = self.cookies.cookie_holds.remove(id) {
            self.run_held(id, held);
        }
    }

    /// Acts on every hold whose deadline has passed (a host that never answered its pull).
    pub(crate) fn release_expired_cookie_holds(&mut self) {
        let now = std::time::Instant::now();
        let due: Vec<String> =
            self.cookies.cookie_holds.iter().filter(|(_, (t, _))| *t <= now).map(|(id, _)| id.clone()).collect();
        for id in due {
            self.release_cookie_hold(&id);
        }
    }

    pub(crate) fn run_held(&mut self, id: &str, held: Held) {
        match held {
            Held::PageState(state) => self.finish_sleep(id, state),
            Held::Snapshot(target, url) => self.finish_swap(id, target, url),
        }
    }

    /// Asks every live WebView2 and CEF tab for its cookies.
    pub(crate) fn jar_pull_all(&mut self) {
        self.cookies.jar_pulls.retain(|id, _| self.nodes.contains_key(id));
        let ids: Vec<String> =
            self.nodes.iter().filter(|(_, n)| n.as_tab().is_some()).map(|(id, _)| id.clone()).collect();
        for id in ids {
            self.pull_cookies(&id);
        }
    }

    /// One last pull as the app quits, so a login made since the last timed pull is kept. Waits at most
    /// `QUIT_PULL_WAIT` for the replies. Any other engine message read meanwhile is dropped: nothing would act on it
    /// after quit anyway.
    pub(crate) fn final_cookie_pull(&mut self) {
        self.jar_pull_all();
        let deadline = std::time::Instant::now() + QUIT_PULL_WAIT;
        while !self.cookies.jar_pulls.is_empty() && std::time::Instant::now() < deadline {
            for id in self.cookies.jar_pulls.keys().cloned().collect::<Vec<_>>() {
                let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_tab()) else {
                    self.cookies.jar_pulls.remove(&id);
                    continue;
                };
                let reply = std::iter::from_fn(|| tab.tab.events_rx.try_recv().ok())
                    .filter_map(|msg| match msg {
                        EngineToHost::Cookies(json) => Some(json),
                        _ => None,
                    })
                    .last();
                if let Some(json) = reply {
                    self.cookies.jar_pulls.remove(&id);
                    self.apply_pulled_cookies(&id, &json);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Starts every live tab's engine shutdown at once (`EngineTab::begin_shutdown`), so dropping the app waits one
    /// `SHUTDOWN_FLUSH` for all of them rather than one per tab, and a stuck engine adds nothing.
    pub(crate) fn begin_engine_shutdown(&self) {
        for node in self.nodes.values() {
            if let Some(tab) = node.as_tab() {
                tab.tab.begin_shutdown();
            }
        }
    }

    /// Applies a finished `spawn_tab_async` call — inserts the new tab node
    /// (or leaves a failed card, or toasts for a split), with per-`SpawnPurpose` follow-up
    /// matching what `spawn_root_tab`/`spawn_new_tab`/`respawn_tab` used to
    /// do synchronously right after `process_manager::spawn_tab` returned.
    /// Focus/visibility for `Respawn` is decided here, at completion time —
    /// same moment the old inline code decided it, just later in wall time.
    pub(crate) fn handle_tab_spawn_result(&mut self, outcome: TabSpawnOutcome) {
        let TabSpawnOutcome {
            spawn: TabSpawn { id, url, kind, parent, group, jar: spawn_jar, pos, purpose, carry, fallback_kind },
            generation,
            result,
            seeded,
            jar,
            seed_clears,
        } = outcome;
        let TabCarry { pinned, color, zoom_factor, title, muted } = carry;
        self.spawns.spawning.remove(&id);
        let remote = self.spawns.remote_in_flight.remove(&id);
        self.spawns.tab_spawns_in_flight = self.spawns.tab_spawns_in_flight.saturating_sub(1);
        if self.spawns.tab_spawns_in_flight == 0 {
            for waiter in self.spawns.tabs_spawned_waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
        // Spawned for a session that's since been replaced: dropping the
        // `EngineTab` shuts its engine host down. See `session_generation`.
        if generation != self.spawns.session_generation {
            return;
        }
        // A replaced engine (crash, wake, swap) knows only `url`: from here
        // Back and Forward come from the host's list.
        if let Some(history) = self.nav_history.get_mut(&id) {
            history.engine_reset(&url);
        }
        // A group dissolved while this spawn was still in flight (spawning
        // is async, see this fn's own doc comment) would otherwise leave
        // the new tab pointing at a group id nothing can navigate into —
        // same guard `reopen_closed`'s relinking applies for the same
        // reason, centralized here since every spawn path funnels through
        // this one function.
        let (parent, group, color, pinned, muted) = if self.spawns.wakes_in_flight.remove(&id) {
            match self.nodes.get(&id).and_then(|n| n.as_unloaded()) {
                Some(tab) => {
                    let Carried { color, pinned, parent, group, .. } = self.mirror.carried(&id);
                    (parent, group, color, pinned, tab.muted)
                }
                // Closed while waking (see `wakes_in_flight`): `remove_tab`
                // queued its delete, which this save commits -- without
                // this, nothing else ever saves it again.
                None => {
                    self.save_session();
                    return;
                }
            }
        } else {
            (parent, group, color, pinned, muted)
        };
        if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Restore | SpawnPurpose::Wake)
            && self.spawns.pending_respawn_focus.as_deref() == Some(id.as_str())
        {
            self.spawns.pending_respawn_focus = None;
        }
        match result {
            Ok(tab) => {
                self.note_spawned_jar(&id, jar, seeded, seed_clears);
                let is_focused = match purpose {
                    SpawnPurpose::Root => true,
                    SpawnPurpose::New | SpawnPurpose::Split => false,
                    SpawnPurpose::Respawn | SpawnPurpose::Restore | SpawnPurpose::Wake => {
                        self.focused.as_deref() == Some(id.as_str())
                    }
                };
                // A split's secondary pane is always live even though it's
                // never `focused` -- true for a fresh `Split` spawn, and
                // also (the `self.split_tab` check) for a crash-respawn of
                // whatever tab currently happens to be playing that role,
                // which arrives as an ordinary `Respawn`, not `Split` --
                // see `SpawnPurpose::Split`'s own doc comment.
                let is_split = matches!(purpose, SpawnPurpose::Split) || self.split_tab.as_deref() == Some(id.as_str());
                // The focused tab stays hidden while Image view is showing
                // in front of it -- e.g. relaunching straight into Image
                // view, or a crash respawn of the tab Image view is
                // currently covering (see `spawned_tab_visible`'s doc).
                let visible = spawned_tab_visible(is_focused, is_split, self.showing_image_view());
                tab.set_visible(visible);
                if muted {
                    tab.send(&HostToEngine::SetMuted { muted: true });
                }
                self.nodes.insert(
                    id.clone(),
                    GraphNode {
                        runtime: Runtime::Tab(TabPayload {
                            tab,
                            url: url.clone(),
                            title,
                            can_go_back: false,
                            can_go_forward: false,
                            loading: false,
                            audio_playing: false,
                            muted,
                            zoom_factor,
                            swap_target: None,
                        }),
                    },
                );
                let engine = session_sync::engine_code(kind);
                let pos = Some(pos);
                self.seed_landing(
                    &id,
                    Carried {
                        pos,
                        color,
                        pinned,
                        parent,
                        group,
                        url: url.clone(),
                        engine,
                        jar: spawn_jar,
                        ..Default::default()
                    },
                );
                if !self.node_order.iter().any(|t| t == &id) {
                    self.node_order.push(id.clone());
                }
                self.apply_pending_local(&id);
                // A fresh attempt at a url resolves its earlier failed attempts; landing an existing tab (respawn,
                // restore, wake) is no new intent and leaves a failed card the user placed alone.
                if matches!(purpose, SpawnPurpose::Root | SpawnPurpose::New) {
                    self.clear_failed_cards(&url, &id);
                }
                match purpose {
                    SpawnPurpose::Root => {
                        self.focused = Some(id.clone());
                        self.view_mode = ViewMode::Graph;
                        self.address_bar.text = url;
                        self.send_resize_to(&id);
                        self.send_zoom_to(&id);
                    }
                    SpawnPurpose::New => {
                        self.send_zoom_to(&id);
                    }
                    SpawnPurpose::Split => {
                        self.split_tab = Some(id.clone());
                        self.send_zoom_to(&id);
                        self.send_resize_to(&id);
                        // The primary pane was rendering at full width until
                        // now -- it needs to shrink to half now that
                        // `content_size_px` sees `split_tab` as set.
                        if let Some(focused_id) = self.focused.clone() {
                            self.send_resize_to(&focused_id);
                        }
                    }
                    SpawnPurpose::Respawn | SpawnPurpose::Wake => {
                        // A wake is a host-started load, same as a restore
                        // (`note_host_load`); a plain respawn (crash
                        // recovery, engine swap) needs no such marking --
                        // it isn't a url change at all.
                        if matches!(purpose, SpawnPurpose::Wake) {
                            self.note_host_load(&id);
                        }
                        // A respawned tab is a fresh engine-host process
                        // that starts at that engine's own default zoom --
                        // unlike Root/New/Split/Restore above, this arm
                        // never told it about `zoom_factor` at all, so a
                        // crash-recovery or engine-swap respawn silently
                        // reset the page's zoom while Settings kept
                        // showing the configured value.
                        self.send_zoom_to(&id);
                        if is_focused {
                            if !self.address_bar.focused {
                                self.address_bar.text = url;
                            }
                            // Resizing only matters for what's actually
                            // shown -- skip it while Image view hides this
                            // tab, same as `visible` above.
                            if visible {
                                self.send_resize_to(&id);
                            }
                        } else if is_split {
                            self.send_resize_to(&id);
                        }
                    }
                    SpawnPurpose::Restore => {
                        self.note_host_load(&id);
                        self.send_zoom_to(&id);
                        if is_focused {
                            self.address_bar.text = url;
                            if visible {
                                self.send_resize_to(&id);
                            }
                        }
                    }
                }
                self.land_remote_in_flight(&id, remote);
                self.save_session();
            }
            Err(err) => {
                eprintln!("spawn tab failed for {id}: {err}");
                if let Some(fallback) = fallback_kind {
                    // The switch failed: the old engine is written back (2b-3 Ruling 5).
                    self.edit_engine(&id, fallback);
                    self.queue_toast(format!(
                        "Couldn't switch to {}: {err}. Kept {}.",
                        engine_label(kind),
                        engine_label(fallback)
                    ));
                    let carry = TabCarry { pinned, color, zoom_factor, title, muted };
                    let spawn = TabSpawn {
                        id,
                        url,
                        kind: fallback,
                        parent,
                        group,
                        jar: spawn_jar,
                        pos,
                        purpose,
                        carry,
                        fallback_kind: None,
                    };
                    if let Some(remote) = remote {
                        self.spawns.remote_in_flight.insert(spawn.id.clone(), remote);
                    }
                    self.respawn_tab(spawn);
                    return;
                }
                // A tab keeps its place as a failed card rather than disappearing (see `UnloadedTabPayload`):
                // one that already existed (a crash restart, a restored session or group, a retry) and one just
                // asked for (Root, New). A split's secondary pane is not a card (`App::split_tab`).
                if !matches!(purpose, SpawnPurpose::Split) {
                    // "Couldn't start Blink (CEF): Blink (CEF) is not installed" says it twice.
                    let toast = match err {
                        process_manager::SpawnError::EngineMissing(_)
                        | process_manager::SpawnError::EngineOutdated(_)
                        | process_manager::SpawnError::EngineUnavailable(..) => {
                            format!("Couldn't open {}: {err}", Self::toast_url(&url))
                        }
                        _ => format!("Couldn't start {} for {}: {err}", engine_label(kind), Self::toast_url(&url)),
                    };
                    self.queue_failure_toast(kind, &url, group.as_deref(), &err.to_string(), toast);
                    // The same intent failing again refreshes the card already standing for it.
                    if matches!(purpose, SpawnPurpose::Root | SpawnPurpose::New) {
                        if let Some(twin) = self.failed_twin(&id, &url, kind, group.as_deref()) {
                            if let Some(Runtime::Unloaded(tab)) =
                                self.nodes.get_mut(&twin).map(|node| &mut node.runtime)
                            {
                                tab.state = UnloadedState::Failed(err.to_string());
                            }
                            if matches!(purpose, SpawnPurpose::Root) {
                                self.view_mode = ViewMode::Graph;
                            }
                            self.queue_delete(&id);
                            self.save_session();
                            return;
                        }
                    }
                    // A retry (or restart) that fails beside a card already failed for its intent, which a Root/New
                    // failure made while the retry was waking, keeps the retried card (its id is the one in use) and
                    // drops the other: one card per intent, with this error.
                    if matches!(purpose, SpawnPurpose::Respawn | SpawnPurpose::Wake) {
                        if let Some(twin) = self.failed_twin(&id, &url, kind, group.as_deref()) {
                            self.record_closed(&twin);
                            self.remove_tab(&twin);
                        }
                    }
                    let state = UnloadedState::Failed(err.to_string());
                    let failed = UnloadedTabPayload { url: url.clone(), title, muted, zoom_factor, state };
                    let runtime = Runtime::Unloaded(failed);
                    self.nodes.insert(id.clone(), GraphNode { runtime });
                    // A tab the mirror holds already shows the engine that failed: a switch's target, written when
                    // it started (`edit_engine`); a new one lands on it here.
                    let engine = session_sync::engine_code(kind);
                    let (pos, jar) = (Some(pos), spawn_jar);
                    self.seed_landing(
                        &id,
                        Carried { pos, color, pinned, parent, group, url, engine, jar, ..Default::default() },
                    );
                    if !self.node_order.iter().any(|t| t == &id) {
                        self.node_order.push(id.clone());
                    }
                    self.apply_pending_local(&id);
                    if matches!(purpose, SpawnPurpose::Root) {
                        // Success switches to Graph view; the failed card must be seen too.
                        self.view_mode = ViewMode::Graph;
                    }
                    self.land_remote_in_flight(&id, remote);
                    self.save_session();
                    return;
                }
                self.queue_toast(format!("Couldn't start split view: {err}"));
                // A save while it was in flight may have created it
                // (`spawning`); it's gone now (Ruling 4). One never
                // created is just forgotten.
                self.queue_delete(&id);
                self.save_session();
            }
        }
    }

    /// A failed card already standing for the intent `(url, kind, group)`, other than `id`: a repeat failure refreshes
    /// it instead of adding a second card. Urls compare as exact strings (no normalization).
    fn failed_twin(&self, id: &str, url: &str, kind: EngineKind, group: Option<&str>) -> Option<String> {
        self.nodes.iter().find_map(|(other, node)| {
            let tab = node.as_unloaded()?;
            let same = other != id
                && matches!(tab.state, UnloadedState::Failed(_))
                && tab.url == url
                && self.mirror.engine(other) == kind
                && self.mirror.group(other) == group;
            same.then(|| other.clone())
        })
    }

    /// A tab for `url` landed (`landed`): failed cards for the same url in the landing tab's group (any engine),
    /// other than `landed`, are resolved. Each is recorded in Recently Closed first (it may be an arranged
    /// tab's card), then removed without a save: the landing's own save commits the delete, after the parked remote changes
    /// land. Another url's card, or the same url's card in another group, stays.
    fn clear_failed_cards(&mut self, url: &str, landed: &str) {
        let group = self.mirror.group(landed);
        let stale: Vec<String> = self
            .nodes
            .iter()
            .filter(|(id, node)| {
                id.as_str() != landed
                    && self.mirror.group(id) == group
                    && node
                        .as_unloaded()
                        .is_some_and(|tab| matches!(tab.state, UnloadedState::Failed(_)) && tab.url == url)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.record_closed(&id);
            self.remove_tab(&id);
        }
    }

    /// Tab context menu's "Duplicate": a new tab at `id`'s current url and
    /// engine, sharing its parent and group -- a sibling, not a child of
    /// `id` itself, matching what "duplicate" means in a real browser's
    /// own tab strip. Its engine is the mirror's (a swap in flight: its target).
    /// The copy keeps `id`'s jar choice (deviation 3).
    pub(crate) fn duplicate_tab(&mut self, id: &str) {
        let Some((parent, group, url, kind)) = self.nodes.get(id).and_then(|n| {
            n.runtime.saved_tab().map(|url| {
                (
                    self.mirror.parent(id).map(str::to_owned),
                    self.mirror.group(id).map(str::to_owned),
                    url.to_string(),
                    self.mirror.engine(id),
                )
            })
        }) else {
            return;
        };
        let jar = self.tab_jar(id);
        self.spawn_new_tab_with_kind(parent, group, &url, kind, SpawnPurpose::New, jar);
    }

    /// Graph view's "+ New Tab" action: a child of `parent` (or a new root
    /// if `parent` is `None`), spawned invisible — it stays in Graph view
    /// rather than auto-focusing (see plan's M5 scope decision 5), and
    /// background tabs render nothing until focused anyway. Also used for
    /// a page-triggered new tab (`EngineToHost::OpenTabRequested`), with
    /// that request's URL instead of the configured home page.
    pub(crate) fn spawn_new_tab(&mut self, parent: Option<String>, group: Option<String>, url: &str) {
        let kind = self.settings.default_starting_engine;
        self.spawn_new_tab_with_kind(parent, group, url, kind, SpawnPurpose::New, None);
    }

    /// A tab opened by tab `opener` (a page link or `window.open`): a child of it, in its group, with its jar
    /// (deviation 3).
    pub(crate) fn spawn_tab_from(&mut self, opener: &str, url: &str) -> String {
        let parent = Some(opener.to_string());
        let group = self.group_of(&parent);
        let (kind, jar) = (self.settings.default_starting_engine, self.tab_jar(opener));
        self.spawn_new_tab_with_kind(parent, group, url, kind, SpawnPurpose::New, jar)
    }

    /// Shared by `spawn_new_tab` (always `default_starting_engine`),
    /// `duplicate_tab` (the original tab's own engine), and `start_split`
    /// (`SpawnPurpose::Split` instead of `New`, everything else identical)
    /// -- `purpose` is threaded through to `spawn_tab_async` rather than
    /// hardcoded, since those cases genuinely differ in what should happen once the
    /// spawn completes (see `handle_tab_spawn_result`'s own per-purpose
    /// match). Returns the new tab's id, allocated synchronously even
    /// though the real engine-host process spawn (and thus the node's own
    /// insertion into `self.nodes`) completes asynchronously -- see
    /// `spawn_tab_async`'s own doc comment. `jar` is the opener's choice to copy; `None` inherits from `group`.
    pub(crate) fn spawn_new_tab_with_kind(
        &mut self,
        parent: Option<String>,
        group: Option<String>,
        url: &str,
        kind: EngineKind,
        purpose: SpawnPurpose,
        jar: Option<crate::jar::JarChoice>,
    ) -> String {
        let id = self.alloc_node_id();
        // `parent`'s own position only if it actually shares `group` --
        // the toolbar's "+ New Tab" passes `self.current_group` as `group`
        // but `self.focused` (unconditionally) as `parent`, and those can
        // point at different levels (see `spawn_new_tab`'s own doc
        // comment). Falling back to any existing member of `group` keeps
        // the new tab near its actual new groupmates instead of near an
        // unrelated tab at a different level; `ZERO` (same as a fresh
        // group's own spawn point) covers a brand new, still-empty group.
        let in_group = |id: &str| self.mirror.group(id) == group.as_deref();
        let parent_pos = parent
            .as_deref()
            .filter(|p| in_group(p) && self.nodes.contains_key(*p))
            .or_else(|| self.nodes.keys().map(String::as_str).find(|id| in_group(id)))
            .map_or(egui::Vec2::ZERO, |id| self.layout_pos(id));
        let pos = parent_pos + graph_layout::spawn_jitter(&id);
        let spawn = TabSpawn::new(id.clone(), url.to_string(), kind, pos, purpose, self.settings.zoom_factor);
        let jar = jar.unwrap_or_else(|| self.new_tab_jar(group.as_deref()));
        self.spawn_tab_async(TabSpawn { parent, group, jar, ..spawn });
        id
    }

    /// Stops the focused tab rendering (and drops its find bar, context
    /// menu, and split) -- what switching the maximized node away from it
    /// needs, whether to another tab (`focus_tab`) or an Image
    /// (`focus_item`). `focused` itself is left alone.
    pub(crate) fn hide_focused_tab(&mut self) {
        self.close_find_bar();
        self.menus.context_menu = None;
        self.close_split();
        if let Some(old_id) = self.focused.clone() {
            if let Some(old) = self.nodes.get_mut(&old_id).and_then(|n| n.as_tab_mut()) {
                old.tab.set_visible(false);
            }
            if let Some(gpu) = &mut self.gpu {
                gpu.free_tab_texture(&old_id);
            }
        }
    }

    /// Peek-click commits focus: the tab losing focus (if any, and if
    /// different) is told to stop rendering, the newly focused one is told
    /// to start, and the view switches to Tab. At most one tab is ever the
    /// primary/focused one at a time -- a split's own secondary pane
    /// (`split_tab`) is the one exception, also visible but never focused;
    /// switching primary focus to a genuinely different tab always closes
    /// whatever split was open, since a split is only ever meaningful
    /// paired with the specific tab it was opened against. An unloaded tab
    /// (`UnloadedTabPayload`) is woken.
    pub(crate) fn focus_tab(&mut self, id: &str) {
        self.end_note_edits();
        if self.focused.as_deref() != Some(id) {
            self.hide_focused_tab();
        }
        if let Some(node) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) {
            node.tab.set_visible(true);
            if !self.address_bar.focused {
                self.address_bar.text = node.url.clone();
            }
        }
        self.focused = Some(id.to_string());
        self.viewed_image = None;
        self.view_mode = ViewMode::Item;
        // An unloaded tab opens by starting its engine; Tab view shows the
        // respawning placeholder until it lands (`pending_respawn_focus`).
        if self.nodes.get(id).and_then(|n| n.as_unloaded()).is_some() {
            self.spawns.pending_respawn_focus = Some(id.to_string());
            self.wake_tab(id, self.mirror.engine(id));
        }
        self.send_resize_to(id);
        self.save_session();
    }

    /// Maximizes `id` into Item view, whatever its kind -- a Tab goes
    /// through `focus_tab`; an Image becomes `viewed_image` (the last tab
    /// stays `focused`, hidden, exactly as in Graph view). Kinds with no
    /// Item view are a no-op (and a debug-build assertion).
    pub(crate) fn focus_item(&mut self, id: &str) {
        let Some(kind) = self.card_kind(id) else { return };
        match kind.ty {
            crate::store::model::TYPE_TAB => self.focus_tab(id),
            crate::store::model::TYPE_IMAGE => {
                self.end_note_edits();
                self.hide_focused_tab();
                self.viewed_image = Some(id.to_string());
                self.image_view = ImageViewState::default();
                self.view_mode = ViewMode::Item;
                self.save_session();
            }
            ty => debug_assert!(false, "focus_item: {ty} nodes have no Item view"),
        }
    }

    /// Opens (or replaces) split view: the Tab-view toolbar's "Split"
    /// picker -- spawns a new tab at the focused tab's own current url on
    /// `kind`, parented to it and sharing its group, shown as the secondary
    /// pane once it finishes spawning (`SpawnPurpose::Split` -- see
    /// `handle_tab_spawn_result`). A no-op if there's no focused tab to
    /// compare against. Replacing an already-open split just spawns a
    /// fresh secondary tab and overwrites `split_tab` once *that* finishes
    /// spawning -- the old secondary tab is left exactly as `close_split`
    /// would leave it (unpaired, but not closed), since a real tab node no
    /// longer being shown in a split isn't otherwise different from any
    /// other background tab.
    pub(crate) fn start_split(&mut self, kind: EngineKind) {
        let Some(focused_id) = self.focused.clone() else { return };
        let Some(url) = self.nodes.get(&focused_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) else {
            return;
        };
        let group = self.group_of(&Some(focused_id.clone()));
        let jar = self.tab_jar(&focused_id);
        self.spawn_new_tab_with_kind(Some(focused_id), group, &url, kind, SpawnPurpose::Split, jar);
    }

    /// Closes split view -- unpairs the secondary pane and stops its
    /// rendering/GPU texture, but doesn't close the tab itself: it remains
    /// a normal graph node, reachable and closable from Graph view like any
    /// other tab. A no-op if no split is open.
    pub(crate) fn close_split(&mut self) {
        let Some(id) = self.split_tab.take() else { return };
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.tab.set_visible(false);
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_texture(&id);
        }
        // The primary pane was rendering at half width for the split --
        // needs to grow back to the full content width now that
        // `content_size_px` no longer sees `split_tab` as set.
        if let Some(focused_id) = self.focused.clone() {
            self.send_resize_to(&focused_id);
        }
    }

    /// Leaves Item view for Graph view -- starts a shrink animation (see
    /// `Transition`) toward whichever node is shown, rather than switching
    /// instantly. For a Tab (`viewed_image` is `None`), the tab keeps
    /// rendering (`set_visible(true)`, unchanged) for the animation's whole
    /// duration, so the thumbnail it shrinks into shows real, current
    /// content instead of whatever frame happened to be on screen when the
    /// animation started, only actually suspended once the animation
    /// finishes (`finish_transition_if_done`). For an Image, the texture is
    /// already there either way, so there's nothing equivalent to suspend.
    pub(crate) fn leave_to_graph(&mut self) {
        self.menus.context_menu = None;
        match self.viewed_image.clone().or_else(|| self.focused.clone()) {
            Some(id) => {
                self.transition =
                    Some(Transition { node_id: id, kind: TransitionKind::ToGraph, start: std::time::Instant::now() });
            }
            // No focused tab and no viewed image (shouldn't normally
            // happen — Item view always has one — but nothing to animate
            // either way).
            None => {
                self.view_mode = ViewMode::Graph;
                self.save_session();
            }
        }
    }

    /// Closes a tab: tears down its process (via `EngineTab`'s `Drop` —
    /// `Shutdown` + kill), frees its graph-view thumbnail, and reparents
    /// its children to its own parent so the forest stays connected rather
    /// than orphaning that whole subtree. Falls back to Graph view if the
    /// closed tab was focused. Records nothing: `close_node` is the close
    /// that feeds "Recently Closed".
    pub(crate) fn destroy_tab(&mut self, id: &str) {
        self.remove_tab(id);
        self.save_session();
    }

    /// Second half of "Sleep": shuts a live tab's engine down and keeps its
    /// card, as the unloaded tab a restore makes (`UnloadedState::Sleeping`):
    /// same id, place, url, pin, mute and zoom, and it saves as the same tab.
    /// Opening it starts the engine again (`wake_tab`) and reloads the url;
    /// `scroll`, when not the origin, is put back once that load finishes. The
    /// tab Tab view shows and the split pane's tab stay live. Returns whether
    /// it slept. Callers go through `request_sleep`.
    fn sleep_now(&mut self, id: &str, scroll: Option<(f64, f64)>) -> bool {
        let open = self.showing_tab_view() && self.focused.as_deref() == Some(id);
        if open || self.split_tab.as_deref() == Some(id) {
            return false;
        }
        let Some(node) = self.nodes.get_mut(id) else { return false };
        let Runtime::Tab(tab) = &node.runtime else { return false };
        let asleep = unloaded_from(tab, UnloadedState::Sleeping);
        // Replacing the payload drops the `EngineTab`, whose `Drop` shuts
        // the engine host down (see `teardown_running_session`).
        node.runtime = Runtime::Unloaded(asleep);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
        }
        // A scroll left from an earlier wake that never finished loading is stale.
        self.saved_scroll.remove(id);
        if let Some((x, y)) = scroll {
            if x != 0.0 || y != 0.0 {
                self.saved_scroll.insert(id.to_string(), SavedScroll::new(x, y));
            }
        }
        self.save_session();
        true
    }

    /// A tab whose engine keeps crashing (`crashes_too_often`), or speaks
    /// another protocol version, becomes a failed card: the reason is its
    /// tooltip, and opening it tries again. Respawning would only loop.
    /// `why` picks the toast: `CannotStart` says why the host could not start.
    pub(crate) fn leave_failed_card(&mut self, id: &str, reason: &str, why: LeftClosed) {
        let Some(node) = self.nodes.get_mut(id) else { return };
        let Runtime::Tab(tab) = &node.runtime else { return };
        let (url, kind) = (tab.url.clone(), tab.tab.kind);
        // Replacing the payload drops the `EngineTab`, which reaps the dead host.
        node.runtime = Runtime::Unloaded(unloaded_from(tab, UnloadedState::Failed(reason.to_string())));
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
        }
        // Tab view, focused on this card, falls back to Graph by itself
        // (`build_tab_view_ui`) unless a stale respawn spinner names it; a
        // split pane showing it has nothing left to show.
        if self.spawns.pending_respawn_focus.as_deref() == Some(id) {
            self.spawns.pending_respawn_focus = None;
        }
        if self.split_tab.as_deref() == Some(id) {
            self.close_split();
        }
        self.spawns.tab_crashes.remove(id);
        let group = self.mirror.group(id).map(str::to_owned);
        // One failed card per intent: keep this one (its id is the live one) and drop another standing for it.
        if let Some(twin) = self.failed_twin(id, &url, kind, group.as_deref()) {
            self.record_closed(&twin);
            self.remove_tab(&twin);
        }
        match why {
            LeftClosed::CannotStart => {
                let message = format!("Couldn't start {} for {}: {reason}", engine_label(kind), Self::toast_url(&url));
                self.queue_failure_toast(kind, &url, group.as_deref(), reason, message);
            }
            LeftClosed::KeepsCrashing => {
                self.queue_toast(format!("Tab keeps crashing, left closed: {}", Self::toast_url(&url)))
            }
            LeftClosed::Ended => self.queue_toast(format!("Ended (not responding): {}", Self::toast_url(&url))),
        }
        self.save_session();
    }

    /// The spawn that restarts live tab `id` on the engine the mirror holds for it: a swap in flight (`swap_target`,
    /// or one waiting for its cookies) has already saved its target, so the tab restarts on it, the running engine
    /// its fallback. `None` when `id` is no live tab.
    pub(crate) fn restart_spawn(&self, id: &str) -> Option<TabSpawn> {
        let want = self.mirror.engine(id);
        let node = self.nodes.get(id)?;
        let tab = node.as_tab()?;
        let fallback_kind = (tab.tab.kind != want).then_some(tab.tab.kind);
        let spawn = TabSpawn::respawn(id, node, self.mirror.carried(id), want, tab.url.clone())?;
        Some(TabSpawn { fallback_kind, ..spawn })
    }

    /// Replaces live tab `id`'s engine with one started from `spawn` (`restart_spawn`). Dropping the old `EngineTab`
    /// ends its host.
    pub(crate) fn restart_with(&mut self, id: &str, spawn: TabSpawn) {
        self.nodes.remove(id);
        // Same stale-state guard `destroy_tab`/`finish_swap` apply — see `finish_swap`'s own comment on this.
        if self.interaction.dragging.as_deref() == Some(id) {
            self.interaction.dragging = None;
        }
        if self.interaction.hovered.as_deref() == Some(id) {
            self.interaction.hovered = None;
        }
        self.respawn_tab(spawn);
    }

    /// "Restart Engine": a new host for live tab `id` at its current URL, as a crash restart does, without counting
    /// toward the crash limit. False when `id` is no live tab.
    pub(crate) fn restart_engine(&mut self, id: &str) -> bool {
        let Some(spawn) = self.restart_spawn(id) else { return false };
        self.queue_toast(format!("Restarted: {}", Self::toast_url(&spawn.url)));
        self.restart_with(id, spawn);
        true
    }

    /// "End Engine": ends live tab `id`'s host and leaves a failed card, which a click starts again. False when `id`
    /// is no live tab.
    pub(crate) fn end_engine(&mut self, id: &str) -> bool {
        if self.nodes.get(id).and_then(|n| n.as_tab()).is_none() {
            return false;
        }
        self.leave_failed_card(id, "It stopped responding and was ended.", LeftClosed::Ended);
        true
    }

    /// First phase of every sleep: validates as `sleep_now` does, asks the
    /// engine for its page state, and notes the request. `finish_sleep`
    /// completes it when `EngineToHost::PageState` arrives;
    /// `expire_pending_sleeps` (run by `sweep_idle_tabs`) settles requests
    /// nobody answered. Returns whether a query was sent.
    pub(crate) fn request_sleep(&mut self, id: &str, why: SleepWhy) -> bool {
        let open = self.showing_tab_view() && self.focused.as_deref() == Some(id);
        // A request nobody answered must not block a new one.
        self.expire_pending_sleeps(std::time::Instant::now());
        if open || self.split_tab.as_deref() == Some(id) || self.pending_sleeps.contains_key(id) {
            return false;
        }
        if self.nodes.get(id).and_then(|n| n.as_tab()).is_none() {
            return false;
        }
        self.pull_cookies(id);
        let Some(tab) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) else { return false };
        tab.tab.send(&HostToEngine::QueryPageState);
        self.pending_sleeps.insert(id.to_string(), PendingSleep { why, sent: std::time::Instant::now() });
        true
    }

    /// Second phase: the engine's answer. Idle sleep stays awake for a dirty
    /// form (and restarts that tab's idle clock); otherwise the tab sleeps,
    /// keeping its scroll position for the wake.
    pub(crate) fn finish_sleep(&mut self, id: &str, state: PageState) {
        let Some(pending) = self.pending_sleeps.remove(id) else { return };
        match sleep_state::verdict(pending.why, &state) {
            Verdict::Stay => self.idle_clock.restart(id, std::time::Instant::now()),
            Verdict::Sleep => {
                self.sleep_now(id, Some((state.scroll_x, state.scroll_y)));
            }
        }
    }

    /// Settles requests the engine never answered (hung page, no script
    /// context). A manual one sleeps the tab anyway, without a scroll to
    /// restore, so Sleep still works on a stuck tab; an idle or jar-switch one
    /// is dropped and the tab's clock restarts so it is not asked again every
    /// sweep (a waiting jar switch is asked again by the next sweep).
    fn expire_pending_sleeps(&mut self, now: std::time::Instant) {
        let dead: Vec<(String, SleepWhy)> = self
            .pending_sleeps
            .iter()
            .filter(|(_, p)| sleep_state::expired(p, now))
            .map(|(id, p)| (id.clone(), p.why))
            .collect();
        for (id, why) in dead {
            self.pending_sleeps.remove(&id);
            match why {
                SleepWhy::Manual => {
                    self.sleep_now(&id, None);
                }
                SleepWhy::Idle | SleepWhy::JarSwitch => self.idle_clock.restart(&id, now),
            }
        }
    }

    /// Idle auto-sleep (`idle_sleep`): updates every live tab's clock and
    /// asks those out of sight for `limit` to sleep (`request_sleep`). `limit`
    /// is the setting's `idle_sleep_minutes`, or the headless `IDLESWEEP`'s
    /// seconds.
    /// Each sweep also switches the tabs whose jar change was waiting and that now qualify (`sweep_jar_switches`).
    pub(crate) fn sweep_idle_tabs(&mut self, limit: Option<std::time::Duration>) {
        let now = std::time::Instant::now();
        self.expire_pending_sleeps(now);
        // Taken out so the views can borrow the rest of `self`.
        let mut clock = std::mem::take(&mut self.idle_clock);
        let idle = clock.sweep(now, limit, self.tab_idle_views());
        self.idle_clock = clock;
        for id in idle {
            self.request_sleep(&id, SleepWhy::Idle);
        }
        self.sweep_jar_switches();
    }

    /// What the idle and jar-switch sweeps need to know about each live tab.
    fn tab_idle_views(&self) -> Vec<crate::idle_sleep::TabIdleView<'_>> {
        let on_screen = self.showing_tab_view().then_some(self.focused.as_deref()).flatten();
        // A download card's parent is the tab it came from; `completed` flips when the engine finishes it.
        let downloading: std::collections::HashSet<&str> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.as_download().is_some_and(|d| !d.completed && !d.failed))
            .filter_map(|(id, _)| self.mirror.parent(id))
            .collect();
        self.nodes
            .iter()
            .filter_map(|(id, n)| {
                let t = n.as_tab()?;
                Some(crate::idle_sleep::TabIdleView {
                    id,
                    on_screen: on_screen == Some(id.as_str()) || self.split_tab.as_deref() == Some(id),
                    audio_playing: t.audio_playing,
                    pinned: self.mirror.pinned(id),
                    loading: t.loading,
                    downloading: downloading.contains(id.as_str()),
                })
            })
            .collect()
    }

    /// A live WebView2 or CEF tab running on a jar other than the one it now resolves to, or on a jar the user
    /// cleared (`jar_reseed`): it switches when it next qualifies (chip "will switch").
    pub(crate) fn will_switch(&self, id: &str) -> bool {
        let Some(tab) = self.nodes.get(id).and_then(|n| n.as_tab()) else { return false };
        let Some((had, _)) = self.cookies.jar_tabs.get(id) else { return false };
        matches!(tab.tab.kind, EngineKind::Webview2 | EngineKind::Cef)
            && (*had != self.resolved_jar(&self.mirror.jar_choice(id)).0 || self.cookies.jar_reseed.contains(id))
    }

    /// Puts every live tab whose jar changed and that is not busy, focused or showing to sleep; it wakes into the
    /// new jar (spec table). Runs after a local jar change, a remote batch (`apply_remote`) and each idle sweep, so a
    /// waiting tab switches once it qualifies (deviation 11). A dirty form keeps it awake (`sleep_state::verdict`).
    pub(crate) fn sweep_jar_switches(&mut self) {
        let todo: Vec<String> = self
            .tab_idle_views()
            .iter()
            .filter(|v| self.will_switch(v.id))
            .filter(|v| {
                if self.nodes.get(v.id).and_then(|n| n.as_tab()).is_none() {
                    return false;
                }
                let had = self.cookies.jar_tabs.get(v.id).map(|(k, _)| *k);
                let focused = self.focused.as_deref() == Some(v.id);
                let reseed = self.cookies.jar_reseed.contains(v.id);
                let now = self.resolved_jar(&self.mirror.jar_choice(v.id)).0;
                sleep_state::jar_switch(had, now, reseed, v.busy(), focused) == sleep_state::JarSwitch::SleepNow
            })
            .map(|v| v.id.to_string())
            .collect();
        for id in todo {
            self.request_sleep(&id, SleepWhy::JarSwitch);
        }
    }

    /// "Switch jar now": the old jar gets the tab's final cookies, then the host is replaced in place by one seeded
    /// from the new jar (the engine-swap path, same engine). Does nothing while another host change is on its way
    /// (`host_change_pending`): that one replaces the host anyway.
    pub(crate) fn switch_jar_now(&mut self, id: &str) {
        let Some((kind, url)) = self.nodes.get(id).and_then(|n| n.as_tab()).map(|t| (t.tab.kind, t.url.clone())) else {
            return;
        };
        if !self.can_switch_jar_now(id) {
            return;
        }
        self.pull_cookies(id);
        if let Some(held) = self.hold_for_cookies(id, Held::Snapshot(kind, url)) {
            self.run_held(id, held);
        }
    }

    /// Whether "Switch Jar Now" would do anything for tab `id`: it waits to switch and nothing else is about to
    /// replace its host. The menu hides the item otherwise.
    pub(crate) fn can_switch_jar_now(&self, id: &str) -> bool {
        self.will_switch(id) && !self.host_change_pending(id)
    }

    /// An engine swap, a sleep or an earlier "Switch Jar Now" is already on its way for tab `id` (Task 6 review M3).
    /// Each replaces or shuts the host, and a second `hold_for_cookies` would cancel the one parked first.
    pub(crate) fn host_change_pending(&self, id: &str) -> bool {
        self.pending_sleeps.contains_key(id)
            || self.cookies.cookie_holds.contains_key(id)
            || self.nodes.get(id).and_then(|n| n.as_tab()).is_some_and(|t| t.swap_target.is_some())
    }

    /// Seeds a landing tab's mirror entry (`Mirror::seed`: only an id with none) and its layout entry, both at
    /// `carried.pos` (a new tab's spawn position, 2b-4 Ruling 3). A tab mid-respawn or waking keeps both: its layout
    /// entry is keyed by id, so the respawn's remove-and-reinsert leaves it where it was drawn (Ruling 1).
    ///
    /// A tab mid-respawn keeps its mirror entry
    /// and the link cascades reach it (`App::linked_ids`), but a new spawn has none until here: one whose group was
    /// removed while it was in flight would land in a group nothing can navigate into. It lands where the document
    /// shows it instead, in the mirror only (`follow_remote_group`): the removal was a cascade, not a move of this
    /// tab, and a queued move would override another replica's placement of it (final review I1).
    ///
    /// Likewise a jar deleted while a new spawn was in flight (`delete_jar` cannot reach a tab with no entry): the
    /// tab lands on the default jar, as the delete would have put it, and the document is told so whether its
    /// `Create` already named the jar or resolved it to none (2b-3 final review Minor 1).
    fn seed_landing(&mut self, id: &str, carried: Carried) {
        let ty = crate::store::model::Kind::Tab.type_str().unwrap_or_default().to_owned();
        let carried = Carried { ty, ..carried };
        self.layout.entry(id.to_owned()).or_insert(Layout::at(carried.pos.unwrap_or_default()));
        let new = !self.mirror.has(id);
        self.mirror.seed(id, &carried);
        if new && self.mirror.jar_choice(id).jar.is_some_and(|jar| !self.mirror.jar_defs().contains_key(&jar)) {
            self.edit_jar(id, crate::jar::JarChoice::default());
        }
        if self.mirror.group(id).is_some_and(|g| !self.nodes.contains_key(g)) {
            let group = self.shown_group(id);
            self.follow_remote_group(id, group);
        }
    }

    /// The nearest group above `id` in the document that is here: `Doc::effective_group`, except that a group this
    /// replica removed and hasn't saved yet is passed over too, as the save that deletes it will. `None` (the top
    /// level) for a node the document doesn't hold yet.
    fn shown_group(&self, id: &str) -> Option<String> {
        let doc = self.journal.as_ref()?.doc();
        let mut group = doc.nodes.get(&id.parse::<crate::store::model::NodeId>().ok()?)?.group;
        for _ in 0..=doc.nodes.len() {
            let at = group?;
            if self.nodes.contains_key(&at.to_string()) {
                return Some(at.to_string());
            }
            group = doc.nodes.get(&at)?.group;
        }
        None
    }

    /// "Sleep Tabs" on a group: `request_sleep` for each of its live tabs.
    /// Returns how many were requested.
    pub(crate) fn sleep_group(&mut self, id: &str) -> usize {
        let members: Vec<String> = self.members_of(id).map(str::to_owned).collect();
        members.iter().filter(|m| self.request_sleep(m, SleepWhy::Manual)).count()
    }

    /// The local close without the save (`destroy_tab`, `destroy_group_members`, `redirect_tab_to_content`); queues
    /// the delete.
    pub(crate) fn remove_tab(&mut self, id: &str) {
        if self.drop_tab(id) {
            self.queue_delete(id);
        }
    }

    /// `remove_tab` without the queued delete, for a tab another replica deleted (`remove_remotely_deleted`).
    /// False when `id` is not a tab.
    pub(crate) fn drop_tab(&mut self, id: &str) -> bool {
        if !self.nodes.get(id).is_some_and(|n| n.runtime.saved_tab().is_some()) {
            return false;
        }
        let parent = self.mirror.parent(id).map(str::to_owned);
        let children: Vec<String> = self.children_of(id).map(str::to_owned).collect();
        for child in children {
            self.edit_parent(&child, parent.clone());
        }
        self.nodes.remove(id);
        self.node_order.retain(|t| t != id);
        self.forget_id(id);
        // A split only ever makes sense paired with a specific focused tab
        // (see `focus_tab`'s own doc comment) -- closing either half of it
        // (the tab node is already gone either way, so no `set_visible`/
        // resize cleanup to do the way `close_split` normally would) clears
        // the whole thing rather than leaving a dangling reference to a
        // node that no longer exists.
        if self.split_tab.as_deref() == Some(id) || self.focused.as_deref() == Some(id) {
            self.split_tab = None;
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
            // Only drop to Graph if a Tab was actually showing -- while an
            // Image is maximized, `focused` is just the hidden last tab
            // (see the Image Item View spec's "Plan-time corrections"), so
            // closing it should leave Image view showing exactly as
            // closing it from Graph view wouldn't change `view_mode`
            // either.
            if self.showing_tab_view() {
                self.view_mode = ViewMode::Graph;
            }
            // The tab (and its `EngineTab`) is already gone — nothing left
            // to send `FindStop` to, so only the UI side of the close.
            self.find.close();
        }
        true
    }

    /// Shared by swap-finish and crash recovery: starts `spawn` (from
    /// `TabSpawn::respawn`, so the tab keeps its `TabCarry`; its place in the
    /// graph is its `App::layout` entry) and, if it's the focused tab, shows a spinner until it's
    /// back (only the focused tab is ever visible).
    pub(crate) fn respawn_tab(&mut self, spawn: TabSpawn) {
        if self.focused.as_deref() == Some(spawn.id.as_str()) {
            self.spawns.pending_respawn_focus = Some(spawn.id.clone());
        }
        self.spawn_tab_async(spawn);
    }

    /// A new tab at `url`, placed at `pos` in `group`, opened from `parent` (if any). Returns its id.
    pub(crate) fn spawn_tab_at(
        &mut self,
        url: &str,
        pos: egui::Vec2,
        group: Option<String>,
        parent: Option<String>,
    ) -> String {
        let id = self.alloc_node_id();
        let (kind, zoom) = (self.settings.default_starting_engine, self.settings.zoom_factor);
        let spawn = TabSpawn::new(id.clone(), url.to_string(), kind, pos, SpawnPurpose::New, zoom);
        let jar = self.new_tab_jar(group.as_deref());
        self.spawn_tab_async(TabSpawn { group, parent, jar, ..spawn });
        id
    }

    /// `Ctrl+Tab`/`Ctrl+Shift+Tab`: focuses the next/previous tab in
    /// `tab_order` (insertion order — this graph-based UI has no tab strip
    /// to define a more meaningful order), wrapping at both ends. A no-op
    /// with zero or one tab open.
    pub(crate) fn cycle_focused_tab(&mut self, forward: bool) {
        // `node_order` holds every graph node (tabs, notes, images, PDFs,
        // JSON files), not just tabs -- filter to live and unloaded tabs
        // first (same filter `quick_switcher_matches` applies) so this
        // never lands on a non-tab node. Landing on an unloaded tab is
        // fine: `focus_tab` wakes it, and `render_tab_toolbar_content`
        // shows the respawning placeholder until it's live rather than
        // assuming `as_tab().unwrap()`.
        let tab_ids: Vec<&String> = self
            .node_order
            .iter()
            .filter(|id| self.nodes.get(id.as_str()).and_then(|n| n.runtime.listed_tab()).is_some())
            .collect();
        if tab_ids.len() < 2 {
            return;
        }
        let current = self.focused.as_ref().and_then(|id| tab_ids.iter().position(|t| *t == id));
        let len = tab_ids.len() as isize;
        let next = match current {
            Some(i) => {
                let delta = if forward { 1 } else { -1 };
                (i as isize + delta).rem_euclid(len) as usize
            }
            None => 0,
        };
        let id = tab_ids[next].clone();
        self.focus_tab(&id);
    }
}

#[cfg(test)]
mod crash_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_third_crash_within_the_window_gives_up() {
        let t0 = Instant::now();
        let mut times = Vec::new();
        assert!(!crashes_too_often(&mut times, t0));
        assert!(!crashes_too_often(&mut times, t0 + Duration::from_secs(1)));
        assert!(crashes_too_often(&mut times, t0 + Duration::from_secs(2)));
    }

    #[test]
    fn a_host_that_cannot_start_is_not_restarted_and_says_why() {
        let t0 = Instant::now();
        let mut times = Vec::new();
        let why = "WebView2 couldn't create its environment (0x80070005: Access is denied.)";
        let fatal = format!("{}{why}", ipc_protocol::HOST_FATAL);
        assert_eq!(after_crash(&fatal, &mut times, t0), AfterCrash::CannotStart(why));
        assert!(times.is_empty(), "not counted as a crash");
        let mismatch = format!("{}: host 2, engine 3", ipc_protocol::PROTOCOL_MISMATCH);
        assert_eq!(after_crash(&mismatch, &mut times, t0), AfterCrash::GiveUp);
        // A plain crash still restarts, up to the cap.
        let lost = "engine-host connection lost";
        assert_eq!(after_crash(lost, &mut times, t0), AfterCrash::Restart);
        assert_eq!(after_crash(lost, &mut times, t0 + Duration::from_secs(1)), AfterCrash::Restart);
        assert_eq!(after_crash(lost, &mut times, t0 + Duration::from_secs(2)), AfterCrash::GiveUp);
    }

    #[test]
    fn crashes_spread_past_the_window_keep_respawning() {
        let t0 = Instant::now();
        let mut times = Vec::new();
        for i in 0..10 {
            assert!(!crashes_too_often(&mut times, t0 + CRASH_WINDOW * i), "crash {i}");
        }
    }
}

#[cfg(test)]
mod tests {
    /// `SpawnTracker::forget` takes what `forget_id` cleared by hand (`skip_convert`, `navigated_from`,
    /// `tab_crashes`, a matching `pending_respawn_focus`) and `reset` what `teardown_running_session` cleared (the
    /// spawns in flight, `host_loads`, `navigated_from`) plus a generation bump; neither touches the rest.
    #[test]
    fn forget_and_reset_clear_what_the_old_sites_cleared() {
        let now = std::time::Instant::now();
        let filled = || {
            let mut s = SpawnTracker::default();
            s.skip_convert.insert("t".into(), true);
            s.navigated_from.insert("t".into(), ("u".into(), now, false));
            s.tab_crashes.insert("t".into(), vec![now]);
            s.pending_respawn_focus = Some("t".into());
            s.wakes_in_flight.insert("t".into());
            s.host_loads.insert("t".into(), now);
            s.remote_in_flight.insert("t".into(), None);
            s.tab_spawns_in_flight = 2;
            s
        };
        let mut s = filled();
        s.forget("u");
        assert!(s.skip_convert.contains_key("t") && s.pending_respawn_focus.is_some(), "another id's state stays");
        s.forget("t");
        assert!(s.skip_convert.is_empty() && s.navigated_from.is_empty() && s.tab_crashes.is_empty());
        assert!(s.pending_respawn_focus.is_none());
        assert!(
            s.wakes_in_flight.contains("t") && s.host_loads.contains_key("t") && s.remote_in_flight.contains_key("t")
        );

        let mut s = filled();
        s.reset();
        assert!(s.wakes_in_flight.is_empty() && s.host_loads.is_empty() && s.remote_in_flight.is_empty());
        assert!(s.spawning.is_empty() && s.navigated_from.is_empty());
        assert_eq!(s.session_generation, 1);
        assert!(
            s.skip_convert.contains_key("t") && s.tab_crashes.contains_key("t") && s.pending_respawn_focus.is_some()
        );
        assert_eq!(s.tab_spawns_in_flight, 2);
    }

    use super::*;

    #[test]
    fn finish_sleep_without_a_request_does_nothing() {
        let mut app = App::default();
        app.finish_sleep("t1", PageState { scroll_x: 0.0, scroll_y: 10.0, dirty_form: false });
        assert!(app.pending_sleeps.is_empty() && app.saved_scroll.is_empty());
    }

    #[test]
    fn request_sleep_needs_a_live_tab() {
        let mut app = App::default();
        assert!(!app.request_sleep("missing", SleepWhy::Manual));
        assert!(app.pending_sleeps.is_empty());
    }

    #[test]
    fn unanswered_requests_are_settled_after_the_timeout() {
        let mut app = App::default();
        let t0 = std::time::Instant::now();
        for (id, why) in [("m", SleepWhy::Manual), ("i", SleepWhy::Idle), ("j", SleepWhy::JarSwitch)] {
            app.pending_sleeps.insert(id.into(), PendingSleep { why, sent: t0 });
        }
        app.expire_pending_sleeps(t0 + sleep_state::PENDING_TIMEOUT - std::time::Duration::from_millis(1));
        assert_eq!(app.pending_sleeps.len(), 3);
        app.expire_pending_sleeps(t0 + sleep_state::PENDING_TIMEOUT);
        assert!(app.pending_sleeps.is_empty());
    }
}
