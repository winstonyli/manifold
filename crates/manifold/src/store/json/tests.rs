use super::*;
use crate::store::model::{ActorId, DocId, Hlc, TYPE_GROUP};
use crate::store::ops::Op;
use crate::store::tree::{Leaf, Seg};

fn ids(actor: u64) -> impl FnMut() -> NodeId {
    let mut counter = 0;
    move || {
        counter += 1;
        NodeId { actor: ActorId(actor), counter }
    }
}

#[test]
fn an_inline_card_exports_without_a_path_and_round_trips() {
    let json = r##"{"nodes": [{"id": "a", "type": "application/json", "body": {"k": [1, 2]}}]}"##;
    let doc: JsonDoc = serde_json::from_str(json).unwrap();
    let imported = import(&doc, ids(7), Trust::Foreign).unwrap();
    let out = node_json("a", "application/json", &imported[0].fields, &imported[0].trees, None);
    assert_eq!(out.fields["body"], serde_json::json!({"k": [1, 2]}));
    assert!(!out.fields.contains_key("path"), "no path field for an inline card");
}

fn doc_from(json: &str, actor: u64) -> Doc {
    let parsed: JsonDoc = serde_json::from_str(json).unwrap();
    let mut doc = Doc::new(DocId([0; 16]));
    let mut op_ids = ids(actor + 100);
    for (i, body) in creates(&import(&parsed, ids(actor), Trust::Own).unwrap()).into_iter().enumerate() {
        doc.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
    }
    doc
}

const HAND_WRITTEN: &str = r##"{"nodes": [
        {"id": "g", "type": "application/vnd.manifold.group", "name": "Reading"},
        {"id": "t1", "type": "application/vnd.manifold.tab", "group": "g", "url": "https://a.example/", "pos": [10.5, -3], "view_kind": "cef", "pinned": true},
        {"id": "t2", "type": "application/vnd.manifold.tab", "group": "g", "parent": "t1", "url": "https://b.example/", "future_field": 7},
        {"id": "n", "type": "text/markdown", "text": "hi"}
    ]}"##;

const WITH_LINKS: &str = r##"{"nodes": [
        {"id": "a", "type": "text/markdown", "text": "a"},
        {"id": "b", "type": "text/markdown", "text": "b"},
        {"id": "ok", "type": "application/vnd.manifold.link", "from": "a", "to": "b"},
        {"id": "dangling", "type": "application/vnd.manifold.link", "from": "a", "to": "424242"},
        {"id": "loop", "type": "application/vnd.manifold.link", "from": "a", "to": "loop"}
    ]}"##;

#[test]
fn creates_drops_a_link_with_an_end_that_is_no_card_in_the_file() {
    let parsed: JsonDoc = serde_json::from_str(WITH_LINKS).unwrap();
    let imported = import(&parsed, ids(1), Trust::Foreign).unwrap();
    let made = creates(&imported);
    assert_eq!(made.len(), 3, "two notes and the whole link");
    assert_eq!(imported.len(), 5, "import keeps index alignment with the file");
}

#[test]
fn dump_shows_dangling_links_that_an_export_hides() {
    let mut doc = Doc::new(DocId([0; 16]));
    let mut op_ids = ids(200);
    let parsed: JsonDoc = serde_json::from_str(WITH_LINKS).unwrap();
    // Own trust keeps the dangling end as written, as a half-made link in a store would be.
    for (i, body) in import(&parsed, ids(1), Trust::Own).unwrap().iter().map(create_op).enumerate() {
        doc.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
    }
    let links = |j: JsonDoc| j.nodes.iter().filter(|n| n.kind() == Some(Kind::Link)).count();
    assert_eq!(links(doc_to_json(&doc)), 1);
    assert_eq!(links(doc_to_json_all(&doc)), 3);
}

