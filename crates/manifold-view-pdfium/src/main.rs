//! One-shot PDF view: `manifold-view-pdfium <path>` renders the first page and writes one frame to stdout
//! (`ipc_protocol::render`). A failure prints one line on stderr and exits 1; a PDFium crash is the process dying,
//! which the app reports as a crashed renderer instead of dying itself.

use ipc_protocol::render::{write_frame, Frame};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium, PdfiumError};

/// Capped well above the card's size so the page still looks sharp in the enlarged peek preview, without paying for a
/// full-resolution render of every page.
const PDF_RENDER_MAX_DIMENSION: i32 = 1000;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: manifold-view-pdfium <path>");
        std::process::exit(1);
    };
    let result = pdfium_auto::bind_pdfium_silent()
        .map_err(|e| e.to_string())
        .and_then(|pdfium| render_first_page(&pdfium, &path));
    match result {
        Ok(frame) => {
            if let Err(e) = write_frame(&mut std::io::stdout().lock(), &frame) {
                eprintln!("couldn't write the frame: {e}");
                std::process::exit(1);
            }
        }
        Err(reason) => {
            eprintln!("{reason}");
            std::process::exit(1);
        }
    }
}

/// `PdfiumError`'s own `Display` is its pretty-printed `Debug` (a missing file comes out as a nine-line
/// `IoError(Os { .. })`), too much for a toast.
fn pdfium_error(err: PdfiumError) -> String {
    match err {
        PdfiumError::IoError(err) => err.to_string(),
        other => format!("{other:?}"),
    }
}

fn render_first_page(pdfium: &Pdfium, path: &str) -> Result<Frame, String> {
    let document = pdfium.load_pdf_from_file(path, None).map_err(pdfium_error)?;
    let pages = document.pages();
    let page_count = pages.len() as u32;
    let page = pages.get(0).map_err(pdfium_error)?;
    let render_config =
        PdfRenderConfig::new().set_target_width(PDF_RENDER_MAX_DIMENSION).set_maximum_height(PDF_RENDER_MAX_DIMENSION);
    let bitmap = page.render_with_config(&render_config).map_err(pdfium_error)?;
    Ok(Frame { width: bitmap.width() as u32, height: bitmap.height() as u32, page_count, rgba: bitmap.as_rgba_bytes() })
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
