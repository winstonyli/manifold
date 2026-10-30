//! `gate tier [BASE]` / `gate tier --stdin` (was scripts/gate-tier.sh): the tier to gate the changes since BASE
//! (default: the merge-base with main), as advice for `gate all LOG ui|light|full`. Prints one line per changed path,
//! strictest first, then `TIER ui|light|full`; the strictest tier of any path wins. Changed means tracked files that
//! differ from BASE in the working tree, plus untracked files. `--stdin` classifies the paths on stdin instead. Exit 3
//! when no path changed (on `main` with everything committed the default base is HEAD: pass the commit the last gate
//! ran at). Edit the lists below when a module changes its role.

use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

/// The manifold crate's source folder, as git prints it.
const SRC: &str = "crates/manifold/src";

/// Modules a UI-only change may touch: drawing, input and view state, none of which write the store or reach an
/// engine. A file here that imports a core module is demoted to light (`classify` reads the file).
const UI_FILES: &[&str] = &[
    "bookmarks",
    "click_count",
    "failure_toasts",
    "graph_layout",
    "history",
    "image_view",
    "input",
    "json_level_ui",
    "json_view",
    "pdf_render",
    "rgba",
    "shortcuts",
    "theme",
    "view_modes",
    "zoom_chip",
];

/// A file or folder of one of these under the source folder is the sync, store, mirror, session, engine or network
/// core (`store` is a folder only).
const CORE_MODULES: &[&str] = &[
    "session_sync",
    "mirror",
    "session",
    "local_state",
    "process_manager",
    "net",
    "view_install",
    "redirect_fetch",
    "favicon_fetch",
    "jar",
];

/// `crate::<one of these>` in a file makes it core code.
const CORE_IMPORTS: &[&str] = &["store", "session_sync", "mirror", "session", "process_manager", "net"];

/// Layer directories (view/, app/, ...) group module files without changing a module's name
/// (`#[path = "model/jar.rs"] mod jar;`): a file in one is classified as if it sat directly under the source folder.
const LAYERS: &[&str] = &["view", "app", "model", "network", "engine", "io"];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    Ui,
    Light,
    Full,
}

impl Tier {
    fn name(self) -> &'static str {
        match self {
            Tier::Ui => "ui",
            Tier::Light => "light",
            Tier::Full => "full",
        }
    }
}