#[test]
fn hand_written_short_ids_resolve_within_the_file() {
    let doc = doc_from(HAND_WRITTEN, 1);
    let find = |url: &str| doc.live().find(|(_, n)| n.get(field::URL) == FieldValue::Str(url.into())).unwrap();
    let (t1, tab1) = find("https://a.example/");
    let (_, tab2) = find("https://b.example/");
    let group = tab1.group.unwrap();
    assert_eq!(doc.nodes[&group].get(field::NAME), FieldValue::Str("Reading".into()));
    assert_eq!(tab2.group, Some(group));
    assert_eq!(tab2.get(field::PARENT), FieldValue::Id(Some(t1)));
    assert_eq!(tab1.get(field::POS), FieldValue::Pos(Some((10.5, -3.0))));
    assert_eq!(tab1.get(field::VIEW_KIND), FieldValue::ViewCode(2));
    assert_eq!(tab1.get(field::ANCHOR), FieldValue::Anchor(None), "an old export's pinned is dropped");
}

#[test]
fn black_is_a_color_not_the_absence_of_one() {
    let doc = doc_from(
        r##"{"nodes": [{"id": "l", "type": "application/vnd.manifold.label", "name": "x", "color": "#000000"}]}"##,
        1,
    );
    let (_, label) = doc.live().find(|(_, n)| n.kind() == Kind::Label).unwrap();
    assert_eq!(label.get(field::COLOR), FieldValue::Color(Some(0)));
    let json = serde_json::to_value(doc_to_json(&doc)).unwrap();
    assert!(json["nodes"].as_array().unwrap().iter().any(|n| n["color"] == "#000000"));
}

