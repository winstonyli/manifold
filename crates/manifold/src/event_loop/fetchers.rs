//! The per-tick glue between the event loop and the fetchers: tab events, the idle sweep and image decodes.

use super::*;

/// The background fetchers and decoders `tick_fetchers` drains (`App::workers`). `default()` keeps no files; `App::load`
/// gives the favicon and redirect fetchers their directories.
pub(crate) struct Workers {
    /// Background fetcher for `favicon_fetch::fetch_favicon` -- see that
    /// module's own doc comment for why favicons are fetched independently
    /// here rather than aggregated from each engine's own (inconsistent)
    /// native favicon API.
    pub(crate) favicon_fetcher: FaviconFetcher,
    /// See `PdfPayload::outcome`'s own doc comment.
    pub(crate) pdf_renderer: PdfRenderer,
    /// Decodes image nodes' files off the UI thread (`insert_image_node_with_id`).
    pub(crate) image_decoder: ImageDecoder,
    /// See `redirect_fetch`'s own module doc comment.
    pub(crate) redirect_fetcher: RedirectFetcher,
    /// Origins already resolved (successfully or not) by `favicon_fetcher`
    /// -- checked before requesting again, so a failed origin (no
    /// `/favicon.ico`, a network error, undecodable bytes) isn't refetched
    /// on every navigation back to it. A *successful* fetch's actual pixel
    /// data lives in `GpuState::favicons` (keyed by the same origin
    /// string), not duplicated here -- this only needs to remember whether
    /// resolution was already attempted, not the result itself.
    pub(crate) favicon_resolved: HashSet<String>,
}

impl Default for Workers {
    fn default() -> Self {
        Self {
            favicon_fetcher: FaviconFetcher::new(None),
            pdf_renderer: PdfRenderer::new(),
            image_decoder: ImageDecoder::new(),
            redirect_fetcher: RedirectFetcher::new(None),
            favicon_resolved: HashSet::new(),
        }
    }
}

impl App {
    /// Drains every tab's `events_rx` (not just the focused one — a
    /// background tab can still crash, or finish an in-flight swap, while
    /// suspended), applies every message but frames in arrival order through
    /// `handle_view_message`, and then uploads each tab's newest frame.
    /// Draining first means no tab node stays borrowed while a message is
    /// applied, so any message can touch any node. Frames go last because a
    /// crash or swap in the same drain replaces the tab they came from.
    /// The order is load-bearing: ipc may remove tabs before the idle
    /// sweep looks at them, the sweep and the fetches can remove tabs
    /// too, and frame uploads run last so none lands on a removed tab.
    pub(crate) fn drain_tab_events(&mut self) {
        let frame_updates = self.drain_view_ipc();
        self.poll_text_timers(std::time::Instant::now());
        self.idle_sweep();
        self.tick_fetchers();
        self.upload_frames(frame_updates);
    }

    /// Puts idle tabs to sleep (`sweep_idle_tabs`) when `idle_clock` says a sweep is due.
    fn idle_sweep(&mut self) {
        if self.idle_clock.due(std::time::Instant::now()) {
            let limit =
                self.settings.value.idle_sleep_minutes.map(|m| std::time::Duration::from_secs(u64::from(m) * 60));
            self.sweep_idle_tabs(limit);
        }
    }

