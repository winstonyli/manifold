//! The Settings UI's install state (`App::view_ui`) and the `App` methods that start, cancel and report installs.

use crate::process_manager::ViewKind;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use super::*;
use super::{remove::*, state::*, unpack::*};

/// The engine runtime installs and their Settings UI (`App::view_ui`).
#[derive(Default)]
pub(crate) struct ViewInstallUi {
    /// Engine runtime installs, at most one per engine (see `InstallJob`).
    pub(crate) view_installs: HashMap<ViewKind, InstallJob>,
    /// The last install's failure message per engine, until the next attempt starts.
    pub(crate) view_failures: HashMap<ViewKind, String>,
    /// `view_status` per engine for the current frame: it stats the disk, and cards, pickers, menus
    /// and Settings all ask. Cleared at the start of each frame and whenever an install changes.
    pub(crate) view_status_cache: std::cell::RefCell<HashMap<ViewKind, ViewStatus>>,
    /// The bring-your-own CEF directory's last validation, with the path it was for (`custom_cef_dir`).
    pub(crate) cef_dir_check: std::cell::RefCell<Option<DirCheck>>,
    /// Settings' text field for that directory while it is being edited; committed on Enter or focus loss.
    pub(crate) cef_dir_draft: Option<String>,
}

/// A running (or just cancelled) install of one engine's components, on its own thread.
pub(crate) struct InstallJob {
    /// Bytes done across the plan's parts so far, written by the thread.
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    /// The plan's total size (host files plus runtime), for `(received, total)`.
    total: u64,
    /// Set by Cancel: the UI no longer shows the job, but the thread may linger briefly (until it next
    /// checks the flag), and its result is discarded. The job stays in the map until the thread has
    /// finished, so a new install never races its `.part`.
    cancelled: bool,
    handle: JoinHandle<Result<PathBuf, InstallError>>,
}

impl crate::App {
    /// How `kind` is available now: the disk, then this app's running install (a cancelled one doesn't
    /// count) and its last failure.
    ///
    /// Cached for the frame (`view_status_cache`); the disk answers under it are reused for a few seconds (`view_on_disk`).
    pub(crate) fn view_status(&self, kind: ViewKind) -> ViewStatus {
        if let Some(state) = self.view_ui.view_status_cache.borrow().get(&kind) {
            return state.clone();
        }
        let state = self.view_status_at(kind, crate::process_manager::view_on_disk(kind), &plan_for(kind));
        self.view_ui.view_status_cache.borrow_mut().insert(kind, state.clone());
        state
    }

    /// `view_status` with the engine's on-disk state (`view_on_disk`) and install plan (`plan_for`) given (tests
    /// use a temporary directory and a plan of their own).
    pub(crate) fn view_status_at(&self, kind: ViewKind, on_disk: OnDisk, plan: &Result<Plan, String>) -> ViewStatus {
        let installing = self
            .view_ui
            .view_installs
            .get(&kind)
            .filter(|job| !job.cancelled)
            .map(|job| (job.progress.load(Ordering::Relaxed), job.total));
        let failed = self.view_ui.view_failures.get(&kind).map(String::as_str);
        let custom = has_runtime(kind).then(|| self.custom_cef_dir()).flatten();
        crate::process_manager::view_availability(kind, plan, on_disk, installing, failed, custom.as_deref())
    }

    /// The bytes installing `kind` would fetch now, for the confirm dialog and the failed card's hint: what is not
    /// on disk yet (a bring-your-own CEF runtime is never fetched).
    pub(crate) fn view_download_size(&self, kind: ViewKind) -> u64 {
        plan_for(kind).map_or(0, |plan| {
            plan.fetch_size(crate::process_manager::view_on_disk(kind), self.custom_cef_dir().is_some())
        })
    }

    /// The configured bring-your-own CEF directory, only when it validates against the pin.
    pub(crate) fn custom_cef_dir(&self) -> Option<PathBuf> {
        self.cef_dir_verdict().and_then(|(dir, verdict)| verdict.ok().map(|()| dir))
    }

