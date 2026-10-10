//! Viewport geometry, window title, zoom and navigation helpers of `App`.

use super::*;

impl App {
    /// winit's own DPI scale factor — physical pixels per logical point.
    ///
    /// **Not applied to outgoing `InputEvent::Mouse*`/`Scroll` coordinates.**
    /// Those are sent in the engine's own *physical-pixel surface* space,
    /// the exact same space as `HostToEngine::Resize`'s `width`/`height`
    /// (see `ipc_protocol::InputEvent`'s doc comment) — every engine-host
    /// renders a surface of precisely that pixel size, so a physical offset
    /// into the on-screen content quad is already the right value to send,
    /// with no conversion at all. Used instead for the two places that
    /// genuinely need logical points: saving window geometry
    /// (`Settings::window_width`) and placing egui UI at a coordinate an
    /// engine reported back (`EngineToHost::ContextMenuLink`).
    pub(crate) fn ppp(&self) -> f64 {
        self.window.as_ref().map_or(1.0, |w| w.scale_factor())
    }

    pub(crate) fn chrome_height_points(&self) -> f32 {
        match self.view_mode {
            ViewMode::Item => CHROME_HEIGHT_POINTS,
            ViewMode::Graph | ViewMode::Settings => GRAPH_TOOLBAR_HEIGHT_POINTS,
        }
    }

    /// Item view showing a Tab -- the case every engine-facing path (input
    /// forwarding, content panes, address bar, find bar) cares about.
    pub(crate) fn showing_tab_view(&self) -> bool {
        self.view_mode == ViewMode::Item && self.viewed_image.is_none()
    }

    pub(crate) fn showing_image_view(&self) -> bool {
        self.view_mode == ViewMode::Item && self.viewed_image.is_some()
    }

    pub(crate) fn screen_size_points(&self) -> egui::Vec2 {
        let screen = self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let ppp = self.ppp() as f32;
        egui::vec2(screen.width as f32 / ppp, screen.height as f32 / ppp)
    }

    /// Item view's content area (points, screen space): everything below
    /// the toolbar.
    pub(crate) fn item_body_rect(&self) -> egui::Rect {
        let screen = self.screen_size_points();
        egui::Rect::from_min_max(
            egui::pos2(0.0, CHROME_HEIGHT_POINTS),
            egui::pos2(screen.x, screen.y.max(CHROME_HEIGHT_POINTS)),
        )
    }

