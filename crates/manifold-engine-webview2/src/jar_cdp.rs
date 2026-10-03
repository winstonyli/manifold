//! Conversions between manifold's jar file and the DevTools `Network` cookie shapes.

use serde_json::{json, Value};

/// `Network.setCookies` params from a JSON array of neutral cookies. Unreadable input seeds nothing.
pub fn cdp_cookies_from_jar(jar_json: &str) -> Value {
    let list = serde_json::from_str::<Vec<Value>>(jar_json).unwrap_or_default();
    let cookies: Vec<Value> = list
        .iter()
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
        .collect();
    json!({ "cookies": cookies })
}

/// The neutral JSON array (`jar::Cookie` shape) from a `Network.getAllCookies` result.
pub fn jar_from_cdp(result: &Value) -> String {
    let list = result["cookies"].as_array().cloned().unwrap_or_default();
    let neutral: Vec<Value> = list
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
    Value::Array(neutral).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_jar_becomes_cdp_set_cookies_params() {
        let jar = r#"[{"name":"sid","value":"1","domain":"a.com","path":"/","expires":4102444800.0,"secure":true,"http_only":true,"same_site":"Lax"},
                      {"name":"s","value":"2","domain":"a.com","path":"/","expires":null,"secure":false,"http_only":false,"same_site":"Unspecified"}]"#;
        let p = cdp_cookies_from_jar(jar);
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
        let out: serde_json::Value = serde_json::from_str(&jar_from_cdp(&r)).unwrap();
        assert_eq!(out[0]["http_only"], true);
        assert_eq!(out[0]["same_site"], "Strict");
        assert_eq!(out[1]["expires"], serde_json::Value::Null);
        assert_eq!(out[1]["same_site"], "Unspecified");
    }

    #[test]
    fn an_unreadable_jar_seeds_nothing() {
        assert_eq!(cdp_cookies_from_jar("garbage")["cookies"], json!([]));
    }
}
