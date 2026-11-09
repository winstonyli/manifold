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
const APP_FIELDS_MAX: usize = 60;

/// Files over `PROD_LIMIT` today: path under `crates/` and the prod line count the file may not exceed.
const PROD_ALLOWED: &[(&str, usize)] = &[
    ("gate/src/shots.rs", 1497),
    ("manifold/src/card.rs", 1639),
    ("manifold/src/gpu.rs", 1542),
    ("manifold/src/store/journal.rs", 1492),
    ("gate/src/smoke.rs", 1258),
];
/// Inline test blocks at or over `TEST_BLOCK_LIMIT` today, with the block's line count.
const TEST_ALLOWED: &[(&str, usize)] =
    &[("manifold-hosts/src/lib.rs", 353), ("manifold-view-protocol/src/downloads.rs", 262)];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every `.rs` file of a crate (`src`, `tests` and `build.rs`), as `(path under crates/, source)`.
fn all_sources() -> Vec<(String, String)> {
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
        for dir in ["src", "tests"].map(|d| krate.path().join(d)).into_iter().filter(|d| d.is_dir()) {
            walk(&dir, &mut files);
        }
        let build = krate.path().join("build.rs");
        if build.is_file() {
            files.push(build);
        }
    }
    let mut found = Vec::new();
    for path in files {
        let rel = path.strip_prefix(crates_dir()).unwrap().to_string_lossy().replace('\\', "/");
        found.push((rel, std::fs::read_to_string(&path).unwrap()));
    }
    found.sort();
    found
}

