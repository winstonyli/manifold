//! Background decoding of an image node's file (`ImagePayload`): a card is placed at once and its pixels arrive
//! later, as `PdfRenderer` does for a PDF page. A `worker::Pool` runs at most two decodes at once, so a few big photos
//! decode in parallel and leave most cores free; the app takes finished images once a frame (`tick`) and uploads
//! them itself.

use crate::profile::file::file_name;
use crate::store::model::NodeId;
use crate::worker::Pool;
use std::path::Path;

/// Decodes running at once.
const MAX_DECODES: usize = 2;

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct ImageDecoder {
    /// Keyed by node; the job is the file's path. A panic inside the decoder (a bug on a hostile file) comes back
    /// as `None`, for that one image.
    pool: Pool<NodeId, String, Result<Decoded, String>>,
}

impl ImageDecoder {
    /// `decode` fails with a bare reason; `Workers::new` passes `decode`, tests a stub.
    pub(crate) fn new(decode: impl Fn(&str) -> Result<Decoded, String> + Send + Sync + 'static) -> Self {
        Self { pool: Pool::new(MAX_DECODES, move |path: &String| decode(path)) }
    }

    /// Queues `path`'s decode for node `id`, replacing any earlier one for it: the newest request decides the image.
    /// A node deleted meanwhile is the caller's to cancel (`Workers::cancel_node`).
    pub fn request(&self, id: NodeId, path: String) {
        self.pool.cancel(&id);
        self.pool.request(id, path);
    }

    /// Every decode finished since the last call. An error reads as a toast ("Couldn't open image name: why").
    pub fn tick(&self) -> Vec<(NodeId, Result<Decoded, String>)> {
        self.pool
            .tick()
            .into_iter()
            .map(|(id, path, result)| {
                let result = result.unwrap_or_else(|| {
                    eprintln!("image: decoder panicked on {path}");
                    Err("the decoder crashed".into())
                });
                (id, result.map_err(|why| format!("Couldn't open image {}: {why}", file_name(Path::new(&path)))))
            })
            .collect()
    }

    /// How many requests have not been returned by `tick` yet.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.pool.pending()
    }

    /// See `Pool::wait_for_threads`.
    #[cfg(test)]
    pub fn wait_for_threads(&self, n: usize) {
        self.pool.wait_for_threads(n);
    }
}

/// Reads and decodes `path`, by content rather than extension (a lying extension must not defeat the sniff
/// `insert_content` already did), scaled to fit a texture. Fails with the bare reason: `tick` names the file.
pub(crate) fn decode(path: &str) -> Result<Decoded, String> {
    let failed = |err: &dyn std::fmt::Display| {
        eprintln!("image: couldn't open {path}: {err}");
        err.to_string()
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
    use crate::card_id::testutil::node;
    use std::sync::{mpsc, Arc, Mutex};

    fn wait(decoder: &ImageDecoder) -> Vec<(NodeId, Result<Decoded, String>)> {
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
        let decoder = ImageDecoder::new(decode);
        decoder.request(node(3), good.to_string_lossy().into_owned());
        decoder.request(node(1), bad.to_string_lossy().into_owned());
        decoder.request(node(2), dir.join("missing.png").to_string_lossy().into_owned());
        let mut done = wait(&decoder);
        done.sort_by_key(|a| a.0);
        assert_eq!(decoder.pending(), 0);
        let names: Vec<_> = done.iter().map(|(id, _)| *id).collect();
        assert_eq!(names, [node(1), node(2), node(3)]);
        let good = done[2].1.as_ref().unwrap();
        assert_eq!((good.width, good.height, good.rgba.len()), (3, 2, 24));
        let why = image::load_from_memory(b"not a picture").err().unwrap();
        assert_eq!(done[0].1.as_ref().err(), Some(&format!("Couldn't open image bad.png: {why}")), "named once");
        assert!(done[1].1.is_err());
    }

    /// A reload re-requests a node while its old decode still runs: the old one finishing last must not win.
    #[test]
    fn the_newest_request_for_a_node_decides_its_image() {
        let (started_tx, started) = mpsc::channel();
        let (release_tx, release) = mpsc::channel::<()>();
        let (started_tx, release) = (Mutex::new(started_tx), Arc::new(Mutex::new(release)));
        let decoder = ImageDecoder::new(move |path: &str| {
            if path == "old" {
                started_tx.lock().unwrap().send(()).unwrap();
                let _ = release.lock().unwrap().recv();
            }
            Ok(Decoded { width: if path == "old" { 1 } else { 2 }, height: 1, rgba: Vec::new() })
        });
        decoder.request(node(1), "old".into());
        started.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        decoder.request(node(1), "new".into());
        release_tx.send(()).unwrap();
        decoder.wait_for_threads(0);
        let widths: Vec<u32> = decoder.tick().into_iter().map(|(_, r)| r.unwrap().width).collect();
        assert_eq!(widths, [2], "the newest request's image, alone");
    }

    /// The first decode finished and waits for a `tick` when the second request comes: it must not reach the card.
    #[test]
    fn a_repeat_request_replaces_a_finished_decode() {
        let decoder = ImageDecoder::new(|path: &str| {
            Ok(Decoded { width: if path == "old" { 1 } else { 2 }, height: 1, rgba: Vec::new() })
        });
        decoder.request(node(1), "old".into());
        decoder.wait_for_threads(0);
        decoder.request(node(1), "new".into());
        decoder.wait_for_threads(0);
        let widths: Vec<u32> = decoder.tick().into_iter().map(|(_, r)| r.unwrap().width).collect();
        assert_eq!(widths, [2]);
    }

    #[test]
    fn a_panicking_decode_is_an_error_for_that_image_only() {
        let decoder = ImageDecoder::new(|path: &str| {
            assert!(path != "dir/p.png", "stub decode panics (expected in this test)");
            Ok(Decoded { width: 1, height: 1, rgba: vec![0; 4] })
        });
        decoder.request(node(1), "dir/p.png".into());
        decoder.request(node(2), "dir/q.png".into());
        let mut done = wait(&decoder);
        done.sort_by_key(|a| a.0);
        assert_eq!(
            done[0].1.as_ref().err().map(String::as_str),
            Some("Couldn't open image p.png: the decoder crashed")
        );
        assert!(done[1].1.is_ok());
    }
}
