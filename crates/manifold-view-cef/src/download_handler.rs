use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};
use manifold_view_protocol::ViewToHost;

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

        // CEF has no default save location of its own and must be told one (`callback.cont`), or the download never
        // starts. The path comes from the rule every host uses (`downloads::destination`): the folder the app set,
        // made if missing, and the suggested name's last component, numbered if a file or a download still in
        // `assigned` has it. `show_dialog: false`: there is no window for a save-as dialog. A download that can't be
        // placed is reported started and failed (`downloads::refused`) and handed back to CEF's default handling
        // (return 0), which cancels it for a windowless browser (Alloy style); `on_download_updated` reports nothing
        // of it, since its id was never assigned.
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(callback)) = (download_item, callback) else { return true as _ };
            let suggested = suggested_name.map(|s| s.to_string()).unwrap_or_default();
            let mut results = self.handler.result.borrow_mut();
            let decided = manifold_view_protocol::downloads::destination(
                manifold_view_protocol::downloads::folder().as_deref(),
                &suggested,
                |p| {
                    p.exists()
                        || results
                            .assigned
                            .values()
                            .any(|taken| manifold_view_protocol::downloads::same_download_path(taken, p))
                },
            );
            match decided {
                Ok(path) => {
                    results.assigned.insert(item.id(), path.clone());
                    drop(results);
                    callback.cont(Some(&path.to_string_lossy().as_ref().into()), false as _);
                    true as _
                }
                Err(path) => {
                    let url = Some(CefString::from(&item.url()).to_string()).filter(|u| !u.is_empty());
                    let messages = manifold_view_protocol::downloads::refused(path.to_string_lossy().into_owned(), url);
                    results.queue.extend(messages);
                    0
                }
            }
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
