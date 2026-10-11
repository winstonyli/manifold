//! The smoke test's local suggestions endpoint: what `scripts/probe_server.py` answers on `GET /ac/?q=X` (the only
//! route `scripts/headless-smoke-test.sh` uses), as a `std::net` server on an ephemeral port. The app reaches it
//! through `MANIFOLD_SUGGESTIONS_URL` (crates/manifold/src/suggestions.rs), so the address-bar checks need no
//! network. The body is DuckDuckGo's `type=list` shape: `[q, [q one, q two, q three]]`.
//!
//! The python server also served the key-probe, scroll and page-kind routes; no smoke check uses them, so any
//! other path is a 404 here. It kept a `page.log` of those routes' requests; the smoke test never reads one, so
//! the log here is the paths requested, in memory (`Server::requests`).
//!
//! Also here: the query-string decoding both the server and the smoke test's `q` variable need (python's
//! `urllib.parse.parse_qs`).

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A connection that sends nothing is dropped after this long (python's server would wait forever in a thread).
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Python's `unquote(s.replace('+', ' '))`: `+` is a space, `%XX` a byte, the bytes read as UTF-8 with invalid
/// sequences replaced (`errors='replace'`). A `%` not followed by two hex digits stays as it is.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() => {
                out.push(hex(b[i + 1]).unwrap_or(0) * 16 + hex(b[i + 2]).unwrap_or(0));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    char::from(c).to_digit(16).map(|d| d as u8)
}

/// The first value of `key` in a query string, decoded as python's `parse_qs(query, keep_blank_values=...)[key][0]`
/// does: pairs split on `&`, a pair with no `=` counts (as a blank value) only when blanks are kept, and a blank
/// value is skipped unless they are.
pub fn query_param(query: &str, key: &str, keep_blank: bool) -> Option<String> {
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None if keep_blank => (pair, ""),
            None => continue,
        };
        if (!v.is_empty() || keep_blank) && percent_decode(k) == key {
            return Some(percent_decode(v));
        }
    }
    None
}

/// Python's `urlparse(url).query`: what follows the first `?`, before any `#`.
pub fn url_query(url: &str) -> &str {
    let no_fragment = url.split('#').next().unwrap_or("");
    no_fragment.split_once('?').map_or("", |(_, q)| q)
}

/// The complete HTTP response to a request line (`GET /ac/?q=weather&type=list HTTP/1.1`).
pub fn respond(request_line: &str) -> Vec<u8> {
    let target = request_line.split(' ').nth(1).unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (status, ctype, body) = if request_line.starts_with("GET ") && path == "/ac/" {
        let q = query_param(query, "q", true).unwrap_or_default();
        let body = serde_json::json!([q, [format!("{q} one"), format!("{q} two"), format!("{q} three")]]);
        ("200 OK", "application/json", body.to_string())
    } else {
        ("404 Not Found", "text/plain", String::new())
    };
    format!(
        "HTTP/1.0 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn handle(mut stream: TcpStream, log: &Mutex<Vec<String>>) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&head);
    let line = text.lines().next().unwrap_or("");
    if let Ok(mut l) = log.lock() {
        l.push(line.split(' ').nth(1).unwrap_or("").to_string());
    }
    stream.write_all(&respond(line))?;
    stream.shutdown(Shutdown::Both)
}

/// The running endpoint. Dropping it stops the accept loop.
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Listens on `127.0.0.1` with an ephemeral port and answers each connection on a thread of its own.
    pub fn start() -> io::Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (stop2, log2) = (stop.clone(), log.clone());
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let log = log2.clone();
                    thread::spawn(move || {
                        let _ = handle(stream, &log);
                    });
                }
            }
        });
        Ok(Server { addr, stop, log, thread: Some(thread) })
    }

    /// The value for `MANIFOLD_SUGGESTIONS_URL`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/ac/", self.addr.port())
    }

    /// The request targets seen so far (path and query), oldest first.
    pub fn requests(&self) -> Vec<String> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_like_parse_qs() {
        assert_eq!(percent_decode("sourdough+bread+two"), "sourdough bread two");
        assert_eq!(percent_decode("a%20b%2Bc"), "a b+c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        assert_eq!(percent_decode("%ff"), "\u{fffd}");
    }

    #[test]
    fn query_params() {
        let q = url_query("https://duckduckgo.com/?q=weather+one&ia=web");
        assert_eq!(q, "q=weather+one&ia=web");
        assert_eq!(query_param(q, "q", false).as_deref(), Some("weather one"));
        assert_eq!(url_query("about:blank"), "");
        assert_eq!(url_query("https://x/?q=a#frag?q=b"), "q=a");
        // Blank values: kept for the server (keep_blank_values=True), skipped for the smoke test's `q`.
        assert_eq!(query_param("q=&q=x", "q", true).as_deref(), Some(""));
        assert_eq!(query_param("q=&q=x", "q", false).as_deref(), Some("x"));
        assert_eq!(query_param("q", "q", true).as_deref(), Some(""));
        assert_eq!(query_param("q", "q", false), None);
        assert_eq!(query_param("a=1", "q", true), None);
    }

    #[test]
    fn suggestions_have_duckduckgos_list_shape() {
        let r = String::from_utf8(respond("GET /ac/?q=sourdough+bread&type=list HTTP/1.1")).unwrap();
        let (head, body) = r.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.0 200 OK"));
        assert!(head.contains("Content-Type: application/json"));
        assert!(head.contains(&format!("Content-Length: {}", body.len())));
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            v,
            serde_json::json!([
                "sourdough bread",
                ["sourdough bread one", "sourdough bread two", "sourdough bread three"]
            ])
        );
        // No q: python answers `["", [" one", " two", " three"]]`.
        let r = String::from_utf8(respond("GET /ac/ HTTP/1.1")).unwrap();
        assert!(r.ends_with(r#"["",[" one"," two"," three"]]"#));
        assert!(String::from_utf8(respond("GET /other HTTP/1.1")).unwrap().starts_with("HTTP/1.0 404"));
    }

    #[test]
    fn serves_over_a_socket_and_logs_the_request() {
        let server = Server::start().unwrap();
        let addr = server.url().trim_start_matches("http://").trim_end_matches("/ac/").to_string();
        let mut c = TcpStream::connect(&addr).unwrap();
        c.write_all(b"GET /ac/?q=weather&type=list HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut got = String::new();
        c.read_to_string(&mut got).unwrap();
        assert!(got.ends_with(r#"["weather",["weather one","weather two","weather three"]]"#), "{got}");
        assert_eq!(server.requests(), ["/ac/?q=weather&type=list"]);
    }
}