/// Whether `text` has `crate::<core module>` with the module name ending there (`crate::net` yes, `crate::network` no).
fn imports_core(text: &str) -> bool {
    text.match_indices("crate::").any(|(at, m)| {
        let word: String =
            text[at + m.len()..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        CORE_IMPORTS.contains(&word.as_str())
    })
}

/// `path` with a layer directory under `src` removed (`src/model/jar.rs` -> `src/jar.rs`).
fn unlayered(path: &str, src: &str) -> String {
    if let Some(rel) = path.strip_prefix(src).and_then(|r| r.strip_prefix('/')) {
        if let Some((first, rest)) = rel.split_once('/') {
            if LAYERS.contains(&first) {
                return format!("{src}/{rest}");
            }
        }
    }
    path.to_string()
}

/// The tier of one changed path and why. `read` returns a source file's text (for the UI list's import check).
pub fn classify(path: &str, src: &str, read: &dyn Fn(&str) -> Option<String>) -> (Tier, String) {
    let p = unlayered(path, src);
    let p = p.as_str();
    let full = |why: &str| (Tier::Full, why.to_string());
    let light = |why: String| (Tier::Light, why);
    let under = |dir: &str| p.strip_prefix(dir).is_some_and(|r| r.starts_with('/'));
    let in_src = p.strip_prefix(src).and_then(|r| r.strip_prefix('/'));

    if matches!(p, "Cargo.toml" | "Cargo.lock")
        || p.starts_with(".cargo/")
        || p.starts_with("rust-toolchain")
        || (p.starts_with("crates/") && (p.ends_with("/Cargo.toml") || !under("crates/manifold")))
    {
        return full("dependency, or any crate but the app (the gate, protocol, pages, host table and engines)");
    }
    if let Some(rel) = in_src {
        let first = rel.split('/').next().unwrap_or("");
        let is_dir = rel.contains('/');
        let stem = first.strip_suffix(".rs").filter(|_| !is_dir).unwrap_or(first);
        if (CORE_MODULES.contains(&stem) && (is_dir || first.ends_with(".rs"))) || (is_dir && first == "store") {
            return full("sync, store, mirror, session, engine or network core");
        }
    }
    if let Some(rel) = in_src {
        if rel.contains('/') {
            return light("a module directory on no list".into());
        }
        if let Some(b) = rel.strip_suffix(".rs") {
            if UI_FILES.contains(&b) {
                return if read(p).is_some_and(|t| imports_core(&t)) {
                    light(format!("{b} is on the ui list but imports a core module"))
                } else {
                    (Tier::Ui, format!("{b}: drawing or input, no core import"))
                };
            }
            return light(format!("{b} is on neither the ui nor the full list"));
        }
    }
    if p.starts_with("scripts/") || p.starts_with(".githooks/") || p.starts_with(".github/") {
        return light("script or tooling".into());
    }
    if p.ends_with(".md")
        || p.starts_with("docs/")
        || p.starts_with(".superpowers/")
        || matches!(p, ".gitignore" | ".gitattributes" | "rustfmt.toml")
        || p.starts_with("LICENSE")
    {
        return (Tier::Ui, "documentation or config only (nothing to test)".into());
    }
    light("path on no list".into())
}

/// The report: a line per path, strictest first, then `TIER x`; None if there is no path.
pub fn report(paths: &[String], src: &str, read: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut rows: Vec<(Tier, String)> = paths
        .iter()
        .map(|p| {
            let (tier, why) = classify(p, src, read);
            (tier, format!("{} {p} ({why})", tier.name()))
        })
        .collect();
    let best = rows.iter().map(|r| r.0).max()?;
    rows.sort_by(|a, b| b.cmp(a));
    let mut out: String = rows.iter().map(|r| format!("{}\n", r.1)).collect();
    out.push_str(&format!("TIER {}", best.name()));
    Some(out)
}

fn git(repo: &Path, args: &[&str]) -> io::Result<Vec<u8>> {
    let out = Command::new("git").args(args).current_dir(repo).stdin(Stdio::null()).output()?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(io::Error::other(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim())))
    }
}

fn nul_split(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned())
}

