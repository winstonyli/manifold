//! The two-instance test's local site (was scripts/nonce_server.py), served in process so the test reads its counters
//! directly instead of from files. `/r` redirects to a fresh `/r?n=N` on every load and serves each issued `n` once,
//! like a login nonce consumed on use: another instance loading a url one instance shared asks for a served `n` again
//! (`r_repeats`). `/link` is one window-sized link to `/r`; `/js` navigates by script, 500 ms after load, to a fresh
//! `/dest?n=` (`dests`, `dest_repeats`); `/spa` is a single-page app whose click pushes `/spa/c?n=` and whose timer
//! pushes `/spa/t?n=` a second later. Neither push asks the server for anything; only a load of a shared url does
//! (`spa_clicks`, `spa_timers`).

use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::http::{self, response};
use crate::suggest::{query_param, url_query};

const LINK: &str =
    "<!doctype html><title>link</title>\n<a href=\"/r\" style=\"position:fixed;inset:0;display:block\">go</a>";
const JS: &str = "<!doctype html><title>js</title>\n<script>setTimeout(function () { location.href = \"/dest?n=\" + Math.random(); }, 500);</script>";
const SPA: &str = "<!doctype html><title>spa</title>\n<div style=\"position:fixed;inset:0\">spa</div>\n<script>onclick = function () {\n  history.pushState({}, \"\", \"/spa/c?n=\" + Math.random());\n  setTimeout(function () { history.pushState({}, \"\", \"/spa/t?n=\" + Math.random()); }, 1000);\n};</script>";
const DEST: &str = "<!doctype html><title>dest</title>";
const ONCE: &str = "<!doctype html><title>nonce</title><p>served once";

#[derive(Default)]
pub struct NonceState {
    next: u64,
    issued: HashSet<String>,
    served: HashSet<String>,
    hits: u32,
    r_repeats: u32,
    dests: HashSet<Option<String>>,
    dest_repeats: u32,
    spa: [u32; 2],
    /// Every GET, as (time since the site started, raw request target).
    log: Vec<(Duration, String)>,
}

impl NonceState {
    /// The response to one request line; `now` is the time since the site started, for the log.
    pub fn respond(&mut self, request_line: &str, now: Duration) -> Vec<u8> {
        let mut parts = request_line.split(' ');
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if method != "GET" {
            return response("501 Unsupported method", &[], "text/html", b"");
        }
        self.log.push((now, target.to_string()));
        let path = target.split(['?', '#']).next().unwrap_or("");
        let n = query_param(url_query(target), "n", false);
        let page = |body: &str| response("200 OK", &[], "text/html", body.as_bytes());
        match path {
            "/link" => page(LINK),
            "/js" => page(JS),
            "/spa" => page(SPA),
            "/spa/c" => {
                self.spa[0] += 1;
                page(SPA)
            }
            "/spa/t" => {
                self.spa[1] += 1;
                page(SPA)
            }
            "/dest" => {
                if self.dests.contains(&n) {
                    self.dest_repeats += 1;
                }
                self.dests.insert(n);
                page(DEST)
            }
            "/r" => {
                self.hits += 1;
                let was_served = n.as_ref().is_some_and(|n| self.served.contains(n));
                if was_served {
                    self.r_repeats += 1;
                }
                match n {
                    Some(n) if self.issued.contains(&n) && !was_served => {
                        self.served.insert(n);
                        page(ONCE)
                    }
                    _ => {
                        self.next += 1;
                        self.issued.insert(self.next.to_string());
                        let to = format!("/r?n={}", self.next);
                        response("302 Found", &[("Location", &to)], "text/html", b"")
                    }
                }
            }
            _ => response("404 Not Found", &[], "text/html", b""),
        }
    }
}

/// A handle on the site's state, shared with its server threads.
#[derive(Clone, Default)]
pub struct Counters(Arc<Mutex<NonceState>>);

impl Counters {
    fn lock(&self) -> MutexGuard<'_, NonceState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Requests for `/r` (python's `<hits-file>`).
    pub fn hits(&self) -> u32 {
        self.lock().hits
    }
    /// An already-served `/r?n=` asked for again (`.repeats` field 1).
    pub fn r_repeats(&self) -> u32 {
        self.lock().r_repeats
    }
    /// An already-seen `/dest?n=` asked for again (field 2).
    pub fn dest_repeats(&self) -> u32 {
        self.lock().dest_repeats
    }
    /// Distinct `/dest` urls asked for (field 3).
    pub fn dests(&self) -> usize {
        self.lock().dests.len()
    }
    /// `/spa/c` and `/spa/t` loads (fields 4 and 5).
    pub fn spa_clicks(&self) -> u32 {
        self.lock().spa[0]
    }
    pub fn spa_timers(&self) -> u32 {
        self.lock().spa[1]
    }
    /// GETs whose raw target is exactly `target` (the script's `grep -c " $1\$" hits.log`).
    pub fn served(&self, target: &str) -> usize {
        self.lock().log.iter().filter(|(_, t)| t == target).count()
    }
    /// The request log as `hits.log` had it ("<seconds> <target>" per line) and the `.repeats` line, for failures.
    pub fn report(&self) -> String {
        let s = self.lock();
        let mut out: Vec<String> = s.log.iter().map(|(t, p)| format!("{:.2} {p}", t.as_secs_f64())).collect();
        out.push(format!(
            "repeats: r {} dest {} dests {} spa_c {} spa_t {}",
            s.r_repeats,
            s.dest_repeats,
            s.dests.len(),
            s.spa[0],
            s.spa[1]
        ));
        out.join("\n")
    }
}

