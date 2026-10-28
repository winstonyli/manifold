//! The two servers `gate chrome-shots` needs that http.rs cannot be (they hold a connection open after answering):
//! the download server (was the inline python `serve.py`) and the fake proxy (was scripts/fake_proxy.py).

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The connections a server keeps open after answering, and whether `break_all` has run: a connection kept after
/// that is broken at once, so a break never misses a reply still being written.
#[derive(Default)]
struct Held(Mutex<(bool, Vec<TcpStream>)>);

impl Held {
    /// Keeps `stream` open until `break_all`, or breaks it now when that already ran.
    fn keep(&self, stream: TcpStream) {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if held.0 {
            let _ = stream.shutdown(Shutdown::Both);
        } else {
            held.1.push(stream);
        }
    }

    /// Breaks every kept connection, and every one kept from now on.
    fn break_all(&self) {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        held.0 = true;
        for stream in held.1.drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// A listener on 127.0.0.1 (or `SITE_HOST`) that hands each connection to `on_conn` on its own thread. A connection
/// the handler keeps (`Held::keep`) stays open until drop or `Held::break_all`.
struct Hold {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    held: Arc<Held>,
    thread: Option<JoinHandle<()>>,
}

impl Hold {
    /// Listens on `port`, or on any free port if that one is busy (0 asks for any) and says so.
    fn start(port: u16, on_conn: impl Fn(TcpStream, &Held) + Send + Sync + 'static) -> io::Result<Hold> {
        let host = crate::http::site_host();
        let listener = match TcpListener::bind((host.as_str(), port)) {
            Err(e) if port != 0 => {
                println!(
                    "port {port} is busy ({e}), so the shots that show this server's url won't match another run's"
                );
                TcpListener::bind((host.as_str(), 0))?
            }
            bound => bound?,
        };
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let held = Arc::new(Held::default());
        let on_conn = Arc::new(on_conn);
        let (stop2, held2) = (stop.clone(), held.clone());
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let (on_conn, held) = (on_conn.clone(), held2.clone());
                    thread::spawn(move || on_conn(stream, &held));
                }
            }
        });
        Ok(Hold { addr, stop, held, thread: Some(thread) })
    }
}

/// Stops listening (the accept thread exits and closes the listener) and breaks the held connections.
impl Drop for Hold {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        self.held.break_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Reads the request head (up to the blank line), as text.
fn request_head(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(n) if n > 0 => head.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// The path a request head asks for ("" when there is none).
fn request_path(head: &str) -> &str {
    head.lines().next().and_then(|line| line.split(' ').nth(1)).unwrap_or("")
}

/// The start of an open-ended `Range: bytes=N-`, the form an engine resuming a download sends.
fn range_start(head: &str) -> Option<usize> {
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("range") {
            return None;
        }
        value.trim().strip_prefix("bytes=")?.strip_suffix('-')?.parse().ok()
    })
}

/// The download server listens here: the download views show its url, so two runs' shots match only on the same port.
const DOWNLOADS_PORT: u16 = 47_311;

/// `/resume`'s size, and where its first transfer breaks.
const RESUME_LEN: usize = 1_000_000;
const RESUME_BREAK: usize = 300_000;

/// `/resume`'s bytes: not all one value, so a resume that continues from the wrong place makes a different file.
pub fn resume_body() -> Vec<u8> {
    (0..RESUME_LEN).map(|i| (i % 251) as u8).collect()
}

/// What the download server sends for one request: the bytes, whether to keep the connection open after them
/// (`Held::keep`), and whether it is the rest of `/resume` (a `206`).
struct Answer {
    bytes: Vec<u8>,
    hold: bool,
    partial: bool,
}

/// The reply to `path`. `range` is the start of an open-ended `Range: bytes=N-`; `first` marks the first plain
/// request for `/resume`, the one whose transfer breaks.
fn answer(path: &str, range: Option<usize>, first: bool) -> Answer {
    if path == "/resume" {
        let body = resume_body();
        let common = "Content-Type: application/zip\r\nContent-Disposition: attachment; filename=\"resume.zip\"\r\nETag: \"resume-1\"\r\nAccept-Ranges: bytes\r\nConnection: close\r\n";
        if let Some(start) = range.filter(|&start| start < RESUME_LEN) {
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\n{common}Content-Range: bytes {start}-{}/{RESUME_LEN}\r\nContent-Length: {}\r\n\r\n",
                RESUME_LEN - 1,
                RESUME_LEN - start
            );
            return Answer { bytes: [head.as_bytes(), &body[start..]].concat(), hold: false, partial: true };
        }
        let head = format!("HTTP/1.1 200 OK\r\n{common}Content-Length: {RESUME_LEN}\r\n\r\n");
        let sent = if first { &body[..RESUME_BREAK] } else { body.as_slice() };
        return Answer { bytes: [head.as_bytes(), sent].concat(), hold: first, partial: false };
    }
    let slow = path == "/slow";
    let (name, len) = if slow { ("slow", 1_000_000) } else { ("real", 4) };
    let head = format!(
        "HTTP/1.0 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"{name}.zip\"\r\nContent-Length: {len}\r\n\r\n"
    );
    let body: &[u8] = if slow { &[b'x'; 300_000] } else { b"data" };
    Answer { bytes: [head.as_bytes(), body].concat(), hold: slow, partial: false }
}

