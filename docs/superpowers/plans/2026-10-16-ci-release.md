# Releases on GitHub Actions, P1 (CEF) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `vX.Y.Z` tag push builds `manifold.exe` on a GitHub runner and leaves a draft release; downloadable CEF host binaries are built by their own workflow and pinned by a source fingerprint.

**Architecture:** `gate host-fingerprint` hashes a host's cargo dependency closure (git blob ids of workspace crates, name/version/source/features of external ones), the toolchain and a recipe file. A pin URL carries that fingerprint, so `gate host-pin --check` proves the table fresh without the network. `hosts.yml` publishes `hosts-KIND-FP` releases; `gate host-pin --adopt` rewrites the table from one. `release.yml` verifies the tag, builds the embedded hosts and the exe, runs `gate package` and `gate release-verify`, and creates a draft.

**Tech Stack:** Rust (`crates/gate`, serde_json, sha2 already present), GitHub Actions (pwsh on `windows-latest`), `gh`, `cargo metadata`.

**Spec:** `docs/superpowers/specs/2026-10-16-ci-release-design.md` (decisions numbered below as "spec D<n>").

## Global Constraints

- Work on `main`; `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass; never `--no-verify`; lints fixed or `#[allow(..., reason = "...")]`.
- No test may read or write the real `%APPDATA%\manifold`.
- No compatibility shims: when a mechanism is replaced, delete the old one and everything only it used (spec D9).
- Fingerprint is 12 lowercase hex digits (SHA-256 truncated); release names are `hosts-KIND-FP`; pin URL is `https://github.com/winstonyli/manifold/releases/download/hosts-KIND-FP/FILE-PLATFORM`.
- `PIN_VERSION` is removed everywhere; host and app agree through `PROTOCOL_VERSION` only (spec D10).
- Python never reads stdin; use the Edit tool for edits. Use Git Bash at `C:\Program Files\Git\bin\bash.exe`, not WSL's `bash`.
- Use `cargo ... -j 8`. Run builds and tests at BelowNormal if the machine is busy.
- Pushing, tagging, running workflows and publishing releases need the maintainer's go-ahead each time (Tasks 7 and 8 stop for it).

## Rulings against the spec (made while planning, each from a checked fact)

