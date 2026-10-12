//! Background decoding of an image node's file (`ImagePayload`): a card is placed at once and its pixels arrive
//! later, as `PdfRenderer` does for a PDF page. Two workers share one queue, so a few big photos decode in parallel
//! and leave most cores free; the app takes finished images once a frame (`tick`) and uploads them itself.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

const WORKERS: usize = 2;

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A finished decode for node `id`; the error reads as a toast ("Couldn't open image name: why").
pub type Finished = (String, Result<Decoded, String>);

pub struct ImageDecoder {
    tx: mpsc::Sender<(String, String)>,
    rx: mpsc::Receiver<Finished>,
    /// Requests whose result `tick` has not yet returned.
    pending: Arc<AtomicUsize>,
}

impl ImageDecoder {
    pub fn new() -> Self {
        let (tx, queue) = mpsc::channel::<(String, String)>();
        let (result_tx, rx) = mpsc::channel();
        let queue = Arc::new(Mutex::new(queue));
        for _ in 0..WORKERS {
            let (queue, result_tx) = (Arc::clone(&queue), result_tx.clone());
            thread::spawn(move || loop {
                // The lock is held only while waiting for the next request, never while decoding.
                let Ok((id, path)) = queue.lock().map(|q| q.recv()).unwrap_or(Err(mpsc::RecvError)) else { return };
                if result_tx.send((id, decode(&path))).is_err() {
                    return;
                }
            });
        }
        Self { tx, rx, pending: Arc::default() }
    }

    /// Queues `path`'s decode for node `id`. A node deleted meanwhile is the caller's to ignore (`tick`'s results
    /// name their node).
    pub fn request(&self, id: String, path: String) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send((id, path));
    }

    /// Every decode finished since the last call.
    pub fn tick(&self) -> Vec<Finished> {
        let done: Vec<Finished> = self.rx.try_iter().collect();
        self.pending.fetch_sub(done.len(), Ordering::SeqCst);
        done
    }

    /// How many requests have not been returned by `tick` yet.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }
}

/// Reads and decodes `path`, by content rather than extension (a lying extension must not defeat the sniff
/// `insert_content` already did), scaled to fit a texture.
fn decode(path: &str) -> Result<Decoded, String> {
    let failed = |err: &dyn std::fmt::Display| {
        let name = crate::json_file::file_name(std::path::Path::new(path));
        eprintln!("image: couldn't open {path}: {err}");
        format!("Couldn't open image {name}: {err}")
    };
    let bytes = std::fs::read(path).map_err(|err| failed(&err))?;
    let img = image::load_from_memory(&bytes).map_err(|err| failed(&err))?;
    // `into_rgba8` moves when the image is already RGBA8.
    let rgba = crate::rgba::fit_image(img.into_rgba8(), crate::rgba::MAX_TEXTURE_SIDE);
    let (width, height) = rgba.dimensions();
    Ok(Decoded { width, height, rgba: rgba.into_raw() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(decoder: &ImageDecoder) -> Vec<Finished> {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut done = Vec::new();
        while decoder.pending() > 0 && std::time::Instant::now() < end {
            done.extend(decoder.tick());
            std::thread::yield_now();
        }
        done
    }

    #[test]
    fn a_decode_comes_back_named_and_a_bad_file_comes_back_as_an_error() {
        let dir = crate::store::testutil::temp_dir("image-decode");
        let good = dir.join("p.png");
        image::RgbaImage::new(3, 2).save(&good).unwrap();
        let bad = dir.join("bad.png");
        std::fs::write(&bad, b"not a picture").unwrap();
        let decoder = ImageDecoder::new();
        decoder.request("good".into(), good.to_string_lossy().into_owned());
        decoder.request("bad".into(), bad.to_string_lossy().into_owned());
        decoder.request("gone".into(), dir.join("missing.png").to_string_lossy().into_owned());
        let mut done = wait(&decoder);
        done.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(decoder.pending(), 0);
        let names: Vec<_> = done.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(names, ["bad", "gone", "good"]);
        let good = done[2].1.as_ref().unwrap();
        assert_eq!((good.width, good.height, good.rgba.len()), (3, 2, 24));
        assert!(done[0].1.as_ref().is_err_and(|e| e.starts_with("Couldn't open image bad.png: ")));
        assert!(done[1].1.is_err());
    }
}
