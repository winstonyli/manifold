//! `gate release-verify vX.Y.Z [DIST]`: re-checks a built release with the tag checked out. Every host the tag's
//! host_pin.rs pins must download from its URL with the pinned size and SHA-256 (the network); the exe in DIST
//! (default dist/) must match its .sha256; and the embedded WebView2 host and PDF view copies in DIST/embedded must
//! match the recorded hashes. That the exe really contains those bytes is not checked here (build.rs embeds the file
//! and hashes it at build time).

use std::path::Path;
use std::process::ExitCode;

use crate::check::{check, exit_code, note_failure};
use crate::pin::{download, hash_file, read_table, repo};
use crate::release_check::at_tag;

/// The first whitespace-separated word of a hash record file.
fn recorded(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()?.split_whitespace().next().map(str::to_owned)
}

/// `file`'s SHA-256 equals the hash recorded in `record`.
fn matches_record(file: &Path, record: &Path) -> bool {
    matches!((hash_file(file), recorded(record)), (Ok((sha, ..)), Some(want)) if sha == want)
}

fn verify(tag: &str, dist: &Path) -> Result<(), String> {
    check(&format!("HEAD is the commit {tag} points at"), at_tag(tag));
    let table = read_table(&repo().join("crates/manifold/src/host_pin.rs"))?;
    let tmp = std::env::temp_dir().join(format!("verify-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    for ((kind, platform), files) in table {
        for (file, (url, size, sha)) in files {
            let dest = tmp.join(format!("{file}-{platform}"));
            let ok =
                download(&url, &dest).and_then(|()| hash_file(&dest)).is_ok_and(|(got, _, n)| got == sha && n == size);
            check(&format!("{kind}/{platform} {file} downloads from {url} and matches the pin"), ok);
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
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
