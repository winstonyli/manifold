//! `gate cef-pin [--platform ID] PREFIX` (e.g. `152.0.6`) re-pins one CEF platform in crates/manifold-hosts/src/cef.rs:
//! it reads https://cef-builds.spotifycdn.com/index.json, picks that platform's `minimal` file whose cef_version
//! starts with `PREFIX+`, downloads it ONCE with curl (about 172 MB for windows64; running the command is the
//! confirmation, and the notice goes to stderr first) to a temp file, checks it against the index's size and sha1,
//! computes the SHA-256 (the index has none), deletes the temp file (`CEF_PIN_KEEP=1` keeps it), and replaces that
//! platform's `// BEGIN <id>` block in cef.rs. Nothing else in the file changes. A platform with no block yet is
//! refused before anything is fetched: it needs a block, a `pin` arm and an install layout added by hand. Platforms:
//! windows64 (default), linux64, macosx64, macosarm64. Re-pin every pinned platform to the same CEF build after a
//! `cef` crate bump (see `the_pin_matches_the_cef_crates_version` in manifold-hosts' tests/layout.rs).

use std::fmt::Write as _;
use std::process::{Command, ExitCode, Stdio};

use crate::fresh::repo;
use crate::release_hosts::hash_file;
use serde_json::Value;

// ---- cef.rs ----

const CEF_BASE: &str = "https://cef-builds.spotifycdn.com/";
const CEF_PLATFORMS: [&str; 4] = ["windows64", "linux64", "macosx64", "macosarm64"];

/// The file the pins live in, from the repo root.
const CEF_TABLE: &str = "crates/manifold-hosts/src/cef.rs";

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

/// The `// BEGIN id` .. `// END id` block of `text`, both marker lines included.
fn block_of<'a>(text: &'a str, id: &str) -> Option<&'a str> {
    let (begin, end) = (format!("// BEGIN {id}\n"), format!("// END {id}\n"));
    let start = text.find(&begin)?;
    let len = text[start..].find(&end)?;
    Some(&text[start..start + len + end.len()])
}

/// `text` with platform `id`'s block replaced by `block`, or `None` if it has none.
fn replace_block(text: &str, id: &str, block: &str) -> Option<String> {
    Some(text.replacen(block_of(text, id)?, block, 1))
}

/// Platform `id`'s block as `gate cef-pin` writes it.
fn render_block(id: &str, version: &str, url: &str, size: u64, sha1: &str, sha256: &str) -> String {
    let (upper, size) = (id.to_uppercase(), underscored(size));
    format!(
        "// BEGIN {id}\nconst PIN_{upper}: Pin = Pin {{\n    version: \"{version}\",\n    url: \"{url}\",\n    size: {size},\n    sha1: \"{sha1}\",\n    sha256: \"{sha256}\",\n}};\n// END {id}\n"
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
    let out = repo().join(CEF_TABLE);
    let text = std::fs::read_to_string(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    if block_of(&text, platform).is_none() {
        return Err(format!(
            "{CEF_TABLE} has no {platform} block: add one by hand, with its `pin` arm and an install layout"
        ));
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
    let block = render_block(platform, version, &url, size, sha1, &sha256);
    let table = replace_block(&text, platform, &block).ok_or_else(|| format!("no {platform} block in {CEF_TABLE}"))?;
    let tmp_out = out.with_extension("rs.new");
    std::fs::write(&tmp_out, table).map_err(|e| e.to_string())?;
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

pub fn cef_pin_main(args: &[String]) -> ExitCode {
    exit(cef_pin(args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifold_hosts::{cef, Os};

    /// Each pinned platform's block renders back to the same text and replacing it changes nothing, so a re-pin
    /// touches only what moved.
    #[test]
    fn the_checked_in_cef_table_round_trips() {
        let text = std::fs::read_to_string(repo().join(CEF_TABLE)).unwrap();
        for (id, os) in [("windows64", Os::Windows), ("linux64", Os::Linux)] {
            let pin = cef::pin(os).unwrap();
            let block = render_block(id, pin.version, pin.url, pin.size, pin.sha1, pin.sha256);
            assert_eq!(block_of(&text, id), Some(block.as_str()));
            assert_eq!(replace_block(&text, id, &block).as_deref(), Some(text.as_str()));
        }
    }

    /// Refused before the index is read, and says what to add.
    #[test]
    fn an_unknown_block_is_refused() {
        let args = ["--platform", "macosx64", "1.2.3"].map(str::to_owned);
        let refusal = cef_pin(&args).unwrap_err();
        assert!(refusal.contains("`pin` arm") && refusal.contains(CEF_TABLE), "{refusal}");
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
