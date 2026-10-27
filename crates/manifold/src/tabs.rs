//! Tab lifecycle on App: spawn, respawn, focus, hide, split, leave to graph, destroy and remove. The cookie-jar pull
//! (`jar_pull`), landing a finished spawn (`spawn_result`) and sleeping (`sleep`) live in child modules.

use super::*;
use crate::store::model::NodeId;
use manifold_view_protocol::PageState;
use sleep_state::{PendingSleep, SavedScroll, SleepWhy, Verdict};

mod jar_pull;
mod page_text;
mod sleep;
mod spawn_result;
pub(crate) use jar_pull::Held;
#[cfg(test)]
pub(crate) use jar_pull::{pull_pending, start_pull, JAR_FLUSH_AFTER, JAR_PULL_TIMEOUT};
pub(crate) use page_text::PageTextTracker;

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
    pub(crate) spawning: HashMap<NodeId, session_sync::Snapshot>,

    /// Other replicas' latest change to a node in `spawning`, applied when
    /// its spawn lands (`Shown`: the change to apply; `Hidden`: hide it once it lands; `Deleted`: remove it). The
    /// spawn itself carries only what it was started with.
    pub(crate) remote_in_flight: HashMap<NodeId, session_sync::Remote<session_sync::Snapshot>>,

    /// Tabs loading a url the host gave them, with when each was last seen
    /// loading; their url changes aren't shared meanwhile
    /// (`App::shares_url_change`).
    pub(crate) host_loads: HashMap<NodeId, std::time::Instant>,

    /// Tabs `navigate_tab` just moved on, with the url each left, when, and
    /// whether the engine has reported loading since (`App::reported_url`).
    pub(crate) navigated_from: HashMap<NodeId, (String, std::time::Instant, bool)>,

    pub(crate) tabs_spawned_waiters: crate::debug_control::Waiters,

    /// Bumped by `replace_running_session`. A tab spawn carries the value it
    /// started under, and `handle_tab_spawn_result` drops a result from an
    /// older one: it belongs to the replaced session, and inserting it would
    /// add a stray tab to the new scene, so a spawn from a replaced session
    /// never lands in it.
    pub(crate) session_generation: u64,

    /// Recent engine crash times per tab id (`tabs::crashes_too_often`): a
    /// tab crashing too often is left as a failed card, not respawned.
    pub(crate) tab_crashes: HashMap<NodeId, Vec<std::time::Instant>>,

    /// The tab id `respawn_tab` was called for *while it was the focused
    /// tab* — i.e. an engine swap or crash recovery is currently respawning
    /// the tab the user is looking at. `build_tab_view_ui`'s "focused
    /// tab missing from `self.nodes`" guard would otherwise bounce the user
    /// to Graph view for the tab's *entire* respawn duration, a visible
    /// multi-second flicker back to Graph and then back to Tab view when a
    /// WebKit respawn takes many seconds. Set in `respawn_tab`, cleared in
    /// `handle_tab_spawn_result` once that same id's `Respawn` outcome
    /// (`Ok` or `Err`) arrives.
    pub(crate) pending_respawn_focus: Option<NodeId>,

    /// The opened-from step's target (trail spec decision 6) while a reopen has not put it in `nodes` yet;
    /// `App::poll_trail_focus` finishes it.
    pub(crate) pending_trail_focus: Option<NodeId>,

    /// Unloaded tabs' cards whose `wake_tab` spawn is in flight. Unlike a
    /// crash respawn, the card is still on the graph meanwhile, so it can
    /// be moved or closed: the outcome takes the card's placement as it is
    /// now, and is dropped if the card is gone. Set in `wake_tab`, cleared
    /// by that outcome in `handle_tab_spawn_result`.
    pub(crate) wakes_in_flight: HashSet<NodeId>,

    /// Tabs that stay tabs whatever they load, each with whether that load
    /// has settled (reported `loading == false`): one opened by "Open as
    /// tab", and one loading another replica's url (`materialize`), which
    /// that replica converts itself. Every `RedirectableContent`/`JsonBody`
    /// report is skipped while a tab is in here. It stays until a real url
    /// change after the load settles (an HTTP redirect during it doesn't
    /// count; see `skip_convert_after`) or the tab closes (`remove_tab`).
    /// Not persisted (spec "Left open").
    pub(crate) skip_convert: HashMap<NodeId, bool>,
}

