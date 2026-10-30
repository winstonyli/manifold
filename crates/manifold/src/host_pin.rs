//! The engine host binaries the app downloads on demand, one list per (engine, platform).
//!
//! A release carries every host next to `manifold.exe` (`release.yml` builds them first), and the exe is built with
//! `MANIFOLD_HOST_PINS` naming a manifest of their names, sizes and SHA-256 hashes. `build.rs` embeds that manifest;
//! the hash compiled into the exe is the trust anchor, the URL is not trusted. A host's URL is its own release's:
//! `releases/download/v<this exe's version>/<file>-<platform>`. A build with no manifest (a dev build, the gate)
//! pins nothing, so its engines read Unavailable unless a host built in place (`run_in_place`: its dev build folder,
//! or beside `manifold.exe`) speaks this build's protocol; a build with one never looks in a dev build folder
//! (`walks_dev_folders`). A debug build also reads the file named by `MANIFOLD_HOST_PINS_FILE`, which is how the gate
//! shows the download states.
//!
//! The manifest is `{"cef": {"windows-x64": [{"file": "manifold-view-cef.exe", "size": 123, "sha256": "..."}]}}`
//! with the host's id (`manifold_hosts::Host::id`, the engine's store name) as the first key, and under each platform
//! exactly the files the host table pins there (`Host::pinned_under`).

use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::process_manager::ViewKind;

