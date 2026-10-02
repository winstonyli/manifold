use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};

/// One thing that happened to a download since the last poll -- mirrors
/// `ipc_protocol::EngineToHost`'s own `Download*` variants; kept as its own
/// type here (rather than depending on `ipc_protocol` from this leaf
/// module) so `main.rs` does the actual translation, same separation
/// `manifold-engine-webview2`'s own `download::DownloadEvent` uses.
pub enum DownloadEvent {
    Started { path: String, total_bytes: Option<u64>, url: Option<String> },
    Progress { path: String, bytes_received: u64 },
    Completed { path: String },
    Failed { path: String },
}

/// Download events, queued rather than a single slot -- two downloads
/// progressing within the same tick would otherwise silently clobber each
/// other. `on_download_updated` fires repeatedly for the same item over its
/// whole lifetime (not just once at completion) -- `started` dedupes it
/// down to exactly one `Started` per item id, `reported` the same for
/// `Completed` or `Failed`; every other call in between is a `Progress`.
pub struct DownloadResults {
    pub queue: Vec<DownloadEvent>,
    started: HashSet<u32>,
    reported: HashSet<u32>,
}

pub type SharedDownloadPath = Rc<RefCell<DownloadResults>>;

pub fn new_shared_download_path() -> SharedDownloadPath {
    Rc::new(RefCell::new(DownloadResults { queue: Vec::new(), started: HashSet::new(), reported: HashSet::new() }))
}

/// `%USERPROFILE%\Downloads` -- CEF (unlike WebView2) has no built-in
/// default save location of its own; `on_before_download` must supply one
/// or the download never starts. Falls back to the current directory in
/// the (practically never, on a real Windows install) case `USERPROFILE`
/// isn't set, rather than failing the download outright.
fn downloads_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Downloads")).unwrap_or_default()
}

#[derive(Clone)]
pub struct EngineDownloadHandler {
    result: SharedDownloadPath,
}

impl EngineDownloadHandler {
    pub fn new(result: SharedDownloadPath) -> Self {
        Self { result }
    }
}

wrap_download_handler! {
    pub struct DownloadHandlerBuilder {
        handler: EngineDownloadHandler,
    }

    impl DownloadHandler {
        // Must call `callback.cont(...)` or the download never actually
        // starts -- CEF has no default location of its own the way
        // WebView2 does. `show_dialog: false` since there's no real
        // window for a native save-as dialog to appear in anyway.
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            _download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            if let Some(callback) = callback {
                // Server-controlled -- only its final path component is
                // ever used, so a `..`-laden or absolute suggested name
                // can't escape `downloads_dir()` (`PathBuf::join` doesn't
                // sanitize either case: `..` segments pass through
                // untouched and an absolute joinee replaces the base
                // entirely).
                let name = suggested_name
                    .map(|s| s.to_string())
                    .as_deref()
                    .map(std::path::Path::new)
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("download")
                    .to_string();
                let path = downloads_dir().join(name);
                callback.cont(Some(&path.to_string_lossy().as_ref().into()), false as _);
            }
            true as _
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            if let Some(item) = download_item {
                let mut results = self.handler.result.borrow_mut();
                let id = item.id();
                // Final by contract as soon as the download item exists --
                // `on_before_download`'s callback already supplied it,
                // CEF has no default location of its own to override it
                // with. Same value the `Completed` branch below already
                // trusted before this progress reporting existed.
                let path = CefString::from(&item.full_path()).to_string();
                if results.started.insert(id) {
                    let total = item.total_bytes();
                    let total_bytes = (total > 0).then_some(total as u64);
                    let url = Some(CefString::from(&item.url()).to_string()).filter(|u| !u.is_empty());
                    results.queue.push(DownloadEvent::Started { path: path.clone(), total_bytes, url });
                }
                if item.is_complete() != 0 {
                    if results.reported.insert(id) {
                        results.queue.push(DownloadEvent::Completed { path });
                    }
                } else if item.is_canceled() != 0 || item.is_interrupted() != 0 {
                    if results.reported.insert(id) {
                        results.queue.push(DownloadEvent::Failed { path });
                    }
                } else {
                    let received = item.received_bytes();
                    if received >= 0 {
                        results.queue.push(DownloadEvent::Progress { path, bytes_received: received as u64 });
                    }
                }
            }
        }
    }
}

impl DownloadHandlerBuilder {
    pub fn build(handler: EngineDownloadHandler) -> DownloadHandler {
        Self::new(handler)
    }
}