#[test]
fn the_origin_is_a_position_not_the_absence_of_one() {
    let doc = doc_from(r##"{"nodes": [{"id": "n", "type": "text/plain", "pos": [0.0, 0.0]}]}"##, 1);
    let (_, note) = doc.live().next().unwrap();
    assert_eq!(note.get(field::POS), FieldValue::Pos(Some((0.0, 0.0))));
    let json = serde_json::to_value(doc_to_json(&doc)).unwrap();
    assert_eq!(json["nodes"][0]["pos"], serde_json::json!([0.0, 0.0]));
}

#[test]
fn export_then_import_gives_the_same_graph_under_fresh_ids() {
    let doc = doc_from(HAND_WRITTEN, 1);
    let exported = serde_json::to_string(&doc_to_json(&doc)).unwrap();
    let again = doc_from(&exported, 2);
    // Same nodes, relabelled: exporting both with ids hidden matches.
    let shape = |doc: &Doc| {
        let json = doc_to_json(doc);
        let label: HashMap<String, usize> = json.nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
        let mut out: Vec<String> = json
            .nodes
            .iter()
            .map(|n| {
                let mut f = n.fields.clone();
                for key in ["group", "parent"] {
                    if let Some(Value::String(id)) = f.get(key) {
                        f.insert(key.into(), Value::from(label[id]));
                    }
                }
                format!("{} {}", label[&n.id], Value::Object(f)) // `f` holds the type
            })
            .collect();
        out.sort();
        out
    };
    assert_eq!(shape(&doc), shape(&again));
    assert!(again.nodes.keys().all(|id| id.actor == ActorId(2)));
}

#[test]
fn a_group_export_lists_the_group_first_then_its_subtree() {
    let doc = doc_from(HAND_WRITTEN, 1);
    let (group, _) = doc.live().find(|(_, n)| n.kind() == Kind::Group).unwrap();
    let export = subtree_to_json(&doc, group);
    assert_eq!(
        export.nodes.iter().map(JsonNode::kind).collect::<Vec<_>>(),
        [Some(Kind::Group), Some(Kind::Tab), Some(Kind::Tab)]
    );
    assert!(!export.nodes[0].fields.contains_key("group"));
}

#[test]
fn bad_input_is_an_error() {
    let bad = |json: &str| import(&serde_json::from_str(json).unwrap(), ids(1), Trust::Own).unwrap_err();
    bad(r#"{"nodes": [{"id": "a", "type": "application/vnd.manifold.tab", "view_kind": "gecko"}]}"#);
    bad(r#"{"nodes": [{"id": "a", "type": "application/vnd.manifold.label", "color": "red"}]}"#);
    bad(
        r#"{"nodes": [{"id": "a", "type": "application/vnd.manifold.tab"}, {"id": "a", "type": "application/vnd.manifold.tab"}]}"#,
    );
}

#[test]
fn export_writes_type_as_a_field_and_import_reads_it_back() {
    let doc = doc_from(HAND_WRITTEN, 1);
    let (group, _) = doc.live().find(|(_, n)| n.kind() == Kind::Group).unwrap();
    let export = subtree_to_json(&doc, group);
    assert_eq!(export.nodes[0].kind(), Some(Kind::Group));
    assert_eq!(export.nodes[1].kind(), Some(Kind::Tab));

    let text = serde_json::to_value(&export).unwrap();
    for node in text["nodes"].as_array().unwrap() {
        assert!(node["type"].is_string(), "{node}");
        assert!(node.get("kind").is_none(), "{node}");
    }
    assert_eq!(text["nodes"][0]["type"], "application/vnd.manifold.group");

    let imported = import(&export, ids(2), Trust::Own).unwrap();
    assert_eq!(imported.iter().map(Imported::kind).collect::<Vec<_>>(), [Kind::Group, Kind::Tab, Kind::Tab]);
    assert!(imported.iter().all(|n| !n.fields.contains_key(&field::TYPE)), "the type is carried separately");
    let mut again = Doc::new(DocId([0; 16]));
    let mut op_ids = ids(3);
    for (i, body) in creates(&imported).into_iter().enumerate() {
        again.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
    }
    let kinds: Vec<Kind> = imported.iter().map(|n| again.nodes[&n.id].kind()).collect();
    assert_eq!(kinds, [Kind::Group, Kind::Tab, Kind::Tab], "creates() writes the type back");
    assert_eq!(again.nodes[&imported[0].id].get(field::TYPE), FieldValue::Str(TYPE_GROUP.into()));
}

#[test]
fn import_rejects_a_missing_or_empty_type() {
    let bad = |json: &str| import(&serde_json::from_str(json).unwrap(), ids(1), Trust::Own).unwrap_err();
    bad(r#"{"nodes": [{"id": "a"}]}"#);
    bad(r#"{"nodes": [{"id": "a", "type": 7}]}"#);
    bad(r#"{"nodes": [{"id": "a", "type": ""}]}"#);
}

/// A type this build doesn't know is kept with its string and the generic fields only: a newer build's field,
/// an `Id` among them, can't be remapped, so it is never copied (spec decision 4). A known kind keeps only its own
/// fields too.
#[test]
fn import_keeps_an_unknown_type_with_the_generic_fields_only() {
    let json = doc(serde_json::json!([
        {"id": "u", "type": "application/x-from-the-future", "name": "Old", "pos": [1, 2], "color": "#ff0000",
         "url": "https://x/", "jar": "g", "text": "dropped", "future_field": 7, "group": "g"},
        {"id": "g", "type": "application/vnd.manifold.group", "name": "G", "parent": "u"},
    ]));
    let imported = import(&json, ids(1), Trust::Own).unwrap();
    assert_eq!(imported[0].ty, "application/x-from-the-future");
    assert_eq!(imported[0].kind(), Kind::Unknown);
    let kept: Vec<u32> = imported[0].fields.keys().copied().collect();
    assert_eq!(kept, [field::POS, field::GROUP, field::NAME]);
    assert_eq!(imported[0].fields[&field::GROUP], FieldValue::Id(Some(imported[1].id)));
    assert!(!imported[1].fields.contains_key(&field::PARENT), "a group has no `parent`");
    let made = creates(&imported);
    let mut d = Doc::new(DocId([0; 16]));
    let mut op_ids = ids(3);
    for (i, body) in made.into_iter().enumerate() {
        d.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
    }
    assert_eq!(d.nodes[&imported[0].id].ty(), "application/x-from-the-future", "creates() writes the string back");
    let uid = imported[0].id;
    let out = node_to_json(uid, &d.nodes[&uid], None);
    assert_eq!(out.fields["type"], "application/x-from-the-future", "export writes the real string");
}

#[test]
fn node_json_matches_a_document_nodes_json() {
    let mut fields = BTreeMap::new();
    fields.insert(field::URL, FieldValue::Str("https://a.example/".into()));
    fields.insert(field::NAME, FieldValue::Str("Mine".into()));
    fields.insert(field::COLOR, FieldValue::Color(None));
    let json = node_json("n1", crate::store::model::TYPE_TAB, &fields, &Trees::new(), None);
    assert_eq!(json.kind(), Some(Kind::Tab));
    assert_eq!(json.fields.get("url"), Some(&Value::String("https://a.example/".into())));
    assert_eq!(json.fields.get("name"), Some(&Value::String("Mine".into())));
    assert!(!json.fields.contains_key("color"), "defaults are left out");
    assert!(!json.fields.contains_key("group"));
}

#[test]
fn an_anchor_is_neither_exported_nor_imported() {
    use crate::store::model::Anchor;
    let mut fields = BTreeMap::new();
    fields.insert(field::URL, FieldValue::Str("https://a.example/".into()));
    fields.insert(field::ANCHOR, FieldValue::Anchor(Some(Anchor { pos: [1.0, 2.0], group: None, explicit: true })));
    let json = node_json("n1", crate::store::model::TYPE_TAB, &fields, &Trees::new(), None);
    assert!(!json.fields.contains_key("anchor"), "the hold is not exported");
    let doc = doc_from(
        r##"{"nodes": [{"id": "t", "type": "application/vnd.manifold.tab", "url": "https://a.example/", "pos": [1, 2], "anchor": {"pos": [1, 2]}}]}"##,
        1,
    );
    let (_, tab) = doc.live().next().unwrap();
    assert_eq!(tab.get(field::POS), FieldValue::Pos(Some((1.0, 2.0))), "the position is kept");
    assert_eq!(tab.get(field::ANCHOR), FieldValue::Anchor(None), "an anchor key is skipped, not an error");
}

#[test]
fn a_body_exports_as_plain_json_and_imports_with_fresh_element_ids() {
    let json = r##"{"nodes": [{"id": "a", "type": "application/json",
            "body": {"k": [1, {"x": true}, []], "e": {}, "$ref": "plain"}}]}"##;
    let doc: JsonDoc = serde_json::from_str(json).unwrap();
    let imported = import(&doc, ids(7), Trust::Own).unwrap();
    let tree = &imported[0].trees[&field::BODY];
    // The ids are the importer's (`ids(7)` mints under actor 7), not the file's.
    assert!(tree.keys().flat_map(|p| p.segments()).any(|s| matches!(s, Seg::Id(id) if id.actor.0 == 7)));
    let back = node_json("a", "application/json", &Default::default(), &imported[0].trees, None);
    assert_eq!(back.fields["body"], doc.nodes[0].fields["body"], "export is the document, arrays and all");
}

#[test]
fn an_inline_card_exports_without_a_path_and_round_trips_foreign() {
    let json = r##"{"nodes": [{"id": "a", "type": "application/json", "body": {"k": [1, 2]}}]}"##;
    let doc: JsonDoc = serde_json::from_str(json).unwrap();
    let imported = import(&doc, ids(7), Trust::Foreign).unwrap();
    let out = node_json("a", "application/json", &imported[0].fields, &imported[0].trees, None);
    assert_eq!(out.fields["body"], serde_json::json!({"k": [1, 2]}));
    assert!(!out.fields.contains_key("path"), "no path field for an inline card");
}

#[test]
fn import_keeps_file_order() {
    let json: JsonDoc =
        serde_json::from_str(r#"{"nodes":[{"id":"b","type":"text/markdown"},{"id":"a","type":"text/markdown"}]}"#)
            .unwrap();
    let mut next = ids(7);
    let first = next();
    let imported = import(&json, ids(7), Trust::Own).unwrap();
    assert_eq!(imported[0].id, first, "the first node in the file gets the first fresh id");
}

fn doc(nodes: Value) -> JsonDoc {
    serde_json::from_value(serde_json::json!({ "nodes": nodes })).unwrap()
}

#[test]
fn a_foreign_import_drops_download_paths_and_outside_jars() {
    let outside = NodeId { actor: ActorId(9), counter: 9 }.to_string();
    // `JsonNode` flattens its fields into the node object.
    let json = doc(serde_json::json!([
        { "id": "j", "type": "application/vnd.manifold.jar" },
        { "id": "d", "type": "application/octet-stream", "path": "C:\\Windows\\x.exe" },
        { "id": "t", "type": "application/vnd.manifold.tab", "url": "https://a", "jar": "j" },
        { "id": "u", "type": "application/vnd.manifold.tab", "url": "https://b", "jar": outside },
        { "id": "g", "type": "application/vnd.manifold.group", "jar": outside },
    ]));
    let foreign = import(&json, ids(1), Trust::Foreign).unwrap();
    assert!(!foreign[1].fields.contains_key(&field::PATH));
    assert_eq!(foreign[2].fields.get(&field::JAR), Some(&FieldValue::Id(Some(foreign[0].id))), "in-file jar kept");
    assert!(!foreign[3].fields.contains_key(&field::JAR), "a jar outside the file is dropped");
    assert!(!foreign[4].fields.contains_key(&field::JAR), "so is a group's outside jar");
    let own = import(&json, ids(2), Trust::Own).unwrap();
    assert!(own[1].fields.contains_key(&field::PATH));
    assert!(own[3].fields.contains_key(&field::JAR));
    assert!(own[4].fields.contains_key(&field::JAR));
}

#[test]
fn a_foreign_import_drops_every_kinds_machine_path() {
    // A UNC path would make restore open an SMB connection to a host the file's author chose.
    let unc = r"\\evil\share\x";
    let json = doc(serde_json::json!([
        { "id": "i", "type": "image/png", "path": unc },
        { "id": "p", "type": "application/pdf", "path": unc },
        { "id": "j", "type": "application/json", "path": unc },
    ]));
    for node in import(&json, ids(1), Trust::Foreign).unwrap() {
        assert!(!node.fields.contains_key(&field::PATH));
    }
    for node in import(&json, ids(2), Trust::Own).unwrap() {
        assert!(node.fields.contains_key(&field::PATH), "a session's own file card keeps its path");
    }
}

#[test]
fn export_writes_shown_nodes_only_and_import_never_carries_closed() {
    use crate::store::model::{encode_field, Reg, Stamp};
    let id = |c: u64| NodeId { actor: ActorId(1), counter: c };
    let mut doc = Doc::new(DocId([0; 16]));
    let mut group = Node::typed(Kind::Group);
    let stamp = Stamp { hlc: Hlc { ms: 1, logical: 0 }, actor: ActorId(1) };
    group.fields.insert(
        crate::store::model::RegKey::of(field::CLOSED),
        Reg { stamp, enc: encode_field(field::CLOSED, &FieldValue::Replica(7)) },
    );
    doc.nodes.insert(id(1), group);
    let mut inside = Node::typed(Kind::Note);
    inside.group = Some(id(1));
    doc.nodes.insert(id(2), inside);
    doc.nodes.insert(id(3), Node::typed(Kind::Note));
    let ids = |j: &JsonDoc| j.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&doc_to_json(&doc)), [id(3).to_string()], "the closed group and its member stay out");
    assert_eq!(ids(&doc_to_json_all(&doc)).len(), 3, "the forensic dump keeps them");
    assert!(subtree_to_json(&doc, id(1)).nodes.is_empty(), "a closed group exports nothing");
    // Re-importing the dump brings the closed group back shown: `closed` is never imported.
    let mut n = 100;
    let imported = import(
        &doc_to_json_all(&doc),
        || {
            n += 1;
            id(n)
        },
        Trust::Own,
    )
    .unwrap();
    assert!(imported.iter().all(|i| !i.fields.contains_key(&field::CLOSED)));
}

const WITH_PROPS: &str = r##"{"nodes": [
        {"id": "a", "type": "text/markdown", "text": "a", "props": {"label": "x", "n": {"deep": 2}, "link": {"$ref": "b"}}},
        {"id": "b", "type": "text/markdown", "text": "b"}
    ]}"##;

