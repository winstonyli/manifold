//! `gate release-verify vX.Y.Z [DIST]`: re-checks a built release (which commit it was built from is
//! `release-check`'s job). The release's platform is `Os::here()`, since the gate runs on the release runner, and its
//! files are named from the stem `manifold-vX.Y.Z-<platform>`. The pin manifest (`DIST/<stem>.host-pins.json`, the
//! file the exe was built with) must pin exactly the host table's files for that platform (`manifest_gaps`), and every
//! file it lists must sit in DIST (default dist/) as `FILE-PLATFORM` with the listed size and SHA-256; the exe in DIST
//! must match its .sha256; and each embedded host's copy in DIST/embedded must match its hash record
//! (`release_hosts::embedded_record`). That the exe really contains those bytes and that manifest is not checked here
//! (build.rs embeds them; `gate package` reads the engine states the manifest produces).

use std::path::Path;
use std::process::ExitCode;

use manifold_hosts::{Delivery, Os};

use crate::cef_pin::{hash_file, repo};
use crate::check::{check, check_is, exit_code, note_failure};
use crate::release_hosts::embedded_record;

/// The first whitespace-separated word of a hash record file.
fn recorded(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()?.split_whitespace().next().map(str::to_owned)
}

/// `file`'s SHA-256 equals the hash recorded in `record`.
fn matches_record(file: &Path, record: &Path) -> bool {
    matches!((hash_file(file), recorded(record)), (Ok((sha, ..)), Some(want)) if sha == want)
}

/// One file the pin manifest lists.
#[derive(Debug, PartialEq)]
pub(crate) struct Pinned {
    pub(crate) kind: String,
    pub(crate) platform: String,
    pub(crate) file: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

/// Every file the manifest lists.
pub(crate) fn manifest_files(manifest: &str) -> Result<Vec<Pinned>, String> {
    let root: serde_json::Value = serde_json::from_str(manifest).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (kind, platforms) in root.as_object().ok_or("the manifest is not an object")? {
        for (platform, files) in platforms.as_object().ok_or("an engine is not an object")? {
            for f in files.as_array().ok_or("a platform is not a list")? {
                let text = |key: &str| f[key].as_str().map(str::to_owned).ok_or(format!("{kind}/{platform}: no {key}"));
                let size = f["size"].as_u64().ok_or(format!("{kind}/{platform}: no size"))?;
                out.push(Pinned {
                    kind: kind.clone(),
                    platform: platform.clone(),
                    file: text("file")?,
                    size,
                    sha256: text("sha256")?,
                });
            }
        }
    }
    Ok(out)
}

/// What keeps `entries` (a parsed manifest) from pinning exactly the host table's files for a release built on
/// `running`: each file a `Download` row pins under its platform that the manifest lacks, then each file the manifest
/// lists that no row pins there. Stricter than `Host::pinned_under`, the check `host_pin::parse` makes: a host under
/// another OS's platform (`cef` under `linux-x64` in a Windows release) is a gap here.
pub(crate) fn manifest_gaps(entries: &[Pinned], running: Os) -> Vec<String> {
    let mut expected = Vec::new();
    for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Download) {
        for file in host.pinned_files(running) {
            expected.push((host.id, host.pin_platform(running), file));
        }
    }
    let mut gaps = Vec::new();
    for (id, platform, file) in &expected {
        if !entries.iter().any(|e| e.kind == *id && e.platform == *platform && e.file == *file) {
            gaps.push(format!("{id}/{platform} {file}: missing from the manifest"));
        }
    }
    for e in entries {
        if !expected.iter().any(|(id, platform, file)| e.kind == *id && e.platform == *platform && e.file == *file) {
            gaps.push(format!("{}/{} {}: not pinned by the host table", e.kind, e.platform, e.file));
        }
    }
    gaps
}

fn verify(tag: &str, dist: &Path) -> Result<(), String> {
    let running = Os::here();
    let stem = format!("manifold-{tag}-{}", running.pin_platform());
    let manifest = std::fs::read_to_string(dist.join(format!("{stem}.host-pins.json")))
        .map_err(|e| format!("{stem}.host-pins.json: {e}"))?;
    let entries = manifest_files(&manifest)?;
    check_is(
        "the manifest pins exactly the host table's files for this release's platform",
        Vec::<String>::new(),
        manifest_gaps(&entries, running),
    );
    for Pinned { kind, platform, file, size, sha256 } in entries {
        let path = dist.join(format!("{file}-{platform}"));
        let ok = hash_file(&path).is_ok_and(|(got, _, n)| got == sha256 && n == size);
        check(&format!("{kind}/{platform} {file} sits in dist with the pinned size and SHA-256"), ok);
    }
    let exe_name = format!("{stem}{}", running.exe_suffix());
    check(
        &format!("{exe_name} matches its .sha256"),
        matches_record(&dist.join(&exe_name), &dist.join(format!("{exe_name}.sha256"))),
    );
    for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Embedded) {
        let (file, record) = (host.file_name(running), embedded_record(&stem, host));
        let label = format!("embedded/{file} matches {record}");
        check(&label, matches_record(&dist.join("embedded").join(file), &dist.join(record)));
    }
    Ok(())
}