/// One downloadable host file.
#[derive(Debug, PartialEq, Eq)]
pub struct Component {
    /// The file name it installs under (`manifold-view-cef.exe`).
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

/// Whether this exe was built with a manifest (a release). `MANIFOLD_HOST_PINS_FILE` does not count: a dev build that
/// reads one is still a dev build.
pub fn embedded() -> bool {
    !EMBEDDED.trim().is_empty()
}

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
/// malformed hash, a zero size or a path in a file name is an error (the whole manifest is refused), and so is an entry
/// whose files are not, as a set, the ones the host table pins for that host under that platform (`Host::pinned_under`:
/// none for a host that is not downloaded, a platform no host is pinned for, or another OS's platform). The check reads
/// the platform string, not the OS this runs on, so a manifest parses the same on Windows and Linux.
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
            let Some(pinned) = kind.desc().host.pinned_under(platform) else {
                return Err(format!("{who}: the host table pins no files there"));
            };
            let want: BTreeSet<&str> = pinned.iter().map(String::as_str).collect();
            let got: BTreeSet<&str> = components.iter().map(|c| c.file).collect();
            if got != want {
                return Err(format!("{who}: lists {got:?}, the host table pins {want:?}"));
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
    use manifold_hosts::{Os, CEF, SERVO, WEBKIT, WEBVIEW2};

    const HASH: &str = "db3e0751979c3cb3068f732ed4f0ef12ade69d183d98ceee120ad08c8d00f74c";

    fn manifest(file: &str, size: u64, sha: &str) -> String {
        format!(r#"{{"cef": {{"windows-x64": [{{"file": "{file}", "size": {size}, "sha256": "{sha}"}}]}}}}"#)
    }

    #[test]
    fn the_embedded_manifest_is_well_formed() {
        let entries = parse(EMBEDDED, "0.0.0").unwrap();
        assert_eq!(entries.is_empty(), !embedded());
    }

    #[test]
    fn no_manifest_pins_nothing() {
        assert!(parse("", "1.2.3").unwrap().is_empty());
        assert!(parse(" \n", "1.2.3").unwrap().is_empty());
        assert!(components_for(ViewKind::Cef, "plan9-x64").is_empty());
    }

    #[test]
    fn a_host_is_fetched_from_its_own_releases_download() {
        let exe = CEF.file_name(Os::Windows);
        let entries = parse(&manifest(&exe, 740_000, HASH), "1.2.3").unwrap();
        let table: &'static [Entry] = Box::leak(entries.into_boxed_slice());
        let c = &lookup(table, ViewKind::Cef, "windows-x64")[0];
        assert_eq!(c.file, exe);
        assert_eq!(c.url, format!("https://github.com/winstonyli/manifold/releases/download/v1.2.3/{exe}-windows-x64"));
        assert_eq!((c.size, c.sha256), (740_000, HASH));
        assert!(lookup(table, ViewKind::Cef, "linux-x64").is_empty());
        assert!(lookup(table, ViewKind::Servo, "windows-x64").is_empty());
    }

    #[test]
    fn a_bad_manifest_is_refused_whole() {
        let exe = CEF.file_name(Os::Windows);
        assert!(parse(&manifest(&exe, 1, HASH), "1.2.3").is_ok(), "each case below is this one manifest, broken once");
        for bad in [
            manifest(&exe, 1, ""),
            manifest(&exe, 1, &HASH.to_uppercase()),
            manifest(&exe, 1, &HASH[1..]),
            manifest(&exe, 1, &HASH.replace('d', "g")),
            manifest(&exe, 0, HASH),
            manifest("", 1, HASH),
            manifest("../x.exe", 1, HASH),
            manifest("a\\\\b.exe", 1, HASH), // JSON `a\\b.exe`, the name a\b.exe
            manifest(&exe, 1, HASH).replace("cef", "nosuch"),
            "[]".to_owned(),
            "{".to_owned(),
        ] {
            assert!(parse(&bad, "1.2.3").is_err(), "accepted {bad}");
        }
    }

    /// Each entry lists exactly the files the host table pins for that host under that platform (`Host::pinned_under`),
    /// whatever OS parses it: a host without one of its sidecars, a host that is not downloaded, a platform no host is
    /// pinned for and a host under another OS's platform each refuse the whole manifest.
    #[test]
    fn a_manifest_must_list_exactly_what_the_host_table_pins() {
        let files = |names: &[String]| -> serde_json::Value {
            names.iter().map(|file| serde_json::json!({"file": file, "size": 1, "sha256": HASH})).collect()
        };
        let servo = SERVO.pinned_under("windows-x64").expect("Servo is pinned for Windows");
        let no_egl: Vec<String> = servo.iter().filter(|file| file.as_str() != "libEGL.dll").cloned().collect();
        assert_eq!(no_egl.len() + 1, servo.len(), "Servo's Windows host has libEGL.dll beside it: {servo:?}");
        let without_egl = serde_json::json!({"servo": {"windows-x64": files(&no_egl)}});
        let err = parse(&without_egl.to_string(), "1.2.3").unwrap_err();
        let listed: std::collections::BTreeSet<&str> = no_egl.iter().map(String::as_str).collect();
        let pinned: std::collections::BTreeSet<&str> = servo.iter().map(String::as_str).collect();
        assert_eq!(err, format!("servo/windows-x64: lists {listed:?}, the host table pins {pinned:?}"));
        let refused = [
            ("webview2", "windows-x64", files(&[WEBVIEW2.file_name(Os::Windows)])),
            ("cef", "plan9-x64", files(&CEF.pinned_files(Os::Windows))),
            ("webkit", "windows-x64", files(&WEBKIT.pinned_files(Os::Linux))),
        ];
        for (id, platform, listed) in refused {
            let err = parse(&serde_json::json!({ id: { platform: listed } }).to_string(), "1.2.3").unwrap_err();
            assert_eq!(err, format!("{id}/{platform}: the host table pins no files there"));
        }
        let cef = files(&CEF.pinned_files(Os::Windows));
        let webkit = files(&WEBKIT.pinned_files(Os::Linux));
        let both = serde_json::json!({"cef": {"windows-x64": cef}, "webkit": {"linux-x64": webkit}});
        let entries = parse(&both.to_string(), "1.2.3").unwrap();
        let got: Vec<(ViewKind, &str)> = entries.iter().map(|e| (e.kind, e.platform)).collect();
        assert_eq!(got, [(ViewKind::Cef, "windows-x64"), (ViewKind::Webkit, "linux-x64")]);
    }
}
