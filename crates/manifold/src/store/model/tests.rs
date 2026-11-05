use super::*;

#[test]
fn the_clock_saturates_instead_of_wrapping() {
    let mut clock = Clock::default();
    let top = Hlc { ms: u64::MAX, logical: u32::MAX };
    clock.observe(top);
    assert!(clock.tick(0) >= top, "never behind a time seen");
}

#[test]
fn node_ids_print_and_parse() {
    let id = NodeId { actor: ActorId(0x1f3a_9c00_0000_00ab), counter: 17 };
    assert_eq!(id.to_string(), "1f3a9c00000000ab-17");
    assert_eq!(id.to_string().parse(), Ok(id));
    for bad in ["", "tab-3", "1f3a9c00000000ab-", "1f3a9c00000000ab-+1", "+f3a9c00000000ab-1"] {
        assert!(bad.parse::<NodeId>().is_err(), "{bad}");
    }
}

#[test]
fn the_clock_never_goes_back() {
    let mut clock = Clock::default();
    assert_eq!(clock.tick(10), Hlc { ms: 10, logical: 0 });
    assert_eq!(clock.tick(10), Hlc { ms: 10, logical: 1 });
    assert_eq!(clock.tick(5), Hlc { ms: 10, logical: 2 });
    clock.observe(Hlc { ms: 20, logical: 4 });
    assert_eq!(clock.tick(15), Hlc { ms: 20, logical: 5 });
    clock.observe(Hlc { ms: 30, logical: u32::MAX });
    assert_eq!(clock.tick(0), Hlc { ms: 31, logical: 0 });
}

#[test]
fn field_numbers_are_unique_and_never_reserved() {
    let mut seen = std::collections::BTreeSet::new();
    for def in &FIELDS {
        assert!(seen.insert(def.no), "field {} twice", def.no);
        assert!(!RESERVED.contains(&def.no), "field {} is reserved", def.no);
    }
    assert!(field_def(7).is_none() && field_by_name("pinned").is_none(), "pinned is retired");
    assert_eq!(field::VIEW, 14);
    let view = field_by_name("view").unwrap();
    assert_eq!(view.ty, FieldType::Str);
    for (no, name, ty) in [
        (field::JAR, "jar", FieldType::Id),
        (field::EPHEMERAL, "ephemeral", FieldType::Bool),
        (field::IS_DEFAULT, "is_default", FieldType::Bool),
    ] {
        let def = field_by_name(name).unwrap();
        assert_eq!((def.no, def.ty), (no, ty), "{name}");
    }
    assert_eq!((field::JAR, field::EPHEMERAL), (15, 18));
    let ty = field_by_name("type").unwrap();
    assert_eq!((ty.no, ty.ty), (19, FieldType::Str));
}

#[test]
fn values_round_trip_and_defaults_encode_to_nothing() {
    let id = NodeId { actor: ActorId(9), counter: 4 };
    let cases = [
        (field::POS, FieldValue::Pos(Some((1.5, -2.0)))),
        (field::POS, FieldValue::Pos(Some((0.0, 3.0)))),
        (field::POS, FieldValue::Pos(Some((0.0, 0.0)))),
        (field::COLOR, FieldValue::Color(Some(0))),
        (field::COLOR, FieldValue::Color(Some(0x12_34_56))),
        (field::PARENT, FieldValue::Id(Some(id))),
        (field::URL, FieldValue::Str("https://example.com/".into())),
        (field::ANCHOR, FieldValue::Anchor(Some(Anchor { pos: [1.0, 2.0], group: Some(id), explicit: true }))),
        (field::VIEW_KIND, FieldValue::ViewCode(4)),
        (field::JAR, FieldValue::Id(Some(id))),
        (field::EPHEMERAL, FieldValue::Bool(true)),
        (field::IS_DEFAULT, FieldValue::Bool(true)),
    ];
    for (no, value) in cases {
        let enc = encode_field(no, &value);
        assert!(!enc.is_empty(), "{value:?}");
        assert_eq!(decode_field(field_def(no).unwrap(), &enc), Ok(value));
    }
    for def in &FIELDS {
        assert!(encode_field(def.no, &FieldValue::absent(def.ty)).is_empty(), "{}", def.name);
        assert_eq!(decode_field(def, &[]), Ok(FieldValue::absent(def.ty)));
    }
}

fn reg(counter: u64, ty: &str) -> Reg {
    let stamp = Stamp { hlc: Hlc { ms: counter, logical: 0 }, actor: ActorId(1) };
    Reg { stamp, enc: encode_field(field::TYPE, &FieldValue::Str(ty.into())) }
}

