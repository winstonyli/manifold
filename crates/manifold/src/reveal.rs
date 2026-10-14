//! Reveal-in-Explorer and favicon-origin methods of `App`.

use super::*;

impl App {
    /// Opens `path`'s containing folder with the file itself pre-selected --
    /// the mechanism behind `reveal_node`.
    #[cfg(windows)]
    fn reveal_in_explorer(path: &str) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        if path.is_empty() {
            return Err("This download has no file path".to_string());
        }
        // `raw_arg`, not `arg`: `arg` would escape the quotes `explorer_select_arg` adds.
        std::process::Command::new("explorer.exe")
            .raw_arg(explorer_select_arg(path))
            .spawn()
            .map(drop)
            .map_err(|e| format!("Couldn't open Explorer: {e}"))
    }
    /// Elsewhere there is no portable "select this file", so the containing folder is opened with the desktop's
    /// own handler (`xdg-open` on Linux, `open` on macOS).
    #[cfg(not(windows))]
    fn reveal_in_explorer(path: &str) -> Result<(), String> {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let folder = std::path::Path::new(path).parent().ok_or_else(|| format!("{path} has no containing folder"))?;
        std::process::Command::new(opener)
            .arg(folder)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't open {}: {opener}: {e}", folder.display()))
    }

    /// The real file behind a Download/Image/Pdf/Json node, in the form
    /// Explorer and a pasted clipboard path understand -- what both
    /// "Open Containing Folder" (`reveal_node`) and "Copy Path" use. A
    /// download's path may be a WSL-side POSIX path (WebKit downloads), so
    /// it goes through `windows_path_for_download`; an image/PDF/JSON path
    /// always came from a real Windows file picker, so it's used as-is.
    /// `None` for kinds with no backing file (Tab, Group, Note, Jar), and for an
    /// empty path: "no file" (a Download from a `Trust::Foreign` import has
    /// its machine-local path dropped).
    pub(crate) fn node_file_path(&self, id: &str) -> Option<String> {
        let path = match &self.nodes.get(id)?.runtime {
            Runtime::Download(_) => self.windows_path_for_download(id),
            Runtime::Image(image) => Some(image.path.clone()),
            Runtime::Pdf(pdf) => Some(pdf.path.clone()),
            Runtime::Json(json) => Some(json.path.clone()),
            Runtime::Tab(_) | Runtime::Unloaded(_) | Runtime::Note(_) | Runtime::Jar | Runtime::None => None,
        };
        path.filter(|p| !p.is_empty())
    }

    /// Opens a node's file's containing folder with the file pre-selected
    /// (a plain click on a download card does this too -- same "just show
    /// the obvious thing, no custom UI" posture as the download itself, see
    /// `ViewToHost::DownloadCompleted`'s doc comment). No-op for kinds
    /// with no file; a file node with no path (`node_file_path`) is toasted.
    pub(crate) fn reveal_node(&mut self, id: &str) {
        let Some(node) = self.nodes.get(id) else { return };
        let has_file =
            matches!(node.runtime, Runtime::Download(_) | Runtime::Image(_) | Runtime::Pdf(_) | Runtime::Json(_));
        match self.node_file_path(id) {
            Some(path) => {
                if let Err(e) = Self::reveal_in_explorer(&path) {
                    self.queue_toast(e);
                }
            }
            None if has_file => self.queue_toast("This file's path isn't known on this machine".to_string()),
            None => {}
        }
    }

    /// Memoized `favicon_fetch::url_origin(url)` -- that function is a
    /// couple of string scans plus a `format!` allocation, and both of
    /// this app's per-frame callers (the Graph-view tab card, the Tab-
    /// view toolbar's own identity chip) called it fresh every single
    /// frame for a URL that's stable between navigations. **Found on a
    /// general (not just large-graph) performance audit**, the same
    /// category as `truncate_cache`: capped and cleared outright past
    /// `TRUNCATE_CACHE_CAP` rather than evicted incrementally, for the
    /// same "should stay small in steady state, insurance against
    /// unbounded growth" reasoning.
    pub(crate) fn cached_favicon_origin(&mut self, url: &str) -> Option<String> {
        if let Some(cached) = self.favicon_origin_cache.get(url) {
            return cached.clone();
        }
        let origin = favicon_fetch::url_origin(url);
        if self.favicon_origin_cache.len() >= TRUNCATE_CACHE_CAP {
            self.favicon_origin_cache.clear();
        }
        self.favicon_origin_cache.insert(url.to_owned(), origin.clone());
        origin
    }
}
