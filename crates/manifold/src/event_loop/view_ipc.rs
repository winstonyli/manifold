//! The engine processes' side of a frame: draining their messages, uploading their frames and reporting a tab's URL.

use super::*;

/// A tab's newest frame, which `App::drain_view_ipc` collects and `App::upload_frames` uploads.
pub(super) struct FrameUpdate {
    tab_id: String,
    kind: ViewKind,
    /// The frame file the pixels are in (`FrameReady::generation`).
    generation: u32,
    width: u32,
    height: u32,
    /// The rows that changed since the last upload; `None` is the whole frame.
    rows: Option<std::ops::Range<u32>>,
    pixels: Option<Vec<u8>>,
}

impl App {
    /// Takes up to `VIEW_MESSAGES_PER_DRAIN` messages off each tab's `events_rx`, applies all but frames in
    /// arrival order, and returns each tab's newest frame (`drain_tab_events`).
    pub(super) fn drain_view_ipc(&mut self) -> std::collections::HashMap<String, FrameUpdate> {
        // Keyed by tab id, not a plain `Vec` -- only the *latest* `FrameReady`
        // for a tab ever matters (each one fully supersedes the last in
        // `GpuState::upload_frame`'s own `tab_textures.insert`), matching the
        // same "keep only the newest, drop the rest" policy the WebView2
        // view-host's own capture code already applies one layer down (see
        // `capture.rs`'s `latest` field). A plain `Vec` here used to `push`
        // every buffered `FrameReady` this drain found and then process each
        // one in turn -- harmless while the main loop keeps up with the
        // view-host's own frame rate, but a real, confirmed bug once it
        // falls behind even slightly: found live via temporary
        // instrumentation showing bursts of up to 81 queued frames processed
        // in a single tick for one idle tab, each one a real, wasted GPU
        // texture+bind-group+thumbnail creation immediately thrown away by
        // the next -- severe enough burst GPU churn to both stall the main
        // thread for whole seconds at a time (Windows' own "(Not
        // Responding)", confirmed live) and outpace wgpu's own resource
        // reclamation, observed as continuous unbounded memory growth
        // (roughly 20MB/s from one idle default WebView2 tab). This bug
        // itself is engine-agnostic -- every engine's `FrameReady` messages
        // go through this same drain loop -- but WebView2 is the one that
        // actually triggers it in practice: its `Windows.Graphics.Capture`
        // pipeline captures continuously at the compositor's own refresh
        // rate regardless of whether the page changed, while CEF/Servo/
        // WebKit's paint-driven capture sends far fewer frames for a static,
        // idle page, staying well within what this loop could already keep
        // up with (confirmed live: an otherwise-identical idle-memory test
        // with CEF as the default engine stayed flat).
        // Since wave 3b the reader keeps one frame per tab itself (`ViewTab::take_frame`), so the burst described
        // above cannot build up in the first place.
        let mut frame_updates: std::collections::HashMap<String, FrameUpdate> = std::collections::HashMap::new();
        let mut messages: Vec<(String, ViewToHost)> = Vec::new();
        let now = std::time::Instant::now();
        let mut stuck: Vec<String> = Vec::new();
        for tab_id in &self.node_order {
            let Some(tab) = self.nodes.get(tab_id).and_then(|n| n.as_tab()) else {
                continue;
            };
            for msg in tab.tab.events_rx.try_iter().take(VIEW_MESSAGES_PER_DRAIN) {
                match msg {
                    // Only the last of a run of same-path Progress events is
                    // ever visible (applying one overwrites `bytes_received`),
                    // and each costs a scan of every node, so a run collapses
                    // into its newest.
                    ViewToHost::DownloadProgress { path, bytes_received } => {
                        if let Some((_, ViewToHost::DownloadProgress { path: last, bytes_received: last_bytes })) =
                            messages.last_mut()
                        {
                            if *last == path {
                                *last_bytes = bytes_received;
                                continue;
                            }
                        }
                        messages.push((tab_id.clone(), ViewToHost::DownloadProgress { path, bytes_received }));
                    }
                    msg => messages.push((tab_id.clone(), msg)),
                }
            }
            if let Some(frame) = tab.tab.take_frame() {
                frame_updates.insert(
                    tab_id.clone(),
                    FrameUpdate {
                        tab_id: tab_id.clone(),
                        kind: tab.tab.kind,
                        generation: frame.generation,
                        width: frame.width,
                        height: frame.height,
                        rows: frame.rows,
                        pixels: frame.pixels,
                    },
                );
            }
            if tab.tab.take_newly_unresponsive(now) {
                stuck.push(tab.url.clone());
            }
        }
        for url in stuck {
            self.queue_toast(format!(
                "Not responding: {}. Right-click its card to restart or end it.",
                Self::toast_url(&url)
            ));
        }
        // One `save_session()` for the whole drain rather than one per
        // `NavigationStateChanged`.
        let mut changed = false;
        if !messages.is_empty() {
            self.mark_active();
        }
        self.note_view_frames(frame_updates.len() as u32);
        for (tab_id, msg) in messages {
            let user_started = match &msg {
                ViewToHost::NavigationStateChanged { user_started, .. } => *user_started,
                _ => None,
            };
            let url_changed = self.handle_view_message(&tab_id, msg);
            if url_changed && self.shares_url_change(&tab_id, user_started) {
                // Ruling 3: every reported url change is shared, whichever
                // tab it came from -- share it here rather than leaving it
                // to `navigate_tab` (that only handles this instance's own
                // address-bar navigations, not the engine's own
                // confirmation this responds to, nor back/forward) --
                // except one a script or the store started. Shared as reported
                // now: a later unshared move (a redirect) is not (2b-3 Ruling 4).
                if let Some(url) = self.nodes.get(&tab_id).and_then(|n| n.as_tab()).map(|t| t.url.clone()) {
                    self.edit_url(&tab_id, &url);
                }
            }
            changed |= url_changed;
        }
        if changed {
            self.save_session();
        }
        frame_updates
    }

