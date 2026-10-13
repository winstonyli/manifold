//! `gate host-pin` and `gate cef-pin` (were scripts/host_pin.py and cef_pin.py): rewrite the pin tables in
//! crates/manifold/src/host_pin.rs and cef_pin.rs.
//!
//! `gate host-pin --version V [--out FILE] [--keep-from OLD.rs] --asset KIND/PLATFORM=PATH ...` (run by
//! scripts/release.sh) rewrites the text between `// BEGIN TABLE` and `// END TABLE` with the engine host binaries a
//! release built and uploads. KIND is servo, cef, webkit or webview2; PLATFORM is windows-x64 or linux-x64. Each file
//! is hashed (SHA-256) and measured, installs under its base name and is fetched from
//! `https://github.com/winstonyli/manifold/releases/download/v<V>/<name>-<platform>`. A missing or empty file is
//! refused. `--keep-from` copies an older table's entries unchanged (their URLs keep pointing at the older release);
//! an asset for the same (kind, platform, name) is an error. The SHA-256 of the new table text is printed on stdout
//! for the release notes. The file is left alone if anything is refused. `gate host-pin --list FILE` prints a file's
//! table instead: a `version V` line, then `kind platform file size sha256 url` per component.
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

const RELEASES: &str = "https://github.com/winstonyli/manifold/releases/download/";
const KINDS: [(&str, &str); 4] = [("servo", "Servo"), ("cef", "Cef"), ("webkit", "Webkit"), ("webview2", "Webview2")];
const PLATFORMS: [&str; 2] = ["windows-x64", "linux-x64"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// (SHA-256 hex, SHA-1 hex, byte count) of a file, read in 1 MiB chunks.
fn hash_file(path: &Path) -> Result<(String, String, u64), String> {
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
type Files = BTreeMap<String, (String, u64, String)>;
/// Files per (kind name, platform).
type Table = BTreeMap<(String, String), Files>;

fn region(text: &str) -> Result<(usize, usize), String> {
    let begin = text.find("// BEGIN TABLE\n").ok_or("no `// BEGIN TABLE` line")? + "// BEGIN TABLE\n".len();
    let end = text[begin..].find("// END TABLE\n").ok_or("no `// END TABLE` line")? + begin;
    Ok((begin, end))
}

fn parse_table(body: &str) -> Result<Table, String> {
    let mut table = Table::new();
    let mut cur: Option<(String, String)> = None;
    for line in body.lines() {
        let t = line.trim();
        let q: Vec<&str> = t.split('"').collect();
        if t.starts_with("Entry {") {
            let variant = t.split("ViewKind::").nth(1).and_then(|r| r.split(',').next()).unwrap_or("");
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

fn render_table(version: &str, table: &Table) -> String {
    let mut s = format!(
        "/// The app version this table was cut for.\n#[cfg_attr(not(test), allow(dead_code, reason = \"only the table check in the tests reads it\"))]\npub const PIN_VERSION: &str = \"{version}\";\n\n#[rustfmt::skip]\nconst TABLE: &[Entry] = &[\n"
    );
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

fn host_pin(args: &[String]) -> Result<(), String> {
    let mut version = None;
    let mut out = repo().join("crates/manifold/src/host_pin.rs");
    let (mut keep, mut list, mut assets) = (None, None, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--version" => version = Some(val()?),
            "--out" => out = PathBuf::from(val()?),
            "--keep-from" => keep = Some(PathBuf::from(val()?)),
            "--list" => list = Some(PathBuf::from(val()?)),
            "--asset" => assets.push(val()?),
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    if let Some(path) = list {
        let text = read(&path)?;
        let (b, e) = region(&text).map_err(|m| format!("{}: {m}", path.display()))?;
        let body = &text[b..e];
        let v = body.split("PIN_VERSION: &str = \"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("");
        println!("version {v}");
        for ((kind, platform), files) in parse_table(body)? {
            for (file, (url, size, sha)) in files {
                println!("{kind} {platform} {file} {size} {sha} {url}");
            }
        }
        return Ok(());
    }
    let version = version.ok_or("--version is required")?;
    let mut table = Table::new();
    if let Some(k) = &keep {
        let text = read(k)?;
        let (b, e) = region(&text).map_err(|m| format!("{}: {m}", k.display()))?;
        table = parse_table(&text[b..e])?;
    }
    for spec in &assets {
        let (left, path) = spec.split_once('=').ok_or(format!("bad --asset {spec:?}: want KIND/PLATFORM=PATH"))?;
        let (kind, platform) = left.split_once('/').ok_or(format!("bad --asset {spec:?}: want KIND/PLATFORM=PATH"))?;
        if !KINDS.iter().any(|k| k.0 == kind) || !PLATFORMS.contains(&platform) {
            return Err(format!(
                "bad --asset {spec:?}: KIND is servo, cef, webkit or webview2; PLATFORM windows-x64 or linux-x64"
            ));
        }
        let path = Path::new(path);
        if !path.is_file() {
            return Err(format!("{}: no such file", path.display()));
        }
        let (sha, _, size) = hash_file(path)?;
        if size == 0 {
            return Err(format!("{}: empty file", path.display()));
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_owned();
        let files = table.entry((kind.to_owned(), platform.to_owned())).or_default();
        if files.contains_key(&name) {
            return Err(format!("{kind}/{platform}/{name} is both kept from the older table and given as an asset"));
        }
        eprintln!("{kind}/{platform} {name} {size} bytes sha256 {sha}");
        files.insert(name.clone(), (format!("{RELEASES}v{version}/{name}-{platform}"), size, sha));
    }
    let body = render_table(&version, &table);
    let text = read(&out)?;
    let (b, e) = region(&text).map_err(|m| format!("{}: {m}", out.display()))?;
    let new = format!("{}{body}{}", &text[..b], &text[e..]);
    let tmp = out.with_extension("rs.new");
    std::fs::write(&tmp, new).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    eprintln!("wrote {} ({} components)", out.display(), table.values().map(BTreeMap::len).sum::<usize>());
    println!("{}", hex(&Sha256::digest(body.as_bytes())));
    Ok(())
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
        let version = body.split("PIN_VERSION: &str = \"").nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(render_table(version, &parse_table(body).unwrap()), body);
        let cef = std::fs::read_to_string(repo().join("crates/manifold/src/cef_pin.rs")).unwrap();
        assert_eq!(render_cef(&blocks_of(&cef)), cef);
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
