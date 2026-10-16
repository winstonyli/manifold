//! `gate host-fingerprint KIND PLATFORM [--allow-dirty]`: the identity of a downloadable engine host, 12 hex digits
//! over everything that determines its binary: the host package's resolved dependency closure (`cargo metadata
//! --locked`, dev dependencies excluded), the toolchain and the build recipe in `.github/hosts.json`. A workspace
//! crate contributes its tracked files as git stores them (`git ls-tree -r HEAD`: mode, blob id, path); every package
//! contributes name, version, source and resolved features. Hosts are published as `hosts-KIND-FP` releases and a pin
//! URL carries FP, so `gate host-pin --check` can tell a stale table without the network.
//!
//! The digest reads committed state, so uncommitted edits to the closure's directories, `rust-toolchain.toml` or
//! `.github/hosts.json` are an error unless `--allow-dirty`. Anything else that changes a host binary (an environment
//! variable read by a build script, say) must be listed in the recipe, or a stale host would be reused.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::pin::{hex, repo};

const TRIPLES: [(&str, &str); 2] =
    [("windows-x64", "x86_64-pc-windows-msvc"), ("linux-x64", "x86_64-unknown-linux-gnu")];

fn run(dir: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// One package of a host's closure.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Pkg {
    /// `name version source`.
    pub id: String,
    /// The repo-relative directory of a workspace package; `None` for a registry or git package.
    pub dir: Option<String>,
    pub features: Vec<String>,
}

/// The packages `package` (a workspace crate) depends on, itself included, sorted. Dev dependencies are left out.
pub(crate) fn closure_of(meta: &Value, package: &str) -> Result<Vec<Pkg>, String> {
    let pkgs = meta["packages"].as_array().ok_or("cargo metadata has no packages")?;
    let nodes = meta["resolve"]["nodes"].as_array().ok_or("cargo metadata has no resolve graph")?;
    let ws = meta["workspace_root"].as_str().ok_or("cargo metadata has no workspace_root")?;
    let start = pkgs
        .iter()
        .find(|p| p["name"] == package && p["source"].is_null())
        .and_then(|p| p["id"].as_str())
        .ok_or(format!("no workspace package {package}"))?;
    let (mut seen, mut stack) = (BTreeSet::new(), vec![start.to_owned()]);
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let node = nodes.iter().find(|n| n["id"] == id.as_str()).ok_or(format!("{id} is not in the resolve graph"))?;
        for d in node["deps"].as_array().into_iter().flatten() {
            let used = d["dep_kinds"].as_array().into_iter().flatten().any(|k| k["kind"] != "dev");
            if let (true, Some(pkg)) = (used, d["pkg"].as_str()) {
                stack.push(pkg.to_owned());
            }
        }
    }
    let mut out = Vec::new();
    for id in &seen {
        let p = pkgs.iter().find(|p| p["id"] == id.as_str()).ok_or(format!("{id} is not a package"))?;
        let node = nodes.iter().find(|n| n["id"] == id.as_str()).ok_or(format!("{id} has no node"))?;
        let mut features: Vec<String> =
            node["features"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(str::to_owned)).collect();
        features.sort();
        let source = p["source"].as_str();
        let dir = if source.is_none() {
            let manifest = p["manifest_path"].as_str().ok_or(format!("{id} has no manifest_path"))?;
            let rel =
                manifest.strip_prefix(ws).ok_or(format!("{manifest} is outside {ws}"))?.trim_start_matches(['/', '\\']);
            let dir = rel.rsplit_once(['/', '\\']).map_or("", |(d, _)| d).replace('\\', "/");
            if dir.is_empty() {
                return Err(format!("{id} sits at the workspace root; its whole tree would be hashed"));
            }
            Some(dir)
        } else {
            None
        };
        let name = p["name"].as_str().unwrap_or("");
        let version = p["version"].as_str().unwrap_or("");
        // A workspace crate is covered by its files; its (workspace-wide) version would change every host each release.
        let id = if source.is_none() {
            format!("{name} path")
        } else {
            format!("{name} {version} {}", source.unwrap_or(""))
        };
        out.push(Pkg { id, dir, features });
    }
    out.sort();
    Ok(out)
}

pub(crate) fn closure(root: &Path, package: &str, triple: &str) -> Result<Vec<Pkg>, String> {
    let json = run(root, "cargo", &["metadata", "--locked", "--format-version", "1", "--filter-platform", triple])?;
    closure_of(&serde_json::from_str(&json).map_err(|e| e.to_string())?, package)
}

