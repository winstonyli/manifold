//! The table against the repo's own files: the host crates' names, the ignored build folders, the release workflow,
//! and who may depend on the table. Plain substring matching on whole lines, leading whitespace trimmed and `#`
//! comment lines skipped: the leaf has no TOML, YAML or regex parser.

use manifold_hosts::{Delivery, Os, ALL};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The repo root: this crate is `crates/manifold-hosts`.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// The lines that carry content: leading whitespace trimmed, blank and `#` comment lines dropped.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim_start).filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// A manifest's header lines (those starting `[`), each with its content lines up to the next header.
fn sections(text: &str) -> Vec<(&str, Vec<&str>)> {
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in lines(text) {
        if line.starts_with('[') {
            sections.push((line, Vec::new()));
        } else if let Some((_, body)) = sections.last_mut() {
            body.push(line);
        }
    }
    sections
}

/// Whether `line` holds `needle` followed by the line's end, whitespace or a quote, so a longer name never counts.
fn has_word(line: &str, needle: &str) -> bool {
    line.match_indices(needle).any(|(at, _)| {
        line[at + needle.len()..].chars().next().is_none_or(|next| next.is_whitespace() || next == '"' || next == '\'')
    })
}

fn release_yml() -> String {
    read(&repo().join(".github").join("workflows").join("release.yml"))
}

/// `crates/<exe>/Cargo.toml` has `name = "<exe>"` under `[package]`, and under each `[[bin]]` too.
#[test]
fn every_host_crate_is_named_for_its_exe() {
    for host in ALL {
        let path = repo().join("crates").join(host.exe).join("Cargo.toml");
        let text = read(&path);
        let name = format!("name = \"{}\"", host.exe);
        let mut packages = 0;
        for (header, body) in sections(&text) {
            let names: Vec<&str> =
                body.iter().copied().filter(|line| line.starts_with("name ") || line.starts_with("name=")).collect();
            if header == "[package]" {
                packages += 1;
            }
            if header == "[package]" || header == "[[bin]]" {
                assert_eq!(names, [name.as_str()], "{}: {header}", path.display());
            }
        }
        assert_eq!(packages, 1, "{}: one [package]", path.display());
    }
}

/// Every dev build folder is ignored: `.gitignore` has a `/<dev_dir>` line.
#[test]
fn every_dev_folder_is_ignored() {
    let text = read(&repo().join(".gitignore"));
    for host in ALL {
        let Some(dev) = host.dev_dir else { continue };
        let ignored = format!("/{dev}");
        assert!(lines(&text).any(|line| line.trim_end() == ignored), ".gitignore has no {ignored} line ({})", host.id);
    }
}

/// `release.yml` builds every host (`-p <exe>`), and its Linux job takes each host built for Linux from
/// `target/release/<file>` (H10: the two literals the gate cannot write there).
#[test]
fn the_release_builds_every_host() {
    let text = release_yml();
    for host in ALL {
        let build = format!("-p {}", host.exe);
        assert!(lines(&text).any(|line| has_word(line, &build)), "release.yml has no {build}");
        if host.os == Some(Os::Linux) {
            let built = format!("target/release/{}", host.file_name(Os::Linux));
            assert!(lines(&text).any(|line| has_word(line, &built)), "release.yml has no {built}");
        }
    }
}

/// The draft release uploads every platform the release pins. The `gh release create` line's `-Match '-(...)-x64$'`
/// alternation must equal, as a set, the OS parts of `pin_platform(Os::Windows)` over the `Download` rows, since the
/// release is built on Windows.
#[test]
fn the_release_uploads_every_platform_it_pins() {
    let text = release_yml();
    let line = lines(&text).find(|line| line.contains("gh release create")).expect("a gh release create line");
    let (_, rest) = line.split_once("-Match '-(").unwrap_or_else(|| panic!("no -Match '-( on: {line}"));
    let (alternation, _) = rest.split_once(")-x64$'").unwrap_or_else(|| panic!("no )-x64$' on: {line}"));
    let uploaded: BTreeSet<&str> = alternation.split('|').collect();
    let pinned: BTreeSet<&str> = ALL
        .iter()
        .filter(|host| host.delivery == Delivery::Download)
        .map(|host| host.pin_platform(Os::Windows).strip_suffix("-x64").expect("an -x64 platform"))
        .collect();
    assert_eq!(uploaded, pinned, "{line}");
}

/// Only the app and the gate depend on the table, and the table on nothing (H1). No other `crates/*/Cargo.toml`
/// names `manifold-hosts` on a content line. The leaf's own manifest has an empty body under every header that
/// names dependencies, target-specific ones included.
#[test]
fn only_the_app_and_the_gate_depend_on_the_table_and_it_on_nothing() {
    let mut read_the_leaf = false;
    for entry in std::fs::read_dir(repo().join("crates")).expect("read crates/") {
        let dir = entry.expect("an entry of crates/").path();
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = read(&manifest);
        match dir.file_name().and_then(|name| name.to_str()).unwrap_or_default() {
            "manifold-hosts" => {
                read_the_leaf = true;
                for (header, body) in sections(&text) {
                    if header.contains("dependencies") {
                        assert!(body.is_empty(), "{}: {header} lists {body:?}", manifest.display());
                    }
                }
            }
            "manifold" | "gate" => {}
            _ => assert!(
                !lines(&text).any(|line| line.contains("manifold-hosts")),
                "{} names manifold-hosts",
                manifest.display()
            ),
        }
    }
    assert!(read_the_leaf, "crates/manifold-hosts/Cargo.toml was not read");
}
