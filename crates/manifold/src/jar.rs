//! The cookie jar manifold owns. Engines are seeded from it at tab start and pulled back into it, so a login
//! carries between engines (spec docs/superpowers/specs/2026-10-02-jars-design.md). Pure: no egui, no `App`; it
//! names jars by store `NodeId`.

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

/// The neutral cookie every engine's cookies convert to and from. `expires` is Unix seconds, `None` for a session
/// cookie.
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

/// An engine's cookie dump, as its host serialised it. The error gives the position only: serde's message can quote
/// the input, and a cookie value is a credential.
pub fn from_json(json: &str) -> Result<Vec<Cookie>, String> {
    serde_json::from_str(json)
        .map_err(|e| format!("{:?} error at line {} column {}", e.classify(), e.line(), e.column()))
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
    /// `.unreadable-*` copies and the `.<pid>-<n>.tmp` files of crashed saves, each a full copy of its
    /// credentials. A live process's temp file is its save in progress and stays (`remove_dead_tmps`). A file
    /// already gone is fine; the first other failure is returned after trying the rest.
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

/// `merge`'s deletion of key `k`: every copy, as a `retain` by key would. With no `repeats` in the jar, only the
/// indexed slot holds it. Whether anything was removed.
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

/// The cookie jars' runtime bookkeeping (`App::cookies`): which jar each host was spawned with, the pull rounds and the
/// holds parked for a tab's last cookies. The jars themselves are `App::jars`.
#[derive(Default)]
pub(crate) struct CookieState {
    /// Per running host, the jar it was **spawned with** (spec "Live tabs when `jar` changes") and what its engine is
    /// believed to hold (`jar::TabCookies`): only a cookie the tab itself changed reaches that jar, and only a cookie
    /// the tab itself reported can be deleted by its absence.
    pub(crate) jar_tabs: HashMap<String, (JarKey, TabCookies)>,
    /// Hosts still holding cookies from a jar the user cleared (`clear_jar_cookies`): they switch (`will_switch`)
    /// although their key is unchanged, and their pulls are dropped so the old cookies cannot refill the jar. An id
    /// leaves when its host is replaced or closed (`forget`).
    pub(crate) jar_reseed: HashSet<String>,
    /// A copy of `jars` for spawn threads: the seed is read when the host connects, not when the spawn starts. Also
    /// counts each jar's clears, so a host seeded before a clear is caught when it lands (`note_spawned_jar`).
    pub(crate) jar_snapshots: std::sync::Arc<std::sync::Mutex<Snapshots>>,
    /// A sleep's `PageState` or a swap's `StateSnapshot` waiting for the tab's `Cookies` reply (or its deadline), so
    /// the host is not dropped before its last cookies arrive.
    pub(crate) cookie_holds: HashMap<String, (std::time::Instant, crate::tabs::Held)>,
    /// When `jar_pull_all` last ran, for the 30 s pull in `about_to_wait`.
    pub(crate) jar_last_pull: Option<std::time::Instant>,
    /// Tabs with a `GetCookies` in flight, by tab id. A host may never answer, so an entry expires
    /// (`JAR_PULL_TIMEOUT`) instead of blocking the next pull.
    pub(crate) jar_pulls: HashMap<String, std::time::Instant>,
    /// Consecutive unanswered pulls per tab (`tabs::start_pull`).
    pub(crate) jar_silent: HashMap<String, u8>,
    /// Jars a pull changed and not saved yet, each with when it first changed: saved once the pull round ends
    /// (`flush_dirty_jars`), not once per tab's reply. Memory and `jar_snapshots` are current; only the file lags.
    pub(crate) jar_dirty: BTreeMap<JarKey, std::time::Instant>,
}

impl CookieState {
    /// Drops tab `id`'s host from the per-tab tables (it closed, or a new host replaces it): its `jar_tabs` entry, pull
    /// bookkeeping and reseed mark. Returns the held answer, if any, for the caller to cancel (`App::cancel_held`).
    pub(crate) fn forget(&mut self, id: &str) -> Option<(std::time::Instant, crate::tabs::Held)> {
        self.jar_tabs.remove(id);
        self.jar_reseed.remove(id);
        self.jar_pulls.remove(id);
        self.jar_silent.remove(id);
        self.cookie_holds.remove(id)
    }
}

/// The copy of the loaded jars spawn threads seed new hosts from (`CookieState::jar_snapshots`), with how many times each jar
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
mod tests;