/// `all_sources` without test files.
fn sources() -> Vec<(String, String)> {
    let is_test = |path: &str| {
        let name = path.rsplit('/').next().unwrap_or(path);
        name == "tests.rs" || name.ends_with("_tests.rs") || path.contains("/tests/")
    };
    all_sources().into_iter().filter(|(path, _)| !is_test(path)).collect()
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

/// A shot waits for what it shows. A sleep is long enough on the machine it was tuned on and races on a busier one, and
/// it cost `chrome-shots` over half its time.
#[test]
fn shots_do_not_wait_on_the_clock() {
    let is_shots = |path: &str| path == "gate/src/shots.rs" || path.starts_with("gate/src/shots/");
    let sleepers: Vec<String> =
        sources().into_iter().filter(|(p, s)| is_shots(p) && s.contains("\"SLEEP ")).map(|(p, _)| p).collect();
    assert!(sleepers.is_empty(), "{sleepers:?} send SLEEP: wait on a state instead (`Shots::await_state`)");
}

/// The app processes the gate starts outside `Instance`, which keeps them offline (`shots_net::Offline`), with the count
/// of `Command::new(exe` / `Command::new(&exe` in the file. The list only shrinks.
const RAW_APP_SPAWNS: &[(&str, usize)] = &[
    ("gate/src/launch_forms.rs", 1), // the command-line forms' exit codes; no tab loads
    ("gate/src/probes.rs", 2),       // `--import-json` and `--dump`: no window, no engine
    ("gate/src/profile.rs", 1),      // local content only; the samply path (profile.ps1) can't pass the environment
];

/// A gate check can't wait on the internet: the app starts through `Instance::launch`/`launch_env`, offline, and only
/// `gate package` reaches the network (`launch_online`).
#[test]
fn the_gate_launches_the_app_offline() {
    let mut problems = Vec::new();
    for (path, source) in sources()
        .iter()
        .filter(|(p, _)| p.starts_with("gate/src/") && !p.ends_with("/instance.rs") && !p.ends_with("/limits.rs"))
    {
        if source.contains("launch_online(") && path != "gate/src/package.rs" {
            problems.push(format!("{path}: calls `launch_online`; only gate package may"));
        }
        let raw = source.matches("Command::new(exe").count() + source.matches("Command::new(&exe").count();
        let allowed = RAW_APP_SPAWNS.iter().find(|(p, _)| p == path).map_or(0, |&(_, n)| n);
        if raw != allowed {
            problems.push(format!("{path}: {raw} raw app spawns, {allowed} allowed; launch through `Instance`"));
        }
    }
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

/// A panic guard in production code means a worker with its own queue: keyed background work goes through
/// `worker::Pool`, which catches panics once for every job.
#[test]
fn catch_unwind_lives_only_in_the_worker_pool() {
    let mut users = Vec::new();
    for (path, source) in sources() {
        if !path.starts_with("manifold/src/") {
            continue;
        }
        let production = source.lines().take(measure(&source).0);
        if production.filter(|l| !l.trim_start().starts_with("//")).any(|l| l.contains("catch_unwind(")) {
            users.push(path);
        }
    }
    assert_eq!(users, ["manifold/src/worker.rs"], "catch_unwind belongs to worker.rs alone");
}

/// The `thread::sleep(` calls in each host source, outside comments and inline tests. A host's loop waits on its engine
/// and its inbox (`WakeEvent`, or the engine's own wait), never on a clock: a count rises only with a parked stopgap
/// beside it, and otherwise only falls; a listed file that is gone fails too. Every `manifold-view-*` crate is scanned,
/// but of `manifold-view-protocol` only `host_env.rs` (the startup wait), absent here at 0; `transport.rs`'s
/// connect retries are not a host loop. The scan matches the literal `thread::sleep(` (every sleep in the host crates
/// is spelled that way), so an imported bare `sleep(` slips past it; other clock waits under 50 ms (`recv_timeout`,
/// `park_timeout`, a short `MsgWait` timeout; none is in a host crate) are caught only when `gate hostrate` is run by
/// hand (its still-page poll ceiling), as is a swap that keeps a file's count.
const HOST_SLEEPS: &[(&str, usize)] = &[
    ("manifold-view-cef/src/main.rs", 2), // loop poll and startup idle, until the CEF plan
    ("manifold-view-servo/src/main.rs", 1), // loop poll, until the Servo plan
    ("manifold-view-webkitgtk/src/main.rs", 1), // loop poll, until a paint signal is found
    ("manifold-view-webview2/src/main.rs", 2), // loop poll (the event-driven plan's Task 5); profile-busy retry
    ("manifold-view-webview2/src/window.rs", 1), // taskbar retry on its own thread, not the loop
];

#[test]
fn host_loops_do_not_sleep() {
    let scanned = |path: &str| {
        path.starts_with("manifold-view-")
            && (!path.starts_with("manifold-view-protocol/") || path == "manifold-view-protocol/src/host_env.rs")
    };
    let mut problems = Vec::new();
    let mut seen = Vec::new();
    for (path, source) in sources().into_iter().filter(|(p, _)| scanned(p)) {
        let production = source.lines().take(measure(&source).0);
        let count =
            production.filter(|l| !l.trim_start().starts_with("//")).filter(|l| l.contains("thread::sleep(")).count();
        let allowed = HOST_SLEEPS.iter().find(|(p, _)| *p == path).map_or(0, |&(_, n)| n);
        if count != allowed {
            problems.push(format!("{path}: {count} thread::sleep calls, {allowed} listed; wait on an event instead"));
        }
        seen.push(path);
    }
    for (path, _) in HOST_SLEEPS {
        if !seen.iter().any(|p| p == path) {
            problems.push(format!("{path}: listed but not found; delete its entry"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Rust items whose names start `VIEW_`, which `host_variables_are_named_only_in_host_env` would otherwise read as
/// host variables. An entry no code uses fails, so the list only shrinks.
const VIEW_ITEMS: &[&str] =
    &["VIEW_INBOX", "VIEW_JOB", "VIEW_KIND", "VIEW_KINDS", "VIEW_MESSAGES_PER_DRAIN", "VIEW_TO_HOST_VARIANTS"];

/// The `VIEW_…` words in `source`'s code lines (a comment line is skipped): a `VIEW_` not inside a longer word, with
/// the capitals, digits and underscores after it.
fn view_words(source: &str) -> Vec<&str> {
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut words = Vec::new();
    for line in source.lines().filter(|l| !l.trim_start().starts_with("//")) {
        for (at, _) in line.match_indices("VIEW_") {
            // A word byte before it makes a longer word, unless it ends an escape (`\n`, `\t`).
            let bytes = line.as_bytes();
            if at > 0 && is_word(bytes[at - 1]) && !(at > 1 && bytes[at - 2] == b'\\') {
                continue;
            }
            let len =
                line[at..].bytes().take_while(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_').count();
            words.push(&line[at..at + len]);
        }
    }
    words
}

/// A host's startup variable is named once, in `manifold_view_protocol::host_env`, and both the app that writes it and
/// the host that reads it use that constant: a name spelled on each side can drift apart silently. Any `VIEW_` word in
/// code elsewhere (in a string, a shell line, `format!("VIEW_{x}")`) fails unless it is one of `VIEW_ITEMS`.
#[test]
fn host_variables_are_named_only_in_host_env() {
    let mut spelled = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for (path, source) in all_sources() {
        if path == "manifold-view-protocol/src/host_env.rs" || path == "gate/src/limits.rs" {
            continue;
        }
        for word in view_words(&source) {
            if let Some(item) = VIEW_ITEMS.iter().find(|&&i| i == word) {
                used.insert(*item);
            } else {
                spelled.push(format!("{path}: {word}"));
            }
        }
    }
    assert!(spelled.is_empty(), "{spelled:#?} spell a host variable: use its `host_env::ENV_*` constant");
    let unused: Vec<_> = VIEW_ITEMS.iter().filter(|w| !used.contains(*w)).collect();
    assert!(unused.is_empty(), "{unused:?} are in no code: delete them from VIEW_ITEMS");
}

#[test]
fn view_words_finds_a_name_wherever_code_spells_it() {
    let code =
        "let a = \"VIEW_X\";\nformat!(\"{s}VIEW_Y=1\");\nformat!(\"VIEW_{x}\");\n// VIEW_NOTE\nPREVIEW_ROWS; VIEW_KIND\n\"A=1\\nVIEW_Z=2\\tVIEW_W\"";
    assert_eq!(view_words(code), ["VIEW_X", "VIEW_Y", "VIEW_", "VIEW_KIND", "VIEW_Z", "VIEW_W"]);
}

/// The store is the bottom layer: nothing in it, tests included, names the app above it, so it can become its own
/// crate (plan item CRT1). Only a `crate::` path is seen; one reached through a re-export is not.
#[test]
fn the_store_reaches_nothing_above_it() {
    let mut upward = Vec::new();
    for (path, source) in all_sources() {
        if path != "manifold/src/store.rs" && !path.starts_with("manifold/src/store/") {
            continue;
        }
        for line in source.lines().filter(|l| !l.trim_start().starts_with("//")) {
            if line.split("crate::").skip(1).any(|after| !after.starts_with("store::")) {
                upward.push(format!("{path}: {}", line.trim()));
            }
        }
    }
    assert!(upward.is_empty(), "the store names code above it: {upward:#?}");
}
