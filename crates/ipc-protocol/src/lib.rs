pub mod channel;
pub mod frame_buffer;
pub mod messages;
pub mod transport;

pub use channel::MessageChannel;
pub use frame_buffer::{frame_path, SharedFrameBuffer};
pub use messages::{modifiers, EngineToHost, HostToEngine, InputEvent, KeyCode, MouseButton, TabState};
pub use transport::{LocalTransport, RemoteTransport};

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
            })
            .expect("send NavigationStateChanged");

        server.join().unwrap();
    }
}
