//! Conversions between manifold's jar file and the DevTools `Network` cookie shapes.

use serde_json::{json, Value};

/// The `Network.setCookies` cookie list from a JSON array of neutral cookies. Unreadable input seeds nothing. A cookie
/// without a name or domain is left out: one cookie CDP cannot build may fail the whole `setCookies` call.
pub fn cdp_cookies_from_jar(jar_json: &str) -> Vec<Value> {
    let list = serde_json::from_str::<Vec<Value>>(jar_json).unwrap_or_default();
    let usable = |c: &Value| {
        c["name"].as_str().is_some_and(|s| !s.is_empty())
            && c["domain"].as_str().is_some_and(|s| !s.trim_start_matches('.').is_empty())
    };
    list.iter()
        .filter(|c| usable(c))
        .map(|c| {
            let mut o = json!({
                "name": c["name"], "value": c["value"], "domain": c["domain"], "path": c["path"],
                "secure": c["secure"], "httpOnly": c["http_only"],
            });
            if let Some(s @ ("Strict" | "Lax" | "None")) = c["same_site"].as_str() {
                o["sameSite"] = s.into();
            }
            if !c["expires"].is_null() {
                o["expires"] = c["expires"].clone();
            }
            o
        })
        .collect()
}

/// `Network.setCookies` params for `cookies`.
pub fn set_cookies_params(cookies: &[Value]) -> String {
    json!({ "cookies": cookies }).to_string()
}

/// The neutral JSON array (`jar::Cookie` shape) from a `Network.getAllCookies` result. `None` when the result has no
/// `cookies` array (an error object, say): that is no read at all, and must not be reported as an empty jar.
pub fn jar_from_cdp(result: &Value) -> Option<String> {
    let neutral: Vec<Value> = result["cookies"]
        .as_array()?
        .iter()
        .map(|c| {
            json!({
                "name": c["name"], "value": c["value"], "domain": c["domain"], "path": c["path"],
                "expires": if c["session"].as_bool() == Some(true) { Value::Null } else { c["expires"].clone() },
                "secure": c["secure"], "http_only": c["httpOnly"],
                "same_site": c["sameSite"].as_str().unwrap_or("Unspecified"),
            })
        })
        .collect();
    Some(Value::Array(neutral).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_jar_becomes_cdp_set_cookies_params() {
        let jar = r#"[{"name":"sid","value":"1","domain":"a.com","path":"/","expires":4102444800.0,"secure":true,"http_only":true,"same_site":"Lax"},
                      {"name":"s","value":"2","domain":"a.com","path":"/","expires":null,"secure":false,"http_only":false,"same_site":"Unspecified"}]"#;
        let p: Value = serde_json::from_str(&set_cookies_params(&cdp_cookies_from_jar(jar))).unwrap();
        assert_eq!(p["cookies"][0]["httpOnly"], true);
        assert_eq!(p["cookies"][0]["sameSite"], "Lax");
        assert_eq!(p["cookies"][0]["expires"], 4102444800.0);
        assert!(p["cookies"][1].get("expires").is_none(), "a session cookie has no expiry");
        assert!(p["cookies"][1].get("sameSite").is_none(), "Unspecified is left to the engine");
    }

    #[test]
    fn cdp_cookies_become_neutral_json() {
        let r = json!({"cookies":[{"name":"sid","value":"1","domain":".a.com","path":"/","session":false,"expires":99.0,
                                   "secure":true,"httpOnly":true,"sameSite":"Strict"},
                                  {"name":"t","value":"2","domain":"b.com","path":"/","session":true,"expires":-1,"secure":false,"httpOnly":false}]});
        let out: serde_json::Value = serde_json::from_str(&jar_from_cdp(&r).unwrap()).unwrap();
        assert_eq!(out[0]["http_only"], true);
        assert_eq!(out[0]["same_site"], "Strict");
        assert_eq!(out[1]["expires"], serde_json::Value::Null);
        assert_eq!(out[1]["same_site"], "Unspecified");
    }

    #[test]
    fn an_unreadable_jar_seeds_nothing() {
        assert!(cdp_cookies_from_jar("garbage").is_empty());
    }

    #[test]
    fn a_cookie_without_a_name_or_domain_is_left_out_of_the_batch() {
        let jar = r#"[{"name":"","value":"1","domain":"a.com","path":"/"},{"name":"x","value":"1","domain":".","path":"/"},
                      {"name":"y","value":"1","path":"/"},{"name":"ok","value":"1","domain":"a.com","path":"/"}]"#;
        let list = cdp_cookies_from_jar(jar);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["name"], "ok");
    }

    #[test]
    fn an_error_or_odd_result_is_no_read_not_an_empty_jar() {
        assert_eq!(jar_from_cdp(&json!({"code": -32601, "message": "not found"})), None);
        assert_eq!(jar_from_cdp(&json!({"cookies": null})), None);
        assert_eq!(jar_from_cdp(&json!({"cookies": []})).as_deref(), Some("[]"), "a real empty store");
    }
}