#[test]
fn a_document_with_props_exports_imports_and_creates_them() {
    let first = doc_from(WITH_PROPS, 1);
    let (a, b) = {
        let mut notes: Vec<_> = first.live().map(|(id, n)| (id, n.get(field::TEXT))).collect();
        notes.sort_by_key(|(_, text)| format!("{text:?}"));
        (notes[0].0, notes[1].0)
    };
    assert_eq!(first.nodes[&a].trees()[&field::PROPS].len(), 3);
    let exported = doc_to_json(&first);
    let json_a = exported.nodes.iter().find(|n| n.id == a.to_string()).unwrap();
    assert_eq!(json_a.fields["props"]["n"]["deep"], 2);
    assert_eq!(json_a.fields["props"]["link"]["$ref"], b.to_string());
    // Re-imported under fresh ids, the ref follows its target.
    let imported = import(&exported, ids(2), Trust::Own).unwrap();
    let mut second = Doc::new(DocId([0; 16]));
    let mut op_ids = ids(200);
    for (i, body) in creates(&imported).into_iter().enumerate() {
        second.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
    }
    let id_of = |text: &str| {
        imported.iter().find(|n| n.fields.get(&field::TEXT) == Some(&FieldValue::Str(text.into()))).unwrap().id
    };
    let (new_a, new_b) = (id_of("a"), id_of("b"));
    let tree = &second.nodes[&new_a].trees()[&field::PROPS];
    assert_eq!(tree.len(), 3);
    assert!(tree.values().any(|leaf| *leaf == Leaf::Ref(new_b)), "the ref names the copy of b");
}

