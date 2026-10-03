//! The cookie jar's seed and pull on the CEF side: neutral JSON (`jar::Cookie` shape) to and from CEF cookies.
//! Never logs cookie values.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::*;
use serde_json::{json, Value};

/// CEF never calls a cookie visitor when the store holds no cookies, so a pull with no callback by then is either
/// empty or slow (CEF queues the visit until its store is initialised). `JarPull::poll` decides which.
const EMPTY_PULL_TIMEOUT: Duration = Duration::from_secs(2);
/// Seconds between the Windows epoch (1601-01-01), which CEF's `Basetime` counts from, and the Unix epoch.
const WINDOWS_TO_UNIX_SECS: f64 = 11_644_473_600.0;

fn to_basetime(unix: f64) -> Basetime {
    Basetime { val: ((unix + WINDOWS_TO_UNIX_SECS) * 1_000_000.0) as i64 }
}

fn from_basetime(t: &Basetime) -> f64 {
    t.val as f64 / 1_000_000.0 - WINDOWS_TO_UNIX_SECS
}

/// A CEF cookie from one neutral cookie, with the URL `set_cookie` needs. `None` when it has no name or domain.
///
/// The neutral `domain` follows Chromium's own form: a leading dot is a domain cookie (sent to subdomains), no dot is
/// host-only. CEF hands a non-empty `domain` to Chromium, which prefixes a dot to it, so a host-only cookie is set
/// with an EMPTY domain and its host in the URL instead; that keeps it host-only and lets a `__Host-` cookie (which
/// must have no Domain attribute) be set at all. The URL is https for a `secure` cookie (`__Secure-` and `__Host-`
/// need it) and carries the cookie's path.
pub fn cookie_from_json(c: &Value) -> Option<(String, Cookie)> {
    let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
    let domain = s("domain");
    let host_only = !domain.starts_with('.');
    let host = domain.trim_start_matches('.');
    if s("name").is_empty() || host.is_empty() {
        return None;
    }
    let secure = c["secure"].as_bool().unwrap_or(false);
    let path = Some(s("path")).filter(|p| p.starts_with('/')).unwrap_or_else(|| "/".to_string());
    let url = format!("http{}://{host}{path}", if secure { "s" } else { "" });
    let expires = c["expires"].as_f64();
    let cookie = Cookie {
        name: s("name").as_str().into(),
        value: s("value").as_str().into(),
        domain: if host_only { "" } else { domain.as_str() }.into(),
        path: path.as_str().into(),
        secure: secure as _,
        httponly: c["http_only"].as_bool().unwrap_or(false) as _,
        creation: to_basetime(0.0),
        last_access: to_basetime(0.0),
        has_expires: expires.is_some() as _,
        expires: to_basetime(expires.unwrap_or(0.0)),
        same_site: match c["same_site"].as_str() {
            Some("Strict") => CookieSameSite::STRICT_MODE,
            Some("Lax") => CookieSameSite::LAX_MODE,
            Some("None") => CookieSameSite::NO_RESTRICTION,
            _ => CookieSameSite::UNSPECIFIED,
        },
        ..Default::default()
    };
    Some((url, cookie))
}

/// One neutral cookie from a CEF cookie. The domain needs no mapping: Chromium reports a host-only cookie by its bare
/// host and a domain cookie with its leading dot, which is the neutral form.
pub fn cookie_to_json(c: &Cookie) -> Value {
    let same_site = match c.same_site {
        CookieSameSite::STRICT_MODE => "Strict",
        CookieSameSite::LAX_MODE => "Lax",
        CookieSameSite::NO_RESTRICTION => "None",
        _ => "Unspecified",
    };
    json!({
        "name": c.name.to_string(),
        "value": c.value.to_string(),
        "domain": c.domain.to_string(),
        "path": c.path.to_string(),
        "expires": if c.has_expires != 0 { json!(from_basetime(&c.expires)) } else { Value::Null },
        "secure": c.secure != 0,
        "http_only": c.httponly != 0,
        "same_site": same_site,
    })
}

