use super::*;
use crate::card_id::testutil::node;
use crate::session_sync::color_value;

fn label(m: &mut Mirror, n: u64, name: &str) -> NodeId {
    let id = node(n);
    m.add_stored(id, Kind::Label);
    m.set(id, field::NAME, FieldValue::Str(name.to_owned()));
    id
}

#[test]
fn same_key_labels_group_under_the_lowest_id() {
    let mut m = Mirror::default();
    let (c, a, b) = (label(&mut m, 5, "Urgent"), label(&mut m, 2, "urgent"), label(&mut m, 9, " URGENT "));
    assert_eq!([m.canonical_label(a), m.canonical_label(b), m.canonical_label(c)], [a, a, a]);
    assert_eq!(m.label_members(a), vec![a, c, b]);
}

#[test]
fn an_empty_key_is_its_own_group() {
    let mut m = Mirror::default();
    let (a, b) = (label(&mut m, 1, "\u{200b}"), label(&mut m, 2, "\u{200b}"));
    assert_eq!([m.canonical_label(a), m.canonical_label(b)], [a, b]);
}

#[test]
fn renaming_a_member_splits_and_reforms_the_group() {
    let mut m = Mirror::default();
    let (a, b) = (label(&mut m, 1, "x"), label(&mut m, 2, "x"));
    m.set(a, field::NAME, FieldValue::Str("y".to_owned()));
    assert_eq!([m.canonical_label(a), m.canonical_label(b)], [a, b]);
    m.set(a, field::NAME, FieldValue::Str("x".to_owned()));
    assert_eq!(m.canonical_label(b), a);
    m.forget_stored(a);
    assert_eq!(m.canonical_label(b), b, "the survivor is canonical again");
}

#[test]
fn a_card_carrying_two_members_shows_one_chip_with_the_earlier_stamp() {
    let mut m = Mirror::default();
    let (a, b, other) = (label(&mut m, 1, "x"), label(&mut m, 2, "X"), label(&mut m, 3, "z"));
    let card = node(10);
    for (l, at) in [(a, 20), (b, 10), (other, 15)] {
        m.edit_tree(card, field::PROPS, &crate::labels::labels_path(l), Some(Leaf::Int(at)));
    }
    assert_eq!(m.card_labels(card), vec![a, other], "b folds into a at stamp 10, so a leads");
}

#[test]
fn the_default_index_follows_is_default_writes_and_retain() {
    let mut m = Mirror::default();
    let (a, b) = (node(1), node(2));
    m.set(b, field::IS_DEFAULT, FieldValue::Bool(true));
    m.set(a, field::IS_DEFAULT, FieldValue::Bool(true));
    assert_eq!(m.default_jar_ids().collect::<Vec<_>>(), [a, b], "lowest first");
    m.set(a, field::IS_DEFAULT, FieldValue::Bool(false));
    m.retain(|id| id != b);
    assert_eq!(m.default_jar_ids().count(), 0);
}

#[test]
fn seed_then_carried_round_trips_and_never_overwrites() {
    let mut mirror = Mirror::default();
    let jar = crate::store::model::NodeId { actor: crate::store::model::ActorId(1), counter: 2 };
    let first = Carried { parent: Some(node(3)), group: Some(node(4)), ..Carried::default() }
        .with(field::POS, pos_value(egui::vec2(3.0, -4.0)))
        .with(field::COLOR, color_value(Some(egui::Color32::RED)))
        .with(
            field::ANCHOR,
            FieldValue::Anchor(Some(crate::store::model::Anchor {
                pos: [3.0, -4.0],
                group: Some(node(4)),
                explicit: true,
            })),
        )
        .with_str(field::URL, "https://a/")
        .with_str(field::TEXT, "note")
        .with(field::VIEW_KIND, FieldValue::ViewCode(99))
        .with(field::JAR, FieldValue::Id(Some(jar)))
        .with_str(field::TYPE, "text/x")
        .with_str(field::PATH, "C:/a.json")
        .with_str(field::NAME, "Mine");
    mirror.seed(node(7), &first);
    assert_eq!(mirror.carried(node(7)), first);
    mirror.seed(node(7), &Carried::default().with_str(field::URL, "https://b/"));
    assert_eq!(mirror.carried(node(7)), first);
    mirror.seed(node(8), &Carried::default().with(field::POS, pos_value(egui::Vec2::ZERO)));
    assert_eq!(mirror.pos(node(8)), Some(egui::Vec2::ZERO), "the origin is a position like any other");
    mirror.seed(node(9), &Carried::default().with(field::POS, pos_value(egui::vec2(f32::NAN, 0.0))));
    assert_eq!(mirror.pos(node(9)), None, "a non-finite pos is none");
}

