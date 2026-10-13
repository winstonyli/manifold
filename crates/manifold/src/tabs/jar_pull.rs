//! The cookie jars' pull cycle on App: asking hosts for their cookies, parking answers until they arrive, and
//! saving the jars they changed.

use super::*;

/// An engine answer parked until the tab's cookies arrive (`App::hold_for_cookies`).
pub(crate) enum Held {
    PageState(PageState),
    Snapshot(ViewKind, String),
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

impl App {
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

    /// Asks one jar engine's tab (`ViewDesc::jar`) for its cookies; the answer arrives as `ViewToHost::Cookies`. Best effort:
    /// a host may never answer, so nothing waits on it, and a pull still pending (under `JAR_PULL_TIMEOUT`) is not
    /// stacked.
    pub(crate) fn pull_cookies(&mut self, id: &str) {
        let Some(node) = self.nodes.get_mut(id).and_then(|n| n.as_tab_mut()) else { return };
        if node.tab.kind.desc().jar
            && start_pull(&mut self.cookies.jar_pulls, &mut self.cookies.jar_silent, id, std::time::Instant::now())
        {
            node.tab.send(&HostToView::GetCookies);
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
        // Not a tab whose engine stopped reading: its reply cannot come, and the final pull would wait on it.
        let now = std::time::Instant::now();
        let ids: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.as_tab().is_some_and(|t| !t.tab.is_unresponsive(now)))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.pull_cookies(&id);
        }
    }

    /// One last pull as the app quits, so a login made since the last timed pull is kept. Asks every tab at once, then
    /// waits at most `QUIT_PULL_WAIT` in total (one shared deadline, not per tab) for the replies. Any other engine message read meanwhile is dropped: nothing would act on it
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
                        ViewToHost::Cookies(json) => Some(json),
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
}
