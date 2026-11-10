//! What manifold tells a local view-host at startup, through the environment (not argv: CEF parses its own argv as a
//! Chromium command line), and the bounded wait for the cookie-jar seed. The app's settings a host acts on
//! (`HostSettings`) come the same way at spawn, as hex (`ENV_SETTINGS`), and later as `HostToView::SetSettings`; a host
//! applies both with `apply`. `process_manager::spawn_tab` writes it with the `ENV_*` names; every host reads the shared
//! part here (`HostEnv`) and its engine's own variables in its `main`, under the same names, so each name is stated once.

use std::sync::mpsc;
use std::time::{Duration, Instant};

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
/// Set (to anything) by `gate hostrate`: every `FrameSink::take` logs `[frame] 1` (a frame sent), `[frame] 0` (a
/// poll that found nothing) or `[frame] -` (a turn while hidden), so the gate can count frames and loop turns from
/// the host log.
pub const ENV_FRAME_STATS: &str = "VIEW_FRAME_STATS";

// One engine's own variables: only its host reads them, but they are named here too, once for both sides
// (`gate`'s `host_variables_are_named_only_in_host_env`).
/// CEF: this host's own `root_cache_path`, a folder no other CEF process uses.
pub const ENV_CEF_CACHE_DIR: &str = "VIEW_CACHE_DIR";
/// WebView2: the profile name this host uses, one no other live host of this manifold uses.
pub const ENV_WEBVIEW2_PROFILE: &str = "VIEW_WEBVIEW2_PROFILE";
/// WebView2: the user-data folder this manifold's WebView2 hosts share, and no other manifold's.
pub const ENV_WEBVIEW2_USER_DATA: &str = "VIEW_WEBVIEW2_USER_DATA";
/// WebKit: the first URL, as hex, shell-safe on the WSL command line.
pub const ENV_WEBKIT_INITIAL_URL_HEX: &str = "VIEW_INITIAL_URL_HEX";
/// WebKit: the port of manifold's TCP listener.
pub const ENV_WEBKIT_TCP_PORT: &str = "VIEW_TCP_PORT";
/// WebKit: the connection token, as hex.
pub const ENV_WEBKIT_TOKEN: &str = "VIEW_TOKEN";
/// WebKit: the address manifold's listener is on.
pub const ENV_WEBKIT_HOST_IP: &str = "VIEW_HOST_IP";

/// How long a host started with `ENV_AWAIT_COOKIES` waits for manifold's `SetCookies` before navigating. The seed goes
/// out when the UI thread lands the spawn, so a long stall there uses up this wait; a late seed still applies, from the
/// next load on.
pub const COOKIE_SEED_WAIT: Duration = Duration::from_secs(5);

/// The URL a host opens when manifold passes none.
const DEFAULT_URL: &str = "https://example.com";

/// The shared part of a local host's startup environment. Per-engine variables (CEF's cache dir, WebView2's profile
/// name and folder, WebKit's TCP settings) are read by their hosts, under the names above.
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
    /// hosts started with `ENV_AWAIT_COOKIES`). Each time the inbox is empty it calls `idle(deadline)`, which runs the
    /// engine's own loop until a wake (the reader's, after a message) or `deadline`. Any other first message is
    /// returned for the host's loop rather than dropped. Nothing is waited for otherwise.
    pub fn await_cookies(&self, rx: &mpsc::Receiver<HostToView>, idle: impl FnMut(Instant)) -> StartupMessages {
        self.await_cookies_within(rx, COOKIE_SEED_WAIT, idle)
    }

    fn await_cookies_within(
        &self,
        rx: &mpsc::Receiver<HostToView>,
        timeout: Duration,
        mut idle: impl FnMut(Instant),
    ) -> StartupMessages {
        let mut got = StartupMessages::default();
        if !self.await_cookies {
            return got;
        }
        let deadline = Instant::now() + timeout;
        let first = loop {
            match rx.try_recv() {
                Ok(msg) => break Some(msg),
                Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => idle(deadline),
                Err(_) => break None,
            }
        };
        match first {
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

/// Whether `ENV_FRAME_STATS` is set, read once.
pub fn frame_stats() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os(ENV_FRAME_STATS).is_some())
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
        let got = env.await_cookies(&rx, |_| {});
        assert_eq!((got.cookies.as_deref(), got.first_message.is_some()), (Some("[]"), false));
        tx.send(HostToView::GetCookies).unwrap();
        let got = env.await_cookies(&rx, |_| {});
        assert!(got.cookies.is_none() && matches!(got.first_message, Some(HostToView::GetCookies)));
    }

    fn awaiting() -> HostEnv {
        HostEnv {
            socket_id: "t".into(),
            initial_url: "u".into(),
            await_cookies: true,
            settings: HostSettings::default(),
        }
    }

    #[test]
    fn await_cookies_idles_until_the_cookies_arrive() {
        let (tx, rx) = mpsc::channel();
        let mut deadlines = Vec::new();
        let start = Instant::now();
        let got = awaiting().await_cookies(&rx, |deadline| {
            deadlines.push(deadline);
            if deadlines.len() == 3 {
                tx.send(HostToView::SetCookies("[]".into())).unwrap();
            }
        });
        assert_eq!(got.cookies.as_deref(), Some("[]"));
        assert_eq!(deadlines.len(), 3, "idle ran once per empty look");
        assert!(deadlines.iter().all(|d| *d == deadlines[0]), "one deadline for the whole wait");
        assert!(deadlines[0] >= start + COOKIE_SEED_WAIT - Duration::from_millis(100));
    }

    #[test]
    fn await_cookies_stops_at_its_deadline() {
        let (tx, rx) = mpsc::channel::<HostToView>();
        let (done_tx, done) = mpsc::channel();
        std::thread::spawn(move || {
            let mut idles = 0;
            let got = awaiting().await_cookies_within(&rx, Duration::from_millis(50), |_| {
                idles += 1;
                assert!(idles < 2000, "idle ran 2,000 times: the deadline is not checked");
                std::thread::sleep(Duration::from_millis(1));
            });
            done_tx.send((got.cookies, got.first_message.is_some())).unwrap();
        });
        let got = done.recv_timeout(Duration::from_secs(1)).expect("an empty result within 1 s");
        assert_eq!(got, (None, false));
        drop(tx); // alive until here, so the wait ended on its deadline, not on a closed channel
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
        let got = env.await_cookies(&rx, |_| {});
        assert!(got.cookies.is_none() && got.first_message.is_none());
        assert!(matches!(rx.try_recv(), Ok(HostToView::SetCookies(_))), "still queued for the loop");
    }
}
