//! `gate host-pin` and `gate cef-pin` (were scripts/host_pin.py and cef_pin.py): rewrite the pin tables in
//! crates/manifold/src/host_pin.rs and cef_pin.rs.
//!
//! `gate host-pin --check` fails if any entry's URL carries a fingerprint other than `gate host-fingerprint`'s for the
//! current tree (also run in ci.yml and release.yml). `gate host-pin --adopt KIND PLATFORM [--from DIR]` takes
//! `pin.json` and the files of the `hosts-KIND-FP` release (or of DIR, for tests and offline use), verifies each
//! file's size and SHA-256, and rewrites that entry between `// BEGIN TABLE` and `// END TABLE`.
//! `gate host-pin --manifest KIND PLATFORM FP DIR` writes `DIR/pin.json` for the files named `FILE-PLATFORM` in DIR
//! (hosts.yml uses it). All take `--out FILE` (default crates/manifold/src/host_pin.rs). The rewrite is atomic and
//! leaves the file alone on any refusal. KIND is servo, cef, webkit or webview2; PLATFORM windows-x64 or linux-x64.
//!
//! `gate cef-pin [--platform ID] PREFIX` (e.g. `152.0.6`) re-pins one CEF platform: it reads
//! https://cef-builds.spotifycdn.com/index.json, picks that platform's `minimal` file whose cef_version starts with
//! `PREFIX+`, downloads it ONCE with curl (about 172 MB for windows64; running the command is the confirmation, and
//! the notice goes to stderr first) to a temp file, checks it against the index's size and sha1, computes the SHA-256
//! (the index has none), deletes the temp file (`CEF_PIN_KEEP=1` keeps it), and replaces that platform's
//! `// BEGIN <id>` block in cef_pin.rs, adding it and its `pin_for` arm when new. Other platforms' blocks stay.
//! Platforms: windows64 (default), linux64, macosx64, macosarm64. Re-pin every pinned platform to the same CEF build
//! after a `cef` crate bump (see `the_pin_matches_the_cef_crates_version` in view_install.rs).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};

pub(crate) const RELEASES: &str = "https://github.com/winstonyli/manifold/releases/download/";
const KINDS: [(&str, &str); 4] = [("servo", "Servo"), ("cef", "Cef"), ("webkit", "Webkit"), ("webview2", "Webview2")];

pub(crate) fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// (SHA-256 hex, SHA-1 hex, byte count) of a file, read in 1 MiB chunks.
pub(crate) fn hash_file(path: &Path) -> Result<(String, String, u64), String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (mut s256, mut s1, mut n) = (Sha256::new(), Sha1::new(), 0u64);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let got = f.read(&mut buf).map_err(|e| e.to_string())?;
        if got == 0 {
            return Ok((hex(&s256.finalize()), hex(&s1.finalize()), n));
        }
        s256.update(&buf[..got]);
        s1.update(&buf[..got]);
        n += got as u64;
    }
}

// ---- host_pin.rs ----

/// url, size, sha256 per file name.
pub(crate) type Files = BTreeMap<String, (String, u64, String)>;
/// Files per (kind name, platform).
pub(crate) type Table = BTreeMap<(String, String), Files>;

pub(crate) fn region(text: &str) -> Result<(usize, usize), String> {
    let begin = text.find("// BEGIN TABLE\n").ok_or("no `// BEGIN TABLE` line")? + "// BEGIN TABLE\n".len();
    let end = text[begin..].find("// END TABLE\n").ok_or("no `// END TABLE` line")? + begin;
    Ok((begin, end))
}

pub(crate) fn parse_table(body: &str) -> Result<Table, String> {
    let mut table = Table::new();
    let mut cur: Option<(String, String)> = None;
    for line in body.lines() {
        let t = line.trim();
        let q: Vec<&str> = t.split('"').collect();
        if t.starts_with("Entry {") {
            let variant = t
                .split("kind: ")
                .nth(1)
                .and_then(|r| r.split(',').next())
                .and_then(|p| p.rsplit("::").next())
                .unwrap_or("");
            let kind = KINDS.iter().find(|k| k.1 == variant).ok_or(format!("unknown kind in {t}"))?.0;
            cur = Some((kind.to_owned(), q.get(1).ok_or("Entry without a platform")?.to_string()));
            table.entry(cur.clone().unwrap_or_default()).or_default();
        } else if t.starts_with("Component {") && q.len() >= 7 {
            let key = cur.clone().ok_or("a Component line outside an Entry")?;
            let size: u64 =
                q[4].trim_start_matches(", size: ").trim_end_matches(", sha256: ").parse().map_err(|_| t.to_owned())?;
            table.entry(key).or_default().insert(q[1].into(), (q[3].into(), size, q[5].into()));
        }
    }
    Ok(table)
}