/// `/real.zip` answers a 4-byte attachment; `/slow` promises 1 MB, sends 300 KB and then holds the connection until
/// `kill`: the server dying mid-transfer. `/resume` is a file an engine can resume (HTTP/1.1, an `ETag`,
/// `Accept-Ranges`): the first request gets 300 KB of 1 MB and is held until `break_transfers`, and a `Range`
/// request gets the rest as a `206`, so a resumed download completes `resume_body`.
pub struct Downloads {
    hold: Hold,
    /// How many `Range` requests were answered with the rest (`ranges_answered`).
    ranges: Arc<AtomicUsize>,
}

impl Downloads {
    pub fn start() -> io::Result<Downloads> {
        let ranges = Arc::new(AtomicUsize::new(0));
        let (counted, plain) = (ranges.clone(), AtomicUsize::new(0));
        let hold = Hold::start(DOWNLOADS_PORT, move |mut stream, held| {
            let head = request_head(&mut stream);
            let (path, range) = (request_path(&head), range_start(&head));
            let first = path == "/resume" && range.is_none() && plain.fetch_add(1, Ordering::SeqCst) == 0;
            let reply = answer(path, range, first);
            // Counted before the reply goes out, so a check that saw the reply sees the count.
            if reply.partial {
                counted.fetch_add(1, Ordering::SeqCst);
            }
            if stream.write_all(&reply.bytes).and_then(|()| stream.flush()).is_err() {
                return;
            }
            if reply.hold {
                held.keep(stream);
            }
        })?;
        Ok(Downloads { hold, ranges })
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}:{}{path}", crate::http::site_host(), self.hold.addr.port())
    }

    /// Breaks the transfers in progress, and any that would be held from now on, while the server keeps answering:
    /// a dropped connection, not a dead server.
    pub fn break_transfers(&self) {
        self.hold.held.break_all();
    }

    /// How many `Range` requests for `/resume` were answered with the rest.
    pub fn ranges_answered(&self) -> usize {
        self.ranges.load(Ordering::SeqCst)
    }

    /// The server dies: transfers in progress break and new connections are refused (the engine retries a broken
    /// download at once, and a server that still answered would serve it again).
    pub fn kill(self) {}
}

/// An HTTP proxy for the engine installer's download: `refuse` answers each CONNECT 502 (the install fails at once);
/// otherwise it opens the tunnel and says nothing (the install sits in Installing with no bytes).
pub struct Proxy {
    hold: Hold,
    refuse: Arc<AtomicBool>,
}

impl Proxy {
    pub fn start(refuse: bool) -> io::Result<Proxy> {
        let refuse = Arc::new(AtomicBool::new(refuse));
        let mode = refuse.clone();
        let hold = Hold::start(0, move |mut stream, held| {
            let _ = request_head(&mut stream);
            let reply: &[u8] = if mode.load(Ordering::SeqCst) {
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n"
            } else {
                b"HTTP/1.1 200 Connection established\r\n\r\n"
            };
            if stream.write_all(reply).is_ok() && !mode.load(Ordering::SeqCst) {
                held.keep(stream);
            }
        })?;
        Ok(Proxy { hold, refuse })
    }

    /// The mode is read per connection, so it can change between installs.
    pub fn set_refuse(&self, refuse: bool) {
        self.refuse.store(refuse, Ordering::SeqCst);
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}", crate::http::site_host(), self.hold.addr.port())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url_port: u16, path: &str, headers: &str, read: usize) -> Vec<u8> {
        let mut s = TcpStream::connect((crate::http::site_host().as_str(), url_port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\n{headers}\r\n").unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while got.len() < read {
            match s.read(&mut buf) {
                Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
                _ => break,
            }
        }
        got
    }

