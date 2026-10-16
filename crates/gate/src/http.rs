//! A minimal HTTP/1.0 server on 127.0.0.1 (or `SITE_HOST`) with an ephemeral port, for the gate's local sites: the smoke test's
//! suggestions endpoint (suggest.rs) and the two-instance test's nonce site (nonce.rs). Each connection gets a thread;
//! the handler turns the request line into the whole response, which is written before the connection closes.
//! Dropping the server stops its accept loop. `SITE_HOST=<ip>` binds and addresses the site at that local address instead,
//! for an engine host that cannot reach loopback: WebKit runs in WSL, and the Windows address it sees is the gateway
//! from `ip route` in WSL (`ip route | awk '/default/ {print $3}'`).

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A connection that sends nothing is dropped after this long (python's server would wait forever in a thread).
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The request line (`GET /r?n=1 HTTP/1.1`) to the complete response bytes.
pub type Handler = dyn Fn(&str) -> Vec<u8> + Send + Sync;

/// The request line and the whole request head (line and headers) to the complete response bytes.
pub type HeadHandler = dyn Fn(&str, &str) -> Vec<u8> + Send + Sync;

/// `HTTP/1.0 <status>` with Content-Type, Content-Length, the `extra` headers and `Connection: close`, then `body`.
pub fn response(status: &str, extra: &[(&str, &str)], ctype: &str, body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.0 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

fn handle(mut stream: TcpStream, handler: &HeadHandler) -> io::Result<()> {
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
    stream.write_all(&handler(line, &text))?;
    stream.shutdown(Shutdown::Both)
}

/// The running server. Dropping it stops the accept loop.
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// `SITE_HOST`, default `127.0.0.1`: the local address the gate's site listens on.
fn site_host() -> String {
    std::env::var("SITE_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned())
}

impl Server {
    pub fn start(handler: Arc<Handler>) -> io::Result<Server> {
        Self::start_head(Arc::new(move |line, _head| handler(line)))
    }

    /// `start` for a handler that reads the request headers too (cookies, user agent).
    pub fn start_head(handler: Arc<HeadHandler>) -> io::Result<Server> {
        let listener = TcpListener::bind((site_host().as_str(), 0))?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let handler = handler.clone();
                    thread::spawn(move || {
                        let _ = handle(stream, &*handler);
                    });
                }
            }
        });
        Ok(Server { addr, stop, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// `http://HOST:PORT`, the site's origin.
    pub fn base(&self) -> String {
        format!("http://{}:{}", self.addr.ip(), self.addr.port())
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
