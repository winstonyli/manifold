//! Layout limits as a ratchet: new drift fails `cargo unit`, existing offenders sit in an allowlist at their current
//! size, and an entry that no longer matches its file fails too, so the lists only shrink. The limits are the end
//! state's size rules (plan docs/superpowers/plans/2026-11-04-restructuring-cleanup.md, WORKING_STYLE "split before
//! you add"); a file over its limit is split by responsibility (`foo.rs` + `foo/`), its tests moved to `foo/tests.rs`.

use std::path::{Path, PathBuf};

/// A file's production lines (everything before an inline `mod tests {`) may not exceed this.
const PROD_LIMIT: usize = 1200;
/// An inline `mod tests {` block (to the end of the file) may not reach this many lines: it moves to `foo/tests.rs`.
const TEST_BLOCK_LIMIT: usize = 250;
/// `struct App`'s direct fields. Lower it when fields move into groups (ARC7); never raise it.
const APP_FIELDS_MAX: usize = 61;

/// Files over `PROD_LIMIT` today: path under `crates/` and the prod line count the file may not exceed.
const PROD_ALLOWED: &[(&str, usize)] = &[
    ("gate/src/shots.rs", 1789),
    ("manifold/src/card.rs", 1662),
    ("manifold/src/gpu.rs", 1542),
    ("manifold/src/store/journal.rs", 1492),
    ("gate/src/smoke.rs", 1305),
    ("manifold/src/view_install.rs", 1206),
];
/// Inline test blocks at or over `TEST_BLOCK_LIMIT` today, with the block's line count.
const TEST_ALLOWED: &[(&str, usize)] = &[
    ("manifold-hosts/src/lib.rs", 353),
    ("manifold-view-protocol/src/transport.rs", 274),
    ("manifold-view-protocol/src/frame_sink.rs", 273),
    ("manifold-view-protocol/src/downloads.rs", 262),
];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every non-test `.rs` file under a crate's `src`, as `(path under crates/, source)`.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for krate in std::fs::read_dir(crates_dir()).unwrap().flatten() {
        let src = krate.path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    let mut found = Vec::new();
    for path in files {
        let rel = path.strip_prefix(crates_dir()).unwrap().to_string_lossy().replace('\\', "/");
        let name = path.file_name().unwrap().to_string_lossy();
        if name == "tests.rs" || name.ends_with("_tests.rs") || rel.contains("/tests/") {
            continue;
        }
        found.push((rel, std::fs::read_to_string(&path).unwrap()));
    }
    found.sort();
    found
}

/// `(prod lines, inline test block lines)` of a source: the block starts at a `#[cfg(test)]` line directly above
/// `mod tests {` and runs to the end of the file.
fn measure(source: &str) -> (usize, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let at = lines.windows(2).position(|w| w[0] == "#[cfg(test)]" && w[1] == "mod tests {");
    match at {
        Some(i) => (i, lines.len() - i),
        None => (lines.len(), 0),
    }
}

/// Compares measured sizes against a ceiling table: over the limit needs an entry, an entry must equal the measure
/// (smaller means lower the ceiling, or delete it once the file is under the limit).
fn ratchet(what: &str, limit: usize, allowed: &[(&str, usize)], measured: &[(String, usize)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (path, size) in measured {
        match allowed.iter().find(|(p, _)| p == path) {
            Some(&(_, ceiling)) if *size > ceiling => {
                problems.push(format!("{path}: {what} grew to {size}, ceiling {ceiling}; split it instead"));
            }
            Some(&(_, ceiling)) if *size < ceiling && *size >= limit => {
                problems.push(format!("{path}: {what} shrank to {size}; lower its ceiling from {ceiling}"));
            }
            Some(_) if *size < limit => {
                problems.push(format!("{path}: {what} is {size}, under {limit}; delete its entry"))
            }
            None if *size >= limit => problems.push(format!("{path}: {what} is {size}, over {limit}; split it")),
            _ => {}
        }
    }
    for (path, _) in allowed {
        if !measured.iter().any(|(p, _)| p == path) {
            problems.push(format!("{path}: listed but not found; delete its entry"));
        }
    }
    problems
}

#[test]
fn no_file_grows_past_the_production_size_limit() {
    let measured: Vec<(String, usize)> = sources().into_iter().map(|(p, s)| (p, measure(&s).0)).collect();
    let problems = ratchet("prod lines", PROD_LIMIT + 1, PROD_ALLOWED, &measured);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn inline_test_blocks_stay_under_the_split_threshold() {
    let measured: Vec<(String, usize)> = sources().into_iter().map(|(p, s)| (p, measure(&s).1)).collect();
    let problems = ratchet("test block", TEST_BLOCK_LIMIT, TEST_ALLOWED, &measured);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn app_does_not_gain_loose_fields() {
    let source = std::fs::read_to_string(crates_dir().join("manifold/src/main.rs")).unwrap();
    let fields = source
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("struct App {"))
        .skip(1)
        .take_while(|l| *l != "}")
        .map(str::trim)
        .filter(|l| !l.starts_with("//") && !l.starts_with('#'))
        .filter(|l| l.split_once(": ").is_some())
        .count();
    assert!(fields > 40, "App's fields were found ({fields})");
    assert!(fields <= APP_FIELDS_MAX, "App has {fields} fields, cap {APP_FIELDS_MAX}: put new state in a group");
    assert!(fields == APP_FIELDS_MAX, "App shrank to {fields} fields: lower APP_FIELDS_MAX");
}
