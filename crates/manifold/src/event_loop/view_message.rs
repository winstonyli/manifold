//! What each `ViewToHost` message does to the host.

use super::*;
use crate::store::model::NodeId;

impl App {
    /// Applies one message from tab `tab_id`'s engine (see
    /// `drain_tab_events`, which uploads frames itself). Messages that
    /// update the tab do nothing once it's gone, e.g. after a crash earlier
    /// in the same drain. Returns whether something worth saving the
    /// session for happened: the tab's settled url changed (title/history-
    /// only changes and a same-url reload don't count), or a download
    /// completed (the store keeps completed downloads).
    pub(crate) fn handle_view_message(&mut self, tab_id: NodeId, msg: ViewToHost) -> bool {
        match msg {
            ViewToHost::FrameReady { .. } => {}
            ViewToHost::PageState(state) => {
                if let Some(Held::PageState(state)) = self.hold_for_cookies(tab_id, Held::PageState(state)) {
                    self.finish_sleep(tab_id, state);
                }
            }
            ViewToHost::Cookies(json) => {
                self.cookies.jar_pulls.remove(&tab_id);
                self.cookies.jar_silent.remove(&tab_id);
                self.apply_pulled_cookies(tab_id, &json);
                self.release_cookie_hold(tab_id);
            }
            ViewToHost::StateSnapshot(state) => {
                let target =
                    self.nodes.get_mut(&tab_id).and_then(|n| n.as_tab_mut()).and_then(|t| t.swap_target.take());
                if let Some(target) = target {
                    if let Some(Held::Snapshot(target, url)) =
                        self.hold_for_cookies(tab_id, Held::Snapshot(target, state.url))
                    {
                        self.finish_swap(tab_id, target, url);
                    }
                }
            }
            ViewToHost::NavigationStateChanged { url, title, can_go_back, can_go_forward, loading, .. } => {
                let Some(current) = self.nodes.get(&tab_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) else {
                    return false;
                };
                let url = self.reported_url(tab_id, url, loading, current);
                let stale = self.nav_history.entry(tab_id).or_default().observe(
                    &url,
                    &title,
                    loading,
                    can_go_forward,
                    std::time::Instant::now(),
                );
                let Some(node) = self.nodes.get_mut(&tab_id).and_then(|n| n.as_tab_mut()) else {
                    return false;
                };
                let (url, title) = if stale { (node.url.clone(), node.title.clone()) } else { (url, title) };
                let url_changed = node.url != url;
                let skip = skip_convert_after(self.spawns.skip_convert.get(&tab_id).copied(), url_changed, loading);
                match skip {
                    Some(settled) => self.spawns.skip_convert.insert(tab_id, settled),
                    None => self.spawns.skip_convert.remove(&tab_id),
                };
                if url_changed {
                    self.history.push(history::HistoryEntry { title: title.clone(), url: url.clone() });
                    if self.history.len() > history::MAX_HISTORY_ENTRIES {
                        self.history.remove(0);
                    }
                    self.history_ui.history_dirty = true;
                    self.history_ui.history_matches_cache = None;
                }
                node.url = url.clone();
                node.title = title;
                node.can_go_back = can_go_back;
                node.can_go_forward = can_go_forward;
                node.loading = loading;
                // The origin is known as soon as the url is, without waiting for the page to finish loading
                // (`favicon_fetch` guesses `/favicon.ico`); a typed navigation asks in `navigate_tab`.
                if url_changed {
                    self.request_favicons([url.as_str()]);
                }
                let restore = self.saved_scroll.get_mut(&tab_id).is_some_and(|s| s.on_loading(loading));
                if restore {
                    if let Some(s) = self.saved_scroll.remove(&tab_id) {
                        if let Some(tab) = self.nodes.get_mut(&tab_id).and_then(|n| n.as_tab_mut()) {
                            tab.tab.send(&HostToView::RestoreScroll { x: s.x, y: s.y });
                        }
                    }
                }
                return url_changed;
            }
            ViewToHost::Crashed { reason } => {
                let Some(spawn) = self.restart_spawn(tab_id) else { return false };
                // A stale engine binary (protocol mismatch) or a host that cannot
                // start would fail the same way again, and a page or install that
                // crashes on every start would loop forever; each is left as a
                // failed card instead.
                let times = self.spawns.tab_crashes.entry(tab_id).or_default();
                match crate::tabs::after_crash(&reason, times, std::time::Instant::now()) {
                    AfterCrash::Restart => {}
                    AfterCrash::GiveUp => {
                        eprintln!("view-host for {tab_id} crashed ({reason}); not restarting");
                        self.leave_failed_card(tab_id, &reason, LeftClosed::KeepsCrashing);
                        return false;
                    }
                    AfterCrash::CannotStart(why) => {
                        eprintln!("view-host for {tab_id} could not start ({why}); not restarting");
                        self.leave_failed_card(tab_id, why, LeftClosed::CannotStart);
                        return false;
                    }
                }
                let url = &spawn.url;
                // The origin only (`redacted_url`): a path or query can carry a token.
                eprintln!(
                    "view-host crashed ({reason}) for {tab_id}, restarting at {}",
                    crash_report::redacted_url(url)
                );
                // Previously silent from the actual app's own UI -- this only
                // ever showed up in the console, so a user with no terminal
                // open had no way to know a tab had crashed and quietly come
                // back, e.g. mid-navigation with unsaved page state lost.
                // Reuses the same toast infrastructure "Saved <path>" already
                // has (generalized to a plain message, see `save_toast`'s own
                // doc comment) rather than a second, separate notification
                // mechanism.
                self.queue_toast(format!("Tab crashed, restarted: {}", Self::toast_url(url)));
                self.restart_with(tab_id, spawn);
            }
            // Page-triggered new tab (target=_blank/window.open()/ctrl+click,
            // intercepted by CEF/WebKit/WebView2 - see ViewToHost::OpenTabRequested's
            // doc comment): a child of whichever tab requested it, staying in
            // the current view rather than auto-focusing, same as an explicit
            // "+ New Tab" click. Only web/internal schemes, only with a user
            // gesture, and only a few per source tab (`page_tab_gate.rs`).
            ViewToHost::OpenTabRequested { url, user_gesture } => {
                if !crate::page_tab_gate::page_tab_url_allowed(&url) {
                    match crate::page_tab_gate::scheme_for_log(&url) {
                        Some(scheme) => eprintln!("popup to a disallowed scheme dropped: {scheme}:"),
                        None => eprintln!("popup to a url with no scheme dropped"),
                    }
                } else {
                    match self.page_tab_gate.admit(tab_id, user_gesture, std::time::Instant::now()) {
                        crate::page_tab_gate::Admit::Open => {
                            self.spawn_tab_from(tab_id, &url, None);
                        }
                        crate::page_tab_gate::Admit::Drop { toast } => {
                            if toast {
                                self.queue_toast("Blocked pop-ups from this page".to_string());
                            }
                        }
                    }
                }
            }
            ViewToHost::FindResult { count, active_index } => {
                // Ignore a result from a tab that isn't the one the find bar
                // is currently searching — shouldn't normally happen
                // (`close_find_bar` sends `FindStop` on every focus change),
                // but a stray late result from just before a switch is cheap
                // to guard against.
                if self.focused == Some(tab_id) {
                    self.find.result = Some((count, active_index));
                }
            }
            ViewToHost::AudioPlayingChanged { playing } => {
                if let Some(tab) = self.nodes.get_mut(&tab_id).and_then(|n| n.as_tab_mut()) {
                    tab.audio_playing = playing;
                }
            }
            // The engine's own in-memory page text, already sent over IPC --
            // saved and converted right away, same as any other content
            // kind. Written on the UI thread, unlike every other kind's
            // fetch: it's already in hand, so there's nothing to background.
            ViewToHost::JsonBody { body } => {
                let url = self.nodes.get(&tab_id).and_then(|n| n.as_tab()).map(|tab| tab.url.clone());
                if self.spawns.skip_convert.contains_key(&tab_id) {
                    return false;
                }
                if let (Some(url), Some(dir)) = (url, crate::profile::content_dir()) {
                    let node_id = self.alloc_node_id();
                    if let Some(web) =
                        crate::redirect_fetch::save(&dir, node_id, crate::content::ContentKind::Json, body.into_bytes())
                    {
                        self.redirect_tab_to_content(tab_id, node_id, url, web);
                    }
                }
            }
            // See `manifold_view_protocol::ViewToHost::RedirectableContent`'s own doc
            // comment -- kicks off `manifold`'s own independent fetch of this
            // tab's current url; the actual tab-to-node conversion happens
            // once that completes, in `redirect_fetcher`'s own `tick()` drain
            // in `drain_tab_events`, not here.
            ViewToHost::RedirectableContent { content_type } => {
                use crate::content::ContentKind as K;
                if self.spawns.skip_convert.contains_key(&tab_id) {
                    return false;
                }
                let url = self.nodes.get(&tab_id).and_then(|n| n.as_tab()).map(|t| t.url.clone());
                let kind = crate::content::kind_of(&content_type);
                if let (Some(url), K::Image | K::Pdf | K::Json | K::Note) = (url, kind) {
                    let node_id = self.alloc_node_id();
                    let target = FetchTarget::Tab(tab_id);
                    self.redirect_fetcher.request(FetchJob { target, node_id, url, kind });
                }
            }
            ViewToHost::ContextMenuLink { x, y, href } => {
                let ppp = self.ppp() as f32;
                // Content-area coordinates -> window point space.
                // The engine echoes back the same physical-pixel
                // surface coordinates the right-click was sent in
                // (see `manifold_view_protocol::InputEvent`'s doc comment), so
                // this is the one place that has to divide out the
                // DPI scale: egui lays out in logical points. Then
                // shifted down past the toolbar, since Tab view's
                // content starts below it (see
                // `CHROME_HEIGHT_POINTS`'s other call sites). Only
                // meaningful for the tab currently on screen.
                if self.focused == Some(tab_id) {
                    self.menus.context_menu.open =
                        Some(ContextMenuState { x: x as f32 / ppp, y: y as f32 / ppp + CHROME_HEIGHT_POINTS, href });
                }
            }
            ViewToHost::DownloadStarted { path, total_bytes, url } => {
                self.insert_download_node(tab_id, path, total_bytes, url);
            }
            // Each finds its node with `in_flight_download_id`: the in-flight
            // one this tab started, never a finished one with the same path.
            ViewToHost::DownloadProgress { path, bytes_received } => {
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.bytes_received = bytes_received;
                    }
                }
            }
            ViewToHost::DownloadFailed { path } => {
                self.queue_toast(format!("Download failed: {path}"));
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.failed = true;
                    }
                }
            }
            ViewToHost::DownloadCompleted { path } => {
                self.queue_toast(format!("Saved {path}"));
                if let Some(id) = self.in_flight_download_id(tab_id, &path) {
                    if let Some(dl) = self.nodes.get_mut(&id).and_then(|n| n.as_download_mut()) {
                        dl.completed = true;
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::store::model::{ActorId, NodeId};

    const N: NodeId = NodeId { actor: ActorId(1), counter: 1 };
    const A: NodeId = NodeId { actor: ActorId(1), counter: 2 };
    const B: NodeId = NodeId { actor: ActorId(1), counter: 3 };

    #[test]
    fn a_failed_download_is_marked_and_no_longer_in_flight() {
        let mut app = App::default();
        app.insert_note_at(N, None);
        app.insert_download_node(N, "f.zip".into(), Some(10), None);
        let in_flight =
            |app: &App| app.nodes.values().any(|n| n.as_download().is_some_and(|d| !d.completed && !d.failed));
        assert!(in_flight(&app));
        app.handle_view_message(N, ViewToHost::DownloadFailed { path: "f.zip".into() });
        let download = app.nodes.values().find_map(|n| n.as_download()).unwrap();
        assert!(download.failed && !download.completed);
        assert!(!in_flight(&app));
    }

    #[test]
    fn a_redownload_updates_the_new_node_not_the_finished_one() {
        let mut app = App::default();
        app.insert_note_at(N, None);
        app.insert_download_node(N, "f.zip".into(), Some(10), None);
        app.handle_view_message(N, ViewToHost::DownloadCompleted { path: "f.zip".into() });
        app.insert_download_node(N, "f.zip".into(), Some(20), None);
        app.handle_view_message(N, ViewToHost::DownloadProgress { path: "f.zip".into(), bytes_received: 7 });
        let downloads: Vec<_> = app.node_order.iter().filter_map(|id| app.nodes[id].as_download()).collect();
        assert_eq!(downloads.len(), 2);
        assert!(downloads[0].completed && downloads[0].bytes_received == 0, "the old node is untouched");
        assert_eq!(downloads[1].bytes_received, 7);
    }

    #[test]
    fn same_named_downloads_from_two_tabs_stay_apart() {
        let mut app = App::default();
        app.insert_note_at(A, None);
        app.insert_note_at(B, None);
        app.insert_download_node(A, "f.zip".into(), None, None);
        app.insert_download_node(B, "f.zip".into(), None, None);
        app.handle_view_message(A, ViewToHost::DownloadFailed { path: "f.zip".into() });
        let by_parent = |p: NodeId| {
            app.nodes
                .iter()
                .find(|(id, _)| app.opened_from(**id) == Some(p))
                .and_then(|(_, n)| n.as_download())
                .unwrap()
                .failed
        };
        assert!(by_parent(A) && !by_parent(B));
    }
}