#[test]
fn a_labelled_note_and_its_label_node_round_trip_with_the_membership_following_the_new_id() {
    let json = r##"{"nodes": [
            {"id": "a", "type": "text/markdown", "text": "a", "props": {"labels": {"@l": 5}}},
            {"id": "l", "type": "application/vnd.manifold.label", "name": "urgent"}
        ]}"##;
    let first = doc_from(json, 1);
    let exported = doc_to_json(&first);
    let imported = import(&exported, ids(2), Trust::Own).unwrap();
    let mut second = Doc::new(DocId([0; 16]));
    let mut op_ids = ids(200);
    for (i, body) in creates(&imported).into_iter().enumerate() {
        second.apply(&Op { id: op_ids(), hlc: Hlc { ms: 1, logical: i as u32 }, body });
    }
    let note = imported.iter().find(|n| n.fields.get(&field::TEXT) == Some(&FieldValue::Str("a".into()))).unwrap().id;
    let label =
        imported.iter().find(|n| n.fields.get(&field::NAME) == Some(&FieldValue::Str("urgent".into()))).unwrap().id;
    let tree = &second.nodes[&note].trees()[&field::PROPS];
    assert_eq!(tree.len(), 1);
    let (path, leaf) = tree.iter().next().unwrap();
    assert_eq!((leaf, path.segments().last()), (&Leaf::Int(5), Some(&Seg::Id(label))), "keyed by the new label id");
}