    /// The configured directory and whether it validates; `None` when none is set. The verdict is cached
    /// with the path (`cef_dir_check`): the disk is read when the path changes or every `CEF_DIR_CHECK_TTL`,
    /// not every frame, so a folder deleted or replaced behind the app's back is noticed.
    pub(crate) fn cef_dir_verdict(&self) -> Option<(PathBuf, Result<(), String>)> {
        let configured = self.settings.value.cef_directory.as_deref().filter(|s| !s.trim().is_empty())?;
        let mut cache = self.view_ui.cef_dir_check.borrow_mut();
        if cache.as_ref().is_none_or(|(path, at, _)| path != configured || at.elapsed() > CEF_DIR_CHECK_TTL) {
            let verdict = match crate::cef_pin::pin_for_platform() {
                Some(pin) => validate_custom(Path::new(configured), pin),
                None => Err(NO_PIN_REASON.to_string()),
            };
            *cache = Some((configured.to_string(), std::time::Instant::now(), verdict));
        }
        let verdict = cache.as_ref().map(|(_, _, v)| v.clone())?;
        Some((PathBuf::from(configured), verdict))
    }

    /// Forget the cached verdict (and the states built on it, and the Settings page's disk reads) so the next
    /// read looks at the disk again.
    pub(crate) fn invalidate_cef_dir_check(&self) {
        *self.view_ui.cef_dir_check.borrow_mut() = None;
        self.view_ui.view_status_cache.borrow_mut().clear();
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
    }

    /// Set (or clear, for an empty path) the bring-your-own CEF directory: save it, drop the cached
    /// verdict and states so the next read validates it.
    pub(crate) fn set_cef_directory(&mut self, path: &str) {
        let path = crate::settings::typed_folder(path);
        self.settings.value.cef_directory = (!path.is_empty()).then(|| path.to_string());
        self.invalidate_cef_dir_check();
        self.save_settings();
    }

    /// Download and install `kind`'s components into the data directory. See `start_view_install_at`.
    pub(crate) fn start_view_install(&mut self, kind: ViewKind) -> bool {
        let Some(data_dir) = crate::process_manager::view_hosts_dir() else {
            self.queue_toast("No data directory to install into".into());
            return false;
        };
        let mut plan = match plan_for(kind) {
            Ok(plan) => plan,
            Err(reason) => {
                self.queue_toast(crate::view_info::PickOutcome::unavailable_toast(kind, &reason));
                return false;
            }
        };
        if self.custom_cef_dir().is_some() {
            plan.runtime = None;
        }
        self.start_view_install_at(kind, &data_dir, &plan)
    }

    /// Start installing `plan` for `kind` (only a downloaded engine installs) under `data_dir`. Refused, with a
    /// toast saying why, while that engine's install is running or its cancelled thread has not yet stopped (it
    /// would share the `.part`), or while an engine process of that kind is alive (it holds the files, so the
    /// replace would fail). Returns whether a job started.
    pub(crate) fn start_view_install_at(&mut self, kind: ViewKind, data_dir: &Path, plan: &Plan) -> bool {
        if !downloads(kind) {
            return false;
        }
        let label = crate::view_info::view_label(kind);
        if let Some(job) = self.view_ui.view_installs.get(&kind) {
            if !job.cancelled {
                self.queue_toast(format!("{label} is already installing"));
            } else if !job.handle.is_finished() {
                self.queue_toast(format!("The cancelled {label} download is still stopping; try again in a moment"));
            } else {
                // Finished: collect it first (its result is discarded), then start afresh.
                self.poll_view_installs();
                return self.start_view_install_at(kind, data_dir, plan);
            }
            return false;
        }
        if self.nodes.values().any(|n| n.as_tab().is_some_and(|t| t.tab.kind == kind)) {
            self.queue_toast(format!("Close {label} tabs before installing it"));
            return false;
        }
        if !self.spawns.spawning.is_empty() {
            // A tab still starting may be a CEF host about to load the runtime's DLLs.
            self.queue_toast(format!("A tab is still starting; try installing {label} again in a moment"));
            return false;
        }
        self.view_ui.view_failures.remove(&kind);
        self.view_ui.view_status_cache.borrow_mut().clear();
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (thread_progress, thread_cancel) = (progress.clone(), cancel.clone());
        let (data_dir, plan) = (data_dir.to_path_buf(), plan.clone());
        let total = plan.size();
        let handle = std::thread::spawn(move || {
            install_plan(kind, &plan, &data_dir, &mut |n| thread_progress.store(n, Ordering::Relaxed), &thread_cancel)
        });
        self.view_ui.view_installs.insert(kind, InstallJob { progress, cancel, total, cancelled: false, handle });
        self.egui_ctx.request_repaint();
        true
    }