#[test]
fn only_stored_only_kinds_are_not_cards() {
    for kind in [
        Kind::Tab,
        Kind::Group,
        Kind::Note,
        Kind::Image,
        Kind::Pdf,
        Kind::Json,
        Kind::Download,
        Kind::Jar,
        Kind::Unknown,
    ] {
        assert!(kind.card(), "{kind:?}");
    }
    assert!(!Kind::Link.card() && !Kind::Label.card());
}

#[test]
fn a_label_is_a_stored_node_with_a_name_and_a_colour() {
    assert_eq!(Kind::Label.type_str(), Some(TYPE_LABEL));
    assert_eq!(Kind::from_type(TYPE_LABEL), Kind::Label);
    assert_eq!(Kind::Label.fields(), &[field::NAME, field::COLOR, field::CLOSED]);
}

#[test]
fn every_known_kind_has_a_type_string_that_maps_back() {
    for kind in [
        Kind::Tab,
        Kind::Group,
        Kind::Note,
        Kind::Image,
        Kind::Pdf,
        Kind::Json,
        Kind::Download,
        Kind::Jar,
        Kind::Link,
        Kind::Label,
    ] {
        let ty = kind.type_str().expect("a known kind has a type");
        assert_eq!(Kind::from_type(ty), kind, "{ty}");
    }
    assert_eq!(Kind::Unknown.type_str(), None);
}

#[test]
fn from_type_is_case_insensitive_takes_any_image_and_rejects_the_rest() {
    assert_eq!(Kind::from_type("Application/VND.manifold.TAB"), Kind::Tab);
    assert_eq!(Kind::from_type("image/jpeg"), Kind::Image);
    assert_eq!(Kind::from_type("IMAGE/webp"), Kind::Image);
    assert_eq!(Kind::from_type("video/mp4"), Kind::Unknown);
    // The retired JSON leaf's type is one this build doesn't know (an unknown-type card).
    assert_eq!(Kind::from_type("application/vnd.manifold.json-leaf"), Kind::Unknown);
    assert_eq!(Kind::from_type(""), Kind::Unknown);
}

#[test]
fn a_node_reads_its_kind_from_the_last_type_write() {
    let mut node = Node::placeholder();
    assert_eq!(node.kind(), Kind::Unknown, "no type yet");
    assert!(!node.has_type() && !node.is_live());
    node.fields.insert(RegKey::of(field::TYPE), reg(1, "application/vnd.manifold.jar"));
    assert_eq!((node.kind(), node.has_type(), node.is_live()), (Kind::Jar, true, true));
    node.fields.insert(RegKey::of(field::TYPE), reg(2, "application/x-from-the-future"));
    assert_eq!((node.kind(), node.has_type(), node.is_live()), (Kind::Unknown, true, true), "a card of its own");
    node.fields.insert(RegKey::of(field::TYPE), reg(3, ""));
    assert!(node.has_type() && !node.is_live(), "an empty type is no type");
}

/// Any card can be pinned or dropped, so every card kind stores the anchor (a kind that didn't lost it on restart
/// and in sync).
#[test]
fn every_card_kind_carries_the_anchor() {
    let kinds = TYPES.iter().map(|t| t.0).chain([Kind::Unknown]);
    for kind in kinds.filter(|k| k.card()) {
        assert!(kind.fields().contains(&field::ANCHOR), "{kind:?}");
    }
}

/// A type this build doesn't know carries the generic fields (spec decision 3): what the app reads, mirrors and
/// copies of it. Its type string is kept as written.
#[test]
fn an_unknown_kind_carries_the_generic_fields() {
    use field::*;
    assert_eq!(Kind::Unknown.fields(), &[POS, ANCHOR, PARENT, NAME, CLOSED]);
    assert_eq!(Node::with_type("application/x-from-the-future").ty(), "application/x-from-the-future");
    assert_eq!(Node::with_type("Text/Markdown").ty(), "Text/Markdown", "kept as written, case and all");
    assert_eq!(Node::placeholder().ty(), "");
}

#[test]
fn a_non_finite_position_decodes_as_absent() {
    let def = field_def(field::POS).unwrap();
    for bad in [f32::NAN, f32::INFINITY] {
        let mut pos = Writer::new();
        pos.fixed32(1, bad.to_bits());
        let mut enc = Writer::new();
        enc.message(field::POS, &pos);
        assert_eq!(decode_field(def, &enc.into_bytes()).unwrap(), FieldValue::Pos(None));
    }
}

