//! Sleepi        if self.text_is_stale(id) {g tabs on App: the request/answer/expiry cycle, the idle sweep, and "Sleep Tabs" on a group.

use super::*;

impl App {
    /// Second half of "Sleep": shuts a live tab's engine down and keeps its
    /// card, as the unloaded tab a restore makes (`UnloadedState::Sleeping`):
    /// same id, place, url, pin, mute and zoom, and it saves as the same tab.
    /// Opening it starts the engine again (`wake_tab`) and reloads the url;
    /// `scroll`, when not the origin, is put back once that load finishes. The
    /// tab Tab view shows and the split pane's tab stay live. Returns whether
    /// it slept. Callers go through `request_sleep`.
    fn sleep_now(&mut self, id: NodeId, scroll: Option<(f64, f64)>) -> bool {
        let open = self.showing_tab_view() && self.focused == Some(id);
        if open || self.split_tab == Some(id) {
            return false;
        }
        let Some(node) = self.nodes.get_mut(&id) else { return false };
        let Runtime::Tab(tab) = &node.runtime else { return false };
        let asleep = unloaded_from(tab, UnloadedState::Sleeping);
        // Replacing the payload drops the `ViewTab`, whose `Drop` shuts
        // the engine host down (see `teardown_running_session`).
        node.runtime = Runtime::Unloaded(asleep);
        if let Some(gpu) = &mut self.gpu {
            gpu.free_tab_thumbnail(id);
            gpu.free_tab_texture(id);
        }
        // A scroll left from an earlier wake that never finished loading is stale.
        self.saved_scroll.remove(&id);
        if let Some((x, y)) = scroll {
            if x != 0.0 || y != 0.0 {
                self.saved_scroll.insert(id, SavedScroll::new(x, y));
            }
        }
        self.save_session();
        true
    }

    /// First phase of every sleep: validates as `sleep_now` does, asks the
    /// engine for its page state, and notes the request. `finish_sleep`
    /// completes it when `ViewToHost::PageState` arrives;
    /// `expire_pending_sleeps` (run by `sweep_idle_tabs`) settles requests
    /// nobody answered. Returns whether a query was sent.
    pub(crate) fn request_sleep(&mut self, id: NodeId, why: SleepWhy) -> bool {
        let open = self.showing_tab_view() && self.focused == Some(id);
        // A request nobody answered must not block a new one.
        self.expire_pending_sleeps(std::time::Instant::now());
        if open || self.split_tab == Some(id) || self.pending_sleeps.contains_key(&id) {
            return false;
        }
        if self.nodes.get(&id).and_then(|n| n.as_tab()).is_none() {
            return false;
        }
        self.pull_cookies(id);
        if self.text_is_stale(id) {
            self.ask_page_text(id);
        }
        let Some(tab) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) else { return false };
        tab.tab.send(&HostToView::QueryPageState);
        self.pending_sleeps.insert(id, PendingSleep { why, sent: std::time::Instant::now() });
        true
    }

    /// Second phase: the engine's answer. Idle sleep stays awake for a dirty
    /// form (and restarts that tab's idle clock); otherwise the tab sleeps,
    /// keeping its scroll position for the wake.
    pub(crate) fn finish_sleep(&mut self, id: NodeId, state: PageState) {
        let Some(pending) = self.pending_sleeps.remove(&id) else { return };
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
    pub(super) fn expire_pending_sleeps(&mut self, now: std::time::Instant) {
        let dead: Vec<(NodeId, SleepWhy)> = self
            .pending_sleeps
            .iter()
            .filter(|(_, p)| sleep_state::expired(p, now))
            .map(|(id, p)| (*id, p.why))
            .collect();
        for (id, why) in dead {
            self.pending_sleeps.remove(&id);
            match why {
                SleepWhy::Manual => {
                    self.sleep_now(id, None);
                }
                SleepWhy::Idle | SleepWhy::JarSwitch => self.idle_clock.restart(id, now),
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
            self.request_sleep(id, SleepWhy::Idle);
        }
        self.sweep_jar_switches();
    }

    /// What the idle and jar-switch sweeps need to know about each live tab.
    pub(super) fn tab_idle_views(&self) -> Vec<crate::idle_sleep::TabIdleView> {
        let on_screen = self.showing_tab_view().then_some(self.focused).flatten();
        // A download card's parent is the tab it came from; `completed` flips when the engine finishes it.
        let downloading: std::collections::HashSet<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.as_download().is_some_and(|d| !d.completed && !d.failed))
            .filter_map(|(id, _)| self.opened_from(*id))
            .collect();
        self.nodes
            .iter()
            .filter_map(|(id, n)| {
                let t = n.as_tab()?;
                Some(crate::idle_sleep::TabIdleView {
                    id: *id,
                    on_screen: on_screen == Some(*id) || self.split_tab == Some(*id),
                    audio_playing: t.audio_playing,
                    pinned: self.doc.mirror.pinned(*id),
                    loading: t.loading,
                    downloading: downloading.contains(id),
                })
            })
            .collect()
    }

    /// "Sleep Tabs" on a group: `request_sleep` for each of its live tabs.
    /// Returns how many were requested.
    pub(crate) fn sleep_group(&mut self, id: NodeId) -> usize {
        let members: Vec<NodeId> = self.members_of(id).collect();
        members.into_iter().filter(|&m| self.request_sleep(m, SleepWhy::Manual)).count()
    }
}
