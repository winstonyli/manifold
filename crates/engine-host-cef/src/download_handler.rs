use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};

/// The completed download's final path, once `DownloadItem::is_complete()`
/// -- see `ipc_protocol::EngineToHost::DownloadCompleted`'s doc comment.
pub type SharedDownloadPath = Rc<RefCell<Option<String>>>;

pub fn new_shared_download_path() -> SharedDownloadPath {
    Rc::new(RefCell::new(None))
}

/// `%USERPROFILE%\Downloads` -- CEF (unlike WebView2) has no built-in
/// default save location of its own; `on_before_download` must supply one
/// or the download never starts. Falls back to the current directory in
/// the (practically never, on a real Windows install) case `USERPROFILE`
/// isn't set, rather than failing the download outright.
fn downloads_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .unwrap_or_default()
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
                if item.is_complete() != 0 {
                    let path = CefString::from(&item.full_path()).to_string();
                    *self.handler.result.borrow_mut() = Some(path);
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
