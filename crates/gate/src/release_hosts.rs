//! `gate release-pins vX.Y.Z DIST FOREIGN` and `gate release-embedded vX.Y.Z DIST`: a release's host files, from the
//! host table (`crates/manifold-hosts`). The release's own platform is `Os::here()`, since the gate runs on the release
//! runner.
//! - `release-pins` copies every downloaded host (`Delivery::Download`) and its sidecars into DIST as `FILE-PLATFORM`
//!   and writes their pin manifest, `DIST/manifold-vX.Y.Z-<platform>.host-pins.json`, which manifold.exe is built with.
//!   A host built for this OS comes from `target/release/`, its sidecars from the newest build output under
//!   `target/release/build/` (`sync_deps::find_first`); a host built for another OS comes from FOREIGN (the
//!   `webkit-host` job's artifact).
//! - `release-embedded` copies every embedded host (`Delivery::Embedded`) into `DIST/embedded/` and writes its hash
//!   record, `DIST/<stem>.embedded-<id>.sha256` (`embedded_record`), which `release-verify` checks.
//!
//! Both print `NAME=path` lines on stdout, the build variables the job's later `cargo build -p manifold` reads, so the
//! release job appends them to `$GITHUB_ENV`. DIST and FOREIGN are relative to the repo root (the runner's working
//! directory); the printed paths are absolute. Each stage is a thin wrapper over a core that takes the root and returns
//! the lines, so the tests run the cores on temp trees.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use manifold_hosts::{Delivery, Host, Os};
use serde_json::{json, Map, Value};

use crate::cef_pin::{hash_file, repo};
use crate::sync_deps::find_first;

/// The hash record of an embedded host in a release's DIST, `<stem>.embedded-<id>.sha256`, where `stem` is
/// `manifold-vX.Y.Z-<platform>`. `release-embedded` writes it, the draft release uploads it, `release-verify` reads it.
pub(crate) fn embedded_record(stem: &str, host: &Host) -> String {
    format!("{stem}.embedded-{}.sha256", host.id)
}

/// `path` made absolute without `canonicalize`'s `\\?\` prefix, like the `Resolve-Path` values the job used before.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Each file a release pins for `host`, in `pinned_files`' order, with where it comes from: the file, or where it was
/// looked for. A host built for another OS comes whole from `foreign`; one built for `running` from `release`, its
/// sidecars from the newest build output under `release/build`.
fn sources(host: &Host, running: Os, release: &Path, foreign: &Path) -> Vec<(String, Result<PathBuf, String>)> {
    let at = |path: PathBuf| if path.is_file() { Ok(path) } else { Err(path.display().to_string()) };
    if host.built_for(running) != running {
        return host.pinned_files(running).into_iter().map(|file| (file.clone(), at(foreign.join(file)))).collect();
    }
    let file = host.file_name(running);
    let exe = release.join(&file);
    let mut out = vec![(file, at(exe))];
    let builds = [release.join("build")];
    for sidecar in host.sidecars_for(running) {
        let found = find_first(&builds, &sidecar.from)
            .ok_or_else(|| format!("{}'s out/{} under {}", sidecar.from.krate, sidecar.from.tail, builds[0].display()));
        out.push((sidecar.file.to_owned(), found));
    }
    out
}

/// `gate release-pins`' core: copies each downloaded host's files into `dist` as `<file>-<platform>` and writes their
/// pin manifest there, `manifold-<tag>-<platform>.host-pins.json`; returns the one line `MANIFOLD_HOST_PINS=<path>`.
/// `dist` and `foreign` are joined to `root` (an absolute one stays as it is). The first file not found is the error.
pub(crate) fn release_pins_in(root: &Path, tag: &str, dist: &Path, foreign: &Path) -> Result<Vec<String>, String> {
    let running = Os::here();
    let root = absolute(root)?;
    let (dist, foreign, release) = (root.join(dist), root.join(foreign), root.join("target").join("release"));
    fs::create_dir_all(&dist).map_err(|e| format!("{}: {e}", dist.display()))?;
    let mut manifest = Map::new();
    for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Download) {
        let platform = host.pin_platform(running);
        let mut files = Vec::new();
        for (file, source) in sources(host, running, &release, &foreign) {
            let source =
                source.map_err(|looked| format!("{}/{platform} {file}: not found (looked for {looked})", host.id))?;
            let copy = dist.join(format!("{file}-{platform}"));
            fs::copy(&source, &copy).map_err(|e| format!("{} -> {}: {e}", source.display(), copy.display()))?;
            let (sha256, _, size) = hash_file(&copy)?;
            files.push(json!({ "file": file, "size": size, "sha256": sha256 }));
        }
        let mut platforms = Map::new();
        platforms.insert(platform.to_owned(), Value::Array(files));
        manifest.insert(host.id.to_owned(), Value::Object(platforms));
    }
    let path = dist.join(format!("manifold-{tag}-{}.host-pins.json", running.pin_platform()));
    let text = serde_json::to_string_pretty(&Value::Object(manifest)).map_err(|e| e.to_string())?;
    fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(vec![format!("MANIFOLD_HOST_PINS={}", absolute(&path)?.display())])
}

