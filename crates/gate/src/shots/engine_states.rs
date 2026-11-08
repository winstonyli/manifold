//! `chrome-shots`' CEF install states (see `shots`): a copy of the exe with no CEF host beside it, walked through Not
//! installed, Failed, Installing, Installed, Update needed and Custom.

use std::fs;
use std::io;
use std::path::Path;

use manifold_hosts::{cef, Os, CEF, CEF_RUNTIME_DIR, HOSTS_DIR, WEBVIEW2};
use serde_json::{json, Value};

use super::{content_dir, Shots, AWAY, DEFAULT_ENGINE};
use crate::instance::{seed_settings, Instance, QUIT_TIMEOUT};
use crate::probes::import_session;
use crate::state::TAB;

/// The CEF host the fixture manifest pins: (file, size, SHA-256). Nothing downloads it; the files on disk are sparse.
fn fake_cef_host() -> (String, u64, &'static str) {
    (CEF.file_name(Os::here()), 740_000, "db3e0751979c3cb3068f732ed4f0ef12ade69d183d98ceee120ad08c8d00f74c")
}
impl Shots {
    /// Restarts the instance on a fresh profile `eng-<name>` (a copy of the exe, the settings page open).
    fn engine_app(&mut self, eng_exe: &Path, name: &str, more: &str) -> io::Result<()> {
        let root = self.work.join(format!("eng-{name}"));
        let profile = root.join("manifold");
        seed_settings(&profile, "webview2", more)?;
        // etcetera's data dir under this APPDATA.
        self.eng_data = profile.join("manifold").join("data").join(HOSTS_DIR);
        fs::create_dir_all(&self.eng_data)?;
        import_session(
            eng_exe,
            &profile,
            &[
                json!({"id": "t1", "type": TAB, "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]}),
                json!({"id": "t3", "type": TAB, "url": "https://example.com/", "view_kind": "cef", "pos": [300.0, 0.0]}),
            ],
        )?;
        let appdata = root.display().to_string();
        let pins_file = self.work.join("host-pins.json").display().to_string();
        self.inst = Instance::launch_env(
            eng_exe,
            &profile,
            &self.work,
            &format!("log-eng-{name}"),
            &[("APPDATA", &appdata), ("MANIFOLD_HOST_PINS_FILE", &pins_file)],
        )?;
        // The proxy outlives the instance (`instance::offline`): each one starts it refusing again.
        self.inst.proxy().expect("launched offline").set_refuse(true);
        send!(self, format!("RESIZE {} 1000", self.size.0), "WAITTABS", "WAITIDLE")
    }

    /// CEF's pinned host files (sparse, at their pinned sizes) and the `cef.host` marker that lists their hashes
    /// (`old`: other hashes, so the host reads Update needed).
    fn fake_host(&self, pins: &[(String, u64, String)], old: bool) -> io::Result<()> {
        let mut marker = String::new();
        for (file, size, sha) in pins {
            fs::File::create(self.eng_data.join(file))?.set_len(*size)?;
            marker.push_str(&format!("{} {file}\n", if old { "0".repeat(64) } else { sha.clone() }));
        }
        fs::write(self.eng_data.join(format!("{}.host", CEF.id)), marker)
    }

    /// A runtime directory with a (sparse) 423 MB libcef.dll and `marker` in `ready`.
    fn fake_runtime(&self, marker: &str) -> io::Result<()> {
        let dir = self.eng_data.join(CEF_RUNTIME_DIR);
        fs::create_dir_all(&dir)?;
        let lib = CEF.runtime_lib(Os::here()).expect("CEF has a runtime");
        fs::File::create(dir.join(lib))?.set_len(423 * 1024 * 1024)?;
        fs::write(dir.join("ready"), marker)
    }

    fn engine_stop(&mut self) -> io::Result<()> {
        self.inst.quit(QUIT_TIMEOUT).map(|_| ())
    }

    pub(super) fn engine_states(&mut self, exe: &Path) -> io::Result<()> {
        let (file, size, sha256) = fake_cef_host();
        let pins = vec![(file.clone(), size, sha256.to_string())];
        // The manifest's keys: the host's id, then the platform it is pinned under (the app's parser reads both).
        let (id, platform) = (CEF.id, Os::here().pin_platform());
        fs::write(
            self.work.join("host-pins.json"),
            json!({id: {platform: [{"file": file, "size": size, "sha256": sha256}]}}).to_string(),
        )?;
        let pin_version = cef::pin(Os::here()).ok_or_else(|| io::Error::other("no CEF pin for this OS"))?.version;
        let eng = self.work.join("eng");
        fs::create_dir_all(&eng)?;
        let eng_exe = eng.join("manifold.exe");
        fs::copy(exe, &eng_exe)?;
        let webview2 = WEBVIEW2.file_name(Os::here());
        fs::copy(exe.with_file_name(&webview2), eng.join(&webview2))?;

        self.engine_app(&eng_exe, "states", "")?;
        // The seeded CEF tab can't start: its card says so (graph view, then its Tab view).
        self.shot("engine-states-graph")?;
        self.toast_shot("engine-states-failed-tab", "is not installed", |s| send!(s, "GRAPHRIGHT", "GRAPHOPEN"))?;
        send!(self, "TOGGLEVIEW", "WAITIDLE", "TOGGLESETTINGSPAGE")?;
        self.shot("engine-states-settings-notinstalled")?;
        self.picker_shot("engine-states-picker-notinstalled")?;
        // The Engines section's Download button asks first, with the size.
        self.inst.click_control("Download")?;
        self.shot("engine-install-confirm")?;
        // Confirm with the proxy refusing: the install fails and the row says why, with Retry. The dialog is in
        // front, so its own Download button is the one found by that name.
        self.toast_shot("engine-states-settings-failed", "install failed", |s| s.inst.click_control("Download"))?;
        self.picker_shot("engine-states-picker-failed")?;
        // Retry with the proxy silent: Installing, with Cancel.
        self.inst.proxy().expect("launched offline").set_refuse(false);
        self.inst.click_control("Retry")?;
        self.settle()?;
        self.inst.click_control("Download")?;
        self.shot("engine-states-settings-installing")?;
        self.picker_shot("engine-states-picker-installing")?;
        self.engine_stop()?;
        // Files on disk, judged by their marker: this pin's is Installed (size, Remove), another's is Update needed.
        // Fresh instances: a failed install outranks the disk in the state, so they can't follow the one above.
        self.engine_app(&eng_exe, "installed", "")?;
        self.fake_host(&pins, false)?;
        self.fake_runtime(pin_version)?;
        send!(self, "TOGGLESETTINGSPAGE")?;
        self.shot("engine-states-settings-installed")?;
        self.engine_stop()?;
        self.engine_app(&eng_exe, "stale", "")?;
        self.fake_host(&pins, true)?;
        self.fake_runtime("old-version")?;
        send!(self, "TOGGLESETTINGSPAGE")?;
        self.shot("engine-states-settings-stale")?;
        self.picker_shot("engine-states-picker-stale")?;
        self.engine_stop()?;
        // Bring-your-own folders: one whose archive.json names this pin (Custom), one that names another build.
        for (name, archive) in [("custom", pin_version), ("custom-bad", "151.0.1+gabcdef+chromium-151.0.0.1")] {
            // Its path shows on the settings page, so it is named by its files, not kept in the work dir.
            let archive = json!({"type": "minimal", "name": format!("cef_binary_{archive}_windows64_minimal.tar.bz2")})
                .to_string();
            let lib = CEF.runtime_lib(Os::here()).expect("CEF has a runtime");
            let dir = content_dir(&std::env::temp_dir(), &[(lib, b""), ("archive.json", archive.as_bytes())])?;
            let more = format!(", \"cef_directory\": {}", Value::String(dir.display().to_string().replace('\\', "/")));
            self.engine_app(&eng_exe, name, &more)?;
            self.fake_host(&pins, false)?; // a bring-your-own runtime still needs the downloaded host
            send!(self, "TOGGLESETTINGSPAGE")?;
            self.shot(&format!("engine-states-settings-{name}"))?;
            self.picker_shot(&format!("engine-states-picker-{name}"))?;
            self.engine_stop()?;
        }
        Ok(())
    }

    /// The Default starting engine picker open, shot, closed.
    fn picker_shot(&mut self, name: &str) -> io::Result<()> {
        self.inst.click_control(DEFAULT_ENGINE)?;
        self.shot(name)?;
        send!(self, AWAY, "WAITIDLE")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_host_is_this_oss_cef_file() {
        let (file, size, sha) = fake_cef_host();
        assert!(
            file == if cfg!(windows) { "manifold-view-cef.exe" } else { "manifold-view-cef" }
                && size > 0
                && sha.len() == 64
        );
    }
}
