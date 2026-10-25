//! `gate release-check vX.Y.Z`: the tag a release workflow was triggered by is one it can build. HEAD is the tag's
//! commit, the tag is annotated, its message is `manifold vX.Y.Z`, a blank line and the `CHANGELOG.md` entry's body,
//! and the workspace version is X.Y.Z.
//! `gate release-tag vX.Y.Z` tags HEAD from the CHANGELOG.md entry (nothing is pushed), so the tag passes this check.
//! `gate release-notes vX.Y.Z [HOST_PINS]` prints the draft release's notes: that body and, when the release carries
//! downloadable hosts, the SHA-256 of their pin manifest (the file the exe was built with).

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use crate::cef_pin::{hash_file, repo};
use crate::check::{check, exit_code, note_failure};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(root).stdin(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The text under `## X.Y.Z - DATE` (the current format) or `## vX.Y.Z (DATE)` (older entries) up to the next `## `
/// heading, trimmed.
pub(crate) fn changelog_body(changelog: &str, tag: &str) -> Option<String> {
    let lines: Vec<&str> = changelog.lines().collect();
    let version = tag.trim_start_matches('v');
    let is_heading =
        |l: &str| [tag, version].iter().any(|name| l == format!("## {name}") || l.starts_with(&format!("## {name} ")));
    let start = lines.iter().position(|l| is_heading(l))?;
    let end = lines[start + 1..].iter().position(|l| l.starts_with("## ")).map_or(lines.len(), |i| start + 1 + i);
    Some(lines[start + 1..end].join("\n").trim().to_owned())
}

/// The message `release-check` accepts: `manifold vX.Y.Z`, a blank line and the changelog entry's body. The one place
/// a tag message is built, so `release-tag` and the check cannot disagree.
pub(crate) fn tag_message(tag: &str, body: &str) -> String {
    format!(
        "manifold {tag}

{body}
"
    )
}

pub(crate) fn tag_message_matches(message: &str, tag: &str, body: &str) -> bool {
    message.trim().strip_prefix(&format!("manifold {tag}")).is_some_and(|rest| rest.trim() == body)
}

pub(crate) fn workspace_version(cargo_toml: &str) -> Option<String> {
    let rest = cargo_toml.split("[workspace.package]").nth(1)?;
    let line = rest.lines().take_while(|l| !l.starts_with('[')).find(|l| l.trim_start().starts_with("version"))?;
    Some(line.split('"').nth(1)?.to_owned())
}

/// HEAD is the commit `tag` points at.
fn at_tag(tag: &str) -> bool {
    let root = repo();
    matches!(
        (git(&root, &["rev-parse", "HEAD"]), git(&root, &["rev-parse", &format!("{tag}^{{commit}}")])),
        (Some(a), Some(b)) if a == b
    )
}

fn only_tag(args: &[String], usage: &str) -> Option<String> {
    if let [tag] = args {
        if tag.starts_with('v') {
            return Some(tag.clone());
        }
    }
    eprintln!("usage: gate {usage} vX.Y.Z");
    None
}

pub fn release_check(args: &[String]) -> ExitCode {
    let Some(tag) = only_tag(args, "release-check") else { return ExitCode::from(2) };
    let root = repo();
    check(&format!("HEAD is the commit {tag} points at"), at_tag(&tag));
    check(
        &format!("{tag} is an annotated tag"),
        git(&root, &["cat-file", "-t", &format!("refs/tags/{tag}")]).is_some_and(|t| t.trim() == "tag"),
    );
    let log = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap_or_default();
    let body = changelog_body(&log, &tag);
    let message = git(&root, &["tag", "-l", "--format=%(contents)", &tag]).unwrap_or_default();
    check(
        &format!("{tag}'s message is `manifold {tag}` and the CHANGELOG.md entry (tag with `git tag -a --cleanup=verbatim -F FILE`, or git drops the # headings)"),
        body.as_deref().is_some_and(|b| tag_message_matches(&message, &tag, b)),
    );
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    check(
        &format!("the workspace version is {}", tag.trim_start_matches('v')),
        workspace_version(&toml).as_deref() == Some(tag.trim_start_matches('v')),
    );
    exit_code()
}

/// `gate release-tag vX.Y.Z`: tags HEAD from `CHANGELOG.md` so the tag passes `release-check` (nothing is pushed).
/// Refuses unless the tree is clean, the workspace version is X.Y.Z and the changelog has an entry, and removes the
/// tag again if the message it wrote does not read back as the entry.
pub fn release_tag(args: &[String]) -> ExitCode {
    let Some(tag) = only_tag(args, "release-tag") else { return ExitCode::from(2) };
    let root = repo();
    let version = tag.trim_start_matches('v');
    let log = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap_or_default();
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    let body = changelog_body(&log, &tag).filter(|b| !b.is_empty());
    let clean = git(&root, &["status", "--porcelain"]).is_some_and(|s| s.trim().is_empty());
    let exists = git(&root, &["rev-parse", "-q", "--verify", &format!("refs/tags/{tag}")]).is_some();
    check("the working tree is clean", clean);
    check(&format!("the workspace version is {version}"), workspace_version(&toml).as_deref() == Some(version));
    check(&format!("CHANGELOG.md has an entry for {tag}"), body.is_some());
    check(&format!("{tag} does not exist yet"), !exists);
    let (true, Some(body)) = (clean && !exists && workspace_version(&toml).as_deref() == Some(version), body) else {
        return exit_code();
    };
    let file = std::env::temp_dir().join(format!("release-tag-{tag}.txt"));
    let made = std::fs::write(&file, tag_message(&tag, &body)).is_ok()
        && Command::new("git")
            .args(["tag", "-a", "--cleanup=verbatim", "-F"])
            .arg(&file)
            .arg(&tag)
            .current_dir(&root)
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&file);
    check(&format!("{tag} is created"), made);
    if made {
        let message = git(&root, &["tag", "-l", "--format=%(contents)", &tag]).unwrap_or_default();
        let ok = tag_message_matches(&message, &tag, &body);
        check(&format!("{tag}'s message reads back as the CHANGELOG.md entry"), ok);
        if !ok {
            git(&root, &["tag", "-d", &tag]);
        }
    }
    exit_code()
}