/// The running site. Dropping it stops the server.
pub struct Site {
    server: http::Server,
    counters: Counters,
}

impl Site {
    pub fn start() -> io::Result<Site> {
        let counters = Counters::default();
        let c = counters.clone();
        let t0 = Instant::now();
        let server = http::Server::start(Arc::new(move |line: &str| c.lock().respond(line, t0.elapsed())))?;
        Ok(Site { server, counters })
    }
    pub fn port(&self) -> u16 {
        self.server.port()
    }
    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::*;

    fn get(s: &mut NonceState, target: &str) -> String {
        String::from_utf8(s.respond(&format!("GET {target} HTTP/1.1"), Duration::ZERO)).unwrap()
    }

    #[test]
    fn r_redirects_then_serves_each_issued_n_once() {
        let mut s = NonceState::default();
        let r = get(&mut s, "/r");
        assert!(r.starts_with("HTTP/1.0 302"), "{r}");
        assert!(r.contains("Location: /r?n=1\r\n"), "{r}");
        assert_eq!(s.hits, 1);
        let r = get(&mut s, "/r?n=1");
        assert!(r.starts_with("HTTP/1.0 200") && r.ends_with("served once"), "{r}");
        assert_eq!(s.r_repeats, 0);
        let r = get(&mut s, "/r?n=1");
        assert!(r.starts_with("HTTP/1.0 302") && r.contains("Location: /r?n=2\r\n"), "{r}");
        assert_eq!(s.r_repeats, 1);
        let r = get(&mut s, "/r?n=99");
        assert!(r.contains("Location: /r?n=3\r\n"), "{r}");
        assert_eq!(s.r_repeats, 1);
    }

    #[test]
    fn blank_n_is_no_n() {
        let mut s = NonceState::default();
        assert!(get(&mut s, "/r?n=").starts_with("HTTP/1.0 302"));
        assert_eq!(s.r_repeats, 0);
    }

    #[test]
    fn dest_counts_distinct_urls_and_repeats() {
        let mut s = NonceState::default();
        for t in ["/dest?n=a", "/dest?n=a", "/dest?n=b"] {
            assert!(get(&mut s, t).starts_with("HTTP/1.0 200"));
        }
        assert_eq!((s.dests.len(), s.dest_repeats), (2, 1));
        let mut s = NonceState::default();
        get(&mut s, "/dest");
        assert_eq!((s.dests.len(), s.dest_repeats), (1, 0));
        get(&mut s, "/dest");
        assert_eq!((s.dests.len(), s.dest_repeats), (1, 1));
    }

    #[test]
    fn spa_pushes_count_separately() {
        let mut s = NonceState::default();
        assert!(get(&mut s, "/spa").ends_with("};</script>"));
        assert_eq!(s.spa, [0, 0]);
        assert!(get(&mut s, "/spa/c?n=1").ends_with("};</script>"));
        assert_eq!(s.spa, [1, 0]);
        assert!(get(&mut s, "/spa/t?n=1").ends_with("};</script>"));
        assert_eq!(s.spa, [1, 1]);
    }

    #[test]
    fn unknown_paths_404_and_are_logged_exactly() {
        let c = Counters::default();
        let mut s = c.lock();
        assert!(get(&mut s, "/favicon.ico").starts_with("HTTP/1.0 404"));
        get(&mut s, "/js?x=1");
        drop(s);
        assert_eq!(c.served("/favicon.ico"), 1);
        assert_eq!(c.served("/js"), 0);
        assert_eq!(c.served("/js?x=1"), 1);
        assert!(c.report().contains("/favicon.ico"));
    }

    #[test]
    fn non_get_is_501_and_not_logged() {
        let mut s = NonceState::default();
        let r = String::from_utf8(s.respond("POST /r HTTP/1.1", Duration::ZERO)).unwrap();
        assert!(r.starts_with("HTTP/1.0 501"), "{r}");
        assert!(s.log.is_empty());
        assert_eq!(s.hits, 0);
    }

    #[test]
    fn serves_over_a_socket() {
        let site = Site::start().unwrap();
        let ask = |target: &str| {
            let mut c = TcpStream::connect(("127.0.0.1", site.port())).unwrap();
            c.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).unwrap();
            let mut got = String::new();
            c.read_to_string(&mut got).unwrap();
            got
        };
        assert!(ask("/r").starts_with("HTTP/1.0 302"));
        assert!(ask("/r?n=1").starts_with("HTTP/1.0 200"));
        assert_eq!(site.counters().hits(), 2);
    }
}
