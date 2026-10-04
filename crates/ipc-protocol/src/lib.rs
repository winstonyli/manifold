pub mod channel;
pub mod frame_buffer;
pub mod messages;
pub mod transport;

pub use channel::MessageChannel;
pub use frame_buffer::{frame_path, remove_frame_file, sweep_stale_frames_in, SharedFrameBuffer};
pub use messages::{
    modifiers, restore_scroll_script, EngineToHost, HostToEngine, InputEvent, KeyCode, MouseButton, PageKindReply,
    PageState, TabState, PAGE_KIND_SCRIPT, PAGE_STATE_SCRIPT,
};
pub use transport::{LocalTransport, RemoteTransport};

/// Written as the first 4 bytes (u32 LE) of every control stream, before any
/// postcard message. postcard encodes enum variants by position, so a host
/// and engine built from different `messages.rs` would silently misread each
/// other. Bump it on any change to `HostToEngine`, `EngineToHost` or a type
/// they carry.
pub const PROTOCOL_VERSION: u32 = 1;

/// Starts the `EngineToHost::Crashed` reason for a version mismatch, so the
/// host shows a failed card instead of respawning the same stale binary.
pub const PROTOCOL_MISMATCH: &str = "protocol mismatch";

/// `Ok` when the peer speaks `PROTOCOL_VERSION`.
pub fn check_version(theirs: u32) -> Result<(), String> {
    if theirs == PROTOCOL_VERSION {
        return Ok(());
    }
    Err(format!(
        "{PROTOCOL_MISMATCH}: the engine host speaks protocol {theirs}, manifold speaks {PROTOCOL_VERSION}; rebuild the engine host"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// M0 acceptance test: a `HostToEngine`/`EngineToHost` round trip over
    /// the real transport each side will use in M1 — `LocalTransport`
    /// (named pipe on Windows) for the control channel.
    #[test]
    fn local_transport_loopback_ping() {
        let tab_id = "test-loopback";
        let listeners = transport::listen_local_pair(tab_id).expect("listen_local_pair");

        let server = thread::spawn(move || {
            let (mut to_engine, mut from_engine) =
                transport::accept_local_pair(&listeners, std::time::Duration::from_secs(5)).expect("accept_local_pair");
            assert_eq!(from_engine.recv_version().expect("engine's version"), PROTOCOL_VERSION);
            to_engine
                .send(&HostToEngine::Navigate { url: "https://example.com".into(), referrer: None })
                .expect("send Navigate");
            let reply: EngineToHost = from_engine.recv().expect("recv NavigationStateChanged");
            match reply {
                EngineToHost::NavigationStateChanged { url, .. } => {
                    assert_eq!(url, "https://example.com")
                }
                other => panic!("unexpected reply: {other:?}"),
            }
        });

        // Give the listeners a moment to be ready before the client dials in.
        thread::sleep(std::time::Duration::from_millis(50));

        let (mut from_host, mut to_host) = transport::connect_local_pair(tab_id).expect("connect_local_pair");
        assert_eq!(from_host.recv_version().expect("host's version"), PROTOCOL_VERSION);
        let msg: HostToEngine = from_host.recv().expect("recv Navigate");
        match msg {
            HostToEngine::Navigate { url, .. } => assert_eq!(url, "https://example.com"),
            other => panic!("unexpected message: {other:?}"),
        }
        to_host
            .send(&EngineToHost::NavigationStateChanged {
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
