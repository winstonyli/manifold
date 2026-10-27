use super::*;
use serde_json::json;

/// Wave 2 (H1b): revealing the last of 20 000 objects moves the array's window to it: one page of rows, so one
/// page of cards.
#[test]
fn reveal_bounds_rows() {
    let root = json!({"items": (0..20_000).map(|n| json!({"n": n})).collect::<Vec<_>>()});
    let expanded = default_expanded(&root);
    let mut view = JsonView::default();
    assert!(reveal(&root, &mut view, &expanded, "/items/19999"));
    let cards = visible_cards(&root, &view, &expanded);
    let items = cards.iter().find(|c| c.pointer == "/items").expect("the array's card");
    assert_eq!((cards.len(), items.rows, items.skip, items.more), (22, 20, 19_980, 0));
}

/// Wave 2 (H1b): a view saved before `first` existed reads, lays out and writes back exactly as before.
#[test]
fn an_old_view_round_trips_without_first() {
    let text = r#"{"/items":{"collapsed":false,"pages":3}}"#;
    let view = JsonView::from_field(text);
    assert_eq!(view.first("/items"), 0);
    assert_eq!(view.pages("/items"), 3);
    assert_eq!(view.to_field(), text);
}

/// Wave 2 (H1b): a reveal near the window grows it as before; a far one moves it; `+ N earlier` grows it up.
#[test]
fn reveal_moves_the_window_and_earlier_grows_it_up() {
    let root = json!({"items": (0..1000).collect::<Vec<_>>()});
    let expanded = default_expanded(&root);
    let mut view = JsonView::default();
    reveal(&root, &mut view, &expanded, "/items/90"); // page 4: within 5 of the window's end
    assert_eq!((view.first("/items"), view.pages("/items")), (0, 5));
    reveal(&root, &mut view, &expanded, "/items/900"); // page 45: far
    assert_eq!((view.first("/items"), view.pages("/items")), (45, 1));
    show_earlier(&root, &mut view, "/items");
    assert_eq!((view.first("/items"), view.pages("/items")), (44, 2));
    let items = visible_cards(&root, &view, &expanded).into_iter().find(|c| c.pointer == "/items").unwrap();
    assert_eq!((items.skip, items.rows, items.more, items.lead()), (880, 40, 80, 1));
}

