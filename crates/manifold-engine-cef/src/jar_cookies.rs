//! The cookie jar's seed and pull on the CEF side: neutral JSON (`jar::Cookie` shape) to and from CEF cookies.
//! Never logs cookie values.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::*;
use serde_json::{json, Value};

/// CEF never calls a cookie visitor when the store holds no cookies, so a pull with no callback by then is empty.
const EMPTY_PULL_TIMEOUT: Duration = Duration::from_millis(500);
/// Seconds between the Windows epoch (1601-01-01), which CEF's `Basetime` counts from, and the Unix epoch.
const WINDOWS_TO_UNIX_SECS: f64 = 11_644_473_600.0;

fn to_basetime(unix: f64) -> Basetime {
    Basetime { val: ((unix + WINDOWS_TO_UNIX_SECS) * 1_000_000.0) as i64 }
}

fn from_basetime(t: &Basetime) -> f64 {
    t.val as f64 / 1_000_000.0 - WINDOWS_TO_UNIX_SECS
}

/// A CEF cookie from one neutral cookie, with the URL `set_cookie` needs. `None` when it has no name or domain.
pub fn cookie_from_json(c: &Value) -> Option<(String, Cookie)> {
    let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
    let domain = s("domain");
    let host = domain.trim_start_matches('.');
    if s("name").is_empty() || host.is_empty() {
        return None;
    }
    let secure = c["secure"].as_bool().unwrap_or(false);
    let url = format!("http{}://{host}/", if secure { "s" } else { "" });
    let expires = c["expires"].as_f64();
    let cookie = Cookie {
        name: s("name").as_str().into(),
        value: s("value").as_str().into(),
        domain: domain.as_str().into(),
        path: s("path").as_str().into(),
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

/// One neutral cookie from a CEF cookie.
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
pub fn seed(jar_json: &str) {
    let list = serde_json::from_str::<Vec<Value>>(jar_json).unwrap_or_default();
    let Some(manager) = cookie_manager_get_global_manager(None) else {
        eprintln!("jar: no global cookie manager; seeded 0 of {}", list.len());
        return;
    };
    let mut ok = 0;
    for (url, cookie) in list.iter().filter_map(cookie_from_json) {
        ok += manager.set_cookie(Some(&url.as_str().into()), Some(&cookie), None);
    }
    eprintln!("jar: seeded {ok} of {} cookies", list.len());
}

/// A running `GetCookies` pull: filled by the visitor, or answered empty after `EMPTY_PULL_TIMEOUT`.
pub struct JarPull {
    started: Instant,
    seen: Rc<RefCell<Vec<Value>>>,
    done: Rc<RefCell<Option<String>>>,
}

impl JarPull {
    /// The answer (a JSON array string) once it is ready, else `None`.
    pub fn take(&self) -> Option<String> {
        if let Some(json) = self.done.borrow_mut().take() {
            return Some(json);
        }
        (self.seen.borrow().is_empty() && self.started.elapsed() >= EMPTY_PULL_TIMEOUT).then(|| "[]".to_string())
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
    let pull = JarPull { started: Instant::now(), seen: seen.clone(), done: done.clone() };
    let mut visitor = JarVisitor::new(seen, done.clone());
    let started = cookie_manager_get_global_manager(None).map(|m| m.visit_all_cookies(Some(&mut visitor)));
    if started != Some(1) {
        eprintln!("jar: could not start the cookie read");
        *done.borrow_mut() = Some("[]".to_string());
    }
    pull
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