#[test]
fn nodes_show_at_their_nearest_live_group_never_inside_another_card() {
    let id = |counter| NodeId { actor: ActorId(1), counter };
    let at = |mut node: Node, group: Option<u64>, deleted| {
        node.group = group.map(id);
        node.deleted = deleted;
        node
    };
    let node = |kind, group, deleted| at(Node::typed(kind), group, deleted);
    let mut doc = Doc::new(DocId([0; 16]));
    doc.nodes.insert(id(1), node(Kind::Group, None, false));
    doc.nodes.insert(id(2), node(Kind::Group, Some(1), true)); // deleted, in 1
    doc.nodes.insert(id(3), node(Kind::Tab, Some(2), false)); // shown in 1
    doc.nodes.insert(id(4), node(Kind::Tab, Some(99), false)); // group missing
    doc.nodes.insert(id(5), at(Node::with_type("application/x-future"), Some(1), false)); // a newer build's container: live, not a group
    doc.nodes.insert(id(6), node(Kind::Tab, Some(5), false)); // shown in 1
    doc.nodes.insert(id(7), node(Kind::Group, Some(8), true)); // 7 and 8: a damaged cycle
    doc.nodes.insert(id(8), node(Kind::Group, Some(7), true));
    doc.nodes.insert(id(9), node(Kind::Tab, Some(7), false));
    doc.nodes.insert(id(10), node(Kind::Note, Some(3), false)); // in a tab: shown in 1
    assert_eq!(doc.effective_group(id(3)), Some(id(1)));
    assert_eq!(doc.effective_group(id(4)), None);
    assert_eq!(doc.effective_group(id(6)), Some(id(1)));
    assert_eq!(doc.effective_group(id(9)), None);
    assert_eq!(doc.effective_group(id(10)), Some(id(1)), "a tab is no group");
    assert_eq!(doc.with_descendants(BTreeSet::from([id(2)])), BTreeSet::from([id(2), id(3), id(10)]));
    assert_eq!(doc.with_descendants(BTreeSet::from([id(1)])), BTreeSet::from([1, 2, 3, 5, 6, 10].map(id)));
}

/// A doc of `(id, kind, group, closed)` rows, all live; ids are `NodeId { actor: ActorId(1), counter }`.
fn doc_of(rows: &[(u64, Kind, Option<u64>, bool)]) -> (Doc, impl Fn(u64) -> NodeId) {
    let id = |c: u64| NodeId { actor: ActorId(1), counter: c };
    let mut doc = Doc::new(DocId([0; 16]));
    for &(c, kind, group, closed) in rows {
        let mut node = Node::typed(kind);
        node.group = group.map(id);
        if closed {
            let stamp = Stamp { hlc: Hlc { ms: c, logical: 0 }, actor: ActorId(1) };
            let enc = encode_field(field::CLOSED, &FieldValue::Replica(7));
            node.fields.insert(RegKey::of(field::CLOSED), Reg { stamp, enc });
        }
        doc.nodes.insert(id(c), node);
    }
    (doc, id)
}

#[test]
fn a_closed_group_hides_everything_under_it_and_a_deleted_one_passes_members_up() {
    let (mut doc, id) = doc_of(&[
        (1, Kind::Group, None, false),
        (2, Kind::Group, Some(1), true),
        (3, Kind::Tab, Some(2), false),
        (4, Kind::Group, Some(3), false), // under a tab: still hidden by 2
        (5, Kind::Note, Some(1), false),
    ]);
    assert!(doc.nodes[&id(2)].closed());
    assert_eq!(doc.placement(id(2)), Placement::Shown(Some(id(1))), "a closed node's own place is its group's");
    assert!(!doc.is_shown(id(2)), "but it is not shown: it is closed");
    assert_eq!(doc.placement(id(3)), Placement::Hidden);
    assert_eq!(doc.placement(id(4)), Placement::Hidden);
    assert_eq!(doc.placement(id(5)), Placement::Shown(Some(id(1))));
    let shown: Vec<NodeId> = doc.shown().map(|(n, _)| n).collect();
    assert_eq!(shown, [id(1), id(5)]);
    // A deleted closed group hides nothing: its members pass up to the nearest shown group.
    doc.nodes.get_mut(&id(2)).unwrap().deleted = true;
    assert_eq!(doc.placement(id(3)), Placement::Shown(Some(id(1))));
    assert!(doc.is_shown(id(3)));
}

#[test]
fn shown_level_is_the_nearest_shown_node_up_the_raw_chain() {
    let (doc, id) =
        doc_of(&[(1, Kind::Group, None, false), (2, Kind::Group, Some(1), true), (3, Kind::Group, Some(2), false)]);
    assert_eq!(doc.shown_level(id(1)), Some(id(1)));
    assert_eq!(doc.shown_level(id(3)), Some(id(1)), "hidden by 2, closed itself: both skipped");
    assert_eq!(doc.shown_level(id(9)), None, "unknown: the top level");
}

