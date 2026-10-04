//! The HTTP client for content conversions, node Refresh and favicons: a
//! shared ureq agent with a User-Agent, and a fetch that keeps the error's
//! reason (with any URL left out). Engines fetch pages themselves. The
//! search suggestions and the engine installer still make their own ureq
//! calls; the installer shares `body_budget`.

use std::sync::OnceLock;
use std::time::Duration;

/// Sent with every fetch: some servers refuse a request without one.
const USER_AGENT: &str = concat!("manifold/", env!("CARGO_PKG_VERSION"));

/// How long each stage of one fetch may take.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    /// Resolving the host name, and then (again) connecting, TLS included:
    /// each of the two stages gets this much.
    pub connect: Duration,
    /// From the request to the response's headers.
    pub response: Duration,
    /// The whole body, in total (ureq 3 has no per-read idle timeout).
    pub body: Duration,
}

fn agent_config() -> ureq::config::Config {
    ureq::Agent::config_builder().user_agent(USER_AGENT).build()
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| agent_config().into())
}

/// `GET url`'s body, at most `limit` bytes, within `budget`. The error says
/// why (status, timeout, too large), for the caller to log; it never holds
/// the URL (see `describe`). Besides each stage's own limit, the whole call
/// is capped at the stages' sum, a backstop for any stage ureq leaves
/// unbounded.
pub(crate) fn get_limited(url: &str, limit: u64, budget: Budget) -> Result<Vec<u8>, String> {
    get_limited_with(agent(), url, limit, budget)
}

fn get_limited_with(agent: &ureq::Agent, url: &str, limit: u64, budget: Budget) -> Result<Vec<u8>, String> {
    let response = agent
        .get(url)
        .config()
        .timeout_global(Some(budget.connect * 2 + budget.response + budget.body))
        .timeout_resolve(Some(budget.connect))
        .timeout_connect(Some(budget.connect))
        .timeout_recv_response(Some(budget.response))
        .timeout_recv_body(Some(budget.body))
        .build()
        .call()
        .map_err(|e| describe(&e))?;
    response.into_body().with_config().limit(limit).read_to_vec().map_err(|e| describe(&e))
}

/// `e` for a log line. The variants that carry the request's URI (which can
/// hold userinfo or a token) give only their kind.
fn describe(e: &ureq::Error) -> String {
    match e {
        ureq::Error::BadUri(_) => "bad uri".into(),
        ureq::Error::Http(_) => "http: invalid request".into(),
        ureq::Error::RequireHttpsOnly(_) => "https only".into(),
        ureq::Error::Other(_) => "other error".into(),
        e => e.to_string(),
    }
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

    /// Records the time the resolve stage was given, and fails it.
    #[derive(Debug, Default)]
    struct RecordingResolver(std::sync::Arc<std::sync::Mutex<Option<Option<Duration>>>>);

    impl ureq::unversioned::resolver::Resolver for RecordingResolver {
        fn resolve(
            &self,
            _uri: &ureq::http::Uri,
            _config: &ureq::config::Config,
            timeout: ureq::unversioned::transport::NextTimeout,
        ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
            use ureq::unversioned::transport::time::Duration as UreqDuration;
            let after = match timeout.after {
                UreqDuration::Exact(d) => Some(d),
                _ => None, // `NotHappening`: no deadline at all
            };
            *self.0.lock().unwrap() = Some(after);
            Err(ureq::Error::HostNotFound)
        }
    }

    /// A blackholed resolver must not hold a fetch (and a favicon slot) for
    /// ever: the lookup gets a deadline no later than the connect budget.
    #[test]
    fn the_name_lookup_has_a_deadline() {
        let resolver = RecordingResolver::default();
        let seen = resolver.0.clone();
        let agent =
            ureq::Agent::with_parts(agent_config(), ureq::unversioned::transport::DefaultConnector::new(), resolver);
        assert!(get_limited_with(&agent, "http://unresolvable.invalid/x", 100, QUICK).is_err());
        let given = seen.lock().unwrap().expect("the resolver was asked");
        assert!(given.is_some_and(|d| d <= QUICK.connect), "{given:?}");
    }

    #[test]
    fn an_error_never_carries_the_url() {
        let e = ureq::Error::BadUri("https://user:secret@host/x?token=t".into());
        let text = describe(&e);
        assert!(!text.contains("secret") && !text.contains("token"), "{text}");
        assert_eq!(describe(&ureq::Error::HostNotFound), "host not found");
    }

    #[test]
    fn the_body_budget_scales_with_size_and_has_a_floor() {
        let floor = Duration::from_secs(120);
        assert_eq!(body_budget(0, 32 * 1024, floor), floor);
        assert_eq!(body_budget(171_619_649, 32 * 1024, floor), Duration::from_secs(171_619_649 / (32 * 1024)));
    }
}
