//! One-shot background PDF-to-image rendering for `NodePayload::Pdf` --
//! mirrors `favicon_fetch`'s own "spawn a thread, `mpsc` channel, main loop
//! `tick()`s it" pattern, but for a one-time render triggered by the user
//! adding a PDF node, not a per-navigation fetch. Both halves of the work
//! are genuinely slow the first time -- `pdfium-auto`'s own download+cache
//! of the real `pdfium.dll` (a real network fetch, only once per machine)
//! and then loading/rendering a PDF through it -- so keeping all of it off
//! the UI thread matters here even more than it does for a favicon's own
//! small HTTP fetch.

use std::sync::mpsc;
use std::thread;

use pdfium_render::prelude::{Pdfium, PdfRenderConfig, PdfiumError};

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

pub struct PdfFetcher {
    tx: mpsc::Sender<Request>,
    rx: mpsc::Receiver<(String, Result<PdfRenderResult, String>)>,
}

impl PdfFetcher {
    pub fn new() -> Self {
        let (tx, thread_rx) = mpsc::channel::<Request>();
        let (result_tx, rx) = mpsc::channel();
        thread::spawn(move || {
            // Bound lazily, on the *first* real request -- not eagerly here
            // at thread-spawn time (i.e. at every app launch, whether or
            // not the user ever adds a PDF this run). `bind_pdfium_silent`
            // can mean a real network fetch the very first time it's ever
            // called on a machine, so paying that cost unconditionally at
            // startup would be exactly the kind of surprise cost this
            // project's own "extreme lag on startup" fix (see README) was
            // about avoiding. Bound once per request after that, not once
            // per request forever -- `bind_pdfium_silent` already skips the
            // download when a valid cached library is present, but there's
            // no reason to even pay that cheap check again once one bound
            // `Pdfium` handle already works for every later request.
            let mut pdfium: Option<Result<Pdfium, String>> = None;
            for request in thread_rx {
                let pdfium =
                    pdfium.get_or_insert_with(|| pdfium_auto::bind_pdfium_silent().map_err(|e| e.to_string()));
                let result = render_first_page(pdfium, &request.path);
                let _ = result_tx.send((request.id, result));
            }
        });
        Self { tx, rx }
    }

    pub fn request(&self, id: String, path: String) {
        let _ = self.tx.send(Request { id, path });
    }

    /// Drains every render that's finished since the last call -- same
    /// "tick once a frame" shape `SuggestionFetcher`/`FaviconFetcher`
    /// already have.
    pub fn tick(&self) -> Vec<(String, Result<PdfRenderResult, String>)> {
        self.rx.try_iter().collect()
    }
}

/// `PdfiumError`'s own `Display` is its pretty-printed `Debug` (a missing
/// file comes out as a nine-line `IoError(Os { .. })`), too much for a toast.
fn pdfium_error(err: PdfiumError) -> String {
    match err {
        PdfiumError::IoError(err) => err.to_string(),
        other => format!("{other:?}"),
    }
}

fn render_first_page(pdfium: &Result<Pdfium, String>, path: &str) -> Result<PdfRenderResult, String> {
    let pdfium = pdfium.as_ref().map_err(String::clone)?;
    let document = pdfium.load_pdf_from_file(path, None).map_err(pdfium_error)?;
    let pages = document.pages();
    let page_count = pages.len() as usize;
    let page = pages.get(0).map_err(pdfium_error)?;
    let render_config = PdfRenderConfig::new()
        .set_target_width(PDF_RENDER_MAX_DIMENSION)
        .set_maximum_height(PDF_RENDER_MAX_DIMENSION);
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
}