/// An entry that exists without a type (a remote op named the node before it was placed) takes the placed
/// node's type; one with a type keeps it.
#[test]
fn seed_types_an_entry_that_has_none() {
    let mut mirror = Mirror::default();
    mirror.set(node(7), field::URL, FieldValue::Str("https://a/".into()));
    mirror.seed(node(7), &Carried::default().with_str(field::TYPE, "text/x").with_str(field::URL, "https://b/"));
    assert_eq!((mirror.ty(node(7)), mirror.url(node(7))), ("text/x", "https://a/"), "the type lands, the rest is kept");
    mirror.seed(node(7), &Carried::default().with_str(field::TYPE, "text/y"));
    assert_eq!(mirror.ty(node(7)), "text/x");
    mirror.seed(node(8), &Carried::default());
    assert!(!mirror.nodes[&node(8)].contains_key(&field::TYPE), "no type seeds nothing");
}

#[test]
fn a_jar_edge_reads_back_and_a_node_without_one_inherits() {
    let mut mirror = Mirror::default();
    assert_eq!(mirror.jar(node(7)), None);
    mirror.seed(node(8), &Carried::default());
    assert!(!mirror.nodes[&node(8)].contains_key(&field::JAR), "no edge seeds nothing");
    assert!(mirror.has(node(8)) && !mirror.has(node(7)));
    let jar = NodeId { actor: crate::store::model::ActorId(1), counter: 2 };
    mirror.set(node(7), field::JAR, FieldValue::Id(Some(jar)));
    assert_eq!(mirror.jar(node(7)), Some(jar));
}

#[test]
fn a_view_reads_back_decoded_and_keeps_an_unknown_code() {
    let mut mirror = Mirror::default();
    assert_eq!(mirror.view_kind(node(7)), ViewKind::Servo, "absent is Servo");
    mirror.seed(node(8), &Carried::default());
    assert!(!mirror.nodes[&node(8)].contains_key(&field::VIEW_KIND), "code 0 seeds nothing");
    let cef = crate::session_sync::view_code(ViewKind::Cef);
    mirror.set(node(7), field::VIEW_KIND, FieldValue::ViewCode(cef));
    assert_eq!(mirror.view_kind(node(7)), ViewKind::Cef);
    mirror.set(node(7), field::VIEW_KIND, FieldValue::ViewCode(99));
    assert_eq!(mirror.view_kind(node(7)), ViewKind::Servo, "a newer build's code reads as Servo");
    let mut copy = Mirror::default();
    copy.seed(node(8), &mirror.carried(node(7)));
    assert_eq!(copy.view_code(node(8)), 99, "and is carried as is");
}

#[test]
fn a_url_reads_back_and_a_node_without_one_has_none() {
    let mut mirror = Mirror::default();
    assert_eq!(mirror.url(node(7)), "");
    mirror.seed(node(8), &Carried::default());
    assert!(!mirror.nodes[&node(8)].contains_key(&field::URL), "an empty url seeds nothing");
    mirror.set(node(7), field::URL, FieldValue::Str("https://a/".into()));
    assert_eq!(mirror.url(node(7)), "https://a/");
}

#[test]
fn a_node_without_group_or_file_fields_seeds_none_and_reads_them_as_absent() {
    let mut mirror = Mirror::default();
    mirror.seed(node(8), &Carried::default());
    for no in [field::TYPE, field::PATH, field::JAR] {
        assert!(!mirror.nodes[&node(8)].contains_key(&no), "field {no} seeds nothing");
    }
    assert_eq!((mirror.ty(node(8)), mirror.get(node(8), field::PATH)), ("", FieldValue::Str(String::new())));
}

#[test]
fn init_field_never_overwrites_a_value() {
    let mut mirror = Mirror::default();
    mirror.init_field(node(7), field::URL, FieldValue::Str("https://a/".into()));
    mirror.init_field(node(7), field::URL, FieldValue::Str("https://b/".into()));
    assert_eq!(mirror.url(node(7)), "https://a/");
    mirror.set(node(8), field::URL, FieldValue::Str(String::new()));
    mirror.init_field(node(8), field::URL, FieldValue::Str("https://b/".into()));
    assert_eq!(mirror.url(node(8)), "", "an absent value held is a value");
}

#[test]
fn a_text_reads_back_borrowed_and_a_node_without_one_has_none() {
    let mut mirror = Mirror::default();
    assert_eq!(mirror.text(node(7)), "");
    mirror.seed(node(8), &Carried::default());
    assert!(!mirror.nodes[&node(8)].contains_key(&field::TEXT), "an empty text seeds nothing");
    mirror.set(node(7), field::TEXT, FieldValue::Str("hello".into()));
    assert_eq!(mirror.text(node(7)), "hello");
    let FieldValue::Str(stored) = &mirror.nodes[&node(7)][&field::TEXT] else { panic!("a Str") };
    assert_eq!(mirror.text(node(7)).as_ptr(), stored.as_ptr(), "the stored string, not a copy");
}