/// `gate release-embedded`'s core: copies each embedded host from `target/release` into `dist/embedded/` and writes its
/// hash record in `dist` (`embedded_record`; `<sha256>  <file>`); returns one `<embed_var>=<copy>` line per host.
pub(crate) fn release_embedded_in(root: &Path, tag: &str, dist: &Path) -> Result<Vec<String>, String> {
    let running = Os::here();
    let root = absolute(root)?;
    let (dist, release) = (root.join(dist), root.join("target").join("release"));
    let embedded = dist.join("embedded");
    fs::create_dir_all(&embedded).map_err(|e| format!("{}: {e}", embedded.display()))?;
    let stem = format!("manifold-{tag}-{}", running.pin_platform());
    let mut lines = Vec::new();
    for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Embedded) {
        let file = host.file_name(running);
        let source = release.join(&file);
        if !source.is_file() {
            return Err(format!("{} {file}: not found (looked for {})", host.id, source.display()));
        }
        let copy = embedded.join(&file);
        fs::copy(&source, &copy).map_err(|e| format!("{} -> {}: {e}", source.display(), copy.display()))?;
        let (sha256, ..) = hash_file(&copy)?;
        let record = dist.join(embedded_record(&stem, host));
        fs::write(&record, format!("{sha256}  {file}\n")).map_err(|e| format!("{}: {e}", record.display()))?;
        lines.push(format!("{}={}", host.embed_var(), absolute(&copy)?.display()));
    }
    Ok(lines)
}

/// Prints a core's lines on stdout (the release job appends them to `$GITHUB_ENV`), or its error on stderr.
fn print_lines(stage: &str, result: Result<Vec<String>, String>) -> ExitCode {
    match result {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{stage}: {e}");
            ExitCode::from(1)
        }
    }
}

/// `gate release-pins vX.Y.Z DIST FOREIGN`: prints `MANIFOLD_HOST_PINS=<manifest>`, and the manifest itself on stderr
/// for the job's log.
pub fn release_pins(args: &[String]) -> ExitCode {
    let [tag, dist, foreign] = args else {
        eprintln!("usage: gate release-pins vX.Y.Z DIST FOREIGN");
        return ExitCode::from(2);
    };
    let result = release_pins_in(&repo(), tag, Path::new(dist), Path::new(foreign));
    for path in result.iter().flatten().filter_map(|line| line.strip_prefix("MANIFOLD_HOST_PINS=")) {
        match fs::read_to_string(path) {
            Ok(text) => eprint!("{text}"),
            Err(e) => eprintln!("{path}: {e}"),
        }
    }
    print_lines("release-pins", result)
}