fn render_table(table: &Table) -> String {
    let mut s = String::from("#[rustfmt::skip]\nconst TABLE: &[Entry] = &[\n");
    for ((kind, platform), files) in table {
        let variant = KINDS.iter().find(|k| k.0 == kind).map_or("", |k| k.1);
        let _ = writeln!(s, "    Entry {{ kind: ViewKind::{variant}, platform: \"{platform}\", components: &[");
        for (file, (url, size, sha)) in files {
            let _ = writeln!(
                s,
                "        Component {{ file: \"{file}\", url: \"{url}\", size: {size}, sha256: \"{sha}\" }},"
            );
        }
        s.push_str("    ] },\n");
    }
    s.push_str("];\n");
    s
}

fn release_tag(kind: &str, fp: &str) -> String {
    format!("hosts-{kind}-{fp}")
}

/// The fingerprint a pin URL carries (`.../download/hosts-KIND-FP/FILE-PLATFORM`), if it has that shape.
fn url_fingerprint<'a>(url: &'a str, kind: &str) -> Option<&'a str> {
    url.strip_prefix(RELEASES)?.split('/').next()?.strip_prefix(&format!("hosts-{kind}-"))
}

/// One line per entry file whose URL does not carry the fingerprint `fp` computes for its kind and platform.
pub(crate) fn stale_entries(table: &Table, fp: &dyn Fn(&str, &str) -> Result<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for ((kind, platform), files) in table {
        let want = match fp(kind, platform) {
            Ok(w) => w,
            Err(e) => {
                out.push(format!("{kind}/{platform}: {e}"));
                continue;
            }
        };
        for (file, (url, ..)) in files {
            let got = url_fingerprint(url, kind);
            if got != Some(want.as_str()) {
                out.push(format!(
                    "{kind}/{platform} {file}: pinned for {got:?}, sources are {want}: run the hosts workflow for {kind}, then `gate host-pin --adopt {kind} {platform}` and commit"
                ));
            }
        }
    }
    out
}

fn host_pin_path() -> PathBuf {
    repo().join("crates/manifold/src/host_pin.rs")
}

pub(crate) fn read_table(path: &Path) -> Result<Table, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (b, e) = region(&text).map_err(|m| format!("{}: {m}", path.display()))?;
    parse_table(&text[b..e])
}

/// SHA-256 of the generated table text, for the release notes.
pub(crate) fn table_sha256() -> Result<String, String> {
    let path = host_pin_path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (b, e) = region(&text)?;
    Ok(hex(&Sha256::digest(&text.as_bytes()[b..e])))
}

/// Every pinned entry carries the fingerprint of the current tree.
pub(crate) fn pins_are_fresh() -> Result<(), Vec<String>> {
    let table = read_table(&host_pin_path()).map_err(|e| vec![e])?;
    let stale = stale_entries(&table, &|k, p| crate::fingerprint::fingerprint(&repo(), k, p, true));
    if stale.is_empty() {
        Ok(())
    } else {
        Err(stale)
    }
}

fn write_table(out: &Path, table: &Table) -> Result<(), String> {
    let text = std::fs::read_to_string(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let (b, e) = region(&text).map_err(|m| format!("{}: {m}", out.display()))?;
    let new = format!("{}{}{}", &text[..b], render_table(table), &text[e..]);
    let tmp = out.with_extension("rs.new");
    std::fs::write(&tmp, new).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

pub(crate) fn download(url: &str, dest: &Path) -> Result<(), String> {
    curl(&["-o", &dest.to_string_lossy(), url]).map(|_| ())
}

/// What a `hosts-KIND-FP` release says it holds.
#[derive(Debug, PartialEq)]
struct Manifest {
    kind: String,
    platform: String,
    fingerprint: String,
    /// name (without the platform suffix), size, sha256
    files: Vec<(String, u64, String)>,
}

fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("pin.json: {e}"))?;
    let s = |k: &str| v[k].as_str().map(str::to_owned).ok_or(format!("pin.json has no `{k}`"));
    let mut files = Vec::new();
    for f in v["files"].as_array().ok_or("pin.json has no `files`")? {
        let field = |k: &str| f[k].as_str().map(str::to_owned).ok_or(format!("pin.json file without `{k}`"));
        files.push((field("name")?, f["size"].as_u64().ok_or("pin.json file without `size`")?, field("sha256")?));
    }
    Ok(Manifest { kind: s("kind")?, platform: s("platform")?, fingerprint: s("fingerprint")?, files })
}

