pub mod channel;
pub mod downloads;
pub mod frame_buffer;
pub mod frame_sink;
pub mod host_env;
pub mod messages;
pub mod nav_label;
pub mod render;
pub mod transport;

pub use channel::MessageChannel;
pub use frame_buffer::{frame_path, remove_frame_file, sweep_stale_frames_in, SharedFrameBuffer};
pub use frame_sink::{copy_changed_rows, FrameSink};
pub use host_env::{HostEnv, StartupMessages};
pub use messages::{
    modifiers, restore_scroll_script, HostToView, InputEvent, KeyCode, MouseButton, PageKindReply, PageState, TabState,
    ViewToHost, PAGE_KIND_SCRIPT, PAGE_STATE_SCRIPT,
};
pub use transport::{LocalTransport, RemoteTransport};

/// Written as the first 4 bytes (u32 LE) of every control stream, before any
/// postcard message. postcard encodes enum variants by position, so a host
/// and engine built from different `messages.rs` would silently misread each
/// other. Bump it on any change to `HostToView`, `ViewToHost` or a type
/// they carry. The `wire_shape_tests` test in `messages.rs` fails when that shape changes without a bump.
/// 2: `ViewToHost::OpenTabRequested` carries `user_gesture`.
/// 3: `InputEvent::MouseDown`/`MouseUp` carry `click_count`; `TabState` drops `zoom`.
/// 4: `ViewToHost::FrameReady` carries `generation` (frame files are named by generation) and drops `frame_id`.
pub const PROTOCOL_VERSION: u32 = 4;

/// Starts the `ViewToHost::Crashed` reason for a version mismatch, so the
/// host shows a failed card instead of respawning the same stale binary.
pub const PROTOCOL_MISMATCH: &str = "protocol mismatch";

/// Starts the one line an engine host writes to stderr (its per-tab log) when it cannot start at all and
/// exits before connecting, e.g. a CEF runtime that doesn't support the host's API version. manifold shows the rest
/// of that line as the failure (`process_manager::connect_host`) instead of the raw log tail. A host that fails
/// to start after connecting (WebView2 creating its environment) also sends that line as an `ViewToHost::Crashed`
/// reason, and manifold leaves a failed card naming the rest instead of respawning (`tabs::after_crash`).
pub const HOST_FATAL: &str = "view-host fatal: ";

/// `Ok` when the peer speaks `PROTOCOL_VERSION`.
pub fn check_version(theirs: u32) -> Result<(), String> {
    if theirs == PROTOCOL_VERSION {
        return Ok(());
    }
    Err(format!(
        "{PROTOCOL_MISMATCH}: the engine host speaks protocol {theirs}, manifold speaks {PROTOCOL_VERSION}; \
         rebuild the engine host"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// M0 acceptance test: a `HostToView`/`ViewToHost` round trip over
    /// the real transport each side will use in M1 — `LocalTransport`
    /// (named pipe on Windows) for the control channel.
    #[test]
    fn local_transport_loopback_ping() {
        let tab_id = "test-loopback";
        let listeners = transport::listen_local_pair(tab_id).expect("listen_local_pair");

        let server = thread::spawn(move || {
            let (mut to_view, mut from_view) =
                transport::accept_local_pair(&listeners, std::time::Duration::from_secs(5)).expect("accept_local_pair");
            assert_eq!(from_view.recv_version().expect("engine's version"), PROTOCOL_VERSION);
            to_view
                .send(&HostToView::Navigate { url: "https://example.com".into(), referrer: None })
                .expect("send Navigate");
            let reply: ViewToHost = from_view.recv().expect("recv NavigationStateChanged");
            match reply {
                ViewToHost::NavigationStateChanged { url, .. } => {
                    assert_eq!(url, "https://example.com")
                }
                other => panic!("unexpected reply: {other:?}"),
            }
        });

        // Give the listeners a moment to be ready before the client dials in.
        thread::sleep(std::time::Duration::from_millis(50));

        let (mut from_host, mut to_host) = transport::connect_local_pair(tab_id).expect("connect_local_pair");
        assert_eq!(from_host.recv_version().expect("host's version"), PROTOCOL_VERSION);
        let msg: HostToView = from_host.recv().expect("recv Navigate");
        match msg {
            HostToView::Navigate { url, .. } => assert_eq!(url, "https://example.com"),
            other => panic!("unexpected message: {other:?}"),
        }
        to_host
            .send(&ViewToHost::NavigationStateChanged {
                url: "https://example.com".into(),
                title: "Example".into(),
                can_go_back: false,
                can_go_forward: false,
                loading: false,
                user_started: None,
            })
            .expect("send NavigationStateChanged");

        server.join().unwrap();
    }

    #[test]
    fn a_version_mismatch_names_both_versions() {
        assert!(check_version(PROTOCOL_VERSION).is_ok());
        let err = check_version(PROTOCOL_VERSION + 1).unwrap_err();
        assert!(err.starts_with(PROTOCOL_MISMATCH), "{err}");
        assert!(err.contains(&(PROTOCOL_VERSION + 1).to_string()));
    }
}