#[test]
fn view_round_trips_and_stores_nothing_when_empty() {
    assert_eq!(JsonView::default().to_field(), "");
    assert_eq!(JsonView::from_field(""), JsonView::default());
    assert!(JsonView::from_field("not json").is_opaque(), "kept, not read as no overrides");

    let mut view = JsonView::default();
    view.entry("/users/0").pos = Some([520.0, 20.0]);
    view.entry("/users/0").collapsed = Some(true);
    view.entry("/users").pages = Some(2);
    view.entry("/empty"); // an override with nothing set is not stored
    let text = view.to_field();
    assert_eq!(text, r#"{"/users":{"pages":2},"/users/0":{"pos":[520.0,20.0],"collapsed":true}}"#);
    let back = JsonView::from_field(&text);
    assert_eq!(back.get("/users/0"), view.get("/users/0"));
    assert_eq!(back.pages("/users"), 2);
    assert_eq!(back.pages("/elsewhere"), 1);
}

/// M5: one entry this build can't read (a newer build's shape) costs neither the others nor itself.
#[test]
fn an_entry_this_build_cannot_read_survives_an_edit() {
    let mut view = JsonView::from_field(r#"{"/a":{"pages":{"offset":0,"pages":2}},"/b":{"collapsed":true}}"#);
    assert_eq!(view.get("/b").and_then(|o| o.collapsed), Some(true), "the readable entry still loads");
    view.entry("/c").collapsed = Some(true);
    let back: Value = serde_json::from_str(&view.to_field()).unwrap();
    assert_eq!(back["/a"]["pages"]["offset"], 0, "written back as it was");
    assert_eq!(back["/b"]["collapsed"], true);
    assert_eq!(back["/c"]["collapsed"], true);
}

/// M5: an override's unknown keys, a non-object entry and text that isn't an object of overrides are all
/// written back; positions keep their short f32 form.
#[test]
fn unknown_keys_and_unreadable_text_are_written_back() {
    let mut view = JsonView::from_field(r#"{"/a":{"collapsed":true,"width":3},"/x":[]}"#);
    assert_eq!(view.get("/a").and_then(|o| o.collapsed), Some(true));
    view.entry("/a").collapsed = None;
    view.entry("/b").pos = Some([0.1, 0.2]);
    assert_eq!(view.to_field(), r#"{"/a":{"width":3},"/b":{"pos":[0.1,0.2]},"/x":[]}"#);
    let opaque = JsonView::from_field("[1]");
    assert!(opaque.is_opaque() && !opaque.is_empty());
    assert_eq!(opaque.to_field(), "[1]");
    assert!(JsonView::from_field("").is_empty());
}

/// The keys `from_field` treats as known are exactly the ones `CardOverride` writes.
#[test]
fn card_override_keys_are_the_fields_it_reads() {
    let full =
        CardOverride { pos: Some([1.0, 2.0]), collapsed: Some(true), color: Some(1), pages: Some(2), first: Some(3) };
    let Value::Object(object) = serde_json::to_value(&full).unwrap() else { panic!("an object") };
    assert_eq!(object.keys().map(String::as_str).collect::<Vec<_>>(), CARD_OVERRIDE_KEYS);
}

#[test]
fn stale_pointers_and_positions_are_dropped() {
    let root = json!({"a": [1, 2], "b": {"c": 3}});
    let mut view = JsonView::default();
    view.entry("/a").pages = Some(2);
    view.entry("/a/5").collapsed = Some(true);
    view.entry("/gone").pos = Some([1.0, 2.0]);
    view.entry("").pos = Some([0.0, 0.0]);
    view.retain_resolvable(&root);
    assert_eq!(view.0.keys().collect::<Vec<_>>(), ["", "/a"]);
    view.clear_positions();
    assert_eq!(view.0.keys().collect::<Vec<_>>(), ["/a"]);
}

#[test]
fn pointers_escape_tilde_and_slash() {
    assert_eq!(child_pointer("", "users"), "/users");
    assert_eq!(child_pointer("/a", "x/y~z"), "/a/x~1y~0z");
    assert_eq!(pointer_tokens("/a/x~1y~0z"), ["a", "x/y~z"]);
    assert!(pointer_tokens("").is_empty());
    assert_eq!(parent_pointer("/a/b"), Some("/a"));
    assert_eq!(parent_pointer("/a"), Some(""));
    assert_eq!(parent_pointer(""), None);
    assert_eq!(last_key("/a/x~1y"), Some("x/y".to_string()));
    assert_eq!(last_key(""), None);
}

#[test]
fn accessor_paths_quote_what_isnt_an_identifier() {
    let root = json!({"users": [{"address": {"city": "London"}}], "a b": {"he\"y": 1}, "x/y": [[5]]});
    assert_eq!(accessor_path(&root, "/users/0/address"), "users[0].address");
    assert_eq!(accessor_path(&root, "/a b/he\"y"), r#"["a b"]["he\"y"]"#);
    assert_eq!(accessor_path(&root, "/x~1y/0/0"), r#"["x/y"][0][0]"#);
    assert_eq!(accessor_path(&root, ""), "");
    assert_eq!(accessor_path(&json!([[1]]), "/0/0"), "[0][0]");
}

#[test]
fn copy_and_display_text() {
    assert_eq!(copy_value_text(&json!("plain")), "plain");
    assert_eq!(copy_value_text(&json!(1.5)), "1.5");
    assert_eq!(copy_value_text(&json!(null)), "null");
    assert_eq!(copy_value_text(&json!({"a": 1})), "{\n  \"a\": 1\n}");
    assert_eq!(display_text(&json!("say \"hi\"")), r#""say \"hi\"""#);
    assert_eq!(display_text(&json!({"a": 1, "b": 2})), "{2}");
    assert_eq!(display_text(&json!([1, 2, 3])), "[3]");
    assert_eq!(display_text(&json!(true)), "true");
    let long = "x".repeat(DISPLAY_MAX_CHARS + 5);
    assert_eq!(display_text(&json!(long)).chars().count(), DISPLAY_MAX_CHARS + 3);
    // two quotes and "…"
}

#[test]
fn labels_follow_the_spec_rule() {
    assert_eq!(label(&json!({"id": 7, "name": "Ada"})), Some("\"Ada\"".into()));
    assert_eq!(label(&json!({"key": "k", "title": "T"})), Some("\"T\"".into()));
    assert_eq!(label(&json!({"label": "L", "id": 1})), Some("\"L\"".into()));
    assert_eq!(label(&json!({"id": 1, "key": "k"})), Some("1".into()));
    assert_eq!(label(&json!({"key": "k"})), Some("\"k\"".into()));
    // Null and containers never label; the fallback takes the first short string.
    assert_eq!(label(&json!({"name": null, "id": {"x": 1}, "note": "short"})), Some("\"short\"".into()));
    assert_eq!(label(&json!({"long": "y".repeat(41), "n": 1})), None);
    assert_eq!(label(&json!({"s": "y".repeat(40)})), Some(format!("\"{}\"", "y".repeat(40))));
    assert_eq!(label(&json!([-0.12, 51.5])), Some("[-0.12, 51.5]".into()));
    assert_eq!(label(&json!([[1], 2])), None);
    assert_eq!(label(&json!(["a very long string indeed", "and more"])), None);
    assert_eq!(label(&json!([])), Some("[]".into()));
    assert_eq!(label(&json!("scalar")), None);
}

#[test]
fn visible_rows_cover_the_rows_the_rect_touches() {
    let r = |top, bottom| egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(220.0, bottom));
    assert_eq!(visible_rows(26.0, 25, r(137.0, 245.0)), 5..10);
    assert_eq!(visible_rows(26.0, 25, egui::Rect::EVERYTHING), 0..25, "the preview: every row");
    assert_eq!(visible_rows(26.0, 25, r(-500.0, 0.0)), 0..0, "above the rows");
    assert_eq!(visible_rows(26.0, 25, r(5000.0, 6000.0)), 25..25, "below them");
}

#[test]
fn row_of_and_hit_rows_take_canonical_children_only() {
    let root = json!({"a~b": {"x/y": 1, "z": 2}, "arr": [10, 20, 30]});
    assert_eq!(row_of(&root["a~b"], "/a~0b", "/a~0b/x~1y", 0..2), Some(0));
    assert_eq!(row_of(&root["a~b"], "/a~0b", "/a~0b/z", 0..1), None, "outside the window");
    assert_eq!(row_of(&root["arr"], "/arr", "/arr/2", 0..3), Some(2));
    assert_eq!(row_of(&root["arr"], "/arr", "/arr/02", 0..3), None, "not canonical");
    assert_eq!(row_of(&root["arr"], "/arr", "/arr", 0..3), None, "the card itself");
    assert_eq!(row_of(&root, "", "/arr/2", 0..2), None, "a grandchild");
    let hits = HashSet::from(["/a~0b/z", "/arr/0", "/arr/2", "/arr/02"].map(String::from));
    let rows = hit_rows_by_parent(&root, &hits);
    assert_eq!(rows.get("/a~0b"), Some(&vec![1]));
    assert_eq!(rows.get("/arr"), Some(&vec![0, 2]));
    assert_eq!(rows.len(), 2);
}

#[test]
fn entries_from_starts_at_an_entry() {
    let root = json!({"o": {"a": 1, "b": 2, "c": 3}, "arr": [10, 20, 30]});
    let keys = |v: &Value, start| entries_from(v, start).map(|(k, _)| k).collect::<Vec<_>>();
    assert_eq!(keys(&root["o"], 1), ["b", "c"]);
    assert_eq!(keys(&root["arr"], 2), ["2"]);
    assert!(keys(&root["arr"], 9).is_empty());
}

#[test]
fn entries_in_file_order_with_indices_for_arrays() {
    let object = json!({"z": 1, "a": 2});
    let keys: Vec<String> = entries(&object).map(|(k, _)| k).collect();
    assert_eq!(keys, ["z", "a"]); // preserve_order: file order, not sorted
    let array = json!(["x", "y"]);
    assert_eq!(entries(&array).map(|(k, _)| k).collect::<Vec<_>>(), ["0", "1"]);
    assert_eq!(entry_index(&object, "a"), Some(1));
    assert_eq!(entry_index(&array, "1"), Some(1));
    assert_eq!(entry_index(&array, "2"), None);
    assert!(is_card(&json!({"a": 1})) && is_card(&json!([0])));
    assert!(!is_card(&json!({})) && !is_card(&json!([])) && !is_card(&json!(3)));
}

fn users(n: usize) -> Value {
    json!({
        "page": 1,
        "users": (0..n).map(|i| json!({"id": i, "name": format!("u{i}"), "roles": ["a", "b"]})).collect::<Vec<_>>(),
        "meta": {"bbox": [[-0.12, 51.5], [0.33, 51.7]], "empty": {}},
    })
}

fn pointers(cards: &[Card]) -> Vec<&str> {
    cards.iter().map(|c| c.pointer.as_str()).collect()
}

#[test]
fn cards_page_objects_and_arrays_and_nest_arrays() {
    let root = users(45);
    let expanded = default_expanded(&root);
    let cards = visible_cards(&root, &JsonView::default(), &expanded);
    let users_card = cards.iter().find(|c| c.pointer == "/users").unwrap();
    assert_eq!((users_card.rows, users_card.more), (20, 25));
    // Only the first page's elements have cards; `{}` gets none.
    assert!(cards.iter().any(|c| c.pointer == "/users/19"));
    assert!(!cards.iter().any(|c| c.pointer == "/users/20"));
    assert!(!cards.iter().any(|c| c.pointer == "/meta/empty"));
    // A nested array is a card whose rows are arrays with cards of their own.
    assert!(cards.iter().any(|c| c.pointer == "/meta/bbox/0"));

    let mut view = JsonView::default();
    show_more(&mut view, "/users");
    let cards = visible_cards(&root, &view, &expanded);
    let users_card = cards.iter().find(|c| c.pointer == "/users").unwrap();
    assert_eq!((users_card.rows, users_card.more), (40, 5));
    assert_eq!(users_card.height(), HEADER_HEIGHT + ROW_HEIGHT * 41.0);

    let big_object: Value = (0..30).map(|i| (format!("k{i}"), json!(i))).collect::<serde_json::Map<_, _>>().into();
    let cards = visible_cards(&big_object, &JsonView::default(), &default_expanded(&big_object));
    assert_eq!((cards[0].rows, cards[0].more), (20, 10));
}

#[test]
fn default_expansion_stops_at_the_depth_and_the_budget() {
    let deep = json!({"a": {"b": {"c": {"d": {"e": 1}}}}});
    let expanded = default_expanded(&deep);
    let cards = visible_cards(&deep, &JsonView::default(), &expanded);
    assert_eq!(pointers(&cards), ["", "/a", "/a/b", "/a/b/c"]);
    assert!(!cards[2].collapsed && cards[3].collapsed); // depth 3 starts collapsed
    assert_eq!(cards[3].height(), HEADER_HEIGHT);

    // 20 users of 1 + 1 cards each fit; the budget stops before 150.
    let root = users(20);
    let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
    assert!(cards.len() <= EXPANSION_BUDGET, "{} cards", cards.len());
    let root =
        json!((0..20).map(|_| json!((0..20).map(|_| json!({"x": [1]})).collect::<Vec<_>>())).collect::<Vec<_>>());
    let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
    assert!(cards.len() <= EXPANSION_BUDGET + PAGE_SIZE, "{} cards", cards.len());
}

#[test]
fn scalar_and_empty_roots_are_one_header() {
    for root in [json!(3), json!("s"), json!({}), json!([])] {
        let cards = visible_cards(&root, &JsonView::default(), &default_expanded(&root));
        assert_eq!(pointers(&cards), [""]);
        assert_eq!((cards[0].rows, cards[0].more), (0, 0));
    }
}

#[test]
fn toggling_stores_only_a_difference_from_the_default() {
    let root = users(2);
    let expanded = default_expanded(&root);
    let mut view = JsonView::default();
    toggle(&mut view, &expanded, "/users");
    assert_eq!(view.get("/users").unwrap().collapsed, Some(true));
    assert!(is_collapsed(&view, &expanded, "/users"));
    toggle(&mut view, &expanded, "/users");
    assert_eq!(view.to_field(), "");
    // Expanding one card never collapses another:
    // `/meta/bbox/0` (depth 3, collapsed by default) holds only
    // scalars, so exactly one fewer card is collapsed afterwards.
    let collapsed = |view: &JsonView| visible_cards(&root, view, &expanded).iter().filter(|c| c.collapsed).count();
    let before = collapsed(&view);
    toggle(&mut view, &expanded, "/meta/bbox/0");
    assert_eq!(collapsed(&view), before - 1);
}

#[test]
fn reveal_expands_ancestors_and_pages_to_the_target() {
    let root = users(45);
    let expanded = default_expanded(&root);
    let mut view = JsonView::default();
    toggle(&mut view, &expanded, "/users");
    assert!(reveal(&root, &mut view, &expanded, "/users/33/roles/1"));
    assert!(!is_collapsed(&view, &expanded, "/users"));
    assert_eq!(view.pages("/users"), 2);
    let cards = visible_cards(&root, &view, &expanded);
    assert!(cards.iter().any(|c| c.pointer == "/users/33/roles"));
    assert!(!reveal(&root, &mut view, &expanded, "/users/33/roles/1"), "already visible");
}

#[test]
fn layout_is_columns_by_depth_with_parents_centred_and_overrides_local() {
    let root = json!({"a": {"x": 1}, "b": {"y": 2, "z": 3}});
    let expanded = default_expanded(&root);
    let cards = visible_cards(&root, &JsonView::default(), &expanded);
    let pos = tree_layout(&cards, &JsonView::default());
    assert_eq!(pos, tree_layout(&cards, &JsonView::default()), "deterministic");
    assert_eq!(pos[0], egui::pos2(0.0, 0.0), "root at the origin");
    let (a, b) = (1, 2);
    assert_eq!(pos[a].x, CARD_WIDTH + COLUMN_GAP);
    assert_eq!(pos[b].y, pos[a].y + cards[a].height() + SIBLING_GAP);
    let span_mid = (pos[a].y + pos[b].y + cards[b].height()) / 2.0;
    assert!((pos[0].y + cards[0].height() / 2.0 - span_mid).abs() < 0.01, "parent centred on its children");

    let mut view = JsonView::default();
    view.entry("/a").pos = Some([900.0, 900.0]);
    let moved = tree_layout(&cards, &view);
    assert_eq!(moved[a], egui::pos2(900.0, 900.0));
    assert_eq!((moved[0], moved[b]), (pos[0], pos[b]), "only that card moves");
}

#[test]
fn search_matches_keys_and_scalars_in_hidden_pages_and_keeps_ancestors() {
    let root = users(45);
    let result = search(&root, "U33");
    assert_eq!(result.matches, ["/users/33/name"]);
    assert!(result.hits.contains("/users/33/name"));
    for kept in ["", "/users", "/users/33", "/users/33/name"] {
        assert!(result.keep.contains(kept), "{kept}");
    }
    assert_eq!(search(&root, "bbox").matches, ["/meta/bbox"]);
    assert!(search(&root, "").matches.is_empty());
    // Array indices don't match.
    assert!(search(&json!(["x"]), "0").matches.is_empty());
    assert_eq!(count_nodes(&json!({"a": [1, 2]})), 4);
}

/// Results stop at MAX_MATCHES, and a cancelled search returns nothing.
#[test]
fn search_is_capped_and_cancellable() {
    let big = serde_json::Value::Array((0..MAX_MATCHES + 50).map(|i| serde_json::json!(format!("x{i}"))).collect());
    let result = search(&big, "x");
    assert_eq!(result.matches.len(), MAX_MATCHES);
    assert!(result.truncated);
    assert!(search_with(&big, "x", &|| true).is_none());
    assert!(!search(&serde_json::json!({"a": "x"}), "x").truncated);
    let exact = serde_json::Value::Array((0..MAX_MATCHES).map(|i| serde_json::json!(format!("x{i}"))).collect());
    let result = search(&exact, "x");
    assert_eq!(result.matches.len(), MAX_MATCHES);
    assert!(!result.truncated, "exactly MAX_MATCHES: nothing was left out");
}

/// Today's `search_with`, kept as M1's reference (wave 2).
fn reference_search(root: &Value, query: &str, cancelled: &dyn Fn() -> bool) -> Option<SearchResult> {
    let mut result = SearchResult::default();
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return Some(result);
    }
    // Depth-first, children pushed in reverse so they pop in document order.
    let mut stack: Vec<(String, Option<&str>, &Value)> = vec![(String::new(), None, root)];
    let mut visited = 0usize;
    while let Some((pointer, key, value)) = stack.pop() {
        visited += 1;
        if visited.is_multiple_of(4096) && cancelled() {
            return None;
        }
        let key_hit = key.is_some_and(|k| k.to_lowercase().contains(&needle));
        let value_hit = scalar_text(value).is_some_and(|t| t.to_lowercase().contains(&needle));
        if key_hit || value_hit {
            if result.matches.len() == MAX_MATCHES {
                result.truncated = true; // a match past the cap: there are more than it shows
                break;
            }
            for (i, _) in pointer.match_indices('/') {
                result.keep.insert(pointer[..i].to_string());
            }
            result.keep.insert(pointer.clone());
            result.hits.insert(pointer.clone());
            result.matches.push(pointer.clone());
        }
        let children: Vec<(String, Option<&str>, &Value)> = match value {
            Value::Object(map) => map.iter().map(|(k, v)| (child_pointer(&pointer, k), Some(k.as_str()), v)).collect(),
            Value::Array(items) => {
                items.iter().enumerate().map(|(i, v)| (child_pointer(&pointer, &i.to_string()), None, v)).collect()
            }
            _ => Vec::new(),
        };
        stack.extend(children.into_iter().rev());
    }
    Some(result)
}

/// Wave 2 (M1): the search's whole result equals today's, for queries chosen to hit its edge cases (case,
/// Unicode lowercasing, escaped keys, numbers, booleans, null, the root), at the match cap, and its cancel
/// checks come at the same visits.
#[test]
fn search_matches_reference() {
    let root = json!({
        "Name": "Kelvin \u{212A} and \u{130}stanbul",
        "kayak": "plain ascii",
        "a/b": {"c~d": "slash and tilde", "\u{DF}": "strasse ss"},
        "items": [1, 2.5, -3e10, true, false, null, "Item 9", {"item 9": ["x"]}],
        "~": "/",
        "": "empty key",
        "nested": {"deep": {"deeper": ["zzz?", 12_345_678_901_234_567_890u64]}},
    });
    let queries = [
        "",
        "a",
        "A",
        "k",
        "\u{212A}",
        "i\u{307}",
        "\u{130}",
        "\u{DF}",
        "ss",
        "~",
        "/",
        "~1",
        "c~d",
        "2.5",
        "-3",
        "e10",
        "true",
        "fal",
        "null",
        "item 9",
        "ITEM",
        "zzz",
        "nomatch",
        "1234567890",
        "empty",
    ];
    for q in queries {
        assert_eq!(search_with(&root, q, &|| false), reference_search(&root, q, &|| false), "{q:?}");
    }
    assert_eq!(
        search_with(&json!("root text"), "TEXT", &|| false),
        reference_search(&json!("root text"), "TEXT", &|| false)
    );

    let many = Value::Array((0..10_005).map(|i| json!(format!("m{i}"))).collect());
    let new = search(&many, "m");
    assert!(new.truncated && new.matches.len() == MAX_MATCHES);
    assert_eq!(Some(new), reference_search(&many, "m", &|| false));
    type SearchFn = fn(&Value, &str, &dyn Fn() -> bool) -> Option<SearchResult>;
    let checks = |f: SearchFn, stop_at: usize| {
        let n = std::cell::Cell::new(0);
        let result = f(&many, "m", &|| {
            n.set(n.get() + 1);
            n.get() >= stop_at
        });
        (result, n.get())
    };
    assert_eq!(checks(search_with, usize::MAX), checks(reference_search, usize::MAX), "the same cancel checks");
    assert_eq!(checks(search_with, 2), (None, 2), "a cancel stops it");
    assert_eq!(checks(reference_search, 2), (None, 2));
}

/// Wave 2 (M1): a UI-thread search (just under `WORKER_SEARCH_NODES`) of 14 000 items of 7 nodes, with many
/// matches and with none. Timing, not correctness, so ignored:
/// `cargo test --release -j 8 -p manifold --bins json_search_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn json_search_cost() {
    use crate::bench::{report, timed};
    let root = Value::Array(
        (0..14_000)
            .map(|n| json!({"id": n, "name": format!("item {n}"), "score": n as f64 * 0.37, "tags": ["a", "b"]}))
            .collect(),
    );
    assert!(count_nodes(&root) < WORKER_SEARCH_NODES, "{} nodes: searched on the UI thread", count_nodes(&root));
    for q in ["item 9", "zzz"] {
        report(&format!("search {q:?}, 14 000 items"), timed(20, || drop(search(&root, q))));
    }
}

#[test]
fn outline_rows_follow_open_containers_or_the_search() {
    let root = json!({"a": {"x": 1}, "b": [true]});
    let open = HashSet::from([String::new()]);
    let rows: Vec<_> = outline_rows(&root, &open, None).into_iter().map(|r| (r.pointer, r.depth)).collect();
    assert_eq!(rows, [("".into(), 0), ("/a".into(), 1), ("/b".into(), 1)]);
    let open = HashSet::from([String::new(), "/a".to_string()]);
    let rows: Vec<_> = outline_rows(&root, &open, None).into_iter().map(|r| r.pointer).collect();
    assert_eq!(rows, ["", "/a", "/a/x", "/b"]);
    let found = search(&root, "true");
    let rows: Vec<_> = outline_rows(&root, &HashSet::new(), Some(&found.keep)).into_iter().map(|r| r.pointer).collect();
    assert_eq!(rows, ["", "/b", "/b/0"]);
}

#[test]
fn titles_and_preview_sizes() {
    let root = users(3);
    assert_eq!(card_title(&root, "", "users.json"), "users.json");
    assert_eq!(card_title(&root, "/users/0", "users.json"), "users[0]");
    assert_eq!(card_title(&root, "/meta/bbox/1", "users.json"), "bbox[1]");
    assert_eq!(card_title(&root, "/meta", "users.json"), "meta");
    assert_eq!(preview_size(&root), egui::vec2(PREVIEW_CARD_WIDTH, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT * 3.0));
    let wide = json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5});
    assert_eq!(preview_size(&wide).y, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT * 5.0); // 4 rows and "+ 1 more"
    assert_eq!(preview_size(&json!(7)).y, PREVIEW_HEADER_HEIGHT + ROW_HEIGHT);
}
