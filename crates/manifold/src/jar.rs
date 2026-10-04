//! The cookie jar manifold owns. Engines are seeded from it at tab start and pulled back into it, so a login
//! carries between engines (spec docs/superpowers/specs/2026-10-02-jars-design.md). Pure: no egui, no `App`; it names jars by store `NodeId`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::model::NodeId;

/// What a tab's `jar` / `jar_set` fields hold (spec "Data model").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JarChoice {
    /// A `Kind::Jar` node; `None` is the default jar.
    pub jar: Option<NodeId>,
    /// Picked by hand: a group-default cascade leaves it alone.
    pub set: bool,
}

/// Which jar a running host merges into, and which file it saves (spec "Resolution").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JarKey {
    #[default]
    Default,
    Named(NodeId),
}

impl JarKey {
    /// `default` or the node id (`{actor:016x}-{counter}`): safe as a file name on every OS.
    pub fn file_stem(&self) -> String {
        match self {
            JarKey::Default => "default".to_string(),
            JarKey::Named(id) => id.to_string(),
        }
    }

    fn file_name(&self) -> String {
        format!("{}.json", self.file_stem())
    }
}

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
    /// Set when a jar file exists that could not be read or moved aside: the cookies work for this session, but
    /// `save` writes nothing, so that file is never overwritten.
    #[serde(skip)]
    locked: bool,
}

impl Cookie {
    fn key(&self) -> Key {
        (self.name.clone(), self.domain.clone(), self.path.clone())
    }

    fn expired(&self, now: f64) -> bool {
        self.expires.is_some_and(|e| e <= now)
    }

    /// Equal, with expiries within `EXPIRES_TOLERANCE`: an engine's float conversion of the same expiry is not a
    /// change.
    fn same_as(&self, other: &Cookie) -> bool {
        // Destructured, so a new field is a compile error here rather than a silent miss.
        let Cookie { name, value, domain, path, expires, secure, http_only, same_site } = self;
        let expires_same = match (*expires, other.expires) {
            (Some(a), Some(b)) => (a - b).abs() < EXPIRES_TOLERANCE,
            (a, b) => a == b,
        };
        expires_same
            && *name == other.name
            && *value == other.value
            && *domain == other.domain
            && *path == other.path
            && *secure == other.secure
            && *http_only == other.http_only
            && *same_site == other.same_site
    }
}

/// How far apart two expiries (seconds) may be and still count as one. Engines report microseconds; a real change
/// (a new `Max-Age` or `Expires`) moves an expiry by far more.
const EXPIRES_TOLERANCE: f64 = 0.001;

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

/// Windows: DPAPI for the current user, no extra entropy (so any process of the same Windows user can decrypt; the
/// accepted v1 threat model is other users and offline copies). `CRYPTPROTECT_UI_FORBIDDEN`: never raise UI.
#[cfg(windows)]
fn protect(plain: &[u8]) -> std::io::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plain.len()).map_err(std::io::Error::other)?,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: `input` borrows `plain` for the call; DPAPI only reads it.
    unsafe {
        CryptProtectData(
            &input,
            windows::core::w!("manifold jar"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    }
    .map_err(std::io::Error::other)?;
    Ok(take_blob(out))
}

#[cfg(windows)]
fn unprotect(blob: &[u8]) -> std::io::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(blob.len()).map_err(std::io::Error::other)?,
        pbData: blob.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: as in `protect`.
    unsafe { CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }
        .map_err(std::io::Error::other)?;
    Ok(take_blob(out))
}

/// Copies a DPAPI output blob into a `Vec` and frees it with `LocalFree`.
#[cfg(windows)]
fn take_blob(blob: windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Vec<u8> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    // SAFETY: on success DPAPI returned `cbData` bytes at `pbData`, allocated with LocalAlloc. A null pointer or
    // zero length is treated as empty rather than passed to `from_raw_parts`.
    unsafe {
        let v = if blob.pbData.is_null() || blob.cbData == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec()
        };
        // The result is ignored on purpose: a failed free only leaks this one blob and there is no recovery.
        let _ = LocalFree(Some(HLOCAL(blob.pbData.cast())));
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

/// Whether process `pid` is running. When unsure, yes: a live instance's save in progress must not be deleted.
#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    // SAFETY: plain Win32 calls on a handle this function opens and closes.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            // No such process is ERROR_INVALID_PARAMETER; a process we may not open still exists.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let running = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
        CloseHandle(h);
        running
    }
}

#[cfg(not(windows))]
fn process_alive(pid: u32) -> bool {
    let proc = Path::new("/proc");
    !proc.is_dir() || proc.join(pid.to_string()).exists()
}

/// Deletes the `<file>.<pid>[-<n>].tmp` files in `dir` (`file` is a jar's file name, e.g. `default.json`) whose process
/// is gone: a save interrupted by a crash, each a full copy of the jar. A live process's file is its save in progress
/// and is left alone.
fn remove_dead_tmps(dir: &Path, file: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) =
            name.to_str().and_then(|n| crate::store::atomic::temp_pid(n.strip_prefix(file)?.strip_prefix('.')?))
        else {
            continue;
        };
        if !process_alive(pid) {
            match std::fs::remove_file(e.path()) {
                Ok(()) => eprintln!("jar: deleted a temp file left by a crashed save (pid {pid})"),
                Err(e) => eprintln!("jar: could not delete a crashed save's temp file (pid {pid}): {e}"),
            }
        }
    }
}