#[test]
fn a_group_export_carries_the_labels_its_cards_use_and_no_others() {
    let json = r##"{"nodes": [
            {"id": "g", "type": "application/vnd.manifold.group", "name": "G"},
            {"id": "a", "type": "text/markdown", "text": "a", "group": "g", "props": {"labels": {"@used": 5}}},
            {"id": "used", "type": "application/vnd.manifold.label", "name": "urgent"},
            {"id": "other", "type": "application/vnd.manifold.label", "name": "unused"}
        ]}"##;
    let doc = doc_from(json, 1);
    let g = doc.live().find(|(_, n)| n.kind() == Kind::Group).map(|(id, _)| id).unwrap();
    let export = subtree_to_json(&doc, g);
    let names: Vec<_> = export
        .nodes
        .iter()
        .filter(|n| n.kind() == Some(Kind::Label))
        .map(|n| n.fields["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["urgent"], "only the label a card in the subtree carries");
    assert_eq!(export.nodes.last().unwrap().kind(), Some(Kind::Label), "labels come after the subtree");
    let back = import(&export, ids(2), Trust::Foreign).unwrap();
    let note = back.iter().find(|n| n.fields.get(&field::TEXT) == Some(&FieldValue::Str("a".into()))).unwrap();
    assert_eq!(note.trees[&field::PROPS].len(), 1, "the membership survives a foreign import");
}

#[test]
fn a_deleted_label_is_not_exported() {
    let json = r##"{"nodes": [
            {"id": "g", "type": "application/vnd.manifold.group", "name": "G"},
            {"id": "a", "type": "text/markdown", "text": "a", "group": "g", "props": {"labels": {"@l": 5}}},
            {"id": "l", "type": "application/vnd.manifold.label", "name": "gone"}
        ]}"##;
    let mut doc = doc_from(json, 1);
    let g = doc.live().find(|(_, n)| n.kind() == Kind::Group).map(|(id, _)| id).unwrap();
    let l = doc.live().find(|(_, n)| n.kind() == Kind::Label).map(|(id, _)| id).unwrap();
    doc.nodes.get_mut(&l).unwrap().deleted = true;
    let export = subtree_to_json(&doc, g);
    assert!(export.nodes.iter().all(|n| n.kind() != Some(Kind::Label)));
}

