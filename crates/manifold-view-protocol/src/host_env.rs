//! What manifold tells a local view-host at startup, through the environment (not argv: CEF parses its own argv as a
//! Chromium command line), and the bounded wait for the cookie-jar seed. `process_manager::spawn_tab` writes it with
//! the `ENV_*` names; every host reads it here, so the contract is stated once.

use std::time::Duration;

use crate::{transport, HostToView};

/// Names the control sockets and shared-memory frames. Normally the tab id, but manifold passes a different one when
/// a stale host from a crashed run still holds the tab id's pipes (`spawn_tab`'s fallback).
pub const ENV_SOCKET_ID: &str = "VIEW_SOCKET_ID";
/// The URL the first navigation opens.
pub const ENV_INITIAL_URL: &str = "VIEW_INITIAL_URL";
/// Set (to anything) for a jar engine: wait for `SetCookies` before the first navigation.
pub const ENV_AWAIT_COOKIES: &str = "VIEW_AWAIT_COOKIES";

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn the_url_defaults_and_the_socket_id_is_required() {
        let env = HostEnv::from_vars(|name| (name == ENV_SOCKET_ID).then(|| "t1".to_string()), false);
        assert_eq!(env, HostEnv { socket_id: "t1".into(), initial_url: DEFAULT_URL.into(), await_cookies: false });
        let env = HostEnv::from_vars(|name| Some(format!("<{name}>")), true);
        assert_eq!((env.socket_id.as_str(), env.initial_url.as_str()), ("<VIEW_SOCKET_ID>", "<VIEW_INITIAL_URL>"));
        assert!(std::panic::catch_unwind(|| HostEnv::from_vars(|_| None, false)).is_err());
    }

    #[test]
    fn a_seed_is_split_from_any_other_first_message() {
        let env = HostEnv { socket_id: "t".into(), initial_url: "u".into(), await_cookies: true };
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
        let env = HostEnv { socket_id: "t".into(), initial_url: "u".into(), await_cookies: false };
        let (tx, rx) = mpsc::channel();
        tx.send(HostToView::SetCookies("[]".into())).unwrap();
        let got = env.await_cookies(&rx, || {});
        assert!(got.cookies.is_none() && got.first_message.is_none());
        assert!(matches!(rx.try_recv(), Ok(HostToView::SetCookies(_))), "still queued for the loop");
    }
}