/// How many `<file>.unreadable-*` files are kept per jar. Each holds credentials, so older ones are deleted.
const KEEP_ASIDES: usize = 5;

/// The `<file>.unreadable-<secs>[-<n>]` files in `dir` with their `(secs, n)` (`n` is 0 when absent), which orders
/// them oldest to newest.
fn asides_in(dir: &Path, file: &str) -> Vec<((u64, u64), PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            let rest = name.strip_prefix(file)?.strip_prefix(".unreadable-")?;
            let (secs, n) = rest.split_once('-').unwrap_or((rest, "0"));
            Some(((secs.parse().ok()?, n.parse().ok()?), e.path()))
        })
        .collect()
}

/// Deletes all but the `KEEP_ASIDES` newest unreadable copies of `file` in `dir`.
fn prune_asides(dir: &Path, file: &str) {
    let mut asides = asides_in(dir, file);
    asides.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, old) in asides.into_iter().skip(KEEP_ASIDES) {
        if let Err(e) = std::fs::remove_file(&old) {
            eprintln!("jar: could not delete an old unreadable jar file: {e}");
        }
    }
}

/// The named jar a file in the jars folder belongs to: `<id>.json`, `<id>.json.unreadable-…` or
/// `<id>.json.<pid>[-<n>].tmp`, with `<id>` spelled exactly as a node id prints. `None` for any other name,
/// `default.json` and its copies included.
fn named_jar_of(file: &str) -> Option<NodeId> {
    let (stem, rest) = file.split_once(".json")?;
    let id: NodeId = stem.parse().ok().filter(|id: &NodeId| id.to_string() == stem)?;
    let tmp = rest.strip_prefix('.').and_then(crate::store::atomic::temp_pid);
    let ours = rest.is_empty() || rest.starts_with(".unreadable-") || tmp.is_some();
    ours.then_some(id)
}

/// Deletes the files (`Jar::delete_files`) of every named jar in `dir` that `keep` does not name: jars deleted while
/// this device was not running (final review I2b). Touches only names `named_jar_of` reads, so never the default jar's.
/// Returns how many jars it found to remove.
pub fn prune_orphans(dir: &Path, keep: &BTreeSet<NodeId>) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let orphans: BTreeSet<NodeId> = entries
        .flatten()
        .filter_map(|e| named_jar_of(e.file_name().to_str()?))
        .filter(|id| !keep.contains(id))
        .collect();
    for &id in &orphans {
        let key = JarKey::Named(id);
        match Jar::delete_files(dir, key) {
            Ok(()) => eprintln!("jar: deleted the files of jar {}, which no longer exists", key.file_stem()),
            Err(e) => eprintln!("jar: could not delete the files of deleted jar {}: {e}", key.file_stem()),
        }
    }
    orphans.len()
}

/// `<file>.unreadable-<unix seconds>`, or, when that second already has one, `-<n>` past its highest `n`, so a new
/// name always sorts newest (a pruned name is never reused for a newer file).
fn unique_aside(dir: &Path, file: &str) -> PathBuf {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut n =
        asides_in(dir, file).iter().filter(|((s, _), _)| *s == secs).map(|((_, n), _)| n + 1).max().unwrap_or(0);
    loop {
        let name = if n == 0 { format!("{file}.unreadable-{secs}") } else { format!("{file}.unreadable-{secs}-{n}") };
        let aside = dir.join(name);
        if !aside.exists() {
            return aside;
        }
        n += 1;
    }
}

impl Jar {
    /// `<dir>\<key>.json`; `default.json` for the default jar, `<node id>.json` for a named one.
    pub fn path(dir: &Path, key: JarKey) -> PathBuf {
        dir.join(key.file_name())
    }