pub fn release_notes(args: &[String]) -> ExitCode {
    let (tag_args, pins) = match args {
        [tag, pins] => (std::slice::from_ref(tag), Some(Path::new(pins))),
        _ => (args, None),
    };
    let Some(tag) = only_tag(tag_args, "release-notes") else { return ExitCode::from(2) };
    let log = std::fs::read_to_string(repo().join("CHANGELOG.md")).unwrap_or_default();
    let Some(body) = changelog_body(&log, &tag) else {
        eprintln!("no CHANGELOG.md entry for {tag}");
        note_failure();
        return exit_code();
    };
    let pins_line = match pins.map(hash_file) {
        None => String::new(),
        Some(Ok((sha, ..))) => format!(
            "

Host pins sha256: `{sha}`"
        ),
        Some(Err(e)) => {
            eprintln!("{e}");
            note_failure();
            return exit_code();
        }
    };
    println!("{body}{pins_line}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str =
        "# Changelog\n\nintro\n\n## v0.6.0 (2026-10-16)\n\n### Features\n\n- one\n\n## v0.5.1 (2026-10-10)\n\n- old\n";

    #[test]
    fn the_changelog_body_is_the_text_under_the_tags_heading() {
        assert_eq!(changelog_body(LOG, "v0.6.0").unwrap(), "### Features\n\n- one");
        assert_eq!(changelog_body(LOG, "v0.5.1").unwrap(), "- old");
        assert_eq!(changelog_body(LOG, "v0.6.1"), None);
        assert_eq!(changelog_body(LOG, "v0.6"), None);
    }

    #[test]
    fn a_heading_without_the_v_and_with_a_dash_date_is_found() {
        let log = "# Changelog\n\n## 0.7.0 - 2026-10-18\n\n### Fixed\n\n- two\n\n## v0.6.0 (2026-10-16)\n\n- one\n";
        assert_eq!(changelog_body(log, "v0.7.0").unwrap(), "### Fixed\n\n- two");
        assert_eq!(changelog_body(log, "v0.6.0").unwrap(), "- one");
        assert_eq!(changelog_body(log, "v0.7"), None);
    }

    #[test]
    fn the_tag_message_is_a_title_then_the_body() {
        let body = "### Features\n\n- one";
        assert!(tag_message_matches("manifold v0.6.0\n\n### Features\n\n- one\n", "v0.6.0", body));
        assert!(!tag_message_matches("manifold v0.6.0\n\n### Features\n\n- two\n", "v0.6.0", body));
        assert!(!tag_message_matches("manifold v0.5.9\n\n### Features\n\n- one\n", "v0.6.0", body));
    }

    #[test]
    fn a_built_tag_message_passes_the_check_for_every_entry_shape() {
        for log in [
            LOG,
            "## 0.9.0 - 2026-10-25

*Note.*

### Added

- x

## 0.8.0 - 2026-10-23

- y
",
        ] {
            let tag = if log == LOG { "v0.6.0" } else { "v0.9.0" };
            let body = changelog_body(log, tag).unwrap();
            let message = tag_message(tag, &body);
            assert!(tag_message_matches(&message, tag, &body), "{message:?}");
            assert!(!message.lines().any(|l| l.starts_with("## ")), "the heading is not part of the message");
        }
    }

    #[test]
    fn the_workspace_version_is_read_from_cargo_toml() {
        let toml = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.6.0\"\nedition = \"2021\"\n";
        assert_eq!(workspace_version(toml).as_deref(), Some("0.6.0"));
        assert_eq!(workspace_version("[package]\nversion = \"9.9.9\"\n"), None);
    }
}