impl SpawnTracker {
    /// Drops `id`, a node being removed for good, from the per-tab state that is not about a spawn in flight.
    /// `spawning`, `remote_in_flight`, `wakes_in_flight` and `host_loads` are not here: the spawn lifecycle owns them.
    pub(crate) fn forget(&mut self, id: NodeId) {
        self.skip_convert.remove(&id);
        self.navigated_from.remove(&id);
        self.tab_crashes.remove(&id);
        if self.pending_respawn_focus == Some(id) {
            self.pending_respawn_focus = None;
        }
        if self.pending_trail_focus == Some(id) {
            self.pending_trail_focus = None;
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
        self.pending_trail_focus = None;
        self.session_generation += 1;
    }
}

/// Which call site triggered a tab spawn — `handle_tab_spawn_result` uses
/// this to know what to do once the async `process_manager::spawn_tab`
/// call finishes.
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
/// for why this exists: spawning WebKit's view-host blocks on a cold WSL2
/// boot + Xvfb + WebKitGTK init (confirmed live to take several real seconds
/// on first use, occasionally much longer — see `process_manager::
/// spawn_webkit_tab`'s own doc comment), which would otherwise freeze the
/// whole app (redraw, input, headless stdin) for that entire duration if run
/// on this thread.
pub(crate) struct TabSpawnOutcome {
    pub(crate) spawn: TabSpawn,
    /// `App::session_generation` when the spawn started -- see that field.
    pub(crate) generation: u64,
    pub(crate) result: Result<ViewTab, process_manager::SpawnError>,
    /// The cookies the host was seeded with, as its starting `jar::TabCookies` (empty for hosts that are not).
    pub(crate) seeded: jar::TabCookies,
    /// The jar the host was seeded from, resolved when the spawn started: its pulls merge into this one.
    pub(crate) jar: Option<store::model::NodeId>,
    /// That jar's clear count when the seed was read (`jar::Snapshots::seed`); `None` if it never was.
    pub(crate) seed_clears: Option<u64>,
}

/// What `spawn_tab_async` needs to start a tab and, once it's up, to
/// insert its graph node: `TabSpawn::new` for a new tab (no parent or
/// group, nothing carried), `TabSpawn::respawn` for one that
/// already has a node.
pub(crate) struct TabSpawn {
    pub(crate) id: NodeId,
    pub(crate) url: String,
    pub(crate) kind: ViewKind,
    pub(crate) parent: Option<NodeId>,
    pub(crate) group: Option<NodeId>,
    /// The `jar` edge a new tab starts with, which it lands with (`Carried::jar`); `None` inherits. A tab the mirror
    /// already holds (a respawn or wake) uses the mirror's instead, which an edit may change while it is in flight.
    pub(crate) jar: Option<NodeId>,
    /// Where a new tab lands: its layout entry and its document pos start there. A tab already
    /// placed (a respawn, a wake) keeps both (`App::layout` outlives the respawn, `Mirror::seed` never overwrites).
    pub(crate) pos: egui::Vec2,
    pub(crate) purpose: SpawnPurpose,
    pub(crate) carry: TabCarry,
    /// The engine to respawn with if this spawn fails: a swap's old
    /// engine, whose process `finish_swap` has already had to drop.
    pub(crate) fallback_kind: Option<ViewKind>,
}

/// What a tab keeps across a respawn (engine swap, crash recovery) or a
/// wake. A new tab starts unpinned, at the default zoom, with no
/// title or mute of its own yet. A new attribute that must survive a
/// respawn or wake is one more field here.
pub(crate) struct TabCarry {
    /// See `Mirror::pinned`; seeds the mirror when the tab's id has no entry yet.
    pub(crate) pinned: bool,
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
        id: NodeId,
        url: String,
        kind: ViewKind,
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
            carry: TabCarry { pinned: false, zoom_factor, title: String::new(), muted: false },
            fallback_kind: None,
        }
    }

    /// Tab `node` (`id`) respawned as `kind` at `url`, keeping its `TabCarry` (`carried` is the mirror's); its place
    /// in the graph is its `App::layout` entry, which the respawn leaves alone. `None` if `node` isn't a tab.
    pub(crate) fn respawn(id: NodeId, node: &GraphNode, carried: Carried, kind: ViewKind, url: String) -> Option<Self> {
        let tab = node.as_tab()?;
        Some(TabSpawn {
            id,
            url,
            kind,
            parent: carried.parent,
            group: carried.group,
            jar: carried.jar(),
            pos: carried.pos().unwrap_or_default(),
            purpose: SpawnPurpose::Respawn,
            carry: TabCarry {
                pinned: carried.pinned(),
                zoom_factor: tab.zoom_factor,
                title: tab.title.clone(),
                muted: tab.muted,
            },
            fallback_kind: None,
        })
    }
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
    /// "End View" on a tab that stopped responding.
    Ended,
}