    /// Removes every file holding key `key`'s cookies (a deleted or newly ephemeral jar): its file, its
    /// `.unreadable-*` copies and the `.<pid>-<n>.tmp` files of crashed saves, each a full copy of its credentials. A live
    /// process's temp file is its save in progress and stays (`remove_dead_tmps`). A file already gone is fine; the
    /// first other failure is returned after trying the rest.
    pub fn delete_files(dir: &Path, key: JarKey) -> std::io::Result<()> {
        let file = key.file_name();
        remove_dead_tmps(dir, &file);
        let paths = std::iter::once(Self::path(dir, key)).chain(asides_in(dir, &file).into_iter().map(|(_, p)| p));
        let mut first = Ok(());
        for path in paths {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound && first.is_ok() => first = Err(e),
                _ => {}
            }
        }
        first
    }

    /// Key `key`'s saved jar, or an empty one when its file is missing. A file that exists but cannot be read (wrong
    /// user or machine, tampered, truncated, not JSON) is renamed to `<file>.unreadable-<unix seconds>` (never over
    /// an earlier one; only the `KEEP_ASIDES` newest are kept). If it cannot be read at all, or cannot be moved
    /// aside, the jar is `locked`: usable in memory, but `save` writes nothing, so the file is never overwritten.
    /// Only reasons are logged, never contents. A `<file>.<pid>-<n>.tmp` is deleted only when process `pid` is gone:
    /// a live one may be another running instance's save in progress. Each jar's helpers touch only its own files.
    pub fn load(dir: &Path, key: JarKey) -> Jar {
        let file = key.file_name();
        remove_dead_tmps(dir, &file);
        let path = Self::path(dir, key);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Jar::default(),
            Err(e) => return Self::locked_empty(&format!("cannot read the jar file ({e})")),
        };
        let parsed = match bytes.strip_prefix(MAGIC) {
            Some(blob) => unprotect(blob).map_err(|e| e.to_string()).and_then(|plain| parse(&plain)),
            None => parse(&bytes),
        };
        match parsed {
            Ok(jar) => jar,
            Err(why) => {
                let aside = unique_aside(dir, &file);
                match std::fs::rename(&path, &aside) {
                    Ok(()) => {
                        eprintln!("jar: cannot read the jar file ({why}); kept it as {}", aside.display());
                        prune_asides(dir, &file);
                        Jar::default()
                    }
                    Err(e) => {
                        Self::locked_empty(&format!("cannot read the jar file ({why}) and cannot move it aside ({e})"))
                    }
                }
            }
        }
    }

    /// Whether this jar's file exists but could not be read or moved aside (`load`): it is never saved over.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    fn locked_empty(why: &str) -> Jar {
        eprintln!("jar: {why}; the file is left untouched and cookie changes will not be saved this session");
        Jar { locked: true, ..Jar::default() }
    }

    /// Writes beside the target, fsyncs and renames (`store::atomic`), so a crash or power loss mid-write leaves the
    /// old jar. The temp file is new on every call and carries the process id, so two saves at once do not write into
    /// one file, and it is removed when the write or rename fails, so no stray copy of the credentials is left. (They
    /// still overwrite each other's jar: two instances on one profile are not supported in v1.) The file is `MJR1` then
    /// the protected JSON. A locked jar writes nothing (`load` already logged why).
    pub fn save(&self, dir: &Path, key: JarKey) -> std::io::Result<()> {
        if self.locked {
            return Ok(());
        }
        std::fs::create_dir_all(dir)?;
        let mut out = MAGIC.to_vec();
        out.extend(protect(&serde_json::to_vec(self).map_err(std::io::Error::other)?)?);
        crate::store::atomic::replace_private(&Self::path(dir, key), &out)
    }

    /// Folds tab `tab`'s cookie dump into the jar and updates what the tab is believed to hold. Returns whether the
    /// jar changed. Only what the tab itself changed is applied, so the last change wins, not the last pull:
    /// - a pulled cookie equal to the tab's `baseline` entry (expiries within `EXPIRES_TOLERANCE`) is unchanged there
    ///   and is ignored (an idle tab's stale copy cannot overwrite a newer value another tab wrote);
    /// - a pulled cookie that differs or is new is a change: it replaces the jar's (an already-expired one deletes it);
    /// - a key the tab reported before (`reported`) and is now missing is a deletion. A seeded key the tab never
    ///   reported is not: the seed may have failed, which is not a logout.
    ///
    /// The jar is indexed by key once per merge, so a pull costs O(jar + pull), not O(jar * pull). The jar's order is
    /// kept: a replacement stays in place, a new cookie goes at the end.
    pub fn merge(&mut self, pulled: Vec<Cookie>, tab: &mut TabCookies) -> bool {
        let t = now();
        let pulled_keys: HashSet<Key> = pulled.iter().map(Cookie::key).collect();
        // Each key's first slot (built back to front, so the first one wins).
        let mut index: HashMap<Key, usize> = self.cookies.iter().enumerate().rev().map(|(i, c)| (c.key(), i)).collect();
        // No code path stores a key twice, but a hand-edited or old file could.
        let repeats = index.len() < self.cookies.len();
        // One slot per jar cookie; `None` is a deletion, compacted at the end.
        let mut slots: Vec<Option<Cookie>> = std::mem::take(&mut self.cookies).into_iter().map(Some).collect();
        let mut changed = false;
        for gone in tab.reported.difference(&pulled_keys) {
            changed |= remove_key(&mut slots, &mut index, gone, repeats);
        }
        for p in &pulled {
            let k = p.key();
            if tab.baseline.get(&k).is_some_and(|b| b.same_as(p)) {
                continue;
            }
            match index.get(&k).copied() {
                _ if p.expired(t) => changed |= remove_key(&mut slots, &mut index, &k, repeats),
                Some(i) if slots[i].as_ref().is_some_and(|c| c.same_as(p)) => {}
                Some(i) => {
                    slots[i] = Some(p.clone());
                    changed = true;
                }
                None => {
                    index.insert(k, slots.len());
                    slots.push(Some(p.clone()));
                    changed = true;
                }
            }
        }
        self.cookies = slots
            .into_iter()
            .flatten()
            .filter(|c| {
                let keep = !c.expired(t);
                changed |= !keep;
                keep
            })
            .collect();
        // Seeded keys the tab has not reported yet stay in the baseline; reported keys now missing leave it.
        let reported = std::mem::replace(&mut tab.reported, pulled_keys);
        tab.baseline.retain(|k, _| !reported.contains(k));
        tab.baseline.extend(pulled.into_iter().map(|p| (p.key(), p)));
        changed
    }
}