/// Writes `dir/pin.json` for the files named `NAME-PLATFORM` in `dir`.
fn write_manifest(dir: &Path, kind: &str, platform: &str, fp: &str) -> Result<(), String> {
    let suffix = format!("-{platform}");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(&suffix))
        .collect();
    names.sort();
    let mut files = Vec::new();
    for asset in &names {
        let (sha, _, size) = hash_file(&dir.join(asset))?;
        if size == 0 {
            return Err(format!("{asset}: empty file"));
        }
        files.push(serde_json::json!({"name": &asset[..asset.len() - suffix.len()], "size": size, "sha256": sha}));
    }
    if files.is_empty() {
        return Err(format!("no *{suffix} files in {}", dir.display()));
    }
    let json = serde_json::json!({"kind": kind, "platform": platform, "fingerprint": fp, "files": files});
    std::fs::write(dir.join("pin.json"), serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// Replaces `kind`/`platform` in `table` with the files `m` lists, after fetching each through `fetch(asset, dest)`
/// and checking its size and SHA-256. `table` is untouched on any error.
fn adopt(
    table: &mut Table,
    kind: &str,
    platform: &str,
    fp: &str,
    m: &Manifest,
    fetch: &dyn Fn(&str, &Path) -> Result<(), String>,
) -> Result<(), String> {
    if (m.kind.as_str(), m.platform.as_str(), m.fingerprint.as_str()) != (kind, platform, fp) {
        return Err(format!(
            "pin.json is for {}/{} {}, wanted {kind}/{platform} {fp}",
            m.kind, m.platform, m.fingerprint
        ));
    }
    if m.files.is_empty() {
        return Err("pin.json lists no files".into());
    }
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("adopt-{}-{n}-{kind}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut files = Files::new();
        for (name, size, sha) in &m.files {
            let asset = format!("{name}-{platform}");
            let dest = dir.join(&asset);
            fetch(&asset, &dest)?;
            let (got, _, n) = hash_file(&dest)?;
            if &got != sha || n != *size {
                return Err(format!("{asset}: {n} bytes sha256 {got}, pin.json says {size} bytes sha256 {sha}"));
            }
            files.insert(name.clone(), (format!("{RELEASES}{}/{asset}", release_tag(kind, fp)), *size, sha.clone()));
        }
        Ok(files)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    table.insert((kind.to_owned(), platform.to_owned()), result?);
    Ok(())
}

fn host_pin(args: &[String]) -> Result<(), String> {
    let mut out = host_pin_path();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--out" {
            out = PathBuf::from(it.next().ok_or("--out needs a value")?);
        } else {
            rest.push(a.as_str());
        }
    }
    match rest.as_slice() {
        ["--check"] => {
            let table = read_table(&out)?;
            let stale = stale_entries(&table, &|k, p| crate::fingerprint::fingerprint(&repo(), k, p, true));
            if stale.is_empty() {
                eprintln!("host pins match the sources");
                Ok(())
            } else {
                Err(stale.join("\n"))
            }
        }
        ["--manifest", kind, platform, fp, dir] => write_manifest(Path::new(dir), kind, platform, fp),
        ["--adopt", kind, platform, tail @ ..] => {
            let from = match tail {
                [] => None,
                ["--from", dir] => Some(PathBuf::from(dir)),
                _ => return Err("usage: gate host-pin --adopt KIND PLATFORM [--from DIR]".into()),
            };
            let fp = crate::fingerprint::fingerprint(&repo(), kind, platform, true)?;
            let base = format!("{RELEASES}{}/", release_tag(kind, &fp));
            let manifest = match &from {
                Some(dir) => std::fs::read_to_string(dir.join("pin.json")).map_err(|e| e.to_string())?,
                None => String::from_utf8(curl(&[&format!("{base}pin.json")])?).map_err(|e| e.to_string())?,
            };
            let fetch = |asset: &str, dest: &Path| match &from {
                Some(dir) => std::fs::copy(dir.join(asset), dest).map(|_| ()).map_err(|e| e.to_string()),
                None => download(&format!("{base}{asset}"), dest),
            };
            let mut table = read_table(&out)?;
            adopt(&mut table, kind, platform, &fp, &parse_manifest(&manifest)?, &fetch)?;
            write_table(&out, &table)?;
            eprintln!("adopted {kind}/{platform} {fp} into {}", out.display());
            Ok(())
        }
        _ => Err("usage: gate host-pin --check | --adopt KIND PLATFORM [--from DIR] | --manifest KIND PLATFORM FP DIR [--out FILE]".into()),
    }
}

