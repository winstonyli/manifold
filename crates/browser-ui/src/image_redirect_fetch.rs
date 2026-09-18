//! Downloads the bytes behind a tab that turned out to be a raw image
//! response (`ipc_protocol::EngineToHost::ImagePage`) so it can be turned
//! into a real `Image` graph node -- same "browser-ui does its own
//! independent HTTP fetch, engine-agnostic" posture `favicon_fetch` already
//! established, and the same "spawn a thread, `mpsc` channel, main loop
//! `tick()`s it" shape every other one-shot background fetch in this project
//! uses (`favicon_fetch`, `pdf_fetch`).
//!
//! Deliberately reuses the *existing*, synchronous, file-path-based
//! `App::insert_image_node_with_id` rather than teaching `ImagePayload`
//! a second, network-sourced way to exist: the downloaded bytes are saved
//! to a real temp file first, and from that point on a redirected image is
//! indistinguishable from one the user picked via "+ Image" -- same GPU
//! upload path, same session persistence (a moved/deleted temp file is
//! silently skipped at restore, same as any other missing local image
//! file), same context menu.

use std::sync::mpsc;
use std::time::Duration;

/// `url` is carried all the way back through so the caller can confirm the
/// tab is still showing the same url that was fetched before converting it
/// -- the user may have closed or re-navigated the tab while a slow fetch
/// was still in flight.
pub struct ImageRedirectResult {
    pub tab_id: String,
    pub url: String,
    pub path: Option<String>,
}

pub struct ImageRedirectFetcher {
    tx: mpsc::Sender<ImageRedirectResult>,
    rx: mpsc::Receiver<ImageRedirectResult>,
}

impl ImageRedirectFetcher {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }

    /// Fetches `url` (the tab's own current url at the moment its page was
    /// detected as a raw image) on a background thread.
    pub fn request(&self, tab_id: String, url: String) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let path = fetch_to_temp_file(&tab_id, &url);
            let _ = tx.send(ImageRedirectResult { tab_id, url, path });
        });
    }

    /// Call once per frame. `path: None` means the fetch failed (a network
    /// error, or bytes that don't actually decode as an image despite the
    /// engine's own `document.contentType` check) -- the tab is just left
    /// as an ordinary tab in that case, still showing whichever native
    /// image-viewer page the engine itself rendered for it.
    pub fn tick(&self) -> Vec<ImageRedirectResult> {
        self.rx.try_iter().collect()
    }
}

/// `GET url`, saved to `%TEMP%\multiengine-browser-images\<tab_id>.img` once
/// it's confirmed to actually decode as an image (`image::load_from_memory`,
/// same decode `insert_image_node_with_id` will redo from the file a moment
/// later -- a real double-decode, but it's the price of reusing that
/// function completely unchanged rather than giving `ImagePayload` a second
/// source variant just for this). The extension is arbitrary -- `image::open`
/// sniffs real file content rather than trusting it -- so there's no need to
/// parse one out of `url`'s own path or a response header.
fn fetch_to_temp_file(tab_id: &str, url: &str) -> Option<String> {
    let response = ureq::get(url)
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .call()
        .ok()?;
    let bytes = response.into_body().with_config().limit(100_000_000).read_to_vec().ok()?;
    image::load_from_memory(&bytes).ok()?;

    let dir = std::env::temp_dir().join("multiengine-browser-images");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{tab_id}.img"));
    std::fs::write(&path, &bytes).ok()?;
    Some(path.to_string_lossy().into_owned())
}
