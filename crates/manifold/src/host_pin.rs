//! The engine host binaries the app downloads on demand, one list per (engine, platform).
//!
//! A release carries every host next to `manifold.exe` (`release.yml` builds them first), and the exe is built with
//! `MANIFOLD_HOST_PINS` naming a manifest of their names, sizes and SHA-256 hashes. `build.rs` embeds that manifest;
//! the hash compiled into the exe is the trust anchor, the URL is not trusted. A host's URL is its own release's:
//! `releases/download/v<this exe's version>/<file>-<platform>`. A build with no manifest (a dev build, the gate)
//! pins nothing, so its engines read Unavailable unless a sibling host exists. A debug build also reads the file named
//! by `MANIFOLD_HOST_PINS_FILE`, which is how the gate shows the download states.
//!
//! The manifest is `{"cef": {"windows-x64": [{"file": "manifold-engine-cef.exe", "size": 123, "sha256": "..."}]}}`
//! with the engine's store name (`ViewKind::desc().name`) as the first key.

use std::sync::OnceLock;

use crate::process_manager::ViewKind;

/// One downloadable host file.
#[derive(Debug, PartialEq, Eq)]
pub struct Component {
    /// The file name it installs under (`manifold-engine-cef.exe`).
    pub file: &'static str,
    /// Where it is fetched from: `https://github.com/winstonyli/manifold/releases/download/v<ver>/<file>-<platform>`.
    /// Untrusted: only `sha256` is the trust anchor.
    pub url: &'static str,
    pub size: u64,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: &'static str,
}

/// The components of one engine on one platform (`windows-x64` or `linux-x64`).
#[derive(Debug)]
pub struct Entry {
    pub kind: ViewKind,
    pub platform: &'static str,
    pub components: &'static [Component],
}

const RELEASES: &str = "https://github.com/winstonyli/manifold/releases/download/";

/// The manifest `build.rs` embedded; empty when `MANIFOLD_HOST_PINS` was not set.
const EMBEDDED: &str = include_str!(concat!(env!("OUT_DIR"), "/host-pins.json"));

/// The components `kind` needs on `platform`, or an empty slice when this build pins none.
pub fn components_for(kind: ViewKind, platform: &str) -> &'static [Component] {
    lookup(table(), kind, platform)
}

fn table() -> &'static [Entry] {
    static TABLE: OnceLock<Vec<Entry>> = OnceLock::new();
    TABLE.get_or_init(|| {
        parse(&manifest_text(), env!("CARGO_PKG_VERSION")).unwrap_or_else(|e| {
            eprintln!("host pins ignored: {e}");
            Vec::new()
        })
    })
}

#[cfg(debug_assertions)]
fn manifest_text() -> String {
    std::env::var_os("MANIFOLD_HOST_PINS_FILE")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_else(|| EMBEDDED.to_owned())
}

#[cfg(not(debug_assertions))]
fn manifest_text() -> String {
    EMBEDDED.to_owned()
}

fn lookup(table: &'static [Entry], kind: ViewKind, platform: &str) -> &'static [Component] {
    table.iter().find(|e| e.kind == kind && e.platform == platform).map_or(&[], |e| e.components)
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

/// The entries `manifest` lists, with each URL under `v<version>`'s release. Blank text is an empty table; a
/// malformed hash, a zero size or a path in a file name is an error (the whole manifest is refused).
fn parse(manifest: &str, version: &str) -> Result<Vec<Entry>, String> {
    if manifest.trim().is_empty() {
        return Ok(Vec::new());
    }
    let root: serde_json::Value = serde_json::from_str(manifest).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for (name, platforms) in root.as_object().ok_or("the manifest is not an object")? {
        let kind = ViewKind::from_name(name).ok_or_else(|| format!("unknown engine {name:?}"))?;
        for (platform, files) in platforms.as_object().ok_or_else(|| format!("{name}: not an object"))? {
            let who = format!("{name}/{platform}");
            let mut components = Vec::new();
            for f in files.as_array().ok_or_else(|| format!("{who}: not a list"))? {
                let file = f["file"]
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.contains(['/', '\\']))
                    .ok_or_else(|| format!("{who}: bad file name"))?;
                let size = f["size"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("{who} {file}: size is not a positive number"))?;
                let sha256 = f["sha256"]
                    .as_str()
                    .filter(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
                    .ok_or_else(|| format!("{who} {file}: sha256 is not 64 lowercase hex digits"))?;
                let url = leak(&format!("{RELEASES}v{version}/{file}-{platform}"));
                components.push(Component { file: leak(file), url, size, sha256: leak(sha256) });
            }
            let components = Box::leak(components.into_boxed_slice());
            entries.push(Entry { kind, platform: leak(platform), components });
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "db3e0751979c3cb3068f732ed4f0ef12ade69d183d98ceee120ad08c8d00f74c";

    fn manifest(file: &str, size: u64, sha: &str) -> String {
        format!(r#"{{"cef": {{"windows-x64": [{{"file": "{file}", "size": {size}, "sha256": "{sha}"}}]}}}}"#)
    }

    #[test]
    fn the_embedded_manifest_is_well_formed() {
        parse(EMBEDDED, "0.0.0").unwrap();
    }

    #[test]
    fn no_manifest_pins_nothing() {
        assert!(parse("", "1.2.3").unwrap().is_empty());
        assert!(parse(" \n", "1.2.3").unwrap().is_empty());
        assert!(components_for(ViewKind::Cef, "plan9-x64").is_empty());
    }

    #[test]
    fn a_host_is_fetched_from_its_own_releases_download() {
        let entries = parse(&manifest("manifold-engine-cef.exe", 740_000, HASH), "1.2.3").unwrap();
        let table: &'static [Entry] = Box::leak(entries.into_boxed_slice());
        let c = &lookup(table, ViewKind::Cef, "windows-x64")[0];
        assert_eq!(c.file, "manifold-engine-cef.exe");
        assert_eq!(
            c.url,
            "https://github.com/winstonyli/manifold/releases/download/v1.2.3/manifold-engine-cef.exe-windows-x64"
        );
        assert_eq!((c.size, c.sha256), (740_000, HASH));
        assert!(lookup(table, ViewKind::Cef, "linux-x64").is_empty());
        assert!(lookup(table, ViewKind::Servo, "windows-x64").is_empty());
    }

    #[test]
    fn a_bad_manifest_is_refused_whole() {
        for bad in [
            manifest("manifold-engine-cef.exe", 1, ""),
            manifest("manifold-engine-cef.exe", 1, &HASH.to_uppercase()),
            manifest("manifold-engine-cef.exe", 1, &HASH[1..]),
            manifest("manifold-engine-cef.exe", 1, &HASH.replace('d', "g")),
            manifest("manifold-engine-cef.exe", 0, HASH),
            manifest("", 1, HASH),
            manifest("../x.exe", 1, HASH),
            manifest("a\\\\b.exe", 1, HASH), // JSON `a\\b.exe`, the name a\b.exe
            manifest("manifold-engine-cef.exe", 1, HASH).replace("cef", "nosuch"),
            "[]".to_owned(),
            "{".to_owned(),
        ] {
            assert!(parse(&bad, "1.2.3").is_err(), "accepted {bad}");
        }
    }
}