#[test]
fn a_view_is_decoded_on_every_set_and_dropped_with_its_entry() {
    let mut mirror = Mirror::default();
    assert!(mirror.view(node(7)).is_none());
    mirror.set(node(7), field::VIEW, FieldValue::Str(r#"{"/a":{"collapsed":true}}"#.into()));
    assert_eq!(mirror.view(node(7)).and_then(|v| v.get("/a")).and_then(|o| o.collapsed), Some(true));
    mirror.set(node(7), field::VIEW, FieldValue::Str(r#"{"/b":{"collapsed":false}}"#.into()));
    let view = mirror.view(node(7)).expect("a view");
    assert!(view.get("/a").is_none() && view.get("/b").is_some(), "refreshed, not merged");
    mirror.set(node(7), field::VIEW, FieldValue::Str(String::new()));
    assert!(mirror.view(node(7)).is_none(), "an empty view is none");
    mirror.set(node(8), field::VIEW, FieldValue::Str(r#"{"/a":{"collapsed":true}}"#.into()));
    mirror.retain(|id| id != node(8));
    assert!(mirror.view(node(8)).is_none(), "the cache goes with the entry");
}

#[test]
fn a_name_reads_back_borrowed_and_a_node_without_one_has_none() {
    let mut mirror = Mirror::default();
    assert_eq!(mirror.name(node(7)), "");
    mirror.set(node(7), field::NAME, FieldValue::Str("Errands".into()));
    assert_eq!(mirror.name(node(7)), "Errands");
    let FieldValue::Str(stored) = &mirror.nodes[&node(7)][&field::NAME] else { panic!("a Str") };
    assert_eq!(mirror.name(node(7)).as_ptr(), stored.as_ptr(), "the stored string, not a copy");
}

#[test]
fn an_unknown_node_reads_as_absent() {
    let mirror = Mirror::default();
    assert_eq!(mirror.get(node(7), field::COLOR), FieldValue::Color(None));
    assert_eq!(mirror.color(node(7)), None);
}

#[test]
fn a_set_value_reads_back() {
    let mut mirror = Mirror::default();
    mirror.set(node(7), field::COLOR, color_value(Some(egui::Color32::RED)));
    assert_eq!(mirror.color(node(7)), Some(egui::Color32::RED));
    mirror.set(node(7), field::COLOR, color_value(None));
    assert_eq!(mirror.color(node(7)), None);
}

#[test]
fn retain_drops_rejected_entries() {
    let mut mirror = Mirror::default();
    mirror.set(node(1), field::COLOR, color_value(Some(egui::Color32::RED)));
    mirror.set(node(2), field::COLOR, color_value(Some(egui::Color32::RED)));
    mirror.retain(|id| id == node(1));
    assert_eq!(mirror.color(node(1)), Some(egui::Color32::RED));
    assert_eq!(mirror.color(node(2)), None);
}

/// A jar's definition is decoded from its mirrored fields on every read.
#[test]
fn a_jar_def_reads_its_mirrored_fields() {
    let mut mirror = Mirror::default();
    assert_eq!(
        mirror.jar_def(node(5)),
        JarDef { name: String::new(), color: None, ephemeral: false, is_default: false }
    );
    mirror.set(node(5), field::NAME, FieldValue::Str("Job".into()));
    mirror.set(node(5), field::COLOR, color_value(Some(egui::Color32::RED)));
    mirror.set(node(5), field::EPHEMERAL, FieldValue::Bool(true));
    let job = JarDef { name: "Job".into(), color: Some(egui::Color32::RED), ephemeral: true, is_default: false };
    assert_eq!(mirror.jar_def(node(5)), job);
}

#[test]
fn pinned_is_the_explicit_bit_and_held_is_group_scoped() {
    use crate::store::model::Anchor;
    let mut mirror = Mirror::default();
    assert!(!mirror.pinned(node(7)) && mirror.held(node(7)).is_none(), "no anchor: neither");
    let drop_in = |group| FieldValue::Anchor(Some(Anchor { pos: [5.0, 6.0], group, explicit: false }));
    mirror.set(node(7), field::ANCHOR, drop_in(None));
    assert_eq!(mirror.held(node(7)), Some(egui::vec2(5.0, 6.0)), "a drop holds in the group it was written in");
    assert!(!mirror.pinned(node(7)), "a drop is not a pin");
    mirror.set_group(node(7), Some(node(4)));
    assert_eq!(mirror.held(node(7)), None, "inert in another group");
    mirror.set(
        node(7),
        field::ANCHOR,
        FieldValue::Anchor(Some(Anchor { pos: [5.0, 6.0], group: None, explicit: true })),
    );
    assert_eq!(mirror.held(node(7)), Some(egui::vec2(5.0, 6.0)), "a pin holds in any group");
    assert!(mirror.pinned(node(7)));
}

/// G2: every field a tab carries (`Kind::Tab.fields()`) survives `carried` → `seed` onto a fresh entry. A tab
/// field added to `TYPES` but not to `Carried` fails here, rather than going unseeded on a new spawn's `Create`.
#[test]
fn carried_holds_every_tab_field() {
    use crate::store::model::{ActorId, Anchor, FieldType};
    let sample = |ty: FieldType| match ty {
        FieldType::Pos => FieldValue::Pos(Some((1.5, -2.0))),
        FieldType::Color => FieldValue::Color(Some(0x123456)),
        FieldType::Id => FieldValue::Id(Some(NodeId { actor: ActorId(1), counter: 2 })),
        FieldType::Str => FieldValue::Str("s".into()),
        FieldType::Bool => FieldValue::Bool(true),
        FieldType::ViewCode => FieldValue::ViewCode(3),
        FieldType::Replica => FieldValue::Replica(3),
        FieldType::Held => FieldValue::Held { dest: None, members: vec![NodeId { actor: ActorId(1), counter: 2 }] },
        FieldType::Anchor => FieldValue::Anchor(Some(Anchor { pos: [1.5, -2.0], group: None, explicit: true })),
    };
    let fields: Vec<u32> =
        Kind::Tab.fields().iter().copied().filter(|&no| no != field::PARENT && no != field::CLOSED).collect();
    assert!(!fields.is_empty());
    let mut a = Mirror::default();
    for &no in &fields {
        a.set(node(1), no, sample(field_def(no).unwrap().ty));
    }
    a.set_parent(node(1), Some(node(3)));
    a.set_group(node(1), Some(node(4)));
    let mut b = Mirror::default();
    b.seed(node(2), &a.carried(node(1)));
    for &no in &fields {
        assert_eq!(b.get(node(2), no), a.get(node(1), no), "field {no} is not carried");
    }
    assert_eq!((b.parent(node(2)), b.group(node(2))), (Some(node(3)), Some(node(4))));
}

fn leaf_at(segs: &[&str]) -> TreePath {
    TreePath::of(&segs.iter().map(|s| tree::Seg::Key((*s).to_owned())).collect::<Vec<_>>())
}

#[test]
fn a_tree_edit_bumps_the_version_and_a_repeat_does_not() {
    let (mut mirror, id) = (Mirror::default(), node(1));
    assert_eq!(mirror.tree_version(id, field::PROPS), 0);
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1)));
    let v1 = mirror.tree_version(id, field::PROPS);
    assert!(v1 > 0);
    assert!(mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1))).is_none());
    assert_eq!(mirror.tree_version(id, field::PROPS), v1);
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), None);
    assert!(mirror.tree(id, field::PROPS).is_none(), "an emptied tree is dropped");
    assert!(mirror.tree_version(id, field::PROPS) == 0);
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1)));
    assert!(mirror.tree_version(id, field::PROPS) > v1, "a version is never reused");
}

