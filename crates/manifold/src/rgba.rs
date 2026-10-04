//! Size rules for pixels headed to a GPU texture: the device's 2D side limit,
//! and decoding with a cap so a hostile file can't ask for gigabytes.

/// `wgpu::Limits::default().max_texture_dimension_2d`, which `GpuState::new`
/// requests. A texture past it is a wgpu validation error (a panic under the
/// default handler).
pub(crate) const MAX_TEXTURE_SIDE: u32 = 8192;

/// Favicons are drawn at most ~32 logical px; 64 covers a 2x display.
pub(crate) const FAVICON_SIDE: u32 = 64;

/// `(width, height)` scaled down, keeping the aspect ratio, so neither side
/// exceeds `max`. Never returns a 0 side.
pub(crate) fn fit_within(width: u32, height: u32, max: u32) -> (u32, u32) {
    if width <= max && height <= max {
        return (width, height);
    }
    let scale = f64::from(max) / f64::from(width.max(height));
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max);
    (side(width), side(height))
}

/// `img`, downscaled to `fit_within(.., max)` if it's larger.
pub(crate) fn fit_image(img: image::RgbaImage, max: u32) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    let (fw, fh) = fit_within(w, h, max);
    if (fw, fh) == (w, h) {
        return img;
    }
    image::imageops::resize(&img, fw, fh, image::imageops::FilterType::Triangle)
}

/// Decodes `bytes` (format guessed from content), refusing an image wider or
/// taller than `max_side` before its pixels are allocated.
pub(crate) fn decode_limited(bytes: &[u8], max_side: u32) -> image::ImageResult<image::RgbaImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    reader.limits(limits);
    Ok(reader.decode()?.into_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_within_the_limit_is_kept() {
        assert_eq!(fit_within(100, 50, 8192), (100, 50));
        assert_eq!(fit_within(8192, 8192, 8192), (8192, 8192));
    }

    #[test]
    fn an_oversize_image_is_scaled_to_the_limit_keeping_its_aspect() {
        assert_eq!(fit_within(11648, 8736, 8192), (8192, 6144));
        assert_eq!(fit_within(9000, 1, 8192), (8192, 1), "a side never drops to 0");
        assert_eq!(fit_within(1, 9000, 8192), (1, 8192));
    }

    #[test]
    fn the_limit_matches_the_device_limits_gpu_requests() {
        // GpuState::new requests `wgpu::Limits::default()`; if that ever changes, this must too.
        assert_eq!(wgpu::Limits::default().max_texture_dimension_2d, MAX_TEXTURE_SIDE);
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(width, height).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn a_decode_past_the_side_limit_is_refused() {
        assert!(decode_limited(&png(600, 1), 512).is_err());
        assert_eq!(decode_limited(&png(100, 40), 512).unwrap().dimensions(), (100, 40));
    }

    #[test]
    fn fit_image_downscales_only_when_needed() {
        assert_eq!(fit_image(image::RgbaImage::new(100, 100), FAVICON_SIDE).dimensions(), (64, 64));
        assert_eq!(fit_image(image::RgbaImage::new(16, 16), FAVICON_SIDE).dimensions(), (16, 16));
    }
}