/// `merge`'s deletion of key `k`: every copy, as a `retain` by key would. With no `repeats` in the jar, only the indexed
/// slot holds it. Whether anything was removed.
fn remove_key(slots: &mut [Option<Cookie>], index: &mut HashMap<Key, usize>, k: &Key, repeats: bool) -> bool {
    let Some(i) = index.remove(k) else { return false };
    slots[i] = None;
    if repeats {
        for slot in slots.iter_mut().filter(|s| s.as_ref().is_some_and(|c| c.key() == *k)) {
            *slot = None;
        }
    }
    true
}

/// The copy of the loaded jars spawn threads seed new hosts from (`App::jar_snapshots`), with how many times each jar
/// was cleared. Both sit under one lock, so a seed and the clear count it was read at agree (final review I3).
#[derive(Debug, Default)]
pub struct Snapshots {
    jars: BTreeMap<JarKey, Jar>,
    clears: HashMap<JarKey, u64>,
}

impl Snapshots {
    pub fn new(jars: BTreeMap<JarKey, Jar>) -> Snapshots {
        Snapshots { jars, clears: HashMap::new() }
    }

    pub fn insert(&mut self, key: JarKey, jar: Jar) {
        self.jars.insert(key, jar);
    }

    /// Drops `key`'s copy. Its clear count stays, so a host seeded from it still compares against it.
    pub fn remove(&mut self, key: &JarKey) {
        self.jars.remove(key);
    }

    /// `key` was just cleared (Clear Cookies): `jar` is the emptied jar, and the clear is counted.
    pub fn insert_cleared(&mut self, key: JarKey, jar: Jar) {
        self.jars.insert(key, jar);
        *self.clears.entry(key).or_default() += 1;
    }

    /// How many times `key` has been cleared this run.
    pub fn clears(&self, key: JarKey) -> u64 {
        self.clears.get(&key).copied().unwrap_or(0)
    }

    /// A new host's seed: `key`'s cookies, and its clear count as of that read.
    pub fn seed(&self, key: JarKey) -> (Vec<Cookie>, u64) {
        (self.jars.get(&key).map(|j| j.cookies.clone()).unwrap_or_default(), self.clears(key))
    }
}

/// What manifold believes one tab's engine holds, for `Jar::merge`. Per tab, reset when its host closes or is
/// replaced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabCookies {
    /// The tab's cookies as last seen: its seed, updated by each of its pulls.
    pub baseline: HashMap<Key, Cookie>,
    /// Keys the tab itself returned in its last pull. Starts empty: a seed is not proof the engine holds a cookie.
    pub reported: HashSet<Key>,
}

