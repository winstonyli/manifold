//! A grep guard: the names 2b-6 deleted must not come back (plan 2b-6, Ruling 12).

use std::path::Path;

/// A card's identity is `CardKind` (its type and tags) and its live data is `Runtime`; the enums that stood in for
/// both are gone. `LevelItemKind` is not on this list: it is live, and names a level's two item sources.
#[test]
fn no_source_names_a_deleted_enum() {
    let mut offenders = Vec::new();
    let needles = ["NodePayload", "ItemNodeKind", "item_node_kind", "GRAPH_DRAW_ORDER", "GroupPayload", "caps::bridge"];
    scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &needles, &mut offenders);
    assert_eq!(offenders, Vec::<String>::new(), "read the type through App::card_kind and Caps, see plan 2b-6");
}

fn scan(dir: &Path, needles: &[&str], offenders: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, needles, offenders);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && !path.ends_with("deleted_names.rs")
            && needles.iter().any(|needle| std::fs::read_to_string(&path).unwrap().contains(needle))
        {
            offenders.push(path.display().to_string());
        }
    }
}