    /// Delete what `kind` downloaded (Settings' Remove, after its confirm), toasting the outcome.
    pub(crate) fn remove_view(&mut self, kind: ViewKind) {
        let label = crate::view_info::view_label(kind);
        let Some(dir) = crate::process_manager::view_hosts_dir() else { return };
        // A spawn still in flight may be this engine's host (it isn't a node yet), so it counts too.
        let running = !self.spawns.spawning.is_empty()
            || self.nodes.values().any(|n| n.as_tab().is_some_and(|t| t.tab.kind == kind));
        let installing = self.view_ui.view_installs.contains_key(&kind);
        let removed = remove_view_files(&dir, kind, host_components(kind), running, installing);
        // Even a failed removal may have deleted some of it: the Settings page reads the disk again.
        crate::process_manager::forget_on_disk();
        crate::settings_page::forget_disk_cache(&self.egui_ctx);
        match removed {
            Ok(()) => {
                // A recorded failure described the files just deleted.
                self.view_ui.view_failures.remove(&kind);
                self.view_ui.view_status_cache.borrow_mut().clear();
                self.queue_toast(format!("{label} removed"));
            }
            Err(why) => self.queue_toast(format!("Couldn't remove {label}: {why}")),
        }
    }

    /// Stop showing `kind`'s install at once and tell its thread to stop: it notices within about 100 ms
    /// while downloading (even on a stalled read), and between unpack entries and before the swap after
    /// that. The `.part` stays for resuming and no failure is recorded.
    pub(crate) fn cancel_view_install(&mut self, kind: ViewKind) {
        if let Some(job) = self.view_ui.view_installs.get_mut(&kind) {
            job.cancel.store(true, Ordering::Relaxed);
            job.cancelled = true;
        }
        self.view_ui.view_status_cache.borrow_mut().clear();
        self.egui_ctx.request_repaint();
    }

    /// Collect finished jobs (once per frame): toast and record a failure, toast a success, drop a
    /// cancelled job's result. Keeps repainting while any job is running.
    pub(crate) fn poll_view_installs(&mut self) {
        if self.view_ui.view_installs.is_empty() {
            return;
        }
        let finished: Vec<ViewKind> = self
            .view_ui
            .view_installs
            .iter()
            .filter(|(_, job)| job.handle.is_finished())
            .map(|(kind, _)| *kind)
            .collect();
        if !finished.is_empty() {
            self.view_ui.view_status_cache.borrow_mut().clear();
            // An install (or a cancelled one's leftovers) changed what the Settings page shows from disk.
            crate::settings_page::forget_disk_cache(&self.egui_ctx);
            crate::process_manager::forget_on_disk();
        }
        for kind in finished {
            let Some(job) = self.view_ui.view_installs.remove(&kind) else { continue };
            let result =
                job.handle.join().unwrap_or_else(|_| Err(InstallError::Io("the install thread panicked".into())));
            let label = crate::view_info::view_label(kind);
            match result {
                _ if job.cancelled => {}
                Err(InstallError::Cancelled) => {}
                Ok(_) => {
                    self.view_ui.view_failures.remove(&kind);
                    self.queue_toast(format!("{label} installed"));
                }
                Err(e) => {
                    self.queue_toast(format!("{label} install failed: {e}"));
                    self.view_ui.view_failures.insert(kind, e.to_string());
                }
            }
            self.egui_ctx.request_repaint();
        }
        if !self.view_ui.view_installs.is_empty() {
            self.egui_ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
