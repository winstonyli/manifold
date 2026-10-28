//! The two servers `gate chrome-shots` needs that http.rs cannot be (they hold a connection open after answering):
//! the download server (was the inline python `serve.py`) and the fake proxy (was scripts/fake_proxy.py).

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

type Held = Mutex<Vec<TcpStream>>;

/// A listener on 127.0.0.1 (or `SITE_HOST`) that hands each connection to `on_conn` on its own thread. A connection the handler
/// pushes onto the held list stays open until drop.
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
        for stream in self.held.lock().unwrap_or_else(PoisonError::into_inner).drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Reads the request head (up to the blank line) and returns its first line.
fn request_line(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
        match stream.read(&mut buf) {
            Ok(n) if n > 0 => head.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_string()
}

/// The download server listens here: the download views show its url, so two runs' shots match only on the same port.
const DOWNLOADS_PORT: u16 = 47_311;

/// `/real.zip` answers a 4-byte attachment; `/slow` promises 1 MB, sends 300 KB and then holds the connection until
/// `kill`: the server dying mid-transfer.
pub struct Downloads(Hold);

impl Downloads {
    pub fn start() -> io::Result<Downloads> {
        Hold::start(DOWNLOADS_PORT, |mut stream, held| {
            let slow = request_line(&mut stream).split(' ').nth(1) == Some("/slow");
            let (name, len) = if slow { ("slow", 1_000_000) } else { ("real", 4) };
            let head = format!(
                "HTTP/1.0 200 OK\r\nContent-Type: application/zip\r\nContent-Disposition: attachment; filename=\"{name}.zip\"\r\nContent-Length: {len}\r\n\r\n"
            );
            let body: &[u8] = if slow { &[b'x'; 300_000] } else { b"data" };
            if stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body)).and_then(|()| stream.flush()).is_err() {
                return;
            }
            if slow {
                held.lock().unwrap_or_else(PoisonError::into_inner).push(stream);
            }
        })
        .map(Downloads)
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}:{}{path}", crate::http::site_host(), self.0.addr.port())
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
            let _ = request_line(&mut stream);
            let reply: &[u8] = if mode.load(Ordering::SeqCst) {
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n"
            } else {
                b"HTTP/1.1 200 Connection established\r\n\r\n"
            };
            if stream.write_all(reply).is_ok() && !mode.load(Ordering::SeqCst) {
                held.lock().unwrap_or_else(PoisonError::into_inner).push(stream);
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

    fn get(url_port: u16, path: &str, read: usize) -> Vec<u8> {
        let mut s = TcpStream::connect((crate::http::site_host().as_str(), url_port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\n\r\n").unwrap();
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

    #[test]
    fn downloads_answer_real_in_full_and_slow_in_part_until_killed() {
        let server = Downloads::start().unwrap();
        let port = server.0.addr.port();
        let real = String::from_utf8_lossy(&get(port, "/real.zip", usize::MAX)).into_owned();
        assert!(real.contains("filename=\"real.zip\"") && real.ends_with("data"), "{real}");
        let slow = get(port, "/slow", 300_000);
        assert!(slow.len() > 300_000 && slow.len() < 1_000_000, "{}", slow.len());
        server.kill();
    }

    /// `kill` is a dead server, not one that still answers the engine's retry of the broken download.
    #[test]
    fn a_killed_download_server_refuses_the_retry() {
        let server = Downloads::start().unwrap();
        let addr = server.0.addr;
        server.kill();
        assert!(TcpStream::connect(addr).is_err(), "the killed server still accepts connections");
    }

    #[test]
    fn proxy_refuses_or_tunnels_by_mode() {
        let proxy = Proxy::start(true).unwrap();
        let port = proxy.hold.addr.port();
        assert!(String::from_utf8_lossy(&get(port, "x:443", usize::MAX)).starts_with("HTTP/1.1 502"));
        proxy.set_refuse(false);
        assert!(String::from_utf8_lossy(&get(port, "x:443", 10)).starts_with("HTTP/1.1 200"));
    }
}
