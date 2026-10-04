//! One-shot background PDF-to-image rendering for `NodePayload::Pdf` --
//! mirrors `favicon_fetch`'s own "spawn a thread, `mpsc` channel, main loop
//! `tick()`s it" pattern, but for a one-time render triggered by the user
//! adding a PDF node, not a per-navigation fetch. Both halves of the work
//! are genuinely slow the first time -- `pdfium-auto`'s own download+cache
//! of the real `pdfium.dll` (a real network fetch, only once per machine)
//! and then loading/rendering a PDF through it -- so keeping all of it off
//! the UI thread matters here even more than it does for a favicon's own
//! small HTTP fetch.

use std::collections::HashSet;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use pdfium_render::prelude::{PdfRenderConfig, Pdfium, PdfiumError};

/// Capped well above `IMAGE_NODE_MAX_SIZE` (`main.rs`) so the rendered page
/// still looks sharp at the enlarged peek-preview size too, without paying
/// for a full-resolution render of every page a user happens to add --
/// same reasoning `THUMBNAIL_MAX_SIZE` already has in `gpu.rs`.
const PDF_RENDER_MAX_DIMENSION: i32 = 1000;

pub struct PdfRenderResult {
    pub width: u32,
    pub height: u32,
    pub page_count: usize,
    pub rgba: Vec<u8>,
}

struct Request {
    id: String,
    path: String,
}

/// Ids whose node was deleted before their render ran (`PdfFetcher::cancel`).
type CancelSet = Arc<Mutex<HashSet<String>>>;

pub struct PdfFetcher {
    tx: mpsc::Sender<Request>,
    rx: mpsc::Receiver<(String, Result<PdfRenderResult, String>)>,
    cancelled: CancelSet,
}

impl PdfFetcher {
    pub fn new() -> Self {
        let (tx, thread_rx) = mpsc::channel::<Request>();
        let (result_tx, rx) = mpsc::channel();
        let cancelled = CancelSet::default();
        let worker_cancelled = Arc::clone(&cancelled);
        thread::spawn(move || {
            // Bound on the first request, not at launch (a first-time bind
            // can download the DLL). A success is kept for the run; a
            // failure is retried next request (`bind_once`).
            let mut pdfium: Option<Pdfium> = None;
            for request in thread_rx {
                if take_cancelled(&worker_cancelled, &request.id) {
                    continue;
                }
                let result = bind_once(&mut pdfium, || pdfium_auto::bind_pdfium_silent().map_err(|e| e.to_string()))
                    .and_then(|pdfium| render_first_page(pdfium, &request.path));
                let _ = result_tx.send((request.id, result));
            }
        });
        Self { tx, rx, cancelled }
    }

    /// Queues a render of `path`'s first page for node `id`. Clears any
    /// cancel mark for `id`: one left by deleting an already-rendered PDF
    /// (no request pending to take it) would otherwise skip this one.
    pub fn request(&self, id: String, path: String) {
        take_cancelled(&self.cancelled, &id);
        let _ = self.tx.send(Request { id, path });
    }

    /// Node `id` was deleted: skip its render if it hasn't started yet.
    pub fn cancel(&self, id: &str) {
        if let Ok(mut set) = self.cancelled.lock() {
            set.insert(id.to_string());
        }
    }

    /// Drains every render that's finished since the last call -- same
    /// "tick once a frame" shape `SuggestionFetcher`/`FaviconFetcher`
    /// already have. A result that came back despite a cancel (it was
    /// already rendering) drops its mark.
    pub fn tick(&self) -> Vec<(String, Result<PdfRenderResult, String>)> {
        self.rx
            .try_iter()
            .inspect(|(id, _)| {
                take_cancelled(&self.cancelled, id);
            })
            .collect()
    }
}

/// The bound value in `slot`, binding it first if needed. Only success is
/// kept: a failed bind (offline on the first PDF of a run, or a failed
/// first-time DLL download) is retried on the next request.
fn bind_once<T, E>(slot: &mut Option<T>, bind: impl FnOnce() -> Result<T, E>) -> Result<&T, E> {
    if slot.is_none() {
        *slot = Some(bind()?);
    }
    Ok(slot.as_ref().expect("just bound"))
}

/// Whether `id` was cancelled (its node deleted) before its render ran;
/// removes the mark.
fn take_cancelled(cancelled: &Mutex<HashSet<String>>, id: &str) -> bool {
    cancelled.lock().map(|mut set| set.remove(id)).unwrap_or(false)
}

/// `PdfiumError`'s own `Display` is its pretty-printed `Debug` (a missing
/// file comes out as a nine-line `IoError(Os { .. })`), too much for a toast.
fn pdfium_error(err: PdfiumError) -> String {
    match err {
        PdfiumError::IoError(err) => err.to_string(),
        other => format!("{other:?}"),
    }
}

fn render_first_page(pdfium: &Pdfium, path: &str) -> Result<PdfRenderResult, String> {
    let document = pdfium.load_pdf_from_file(path, None).map_err(pdfium_error)?;
    let pages = document.pages();
    let page_count = pages.len() as usize;
    let page = pages.get(0).map_err(pdfium_error)?;
    let render_config =
        PdfRenderConfig::new().set_target_width(PDF_RENDER_MAX_DIMENSION).set_maximum_height(PDF_RENDER_MAX_DIMENSION);
    let bitmap = page.render_with_config(&render_config).map_err(pdfium_error)?;
    let width = bitmap.width() as u32;
    let height = bitmap.height() as u32;
    Ok(PdfRenderResult { width, height, page_count, rgba: bitmap.as_rgba_bytes() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfium_error_is_one_line() {
        let err = PdfiumError::IoError(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert_eq!(pdfium_error(err), "gone");
        assert!(!pdfium_error(PdfiumError::PageIndexOutOfBounds).contains('\n'));
    }

    /// Review D-B4: a failed bind isn't cached; the next request tries again.
    #[test]
    fn a_failed_bind_is_retried_and_a_good_one_kept() {
        let mut slot: Option<u32> = None;
        let calls = std::cell::Cell::new(0);
        let bind = |ok: bool| {
            calls.set(calls.get() + 1);
            if ok {
                Ok(7)
            } else {
                Err("offline".to_string())
            }
        };
        assert!(bind_once(&mut slot, || bind(false)).is_err());
        assert_eq!(bind_once(&mut slot, || bind(true)), Ok(&7));
        assert_eq!(bind_once(&mut slot, || bind(false)), Ok(&7), "kept, not rebound");
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn a_cancelled_request_is_skipped() {
        let cancelled = std::sync::Mutex::new(std::collections::HashSet::from(["gone".to_string()]));
        assert!(take_cancelled(&cancelled, "gone"));
        assert!(!take_cancelled(&cancelled, "gone"), "taken once");
        assert!(!take_cancelled(&cancelled, "kept"));
    }

    /// A mark left by deleting a PDF with no render pending (already
    /// rendered) mustn't skip a later request for the same id.
    #[test]
    fn a_new_request_clears_a_stale_cancel() {
        let (tx, thread_rx) = mpsc::channel();
        let (_result_tx, rx) = mpsc::channel();
        let fetcher = PdfFetcher { tx, rx, cancelled: Default::default() };
        fetcher.cancel("a");
        fetcher.request("a".to_string(), "a.pdf".to_string());
        assert_eq!(thread_rx.try_recv().map(|r| r.id), Ok("a".to_string()));
        assert!(!take_cancelled(&fetcher.cancelled, "a"));
    }
}