/// Writes the jar's cookies (a JSON array of neutral cookies) into the global cookie manager. Logs counts only.
/// Returns how many cookies the jar held, set or not: a host seeded with any may hold cookies from then on.
pub fn seed(jar_json: &str) -> usize {
    let list = serde_json::from_str::<Vec<Value>>(jar_json).unwrap_or_default();
    let Some(manager) = cookie_manager_get_global_manager(None) else {
        eprintln!("jar: no global cookie manager; seeded 0 of {}", list.len());
        return list.len();
    };
    // `set_cookie` reports only that the call was queued, not whether the cookie was accepted.
    let mut queued = 0;
    for (url, cookie) in list.iter().filter_map(cookie_from_json) {
        queued += manager.set_cookie(Some(&url.as_str().into()), Some(&cookie), None);
    }
    eprintln!("jar: queued {queued} of {} cookies", list.len());
    list.len()
}

/// Where a `GetCookies` pull stands (`JarPull::poll`).
#[derive(Debug, PartialEq)]
pub enum PullState {
    /// No answer yet: poll again.
    Pending,
    /// A complete read: send it as `Cookies`.
    Answer(String),
    /// No complete read and none coming: send nothing (manifold's pull times out). Never `[]`, which would read as
    /// "every cookie this tab held is gone".
    Silent,
}

/// A running `GetCookies` pull, filled by the visitor.
pub struct JarPull {
    started: Instant,
    seen: Rc<RefCell<Vec<Value>>>,
    done: Rc<RefCell<Option<String>>>,
    /// `visit_all_cookies` could not be started.
    failed: bool,
}

impl JarPull {
    /// The pull's state. `may_hold_cookies`: this host was seeded with cookies or has reported some, so a store that
    /// stays silent past `EMPTY_PULL_TIMEOUT` (or a read that never started) may be slow rather than empty.
    pub fn poll(&self, may_hold_cookies: bool) -> PullState {
        if let Some(json) = self.done.borrow_mut().take() {
            return PullState::Answer(json);
        }
        let timed_out = self.seen.borrow().is_empty() && self.started.elapsed() >= EMPTY_PULL_TIMEOUT;
        no_callback_answer(self.failed || timed_out, may_hold_cookies)
    }
}

/// What a pull whose visitor has not completed answers: nothing yet unless `given_up` (the read never started, or no
/// callback came in time); then `[]` only for a host that never had a cookie (a genuinely empty store), else silence.
fn no_callback_answer(given_up: bool, may_hold_cookies: bool) -> PullState {
    match (given_up, may_hold_cookies) {
        (false, _) => PullState::Pending,
        (true, false) => PullState::Answer("[]".to_string()),
        (true, true) => PullState::Silent,
    }
}

wrap_cookie_visitor! {
    struct JarVisitor {
        seen: Rc<RefCell<Vec<Value>>>,
        done: Rc<RefCell<Option<String>>>,
    }

    impl CookieVisitor {
        fn visit(
            &self,
            cookie: Option<&Cookie>,
            count: ::std::os::raw::c_int,
            total: ::std::os::raw::c_int,
            _delete_cookie: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            if let Some(cookie) = cookie {
                self.seen.borrow_mut().push(cookie_to_json(cookie));
            }
            if count + 1 >= total {
                *self.done.borrow_mut() = Some(Value::Array(self.seen.borrow().clone()).to_string());
            }
            1
        }
    }
}