#[test]
fn a_version_never_repeats_after_a_forget() {
    let (mut mirror, id) = (Mirror::default(), node(1));
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1)));
    let before = mirror.tree_version(id, field::PROPS);
    mirror.forget_node(id);
    assert_eq!(mirror.tree_version(id, field::PROPS), 0);
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a"]), Some(Leaf::Int(1)));
    assert!(mirror.tree_version(id, field::PROPS) > before);
}

#[test]
fn carried_trees_round_trip_and_seed_never_overwrites() {
    let (mut mirror, id, other) = (Mirror::default(), node(1), node(2));
    mirror.edit_tree(id, field::PROPS, &leaf_at(&["a", "b"]), Some(Leaf::Str("x".into())));
    let carried = mirror.carried(id);
    assert_eq!(carried.trees[&field::PROPS].len(), 1);
    mirror.seed(other, &carried);
    assert_eq!(mirror.trees_of(other), mirror.trees_of(id));
    mirror.edit_tree(other, field::PROPS, &leaf_at(&["a", "b"]), Some(Leaf::Int(2)));
    mirror.seed(other, &carried); // an existing entry stays as it is
    assert_eq!(mirror.tree(other, field::PROPS).unwrap()[&leaf_at(&["a", "b"])], Leaf::Int(2));
}

#[test]
fn retain_drops_the_trees_of_rejected_nodes() {
    let (mut mirror, a, b) = (Mirror::default(), node(1), node(2));
    for id in [a, b] {
        mirror.edit_tree(id, field::PROPS, &leaf_at(&["k"]), Some(Leaf::Int(1)));
    }
    mirror.retain(|id| id == a);
    assert!(mirror.tree(a, field::PROPS).is_some() && mirror.tree(b, field::PROPS).is_none());
}
