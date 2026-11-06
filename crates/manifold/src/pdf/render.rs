//! One-shot background PDF-to-image rendering for `Runtime::Pdf` --
//! runs on a `worker::Pool` like `favicon_fetch`'s fetches, with the main loop
//! `tick()`ing it, but for a one-time render triggered by the user
//! adding a PDF node, not a per-navigation fetch. The render itself runs in
//! the `manifold-view-pdfium` process (`manifold_view_protocol::render`), so a PDFium
//! crash costs one card its preview instead of the app; the first run also
//! downloads the real `pdfium.dll` (a network fetch, once per machine), so
//! none of it belongs on the UI thread.

use crate::store::model::NodeId;
use crate::worker::Pool;
use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;

use manifold_view_protocol::render::{Cancel, Inbox};

/// Renders running at once.
const MAX_RENDERS: usize = 2;
/// Covers the one-time PDFium download as well as a render; a PDF that hangs before its frame costs a worker this long
/// at most (one that hangs after a bad frame: PARKED "Host protocol stamp").
const RENDER_TIMEOUT: Duration = Duration::from_secs(120);
const LABEL: &str = "PDF renderer";

pub struct PdfRenderResult {
    pub width: u32,
    pub height: u32,
    pub page_count: usize,
    pub rgba: Vec<u8>,
}

/// A render's job: the file, and the `Inbox` its `Cancel` (kept in `PdfRenderer::cancels`) talks to. `render` takes
/// the inbox by value, so the job holds it in a `Cell` for `work`, which only borrows the job, to take out.
type Job = (String, Cell<Option<Inbox>>);

pub struct PdfRenderer {
    pool: Pool<NodeId, Job, Result<PdfRenderResult, String>>,
    /// The `Cancel` of every live render, UI-thread only: removed when the render's result is delivered or the render
    /// is cancelled, which is how every live job ends.
    cancels: HashMap<NodeId, Cancel>,
}

impl PdfRenderer {
    /// `Workers::new` passes `render`, tests a stub.
    pub(crate) fn new(render: impl Fn(&str, Inbox) -> Result<PdfRenderResult, String> + Send + Sync + 'static) -> Self {
        let pool = Pool::new(MAX_RENDERS, move |(path, inbox): &Job| {
            render(path, inbox.take().expect("a pool runs a job once"))
        });
        Self { pool, cancels: HashMap::new() }
    }

    /// Queues a render of `path`'s first page for node `id`, replacing (and killing) any earlier one for it.
    pub fn request(&mut self, id: NodeId, path: String) {
        self.cancel(id);
        let (cancel, inbox) = manifold_view_protocol::render::channel();
        self.cancels.insert(id, cancel);
        self.pool.request(id, (path, Cell::new(Some(inbox))));
    }

    /// Node `id` left the live graph: skip its render if it hasn't started, kill it if it has, drop its result if it
    /// is done.
    pub fn cancel(&mut self, id: NodeId) {
        self.pool.cancel(&id);
        if let Some(cancel) = self.cancels.remove(&id) {
            cancel.cancel();
        }
    }

    /// Every render finished since the last call. A panicking render is an error for its node. A node deleted
    /// meanwhile gets nothing (`cancel`).
    pub fn tick(&mut self) -> Vec<(NodeId, Result<PdfRenderResult, String>)> {
        self.pool
            .tick()
            .into_iter()
            .map(|(id, _, result)| {
                self.cancels.remove(&id);
                (id, result.unwrap_or_else(|| Err("the renderer crashed".into())))
            })
            .collect()
    }

    /// How many renders have not been returned by `tick` yet.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.pool.pending()
    }

    /// See `Pool::wait_for_threads`.
    #[cfg(test)]
    pub(crate) fn wait_for_threads(&self, n: usize) {
        self.pool.wait_for_threads(n);
    }
}