/// `mode type blob<TAB>path` lines of the files git tracks under `dir` at HEAD.
pub(crate) fn tree_lines(root: &Path, dir: &str) -> Result<Vec<String>, String> {
    let out = run(root, "git", &["ls-tree", "-r", "HEAD", "--", dir])?;
    let lines: Vec<String> = out.lines().map(str::to_owned).collect();
    if lines.is_empty() {
        return Err(format!("git tracks nothing under {dir} at HEAD"));
    }
    Ok(lines)
}

/// Whether any of `paths` has uncommitted changes (including untracked files).
pub(crate) fn dirty(root: &Path, paths: &[&str]) -> Result<bool, String> {
    let mut args = vec!["status", "--porcelain", "--"];
    args.extend_from_slice(paths);
    Ok(!run(root, "git", &args)?.trim().is_empty())
}

/// 12 hex digits over `parts`, each length-prefixed so `["ab","c"]` and `["a","bc"]` differ.
pub(crate) fn digest(parts: &[String]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    hex(&h.finalize())[..12].to_owned()
}

/// The root `Cargo.toml` tables that change how any crate is compiled (`[profile*]`, `[patch*]`, `[replace]`).
pub(crate) fn build_tables(cargo_toml: &str) -> String {
    let (mut keep, mut out) = (false, String::new());
    for line in cargo_toml.lines() {
        if line.starts_with('[') {
            keep = ["[profile", "[patch", "[replace"].iter().any(|p| line.starts_with(p));
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The recipe for `kind`/`platform` from `.github/hosts.json`.
pub(crate) fn recipe(root: &Path, kind: &str, platform: &str) -> Result<Value, String> {
    let path = root.join(".github/hosts.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let all: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    match &all[kind][platform] {
        Value::Null => Err(format!("{} has no recipe for {kind}/{platform}", path.display())),
        r => Ok(r.clone()),
    }
}

pub(crate) fn fingerprint(root: &Path, kind: &str, platform: &str, require_clean: bool) -> Result<String, String> {
    let recipe = recipe(root, kind, platform)?;
    let package = recipe["package"].as_str().ok_or("the recipe has no `package`")?;
    let triple = TRIPLES.iter().find(|t| t.0 == platform).ok_or(format!("no target triple for {platform}"))?.1;
    let pkgs = closure(root, package, triple)?;
    if require_clean {
        let mut paths: Vec<&str> = pkgs.iter().filter_map(|p| p.dir.as_deref()).collect();
        paths.extend(["rust-toolchain.toml", ".github/hosts.json", ".cargo", "Cargo.toml", "Cargo.lock"]);
        if dirty(root, &paths)? {
            return Err(format!("uncommitted changes under {paths:?}: commit them or pass --allow-dirty"));
        }
    }
    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml")).map_err(|e| e.to_string())?;
    let cargo_config = std::fs::read_to_string(root.join(".cargo/config.toml")).unwrap_or_default();
    let root_toml = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let mut parts = vec![
        format!("recipe {recipe}"),
        format!("toolchain {toolchain}"),
        format!("cargo-config {cargo_config}"),
        format!("build-tables {}", build_tables(&root_toml)),
    ];
    for p in &pkgs {
        parts.push(format!("pkg {} {:?}", p.id, p.features));
        if let Some(dir) = &p.dir {
            parts.extend(tree_lines(root, dir)?.into_iter().map(|l| format!("file {l}")));
        }
    }
    Ok(digest(&parts))
}

pub fn host_fingerprint_main(args: &[String]) -> ExitCode {
    let allow_dirty = args.iter().any(|a| a == "--allow-dirty");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "--allow-dirty").collect();
    let [kind, platform] = rest.as_slice() else {
        eprintln!("usage: gate host-fingerprint KIND PLATFORM [--allow-dirty]");
        return ExitCode::from(2);
    };
    match fingerprint(&repo(), kind, platform, !allow_dirty) {
        Ok(fp) => {
            println!("{fp}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// a (path) -> b (path) -> c (registry); a -> d only as a dev dependency; e is unrelated.
    fn meta() -> Value {
        let pkg = |name: &str, dir: &str, source: Value| {
            json!({"id": name, "name": name, "version": "1.0.0", "source": source,
                   "manifest_path": format!("C:\\w\\{dir}\\Cargo.toml")})
        };
        let node = |id: &str, deps: Vec<(&str, &str)>, features: Vec<&str>| {
            json!({"id": id, "features": features, "deps": deps.iter()
                .map(|(p, k)| json!({"pkg": p, "dep_kinds": [{"kind": if k.is_empty() { Value::Null } else { json!(k) }}]}))
                .collect::<Vec<_>>()})
        };
        json!({
            "workspace_root": "C:\\w",
            "packages": [
                pkg("a", "crates\\a", Value::Null), pkg("b", "crates\\b", Value::Null),
                pkg("c", ".cargo\\c", json!("registry+https://github.com/rust-lang/crates.io-index")),
                pkg("d", "crates\\d", Value::Null), pkg("e", "crates\\e", Value::Null)],
            "resolve": {"nodes": [
                node("a", vec![("b", ""), ("d", "dev")], vec!["x"]), node("b", vec![("c", "build")], vec![]),
                node("c", vec![], vec!["std", "alloc"]), node("d", vec![], vec![]), node("e", vec![], vec![])]}
        })
    }

    #[test]
    fn the_closure_follows_normal_and_build_deps_but_not_dev_deps_or_strangers() {
        let pkgs = closure_of(&meta(), "a").unwrap();
        let names: Vec<&str> = pkgs.iter().map(|p| p.id.split(' ').next().unwrap()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert_eq!(pkgs[0].dir.as_deref(), Some("crates/a"));
        assert_eq!(pkgs[2].dir, None);
        assert_eq!(pkgs[2].features, ["alloc", "std"]);
    }

    #[test]
    fn a_workspace_crates_version_is_not_part_of_its_identity() {
        let mut bumped = meta();
        for p in bumped["packages"].as_array_mut().unwrap() {
            if p["source"].is_null() {
                p["version"] = json!("2.0.0");
            }
        }
        assert_eq!(closure_of(&meta(), "a").unwrap()[..2], closure_of(&bumped, "a").unwrap()[..2]);
    }

    #[test]
    fn only_the_compile_affecting_root_tables_are_hashed() {
        let toml = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1\"\n\n[profile.release]\nlto = true\n\n[patch.crates-io]\nx = { path = \"x\" }\n\n[lints]\nrust = 1\n";
        let t = build_tables(toml);
        assert!(t.contains("lto = true") && t.contains("[patch.crates-io]"), "{t}");
        assert!(!t.contains("version") && !t.contains("[lints]"), "{t}");
    }

    #[test]
    fn digest_is_twelve_hex_and_framed() {
        let d = digest(&["ab".into(), "c".into()]);
        assert_eq!(d.len(), 12);
        assert!(d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_ne!(d, digest(&["a".into(), "bc".into()]));
        assert_eq!(d, digest(&["ab".into(), "c".into()]));
    }

    #[test]
    fn tree_lines_change_when_a_file_changes() {
        let dir = std::env::temp_dir().join(format!("fp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("crates/a")).unwrap();
        let git = |args: &[&str]| {
            let id = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"];
            run(&dir, "git", &[&id[..], args].concat()).unwrap()
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("crates/a/lib.rs"), "one").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "1"]);
        let before = tree_lines(&dir, "crates/a").unwrap();
        std::fs::write(dir.join("crates/a/lib.rs"), "two").unwrap();
        assert!(dirty(&dir, &["crates/a"]).unwrap());
        git(&["commit", "-qam", "2"]);
        assert!(!dirty(&dir, &["crates/a"]).unwrap());
        assert_ne!(before, tree_lines(&dir, "crates/a").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reads the real workspace, so it also proves the hosts do not drag in the app or other engines.
    #[test]
    fn the_cef_host_closure_has_the_protocol_crate_and_no_other_host() {
        let pkgs = closure(&repo(), "manifold-view-cef", "x86_64-pc-windows-msvc").unwrap();
        let dirs: Vec<&str> = pkgs.iter().filter_map(|p| p.dir.as_deref()).collect();
        assert!(dirs.contains(&"crates/ipc-protocol") && dirs.contains(&"crates/manifold-view-cef"), "{dirs:?}");
        assert!(!dirs.contains(&"crates/manifold") && !dirs.contains(&"crates/manifold-view-servo"), "{dirs:?}");
    }

    #[test]
    fn the_same_tree_gives_the_same_fingerprint() {
        let a = fingerprint(&repo(), "cef", "windows-x64", false).unwrap();
        assert_eq!(a, fingerprint(&repo(), "cef", "windows-x64", false).unwrap());
        assert!(fingerprint(&repo(), "plan9", "windows-x64", false).is_err());
    }
}