    /// Waits for every queued image decode and applies the results, as the frames would.
    #[cfg(test)]
    pub(crate) fn finish_image_decodes(&mut self) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while self.workers.image_decoder.pending() > 0 && std::time::Instant::now() < end {
            self.tick_fetchers();
            std::thread::yield_now();
        }
        self.tick_fetchers();
    }

    /// Applies what the favicon fetcher, PDF renderer and redirect fetcher finished since the last drain.
    fn tick_fetchers(&mut self) {
        for (origin, result) in self.workers.favicon_fetcher.tick() {
            self.workers.favicon_resolved.insert(origin.clone());
            if let Some(image) = result {
                if let Some(gpu) = &mut self.gpu {
                    gpu.upload_favicon(&origin, &image.rgba, image.width, image.height);
                }
            }
        }

        for (id, result) in self.workers.image_decoder.tick() {
            // A node deleted (or already failed and retried) while its decode ran has nothing to receive it.
            if self.nodes.get(&id).and_then(|n| n.as_image()).is_none() {
                continue;
            }
            match result {
                Ok(decoded) => {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.upload_image(id, &decoded.rgba, decoded.width, decoded.height);
                    }
                    if let Some(image) = self.nodes.get_mut(&id).and_then(|n| n.as_image_mut()) {
                        (image.width, image.height, image.error) = (decoded.width, decoded.height, None);
                    }
                }
                Err(error) => {
                    self.queue_toast(error.clone());
                    if let Some(image) = self.nodes.get_mut(&id).and_then(|n| n.as_image_mut()) {
                        image.error = Some(error);
                    }
                }
            }
        }

        for (id, result) in self.workers.pdf_renderer.tick() {
            // The node may have been deleted while this render was still
            // in flight -- skip a late-arriving result entirely rather
            // than uploading a texture nothing will ever free (`delete`'s
            // own `gpu.free_image` already ran, before this result could
            // possibly have arrived, so a late upload here would be a real
            // leak, not just wasted work).
            if self.nodes.get(&id).and_then(|n| n.as_pdf()).is_none() {
                continue;
            }
            match result {
                Ok(rendered) => {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.upload_image(id, &rendered.rgba, rendered.width, rendered.height);
                    }
                    if let Some(pdf) = self.nodes.get_mut(&id).and_then(|n| n.as_pdf_mut()) {
                        eprintln!(
                            "pdf: rendered {}: {}x{}, {} page(s)",
                            pdf.path, rendered.width, rendered.height, rendered.page_count
                        );
                        pdf.outcome = Some(Ok(PdfOutcome {
                            width: rendered.width,
                            height: rendered.height,
                            page_count: rendered.page_count,
                        }));
                    }
                }
                // Toasted like a failed image/JSON load, and kept as the
                // card's tooltip with the file named.
                Err(err) => {
                    let Some(path) = self.nodes.get(&id).and_then(|n| n.as_pdf()).map(|p| p.path.clone()) else {
                        continue;
                    };
                    eprintln!("pdf: couldn't open {path}: {err}");
                    let name = crate::json_file::file_name(std::path::Path::new(&path));
                    let err = format!("Couldn't open PDF {name}: {err}");
                    self.queue_toast(err.clone());
                    if let Some(pdf) = self.nodes.get_mut(&id).and_then(|n| n.as_pdf_mut()) {
                        pdf.outcome = Some(Err(err));
                    }
                    // No refetch here: a missing file plus a known URL is
                    // already handled before this can ever be reached --
                    // `materialize`'s own `exists()` check inserts the
                    // placeholder directly (never starting a render) and
                    // `retry_missing_file`'s URL branch covers a click on
                    // the card. Refetching here too would double-fetch a
                    // restore-time miss (two threads writing the same
                    // `<id>.pdf`) and loop forever on a `%PDF-`-shaped file
                    // that simply fails to render.
                }
            }
        }

        for FetchResult { job, body } in self.workers.redirect_fetcher.tick() {
            let Some(body) = body else {
                // A tab whose fetch failed just stays an ordinary tab (see
                // `RedirectFetcher::tick`'s own doc comment) -- silent by
                // design. A Refresh failing this way would otherwise vanish
                // with no feedback at all, since it never reaches
                // `reload_fetched_node`'s own toast.
                if let FetchTarget::Node(id) = job.target {
                    self.refresh_fetch_failed(id);
                }
                continue;
            };
            match job.target {
                // The tab may have been closed, or navigated somewhere else,
                // while this fetch was still in flight -- only convert it if
                // it's still the same tab still showing the same url that
                // triggered the fetch in the first place.
                FetchTarget::Tab(tab_id) => {
                    let still_current =
                        self.nodes.get(&tab_id).and_then(|n| n.as_tab()).is_some_and(|t| t.url == job.url);
                    if still_current {
                        self.redirect_tab_to_content(tab_id, job.node_id, job.url, body);
                    }
                }
                FetchTarget::Node(id) => self.reload_fetched_node(id, body),
            }
        }
    }
}