/// What to do about a tab whose engine reported `Crashed { reason }` at `now`.
#[derive(Debug, PartialEq)]
pub(crate) enum AfterCrash<'a> {
    Restart,
    /// Leave a failed card: the host speaks another protocol version, or keeps crashing (`crashes_too_often`).
    GiveUp,
    /// Leave a failed card naming why: the host could not start at all (its reason began with
    /// `manifold_view_protocol::HOST_FATAL`), so a respawn would fail the same way.
    CannotStart(&'a str),
}

/// Decides for `Crashed { reason }` at `now`; `times` is the tab's recent crashes, recorded only when it counts.
pub(crate) fn after_crash<'a>(
    reason: &'a str,
    times: &mut Vec<std::time::Instant>,
    now: std::time::Instant,
) -> AfterCrash<'a> {
    if let Some(why) = reason.strip_prefix(manifold_view_protocol::HOST_FATAL) {
        return AfterCrash::CannotStart(why.trim());
    }
    if reason.starts_with(manifold_view_protocol::PROTOCOL_MISMATCH) || crashes_too_often(times, now) {
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
        let home_page = self.settings.value.home_page.clone();
        let kind = self.settings.value.default_starting_view_kind;
        let zoom = self.settings.value.zoom_factor;
        self.spawn_tab_async(TabSpawn::new(id, home_page, kind, egui::Vec2::ZERO, SpawnPurpose::Root, zoom));
    }

    /// Runs `process_manager::spawn_tab` on a background thread and posts
    /// its result back via `event_proxy` as `UserEvent::TabSpawned` — see
    /// `TabSpawnOutcome`'s doc comment for why this can't run inline on the
    /// main/event-loop thread. `handle_tab_spawn_result` applies the `Ok`/`Err`
    /// outcome once the result arrives.
    pub(crate) fn spawn_tab_async(&mut self, spawn: TabSpawn) {
        self.spawns.tab_spawns_in_flight += 1;
        // Shared at the pos, url, engine and jar the mirror holds for it (a respawn's `spawn.url` is the live one); a
        // new tab has no entry yet and shares the ones it starts with.
        let shared_url = Some(self.doc.mirror.url(spawn.id)).filter(|url| !url.is_empty());
        let has = self.doc.mirror.has(spawn.id);
        let pos = if has { self.doc.mirror.pos(spawn.id) } else { Some(spawn.pos) };
        let own = if has { self.doc.mirror.jar(spawn.id) } else { spawn.jar };
        let view_kind = if has { self.doc.mirror.view_code(spawn.id) } else { session_sync::view_code(spawn.kind) };
        let url = shared_url.unwrap_or(&spawn.url);
        let mut snapshot = session_sync::spawn_snapshot(&spawn, pos, url, view_kind, own);
        snapshot.trees = self.doc.mirror.trees_of(spawn.id);
        self.spawns.spawning.insert(spawn.id, snapshot);
        // A new tab is created from that snapshot by the next save; a respawn's id is already stored.
        self.queue_create(spawn.id);
        let generation = self.spawns.session_generation;
        // Resolved and loaded before the test early return, so tests see the load happen.
        let jar = self.resolve_jar(own, spawn.group).map(|(jar, _)| jar);
        if let Some(jar) = jar {
            self.ensure_jar_loaded(jar);
        }
        #[cfg(test)]
        if self.event_proxy.is_none() {
            self.test_spawns.push((spawn, generation));
            return;
        }
        let proxy = self.event_proxy.clone().expect("event_proxy set in main before any tab spawn");
        let wake = crate::event_loop::view_wake(std::sync::Arc::clone(&self.pacing.view_wake), proxy.clone());
        let custom_cef = self.custom_cef_dir();
        let snapshots = self.cookies.jar_snapshots.clone();
        std::thread::spawn(move || {
            // Read when the host has connected (the host starts seconds after the spawn): remember its keys, and the
            // jar's clear count then, so a clear before it lands is caught (`note_spawned_jar`).
            let seeded = std::cell::RefCell::new((crate::jar::TabCookies::default(), None));
            let seed = || {
                let (cookies, clears) = match (jar, snapshots.lock()) {
                    (Some(jar), Ok(s)) => {
                        let (cookies, clears) = s.seed(jar);
                        (cookies, Some(clears))
                    }
                    _ => (Vec::new(), None),
                };
                *seeded.borrow_mut() = (crate::jar::TabCookies::seeded(&cookies), clears);
                serde_json::to_string(&cookies).unwrap_or_else(|_| "[]".to_owned())
            };
            let result =
                process_manager::spawn_tab(spawn.kind, spawn.id, &spawn.url, custom_cef.as_deref(), &seed, wake);
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

    /// `begin_view_shutdown` for the live tabs among `ids`, which the caller is about to remove one after another:
    /// their `SHUTDOWN_FLUSH` waits then share one deadline instead of adding up.
    pub(crate) fn begin_tabs_shutdown<'a>(&self, ids: impl IntoIterator<Item = &'a NodeId>) {
        for tab in ids.into_iter().filter_map(|id| self.nodes.get(id)).filter_map(|node| node.as_tab()) {
            tab.tab.begin_shutdown();
        }
    }

    /// Starts every live tab's engine shutdown at once (`ViewTab::begin_shutdown`), so dropping the app waits one
    /// `SHUTDOWN_FLUSH` for all of them rather than one per tab, and a stuck engine adds nothing.
    pub(crate) fn begin_view_shutdown(&self) {
        for node in self.nodes.values() {
            if let Some(tab) = node.as_tab() {
                tab.tab.begin_shutdown();
            }
        }
    }

    /// A failed card already standing for the intent `(url, kind, group)`, other than `id`: a repeat failure refreshes
    /// it instead of adding a second card. Urls compare as exact strings (no normalization).
    fn failed_twin(&self, id: NodeId, url: &str, kind: ViewKind, group: Option<NodeId>) -> Option<NodeId> {
        self.nodes.iter().find_map(|(other, node)| {
            let tab = node.as_unloaded()?;
            let same = *other != id
                && matches!(tab.state, UnloadedState::Failed(_))
                && tab.url == url
                && self.doc.mirror.view_kind(*other) == kind
                && self.doc.mirror.group(*other) == group;
            same.then_some(*other)
        })
    }

    /// Tab context menu's "Duplicate": a new tab at `id`'s current url and
    /// engine, sharing its parent and group -- a sibling, not a child of
    /// `id` itself, matching what "duplicate" means in a real browser's
    /// own tab strip. Its engine is the mirror's (a swap in flight: its target).
    /// The copy keeps `id`'s jar choice.
    pub(crate) fn duplicate_tab(&mut self, id: NodeId) {
        let Some((parent, group, url, kind)) = self.nodes.get(&id).and_then(|n| {
            n.runtime.saved_tab().map(|url| {
                (self.doc.mirror.parent(id), self.doc.mirror.group(id), url.to_string(), self.doc.mirror.view_kind(id))
            })
        }) else {
            return;
        };
        let jar = self.doc.mirror.jar(id);
        let copy = self.spawn_new_tab_with_kind(parent, group, &url, kind, SpawnPurpose::New, jar, None);
        // The copy keeps the original's trees (labels, properties): it is created with them and lands showing them.
        if let Some(snap) = self.spawns.spawning.get_mut(&copy) {
            snap.trees = self.doc.mirror.trees_of(id);
        }
    }

    /// Graph view's "+ New Tab" action: a child of `parent` (or a new root
    /// if `parent` is `None`), spawned invisible — it stays in Graph view
    /// rather than auto-focusing, and
    /// background tabs render nothing until focused anyway. Also used for
    /// a page-triggered new tab (`ViewToHost::OpenTabRequested`), with
    /// that request's URL instead of the configured home page.
    pub(crate) fn spawn_new_tab(&mut self, parent: Option<NodeId>, group: Option<NodeId>, url: &str) {
        let kind = self.settings.value.default_starting_view_kind;
        self.spawn_new_tab_with_kind(parent, group, url, kind, SpawnPurpose::New, None, None);
    }

    /// A tab opened by tab `opener` (a page link or `window.open`): a child of it, in its group, with its jar.
    pub(crate) fn spawn_tab_from(&mut self, opener: NodeId, url: &str, pos: Option<egui::Vec2>) -> NodeId {
        let parent = Some(opener);
        let group = self.group_of(parent);
        let (kind, jar) = (self.settings.value.default_starting_view_kind, self.doc.mirror.jar(opener));
        self.spawn_new_tab_with_kind(parent, group, url, kind, SpawnPurpose::New, jar, pos)
    }

    /// Shared by `spawn_new_tab` (always `default_starting_view_kind`),
    /// `duplicate_tab` (the original tab's own engine), and `start_split`
    /// (`SpawnPurpose::Split` instead of `New`, everything else identical)
    /// -- `purpose` is threaded through to `spawn_tab_async` rather than
    /// hardcoded, since those cases genuinely differ in what should happen once the
    /// spawn completes (see `handle_tab_spawn_result`'s own per-purpose
    /// match). Returns the new tab's id, allocated synchronously even
    /// though the real view-host process spawn (and thus the node's own
    /// insertion into `self.nodes`) completes asynchronously -- see
    /// `spawn_tab_async`'s own doc comment. `jar` is the opener's choice to copy; `None` inherits from `group`.  `pos` (world) replaces the position next to `parent`.
    #[allow(
        clippy::too_many_arguments,
        reason = "every caller names its own parent, group, engine, purpose, jar and position"
    )]
    pub(crate) fn spawn_new_tab_with_kind(
        &mut self,
        parent: Option<NodeId>,
        group: Option<NodeId>,
        url: &str,
        kind: ViewKind,
        purpose: SpawnPurpose,
        jar: Option<NodeId>,
        pos: Option<egui::Vec2>,
    ) -> NodeId {
        let id = self.alloc_node_id();
        // `parent`'s own position only if it actually shares `group` --
        // the toolbar's "+ New Tab" passes `self.current_group` as `group`
        // but `self.focused` (unconditionally) as `parent`, and those can
        // point at different levels (see `spawn_new_tab`'s own doc
        // comment). Falling back to any existing member of `group` keeps
        // the new tab near its actual new groupmates instead of near an
        // unrelated tab at a different level; `ZERO` (same as a fresh
        // group's own spawn point) covers a brand new, still-empty group.
        let in_group = |id: NodeId| self.doc.mirror.group(id) == group;
        let parent_pos = parent
            .filter(|&p| in_group(p) && self.nodes.contains_key(&p))
            .or_else(|| self.nodes.keys().copied().find(|&id| in_group(id)))
            .map_or(egui::Vec2::ZERO, |id| self.layout_pos(id));
        let pos = pos.unwrap_or_else(|| parent_pos + graph_layout::spawn_jitter(id));
        let spawn = TabSpawn::new(id, url.to_string(), kind, pos, purpose, self.settings.value.zoom_factor);
        self.spawn_tab_async(TabSpawn { parent, group, jar, ..spawn });
        id
    }

    /// Stops the focused tab rendering (and drops its find bar, context
    /// menu, and split) -- what switching the maximized node away from it
    /// needs, whether to another tab (`focus_tab`) or an Image
    /// (`focus_item`). `focused` itself is left alone.
    pub(crate) fn hide_focused_tab(&mut self) {
        self.close_find_bar();
        self.menus.context_menu.open = None;
        self.close_split();
        if let Some(old_id) = self.focused {
            if let Some(old) = self.nodes.get_mut(&old_id).and_then(|n| n.as_tab_mut()) {
                old.tab.set_visible(false);
            }
            if let Some(gpu) = &mut self.gpu {
                gpu.free_tab_texture(old_id);
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
    pub(crate) fn focus_tab(&mut self, id: NodeId) {
        self.spawns.pending_trail_focus = None;
        self.end_note_edits();
        if self.focused != Some(id) {
            self.hide_focused_tab();
        }
        if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            node.tab.set_visible(true);
        }
        self.focused = Some(id);
        self.viewed_image = None;
        self.view_mode = ViewMode::Item;
        // An unloaded tab opens by starting its engine; Tab view shows the
        // respawning placeholder until it lands (`pending_respawn_focus`).
        if self.nodes.get(&id).and_then(|n| n.as_unloaded()).is_some() {
            self.spawns.pending_respawn_focus = Some(id);
            self.wake_tab(id, self.doc.mirror.view_kind(id));
        }
        self.send_resize_to(id);
        self.save_session();
    }

    /// Maximizes `id` into Item view, whatever its kind -- a Tab goes
    /// through `focus_tab`; an Image becomes `viewed_image` (the last tab
    /// stays `focused`, hidden, exactly as in Graph view). Kinds with no
    /// Item view are a no-op (and a debug-build assertion).
    pub(crate) fn focus_item(&mut self, id: NodeId) {
        let Some(kind) = self.card_kind(id) else { return };
        match kind.ty {
            crate::store::model::TYPE_TAB => self.focus_tab(id),
            // Nothing to view until the decode lands (`ImagePayload::decoding`).
            crate::store::model::TYPE_IMAGE
                if self.nodes.get(&id).and_then(|n| n.as_image()).is_some_and(|i| i.decoding()) => {}
            crate::store::model::TYPE_IMAGE => {
                self.end_note_edits();
                self.hide_focused_tab();
                self.viewed_image = Some(id);
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
    pub(crate) fn start_split(&mut self, kind: ViewKind) {
        let Some(focused_id) = self.focused else { return };
        let Some(url) = self.nodes.get(&focused_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) else {
            return;
        };
        let group = self.group_of(Some(focused_id));
        let jar = self.doc.mirror.jar(focused_id);
        self.spawn_new_tab_with_kind(Some(focused_id), group, &url, kind, SpawnPurpose::Split, jar, None);
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
            gpu.free_tab_texture(id);
        }
        // The primary pane was rendering at half width for the split --
        // needs to grow back to the full content width now that
        // `content_size_px` sees `split_tab` as unset.
        if let Some(focused_id) = self.focused {
            self.send_resize_to(focused_id);
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
        self.menus.context_menu.open = None;
        match self.viewed_image.or(self.focused) {
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

    /// Closes a tab: tears down its process (via `ViewTab`'s `Drop` —
    /// `Shutdown` + kill), frees its graph-view thumbnail, and reparents
    /// its children to its own parent so the forest stays connected rather
    /// than orphaning that whole subtree. Falls back to Graph view if the
    /// closed tab was focused. Records nothing: `close_node` is the close
    /// that feeds "Recently Closed".
    pub(crate) fn destroy_tab(&mut self, id: NodeId) {
        self.remove_tab(id);
        self.save_session();
    }

    /// A tab whose engine keeps crashing (`crashes_too_often`), or speaks
    /// another protocol version, becomes a failed card: the reason is its
    /// tooltip, and opening it tries again. Respawning would only loop.
    /// `why` picks the toast: `CannotStart` says why the host could not start.
    pub(crate) fn leave_failed_card(&mut self, id: NodeId, reason: &str, why: LeftClosed) {
        let Some(node) = self.nodes.get_mut(&id) else { return };
        let Runtime::Tab(tab) = &node.runtime else { return };
        let (url, kind) = (tab.url.clone(), tab.tab.kind);
        // Replacing the payload drops the `ViewTab`, which reaps the dead host.
        node.runtime = Runtime::Unloaded(unloaded_from(tab, UnloadedState::Failed(reason.to_string())));
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
        }
        // Tab view, focused on this card, falls back to Graph by itself
        // (`build_tab_view_ui`) unless a stale respawn spinner names it; a
        // split pane showing it has nothing left to show.
        if self.spawns.pending_respawn_focus == Some(id) {
            self.spawns.pending_respawn_focus = None;
        }
        if self.split_tab == Some(id) {
            self.close_split();
        }
        self.spawns.tab_crashes.remove(&id);
        let group = self.doc.mirror.group(id);
        // One failed card per intent: keep this one (its id is the live one) and drop another standing for it.
        if let Some(twin) = self.failed_twin(id, &url, kind, group) {
            self.remove_tab(twin);
        }
        match why {
            LeftClosed::CannotStart => {
                let message = format!("Couldn't start {} for {}: {reason}", view_label(kind), Self::toast_url(&url));
                self.queue_failure_toast(kind, &url, group, reason, message);
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
    pub(crate) fn restart_spawn(&self, id: NodeId) -> Option<TabSpawn> {
        let want = self.doc.mirror.view_kind(id);
        let node = self.nodes.get(&id)?;
        let tab = node.as_tab()?;
        let fallback_kind = (tab.tab.kind != want).then_some(tab.tab.kind);
        let spawn = TabSpawn::respawn(id, node, self.doc.mirror.carried(id), want, tab.url.clone())?;
        Some(TabSpawn { fallback_kind, ..spawn })
    }

    /// Replaces live tab `id`'s engine with one started from `spawn` (`restart_spawn`). Dropping the old `ViewTab`
    /// ends its host.
    pub(crate) fn restart_with(&mut self, id: NodeId, spawn: TabSpawn) {
        self.nodes.remove(&id);
        // Same stale-state guard `destroy_tab`/`finish_swap` apply — see `finish_swap`'s own comment on this.
        let card = CardId::from(id);
        if self.interaction.dragging == Some(card) {
            self.interaction.dragging = None;
        }
        if self.interaction.hovered == Some(card) {
            self.interaction.hovered = None;
        }
        self.respawn_tab(spawn);
    }

    /// "Restart View": a new host for live tab `id` at its current URL, as a crash restart does, without counting
    /// toward the crash limit. False when `id` is no live tab.
    pub(crate) fn restart_view(&mut self, id: NodeId) -> bool {
        let Some(spawn) = self.restart_spawn(id) else { return false };
        self.queue_toast(format!("Restarted: {}", Self::toast_url(&spawn.url)));
        self.restart_with(id, spawn);
        true
    }

    /// "End View": ends live tab `id`'s host and leaves a failed card, which a click starts again. False when `id`
    /// is no live tab.
    pub(crate) fn end_view(&mut self, id: NodeId) -> bool {
        if self.nodes.get(&id).and_then(|n| n.as_tab()).is_none() {
            return false;
        }
        self.leave_failed_card(id, "It stopped responding and was ended.", LeftClosed::Ended);
        true
    }

    /// A live tab on a jar engine (`Tag::Jars`) running on a jar other than the one it now resolves to, or on a
    /// jar the user cleared (`jar_reseed`): it switches when it next qualifies (chip "will switch").
    pub(crate) fn will_switch(&self, id: NodeId) -> bool {
        let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_tab()) else { return false };
        let Some((had, _)) = self.cookies.jar_tabs.get(&id) else { return false };
        let Some((now, _)) = self.effective_jar(id) else { return false };
        tab.tab.kind.has(crate::caps::Tag::Jars) && (*had != now || self.cookies.jar_reseed.contains(&id))
    }

    /// Puts every live tab whose jar changed and that is not busy, focused or showing to sleep; it wakes into the
    /// new jar (spec table). Runs after a local jar change, a remote batch (`apply_remote`) and each idle sweep, so a
    /// waiting tab switches once it qualifies. A dirty form keeps it awake (`sleep_state::verdict`).
    pub(crate) fn sweep_jar_switches(&mut self) {
        let todo: Vec<NodeId> = self
            .tab_idle_views()
            .iter()
            .filter(|v| self.will_switch(v.id))
            .filter(|v| {
                if self.nodes.get(&v.id).and_then(|n| n.as_tab()).is_none() {
                    return false;
                }
                let had = self.cookies.jar_tabs.get(&v.id).map(|(k, _)| *k);
                let focused = self.focused == Some(v.id);
                let reseed = self.cookies.jar_reseed.contains(&v.id);
                let Some((now, _)) = self.effective_jar(v.id) else { return false };
                sleep_state::jar_switch(had, now, reseed, v.busy(), focused) == sleep_state::JarSwitch::SleepNow
            })
            .map(|v| v.id)
            .collect();
        for id in todo {
            self.request_sleep(id, SleepWhy::JarSwitch);
        }
    }

    /// "Switch jar now": the old jar gets the tab's final cookies, then the host is replaced in place by one seeded
    /// from the new jar (the engine-swap path, same engine). Does nothing while another host change is on its way
    /// (`host_change_pending`): that one replaces the host anyway.
    pub(crate) fn switch_jar_now(&mut self, id: NodeId) {
        let Some((kind, url)) = self.nodes.get(&id).and_then(|n| n.as_tab()).map(|t| (t.tab.kind, t.url.clone()))
        else {
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
    pub(crate) fn can_switch_jar_now(&self, id: NodeId) -> bool {
        self.will_switch(id) && !self.host_change_pending(id)
    }

    /// An engine swap, a sleep or an earlier "Switch Jar Now" is already on its way for tab `id`.
    /// Each replaces or shuts the host, and a second `hold_for_cookies` would cancel the one parked first.
    pub(crate) fn host_change_pending(&self, id: NodeId) -> bool {
        self.pending_sleeps.contains_key(&id)
            || self.cookies.cookie_holds.contains_key(&id)
            || self.nodes.get(&id).and_then(|n| n.as_tab()).is_some_and(|t| t.swap_target.is_some())
    }

    /// The local close of a tab without the save (`destroy_tab`, `destroy_group_members`, `redirect_tab_to_content`),
    /// queueing the delete (`take_out`). Nothing when `id` is not a tab shown here.
    pub(crate) fn remove_tab(&mut self, id: NodeId) {
        if self.nodes.get(&id).is_some_and(|n| n.runtime.saved_tab().is_some()) {
            self.take_out(id, crate::node_store::Removal::Local);
        }
    }

    /// Shared by swap-finish and crash recovery: starts `spawn` (from
    /// `TabSpawn::respawn`, so the tab keeps its `TabCarry`; its place in the
    /// graph is its `App::layout` entry) and, if it's the focused tab, shows a spinner until it's
    /// back (only the focused tab is ever visible).
    pub(crate) fn respawn_tab(&mut self, spawn: TabSpawn) {
        if self.focused == Some(spawn.id) {
            self.spawns.pending_respawn_focus = Some(spawn.id);
        }
        self.spawn_tab_async(spawn);
    }

    /// A new tab at `url`, placed at `pos` in `group`, opened from `parent` (if any). Returns its id.
    pub(crate) fn spawn_tab_at(
        &mut self,
        url: &str,
        pos: egui::Vec2,
        group: Option<NodeId>,
        parent: Option<NodeId>,
    ) -> NodeId {
        let id = self.alloc_node_id();
        let (kind, zoom) = (self.settings.value.default_starting_view_kind, self.settings.value.zoom_factor);
        let spawn = TabSpawn::new(id, url.to_string(), kind, pos, SpawnPurpose::New, zoom);
        self.spawn_tab_async(TabSpawn { group, parent, ..spawn });
        id
    }

    /// `Ctrl+Tab`/`Ctrl+Shift+Tab`: focuses the next/previous tab in
    /// `tab_order` (insertion order — this graph-based UI has no tab strip
    /// to define a more meaningful order), wrapping at both ends. A no-op
    /// with zero or one tab open.
    pub(crate) fn cycle_focused_tab(&mut self, forward: bool) {
        // `node_order` holds every graph node (tabs, notes, images, PDFs,
        // JSON files), not just tabs -- filter to live and unloaded tabs
        // first (same filter the omnibar's cards use) so this
        // never lands on a non-tab node. Landing on an unloaded tab is
        // fine: `focus_tab` wakes it, and `render_tab_toolbar_content`
        // shows the respawning placeholder until it's live rather than
        // assuming `as_tab().unwrap()`.
        let tab_ids: Vec<NodeId> = self
            .node_order
            .iter()
            .copied()
            .filter(|id| self.nodes.get(id).and_then(|n| n.runtime.listed_tab()).is_some())
            .collect();
        if tab_ids.len() < 2 {
            return;
        }
        let current = self.focused.and_then(|id| tab_ids.iter().position(|t| *t == id));
        let len = tab_ids.len() as isize;
        let next = match current {
            Some(i) => {
                let delta = if forward { 1 } else { -1 };
                (i as isize + delta).rem_euclid(len) as usize
            }
            None => 0,
        };
        self.focus_tab(tab_ids[next]);
    }
}

#[cfg(test)]
mod tests;