#[test]
fn a_reused_node_takes_the_existing_id_and_keeps_its_slot() {
    let json = r##"{"nodes": [
            {"id": "a", "type": "text/markdown", "text": "a", "props": {"labels": {"@l": 5}}},
            {"id": "l", "type": "application/vnd.manifold.label", "name": "urgent"}
        ]}"##;
    let parsed: JsonDoc = serde_json::from_str(json).unwrap();
    let existing = NodeId { actor: ActorId(9), counter: 9 };
    let imported = import_with(&parsed, ids(1), Trust::Foreign, |n| (n.id == "l").then_some(existing)).unwrap();
    assert_eq!(imported.len(), parsed.nodes.len(), "one slot per file node");
    assert!(!imported[0].reused && imported[1].reused);
    assert_eq!(imported[1].id, existing);
    let (path, _) = imported[0].trees[&field::PROPS].iter().next().unwrap();
    assert_eq!(path.segments().last(), Some(&Seg::Id(existing)), "the membership names the reused id");
    assert_eq!(creates(&imported).len(), 2, "a reused slot creates nothing: the note and its one membership");
}

#[test]
fn a_foreign_import_drops_refs_outside_the_file() {
    let json = r##"{"nodes": [{"id": "a", "type": "text/markdown", "text": "a",
            "props": {"keep": 1, "gone": {"$ref": "424242"}, "@424242": {"x": 1}}}]}"##;
    let parsed: JsonDoc = serde_json::from_str(json).unwrap();
    let imported = import(&parsed, ids(1), Trust::Foreign).unwrap();
    assert_eq!(imported[0].trees[&field::PROPS].len(), 1);
}

#[test]
fn props_on_an_unknown_type_are_not_copied() {
    let json = r##"{"nodes": [{"id": "a", "type": "application/x-from-the-future", "props": {"k": 1}}]}"##;
    let parsed: JsonDoc = serde_json::from_str(json).unwrap();
    assert!(import(&parsed, ids(1), Trust::Own).unwrap()[0].trees.is_empty());
}

#[test]
fn a_bad_props_value_is_an_import_error() {
    let json = r##"{"nodes": [{"id": "a", "type": "text/markdown", "props": {"k": [1]}}]}"##;
    let parsed: JsonDoc = serde_json::from_str(json).unwrap();
    assert!(import(&parsed, ids(1), Trust::Own).is_err());
}