/// Runs the PDF view on `path` in its own process.
pub(crate) fn render(path: &str, inbox: Inbox) -> Result<PdfRenderResult, String> {
    let exe = crate::process_manager::pdf_view_exe()
        .map_err(|e| format!("{LABEL} could not be extracted: {e}"))?
        .ok_or_else(|| format!("{LABEL} is not installed: build {}", manifold_hosts::PDFIUM.exe))?;
    let mut command = std::process::Command::new(exe);
    command.arg(path);
    let frame = manifold_view_protocol::render::run_child(command, LABEL, RENDER_TIMEOUT, inbox, |command| {
        // A just-extracted exe can be held for a moment by a scanner: the same retry the WebView2 spawn has.
        let child = crate::view_install::embedded_host::spawn_fresh(command)?;
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
    use crate::card_id::testutil::node;
    use std::sync::{mpsc, Mutex};

    const PATIENCE: Duration = Duration::from_secs(20);

    fn rendered() -> Result<PdfRenderResult, String> {
        Ok(PdfRenderResult { width: 1, height: 1, page_count: 1, rgba: Vec::new() })
    }

    /// Two renders of one id overlap (`reload_fetched_node`): the first finishing must leave the second cancellable.
    #[test]
    fn a_finished_render_leaves_a_later_render_of_its_id_cancellable() {
        let (started_tx, started) = mpsc::channel();
        let (release_a, gate_a) = mpsc::channel::<()>();
        let (release_b, gate_b) = mpsc::channel::<()>();
        let (started_tx, gate_a, gate_b) = (Mutex::new(started_tx), Mutex::new(gate_a), Mutex::new(gate_b));
        let mut renderer = PdfRenderer::new(move |path: &str, _| {
            started_tx.lock().unwrap().send(path.to_string()).unwrap();
            let gate = if path == "a" { &gate_a } else { &gate_b };
            let _ = gate.lock().unwrap().recv();
            rendered()
        });
        let id = node(3);
        renderer.request(id, "a".into());
        assert_eq!(started.recv_timeout(PATIENCE).unwrap(), "a");
        renderer.request(id, "b".into());
        assert_eq!(started.recv_timeout(PATIENCE).unwrap(), "b");
        drop(release_a);
        renderer.wait_for_threads(1);
        assert!(renderer.tick().is_empty());
        assert!(renderer.cancels.contains_key(&id));
        renderer.cancel(id);
        assert!(renderer.cancels.is_empty());
        assert_eq!(renderer.pending(), 0);
        drop(release_b);
    }

    /// Cancelling a queued render and requesting again leaves one job, not two.
    #[test]
    fn a_cancelled_queued_render_is_not_queued_twice() {
        let (started_tx, started) = mpsc::channel();
        let (release, gate) = mpsc::channel::<()>();
        let (started_tx, gate) = (Mutex::new(started_tx), Mutex::new(gate));
        let mut renderer = PdfRenderer::new(move |path: &str, _| {
            started_tx.lock().unwrap().send(path.to_string()).unwrap();
            let _ = gate.lock().unwrap().recv();
            rendered()
        });
        renderer.request(node(1), "first".into());
        renderer.request(node(2), "second".into());
        for _ in 0..MAX_RENDERS {
            started.recv_timeout(PATIENCE).unwrap();
        }
        renderer.request(node(3), "a".into());
        renderer.cancel(node(3));
        renderer.request(node(3), "b".into());
        drop(release);
        renderer.wait_for_threads(0);
        assert_eq!(started.try_iter().collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn a_panicking_render_is_an_error_for_that_node() {
        let mut renderer = PdfRenderer::new(|_: &str, _| panic!("stub render panics (expected in this test)"));
        renderer.request(node(1), "p.pdf".into());
        renderer.wait_for_threads(0);
        let done = renderer.tick();
        assert!(matches!(done.as_slice(), [(id, Err(e))] if *id == node(1) && e == "the renderer crashed"));
        assert!(renderer.cancels.is_empty());
    }
}