/// `gate release-embedded vX.Y.Z DIST`: prints `<embed_var>=<copy>` per embedded host.
pub fn release_embedded(args: &[String]) -> ExitCode {
    let [tag, dist] = args else {
        eprintln!("usage: gate release-embedded vX.Y.Z DIST");
        return ExitCode::from(2);
    };
    print_lines("release-embedded", release_embedded_in(&repo(), tag, Path::new(dist)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_verify::{manifest_files, manifest_gaps, Pinned};

    const TAG: &str = "vX.Y.Z";

    /// A repo root with every host's release build under `target/release/` (Servo's ANGLE DLLs in mozangle's
    /// `build/mozangle/<hash>/out/`) and the Linux job's artifact under `webkit-host/`, each file holding its own name.
    /// No `dist/` yet.
    fn release_tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("gate-release-{name}-{}", std::process::id()));
        let root = std::path::absolute(root).unwrap();
        let _ = fs::remove_dir_all(&root);
        for file in [
            "target/release/manifold-view-servo.exe",
            "target/release/build/mozangle/aaa/out/libEGL.dll",
            "target/release/build/mozangle/aaa/out/libGLESv2.dll",
            "target/release/manifold-view-cef.exe",
            "target/release/manifold-view-webview2.exe",
            "target/release/manifold-view-pdfium.exe",
            "webkit-host/manifold-view-webkitgtk",
        ] {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, file.rsplit('/').next().unwrap()).unwrap();
        }
        root
    }

    #[test]
    fn release_pins_copies_each_downloaded_host_and_writes_its_manifest() {
        let root = release_tree("pins");
        let dist = root.join("dist");
        assert!(!dist.exists());
        let lines = release_pins_in(&root, TAG, &dist, &root.join("webkit-host")).unwrap();
        let manifest_path = dist.join("manifold-vX.Y.Z-windows-x64.host-pins.json");
        assert_eq!(lines, [format!("MANIFOLD_HOST_PINS={}", manifest_path.display())]);
        assert!(!lines[0].contains(r"\\?\"));
        let manifest: Value = serde_json::from_str(&fs::read_to_string(manifest_path).unwrap()).unwrap();
        let ids: Vec<&str> = manifest.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(ids, ["servo", "cef", "webkit"]);
        let pinned = [
            ("servo", "windows-x64", "manifold-view-servo.exe"),
            ("servo", "windows-x64", "libEGL.dll"),
            ("servo", "windows-x64", "libGLESv2.dll"),
            ("cef", "windows-x64", "manifold-view-cef.exe"),
            ("webkit", "linux-x64", "manifold-view-webkitgtk"),
        ];
        for (id, platform, file) in pinned {
            let copy = dist.join(format!("{file}-{platform}"));
            assert_eq!(fs::read_to_string(&copy).unwrap(), file, "{}", copy.display());
            let (sha256, _, size) = hash_file(&copy).unwrap();
            assert_eq!(size, file.len() as u64);
            let entry = manifest[id][platform].as_array().unwrap().iter().find(|e| e["file"] == file);
            let entry = entry.unwrap_or_else(|| panic!("{id}/{platform} lists no {file}"));
            assert_eq!(entry["size"], json!(size), "{id}/{platform} {file}");
            assert_eq!(entry["sha256"], json!(sha256), "{id}/{platform} {file}");
        }
        let platforms = manifest.as_object().unwrap().values().flat_map(|p| p.as_object().unwrap().values());
        assert_eq!(platforms.map(|files| files.as_array().unwrap().len()).sum::<usize>(), pinned.len());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_host_file_is_an_error_naming_it_and_where_it_was_looked_for() {
        let root = release_tree("missing");
        let (dist, foreign) = (root.join("dist"), root.join("webkit-host"));
        let release = root.join("target").join("release");
        fs::remove_file(release.join("build/mozangle/aaa/out/libGLESv2.dll")).unwrap();
        let looked = format!("mozangle's out/libGLESv2.dll under {}", release.join("build").display());
        let want = format!("servo/windows-x64 libGLESv2.dll: not found (looked for {looked})");
        assert_eq!(release_pins_in(&root, TAG, &dist, &foreign), Err(want));
        fs::remove_file(release.join("manifold-view-servo.exe")).unwrap();
        let looked = release.join("manifold-view-servo.exe").display().to_string();
        let want = format!("servo/windows-x64 manifold-view-servo.exe: not found (looked for {looked})");
        assert_eq!(release_pins_in(&root, TAG, &dist, &foreign), Err(want));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_host_built_for_another_os_comes_from_foreign() {
        let root = release_tree("foreign");
        let (dist, foreign) = (root.join("dist"), root.join("webkit-host"));
        // A file of the same name in this OS's target/release is not the Linux job's build.
        fs::write(root.join("target").join("release").join("manifold-view-webkitgtk"), "not this one").unwrap();
        release_pins_in(&root, TAG, &dist, &foreign).unwrap();
        let copied = fs::read_to_string(dist.join("manifold-view-webkitgtk-linux-x64")).unwrap();
        assert_eq!(copied, "manifold-view-webkitgtk");
        let elf = foreign.join("manifold-view-webkitgtk");
        fs::remove_file(&elf).unwrap();
        let want = format!("webkit/linux-x64 manifold-view-webkitgtk: not found (looked for {})", elf.display());
        assert_eq!(release_pins_in(&root, TAG, &dist, &foreign), Err(want));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn release_embedded_copies_each_embedded_host_and_records_its_hash() {
        let root = release_tree("embedded");
        let dist = root.join("dist");
        let lines = release_embedded_in(&root, TAG, &dist).unwrap();
        let webview2 = dist.join("embedded").join("manifold-view-webview2.exe");
        let pdfium = dist.join("embedded").join("manifold-view-pdfium.exe");
        let want = [
            format!("MANIFOLD_WEBVIEW2_HOST={}", webview2.display()),
            format!("MANIFOLD_PDFIUM_HOST={}", pdfium.display()),
        ];
        assert_eq!(lines, want);
        assert!(lines.iter().all(|line| !line.contains(r"\\?\")));
        let records = [
            (&webview2, "manifold-vX.Y.Z-windows-x64.embedded-webview2.sha256"),
            (&pdfium, "manifold-vX.Y.Z-windows-x64.embedded-pdfium.sha256"),
        ];
        for (copy, record) in records {
            let file = copy.file_name().unwrap().to_str().unwrap();
            assert_eq!(fs::read_to_string(copy).unwrap(), file);
            let (sha256, ..) = hash_file(copy).unwrap();
            assert_eq!(fs::read_to_string(dist.join(record)).unwrap(), format!("{sha256}  {file}\n"));
        }
        let source = root.join("target").join("release").join("manifold-view-pdfium.exe");
        fs::remove_file(&source).unwrap();
        let want = format!("pdfium manifold-view-pdfium.exe: not found (looked for {})", source.display());
        assert_eq!(release_embedded_in(&root, TAG, &dist), Err(want));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn embedded_record_names_the_files_the_release_uploads() {
        // Literal: release-embedded and release-verify both call embedded_record, so neither can catch a wrong name.
        let stem = "manifold-vX.Y.Z-windows-x64";
        let webview2 = embedded_record(stem, &manifold_hosts::WEBVIEW2);
        assert_eq!(webview2, "manifold-vX.Y.Z-windows-x64.embedded-webview2.sha256");
        let pdfium = embedded_record(stem, &manifold_hosts::PDFIUM);
        assert_eq!(pdfium, "manifold-vX.Y.Z-windows-x64.embedded-pdfium.sha256");
    }

    /// The manifest `release_pins_in` writes for a `release_tree`, parsed as `release-verify` parses it.
    fn written_manifest(name: &str) -> Vec<Pinned> {
        let root = release_tree(name);
        let lines = release_pins_in(&root, TAG, &root.join("dist"), &root.join("webkit-host")).unwrap();
        let path = lines[0].strip_prefix("MANIFOLD_HOST_PINS=").unwrap();
        let entries = manifest_files(&fs::read_to_string(path).unwrap()).unwrap();
        let _ = fs::remove_dir_all(&root);
        entries
    }

    fn sorted(mut files: Vec<String>) -> Vec<String> {
        files.sort_unstable();
        files
    }

    #[test]
    fn the_written_manifest_has_no_gap() {
        // The round trip: what release-pins writes is what release-verify accepts, so the two cannot drift.
        let entries = written_manifest("round-trip");
        assert_eq!(entries.len(), 5);
        assert_eq!(manifest_gaps(&entries, Os::here()), Vec::<String>::new());
    }

    #[test]
    fn every_entry_of_the_written_manifest_is_one_the_app_accepts() {
        // Spec R6: a manifest manifest_gaps passes also passes host_pin::parse, whose check is Host::pinned_under.
        let entries = written_manifest("agreement");
        assert_eq!(entries.len(), 5);
        for e in &entries {
            let host = manifold_hosts::by_id(&e.kind).unwrap_or_else(|| panic!("no row {}", e.kind));
            let same = entries.iter().filter(|o| o.kind == e.kind && o.platform == e.platform);
            let want = sorted(same.map(|o| o.file.clone()).collect());
            assert_eq!(host.pinned_under(&e.platform).map(sorted), Some(want), "{}/{}", e.kind, e.platform);
        }
    }

    #[test]
    fn the_draft_release_uploads_every_embedded_record() {
        // The upload line first runs on a real tag (spec H14), so its glob is tied to embedded_record here.
        let yml = fs::read_to_string(repo().join(".github").join("workflows").join("release.yml")).unwrap();
        let line = yml.lines().find(|line| line.contains("gh release create")).expect("no gh release create line");
        let stem = "manifold-vX.Y.Z-windows-x64";
        let glob = line
            .split_once("-Filter \"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(glob, _)| glob.replace("$stem", stem))
            .unwrap_or_else(|| panic!("no -Filter \"...\" on: {line}"));
        let (left, right) = glob.split_once('*').unwrap_or_else(|| panic!("no * in the -Filter glob {glob}"));
        let matches = |name: &str| name.strip_prefix(left).is_some_and(|rest| rest.ends_with(right));
        for host in manifold_hosts::ALL.iter().filter(|host| host.delivery == Delivery::Embedded) {
            let record = embedded_record(stem, host);
            assert!(matches(record.as_str()), "{glob} does not match {record}");
        }
        assert!(!matches("manifold-vX.Y.Z-windows-x64.exe.sha256"), "{glob} matches the exe's record too");
    }
}
