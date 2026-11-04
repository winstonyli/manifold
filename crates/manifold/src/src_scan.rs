//! The walk of `src/` that the source-text guards share (`deleted_names`, `profile::tests`, `names::tests`).

use std::path::PathBuf;

/// Every `.rs` file under this crate's `src/`.
pub(crate) fn src_files() -> Vec<PathBuf> {
    let mut stack = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
    let mut files = Vec::new();
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files
}

/// The files under `src/`, other than the guard's own `skip`, whose text contains any of `needles`, sorted.
pub(crate) fn offenders(needles: &[&str], skip: &str) -> Vec<String> {
    let mut found: Vec<String> = src_files()
        .into_iter()
        .filter(|path| !path.ends_with(skip))
        .filter(|path| {
            let text = std::fs::read_to_string(path).unwrap();
            needles.iter().any(|needle| text.contains(needle))
        })
        .map(|path| path.display().to_string())
        .collect();
    found.sort();
    found
}

#[test]
fn offenders_finds_a_needle_outside_the_skipped_file() {
    assert_eq!(offenders(&["fn offenders("], "src_scan.rs"), Vec::<String>::new());
    assert_eq!(offenders(&["fn offenders("], "nothing.rs").len(), 1);
}