#[test]
fn held_round_trips_and_empty_is_absent() {
    let def = field_def(field::HELD).unwrap();
    let id = |counter| NodeId { actor: ActorId(7), counter };
    for held in [
        FieldValue::Held { dest: Some(id(1)), members: vec![id(2), id(3)] },
        FieldValue::Held { dest: None, members: vec![id(2)] },
        FieldValue::Held { dest: Some(id(1)), members: vec![] },
    ] {
        assert_eq!(decode_field(def, &encode_field(field::HELD, &held)).unwrap(), held);
    }
    let none = FieldValue::absent(FieldType::Held);
    assert!(encode_field(field::HELD, &none).is_empty(), "the top level with no members writes nothing");
    assert_eq!(decode_field(def, &[]).unwrap(), none);
}

#[test]
fn anchor_round_trips_and_a_non_finite_point_is_absent() {
    let def = field_def(field::ANCHOR).unwrap();
    assert_eq!((def.name, def.ty), ("anchor", FieldType::Anchor));
    let id = NodeId { actor: ActorId(7), counter: 3 };
    for anchor in [
        Anchor { pos: [1.5, -2.0], group: Some(id), explicit: true },
        Anchor { pos: [0.0, 4.0], group: None, explicit: false },
        // The top-level origin with no bit is still a set anchor: never the empty encoding.
        Anchor { pos: [0.0, 0.0], group: None, explicit: false },
    ] {
        let enc = encode_field(field::ANCHOR, &FieldValue::Anchor(Some(anchor)));
        assert!(!enc.is_empty(), "{anchor:?}");
        assert_eq!(decode_field(def, &enc), Ok(FieldValue::Anchor(Some(anchor))));
        assert_eq!(FieldValue::Anchor(Some(anchor)).as_anchor(), Some(anchor));
    }
    assert!(encode_field(field::ANCHOR, &FieldValue::Anchor(None)).is_empty(), "a clear writes nothing");
    assert_eq!(FieldValue::absent(FieldType::Anchor), FieldValue::Anchor(None));
    for bad in [f32::NAN, f32::INFINITY] {
        // Hand-built: `Writer::f32` refuses NaN, and a file from elsewhere may hold one.
        let mut m = Writer::new();
        m.fixed32(1, bad.to_bits());
        m.f32(2, 1.0);
        m.varint(4, 1);
        let mut enc = Writer::new();
        enc.message(field::ANCHOR, &m);
        assert_eq!(decode_field(def, &enc.into_bytes()), Ok(FieldValue::Anchor(None)), "{bad} is no point");
    }
}

#[test]
fn a_live_link_is_shown_whatever_its_group() {
    let (mut doc, id) = doc_of(&[(1, Kind::Group, None, true), (2, Kind::Link, Some(1), false)]);
    assert!(doc.is_shown(id(2)), "the mirror keeps every live link (D2)");
    doc.nodes.get_mut(&id(2)).unwrap().deleted = true;
    assert!(!doc.is_shown(id(2)));
}

#[test]
fn closed_roots_are_newest_first_and_skip_nested_and_deleted_ones() {
    let (mut doc, id) = doc_of(&[
        (1, Kind::Note, None, true),    // stamp 1
        (2, Kind::Group, None, true),   // stamp 2
        (3, Kind::Note, Some(2), true), // stamp 3, nested: part of 2's entry
        (4, Kind::Tab, None, true),     // stamp 4
        (5, Kind::Tab, None, false),
    ]);
    let ids = |doc: &Doc| doc.closed_roots().into_iter().map(|(n, _)| n).collect::<Vec<_>>();
    assert_eq!(ids(&doc), [id(4), id(2), id(1)]);
    doc.nodes.get_mut(&id(4)).unwrap().deleted = true;
    let reg = doc.nodes.get_mut(&id(2)).unwrap().fields.get_mut(&RegKey::of(field::CLOSED)).unwrap();
    reg.enc = encode_field(field::CLOSED, &FieldValue::Replica(0));
    assert_eq!(ids(&doc), [id(3), id(1)], "clearing the outer one makes the nested one an entry");
}

#[test]
fn closed_is_one_more_generic_field() {
    assert!(GENERIC.contains(&field::CLOSED));
    assert!(TYPES.iter().all(|(_, _, fields)| fields.contains(&field::CLOSED)));
    assert_eq!(field_def(field::CLOSED).map(|d| (d.name, d.ty)), Some(("closed", FieldType::Replica)));
}
