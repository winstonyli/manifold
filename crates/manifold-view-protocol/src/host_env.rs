//! What manifold tells a local view-host at startup, through the environment (not argv: CEF parses its own argv as a
//! Chromium command line), and the bounded wait for the cookie-jar seed. The app's settings a host acts on
//! (`HostSettings`) come the same way at spawn, as hex (`ENV_SETTINGS`), and later as `HostToView::SetSettings`; a host
//! applies both with `apply`. `process_manager::spawn_tab` writes it with the `ENV_*` names; every host reads it here,
//! so the contract is stated once.

use std::time::Duration;

use crate::{transport, HostSettings, HostToView};

/// Names the control sockets and shared-memory frames. Normally the tab id, but manifold passes a different one when
/// a stale host from a crashed run still holds the tab id's pipes (`spawn_tab`'s fallback).
pub const ENV_SOCKET_ID: &str = "VIEW_SOCKET_ID";
/// The URL the first navigation opens.
pub const ENV_INITIAL_URL: &str = "VIEW_INITIAL_URL";
/// Set (to anything) for a jar engine: wait for `SetCookies` before the first navigation.
pub const ENV_AWAIT_COOKIES: &str = "VIEW_AWAIT_COOKIES";
/// The app's settings at spawn, as `HostSettings::to_hex`; later changes arrive as `HostToView::SetSettings`. The
/// WebKit host under WSL reads it from its command line, where hex is shell-safe.
pub const ENV_SETTINGS: &str = "VIEW_SETTINGS";

/// How long a host started with `ENV_AWAIT_COOKIES` waits for manifold's `SetCookies` before navigating.
pub const COOKIE_SEED_WAIT: Duration = Duration::from_secs(5);

/// The URL a host opens when manifold passes none.
const DEFAULT_URL: &str = "https://example.com";

/// The shared part of a local host's startup environment. Per-engine variables (CEF's cache dir, WebView2's profile
/// name, WebKit's TCP settings) stay with their hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEnv {
    pub socket_id: String,
    pub initial_url: String,
    pub await_cookies: bool,
    /// The app's settings at spawn (`ENV_SETTINGS`), for the host to `apply`.
    pub settings: HostSettings,
}

/// What came first from manifold at startup: the jar seed a jar engine waits for, and any other first message,
/// which the host's loop must still handle.
#[derive(Debug, Default)]
pub struct StartupMessages {
    /// The `SetCookies` JSON, when it arrived in time.
    pub cookies: Option<String>,
    pub first_message: Option<HostToView>,
}

impl HostEnv {
    /// Panics without `ENV_SOCKET_ID`: a host started by hand cannot work.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok(), std::env::var_os(ENV_AWAIT_COOKIES).is_some())
    }

    fn from_vars(get: impl Fn(&str) -> Option<String>, await_cookies: bool) -> Self {
        Self {
            socket_id: get(ENV_SOCKET_ID).unwrap_or_else(|| panic!("{ENV_SOCKET_ID} env var required")),
            initial_url: get(ENV_INITIAL_URL).unwrap_or_else(|| DEFAULT_URL.to_string()),
            await_cookies,
            settings: settings_from(get(ENV_SETTINGS).as_deref()),
        }
    }

    /// For a jar engine, waits up to `COOKIE_SEED_WAIT` for manifold's first message (`SetCookies`, sent first to
    /// hosts started with `ENV_AWAIT_COOKIES`), calling `pump` between polls so the engine's own loop keeps running.
    /// Any other first message is returned for the host's loop rather than dropped. Nothing is waited for otherwise.
    pub fn await_cookies(&self, rx: &std::sync::mpsc::Receiver<HostToView>, pump: impl FnMut()) -> StartupMessages {
        let mut got = StartupMessages::default();
        if !self.await_cookies {
            return got;
        }
        match transport::wait_for_first_message(rx, COOKIE_SEED_WAIT, pump) {
            Some(HostToView::SetCookies(json)) => got.cookies = Some(json),
            Some(other) => got.first_message = Some(other),
            None => eprintln!("jar: no seed received"),
        }
        got
    }
}

impl HostSettings {
    /// Hex of the postcard bytes: one variable for every field, shell-safe on the WSL command line as the URL is.
    pub fn to_hex(&self) -> String {
        transport::to_hex(&postcard::to_stdvec(self).expect("HostSettings always encodes"))
    }

    /// `to_hex` read back; `None` for anything else.
    pub fn from_hex(hex: &str) -> Option<Self> {
        postcard::from_bytes(&transport::from_hex(hex)?).ok()
    }
}

/// `ENV_SETTINGS` for a host that reads no other `HostEnv` variable (WebKit).
pub fn settings_from_env() -> HostSettings {
    settings_from(std::env::var(ENV_SETTINGS).ok().as_deref())
}