1. **Recipe file is `.github/hosts.json`, not `hosts.toml`.** `serde_json` is already a `gate` dependency and PowerShell reads JSON natively; TOML would add a crate and a parser step.
2. **External packages hash name + version + source (+ resolved features), not a lockfile checksum.** `cargo metadata` omits the checksum, and a registry version is immutable. Git sources carry their revision in `source`.
3. **Host releases are not drafts.** `gh` cannot look a draft up by tag. `hosts.yml` creates the release with the host files, downloads them back and verifies, then uploads `pin.json` last; "the release exists" means "it has a `pin.json` asset", and a failed verify deletes the release.
4. **The fingerprint reads committed state** (`git ls-tree -r HEAD`), so `gate host-fingerprint` refuses uncommitted changes in the closure's directories, `rust-toolchain.toml` and `.github/hosts.json` unless given `--allow-dirty`.
5. **`scripts/release.sh` is deleted in this phase, not after P3.** The new URL form breaks its `--asset`/`--keep-from` path, and `release.yml` replaces the rest. Servo and WebKit read Unavailable until P2/P3 (the spec's P1 scope says so).
6. **The tag message is `manifold vX.Y.Z`, a blank line, then the changelog entry's body** (checked against `git tag -l --format='%(contents)' v0.6.0`), so `gate release-check` compares the body, not the heading.

## Review Focus

- A source change that touches no host (docs, `manifold` crate, another engine's crate) must leave CEF's fingerprint unchanged (Task 1 test: closure excludes `manifold` and `manifold-view-servo`).
- A dev-dependency or an unrelated workspace crate edit must not change a fingerprint (Task 1 synthetic-metadata test).
- A pin table whose URL carries a stale or foreign fingerprint, or no `hosts-` segment, must fail `--check` with the entry named (Task 2 test).
- `--adopt` against a release whose `pin.json` is for another kind, platform or fingerprint, or whose file hash or size differs, must refuse and leave the table untouched (Task 2 tests).
- A tag whose message does not match `CHANGELOG.md`, or whose `Cargo.toml` version differs, must fail before any build (Task 3 tests).

## File Structure

- Create `crates/gate/src/fingerprint.rs`: `host-fingerprint`, the cargo-metadata closure, the digest.
- Create `crates/gate/src/release_check.rs`: `release-check`, `release-notes`, changelog/tag helpers.
- Modify `crates/gate/src/pin.rs`: table render/parse without `PIN_VERSION`; `--check`, `--adopt [--from DIR]`, `--manifest`; delete `--version/--keep-from/--asset/--list`.
- Modify `crates/gate/src/release_verify.rs`: verify the table by download, the exe and embedded records in a dist dir.
- Modify `crates/gate/src/main.rs`: modules, dispatch, usage.
- Modify `crates/manifold/src/host_pin.rs`: drop `PIN_VERSION`, doc, `check_table`.
- Create `.github/hosts.json`, `.github/workflows/hosts.yml`, `.github/workflows/release.yml`; modify `.github/workflows/ci.yml`.
- Delete `scripts/release.sh`; modify README, CHANGELOG, doc comments that name it.

---

### Task 1: `gate host-fingerprint` and the recipe file

**Files:**
- Create: `.github/hosts.json`, `crates/gate/src/fingerprint.rs`
- Modify: `crates/gate/src/pin.rs` (make `hex` `pub(crate)`), `crates/gate/src/main.rs` (module, dispatch, usage)

**Interfaces:**
- Produces: `fingerprint::fingerprint(root: &Path, kind: &str, platform: &str, require_clean: bool) -> Result<String, String>` (12 hex digits); `fingerprint::host_fingerprint_main(args: &[String]) -> ExitCode`; `fingerprint::digest(parts: &[String]) -> String`.
- Consumes: `pin::hex(&[u8]) -> String`, `pin::repo() -> PathBuf`.

- [ ] **Step 1: Write the recipe file** `.github/hosts.json`:

```json
{
  "cef": {
    "windows-x64": {
      "runner": "windows-latest",
      "package": "manifold-view-cef",
      "rustflags": "-C target-feature=+crt-static",
      "files": ["manifold-engine-cef.exe"]
    }
  }
}
```

(`files` are paths under `target/release/`. List here any environment variable that changes the binary.)

- [ ] **Step 2: Write the failing tests.** Create `crates/gate/src/fingerprint.rs` with the module doc, the tests below and `todo!()`-free stubs only where needed to compile (`fn closure_of(..) -> Result<Vec<Pkg>, String> { Err(String::new()) }` etc. is fine). Tests:

```rust
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
            run(&dir, "git", &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap()
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
```

- [ ] **Step 3: Run to see them fail.** `cargo test -j 8 -p gate fingerprint` → compile errors or failures (functions missing).

- [ ] **Step 4: Implement.** Module body of `fingerprint.rs` (above the tests):

```rust
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

const TRIPLES: [(&str, &str); 2] = [("windows-x64", "x86_64-pc-windows-msvc"), ("linux-x64", "x86_64-unknown-linux-gnu")];

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
            let rel = manifest.strip_prefix(ws).ok_or(format!("{manifest} is outside {ws}"))?.trim_start_matches(['/', '\\']);
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
        out.push(Pkg { id: format!("{name} {version} {}", source.unwrap_or("path")), dir, features });
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
        paths.extend(["rust-toolchain.toml", ".github/hosts.json"]);
        if dirty(root, &paths)? {
            return Err(format!("uncommitted changes under {paths:?}: commit them or pass --allow-dirty"));
        }
    }
    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml")).map_err(|e| e.to_string())?;
    let mut parts = vec![format!("recipe {recipe}"), format!("toolchain {toolchain}")];
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
```

In `pin.rs` change `fn hex` to `pub(crate) fn hex`. In `main.rs` add `#[cfg(windows)] mod fingerprint;` beside `mod pin;`, the dispatch arm `"host-fingerprint" => crate::fingerprint::host_fingerprint_main(&std::env::args().skip(2).collect::<Vec<_>>()),` next to `"host-pin"`, and `gate host-fingerprint KIND PLATFORM [--allow-dirty] | ` in the usage string.

- [ ] **Step 5: Run tests.** `cargo test -j 8 -p gate fingerprint` → all pass (`the_same_tree...` needs the committed recipe: `git add .github/hosts.json` first is not required because `require_clean` is false there). Fix `the_closure...` if `manifest_path` handling differs (it must yield `crates/a`).

- [ ] **Step 6: Try it.** Commit first (Step 7), then `target/debug/gate.exe host-fingerprint cef windows-x64` prints 12 hex digits twice identically; `host-fingerprint servo windows-x64` fails with "no recipe".

- [ ] **Step 7: Commit.** `cargo fmt --all`, clippy as in Global Constraints, then
`git add .github/hosts.json crates/gate/src/fingerprint.rs crates/gate/src/pin.rs crates/gate/src/main.rs && git commit -m "gate host-fingerprint: a downloadable host's identity from its cargo closure"`.

---

### Task 2: Pins carry the fingerprint: drop `PIN_VERSION`, add `--check`, `--adopt`, `--manifest`

**Files:**
- Modify: `crates/manifold/src/host_pin.rs`, `crates/gate/src/pin.rs`, `crates/gate/src/release_verify.rs` (only the `PIN_VERSION` lines, so it compiles; Task 3 rewrites it)

**Interfaces:**
- Consumes: `fingerprint::fingerprint(root, kind, platform, true) -> Result<String, String>`.
- Produces in `pin.rs`: `pub(crate) type Table = BTreeMap<(String, String), Files>` and `type Files = BTreeMap<String, (String, u64, String)>` (url, size, sha256) made `pub(crate)`; `pub(crate) fn read_table(path: &Path) -> Result<Table, String>`; `pub(crate) fn table_sha256() -> Result<String, String>`; `pub(crate) fn stale_entries(table: &Table, fp: &dyn Fn(&str, &str) -> Result<String, String>) -> Vec<String>`; `pub(crate) fn pins_are_fresh() -> Result<(), Vec<String>>`; `pub(crate) fn download(url: &str, dest: &Path) -> Result<(), String>`; `pub(crate) const RELEASES`.
- CLI: `gate host-pin --check | --adopt KIND PLATFORM [--from DIR] | --manifest KIND PLATFORM FP DIR`, all with optional `--out FILE` (default `crates/manifold/src/host_pin.rs`). `--version`, `--keep-from`, `--asset`, `--list` are deleted.

- [ ] **Step 1: Discard the dirty table.** `git checkout -- crates/manifold/src/host_pin.rs` (it holds a v0.6.0 pin for a host that was never uploaded; spec open point).

- [ ] **Step 2: Edit `host_pin.rs`.** In the generated region delete the three `PIN_VERSION` lines (doc comment, `cfg_attr`, `pub const`) and the blank line after them, so the region is `// BEGIN TABLE\n#[rustfmt::skip]\nconst TABLE...`. Change the module doc to: "Everything between `// BEGIN TABLE` and `// END TABLE` is generated by `gate host-pin --adopt`; do not edit it by hand. A pin names the exact file a `hosts-KIND-FP` release holds (PE output is not bit-reproducible, so a rebuild is a different artifact), and the URL carries the source fingerprint `gate host-fingerprint` computes; `gate host-pin --check` fails when it is stale." Change the `Component::url` doc to `.../releases/download/hosts-<kind>-<fingerprint>/<file>-<platform>`. Change `check_table` to `fn check_table(table: &[Entry]) -> Result<(), String>`: delete the `pin_version`/`app_version` parameters, the version check and its doc sentence. In the tests: delete `an_empty_table_skips_the_version_check` and `a_stale_pin_version_fails_once_the_table_is_non_empty`; change every `check_table(X, "1.2.3", "1.2.3")` to `check_table(X)`; the checked-in test becomes `check_table(TABLE).unwrap()`; change the `URL_CEF`/`URL_SERVO` constants to `.../download/hosts-cef-0123456789ab/manifold-engine-cef.exe-windows-x64` and the servo equivalent with `hosts-servo-0123456789ab`.

- [ ] **Step 3: Write failing tests in `pin.rs`.** Replace `the_checked_in_tables_round_trip` body's version handling (`render_table(&parse_table(body).unwrap())`) and add:

```rust
    fn table_of(kind: &str, platform: &str, url: &str) -> Table {
        let mut t = Table::new();
        t.entry((kind.into(), platform.into())).or_default().insert("a.exe".into(), (url.into(), 3, "0".repeat(64)));
        t
    }

    #[test]
    fn a_pin_url_carries_its_fingerprint() {
        let ok = format!("{RELEASES}hosts-cef-0123456789ab/a.exe-windows-x64");
        assert_eq!(url_fingerprint(&ok, "cef"), Some("0123456789ab"));
        assert_eq!(url_fingerprint(&ok, "servo"), None);
        assert_eq!(url_fingerprint(&format!("{RELEASES}v0.5.0/a.exe-windows-x64"), "cef"), None);
        assert_eq!(url_fingerprint("https://example.com/hosts-cef-0123456789ab/a", "cef"), None);
    }

    #[test]
    fn stale_entries_name_the_entry_and_the_fix() {
        let t = table_of("cef", "windows-x64", &format!("{RELEASES}hosts-cef-0123456789ab/a.exe-windows-x64"));
        assert!(stale_entries(&t, &|_, _| Ok("0123456789ab".into())).is_empty());
        let stale = stale_entries(&t, &|_, _| Ok("ffffffffffff".into()));
        assert_eq!(stale.len(), 1);
        assert!(stale[0].contains("cef/windows-x64") && stale[0].contains("--adopt cef windows-x64"), "{stale:?}");
        let old = table_of("cef", "windows-x64", &format!("{RELEASES}v0.5.0/a.exe-windows-x64"));
        assert_eq!(stale_entries(&old, &|_, _| Ok("0123456789ab".into())).len(), 1);
        assert_eq!(stale_entries(&t, &|_, _| Err("no recipe".into())).len(), 1);
    }

    fn release_dir(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.exe-windows-x64"), bytes).unwrap();
        write_manifest(&dir, "cef", "windows-x64", "0123456789ab").unwrap();
        dir
    }

    fn copy_from(dir: PathBuf) -> impl Fn(&str, &Path) -> Result<(), String> {
        move |asset, dest| std::fs::copy(dir.join(asset), dest).map(|_| ()).map_err(|e| e.to_string())
    }

    #[test]
    fn adopt_replaces_one_entry_and_keeps_the_rest() {
        let dir = release_dir("ok", b"abc");
        let m = parse_manifest(&std::fs::read_to_string(dir.join("pin.json")).unwrap()).unwrap();
        let mut t = table_of("servo", "windows-x64", "https://example.com/hosts-servo-111111111111/a.exe-windows-x64");
        t.extend(table_of("cef", "windows-x64", "https://example.com/old/a.exe-windows-x64"));
        adopt(&mut t, "cef", "windows-x64", "0123456789ab", &m, &copy_from(dir.clone())).unwrap();
        let (url, size, sha) = &t[&("cef".into(), "windows-x64".into())]["a.exe"];
        assert_eq!(url, &format!("{RELEASES}hosts-cef-0123456789ab/a.exe-windows-x64"));
        assert_eq!((*size, sha.len()), (3, 64));
        assert!(t.contains_key(&("servo".into(), "windows-x64".into())));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adopt_refuses_a_foreign_manifest_or_a_changed_file_and_leaves_the_table_alone() {
        let dir = release_dir("bad", b"abc");
        let m = parse_manifest(&std::fs::read_to_string(dir.join("pin.json")).unwrap()).unwrap();
        let before = table_of("cef", "windows-x64", "https://example.com/old/a.exe-windows-x64");
        let mut t = before.clone();
        assert!(adopt(&mut t, "cef", "windows-x64", "ffffffffffff", &m, &copy_from(dir.clone())).is_err());
        assert!(adopt(&mut t, "servo", "windows-x64", "0123456789ab", &m, &copy_from(dir.clone())).is_err());
        std::fs::write(dir.join("a.exe-windows-x64"), b"abd").unwrap();
        assert!(adopt(&mut t, "cef", "windows-x64", "0123456789ab", &m, &copy_from(dir.clone())).is_err());
        assert_eq!(t, before);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_manifest_lists_every_platform_file_with_size_and_hash() {
        let dir = release_dir("mf", b"abc");
        std::fs::write(dir.join("other.dll-windows-x64"), b"xy").unwrap();
        std::fs::write(dir.join("ignored.txt"), b"zz").unwrap();
        write_manifest(&dir, "cef", "windows-x64", "0123456789ab").unwrap();
        let m = parse_manifest(&std::fs::read_to_string(dir.join("pin.json")).unwrap()).unwrap();
        let names: Vec<&str> = m.files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names, ["a.exe", "other.dll"]);
        assert_eq!(m.files[1].1, 2);
        let _ = std::fs::remove_dir_all(dir);
    }
```

- [ ] **Step 4: Run to see them fail** (`cargo test -j 8 -p gate pin`): compile errors for the missing functions.

- [ ] **Step 5: Implement in `pin.rs`.** Replace the module doc's `host-pin` paragraph with: "`gate host-pin --check` fails if any entry's URL carries a fingerprint other than `gate host-fingerprint`'s for the current tree (also run in ci.yml and release.yml). `gate host-pin --adopt KIND PLATFORM [--from DIR]` takes `pin.json` and the files of the `hosts-KIND-FP` release (or of DIR, for tests and offline use), verifies each file's size and SHA-256, and rewrites that entry. `gate host-pin --manifest KIND PLATFORM FP DIR` writes `DIR/pin.json` for the files named `FILE-PLATFORM` in DIR (hosts.yml uses it). All take `--out FILE` (default crates/manifold/src/host_pin.rs). The rewrite is atomic and leaves the file alone on any refusal." Delete the old `host_pin` body and write:

```rust
pub(crate) const RELEASES: &str = "https://github.com/winstonyli/manifold/releases/download/";
```
(make the existing const `pub(crate)`), make `Files` and `Table` `pub(crate)`, and:

```rust
fn release_tag(kind: &str, fp: &str) -> String {
    format!("hosts-{kind}-{fp}")
}

/// The fingerprint a pin URL carries (`.../download/hosts-KIND-FP/FILE-PLATFORM`), if it has that shape.
fn url_fingerprint<'a>(url: &'a str, kind: &str) -> Option<&'a str> {
    url.strip_prefix(RELEASES)?.split('/').next()?.strip_prefix(&format!("hosts-{kind}-"))
}

/// One line per entry file whose URL does not carry the fingerprint `fp` computes for its kind and platform.
pub(crate) fn stale_entries(table: &Table, fp: &dyn Fn(&str, &str) -> Result<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for ((kind, platform), files) in table {
        let want = match fp(kind, platform) {
            Ok(w) => w,
            Err(e) => {
                out.push(format!("{kind}/{platform}: {e}"));
                continue;
            }
        };
        for (file, (url, ..)) in files {
            let got = url_fingerprint(url, kind);
            if got != Some(want.as_str()) {
                out.push(format!(
                    "{kind}/{platform} {file}: pinned for {got:?}, sources are {want}: run the hosts workflow for {kind}, then `gate host-pin --adopt {kind} {platform}` and commit"
                ));
            }
        }
    }
    out
}

fn host_pin_path() -> PathBuf {
    repo().join("crates/manifold/src/host_pin.rs")
}

pub(crate) fn read_table(path: &Path) -> Result<Table, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (b, e) = region(&text).map_err(|m| format!("{}: {m}", path.display()))?;
    parse_table(&text[b..e])
}

/// SHA-256 of the generated table text, for the release notes.
pub(crate) fn table_sha256() -> Result<String, String> {
    let path = host_pin_path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (b, e) = region(&text)?;
    Ok(hex(&Sha256::digest(text[b..e].as_bytes())))
}

/// Every pinned entry carries the fingerprint of the current tree.
pub(crate) fn pins_are_fresh() -> Result<(), Vec<String>> {
    let table = read_table(&host_pin_path()).map_err(|e| vec![e])?;
    let stale = stale_entries(&table, &|k, p| crate::fingerprint::fingerprint(&repo(), k, p, true));
    if stale.is_empty() { Ok(()) } else { Err(stale) }
}

fn write_table(out: &Path, table: &Table) -> Result<(), String> {
    let text = std::fs::read_to_string(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let (b, e) = region(&text).map_err(|m| format!("{}: {m}", out.display()))?;
    let new = format!("{}{}{}", &text[..b], render_table(table), &text[e..]);
    let tmp = out.with_extension("rs.new");
    std::fs::write(&tmp, new).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

pub(crate) fn download(url: &str, dest: &Path) -> Result<(), String> {
    curl(&["-o", &dest.to_string_lossy(), url]).map(|_| ())
}

/// What a `hosts-KIND-FP` release says it holds.
#[derive(Debug, PartialEq)]
struct Manifest {
    kind: String,
    platform: String,
    fingerprint: String,
    /// name (without the platform suffix), size, sha256
    files: Vec<(String, u64, String)>,
}

fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("pin.json: {e}"))?;
    let s = |k: &str| v[k].as_str().map(str::to_owned).ok_or(format!("pin.json has no `{k}`"));
    let mut files = Vec::new();
    for f in v["files"].as_array().ok_or("pin.json has no `files`")? {
        let field = |k: &str| f[k].as_str().map(str::to_owned).ok_or(format!("pin.json file without `{k}`"));
        files.push((field("name")?, f["size"].as_u64().ok_or("pin.json file without `size`")?, field("sha256")?));
    }
    Ok(Manifest { kind: s("kind")?, platform: s("platform")?, fingerprint: s("fingerprint")?, files })
}

/// Writes `dir/pin.json` for the files named `NAME-PLATFORM` in `dir`.
fn write_manifest(dir: &Path, kind: &str, platform: &str, fp: &str) -> Result<(), String> {
    let suffix = format!("-{platform}");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(&suffix))
        .collect();
    names.sort();
    let mut files = Vec::new();
    for asset in &names {
        let (sha, _, size) = hash_file(&dir.join(asset))?;
        if size == 0 {
            return Err(format!("{asset}: empty file"));
        }
        files.push(serde_json::json!({"name": &asset[..asset.len() - suffix.len()], "size": size, "sha256": sha}));
    }
    if files.is_empty() {
        return Err(format!("no *{suffix} files in {}", dir.display()));
    }
    let json = serde_json::json!({"kind": kind, "platform": platform, "fingerprint": fp, "files": files});
    std::fs::write(dir.join("pin.json"), serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// Replaces `kind`/`platform` in `table` with the files `m` lists, after fetching each through `fetch(asset, dest)`
/// and checking its size and SHA-256. `table` is untouched on any error.
fn adopt(
    table: &mut Table,
    kind: &str,
    platform: &str,
    fp: &str,
    m: &Manifest,
    fetch: &dyn Fn(&str, &Path) -> Result<(), String>,
) -> Result<(), String> {
    if (m.kind.as_str(), m.platform.as_str(), m.fingerprint.as_str()) != (kind, platform, fp) {
        return Err(format!(
            "pin.json is for {}/{} {}, wanted {kind}/{platform} {fp}",
            m.kind, m.platform, m.fingerprint
        ));
    }
    if m.files.is_empty() {
        return Err("pin.json lists no files".into());
    }
    let dir = std::env::temp_dir().join(format!("adopt-{}-{kind}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut files = Files::new();
        for (name, size, sha) in &m.files {
            let asset = format!("{name}-{platform}");
            let dest = dir.join(&asset);
            fetch(&asset, &dest)?;
            let (got, _, n) = hash_file(&dest)?;
            if &got != sha || n != *size {
                return Err(format!("{asset}: {n} bytes sha256 {got}, pin.json says {size} bytes sha256 {sha}"));
            }
            files.insert(name.clone(), (format!("{RELEASES}{}/{asset}", release_tag(kind, fp)), *size, sha.clone()));
        }
        Ok(files)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    table.insert((kind.to_owned(), platform.to_owned()), result?);
    Ok(())
}

fn host_pin(args: &[String]) -> Result<(), String> {
    let mut out = host_pin_path();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--out" {
            out = PathBuf::from(it.next().ok_or("--out needs a value")?);
        } else {
            rest.push(a.as_str());
        }
    }
    match rest.as_slice() {
        ["--check"] => {
            let table = read_table(&out)?;
            let stale = stale_entries(&table, &|k, p| crate::fingerprint::fingerprint(&repo(), k, p, true));
            if stale.is_empty() {
                eprintln!("host pins match the sources");
                Ok(())
            } else {
                Err(stale.join("\n"))
            }
        }
        ["--manifest", kind, platform, fp, dir] => write_manifest(Path::new(dir), kind, platform, fp),
        ["--adopt", kind, platform, tail @ ..] => {
            let from = match tail {
                [] => None,
                ["--from", dir] => Some(PathBuf::from(dir)),
                _ => return Err("usage: gate host-pin --adopt KIND PLATFORM [--from DIR]".into()),
            };
            let fp = crate::fingerprint::fingerprint(&repo(), kind, platform, true)?;
            let base = format!("{RELEASES}{}/", release_tag(kind, &fp));
            let manifest = match &from {
                Some(dir) => std::fs::read_to_string(dir.join("pin.json")).map_err(|e| e.to_string())?,
                None => String::from_utf8(curl(&[&format!("{base}pin.json")])?).map_err(|e| e.to_string())?,
            };
            let fetch = |asset: &str, dest: &Path| match &from {
                Some(dir) => std::fs::copy(dir.join(asset), dest).map(|_| ()).map_err(|e| e.to_string()),
                None => download(&format!("{base}{asset}"), dest),
            };
            let mut table = read_table(&out)?;
            adopt(&mut table, kind, platform, &fp, &parse_manifest(&manifest)?, &fetch)?;
            write_table(&out, &table)?;
            eprintln!("adopted {kind}/{platform} {fp} into {}", out.display());
            Ok(())
        }
        _ => Err("usage: gate host-pin --check | --adopt KIND PLATFORM [--from DIR] | --manifest KIND PLATFORM FP DIR [--out FILE]".into()),
    }
}
```

In `read_table` callers: `read_table(&out)` takes a path, as written. `render_table` becomes `fn render_table(table: &Table) -> String` with the format string starting `"#[rustfmt::skip]\nconst TABLE: &[Entry] = &[\n"` (no `PIN_VERSION`). `Table` needs `Clone`/`PartialEq` for the test: it is a `BTreeMap`, both derive already. Delete `KINDS`-independent leftovers that become unused (`PLATFORMS`, the `keep`/`assets` code). `PLATFORMS` is unused after this: delete it. The round-trip test becomes `assert_eq!(render_table(&parse_table(body).unwrap()), body);` with `version` lines removed.

In `release_verify.rs` delete lines 36–38 (the three `PIN_VERSION` lines) and the `want` variable use so it compiles; Task 3 rewrites the file.

- [ ] **Step 6: Regenerate the committed table text.** The region in `host_pin.rs` (Step 2) must equal `render_table` output byte for byte; run `cargo test -j 8 -p gate pin` and `cargo test -j 8 -p manifold host_pin` → pass. (The committed entry still has its v0.5.0 URL; `--check` fails until Task 7 adopts; that is expected and the reason Task 7 adds the CI check last.)

- [ ] **Step 7: Run clippy and the two test sets, then commit.** `git add crates && git commit -m "Host pins carry a source fingerprint: host-pin --check, --adopt, --manifest; PIN_VERSION removed"`.

---

### Task 3: `gate release-check`, `release-notes` and the `release-verify` rewrite

**Files:**
- Create: `crates/gate/src/release_check.rs`
- Modify: `crates/gate/src/release_verify.rs` (rewrite), `crates/gate/src/main.rs`

**Interfaces:**
- Consumes: `pin::{pins_are_fresh, read_table, table_sha256, download, hash_file, repo}`, `check::{check, note_failure, exit_code}`.
- Produces: `release_check::{changelog_body(changelog: &str, tag: &str) -> Option<String>, tag_message_matches(message: &str, tag: &str, body: &str) -> bool, workspace_version(cargo_toml: &str) -> Option<String>, release_check(args) -> ExitCode, release_notes(args) -> ExitCode, at_tag(tag: &str) -> bool}`; `gate release-verify vX.Y.Z [DIST]`.

- [ ] **Step 1: Failing tests** in `release_check.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\n\nintro\n\n## v0.6.0 (2026-10-16)\n\n### Features\n\n- one\n\n## v0.5.1 (2026-10-10)\n\n- old\n";

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
```

- [ ] **Step 2: Run, see them fail** (`cargo test -j 8 -p gate release_check`).

- [ ] **Step 3: Implement `release_check.rs`:**

```rust
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
```

- [ ] **Step 4: Run tests** → pass. `table_sha256` takes no argument in Task 2's version; it is called as written.

- [ ] **Step 5: Rewrite `release_verify.rs`.** New module doc: "`gate release-verify vX.Y.Z [DIST]`: re-checks a built release with the tag checked out. Every host the tag's host_pin.rs pins must download from its URL with the pinned size and SHA-256 (the network); the exe in DIST (default dist/) must match its .sha256; and the embedded WebView2 host and PDF view copies in DIST/embedded must match the recorded hashes. That the exe really contains those bytes is not checked here (build.rs embeds the file and hashes it at build time)." Body:

```rust
use std::path::Path;
use std::process::ExitCode;

use crate::check::{check, exit_code, note_failure};
use crate::pin::{download, hash_file, read_table, repo};
use crate::release_check::at_tag;

fn recorded(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()?.split_whitespace().next().map(str::to_owned)
}

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
            let ok = download(&url, &dest).and_then(|()| hash_file(&dest)).is_ok_and(|(got, _, n)| got == sha && n == size);
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
        matches_record(&dist.join("embedded/manifold-engine-webview2.exe"), &dist.join(format!("{stem}.embedded-host.sha256"))),
    );
    check(
        "the embedded PDF view copy matches the recorded hash",
        matches_record(&dist.join("embedded/manifold-view-pdfium.exe"), &dist.join(format!("{stem}.embedded-pdf-view.sha256"))),
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
```

In `main.rs`: add `#[cfg(windows)] mod release_check;`, dispatch arms `"release-check" => crate::release_check::release_check(&std::env::args().skip(2).collect::<Vec<_>>()),` and `"release-notes" => ...release_notes(...)`, and in the usage string `gate release-verify vX.Y.Z [DIST] | gate release-check vX.Y.Z | gate release-notes vX.Y.Z`.

- [ ] **Step 6: Try it against v0.6.0** (the checked-out tag is not HEAD, so the first check fails by design): `target/debug/gate.exe release-notes v0.6.0` prints the entry body and a 64-hex table hash; `release-check v0.6.0` shows the message and version lines passing and the HEAD line failing.

- [ ] **Step 7: Clippy, fmt, commit.** `git add crates && git commit -m "gate release-check and release-notes; release-verify downloads the pinned hosts and checks a dist dir"`.

---

### Task 4: `hosts.yml`

**Files:**
- Create: `.github/workflows/hosts.yml`

**Interfaces:**
- Consumes: `gate host-fingerprint`, `gate host-pin --manifest`, `.github/hosts.json` recipe fields `package`, `rustflags`, `files`.
- Produces: release `hosts-cef-<fp>` with assets `manifold-engine-cef.exe-windows-x64` and `pin.json`.

- [ ] **Step 1: Write the workflow:**

```yaml
name: hosts
# Builds the downloadable engine hosts named in .github/hosts.json and publishes each under a release named for its
# source fingerprint. A release without a pin.json is a failed run and is replaced.
on:
  workflow_dispatch:
    inputs:
      kind:
        description: Host kind (a key of .github/hosts.json)
        required: true
        default: cef
  push:
    branches: [main]
    paths:
      - crates/ipc-protocol/**
      - crates/manifold-view-*/**
      - Cargo.toml
      - Cargo.lock
      - rust-toolchain.toml
      - .github/hosts.json
      - .github/workflows/hosts.yml
permissions:
  contents: read
concurrency:
  group: hosts-cef-windows-x64
  cancel-in-progress: false
jobs:
  cef-windows-x64:
    if: github.event_name == 'push' || inputs.kind == 'cef'
    runs-on: windows-latest
    permissions:
      contents: write
    defaults:
      run:
        shell: pwsh
    env:
      GH_TOKEN: ${{ github.token }}
    steps:
      - uses: actions/checkout@v4
      - run: rustup toolchain install
      - run: cargo build --locked -p gate
      - name: Fingerprint, and whether the release exists
        id: fp
        run: |
          $fp = (& target/debug/gate.exe host-fingerprint cef windows-x64).Trim()
          if ($LASTEXITCODE -ne 0 -or $fp -notmatch '^[0-9a-f]{12}$') { throw "bad fingerprint '$fp'" }
          $assets = gh release view "hosts-cef-$fp" --json assets --jq '.assets[].name' 2>$null
          $state = 'none'
          if ($LASTEXITCODE -eq 0) { $state = if ($assets -contains 'pin.json') { 'done' } else { 'partial' } }
          "fp=$fp" >> $env:GITHUB_OUTPUT
          "state=$state" >> $env:GITHUB_OUTPUT
          Write-Host "hosts-cef-${fp}: $state"
          exit 0
      - name: Drop a release a failed run left behind
        if: steps.fp.outputs.state == 'partial'
        run: gh release delete "hosts-cef-${{ steps.fp.outputs.fp }}" --cleanup-tag --yes
      - name: Build
        if: steps.fp.outputs.state != 'done'
        run: |
          $r = (Get-Content .github/hosts.json -Raw | ConvertFrom-Json).cef.'windows-x64'
          $env:RUSTFLAGS = $r.rustflags
          cargo build --release --locked -p $r.package
          if ($LASTEXITCODE -ne 0) { exit 1 }
          New-Item -ItemType Directory -Force stage | Out-Null
          foreach ($f in $r.files) { Copy-Item "target/release/$f" "stage/$f-windows-x64" }
          & target/debug/gate.exe host-pin --manifest cef windows-x64 "${{ steps.fp.outputs.fp }}" stage
          if ($LASTEXITCODE -ne 0) { exit 1 }
      - name: Publish, verify the download, then add pin.json last
        if: steps.fp.outputs.state != 'done'
        run: |
          $tag = "hosts-cef-${{ steps.fp.outputs.fp }}"
          $files = Get-ChildItem stage -File | Where-Object Name -ne 'pin.json' | ForEach-Object FullName
          gh release create $tag --target $env:GITHUB_SHA --title $tag --notes "Engine host binaries for source fingerprint ${{ steps.fp.outputs.fp }}, built by the hosts workflow at $env:GITHUB_SHA." $files
          if ($LASTEXITCODE -ne 0) { exit 1 }
          New-Item -ItemType Directory -Force back | Out-Null
          gh release download $tag --dir back
          if ($LASTEXITCODE -ne 0) { gh release delete $tag --cleanup-tag --yes; exit 1 }
          $m = Get-Content stage/pin.json -Raw | ConvertFrom-Json
          foreach ($f in $m.files) {
            $p = "back/$($f.name)-windows-x64"
            $h = (Get-FileHash $p -Algorithm SHA256).Hash.ToLower()
            if ($h -ne $f.sha256 -or (Get-Item $p).Length -ne $f.size) {
              gh release delete $tag --cleanup-tag --yes
              throw "$p does not match pin.json after the round trip"
            }
          }
          gh release upload $tag stage/pin.json
          if ($LASTEXITCODE -ne 0) { gh release delete $tag --cleanup-tag --yes; exit 1 }
```

(`gh release delete --cleanup-tag` is the documented flag to remove the tag with the release.)

- [ ] **Step 2: Lint the YAML locally.** `python -P -c "import yaml,sys; yaml.safe_load(open('.github/workflows/hosts.yml'))" < /dev/null` if PyYAML exists; otherwise `gh workflow` cannot lint offline, so re-read the file for indentation. A real run is Task 7.

- [ ] **Step 3: Commit.** `git add .github/workflows/hosts.yml && git commit -m "hosts.yml: build and publish downloadable engine hosts by source fingerprint"`.

---

### Task 5: `release.yml`

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `gate release-check`, `release-notes`, `package`, `release-verify`; env `MANIFOLD_WEBVIEW2_HOST`, `MANIFOLD_PDFIUM_HOST` (read by `crates/manifold/build.rs`).
- Produces: a draft GitHub release for the tag with `manifold-vX.Y.Z-windows-x64.exe`, its `.sha256`, `.embedded-host.sha256`, `.embedded-pdf-view.sha256`.

- [ ] **Step 1: Write the workflow:**

```yaml
name: release
# A pushed vX.Y.Z tag builds manifold.exe against the pinned hosts and leaves a draft release to read and publish.
on:
  push:
    tags: ['v*']
permissions:
  contents: write
concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false
jobs:
  release:
    runs-on: windows-latest
    defaults:
      run:
        shell: pwsh
    env:
      GH_TOKEN: ${{ github.token }}
      TAG: ${{ github.ref_name }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: Keep the tag object (checkout may flatten an annotated tag)
        run: git fetch --force origin "refs/tags/${env:TAG}:refs/tags/${env:TAG}"
      - run: rustup toolchain install
      - run: cargo build --locked -p gate
      - name: The tag is one we can build
        run: |
          & target/debug/gate.exe release-check $env:TAG
          if ($LASTEXITCODE -ne 0) { exit 1 }
      - name: Embedded hosts and manifold.exe
        run: |
          $env:RUSTFLAGS = '-C target-feature=+crt-static'
          cargo build --release --locked -p manifold-view-webview2 -p manifold-view-pdfium
          if ($LASTEXITCODE -ne 0) { exit 1 }
          New-Item -ItemType Directory -Force dist/embedded | Out-Null
          Copy-Item target/release/manifold-engine-webview2.exe dist/embedded/
          Copy-Item target/release/manifold-view-pdfium.exe dist/embedded/
          $env:MANIFOLD_WEBVIEW2_HOST = (Resolve-Path dist/embedded/manifold-engine-webview2.exe).Path
          $env:MANIFOLD_PDFIUM_HOST = (Resolve-Path dist/embedded/manifold-view-pdfium.exe).Path
          cargo build --release --locked -p manifold
          if ($LASTEXITCODE -ne 0) { exit 1 }
          $stem = "manifold-$env:TAG-windows-x64"
          Copy-Item target/release/manifold.exe "dist/$stem.exe"
          function Record($path, $out) {
            $h = (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
            "$h  $(Split-Path $path -Leaf)" | Set-Content -Encoding ascii $out
          }
          Record "dist/$stem.exe" "dist/$stem.exe.sha256"
          Record "dist/embedded/manifold-engine-webview2.exe" "dist/$stem.embedded-host.sha256"
          Record "dist/embedded/manifold-view-pdfium.exe" "dist/$stem.embedded-pdf-view.sha256"
      - name: Package check and release-verify
        run: |
          $stem = "manifold-$env:TAG-windows-x64"
          & target/debug/gate.exe package "dist/$stem.exe"
          if ($LASTEXITCODE -ne 0) { exit 1 }
          & target/debug/gate.exe release-verify $env:TAG dist
          if ($LASTEXITCODE -ne 0) { exit 1 }
      - name: Draft release
        run: |
          $stem = "manifold-$env:TAG-windows-x64"
          & target/debug/gate.exe release-notes $env:TAG | Set-Content -Encoding utf8 notes.md
          if ($LASTEXITCODE -ne 0) { exit 1 }
          gh release create $env:TAG --draft --verify-tag --title $env:TAG --notes-file notes.md "dist/$stem.exe" "dist/$stem.exe.sha256" "dist/$stem.embedded-host.sha256" "dist/$stem.embedded-pdf-view.sha256"
          if ($LASTEXITCODE -ne 0) { exit 1 }
```

Caveats to check on the first run, and fix only in this file: `gate package` on a GPU-less runner; whether `cargo build -p manifold` needs the `depot_tools`-free PATH (the runner has none); `.sha256` file format equals `sha256sum`'s `HASH  NAME` (`matches_record` reads the first word only).

- [ ] **Step 2: Re-read the YAML for indentation** (Task 4 Step 2 lint if available).

- [ ] **Step 3: Commit.** `git add .github/workflows/release.yml && git commit -m "release.yml: a pushed tag builds manifold.exe and leaves a draft release"`.

---

### Task 6: Delete `release.sh`, update docs

**Files:**
- Delete: `scripts/release.sh`
- Modify: `README.md` (lines ~350–354 and any "Packaging"/"release.sh" text; add a "Releases" procedure), `CHANGELOG.md` (nothing yet: the v0.6.1 entry is Task 8), `crates/gate/src/package.rs` (doc lines 1 and 7, message at ~line 89), `crates/gate/src/shots.rs:31`, `crates/manifold/build.rs:2`, `crates/gate/src/pin.rs` module doc if still naming release.sh, `crates/manifold/src/host_pin.rs` module doc (done in Task 2)

- [ ] **Step 1: Find every reference.** `grep -rn "release\.sh\|reuse-hosts-from\|dist/hosts\|--keep-from\|--asset\|PIN_VERSION" README.md CLAUDE.md docs/*.md crates scripts .github .githooks` (ignore `docs/superpowers/` plans and specs from earlier dates: they are history, except mark nothing). Fix each:
  - `package.rs`: "the release exe `release.yml` builds" and "no release exe: pass one, or download the draft release's exe".
  - `shots.rs` and `build.rs`: "(set by release.yml)".
  - `tier.rs:242` test uses the literal path `scripts/release.sh` only to test the `scripts/` rule: change the literal to `scripts/foreground.ps1` if that path also yields `TIER light`, else leave.
- [ ] **Step 2: README.** Replace the "Bare-exe release" and "Packaging" bullets with: "Releases are built by GitHub Actions. `hosts.yml` builds each downloadable engine host (`.github/hosts.json` lists how) and publishes it as release `hosts-KIND-FP`, where FP is `gate host-fingerprint KIND PLATFORM` (a hash of the host's cargo dependency closure, the toolchain and the recipe). `crates/manifold/src/host_pin.rs` pins those files; `gate host-pin --check` (run by ci.yml) fails when a source change makes a pin stale; `gate host-pin --adopt KIND PLATFORM` rewrites the pin from the new release. Pushing an annotated tag `vX.Y.Z` runs `release.yml`: it runs `gate release-check`, builds the embedded WebView2 host and PDF view with the static CRT and `manifold.exe` with them, runs `gate package` and `gate release-verify`, and creates a draft release. Servo and WebKit hosts have no CI build yet and read Unavailable." Replace the release-procedure bullet (line ~350) with the four steps from the spec's "Release procedure after this lands". Keep the embedded-host bullets but say `release.yml` sets the variables.
- [ ] **Step 3: Delete.** `git rm scripts/release.sh`.
- [ ] **Step 4: Verify.** `cargo build -j 8 -p gate && cargo test -j 8 -p gate`, clippy as in Global Constraints, `grep` from Step 1 shows only history under `docs/superpowers/`.
- [ ] **Step 5: Commit.** `git add -A README.md crates scripts && git commit -m "Releases move to GitHub Actions: delete scripts/release.sh, update README and doc comments"`.

---

### Task 7: Bootstrap the first CEF host and turn the pin check on (needs the maintainer's go-ahead: pushes and runs workflows)

**Files:**
- Modify: `crates/manifold/src/host_pin.rs` (by `--adopt`), `.github/workflows/ci.yml`

- [ ] **Step 1: Ask to push `main`.** After approval: `git push origin main`. This also pushes the spec commit `3d376bc`. The push touches `crates/manifold-view-*/**` and `.github/hosts.json`, so `hosts.yml` starts by itself.
- [ ] **Step 2: Watch the run.** `gh run list --workflow hosts.yml --limit 1`, then `gh run watch <id> --exit-status` (a foreground command with a deadline, not a loop). Expect first-run fixes in `hosts.yml` only: missing build tools for `cef-dll-sys` (cmake, ninja, a CEF download), PATH. Fix, push, rerun (`gh workflow run hosts.yml -f kind=cef`). A failed run leaves no `pin.json`, so it is retried safely.
- [ ] **Step 3: Adopt.** On a clean tree at the pushed commit: `target/debug/gate.exe host-pin --adopt cef windows-x64`. Expect `adopted cef/windows-x64 <fp>`; `git diff` shows only the Component line (URL `hosts-cef-<fp>`, new size and sha256).
- [ ] **Step 4: Add the check to `ci.yml`** after the `cargo test` line of the `windows` job:

```yaml
      - run: cargo run --locked -p gate -- host-pin --check
```

- [ ] **Step 5: Verify.** `target/debug/gate.exe host-pin --check` prints `host pins match the sources`; `cargo test -j 8 -p manifold host_pin` passes; edit nothing else.
- [ ] **Step 6: Commit and ask to push.** `git add crates/manifold/src/host_pin.rs .github/workflows/ci.yml && git commit -m "Pin the CI-built CEF host; ci.yml checks pins against the sources"`. After approval, `git push origin main` and `gh run watch` the `ci` run to green.

---

### Task 8: First CI release, v0.6.1 (needs the maintainer's go-ahead: tag push publishes a draft)

**Files:**
- Modify: `Cargo.toml` (workspace `version = "0.6.1"`), `Cargo.lock`, `CHANGELOG.md`

- [ ] **Step 1: Ask which tag.** The spec recommends `v0.6.1` (the pushed `v0.6.0` never had matching pins or a release); the alternative moves `v0.6.0` with a force-push the maintainer must approve. Proceed with `v0.6.1` unless told otherwise.
- [ ] **Step 2: Bump.** Set the workspace version to `0.6.1`, run `cargo update -w --offline` or `cargo check -j 8 -p manifold` to refresh `Cargo.lock`, and add a new top `CHANGELOG.md` entry `## v0.6.1 (date)` with: "`v0.6.0` was tagged but never released; this is its first release." and an Internal item "Releases are built by GitHub Actions: downloadable hosts by source fingerprint (`hosts.yml`), the app and a draft release on a tag (`release.yml`); `scripts/release.sh` and `PIN_VERSION` are gone."
- [ ] **Step 3: Gate and commit.** Version bump does not change host fingerprints (the workspace version is not in a closure's file list; check with `host-pin --check`). Run `target/debug/gate.exe all LOG light` per CLAUDE.md after `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`, detached at BelowNormal; report CPU load and Defender state. `git commit -am "Release v0.6.1"`.
- [ ] **Step 4: Tag and push (after approval).** Write the message to a file: first line `manifold v0.6.1`, a blank line, then the changelog body (`target/debug/gate.exe release-notes v0.6.1` prints the body plus a table-hash line: use the body only). `git tag -a --cleanup=verbatim v0.6.1 -F msg.txt && git push origin main v0.6.1`.
- [ ] **Step 5: Watch and inspect.** `gh run watch` the `release` run; fix `release.yml` for runner differences (spec open points), delete the failed tag only with approval. When it is green, `gh release view v0.6.1` shows a draft with four assets. Download the exe to a scratch folder with `gh release download v0.6.1 --pattern '*.exe' --dir <scratch>` and run `target/debug/gate.exe package <that exe>` on this machine.
- [ ] **Step 6: Publish only on the maintainer's go-ahead:** `gh release edit v0.6.1 --draft=false`.

---

## Self-review

**Spec coverage.** D1–D2 fingerprint: Task 1. D3 per-kind releases: Task 4 (ruling 3 replaces draft). D4 URL fingerprint and `--check` in `ci.yml`: Tasks 2, 7. D5 `--adopt`: Task 2. D6 two workflows and recipe file: Tasks 1, 4, 5 (ruling 1). D7 `release.yml` steps: Task 5 with `release-check` (step 1 and 2 of the spec's list, Task 3), builds, `package`, `release-verify`, draft. D8 `release-verify` rewrite: Task 3. D9 delete: Tasks 2 (flags), 6 (`release.sh`, docs). D10 `PIN_VERSION`: Task 2. D11 loud failures: `release-check` fails a stale pin or bad tag; concurrency groups in both workflows (per-kind-platform rather than per-fingerprint, which serialises the same runs); permissions per workflow. D12 manual tagging: Task 8. Phases P2/P3 are out of scope here (separate plans). Testing section: fingerprint stability/sensitivity (Task 1), `--check` stale (Task 2), `--adopt` against a local dir (Task 2), real runs (Tasks 7, 8). The spec's "build a host twice and compare fingerprints" hole test is covered by `the_same_tree_gives_the_same_fingerprint` (same tree, same digits) but the build-affecting-environment hole is only documented in `fingerprint.rs`'s module doc and `hosts.json` note; no automatic check exists.

**Placeholder scan.** None; Task 6 Step 1 lists greps, not unspecified edits.

**Type consistency.** `Table`/`Files`/`read_table(&Path)`/`table_sha256()`/`pins_are_fresh()` (no arguments) are defined in Task 2 and used identically in Task 3; the Interfaces block in Task 2 lists `read_table(root: &Path)` and `table_sha256(root)` and `pins_are_fresh(root)` — the code is authoritative: `read_table(path: &Path)`, `table_sha256()`, `pins_are_fresh()`. `fingerprint(root, kind, platform, require_clean)` matches in Tasks 1 and 2. `write_manifest(dir, kind, platform, fp)` is used by `--manifest` and the tests.

**Known risks.** (1) The CEF build and `gate package` on a runner are unverified (spec open points); Tasks 7 and 8 absorb workflow fixes. (2) `closure_of` assumes `manifest_path` shares `workspace_root`'s prefix and separator style, as cargo emits them. (3) The `workspace.package` scan in `workspace_version` assumes the current `Cargo.toml` layout. (4) A fingerprint reads `HEAD`, so the pin commit must come after the commit that changes host sources, which the workflow makes the natural order.
