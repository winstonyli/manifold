//! The cookie jar manifold owns. Engines are seeded from it at tab start and pulled back into it, so a login
//! carries between engines (spec docs/superpowers/specs/2026-10-02-jars-design.md). Pure: no egui, no `App`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SameSite {
    Strict,
    Lax,
    None,
    Unspecified,
}

/// The neutral cookie every engine's cookies convert to and from. `expires` is Unix seconds, `None` for a session cookie.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

/// `(name, domain, path)`: what identifies a cookie.
pub type Key = (String, String, String);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Jar {
    pub cookies: Vec<Cookie>,
}

impl Cookie {
    fn key(&self) -> Key {
        (self.name.clone(), self.domain.clone(), self.path.clone())
    }

    fn expired(&self, now: f64) -> bool {
        self.expires.is_some_and(|e| e <= now)
    }
}

/// An engine's cookie dump, as its host serialised it. `None` if it is not that shape.
pub fn from_json(json: &str) -> Option<Vec<Cookie>> {
    serde_json::from_str(json).ok()
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl Jar {
    /// `<dir>\default.json`: v1 has one jar.
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("default.json")
    }

    /// The saved jar, or an empty one when the file is missing or unreadable (a corrupt jar must not stop the app).
    pub fn load(dir: &Path) -> Jar {
        std::fs::read_to_string(Self::path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Writes beside the target and renames, so a crash mid-write leaves the old jar.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("default.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, Self::path(dir))
    }

    pub fn keys(&self) -> HashSet<Key> {
        self.cookies.iter().map(Cookie::key).collect()
    }

    /// Folds an engine's dump into the jar (rule in the plan's Task 1). Returns whether the jar changed.
    pub fn merge(&mut self, pulled: Vec<Cookie>, known: &HashSet<Key>) -> bool {
        let before = self.cookies.clone();
        let t = now();
        let pulled_keys: HashSet<Key> = pulled.iter().map(Cookie::key).collect();
        self.cookies.retain(|c| pulled_keys.contains(&c.key()) || !known.contains(&c.key()));
        for p in pulled.into_iter().filter(|p| !p.expired(t)) {
            match self.cookies.iter_mut().find(|c| c.key() == p.key()) {
                Some(slot) => *slot = p,
                None => self.cookies.push(p),
            }
        }
        self.cookies.retain(|c| !c.expired(t));
        self.cookies != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;

    fn c(name: &str, value: &str) -> Cookie {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: "example.com".into(),
            path: "/".into(),
            expires: None,
            secure: false,
            http_only: true,
            same_site: SameSite::Lax,
        }
    }

    #[test]
    fn a_pulled_cookie_replaces_the_stored_one_with_the_same_key() {
        let mut jar = Jar { cookies: vec![c("sid", "old")] };
        assert!(jar.merge(vec![c("sid", "new")], &jar.keys()));
        assert_eq!(jar.cookies, vec![c("sid", "new")]);
    }

    #[test]
    fn a_cookie_missing_from_the_pull_is_deleted_only_if_it_was_known() {
        let mut jar = Jar { cookies: vec![c("sid", "1"), c("fresh", "2")] };
        let known: HashSet<Key> = [("sid".into(), "example.com".into(), "/".into())].into();
        assert!(jar.merge(vec![], &known));
        assert_eq!(jar.cookies, vec![c("fresh", "2")], "sid was a logout; fresh was never seen by the engine");
    }

    #[test]
    fn merging_the_same_cookies_changes_nothing() {
        let mut jar = Jar { cookies: vec![c("sid", "1")] };
        assert!(!jar.merge(vec![c("sid", "1")], &jar.keys()));
    }

    #[test]
    fn an_expired_cookie_is_dropped_on_merge() {
        let mut gone = c("old", "x");
        gone.expires = Some(1.0);
        let mut jar = Jar::default();
        assert!(!jar.merge(vec![gone], &HashSet::new()), "nothing was added, so nothing changed");
        assert!(jar.cookies.is_empty());
    }

    #[test]
    fn the_jar_round_trips_through_its_file() {
        let dir = temp_dir("jar-roundtrip");
        let jar = Jar { cookies: vec![c("sid", "1")] };
        jar.save(&dir).unwrap();
        assert_eq!(Jar::load(&dir), jar);
    }

    #[test]
    fn a_missing_or_corrupt_file_loads_as_an_empty_jar() {
        let dir = temp_dir("jar-corrupt");
        assert_eq!(Jar::load(&dir), Jar::default());
        std::fs::write(Jar::path(&dir), "not json").unwrap();
        assert_eq!(Jar::load(&dir), Jar::default());
    }

    #[test]
    fn engine_json_parses_into_cookies() {
        let json = r#"[{"name":"a","value":"b","domain":"x.com","path":"/","expires":null,"secure":true,"http_only":false,"same_site":"Strict"}]"#;
        let cookies = from_json(json).unwrap();
        assert_eq!(cookies[0].same_site, SameSite::Strict);
        assert!(from_json("nope").is_none());
    }
}
