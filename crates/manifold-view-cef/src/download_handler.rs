use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};
use ipc_protocol::ViewToHost;

/// Download messages for the host, queued rather than a single slot -- two downloads
/// progressing within the same tick would otherwise silently clobber each
/// other. `on_download_updated` fires repeatedly for the same item over its
/// whole lifetime (not just once at completion) -- `started` dedupes it
/// down to exactly one `Started` per item id, and the item leaving
/// `assigned` makes `Completed` or `Failed` the last message about it;
/// every other call in between is a `Progress`.
pub struct DownloadResults {
    pub queue: Vec<ViewToHost>,
    started: HashSet<u32>,
    /// The path `on_before_download` handed out, by item id, until the
    /// download ends. Every message reports this string, never
    /// `DownloadItem::full_path`, which is empty for the first few updates
    /// (they arrive before `on_before_download`). Its values are also the
    /// reserved names: concurrent same-name downloads get distinct names
    /// before either file exists.
    assigned: HashMap<u32, PathBuf>,
}

pub type SharedDownloadPath = Rc<RefCell<DownloadResults>>;

pub fn new_shared_download_path() -> SharedDownloadPath {
    Rc::new(RefCell::new(DownloadResults { queue: Vec::new(), started: HashSet::new(), assigned: HashMap::new() }))
}

#[derive(Clone)]
pub struct ViewDownloadHandler {
    result: SharedDownloadPath,
}

impl ViewDownloadHandler {
    pub fn new(result: SharedDownloadPath) -> Self {
        Self { result }
    }
}

wrap_download_handler! {
    pub struct DownloadHandlerBuilder {
        handler: ViewDownloadHandler,
    }

    impl DownloadHandler {
        // CEF's C++ default allows every download, but cef-rs's generated
        // default returns `Default::default()` (0, refuse), which cancelled
        // every download before `on_before_download` ran.
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            true as _
        }

        // Must call `callback.cont(...)` or the download never actually
        // starts -- CEF has no default location of its own the way
        // WebView2 does. `show_dialog: false` since there's no real
        // window for a native save-as dialog to appear in anyway.
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            if let (Some(item), Some(callback)) = (download_item, callback) {
                // CEF (unlike WebView2) has no default save location or
                // uniquifying of its own. The suggested name is
                // server-controlled; `safe_name` keeps only its last
                // component (it can't leave the downloads folder) and makes
                // it a name Windows will create as given.
                let dir = ipc_protocol::downloads::downloads_dir();
                let suggested = suggested_name.map(|s| s.to_string()).unwrap_or_default();
                let name = ipc_protocol::downloads::safe_name(&suggested);
                let mut results = self.handler.result.borrow_mut();
                let path = ipc_protocol::downloads::unique_path(&dir, &name, |p| {
                    p.exists()
                        || results.assigned.values().any(|taken| ipc_protocol::downloads::same_download_path(taken, p))
                });
                results.assigned.insert(item.id(), path.clone());
                drop(results);
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
                // Nothing to report before `on_before_download` has named
                // it (CEF sends a few updates first, with an empty
                // `full_path`), or after it ended.
                let Some(path) = results.assigned.get(&id).map(|p| p.to_string_lossy().into_owned()) else {
                    return;
                };
                if results.started.insert(id) {
                    let total = item.total_bytes();
                    let total_bytes = (total > 0).then_some(total as u64);
                    let url = Some(CefString::from(&item.url()).to_string()).filter(|u| !u.is_empty());
                    results.queue.push(ViewToHost::DownloadStarted { path: path.clone(), total_bytes, url });
                }
                let complete = item.is_complete() != 0;
                if complete || item.is_canceled() != 0 || item.is_interrupted() != 0 {
                    results.assigned.remove(&id);
                    results.started.remove(&id);
                    let event = if complete {
                        ViewToHost::DownloadCompleted { path }
                    } else {
                        ViewToHost::DownloadFailed { path }
                    };
                    results.queue.push(event);
                } else {
                    let received = item.received_bytes();
                    if received >= 0 {
                        results.queue.push(ViewToHost::DownloadProgress { path, bytes_received: received as u64 });
                    }
                }
            }
        }
    }
}

impl DownloadHandlerBuilder {
    pub fn build(handler: ViewDownloadHandler) -> DownloadHandler {
        Self::new(handler)
    }
}
