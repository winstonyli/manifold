//! `gate release-verify vX.Y.Z` (was `scripts/release.sh --verify`): re-checks dist/ against that tag. Its
//! host_pin.rs hashes and sizes must equal dist/hosts/*, the exe must match its .sha256, and the embedded hosts'
//! recorded hashes must match the saved copies in dist/embedded (the PDF view's only for tags whose tree has that
//! crate: v0.4.1, v0.5.0 and v0.5.1 shipped without it). Entries reused from an older release are skipped, since
//! their files are not in dist/. That the exe really contains those bytes is not checked here (build.rs embeds the
//! file and hashes it at build time).

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use crate::check::{check, note_failure};
use crate::pin::{hash_file, parse_table, region, repo};

fn git(repo: &Path, args: &[String]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(repo).stdin(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first whitespace-separated word of a hash record file.
fn recorded(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()?.split_whitespace().next().map(str::to_owned)
}

/// `file`'s SHA-256 equals the hash recorded in `record`.
fn matches_record(file: &Path, record: &Path) -> bool {
    matches!((hash_file(file), recorded(record)), (Ok((sha, ..)), Some(want)) if sha == want)
}

fn verify(tag: &str) -> Result<(), String> {
    let repo = repo();
    let dist = repo.join("dist");
    let pin_rs = "crates/manifold/src/host_pin.rs";
    let text = git(&repo, &["show".into(), format!("{tag}:{pin_rs}")]).ok_or(format!("{tag} has no {pin_rs}"))?;
    let (b, e) = region(&text)?;
    let body = &text[b..e];
    let version = body.split("PIN_VERSION: &str = \"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("");
    let want = tag.trim_start_matches('v');
    check(&format!("PIN_VERSION is {want}"), version == want);
    for ((kind, platform), files) in parse_table(body)? {
        for (file, (url, size, sha)) in files {
            let asset = format!("{file}-{platform}");
            if !url.contains(&format!("/download/{tag}/")) {
                println!("SKIP: {kind} {asset} is reused from an older release; not in dist/");
                continue;
            }
            let path = dist.join("hosts").join(&asset);
            match hash_file(&path) {
                Err(_) => {
                    check(&format!("{} is missing", path.display()), false);
                }
                Ok((got, _, n)) => {
                    check(&format!("{asset} matches the pin"), got == sha && n == size);
                }
            }
        }
    }
    let exe_name = format!("manifold-{tag}-windows-x64.exe");
    check(
        &format!("{exe_name} matches its .sha256"),
        matches_record(&dist.join(&exe_name), &dist.join(format!("{exe_name}.sha256"))),
    );
    let stem = format!("manifold-{tag}-windows-x64");
    check(
        "the embedded WebView2 host copy matches the recorded hash",
        matches_record(
            &dist.join("embedded/manifold-engine-webview2.exe"),
            &dist.join(format!("{stem}.embedded-host.sha256")),
        ),
    );
    if git(&repo, &["cat-file".into(), "-e".into(), format!("{tag}:crates/manifold-view-pdfium/Cargo.toml")]).is_some()
    {
        check(
            "the embedded PDF view copy matches the recorded hash",
            matches_record(
                &dist.join("embedded/manifold-view-pdfium.exe"),
                &dist.join(format!("{stem}.embedded-pdf-view.sha256")),
            ),
        );
    }
    Ok(())
}

pub fn release_verify(args: &[String]) -> ExitCode {
    let [tag] = args else {
        eprintln!("usage: gate release-verify vX.Y.Z");
        return ExitCode::from(2);
    };
    if let Err(e) = verify(tag) {
        eprintln!("{e}");
        note_failure();
    }
    crate::check::exit_code()
}