    /// Uploads each tab's newest frame to the GPU. Runs last in `drain_tab_events`: a message or fetch
    /// applied earlier in the same drain can remove the tab a frame came from.
    pub(super) fn upload_frames(&mut self, frame_updates: std::collections::HashMap<String, FrameUpdate>) {
        for update in frame_updates.into_values() {
            // A `JsonBody`/`RedirectableContent` conversion earlier in this
            // same drain (or a plain tab close) can remove `update.tab_id`
            // before its `FrameReady` -- collected up front, above -- gets
            // here. Uploading it anyway would recreate a `tab_textures`
            // entry for a tab that no longer exists and that nothing ever
            // frees again (`remove_tab`'s `free_tab_texture` already ran).
            if !self.nodes.contains_key(&update.tab_id) {
                continue;
            }
            let format = if update.kind.desc().bgra {
                wgpu::TextureFormat::Bgra8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8UnormSrgb
            };
            if let Some(pixels) = update.pixels {
                // RemoteTransport view-host (WebKit over WSL2): no shared
                // memory with this process, so the pixels travel inline in
                // the message itself.
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_frame(&update.tab_id, &pixels, update.width, update.height, format, update.rows);
                }
            } else if let Some(Ok(pixels)) = self
                .nodes
                .get_mut(&update.tab_id)
                .and_then(|n| n.as_tab_mut())
                .map(|t| t.tab.frame_pixels(update.generation, update.width, update.height))
            {
                // The tab keeps one mapping per frame file. A file the engine has already replaced (a resize) no
                // longer opens, or opens at the wrong size, so that frame is skipped; the next one names the new file.
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_frame(&update.tab_id, pixels, update.width, update.height, format, update.rows);
                }
            } else if let Some(gpu) = &mut self.gpu {
                // The skipped frame's changed rows never reached the texture: the next upload writes it whole.
                gpu.mark_stale(&update.tab_id);
            }
        }
    }

    /// The url tab `tab_id` shows, given one its engine reported (`url`)
    /// and the one it shows now (`current`). Right after `navigate_tab` an
    /// engine can still report the url it left, from before it started the
    /// navigation (CEF reports loading first): that keeps `current`, the
    /// typed url, rather than flashing back and adding the old url to
    /// history again. Any other url, or `NAVIGATE_START_WINDOW` passing,
    /// ends that. So does a finished load (`!loading`) at the old url after
    /// one reported loading: the navigation started and never left the page
    /// (an attachment download, a 204), and CEF reports nothing after it.
    pub(crate) fn reported_url(&mut self, tab_id: &str, url: String, loading: bool, current: String) -> String {
        const NAVIGATE_START_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);
        let Some((from, at, started)) = self.spawns.navigated_from.get_mut(tab_id) else { return url };
        if *from == url && at.elapsed() < NAVIGATE_START_WINDOW && (loading || !*started) {
            *started |= loading;
            return current;
        }
        self.spawns.navigated_from.remove(tab_id);
        url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_drain_applies_at_most_its_cap_per_tab() {
        use crate::node::TabPayload;
        use crate::process_manager::{ViewKind, ViewTab};
        let mut app = App::default();
        let (tab, _peer, feed) = ViewTab::fake_fed(ViewKind::Servo);
        let payload = TabPayload {
            tab,
            url: "https://a/".into(),
            title: String::new(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_playing: false,
            muted: false,
            zoom_factor: 1.0,
            swap_target: None,
        };
        app.test_node("t", crate::store::model::Kind::Tab, Runtime::Tab(payload));
        for i in 0..VIEW_MESSAGES_PER_DRAIN + 10 {
            feed.send(ViewToHost::AudioPlayingChanged { playing: i.is_multiple_of(2) }).unwrap();
        }
        app.drain_view_ipc();
        let left = app.nodes.get("t").and_then(|n| n.as_tab()).map_or(0, |t| t.tab.events_rx.try_iter().count());
        assert_eq!(left, 10, "the rest waits for the next drain");
    }
}