impl TabCookies {
    /// A tab just seeded with `cookies`, before it has reported anything.
    pub fn seeded(cookies: &[Cookie]) -> TabCookies {
        TabCookies { baseline: cookies.iter().map(|c| (c.key(), c.clone())).collect(), reported: HashSet::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::{ActorId, NodeId};
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

    fn key(name: &str) -> Key {
        (name.into(), "example.com".into(), "/".into())
    }

    /// `merge` as it was before the index (4b833af), the reference the indexed one must match.
    fn merge_reference(jar: &mut Vec<Cookie>, pulled: Vec<Cookie>, tab: &mut TabCookies) -> bool {
        let before = jar.clone();
        let t = now();
        let pulled_keys: HashSet<Key> = pulled.iter().map(Cookie::key).collect();
        for gone in tab.reported.difference(&pulled_keys) {
            jar.retain(|c| c.key() != *gone);
        }
        for p in &pulled {
            let k = p.key();
            if tab.baseline.get(&k).is_some_and(|b| b.same_as(p)) {
                continue;
            }
            match jar.iter_mut().position(|c| c.key() == k) {
                _ if p.expired(t) => jar.retain(|c| c.key() != k),
                Some(i) if jar[i].same_as(p) => {}
                Some(i) => jar[i] = p.clone(),
                None => jar.push(p.clone()),
            }
        }
        jar.retain(|c| !c.expired(t));
        let reported = std::mem::replace(&mut tab.reported, pulled_keys);
        tab.baseline.retain(|k, _| !reported.contains(k));
        tab.baseline.extend(pulled.into_iter().map(|p| (p.key(), p)));
        *jar != before
    }

    /// Pulls hold one cookie per key, as an engine's dump does. Every fourth starting jar repeats keys, which no code
    /// path writes but a hand-edited or legacy file could hold: the index must match the reference there too.
    #[test]
    fn the_indexed_merge_matches_the_reference() {
        let mut rng = crate::store::testutil::XorShift(11);
        let cookie = |rng: &mut crate::store::testutil::XorShift| {
            let mut x = c(&format!("n{}", rng.below(12)), &format!("v{}", rng.below(3)));
            x.expires = match rng.below(4) {
                0 => Some(1.0),
                1 => Some(4_000_000_000.0),
                _ => None,
            };
            x
        };
        let unique = |v: Vec<Cookie>| {
            let mut seen = HashSet::new();
            v.into_iter().filter(|x| seen.insert(x.key())).collect::<Vec<_>>()
        };
        for round in 0..500 {
            let raw: Vec<Cookie> = (0..rng.below(10)).map(|_| cookie(&mut rng)).collect();
            let start = if round % 4 == 0 { raw } else { unique(raw) };
            let mut tab = TabCookies::seeded(&start);
            let (mut jar, mut reference) = (Jar { locked: false, cookies: start.clone() }, start.clone());
            let mut ref_tab = TabCookies::seeded(&start);
            for _ in 0..3 {
                let pulled = unique((0..rng.below(10)).map(|_| cookie(&mut rng)).collect());
                let got = jar.merge(pulled.clone(), &mut tab);
                let want = merge_reference(&mut reference, pulled, &mut ref_tab);
                assert_eq!((got, &jar.cookies), (want, &reference), "round {round}");
                assert_eq!(tab, ref_tab, "round {round}");
            }
        }
    }

    #[test]
    fn a_pulled_cookie_replaces_the_stored_one_with_the_same_key() {
        let mut jar = Jar { locked: false, cookies: vec![c("sid", "old")] };
        let mut tab = TabCookies::seeded(&jar.cookies);
        assert!(jar.merge(vec![c("sid", "new")], &mut tab));
        assert_eq!(jar.cookies, vec![c("sid", "new")]);
    }

    #[test]
    fn a_cookie_missing_from_the_pull_is_deleted_only_if_the_tab_reported_it() {
        let mut jar = Jar { locked: false, cookies: vec![c("sid", "1"), c("fresh", "2")] };
        let mut tab = TabCookies { reported: [key("sid")].into(), ..TabCookies::default() };
        assert!(jar.merge(vec![], &mut tab));
        assert_eq!(jar.cookies, vec![c("fresh", "2")], "sid was a logout; fresh was never seen by the engine");
    }

    #[test]
    fn an_expired_pulled_cookie_deletes_the_stored_one_with_its_key() {
        let mut jar = Jar { locked: false, cookies: vec![c("sid", "1"), c("other", "2")] };
        let mut gone = c("sid", "1");
        gone.expires = Some(1.0);
        assert!(jar.merge(vec![gone], &mut TabCookies::default()), "the engine's own expiry wins");
        assert_eq!(jar.cookies, vec![c("other", "2")]);
    }

    #[test]
    fn a_stale_idle_tab_does_not_overwrite_a_rotated_token() {
        let mut jar = Jar { locked: false, cookies: vec![c("sid", "v1")] };
        let mut active = TabCookies::seeded(&jar.cookies);
        let mut idle = TabCookies::seeded(&jar.cookies);
        assert!(jar.merge(vec![c("sid", "v2")], &mut active), "the site rotated sid in the active tab");
        assert!(!jar.merge(vec![c("sid", "v1")], &mut idle), "the idle tab's v1 is what it was seeded with");
        assert_eq!(jar.cookies, vec![c("sid", "v2")]);
        assert!(!jar.merge(vec![c("sid", "v1")], &mut idle), "nor on its later pulls");
        assert_eq!(jar.cookies, vec![c("sid", "v2")]);
    }

    #[test]
    fn a_seeded_cookie_the_tab_never_reported_survives_a_pull_without_it() {
        let mut jar = Jar { locked: false, cookies: vec![c("sid", "1"), c("pref", "2")] };
        let mut tab = TabCookies::seeded(&jar.cookies);
        // The engine rejected sid's seed: its first pull has only pref.
        assert!(!jar.merge(vec![c("pref", "2")], &mut tab));
        assert_eq!(jar.cookies, vec![c("sid", "1"), c("pref", "2")]);
        assert!(!jar.merge(vec![c("pref", "2")], &mut tab), "still not a logout on the next pull");
        assert_eq!(jar.cookies.len(), 2);
    }

    #[test]
    fn a_cookie_one_pull_confirmed_and_the_next_lacks_is_deleted() {
        let mut jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
        let mut tab = TabCookies::seeded(&jar.cookies);
        assert!(!jar.merge(vec![c("sid", "1")], &mut tab));
        assert!(jar.merge(vec![], &mut tab), "the tab held sid and now does not: a logout");
        assert!(jar.cookies.is_empty());
        assert!(tab.baseline.is_empty() && tab.reported.is_empty());
    }

    #[test]
    fn an_empty_pull_from_a_tab_that_never_reported_deletes_nothing() {
        let mut jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
        let mut tab = TabCookies::seeded(&jar.cookies);
        assert!(!jar.merge(vec![], &mut tab));
        assert_eq!(jar.cookies, vec![c("sid", "1")]);
    }

    fn expiring(name: &str, value: &str, expires: f64) -> Cookie {
        Cookie { expires: Some(expires), ..c(name, value) }
    }

    #[test]
    fn expiries_within_a_millisecond_are_the_same_cookie() {
        let e = 4_102_444_800.123_456;
        assert!(expiring("sid", "1", e).same_as(&expiring("sid", "1", e + 0.000_001)), "a conversion wobble");
        assert!(!expiring("sid", "1", e).same_as(&expiring("sid", "1", e + 0.01)));
        assert!(!expiring("sid", "1", e).same_as(&expiring("sid", "2", e)), "the value still counts");
        assert!(!expiring("sid", "1", e).same_as(&c("sid", "1")), "a session cookie is not a persistent one");
        assert!(c("sid", "1").same_as(&c("sid", "1")));
    }

    #[test]
    fn an_expiry_wobble_is_ignored_and_a_real_expiry_change_wins() {
        let e = 4_102_444_800.123_456;
        let mut jar = Jar { locked: false, cookies: vec![expiring("sid", "v1", e)] };
        let mut active = TabCookies::seeded(&jar.cookies);
        let mut idle = TabCookies::seeded(&jar.cookies);
        assert!(jar.merge(vec![expiring("sid", "v2", e + 3600.0)], &mut active), "rotated, with a new Max-Age");
        // The idle tab's copy of v1 came back through its engine a microsecond off: still its seed, not a change.
        assert!(!jar.merge(vec![expiring("sid", "v1", e + 0.000_001)], &mut idle));
        assert_eq!(jar.cookies, vec![expiring("sid", "v2", e + 3600.0)]);
        // The active tab's own copy wobbles too: no change, so no save.
        assert!(!jar.merge(vec![expiring("sid", "v2", e + 3_600.000_001)], &mut active));
        // A real expiry change (a sliding session) wins.
        assert!(jar.merge(vec![expiring("sid", "v2", e + 7200.0)], &mut active));
        assert_eq!(jar.cookies, vec![expiring("sid", "v2", e + 7200.0)]);
    }

    #[test]
    fn an_unseeded_tab_reporting_the_jars_cookie_with_a_wobble_changes_nothing() {
        let e = 4_102_444_800.5;
        let mut jar = Jar { locked: false, cookies: vec![expiring("sid", "1", e)] };
        assert!(!jar.merge(vec![expiring("sid", "1", e - 0.000_001)], &mut TabCookies::default()));
        assert_eq!(jar.cookies, vec![expiring("sid", "1", e)]);
    }

    #[test]
    fn debug_never_prints_a_cookie_value() {
        let jar = Jar { locked: false, cookies: vec![c("sid", "TOP-SECRET")] };
        let text = format!("{jar:?} {:?}", c("sid", "TOP-SECRET"));
        assert!(!text.contains("TOP-SECRET") && text.contains("<redacted>") && text.contains("sid"), "{text}");
    }

    #[test]
    fn merging_the_same_cookies_changes_nothing() {
        let mut jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
        let mut tab = TabCookies::seeded(&jar.cookies);
        assert!(!jar.merge(vec![c("sid", "1")], &mut tab));
    }

    #[test]
    fn an_expired_cookie_is_dropped_on_merge() {
        let mut gone = c("old", "x");
        gone.expires = Some(1.0);
        let mut jar = Jar::default();
        assert!(!jar.merge(vec![gone], &mut TabCookies::default()), "nothing was added, so nothing changed");
        assert!(jar.cookies.is_empty());
    }

    #[test]
    fn the_jar_round_trips_through_its_file() {
        let dir = temp_dir("jar-roundtrip");
        let jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
        jar.save(&dir, JarKey::Default).unwrap();
        assert_eq!(Jar::load(&dir, JarKey::Default), jar);
    }

    fn asides(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("default.json.unreadable-"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_second_failure_keeps_the_first_aside() {
        let dir = temp_dir("jar-two-asides");
        std::fs::write(Jar::path(&dir, JarKey::Default), "first").unwrap();
        Jar::load(&dir, JarKey::Default);
        std::fs::write(Jar::path(&dir, JarKey::Default), "second").unwrap();
        Jar::load(&dir, JarKey::Default);
        let a = asides(&dir);
        assert_eq!(a.len(), 2);
        let mut contents: Vec<_> = a.iter().map(|p| std::fs::read(p).unwrap()).collect();
        contents.sort();
        assert_eq!(contents, vec![b"first".to_vec(), b"second".to_vec()]);
    }

    #[test]
    fn a_header_only_or_truncated_file_goes_aside() {
        let dir = temp_dir("jar-header-only");
        std::fs::write(Jar::path(&dir, JarKey::Default), b"MJR1").unwrap();
        assert_eq!(Jar::load(&dir, JarKey::Default), Jar::default());
        assert_eq!(asides(&dir).len(), 1);
        Jar { locked: false, cookies: vec![c("sid", "1")] }.save(&dir, JarKey::Default).unwrap();
        let bytes = std::fs::read(Jar::path(&dir, JarKey::Default)).unwrap();
        std::fs::write(Jar::path(&dir, JarKey::Default), &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(Jar::load(&dir, JarKey::Default), Jar::default());
        assert_eq!(asides(&dir).len(), 2);
    }

    #[test]
    fn an_unreadable_path_locks_the_jar_and_save_leaves_it_alone() {
        let dir = temp_dir("jar-locked");
        // A directory where the file should be: reading it fails with something other than NotFound.
        std::fs::create_dir(Jar::path(&dir, JarKey::Default)).unwrap();
        let jar = Jar::load(&dir, JarKey::Default);
        assert!(jar.locked);
        Jar { cookies: vec![c("sid", "1")], ..jar }.save(&dir, JarKey::Default).unwrap();
        assert!(Jar::path(&dir, JarKey::Default).is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn a_failed_aside_rename_locks_the_jar_and_the_file_survives() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("jar-rename-fails");
        std::fs::write(Jar::path(&dir, JarKey::Default), "not json").unwrap();
        // Opened without FILE_SHARE_DELETE, so the rename fails while it is held (read allowed).
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(Jar::path(&dir, JarKey::Default)).unwrap();
        let jar = Jar::load(&dir, JarKey::Default);
        assert!(jar.locked);
        jar.save(&dir, JarKey::Default).unwrap();
        drop(held);
        assert_eq!(std::fs::read(Jar::path(&dir, JarKey::Default)).unwrap(), b"not json");
    }

    #[test]
    fn load_leaves_a_live_instances_tmp_alone_and_save_leaves_no_tmp() {
        let dir = temp_dir("jar-tmp");
        // This test's own process stands in for another running instance mid-save.
        let live = dir.join(format!("default.json.{}.tmp", std::process::id()));
        std::fs::write(&live, "theirs").unwrap();
        Jar::load(&dir, JarKey::Default);
        assert_eq!(std::fs::read(&live).unwrap(), b"theirs", "a live instance may be mid-save");
        std::fs::remove_file(&live).unwrap();
        Jar { locked: false, cookies: vec![c("sid", "1")] }.save(&dir, JarKey::Default).unwrap();
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names, ["default.json"], "the save's own temp file is gone");
    }

    #[test]
    fn load_deletes_a_tmp_whose_process_is_gone() {
        let dir = temp_dir("jar-dead-tmp");
        // A pid no process has (Windows pids are multiples of 4; Linux caps them far below this).
        let dead = dir.join("default.json.4294967295.tmp");
        let other = dir.join("default.json.notapid.tmp");
        std::fs::write(&dead, "crashed save").unwrap();
        std::fs::write(&other, "x").unwrap();
        Jar::load(&dir, JarKey::Default);
        assert!(!dead.exists(), "a crashed save's copy of the jar");
        assert!(other.exists(), "only default.json.<pid>.tmp files are touched");
        assert!(process_alive(std::process::id()) && !process_alive(4_294_967_295));
    }

    #[test]
    fn a_dead_counter_form_tmp_is_deleted_too() {
        let dir = temp_dir("jar-dead-tmp-n");
        let dead = dir.join("default.json.4294967295-3.tmp");
        std::fs::write(&dead, "crashed save").unwrap();
        Jar::load(&dir, JarKey::Default);
        assert!(!dead.exists());
    }

    #[test]
    fn only_the_five_newest_unreadable_files_are_kept() {
        let dir = temp_dir("jar-aside-cap");
        for i in 0..7 {
            std::fs::write(Jar::path(&dir, JarKey::Default), format!("bad{i}")).unwrap();
            Jar::load(&dir, JarKey::Default);
        }
        let mut kept: Vec<_> = asides(&dir).iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
        kept.sort();
        assert_eq!(kept, ["bad2", "bad3", "bad4", "bad5", "bad6"]);
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_jar() {
        assert_eq!(Jar::load(&temp_dir("jar-missing"), JarKey::Default), Jar::default());
    }

    #[test]
    fn a_corrupt_file_loads_empty_and_is_moved_aside() {
        let dir = temp_dir("jar-corrupt");
        std::fs::write(Jar::path(&dir, JarKey::Default), "not json").unwrap();
        assert_eq!(Jar::load(&dir, JarKey::Default), Jar::default());
        assert!(!Jar::path(&dir, JarKey::Default).exists());
        assert_eq!(std::fs::read(&asides(&dir)[0]).unwrap(), b"not json");
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
        let jar = Jar { locked: false, cookies: vec![c("sid", "SECRET123")] };
        jar.save(&dir, JarKey::Default).unwrap();
        let bytes = std::fs::read(Jar::path(&dir, JarKey::Default)).unwrap();
        assert!(bytes.starts_with(b"MJR1"));
        if cfg!(windows) {
            assert!(!String::from_utf8_lossy(&bytes).contains("SECRET123"), "the value must not appear on disk");
        }
        assert_eq!(Jar::load(&dir, JarKey::Default), jar);
    }

    #[test]
    fn a_legacy_plaintext_jar_loads_and_the_next_save_protects_it() {
        let dir = temp_dir("jar-legacy");
        let jar = Jar { locked: false, cookies: vec![c("sid", "1")] };
        std::fs::write(Jar::path(&dir, JarKey::Default), serde_json::to_vec(&jar).unwrap()).unwrap();
        assert_eq!(Jar::load(&dir, JarKey::Default), jar);
        jar.save(&dir, JarKey::Default).unwrap();
        assert!(std::fs::read(Jar::path(&dir, JarKey::Default)).unwrap().starts_with(b"MJR1"));
    }

    #[test]
    fn a_tampered_protected_file_loads_empty_and_is_kept_aside() {
        let dir = temp_dir("jar-tamper");
        Jar { locked: false, cookies: vec![c("sid", "1")] }.save(&dir, JarKey::Default).unwrap();
        let mut bytes = std::fs::read(Jar::path(&dir, JarKey::Default)).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(Jar::path(&dir, JarKey::Default), &bytes).unwrap();
        assert_eq!(Jar::load(&dir, JarKey::Default), Jar::default());
        assert_eq!(std::fs::read(&asides(&dir)[0]).unwrap(), bytes);
        // A save after the failed load writes a fresh file and leaves the aside copy alone.
        Jar::default().save(&dir, JarKey::Default).unwrap();
        assert_eq!(asides(&dir).len(), 1);
    }

    #[test]
    fn a_named_jar_has_its_own_file_beside_the_default() {
        let dir = temp_dir("jar-named-file");
        let named = JarKey::Named(NodeId { actor: ActorId(0xab), counter: 3 });
        assert_eq!(Jar::path(&dir, JarKey::Default), dir.join("default.json"));
        assert_eq!(Jar::path(&dir, named), dir.join("00000000000000ab-3.json"));
        let a = Jar { locked: false, cookies: vec![c("sid", "alice")] };
        let b = Jar { locked: false, cookies: vec![c("sid", "bob")] };
        a.save(&dir, JarKey::Default).unwrap();
        b.save(&dir, named).unwrap();
        assert_eq!(Jar::load(&dir, JarKey::Default), a);
        assert_eq!(Jar::load(&dir, named), b);
        Jar::delete_files(&dir, named).unwrap();
        assert!(!Jar::path(&dir, named).exists());
        Jar::delete_files(&dir, named).unwrap(); // already gone: fine
        assert_eq!(Jar::load(&dir, JarKey::Default), a, "the default jar is untouched");
    }

    /// Final review I2: every copy of a deleted jar goes, and only that jar's: `…-12` shares `…-1`'s prefix.
    #[test]
    fn delete_files_takes_only_that_jars_copies() {
        let dir = temp_dir("jar-delete-files");
        let one = JarKey::Named(NodeId { actor: ActorId(7), counter: 1 });
        let twelve = JarKey::Named(NodeId { actor: ActorId(7), counter: 12 });
        let (a, b) = (one.file_name(), twelve.file_name());
        let gone =
            [a.clone(), format!("{a}.unreadable-5"), format!("{a}.unreadable-5-1"), format!("{a}.4294967295.tmp")];
        let kept =
            [format!("{a}.{}.tmp", std::process::id()), b.clone(), format!("{b}.unreadable-5"), "default.json".into()];
        for name in gone.iter().chain(&kept) {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        Jar::delete_files(&dir, one).unwrap();
        for name in &gone {
            assert!(!dir.join(name).exists(), "{name}");
        }
        for name in &kept {
            assert!(dir.join(name).exists(), "{name}");
        }
    }

    /// Final review I2b: only a named jar's own file names count, in their exact spelling.
    #[test]
    fn named_jar_of_reads_only_named_jar_file_names() {
        let id = NodeId { actor: ActorId(0xab), counter: 3 };
        for name in [
            "00000000000000ab-3.json",
            "00000000000000ab-3.json.unreadable-9-1",
            "00000000000000ab-3.json.42.tmp",
            "00000000000000ab-3.json.42-7.tmp",
        ] {
            assert_eq!(named_jar_of(name), Some(id), "{name}");
        }
        for name in [
            "default.json",
            "default.json.unreadable-9",
            "00000000000000AB-3.json", // not how ids are spelled
            "00000000000000ab-03.json",
            "00000000000000ab-3.jsonx",
            "00000000000000ab-3.json.x.tmp",
            "00000000000000ab-3.json.bak",
            "notes.txt",
        ] {
            assert_eq!(named_jar_of(name), None, "{name}");
        }
    }

    #[test]
    fn prune_orphans_deletes_only_unkept_named_jars_files() {
        let dir = temp_dir("jar-prune-orphans");
        let kept = NodeId { actor: ActorId(1), counter: 1 };
        let gone = NodeId { actor: ActorId(1), counter: 2 };
        let (k, g) = (JarKey::Named(kept).file_name(), JarKey::Named(gone).file_name());
        let doomed = [g.clone(), format!("{g}.unreadable-5"), format!("{g}.4294967295.tmp")];
        let stays = [
            k.clone(),
            format!("{k}.unreadable-5"),
            format!("{g}.{}.tmp", std::process::id()), // a live save in progress
            "default.json".into(),
            "default.json.unreadable-5".into(),
            "notes.txt".into(),
        ];
        for name in doomed.iter().chain(&stays) {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        assert_eq!(prune_orphans(&dir, &[kept].into()), 1);
        for name in &doomed {
            assert!(!dir.join(name).exists(), "{name}");
        }
        for name in &stays {
            assert!(dir.join(name).exists(), "{name}");
        }
    }

    #[test]
    fn an_unreadable_named_jar_goes_aside_under_its_own_name() {
        let dir = temp_dir("jar-named-aside");
        let named = JarKey::Named(NodeId { actor: ActorId(1), counter: 2 });
        std::fs::write(Jar::path(&dir, named), "garbage").unwrap();
        assert!(Jar::load(&dir, named).cookies.is_empty());
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert!(names.iter().any(|n| n.starts_with("0000000000000001-2.json.unreadable-")), "{names:?}");
        assert!(asides(&dir).is_empty(), "no default.json aside");
    }

    #[test]
    fn engine_json_parses_into_cookies() {
        let json = r#"[{"name":"a","value":"b","domain":"x.com","path":"/","expires":null,"secure":true,"http_only":false,"same_site":"Strict"}]"#;
        let cookies = from_json(json).unwrap();
        assert_eq!(cookies[0].same_site, SameSite::Strict);
        assert!(from_json("nope").is_none());
    }
}
