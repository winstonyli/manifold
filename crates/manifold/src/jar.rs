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
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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

/// Never prints `value`: a session cookie is a credential, and a stray `{:?}` must not leak it into a log.
impl std::fmt::Debug for Cookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cookie")
            .field("name", &self.name)
            .field("domain", &self.domain)
            .field("path", &self.path)
            .field("value", &format_args!("<redacted>"))
            .finish_non_exhaustive()
    }
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

/// First 4 bytes of a protected jar file. A file without them is a legacy plaintext jar.
const MAGIC: &[u8] = b"MJR1";

/// Error text only (serde's message can quote input, so reduce it to the position).
fn parse(bytes: &[u8]) -> Result<Jar, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON at line {} column {}", e.line(), e.column()))
}

/// Windows: DPAPI for the current user, no extra entropy.
#[cfg(windows)]
fn protect(plain: &[u8]) -> std::io::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: `input` borrows `plain` for the call; DPAPI only reads it.
    unsafe { CryptProtectData(&input, windows::core::w!("manifold jar"), None, None, None, 0, &mut out) }
        .map_err(std::io::Error::other)?;
    Ok(take_blob(out))
}

#[cfg(windows)]
fn unprotect(blob: &[u8]) -> std::io::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: as in `protect`.
    unsafe { CryptUnprotectData(&input, None, None, None, None, 0, &mut out) }.map_err(std::io::Error::other)?;
    Ok(take_blob(out))
}

/// Copies a DPAPI output blob into a `Vec` and frees it with `LocalFree`.
#[cfg(windows)]
fn take_blob(blob: windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Vec<u8> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    // SAFETY: DPAPI returned `cbData` bytes at `pbData`, allocated with LocalAlloc.
    unsafe {
        let v = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(blob.pbData.cast())));
        v
    }
}

/// Not Windows: no protection. Linux and macOS keyring protection is not in v1.
#[cfg(not(windows))]
fn protect(plain: &[u8]) -> std::io::Result<Vec<u8>> {
    Ok(plain.to_vec())
}

#[cfg(not(windows))]
fn unprotect(blob: &[u8]) -> std::io::Result<Vec<u8>> {
    Ok(blob.to_vec())
}