// ---- cef_pin.rs ----

const CEF_BASE: &str = "https://cef-builds.spotifycdn.com/";
const CEF_PLATFORMS: [&str; 4] = ["windows64", "linux64", "macosx64", "macosarm64"];

const CEF_HEADER: &str = "//! The CEF builds the app downloads on demand, one pin per CDN platform.\n//! Generated by `gate cef-pin` (see its module doc to re-pin); do not edit by hand.\n\nuse crate::view_install::Pin;\n";

/// `urllib.parse.quote(s, safe="")`: everything but letters, digits and `_.-~` is %-escaped.
fn quote(s: &str) -> String {
    s.bytes().fold(String::new(), |mut o, b| {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) {
            o.push(b as char);
        } else {
            let _ = write!(o, "%{b:02X}");
        }
        o
    })
}

/// `171619649` as `171_619_649`.
fn underscored(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('_');
        }
        out.push(c);
    }
    out
}

/// The `// BEGIN id` .. `// END id` blocks of an existing cef_pin.rs, by platform.
fn blocks_of(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for p in CEF_PLATFORMS {
        let (b, e) = (format!("// BEGIN {p}\n"), format!("// END {p}\n"));
        if let Some(s) = text.find(&b) {
            if let Some(len) = text[s..].find(&e) {
                out.insert(p.to_owned(), text[s..s + len + e.len()].to_owned());
            }
        }
    }
    out
}