pub fn release_verify(args: &[String]) -> ExitCode {
    let (tag, dist) = match args {
        [tag] => (tag, repo().join("dist")),
        [tag, dist] => (tag, Path::new(dist).to_path_buf()),
        _ => {
            eprintln!("usage: gate release-verify vX.Y.Z [DIST]");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = verify(tag, &dist) {
        eprintln!("{e}");
        note_failure();
    }
    exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_lists_every_file_of_every_engine_and_platform() {
        let m = r#"{"cef": {"windows-x64": [{"file": "a.exe", "size": 3, "sha256": "ab"}]},
                    "webkit": {"linux-x64": [{"file": "w", "size": 9, "sha256": "cd"}]}}"#;
        let files = manifest_files(m).unwrap();
        assert_eq!(files.len(), 2);
        let first = Pinned {
            kind: "cef".into(),
            platform: "windows-x64".into(),
            file: "a.exe".into(),
            size: 3,
            sha256: "ab".into(),
        };
        assert_eq!(files[0], first);
        assert!(manifest_files(r#"{"cef": {"windows-x64": [{"file": "a.exe"}]}}"#).is_err());
        assert!(manifest_files("[]").is_err());
    }

    fn pinned(kind: &str, platform: &str, file: &str) -> Pinned {
        Pinned { kind: kind.into(), platform: platform.into(), file: file.into(), size: 1, sha256: "00".into() }
    }

    /// What a Windows release pins, per the host table: Servo with its two ANGLE DLLs, CEF, and WebKit's Linux ELF.
    fn windows_release() -> Vec<Pinned> {
        vec![
            pinned("servo", "windows-x64", "manifold-view-servo.exe"),
            pinned("servo", "windows-x64", "libEGL.dll"),
            pinned("servo", "windows-x64", "libGLESv2.dll"),
            pinned("cef", "windows-x64", "manifold-view-cef.exe"),
            pinned("webkit", "linux-x64", "manifold-view-webkitgtk"),
        ]
    }

    #[test]
    fn a_manifest_that_differs_from_the_host_table_has_a_gap_per_file() {
        let windows = Os::Windows;
        assert_eq!(manifest_gaps(&windows_release(), windows), Vec::<String>::new());
        let without = |file: &str| windows_release().into_iter().filter(|e| e.file != file).collect::<Vec<_>>();
        let missing_host = manifest_gaps(&without("manifold-view-cef.exe"), windows);
        assert_eq!(missing_host, ["cef/windows-x64 manifold-view-cef.exe: missing from the manifest"]);
        let missing_sidecar = manifest_gaps(&without("libEGL.dll"), windows);
        assert_eq!(missing_sidecar, ["servo/windows-x64 libEGL.dll: missing from the manifest"]);
        let mut extra = windows_release();
        extra.push(pinned("servo", "windows-x64", "extra.dll"));
        assert_eq!(manifest_gaps(&extra, windows), ["servo/windows-x64 extra.dll: not pinned by the host table"]);
        let mut moved = without("manifold-view-webkitgtk");
        moved.push(pinned("webkit", "windows-x64", "manifold-view-webkitgtk"));
        let want = [
            "webkit/linux-x64 manifold-view-webkitgtk: missing from the manifest",
            "webkit/windows-x64 manifold-view-webkitgtk: not pinned by the host table",
        ];
        assert_eq!(manifest_gaps(&moved, windows), want);
        // CEF is built for whichever OS runs it, so pinned_under alone (host_pin::parse's check) accepts it under
        // linux-x64; a Windows release does not pin it there.
        assert_eq!(manifold_hosts::CEF.pinned_under("linux-x64"), Some(vec!["manifold-view-cef".to_owned()]));
        let mut linux_cef = windows_release();
        linux_cef.push(pinned("cef", "linux-x64", "manifold-view-cef"));
        let linux_gaps = manifest_gaps(&linux_cef, windows);
        assert_eq!(linux_gaps, ["cef/linux-x64 manifold-view-cef: not pinned by the host table"]);
    }
}
