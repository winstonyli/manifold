//! A grep guard: names deleted from the code must not come back.

use std::path::Path;

/// A card's identity is `CardKind` (its type and tags) and its live data is `Runtime`; the enums that stood in for
/// both are gone. `level::Origin` (a level card's source) is live and is not on this list.
#[test]
fn no_source_names_a_deleted_enum() {
    let mut offenders = Vec::new();
    let needles = [
        "NodePayload",
        "ItemNodeKind",
        "item_node_kind",
        "GRAPH_DRAW_ORDER",
        "GroupPayload",
        "caps::bridge",
        "mod bridge",
        "as_group(",
    ];
    scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &needles, &mut offenders);
    assert_eq!(offenders, Vec::<String>::new(), "read the type through App::card_kind and Caps, see plan 2b-6");
}

/// Each host's layout is one row of `manifold_hosts`, read through `ViewDesc::host` (spec
/// 2026-10-29-host-table-design.md). The per-engine host fields, `HostSource`, the `Embedded` enum and its hash
/// variables, `CefPlatform`'s exe and runtime names and the in-place path helpers are gone. The live
/// `host_file_names` and `Host::runtime_lib` contain two of the old names, so those needles are `host_file_name(` and
/// `PLATFORM.runtime_lib`.
#[test]
fn no_source_names_a_deleted_host_layout_item() {
    let mut offenders = Vec::new();
    let needles = [
        "HostSource",
        "Embedded::",
        "PLATFORM.host_exe",
        "PLATFORM.exe_suffix",
        "PLATFORM.runtime_lib",
        "CefPlatform::runtime_lib",
        "beside_exe",
        "in_place_paths",
        "sibling_binary",
        "host_file_name(",
        "ViewDesc::dev_dir",
        "MANIFOLD_WEBVIEW2_SHA256",
        "MANIFOLD_PDFIUM_SHA256",
    ];
    scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &needles, &mut offenders);
    assert_eq!(offenders, Vec::<String>::new(), "read the host through `ViewDesc::host` and `manifold_hosts`");
}

fn scan(dir: &Path, needles: &[&str], offenders: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, needles, offenders);
        } else if path.extension().is_some_and(|ext| ext == "rs") && !path.ends_with("deleted_names.rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            if needles.iter().any(|needle| text.contains(needle)) {
                offenders.push(path.display().to_string());
            }
        }
    }
}