    /// A reply's head, as text, and its body.
    fn split(bytes: &[u8]) -> (String, &[u8]) {
        let at = bytes.windows(4).position(|w| w == b"\r\n\r\n").expect("a whole head") + 4;
        (String::from_utf8_lossy(&bytes[..at]).into_owned(), &bytes[at..])
    }

    #[test]
    fn downloads_answer_real_in_full_and_slow_in_part_until_killed() {
        let server = Downloads::start().unwrap();
        let port = server.hold.addr.port();
        let real = String::from_utf8_lossy(&get(port, "/real.zip", "", usize::MAX)).into_owned();
        assert!(real.contains("filename=\"real.zip\"") && real.ends_with("data"), "{real}");
        let slow = get(port, "/slow", "", 300_000);
        assert!(slow.len() > 300_000 && slow.len() < 1_000_000, "{}", slow.len());
        server.kill();
    }

    /// `kill` is a dead server, not one that still answers the engine's retry of the broken download.
    #[test]
    fn a_killed_download_server_refuses_the_retry() {
        let server = Downloads::start().unwrap();
        let addr = server.hold.addr;
        server.kill();
        assert!(TcpStream::connect(addr).is_err(), "the killed server still accepts connections");
    }

    #[test]
    fn the_resume_answers_break_first_then_serve_ranges() {
        let body = resume_body();
        let first = answer("/resume", None, true);
        let (head, sent) = split(&first.bytes);
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
        assert!(head.contains("ETag: \"resume-1\"\r\n") && head.contains("Accept-Ranges: bytes\r\n"), "{head}");
        assert!(head.contains(&format!("Content-Length: {RESUME_LEN}\r\n")), "{head}");
        let want = &body[..RESUME_BREAK];
        assert!(sent == want && first.hold && !first.partial, "{} bytes", sent.len());
        let rest = answer("/resume", Some(RESUME_BREAK), false);
        let (head, sent) = split(&rest.bytes);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"), "{head}");
        let range = format!("Content-Range: bytes {RESUME_BREAK}-{}/{RESUME_LEN}\r\n", RESUME_LEN - 1);
        assert!(head.contains(&range), "{head}");
        let want = &body[RESUME_BREAK..];
        assert!(sent == want && !rest.hold && rest.partial, "{} bytes", sent.len());
        let again = answer("/resume", None, false);
        let (_, sent) = split(&again.bytes);
        let whole = body.as_slice();
        assert!(sent == whole && !again.hold, "{} bytes", sent.len());
        assert_eq!(range_start("GET /resume HTTP/1.1\r\nHost: x\r\nrange: bytes=300000-\r\n\r\n"), Some(300_000));
        assert_eq!(range_start("GET / HTTP/1.1\r\nRange: bytes=0-99\r\n\r\n"), None, "not open-ended");
        assert_eq!(range_start("GET /Range:bytes=1- HTTP/1.1\r\n\r\n"), None, "the request line is no header");
        assert_eq!(request_path("GET /resume HTTP/1.1\r\n\r\n"), "/resume");
        assert_eq!(request_path(""), "");
    }

    #[test]
    fn a_broken_resume_is_answered_from_where_it_stopped() {
        let server = Downloads::start().unwrap();
        let port = server.hold.addr.port();
        let body = resume_body();
        let first = get(port, "/resume", "", RESUME_BREAK + 1);
        assert!(
            first.starts_with(b"HTTP/1.1 200 OK\r\n"),
            "{}",
            String::from_utf8_lossy(&first[..first.len().min(64)])
        );
        server.break_transfers();
        let rest = get(port, "/resume", &format!("Range: bytes={RESUME_BREAK}-\r\n"), usize::MAX);
        let (head, sent) = split(&rest);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"), "{head}");
        let want = &body[RESUME_BREAK..];
        assert!(sent == want, "{} bytes", sent.len());
        assert_eq!(server.ranges_answered(), 1);
        let again = get(port, "/resume", "", usize::MAX);
        let (_, sent) = split(&again);
        let whole = body.as_slice();
        assert!(sent == whole, "{} bytes", sent.len());
        server.kill();
    }

    #[test]
    fn proxy_refuses_or_tunnels_by_mode() {
        let proxy = Proxy::start(true).unwrap();
        let port = proxy.hold.addr.port();
        assert!(String::from_utf8_lossy(&get(port, "x:443", "", usize::MAX)).starts_with("HTTP/1.1 502"));
        proxy.set_refuse(false);
        assert!(String::from_utf8_lossy(&get(port, "x:443", "", 10)).starts_with("HTTP/1.1 200"));
    }
}
