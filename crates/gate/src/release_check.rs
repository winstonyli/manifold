//! `gate release-check vX.Y.Z`: the tag a release workflow was triggered by is one it can build. HEAD is the tag's
//! commit, the tag is annotated, its message is `manifold vX.Y.Z`, a blank line and the `CHANGELOG.md` entry's body,
//! the workspace version is X.Y.Z, and every host pin carries the fingerprint of this tree (`host-pin --check`).
//! `gate release-notes vX.Y.Z` prints the draft release's notes: that body and the host table's SHA-256.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use crate::check::{check, exit_code, note_failure};
use crate::pin::{pins_are_fresh, repo, table_sha256};

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(root).stdin(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The text under `## TAG ...` up to the next `## ` heading, trimmed.
pub(crate) fn changelog_body(changelog: &str, tag: &str) -> Option<String> {
    let lines: Vec<&str> = changelog.lines().collect();
    let start = lines.iter().position(|l| *l == format!("## {tag}") || l.starts_with(&format!("## {tag} ")))?;
    let end = lines[start + 1..].iter().position(|l| l.starts_with("## ")).map_or(lines.len(), |i| start + 1 + i);
    Some(lines[start + 1..end].join("\n").trim().to_owned())
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
pub(crate) fn at_tag(tag: &str) -> bool {
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
        &format!("{tag}'s message is `manifold {tag}` and the CHANGELOG.md entry"),
        body.as_deref().is_some_and(|b| tag_message_matches(&message, &tag, b)),
    );
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    check(
        &format!("the workspace version is {}", tag.trim_start_matches('v')),
        workspace_version(&toml).as_deref() == Some(tag.trim_start_matches('v')),
    );
    match pins_are_fresh() {
        Ok(()) => {
            check("every host pin matches the sources", true);
        }
        Err(stale) => {
            for s in stale {
                check(&s, false);
            }
        }
    }
    exit_code()
}

pub fn release_notes(args: &[String]) -> ExitCode {
    let Some(tag) = only_tag(args, "release-notes") else { return ExitCode::from(2) };
    let log = std::fs::read_to_string(repo().join("CHANGELOG.md")).unwrap_or_default();
    let (Some(body), Ok(sha)) = (changelog_body(&log, &tag), table_sha256()) else {
        eprintln!("no CHANGELOG.md entry for {tag}, or no host table");
        note_failure();
        return exit_code();
    };
    println!("{body}\n\nHost pin table sha256: `{sha}`");
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
    fn the_tag_message_is_a_title_then_the_body() {
        let body = "### Features\n\n- one";
        assert!(tag_message_matches("manifold v0.6.0\n\n### Features\n\n- one\n", "v0.6.0", body));
        assert!(!tag_message_matches("manifold v0.6.0\n\n### Features\n\n- two\n", "v0.6.0", body));
        assert!(!tag_message_matches("manifold v0.5.9\n\n### Features\n\n- one\n", "v0.6.0", body));
    }

    #[test]
    fn the_workspace_version_is_read_from_cargo_toml() {
        let toml = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.6.0\"\nedition = \"2021\"\n";
        assert_eq!(workspace_version(toml).as_deref(), Some("0.6.0"));
        assert_eq!(workspace_version("[package]\nversion = \"9.9.9\"\n"), None);
    }
}