/// Starts reading every cookie from the global manager.
pub fn start_pull() -> JarPull {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let done = Rc::new(RefCell::new(None));
    let mut visitor = JarVisitor::new(seen.clone(), done.clone());
    let started = cookie_manager_get_global_manager(None).map(|m| m.visit_all_cookies(Some(&mut visitor)));
    if started != Some(1) {
        eprintln!("jar: could not start the cookie read");
    }
    JarPull { started: Instant::now(), seen, done, failed: started != Some(1) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unix_seconds_round_trip_through_cef_basetime() {
        let t = to_basetime(1_700_000_000.0);
        assert!((from_basetime(&t) - 1_700_000_000.0).abs() < 0.001);
    }

    #[test]
    fn a_neutral_cookie_becomes_a_cef_cookie_and_back() {
        let j = json!({"name":"sid","value":"1","domain":"a.com","path":"/","expires":4102444800.0,"secure":true,"http_only":true,"same_site":"Strict"});
        let (url, cookie) = cookie_from_json(&j).unwrap();
        assert_eq!(url, "https://a.com/");
        assert_eq!(cookie.httponly, 1);
        let back = cookie_to_json(&cookie);
        assert_eq!(back["same_site"], "Strict");
        assert_eq!(back["http_only"], true);
        assert!((back["expires"].as_f64().unwrap() - 4102444800.0).abs() < 0.01);
    }

    fn neutral(name: &str, domain: &str, path: &str, secure: bool) -> Value {
        json!({"name":name,"value":"v","domain":domain,"path":path,"expires":null,"secure":secure,"http_only":false,"same_site":"Lax"})
    }

    #[test]
    fn a_host_only_cookie_is_set_with_an_empty_domain_and_its_host_in_the_url() {
        let (url, cookie) = cookie_from_json(&neutral("sid", "github.com", "/app", false)).unwrap();
        assert_eq!(cookie.domain.to_string(), "", "a non-empty domain would make Chromium widen it to .github.com");
        assert_eq!(url, "http://github.com/app");
        assert_eq!(cookie.path.to_string(), "/app");
    }

    #[test]
    fn a_domain_cookie_keeps_its_dot_and_its_url_drops_it() {
        let (url, cookie) = cookie_from_json(&neutral("s", ".a.com", "/", false)).unwrap();
        assert_eq!(cookie.domain.to_string(), ".a.com");
        assert_eq!(url, "http://a.com/");
    }

    #[test]
    fn a_host_prefixed_cookie_is_host_only_secure_at_the_root() {
        let (url, cookie) = cookie_from_json(&neutral("__Host-sid", "github.com", "/", true)).unwrap();
        assert_eq!(cookie.domain.to_string(), "", "__Host- forbids a Domain attribute");
        assert_eq!(url, "https://github.com/");
        assert_eq!((cookie.secure, cookie.path.to_string()), (1, "/".to_string()));
    }

    #[test]
    fn a_secure_prefixed_cookie_gets_an_https_url() {
        let (url, _) = cookie_from_json(&neutral("__Secure-id", ".a.com", "/x", true)).unwrap();
        assert_eq!(url, "https://a.com/x");
    }

    #[test]
    fn a_missing_or_relative_path_seeds_at_the_root() {
        let (url, cookie) = cookie_from_json(&json!({"name":"n","domain":"a.com","path":"x"})).unwrap();
        assert_eq!((url.as_str(), cookie.path.to_string().as_str()), ("http://a.com/", "/"));
    }

    #[test]
    fn a_read_back_domain_is_already_neutral() {
        // What CEF reports for a host-only and for a domain cookie: the bare host, and the dotted domain.
        for domain in ["github.com", ".github.com"] {
            let cookie = Cookie { name: "n".into(), domain: domain.into(), path: "/".into(), ..Default::default() };
            assert_eq!(cookie_to_json(&cookie)["domain"], domain);
        }
    }

    #[test]
    fn a_pull_without_a_complete_read_answers_empty_only_for_a_host_that_never_had_cookies() {
        assert_eq!(no_callback_answer(false, false), PullState::Pending);
        assert_eq!(no_callback_answer(false, true), PullState::Pending);
        assert_eq!(no_callback_answer(true, false), PullState::Answer("[]".into()), "a genuinely empty store");
        assert_eq!(no_callback_answer(true, true), PullState::Silent, "seeded: slow or failed, never []");
    }

    #[test]
    fn a_session_cookie_has_no_expiry() {
        let j = json!({"name":"s","value":"1","domain":".a.com","path":"/","expires":null,"secure":false,"http_only":false,"same_site":"Unspecified"});
        let (url, cookie) = cookie_from_json(&j).unwrap();
        assert_eq!(url, "http://a.com/");
        assert_eq!(cookie_to_json(&cookie)["expires"], serde_json::Value::Null);
    }

    #[test]
    fn a_cookie_without_a_name_or_domain_is_skipped() {
        assert!(cookie_from_json(&json!({"name":"","domain":"a.com"})).is_none());
        assert!(cookie_from_json(&json!({"name":"x","domain":"."})).is_none());
    }

    #[test]
    fn every_same_site_value_round_trips() {
        for s in ["Strict", "Lax", "None", "Unspecified"] {
            let (_, c) = cookie_from_json(&json!({"name":"n","domain":"a.com","same_site":s})).unwrap();
            assert_eq!(cookie_to_json(&c)["same_site"], s);
        }
    }
}