/// The settings `ENV_SETTINGS` holds: the defaults when it is missing, or (logged) when it is not `to_hex`'s.
fn settings_from(hex: Option<&str>) -> HostSettings {
    let Some(hex) = hex else { return HostSettings::default() };
    HostSettings::from_hex(hex).unwrap_or_else(|| {
        eprintln!("{ENV_SETTINGS} is not a HostSettings encoding; using the defaults");
        HostSettings::default()
    })
}

/// Acts on `settings`, at spawn and on every `HostToView::SetSettings`: the palette `manifold://` pages are served
/// in, and the folder the next download is saved in (`downloads::folder`).
pub fn apply(settings: &HostSettings) {
    manifold_pages::set_dark(settings.dark);
    crate::downloads::set_folder(settings.download_dir.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn the_url_defaults_and_the_socket_id_is_required() {
        let env = HostEnv::from_vars(|name| (name == ENV_SOCKET_ID).then(|| "t1".to_string()), false);
        assert_eq!(
            env,
            HostEnv {
                socket_id: "t1".into(),
                initial_url: DEFAULT_URL.into(),
                await_cookies: false,
                settings: HostSettings::default()
            }
        );
        let env = HostEnv::from_vars(|name| Some(format!("<{name}>")), true);
        assert_eq!((env.socket_id.as_str(), env.initial_url.as_str()), ("<VIEW_SOCKET_ID>", "<VIEW_INITIAL_URL>"));
        assert_eq!(env.settings, HostSettings::default(), "<VIEW_SETTINGS> is not hex: the defaults");
        assert!(std::panic::catch_unwind(|| HostEnv::from_vars(|_| None, false)).is_err());
    }

    /// Review Focus 2: every field crosses the environment exactly, as text no shell can read.
    #[test]
    fn settings_cross_the_environment_exactly() {
        for download_dir in [None, Some(r"C:\Users\Zoë Q\Downloads"), Some("/mnt/c/it's \"here\" $(x)")] {
            for dark in [false, true] {
                let settings = HostSettings { dark, download_dir: download_dir.map(str::to_string) };
                let hex = settings.to_hex();
                assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()), "{hex}");
                assert_eq!(HostSettings::from_hex(&hex).as_ref(), Some(&settings));
                let env = HostEnv::from_vars(
                    |name| match name {
                        ENV_SOCKET_ID => Some("t".into()),
                        ENV_SETTINGS => Some(hex.clone()),
                        _ => None,
                    },
                    false,
                );
                assert_eq!(env.settings, settings);
            }
        }
        assert_eq!(HostSettings::from_hex("zz"), None);
        assert_eq!(settings_from(Some("zz")), HostSettings::default(), "not ours: the defaults");
        assert_eq!(settings_from(None), HostSettings::default(), "missing: the defaults");
    }

    /// The only test in this crate that sets the global palette and download folder; the others pass a folder in.
    /// Keep it so, or serialize it with a lock: a crate's tests run on parallel threads.
    #[test]
    fn applied_settings_set_the_palette_and_the_download_folder() {
        apply(&HostSettings { dark: false, download_dir: Some(r"C:\Zoë Q\dl".into()) });
        assert!(!manifold_pages::is_dark());
        assert_eq!(crate::downloads::folder(), Some(std::path::PathBuf::from(r"C:\Zoë Q\dl")));
        apply(&HostSettings { dark: true, download_dir: None });
        assert!(manifold_pages::is_dark());
        assert_eq!(crate::downloads::folder(), None);
    }

    #[test]
    fn a_seed_is_split_from_any_other_first_message() {
        let env = HostEnv {
            socket_id: "t".into(),
            initial_url: "u".into(),
            await_cookies: true,
            settings: HostSettings::default(),
        };
        let (tx, rx) = mpsc::channel();
        tx.send(HostToView::SetCookies("[]".into())).unwrap();
        let got = env.await_cookies(&rx, || {});
        assert_eq!((got.cookies.as_deref(), got.first_message.is_some()), (Some("[]"), false));
        tx.send(HostToView::GetCookies).unwrap();
        let got = env.await_cookies(&rx, || {});
        assert!(got.cookies.is_none() && matches!(got.first_message, Some(HostToView::GetCookies)));
    }

    #[test]
    fn a_host_not_awaiting_cookies_leaves_its_messages_alone() {
        let env = HostEnv {
            socket_id: "t".into(),
            initial_url: "u".into(),
            await_cookies: false,
            settings: HostSettings::default(),
        };
        let (tx, rx) = mpsc::channel();
        tx.send(HostToView::SetCookies("[]".into())).unwrap();
        let got = env.await_cookies(&rx, || {});
        assert!(got.cookies.is_none() && got.first_message.is_none());
        assert!(matches!(rx.try_recv(), Ok(HostToView::SetCookies(_))), "still queued for the loop");
    }
}