pub fn tier(args: &[String]) -> ExitCode {
    // crates/gate/../..: the repo this gate was built from.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let stdin = args.first().is_some_and(|a| a == "--stdin");
    let (paths, base) = if stdin {
        let mut text = String::new();
        if let Err(e) = io::stdin().read_to_string(&mut text) {
            eprintln!("stdin: {e}");
            return ExitCode::from(1);
        }
        (text.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect::<Vec<_>>(), "stdin".to_string())
    } else {
        let listed = (|| -> io::Result<(Vec<String>, String)> {
            let base = match args.first() {
                Some(b) => b.clone(),
                None => String::from_utf8_lossy(&git(&repo, &["merge-base", "HEAD", "main"])?).trim().to_string(),
            };
            let mut paths: Vec<String> = nul_split(&git(&repo, &["diff", "--name-only", "-z", &base, "--"])?).collect();
            paths.extend(nul_split(&git(&repo, &["ls-files", "-z", "--others", "--exclude-standard"])?));
            Ok((paths, base))
        })();
        match listed {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        }
    };
    let read = |p: &str| std::fs::read_to_string(repo.join(p)).ok();
    match report(&paths, SRC, &read) {
        Some(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("no changed files since {base}; pass the commit the last gate ran at (git log) as BASE");
            ExitCode::from(3)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "src";

    fn tier_of(paths: &[&str], files: &[(&str, &str)]) -> String {
        let read = |p: &str| files.iter().find(|(n, _)| *n == p).map(|(_, t)| t.to_string());
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        report(&paths, S, &read).unwrap().lines().last().unwrap().to_string()
    }

    #[test]
    fn the_strictest_path_wins() {
        assert_eq!(tier_of(&["README.md", "docs/x.md"], &[]), "TIER ui");
        assert_eq!(tier_of(&["src/store/journal.rs"], &[]), "TIER full");
        assert_eq!(
            tier_of(&["README.md", "src/main.rs", "crates/manifold-view-protocol/src/lib.rs"], &[]),
            "TIER full"
        );
        assert_eq!(tier_of(&["src/main.rs"], &[]), "TIER light");
        assert_eq!(tier_of(&["scripts/foreground.ps1"], &[]), "TIER light");
    }

    #[test]
    fn a_ui_file_that_imports_a_core_module_is_demoted() {
        assert_eq!(tier_of(&["src/theme.rs"], &[("src/theme.rs", "use egui;")]), "TIER ui");
        assert_eq!(
            tier_of(&["src/zoom_chip.rs"], &[("src/zoom_chip.rs", "use crate::store::model::X;")]),
            "TIER light"
        );
        // `crate::network` is not `crate::net`.
        assert_eq!(tier_of(&["src/theme.rs"], &[("src/theme.rs", "use crate::network::X;")]), "TIER ui");
        assert!(imports_core("use crate::session::S;") && imports_core("x crate::net;"));
    }

    #[test]
    fn core_modules_and_their_test_dirs_are_full() {
        for p in ["src/view_install/tests.rs", "src/process_manager/tests.rs", "src/jar/tests.rs", "src/net.rs"] {
            assert_eq!(tier_of(&[p], &[]), "TIER full", "{p}");
        }
        assert_eq!(tier_of(&["src/link_gestures/tests.rs"], &[]), "TIER light");
    }

    #[test]
    fn a_layer_directory_is_read_by_its_module_name() {
        assert_eq!(tier_of(&["src/model/jar.rs"], &[]), "TIER full");
        assert_eq!(tier_of(&["src/model/jar/tests.rs"], &[]), "TIER full");
        assert_eq!(tier_of(&["src/view/theme.rs"], &[]), "TIER ui");
        assert_eq!(tier_of(&["src/io/profile.rs"], &[]), "TIER light");
    }

    #[test]
    fn the_gate_and_the_crate_manifests_are_full() {
        assert_eq!(tier_of(&["crates/gate/src/tier.rs"], &[]), "TIER full");
        assert_eq!(tier_of(&["Cargo.lock"], &[]), "TIER full");
        assert_eq!(tier_of(&["crates/manifold-view-cef/src/lib.rs"], &[]), "TIER full");
        assert_eq!(tier_of(&["crates/manifold-pages/assets/start.html"], &[]), "TIER full");
        assert_eq!(tier_of(&["crates/manifold-hosts/src/lib.rs"], &[]), "TIER full");
        assert_eq!(tier_of(&["crates/manifold-hosts/tests/layout.rs"], &[]), "TIER full");
        // Every crate but the app is Full, so a crate added later needs no edit here.
        assert_eq!(tier_of(&["crates/some-later-crate/src/lib.rs"], &[]), "TIER full");
        // The app crate is classified by its own lists: a file outside its source folder is on none.
        assert_eq!(tier_of(&["crates/manifold/build.rs"], &[]), "TIER light");
    }

    #[test]
    fn the_report_lists_the_strictest_first_and_none_is_none() {
        let read = |_: &str| None;
        let text = report(&["README.md".into(), "src/store/a.rs".into()], S, &read).unwrap();
        assert!(text.starts_with("full src/store/a.rs ("), "{text}");
        assert!(report(&[], S, &read).is_none());
    }
}
