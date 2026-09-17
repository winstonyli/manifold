//! Minimal single-page PDF export for the "Print to PDF" toolbar button:
//! wraps an already-captured screenshot as one full-page JPEG image inside
//! a hand-written PDF, rather than pulling in a full PDF-generation crate
//! for what is, in this project's scope, always exactly one raster image.
//! Not a real paginated/text-reflowable print output — same "rasterized
//! visual snapshot, not the real thing" limitation this project's
//! thumbnails/screenshots already have (see `session.rs`'s own doc comment
//! for the same kind of scope tradeoff elsewhere in this project).

use std::path::Path;

const PAGE_WIDTH_PT: f32 = 612.0; // US Letter — matches most default printers.
const PAGE_HEIGHT_PT: f32 = 792.0;

/// Reads `image_path` (any format the `image` crate can decode), re-encodes
/// it as JPEG, and writes a one-page PDF at `pdf_path` with that JPEG
/// scaled to fit the page (preserving aspect ratio, centered). JPEG's
/// `DCTDecode` filter lets the raw encoded bytes go straight into the PDF
/// as the image stream's content with no further transcoding — a PDF
/// viewer hands `DCTDecode` streams to a JPEG decoder unchanged.
pub fn write_image_as_pdf(image_path: &Path, pdf_path: &Path) -> Result<(), String> {
    let img = image::open(image_path).map_err(|err| format!("failed to read {}: {err}", image_path.display()))?;
    let rgb = img.to_rgb8();
    let (width, height) = (rgb.width(), rgb.height());

    let mut jpeg_bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 85)
        .encode(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8)
        .map_err(|err| format!("failed to encode jpeg: {err}"))?;

    let img_aspect = width as f32 / height as f32;
    let page_aspect = PAGE_WIDTH_PT / PAGE_HEIGHT_PT;
    let (draw_w, draw_h) = if img_aspect > page_aspect {
        (PAGE_WIDTH_PT, PAGE_WIDTH_PT / img_aspect)
    } else {
        (PAGE_HEIGHT_PT * img_aspect, PAGE_HEIGHT_PT)
    };
    let x = (PAGE_WIDTH_PT - draw_w) / 2.0;
    let y = (PAGE_HEIGHT_PT - draw_h) / 2.0;
    // Standard PDF content-stream idiom: `cm` sets a transform that maps
    // the unit square to the image's on-page size/position, then `Do`
    // draws the named XObject (`/Im0`) into that transformed square.
    let content = format!("q {draw_w:.2} 0 0 {draw_h:.2} {x:.2} {y:.2} cm /Im0 Do Q");

    // Hand-written PDF: Catalog -> Pages -> one Page with one image
    // XObject, drawn via that trivial content stream. Byte offsets for the
    // xref table are recorded as each object is appended — the standard
    // (if tedious) approach for a document this simple, with no need for
    // a real PDF-writing library.
    let mut pdf = Vec::new();
    let mut offsets = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");

    macro_rules! obj {
        ($body:expr) => {{
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
            pdf.extend_from_slice($body.as_bytes());
            pdf.extend_from_slice(b"\nendobj\n");
        }};
    }

    obj!("<< /Type /Catalog /Pages 2 0 R >>".to_string());
    obj!("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string());
    obj!(format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH_PT} {PAGE_HEIGHT_PT}] \
         /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>"
    ));
    obj!(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));

    // Object 5's stream is binary (raw JPEG bytes), so it can't go through
    // the string-only `obj!` macro above — built by hand instead.
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"5 0 obj\n");
    pdf.extend_from_slice(
        format!(
            "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
             /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            jpeg_bytes.len()
        )
        .as_bytes(),
    );
    pdf.extend_from_slice(&jpeg_bytes);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    let xref_start = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_start}\n%%EOF", offsets.len() + 1)
            .as_bytes(),
    );

    std::fs::write(pdf_path, pdf).map_err(|err| format!("failed to write {}: {err}", pdf_path.display()))
}
