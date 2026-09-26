//! Downloads the bytes behind a tab whose page turned out to be a raw,
//! non-HTML response worth turning into a real graph node instead of
//! leaving it sitting there as an ordinary tab -- same "manifold does its
//! own independent HTTP fetch, engine-agnostic" posture `favicon_fetch`
//! already established, and the same "spawn a thread, `mpsc` channel, main
//! loop `tick()`s it" shape every other one-shot background fetch in this
//! project uses (`favicon_fetch`, `pdf_fetch`).
//!
//! Generalized from the original image-only version: `ipc_protocol::
//! EngineToHost::RedirectableContent` now carries the real `document.
//! contentType` string rather than a bare "yes, redirect this" signal, so
//! adding a new auto-redirectable kind only ever means widening `classify`
//! below (plus its matching branch in `validate`/`App::redirect_tab_to_*`)
//! -- not touching any engine-host's own detection script/eval. See
//! `RedirectKind`'s own doc comment for which kinds are actually auto-
//! redirected today, and why JSON deliberately isn't one of them despite
//! being detected through the same per-engine mechanism.
//!
//! Deliberately reuses each kind's *existing*, synchronous, file-path-based
//! insert function (`App::insert_image_node_with_id`, etc.) rather than
//! teaching that kind's payload a second, network-sourced way to exist: the
//! downloaded bytes are saved to a real temp file first, and from that
//! point on a redirected node is indistinguishable from one the user picked
//! by hand -- same GPU upload path (where relevant), same session
//! persistence (a moved/deleted temp file is silently skipped at restore,
//! same as any other missing local file), same context menu.

use std::sync::mpsc;
use std::time::Duration;

/// The kinds of raw response this project will silently swap a tab out for
/// a real graph node over. Deliberately small and reviewed, not "whatever
/// `classify` can be made to match" -- each variant is something a tab
/// genuinely has *nothing left to offer* once redirected (unlike JSON,
/// which stays a tab with an opt-in "Save as Node" action instead -- see
/// `App::build_json_viewer`'s own doc comment for why that one's trigger is
/// manual, not automatic).
#[derive(Clone, Copy)]
pub enum RedirectKind {
    Image,
}

/// The one place a new content type gets mapped to `RedirectKind` --
/// widen this (and `validate` below) to add a kind; no engine-host change
/// needed, since every engine already forwards the raw `document.
/// contentType` string regardless of what it says.
pub fn classify(content_type: &str) -> Option<RedirectKind> {
    if content_type.starts_with("image/") {
        return Some(RedirectKind::Image);
    }
    None
}

/// Confirms the downloaded bytes actually match `kind`, the same
/// "don't trust a server's own header blindly" caution `EngineToHost::
/// JsonBody`'s own fallback has for a lying `Content-Type`.
fn validate(kind: RedirectKind, bytes: &[u8]) -> bool {
    match kind {
        RedirectKind::Image => image::load_from_memory(bytes).is_ok(),
    }
}

/// `url` is carried all the way back through so the caller can confirm the
/// tab is still showing the same url that was fetched before converting it
/// -- the user may have closed or re-navigated the tab while a slow fetch
/// was still in flight.
pub struct RedirectResult {
    pub tab_id: String,
    pub url: String,
    pub kind: RedirectKind,
    pub path: Option<String>,
}

pub struct RedirectFetcher {
    tx: mpsc::Sender<RedirectResult>,
    rx: mpsc::Receiver<RedirectResult>,
}

impl RedirectFetcher {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }

    /// Fetches `url` (the tab's own current url at the moment its page was
    /// classified as `kind`) on a background thread.
    pub fn request(&self, tab_id: String, url: String, kind: RedirectKind) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let path = fetch_to_temp_file(&tab_id, &url, kind);
            let _ = tx.send(RedirectResult { tab_id, url, kind, path });
        });
    }

    /// Call once per frame. `path: None` means the fetch failed (a network
    /// error, or bytes that don't actually validate as `kind` despite the
    /// engine's own `document.contentType` check) -- the tab is just left
    /// as an ordinary tab in that case, still showing whichever native
    /// viewer page the engine itself rendered for it.
    pub fn tick(&self) -> Vec<RedirectResult> {
        self.rx.try_iter().collect()
    }
}

/// `GET url`, saved to `%TEMP%\manifold-redirects\<tab_id>.bin`
/// once `validate` confirms it actually matches `kind` (the same decode
/// most kinds' own insert function will redo from the file a moment later
/// -- a real double-decode for those, but it's the price of reusing that
/// function completely unchanged rather than giving every payload a
/// second, network-sourced way to exist). The extension is arbitrary --
/// every kind's own insert function sniffs real file content rather than
/// trusting it -- so there's no need to parse one out of `url`'s own path
/// or a response header.
fn fetch_to_temp_file(tab_id: &str, url: &str, kind: RedirectKind) -> Option<String> {
    let response = ureq::get(url).config().timeout_global(Some(Duration::from_secs(15))).build().call().ok()?;
    let bytes = response.into_body().with_config().limit(100_000_000).read_to_vec().ok()?;
    if !validate(kind, &bytes) {
        return None;
    }

    let dir = std::env::temp_dir().join("manifold-redirects");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{tab_id}.bin"));
    std::fs::write(&path, &bytes).ok()?;
    Some(path.to_string_lossy().into_owned())
}