fn render_cef(blocks: &BTreeMap<String, String>) -> String {
    let order: Vec<&str> = CEF_PLATFORMS.into_iter().filter(|p| blocks.contains_key(*p)).collect();
    let arms: String = order.iter().map(|p| format!("        \"{p}\" => Some(&PIN_{}),\n", p.to_uppercase())).collect();
    let all: Vec<&str> = order.iter().map(|p| blocks[*p].as_str()).collect();
    format!(
        "{CEF_HEADER}\n{}\n/// The pin for CDN platform `id` (`CefPlatform::id`), or `None` while that platform has none.\npub(crate) fn pin_for(id: &str) -> Option<&'static Pin> {{\n    match id {{\n{arms}        _ => None,\n    }}\n}}\n\n/// The pin for the platform this build runs on: `None` where no CEF build is pinned yet.\npub(crate) fn pin_for_platform() -> Option<&'static Pin> {{\n    pin_for(crate::cef_platform::PLATFORM.id)\n}}\n",
        all.join("\n")
    )
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl.exe")
        .args(["-sSL", "--fail"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl.exe: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!("curl.exe {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn cef_pin(args: &[String]) -> Result<(), String> {
    let (platform, prefix) = match args {
        [flag, p, prefix] if flag == "--platform" => (p.as_str(), prefix.as_str()),
        [prefix] => ("windows64", prefix.as_str()),
        _ => return Err("usage: gate cef-pin [--platform windows64|linux64|macosx64|macosarm64] PREFIX".into()),
    };
    if !CEF_PLATFORMS.contains(&platform) {
        return Err(format!("unknown platform {platform}"));
    }
    let index: Value =
        serde_json::from_slice(&curl(&[&format!("{CEF_BASE}index.json")])?).map_err(|e| e.to_string())?;
    let want = format!("{prefix}+");
    let hits: Vec<(String, &Value)> = index[platform]["versions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["cef_version"].as_str().is_some_and(|c| c.starts_with(&want)))
        .filter_map(|v| {
            Some((
                v["cef_version"].as_str()?.to_owned(),
                v["files"].as_array()?.iter().find(|f| f["type"] == "minimal")?,
            ))
        })
        .collect();
    let [(version, f)] = hits.as_slice() else {
        return Err(format!("expected exactly one {platform} minimal build for {prefix:?}, found {}", hits.len()));
    };
    let name = f["name"].as_str().ok_or("index file without a name")?;
    let size: u64 =
        f["size"].as_u64().or_else(|| f["size"].as_str()?.parse().ok()).ok_or("index file without a size")?;
    let sha1 = f["sha1"].as_str().ok_or("index file without a sha1")?;
    let url = format!("{CEF_BASE}{}", quote(name));
    eprintln!("About to download {name} ({:.1} MB) from {url} to a temp file, once.", size as f64 / 1e6);
    let tmp = std::env::temp_dir().join(format!("cef-pin-{}.tar.bz2", std::process::id()));
    let result = (|| {
        curl(&["-o", &tmp.to_string_lossy(), &url])?;
        hash_file(&tmp)
    })();
    if std::env::var_os("CEF_PIN_KEEP").is_some() {
        eprintln!("CEF_PIN_KEEP set: keeping {} (delete it yourself).", tmp.display());
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
    let (sha256, got_sha1, got) = result?;
    eprintln!("Downloaded {got} bytes.");
    if got != size || got_sha1 != sha1 {
        return Err(format!(
            "download does not match the index: {got} bytes sha1 {got_sha1}, index says {size} bytes sha1 {sha1}"
        ));
    }
    let out = repo().join("crates/manifold/src/cef_pin.rs");
    let mut blocks = blocks_of(&std::fs::read_to_string(&out).unwrap_or_default());
    let upper = platform.to_uppercase();
    blocks.insert(
        platform.to_owned(),
        format!(
            "// BEGIN {platform}\npub(crate) const PIN_{upper}: Pin = Pin {{\n    version: \"{version}\",\n    url: \"{url}\",\n    size: {},\n    sha1: \"{sha1}\",\n    sha256: \"{sha256}\",\n}};\n// END {platform}\n",
            underscored(size)
        ),
    );
    let tmp_out = out.with_extension("rs.new");
    std::fs::write(&tmp_out, render_cef(&blocks)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp_out, &out).map_err(|e| e.to_string())?;
    eprintln!("Wrote {}.", out.display());
    Ok(())
}

fn exit(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

pub fn host_pin_main(args: &[String]) -> ExitCode {
    exit(host_pin(args))
}

pub fn cef_pin_main(args: &[String]) -> ExitCode {
    exit(cef_pin(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checked-in tables parse and render back to the same text, so the rewrite is byte-stable.
    #[test]
    fn the_checked_in_tables_round_trip() {
        let host = std::fs::read_to_string(repo().join("crates/manifold/src/host_pin.rs")).unwrap();
        let (b, e) = region(&host).unwrap();
        let body = &host[b..e];
        assert_eq!(render_table(&parse_table(body).unwrap()), body);
        let cef = std::fs::read_to_string(repo().join("crates/manifold/src/cef_pin.rs")).unwrap();
        assert_eq!(render_cef(&blocks_of(&cef)), cef);
    }

    fn table_of(kind: &str, platform: &str, url: &str) -> Table {
        let mut t = Table::new();
        t.entry((kind.into(), platform.into())).or_default().insert("a.exe".into(), (url.into(), 3, "0".repeat(64)));
        t
    }

    #[test]
    fn a_pin_url_carries_its_fingerprint() {
        let ok = format!("{RELEASES}hosts-cef-0123456789ab/a.exe-windows-x64");
        assert_eq!(url_fingerprint(&ok, "cef"), Some("0123456789ab"));
        assert_eq!(url_fingerprint(&ok, "servo"), None);
        assert_eq!(url_fingerprint(&format!("{RELEASES}v0.5.0/a.exe-windows-x64"), "cef"), None);
        assert_eq!(url_fingerprint("https://example.com/hosts-cef-0123456789ab/a", "cef"), None);
    }

    #[test]
    fn stale_entries_name_the_entry_and_the_fix() {
        let t = table_of("cef", "windows-x64", &format!("{RELEASES}hosts-cef-0123456789ab/a.exe-windows-x64"));
        assert!(stale_entries(&t, &|_, _| Ok("0123456789ab".into())).is_empty());
        let stale = stale_entries(&t, &|_, _| Ok("ffffffffffff".into()));
        assert_eq!(stale.len(), 1);
        assert!(stale[0].contains("cef/windows-x64") && stale[0].contains("--adopt cef windows-x64"), "{stale:?}");
        let old = table_of("cef", "windows-x64", &format!("{RELEASES}v0.5.0/a.exe-windows-x64"));
        assert_eq!(stale_entries(&old, &|_, _| Ok("0123456789ab".into())).len(), 1);
        assert_eq!(stale_entries(&t, &|_, _| Err("no recipe".into())).len(), 1);
    }

    fn release_dir(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pin-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t").replace("::", "-")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.exe-windows-x64"), bytes).unwrap();
        write_manifest(&dir, "cef", "windows-x64", "0123456789ab").unwrap();
        dir
    }

    fn copy_from(dir: PathBuf) -> impl Fn(&str, &Path) -> Result<(), String> {
        move |asset, dest| std::fs::copy(dir.join(asset), dest).map(|_| ()).map_err(|e| e.to_string())
    }

    #[test]
    fn adopt_replaces_one_entry_and_keeps_the_rest() {
        let dir = release_dir("ok", b"abc");
        let m = parse_manifest(&std::fs::read_to_string(dir.join("pin.json")).unwrap()).unwrap();
        let mut t = table_of("servo", "windows-x64", "https://example.com/hosts-servo-111111111111/a.exe-windows-x64");
        t.extend(table_of("cef", "windows-x64", "https://example.com/old/a.exe-windows-x64"));
        adopt(&mut t, "cef", "windows-x64", "0123456789ab", &m, &copy_from(dir.clone())).unwrap();
        let (url, size, sha) = &t[&("cef".into(), "windows-x64".into())]["a.exe"];
        assert_eq!(url, &format!("{RELEASES}hosts-cef-0123456789ab/a.exe-windows-x64"));
        assert_eq!((*size, sha.len()), (3, 64));
        assert!(t.contains_key(&("servo".into(), "windows-x64".into())));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adopt_refuses_a_foreign_manifest_or_a_changed_file_and_leaves_the_table_alone() {
        let dir = release_dir("bad", b"abc");
        let m = parse_manifest(&std::fs::read_to_string(dir.join("pin.json")).unwrap()).unwrap();
        let before = table_of("cef", "windows-x64", "https://example.com/old/a.exe-windows-x64");
        let mut t = before.clone();
        assert!(adopt(&mut t, "cef", "windows-x64", "ffffffffffff", &m, &copy_from(dir.clone())).is_err());
        assert!(adopt(&mut t, "servo", "windows-x64", "0123456789ab", &m, &copy_from(dir.clone())).is_err());
        std::fs::write(dir.join("a.exe-windows-x64"), b"abd").unwrap();
        assert!(adopt(&mut t, "cef", "windows-x64", "0123456789ab", &m, &copy_from(dir.clone())).is_err());
        assert_eq!(t, before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_manifest_lists_every_platform_file_with_size_and_hash() {
        let dir = release_dir("mf", b"abc");
        std::fs::write(dir.join("other.dll-windows-x64"), b"xy").unwrap();
        std::fs::write(dir.join("ignored.txt"), b"zz").unwrap();
        write_manifest(&dir, "cef", "windows-x64", "0123456789ab").unwrap();
        let m = parse_manifest(&std::fs::read_to_string(dir.join("pin.json")).unwrap()).unwrap();
        let names: Vec<&str> = m.files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names, ["a.exe", "other.dll"]);
        assert_eq!(m.files[1].1, 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn quoting_and_underscores_match_python() {
        assert_eq!(
            quote("cef_binary_152.0.6+g7+chromium_windows64_minimal.tar.bz2"),
            "cef_binary_152.0.6%2Bg7%2Bchromium_windows64_minimal.tar.bz2"
        );
        assert_eq!(underscored(171_619_649), "171_619_649");
        assert_eq!(underscored(999), "999");
    }
}
