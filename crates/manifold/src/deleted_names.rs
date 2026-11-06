//! A grep guard: names deleted from the code must not come back.

/// A card's identity is `CardKind` (its type and tags) and its live data is `Runtime`; the enums that stood in for
/// both are gone. `level::Origin` (a level card's source) is live and is not on this list.
#[test]
fn no_source_names_a_deleted_enum() {
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
    let offenders = crate::src_scan::offenders(&needles, "deleted_names.rs");
    assert_eq!(offenders, Vec::<String>::new(), "read the type through App::card_kind and Caps, see plan 2b-6");
}

/// Each host's layout is one row of `manifold_hosts`, read through `ViewDesc::host` (spec
/// 2026-10-29-host-table-design.md). The per-view host fields, `HostSource`, the `Embedded` enum and its hash
/// variables, `CefPlatform`'s exe and runtime names and the in-place path helpers are gone. The live
/// `host_file_names` and `Host::runtime_lib` contain two of the old names, so those needles are `host_file_name(` and
/// `PLATFORM.runtime_lib`.
#[test]
fn no_source_names_a_deleted_host_layout_item() {
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
    let offenders = crate::src_scan::offenders(&needles, "deleted_names.rs");
    assert_eq!(offenders, Vec::<String>::new(), "read the host through `ViewDesc::host` and `manifold_hosts`");
}

/// A host is seeded on the UI thread when it lands (spec 2026-11-05-jar-seed-at-landing-design.md), so the jars have one
/// copy and a clear needs no count.
#[test]
fn no_source_names_the_spawn_threads_jar_copy() {
    let needles = ["jar_snapshots", "Snapshots", "seed_clears"];
    let offenders = crate::src_scan::offenders(&needles, "deleted_names.rs");
    assert_eq!(offenders, Vec::<String>::new(), "read the jar through `CookieState::jars`, on the UI thread");
}
