//! One-shot background PDF-to-image rendering for `Runtime::Pdf` --
//! mirrors `favicon_fetch`'s own "spawn a thread, `mpsc` channel, main loop
//! `tick()`s it" pattern, but for a one-time render triggered by the user
//! adding a PDF node, not a per-navigation fetch. The render itself runs in
//! the `manifold-view-pdfium` process (`manifold_view_protocol::render`), so a PDFium
//! crash costs one card its preview instead of the app; the first run also
//! downloads the real `pdfium.dll` (a network fetch, once per machine), so
//! none of it belongs on the UI thread.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use manifold_view_protocol::render::Cancel;

const WORKERS: usize = 2;
/// Covers the one-time PDFium download as well as a render; a hung PDF costs a worker this long at most.
const RENDER_TIMEOUT: Duration = Duration::from_secs(120);
const LABEL: &str = "PDF renderer";

pub struct PdfRenderResult {
    pub width: u32,
    pub height: u32,
    pub page_count: usize,
    pub rgba: Vec<u8>,
}

/// Which renders are queued-but-cancelled and which child is running, under one lock so a cancel can't fall between
/// a worker's check and its registration.
#[derive(Default)]
struct State(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    cancelled: HashSet<String>,
    running: HashMap<String, Cancel>,
}

impl State {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    /// Node `id` was deleted: skip its render if queued, kill it if running.
    fn cancel(&self, id: &str) {
        let mut inner = self.lock();
        match inner.running.get(id) {
            Some(running) => running.cancel(),
            None => {
                inner.cancelled.insert(id.to_string());
            }
        }
    }
    /// A fresh request: a mark left by deleting an already-rendered PDF must not skip it.
    fn request(&self, id: &str) {
        self.lock().cancelled.remove(id);
    }
    /// A worker takes `id`: false when it was cancelled while queued, else registers `cancel` for it.
    fn begin(&self, id: &str, cancel: Cancel) -> bool {
        let mut inner = self.lock();
        if inner.cancelled.remove(id) {
            return false;
        }
        inner.running.insert(id.to_string(), cancel);
        true
    }
    fn finish(&self, id: &str) {
        self.lock().running.remove(id);
    }
    #[cfg(test)]
    fn running_ids(&self) -> Vec<String> {
        self.lock().running.keys().cloned().collect()
    }
    #[cfg(test)]
    fn is_cancelled(&self, id: &str) -> bool {
        self.lock().cancelled.contains(id)
    }
}

pub struct PdfRenderer {
    tx: mpsc::Sender<(String, String)>,
    rx: mpsc::Receiver<(String, Result<PdfRenderResult, String>)>,
    state: Arc<State>,
}

impl PdfRenderer {
    pub fn new() -> Self {
        let (tx, queue) = mpsc::channel::<(String, String)>();
        let (result_tx, rx) = mpsc::channel();
        let queue = Arc::new(Mutex::new(queue));
        let state = Arc::new(State::default());
        for _ in 0..WORKERS {
            let (queue, result_tx, state) = (Arc::clone(&queue), result_tx.clone(), Arc::clone(&state));
            thread::spawn(move || loop {
                // The lock is held only while waiting for the next request, never while rendering.
                let Ok((id, path)) = queue.lock().map(|q| q.recv()).unwrap_or(Err(mpsc::RecvError)) else { return };
                let (cancel, inbox) = manifold_view_protocol::render::channel();
                if !state.begin(&id, cancel) {
                    continue;
                }
                let result = render(&path, inbox);
                state.finish(&id);
                if result_tx.send((id, result)).is_err() {
                    return;
                }
            });
        }
        Self { tx, rx, state }
    }

    /// Queues a render of `path`'s first page for node `id`.
    pub fn request(&self, id: String, path: String) {
        self.state.request(&id);
        let _ = self.tx.send((id, path));
    }

    /// Node `id` was deleted: skip its render if it hasn't started, kill it if it has.
    pub fn cancel(&self, id: &str) {
        self.state.cancel(id);
    }

    /// Whether `id`'s render is marked to be skipped (`session_sync` tests).
    #[cfg(test)]
    pub(crate) fn is_cancelled(&self, id: &str) -> bool {
        self.state.is_cancelled(id)
    }

    /// Every render finished since the last call. A result for a node deleted meanwhile is the caller's to ignore.
    pub fn tick(&self) -> Vec<(String, Result<PdfRenderResult, String>)> {
        self.rx.try_iter().collect()
    }
}

/// Runs the PDF view on `path` in its own process.
fn render(path: &str, inbox: manifold_view_protocol::render::Inbox) -> Result<PdfRenderResult, String> {
    let exe = crate::process_manager::pdf_view_exe()
        .map_err(|e| format!("{LABEL} could not be extracted: {e}"))?
        .ok_or_else(|| format!("{LABEL} is not installed: build manifold-view-pdfium"))?;
    let mut command = std::process::Command::new(exe);
    command.arg(path);
    let frame = manifold_view_protocol::render::run_child(command, LABEL, RENDER_TIMEOUT, inbox, |command| {
        // A just-extracted exe can be held for a moment by a scanner: the same retry the WebView2 spawn has.
        let child = crate::embedded_host::spawn_fresh(command)?;
        crate::process_manager::contain_view_process(&child);
        Ok(child)
    })?;
    Ok(PdfRenderResult {
        width: frame.width,
        height: frame.height,
        page_count: frame.page_count as usize,
        rgba: frame.rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_before_the_render_starts_skips_it() {
        let state = State::default();
        state.cancel("gone");
        assert!(!state.begin("gone", manifold_view_protocol::render::channel().0), "skipped");
        assert!(state.begin("gone", manifold_view_protocol::render::channel().0), "the mark was taken once");
    }

    #[test]
    fn a_cancel_while_running_kills_instead_of_marking_and_a_finish_clears_it() {
        let state = State::default();
        assert!(state.begin("a", manifold_view_protocol::render::channel().0));
        state.cancel("a");
        assert!(!state.is_cancelled("a"), "a running render is killed, not marked for skipping");
        state.finish("a");
        assert!(state.running_ids().is_empty());
        assert!(
            state.begin("a", manifold_view_protocol::render::channel().0),
            "a later request for the same id still renders"
        );
    }

    #[test]
    fn a_new_request_clears_a_stale_cancel() {
        let state = State::default();
        state.cancel("a");
        state.request("a");
        assert!(state.begin("a", manifold_view_protocol::render::channel().0));
    }
}
