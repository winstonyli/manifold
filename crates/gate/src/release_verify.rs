//! `gate release-verify vX.Y.Z [DIST]`: re-checks a built release with the tag checked out. Every host the tag's
//! pin manifest (`DIST/manifold-vX.Y.Z-windows-x64.host-pins.json`, the file the exe was built with) lists must sit in
//! DIST as `FILE-PLATFORM` with the listed size and SHA-256; the exe in DIST (default dist/) must match its .sha256;
//! and the embedded WebView2 host and PDF view copies in DIST/embedded must match the recorded hashes. That the exe
//! really contains those bytes and that manifest is not checked here (build.rs embeds them; `gate package` reads the
//! engine states the manifest produces).

use std::path::Path;
use std::process::ExitCode;

use crate::cef_pin::{hash_file, repo};
use crate::check::{check, exit_code, note_failure};
use crate::release_check::at_tag;

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
struct Pinned {
    kind: String,
    platform: String,
    file: String,
    size: u64,
    sha256: String,
}

/// Every file the manifest lists.
fn manifest_files(manifest: &str) -> Result<Vec<Pinned>, String> {
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

fn verify(tag: &str, dist: &Path) -> Result<(), String> {
    check(&format!("HEAD is the commit {tag} points at"), at_tag(tag));
    let stem = format!("manifold-{tag}-windows-x64");
    let manifest = std::fs::read_to_string(dist.join(format!("{stem}.host-pins.json")))
        .map_err(|e| format!("{stem}.host-pins.json: {e}"))?;
    for Pinned { kind, platform, file, size, sha256 } in manifest_files(&manifest)? {
        let path = dist.join(format!("{file}-{platform}"));
        let ok = hash_file(&path).is_ok_and(|(got, _, n)| got == sha256 && n == size);
        check(&format!("{kind}/{platform} {file} sits in dist with the pinned size and SHA-256"), ok);
    }
    let exe_name = format!("manifold-{tag}-windows-x64.exe");
    check(
        &format!("{exe_name} matches its .sha256"),
        matches_record(&dist.join(&exe_name), &dist.join(format!("{exe_name}.sha256"))),
    );
    check(
        "the embedded WebView2 host copy matches the recorded hash",
        matches_record(
            &dist.join("embedded/manifold-engine-webview2.exe"),
            &dist.join(format!("{stem}.embedded-host.sha256")),
        ),
    );
    check(
        "the embedded PDF view copy matches the recorded hash",
        matches_record(
            &dist.join("embedded/manifold-view-pdfium.exe"),
            &dist.join(format!("{stem}.embedded-pdf-view.sha256")),
        ),
    );
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
}