impl Jar {
    /// `<dir>\default.json`: v1 has one jar.
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("default.json")
    }

    /// The saved jar, or an empty one when the file is missing or unreadable (a corrupt jar must not stop the app).
    /// A file that exists but cannot be read (wrong user or machine, tampered, not JSON) is renamed to
    /// `default.json.unreadable` so the next save cannot overwrite it; only the error is logged, never contents.
    pub fn load(dir: &Path) -> Jar {
        let path = Self::path(dir);
        let Ok(bytes) = std::fs::read(&path) else { return Jar::default() };
        let parsed = match bytes.strip_prefix(MAGIC) {
            Some(blob) => unprotect(blob).map_err(|e| e.to_string()).and_then(|plain| parse(&plain)),
            None => parse(&bytes),
        };
        parsed.unwrap_or_else(|why| {
            let aside = dir.join("default.json.unreadable");
            match std::fs::rename(&path, &aside) {
                Ok(()) => eprintln!("jar: cannot read the jar file ({why}); kept it as {}", aside.display()),
                Err(e) => eprintln!("jar: cannot read the jar file ({why}) and could not move it aside: {e}"),
            }
            Jar::default()
        })
    }

    /// Writes beside the target and renames, so a crash mid-write leaves the old jar. The file is `MJR1` then the
    /// protected JSON.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("default.json.tmp");
        let mut out = MAGIC.to_vec();
        out.extend(protect(&serde_json::to_vec(self).map_err(std::io::Error::other)?)?);
        std::fs::write(&tmp, out)?;
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
        // A pulled cookie that is already expired is the engine deleting it: it removes the stored one too.
        let expired_keys: HashSet<Key> = pulled.iter().filter(|p| p.expired(t)).map(Cookie::key).collect();
        self.cookies.retain(|c| {
            let k = c.key();
            !expired_keys.contains(&k) && (pulled_keys.contains(&k) || !known.contains(&k))
        });
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
    fn an_expired_pulled_cookie_deletes_the_stored_one_with_its_key() {
        let mut jar = Jar { cookies: vec![c("sid", "1"), c("other", "2")] };
        let mut gone = c("sid", "1");
        gone.expires = Some(1.0);
        assert!(jar.merge(vec![gone], &HashSet::new()), "not even known: the engine's own expiry still wins");
        assert_eq!(jar.cookies, vec![c("other", "2")]);
    }

    #[test]
    fn debug_never_prints_a_cookie_value() {
        let jar = Jar { cookies: vec![c("sid", "TOP-SECRET")] };
        let text = format!("{jar:?} {:?}", c("sid", "TOP-SECRET"));
        assert!(!text.contains("TOP-SECRET") && text.contains("<redacted>") && text.contains("sid"), "{text}");
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
    fn a_missing_file_loads_as_an_empty_jar() {
        assert_eq!(Jar::load(&temp_dir("jar-missing")), Jar::default());
    }

    #[test]
    fn a_corrupt_file_loads_empty_and_is_moved_aside() {
        let dir = temp_dir("jar-corrupt");
        std::fs::write(Jar::path(&dir), "not json").unwrap();
        assert_eq!(Jar::load(&dir), Jar::default());
        assert!(!Jar::path(&dir).exists());
        assert_eq!(std::fs::read(dir.join("default.json.unreadable")).unwrap(), b"not json");
    }

    #[test]
    fn protect_and_unprotect_round_trip() {
        let plain = b"{\"cookies\":[]} some bytes";
        let blob = protect(plain).unwrap();
        assert_eq!(unprotect(&blob).unwrap(), plain);
    }

    #[test]
    fn a_saved_jar_is_protected_on_disk() {
        let dir = temp_dir("jar-protected");
        let jar = Jar { cookies: vec![c("sid", "SECRET123")] };
        jar.save(&dir).unwrap();
        let bytes = std::fs::read(Jar::path(&dir)).unwrap();
        assert!(bytes.starts_with(b"MJR1"));
        if cfg!(windows) {
            assert!(!String::from_utf8_lossy(&bytes).contains("SECRET123"), "the value must not appear on disk");
        }
        assert_eq!(Jar::load(&dir), jar);
    }

    #[test]
    fn a_legacy_plaintext_jar_loads_and_the_next_save_protects_it() {
        let dir = temp_dir("jar-legacy");
        let jar = Jar { cookies: vec![c("sid", "1")] };
        std::fs::write(Jar::path(&dir), serde_json::to_vec(&jar).unwrap()).unwrap();
        assert_eq!(Jar::load(&dir), jar);
        jar.save(&dir).unwrap();
        assert!(std::fs::read(Jar::path(&dir)).unwrap().starts_with(b"MJR1"));
    }

    #[test]
    fn a_tampered_protected_file_loads_empty_and_is_kept_aside() {
        let dir = temp_dir("jar-tamper");
        Jar { cookies: vec![c("sid", "1")] }.save(&dir).unwrap();
        let mut bytes = std::fs::read(Jar::path(&dir)).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(Jar::path(&dir), &bytes).unwrap();
        assert_eq!(Jar::load(&dir), Jar::default());
        assert_eq!(std::fs::read(dir.join("default.json.unreadable")).unwrap(), bytes);
        // A save after the failed load writes a fresh file and leaves the aside copy alone.
        Jar::default().save(&dir).unwrap();
        assert!(dir.join("default.json.unreadable").exists());
    }

    #[test]
    fn engine_json_parses_into_cookies() {
        let json = r#"[{"name":"a","value":"b","domain":"x.com","path":"/","expires":null,"secure":true,"http_only":false,"same_site":"Strict"}]"#;
        let cookies = from_json(json).unwrap();
        assert_eq!(cookies[0].same_site, SameSite::Strict);
        assert!(from_json("nope").is_none());
    }
}