    /// Keeps the OS window title in sync with the focused tab's own title
    /// -- previously hardcoded to `"manifold"` regardless of
    /// what was open, unlike every real browser's own "the window
    /// represents the page you're looking at" convention. Reflects
    /// `self.focused` regardless of `view_mode` (it's "the last tab you
    /// were looking at" even from Graph/Settings, same reasoning
    /// `self.focused` already uses as the default parent for a new tab
    /// there). Falls back to the url if the title hasn't arrived yet
    /// (early in a fresh navigation), and to the plain app name if nothing
    /// is focused at all (no tabs open, or a tab that's since vanished).
    /// Called once a frame; only actually calls `Window::set_title` when
    /// the computed title changed from `last_window_title`. In Image view,
    /// the image's filename instead.
    pub(crate) fn update_window_title(&mut self) {
        let Some(window) = &self.window else { return };
        let image_title = self
            .viewed_image
            .as_ref()
            .filter(|_| self.view_mode == ViewMode::Item)
            .and_then(|id| self.nodes.get(id))
            .and_then(|n| n.as_image())
            .map(|i| download_filename(&i.path).to_owned());
        let title = image_title
            .or_else(|| {
                self.focused.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|n| {
                    if n.title.is_empty() {
                        n.url.clone()
                    } else {
                        n.title.clone()
                    }
                })
            })
            .filter(|t| !t.is_empty())
            .map(|t| format!("{t} — manifold"))
            .unwrap_or_else(|| "manifold".to_string());
        if title != self.last_window_title {
            window.set_title(&title);
            self.last_window_title = title;
        }
    }

    pub(crate) fn chrome_height_px(&self) -> u32 {
        let ppp = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        (self.chrome_height_points() * ppp).round() as u32
    }

    /// The engine-host's rendered surface only needs to cover the area
    /// below chrome — telling it to render at the full window size (when
    /// the compositor only ever displays it in the smaller content
    /// viewport, see `gpu::GpuState::render`) would waste work and,
    /// because the viewport stretches whatever the engine sends to fill
    /// it, visibly squash the page. Halved (no divider gap reserved) while
    /// `split_tab` is set -- both the primary and secondary pane are
    /// exactly half the window wide on screen, so each engine should
    /// render at that same real footprint rather than the full width, the
    /// same "match the actual on-screen size" reasoning this already
    /// applied to chrome height. `send_resize_to` is the only caller, and
    /// both `focused` and `split_tab` get resent through it whenever this
    /// value would change for either of them (window resize, or a split
    /// opening/closing) -- see those call sites.
    fn content_size_px(&self) -> (u32, u32) {
        let full = self.window.as_ref().map(|w| w.inner_size()).unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let height = full.height.saturating_sub(self.chrome_height_px());
        let width = if self.split_tab.is_some() { full.width / 2 } else { full.width };
        (width, height)
    }

    /// The on-screen boundary (physical pixels, x) between the primary and
    /// secondary pane while a split is open -- `None` when there's no
    /// split, meaning the whole content width belongs to the primary pane.
    /// `window_event`'s mouse handlers only ever forward to the primary
    /// pane (`self.focused`) in this first cut -- the secondary pane is
    /// view-only, see `split_tab`'s own doc comment -- so they gate on the
    /// cursor being left of this boundary (or there being no split at all)
    /// on top of the existing below-chrome check.
    pub(crate) fn split_divider_x_px(&self) -> Option<f64> {
        self.split_tab.as_ref()?;
        let width = self.window.as_ref().map_or(1024, |w| w.inner_size().width);
        Some(width as f64 / 2.0)
    }

    pub(crate) fn send_resize_to(&mut self, tab_id: &str) {
        let (width, height) = self.content_size_px();
        let dpi_scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            node.tab.send(&HostToEngine::Resize { width, height, dpi_scale });
        }
    }

    /// Sends `tab_id`'s own `TabPayload::zoom_factor` to its engine — called
    /// right after spawning it (already initialized from `Settings::
    /// zoom_factor` at that point, see `TabPayload::zoom_factor`'s own doc
    /// comment) and again whenever `set_focused_zoom` changes that specific
    /// tab's value. Deliberately reads the *tab's own* field, not `self.
    /// settings.zoom_factor` directly -- see `set_focused_zoom`'s own doc
    /// comment for the bug this replaced.
    pub(crate) fn send_zoom_to(&mut self, tab_id: &str) {
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            let factor = node.zoom_factor;
            node.tab.send(&HostToEngine::SetZoom { factor });
        }
    }

    /// The focused tab's own real zoom, for the Settings page's display and
    /// as the base the `+`/`-` shortcuts adjust from -- falls back to
    /// `Settings::zoom_factor` (the default for new tabs) only when nothing
    /// is focused at all (e.g. Graph view, nothing selected), since there's
    /// no tab-specific value to show in that case.
    pub(crate) fn focused_zoom_factor(&self) -> f64 {
        self.focused
            .as_deref()
            .and_then(|id| self.nodes.get(id))
            .and_then(|n| n.as_tab())
            .map(|t| t.zoom_factor)
            .unwrap_or(self.settings.zoom_factor)
    }

    /// Sets `factor` as both the new default for future tabs (`Settings::
    /// zoom_factor`, persisted) and, immediately, the focused tab's own
    /// zoom -- replaces the old `apply_zoom_change`, which only ever
    /// touched the shared global value and re-sent *that* to the focused
    /// tab. Confirmed live to cause a real bug: zooming tab A then
    /// switching to tab B left the Settings page's zoom control showing
    /// tab A's value (the stale global) while tab B's engine was still at
    /// its own different real zoom -- pressing "-" from that display then
    /// jumped tab B's actual zoom by an unexpected amount, not the 10%
    /// step the button claimed. Now every read/write of "the current zoom"
    /// goes through this pair (`focused_zoom_factor`/`set_focused_zoom`)
    /// instead of `self.settings.zoom_factor` directly, so the display and
    /// the tab it's supposedly controlling can't diverge.
    pub(crate) fn set_focused_zoom(&mut self, factor: f64) {
        self.settings.zoom_factor = factor;
        self.save_settings();
        if let Some(id) = self.focused.clone() {
            if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                node.zoom_factor = factor;
            }
            self.send_zoom_to(&id);
        }
    }

    pub(crate) fn navigate_tab(&mut self, tab_id: &str, url: String) {
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            // Recorded here, not left to `NavigationStateChanged`'s own
            // history-recording: this sets `node.url` immediately
            // (synchronously, before the engine round-trips its own
            // confirmation), so that handler's own `node.url != url` check
            // would already be false by the time its event arrives,
            // silently missing every address-bar-driven navigation. Title
            // isn't known yet at this point (the page hasn't loaded), so
            // this entry shows as its bare url until superseded by a later
            // one -- `build_history_popup` already falls back to the url
            // for an empty title.
            if node.url != url {
                self.history.push(history::HistoryEntry { title: String::new(), url: url.clone() });
                if self.history.len() > history::MAX_HISTORY_ENTRIES {
                    self.history.remove(0);
                }
                self.history_dirty = true;
                self.history_matches_cache = None;
            }
        }
        if let Some(node) = self.nodes.get_mut(tab_id).and_then(|n| n.as_tab_mut()) {
            if node.url != url {
                self.spawns
                    .navigated_from
                    .insert(tab_id.to_string(), (node.url.clone(), std::time::Instant::now(), false));
            }
            node.url = url.clone();
            node.tab.send(&HostToEngine::Navigate { url: url.clone(), referrer: None });
            // Until the engine labels the navigation, a report can still
            // carry the old url (CEF reported loading before the new address,
            // unlabelled): that isn't the user's either.
            self.note_host_load(tab_id);
            // The typed url, whatever the page then redirects to (2b-3 Ruling 4).
            self.edit_url(tab_id, &url);
            // Saved now so other replicas follow at once. Only a save that
            // doesn't commit leaves the edit for a later one.
            self.save_session();
        }
    }

    /// Address-bar autocomplete's open-tab source: every *other* tab (an
    /// exact-match-only self isn't a useful "switch to" suggestion) whose
    /// URL or title contains `query`, case-insensitively. Capped and
    /// ordered by `tab_order` for determinism — this is a small in-memory
    /// list, not worth a fuzzy-match/relevance-ranking pass.
    pub(crate) fn compute_open_tab_suggestions(&self, query: &str, exclude: &str) -> Vec<Suggestion> {
        if query.is_empty() {
            return Vec::new();
        }
        let query = query.to_lowercase();
        self.node_order
            .iter()
            .filter(|id| id.as_str() != exclude)
            .filter_map(|id| self.nodes.get(id).and_then(|n| n.runtime.listed_tab()).map(|tab| (id, tab)))
            .filter(|(_, (title, url))| url.to_lowercase().contains(&query) || title.to_lowercase().contains(&query))
            .take(5)
            .map(|(id, (title, url))| Suggestion::OpenTab {
                tab_id: id.clone(),
                url: url.to_string(),
                title: title.to_string(),
            })
            .collect()
    }

    /// Accepting a suggestion: jump straight to an already-open tab (no
    /// navigation at all), or navigate the focused tab to a search.
    /// Either way the dropdown itself is cleared and the address bar drops
    /// out of its "actively editing" state so it re-syncs to whichever
    /// tab ends up focused, same as any other navigation.
    pub(crate) fn accept_suggestion(&mut self, suggestion: Suggestion, focused_tab_id: &str) {
        match suggestion {
            Suggestion::OpenTab { tab_id, .. } => self.focus_tab(&tab_id),
            Suggestion::Search { query } => {
                let url = self.settings.default_search_engine.search_url(&query);
                self.navigate_tab(focused_tab_id, url);
            }
        }
        self.address_bar.dismiss();
        self.address_bar.focused = false;
    }
}
