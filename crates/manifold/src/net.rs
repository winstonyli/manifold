//! The one HTTP client for manifold's own fetches (content conversions,
//! Refresh, favicons): a shared ureq agent with a User-Agent, and a fetch
//! that keeps the error's reason. Engines fetch pages themselves; this is
//! only what manifold downloads on its own.

use std::sync::OnceLock;
use std::time::Duration;

/// Sent with every fetch: some servers refuse a request without one.
const USER_AGENT: &str = concat!("manifold/", env!("CARGO_PKG_VERSION"));

/// How long each stage of one fetch may take.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    /// Resolving and connecting, TLS included.
    pub connect: Duration,
    /// From the request to the response's headers.
    pub response: Duration,
    /// The whole body, in total (ureq 3 has no per-read idle timeout).
    pub body: Duration,
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| ureq::Agent::config_builder().user_agent(USER_AGENT).build().into())
}

/// `GET url`'s body, at most `limit` bytes, within `budget`. The error says
/// why (status, timeout, too large), for the caller to log.
pub(crate) fn get_limited(url: &str, limit: u64, budget: Budget) -> Result<Vec<u8>, String> {
    let response = agent()
        .get(url)
        .config()
        .timeout_connect(Some(budget.connect))
        .timeout_recv_response(Some(budget.response))
        .timeout_recv_body(Some(budget.body))
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    response.into_body().with_config().limit(limit).read_to_vec().map_err(|e| e.to_string())
}

/// The time `bytes` take at `bytes_per_sec`, at least `floor`: a body
/// budget that lets a large download finish on a slow link but still ends
/// a dead connection.
pub(crate) fn body_budget(bytes: u64, bytes_per_sec: u64, floor: Duration) -> Duration {
    Duration::from_secs(bytes / bytes_per_sec.max(1)).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serves `response` to one connection, then hands back the request it read.
    fn serve_once(response: &'static [u8], stall: Option<Duration>) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let n = stream.read(&mut request).unwrap();
            stream.write_all(response).unwrap();
            if let Some(stall) = stall {
                std::thread::sleep(stall);
            }
            String::from_utf8_lossy(&request[..n]).into_owned()
        });
        (url, handle)
    }

    const QUICK: Budget =
        Budget { connect: Duration::from_secs(2), response: Duration::from_secs(2), body: Duration::from_millis(300) };

    #[test]
    fn a_fetch_sends_the_user_agent_and_returns_the_body() {
        let (url, server) = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello", None);
        assert_eq!(get_limited(&url, 100, QUICK).unwrap(), b"hello");
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.contains("user-agent: manifold/"), "{request}");
    }

    #[test]
    fn a_body_past_the_limit_is_an_error_with_a_reason() {
        let (url, server) = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello", None);
        let err = get_limited(&url, 3, QUICK).unwrap_err();
        assert!(!err.is_empty());
        server.join().unwrap();
    }

    #[test]
    fn a_stalled_body_gives_up_within_its_budget() {
        let (url, server) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\npartial", Some(Duration::from_secs(3)));
        let started = std::time::Instant::now();
        assert!(get_limited(&url, 100, QUICK).is_err());
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        server.join().unwrap();
    }

    #[test]
    fn the_body_budget_scales_with_size_and_has_a_floor() {
        let floor = Duration::from_secs(120);
        assert_eq!(body_budget(0, 32 * 1024, floor), floor);
        assert_eq!(body_budget(171_619_649, 32 * 1024, floor), Duration::from_secs(171_619_649 / (32 * 1024)));
    }
}
