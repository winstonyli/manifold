//! The app's decoded copy of the document's fields that have moved off `GraphNode` and its payloads (node model
//! spec, 2b "Read model"). Every read goes through the accessors here, so making `Doc` the app's own state later
//! changes only where they read.
//!
//! The mirror equals the document except for fields and links with an edit still waiting for a save
//! (`App::writes`); `App::edit_field`, `App::edit_parent` and `App::edit_group` are the
//! writers, and `App::mirror_remote` applies another replica's change.

use std::collections::{BTreeSet, HashMap};

use crate::jar_defs::JarDef;
use crate::json_view::JsonView;
use crate::labels::{LabelDef, LABELS_KEY};
use crate::process_manager::ViewKind;
use crate::session_sync::{color_of, pos_in, pos_value, view_kind_of, Fields};
use crate::store::model::{field, field_def, FieldValue, Kind, NodeId};
use crate::store::tree::{self, Leaf, Seg, Tree, TreePath, Trees, TREE_FIELDS};

/// Every field of a node's type (`Kind::fields`) but the two links, `parent` and `group`, which are
/// `links`. A `Create` carries them, and on an existing node only `App::pending` writes them (nothing compares the
/// mirror with the document). `URL` is a tab's shared url
/// and a content node's web source (`App::source_url`), seeded at creation. `JAR` is
/// a tab's or group's jar edge (`jar`), written by `App::set_jar_edge`. `VIEW_KIND` is a tab's engine as
/// its stored code (`view_kind`), written by `App::edit_view_kind`. `POS` is the document pos (`pos`), written by
/// `App::edit_pos`; where a card is drawn is `App::layout`, which the document never reads. `TEXT` is a
/// note's text (`text`), written by `App::edit_text` at a save or blur; the note's payload is its edit buffer. `VIEW`
/// is a Json node's view (`view`, decoded), written by `App::edit_view`. `NAME` is a group's name (`name`), written by
/// `App::edit_name`. A jar's `NAME`, `COLOR` and `EPHEMERAL` are its definition (`jar_def`), written by
/// `App::create_jar`, the generic rename (`App::commit_rename`) and `App::set_jar_ephemeral`.
/// `PATH` is the stored, create-time path (`path`), never rewritten: the payload holds the
/// path resolved on this machine. `TYPE` is the node's type string (`ty`), seeded at every creation and never
/// changed.
#[derive(Default)]
pub(crate) struct Mirror {
    nodes: HashMap<NodeId, Fields>,
    links: HashMap<NodeId, Links>,
    /// `field::VIEW` of `nodes`, decoded (`view`); only non-empty views.
    views: HashMap<NodeId, JsonView>,
    /// The ids of the nodes whose `IS_DEFAULT` is true (`default_jar_ids`), kept by `set` and `retain` like `views`. It may hold
    /// a removed jar until the next `retain`; `App::default_jar` checks liveness.
    defaults: BTreeSet<NodeId>,
    /// Every stored-only node (a link, a label) this replica knows (`stored_ids`). A link's ends are plain fields of
    /// `nodes` (`link_ends`).
    stored_nodes: BTreeSet<NodeId>,
    /// Each tree field's live leaves (`store::tree`) with the version they were last changed at, by `(node, field)`.
    /// Only non-empty trees have an entry. A version is a tick of `tree_clock`, which no forget rewinds.
    trees: HashMap<(NodeId, u32), (Tree, u64)>,
    tree_clock: u64,
    /// Each live label whose name key another, lower-id label also has, mapped to that lowest id
    /// (`canonical_label`). Rebuilt by `regroup_labels` wherever a label is added, forgotten or renamed.
    label_group: HashMap<NodeId, NodeId>,
}

/// A node's links to other nodes, as ids.
#[derive(Default)]
struct Links {
    parent: Option<NodeId>,
    group: Option<NodeId>,
}

impl Mirror {
    /// `id`'s value of field `no`; the field's absent value when the mirror has none.
    pub(crate) fn get(&self, id: NodeId, no: u32) -> FieldValue {
        match self.nodes.get(&id).and_then(|fields| fields.get(&no)) {
            Some(value) => value.clone(),
            None => FieldValue::absent(field_def(no).expect("a known field").ty),
        }
    }

    /// Every write of a mirrored field lands here, so the decoded view (`view`) is refreshed in this one place.
    pub(crate) fn set(&mut self, id: NodeId, no: u32, value: FieldValue) {
        debug_assert!(!matches!(no, field::GROUP | field::PARENT), "a link: `set_group` / `set_parent`");
        if no == field::VIEW {
            let view = match &value {
                FieldValue::Str(text) => JsonView::from_field(text),
                _ => JsonView::default(),
            };
            if view.is_empty() {
                self.views.remove(&id);
            } else {
                self.views.insert(id, view);
            }
        }
        if no == field::IS_DEFAULT {
            if value.as_bool() {
                self.defaults.insert(id);
            } else {
                self.defaults.remove(&id);
            }
        }
        self.nodes.entry(id).or_default().insert(no, value);
        if no == field::NAME && self.stored_nodes.contains(&id) && self.kind_of(id) == Kind::Label {
            self.regroup_labels();
        }
    }

    /// Makes `id` a stored-only node of `kind` (not a card) this replica knows, and seeds its type. Never a graph node.
    pub(crate) fn add_stored(&mut self, id: NodeId, kind: Kind) {
        debug_assert!(!kind.card(), "{kind:?} is a card");
        if self.stored_nodes.insert(id) {
            let ty = kind.type_str().expect("a stored-only kind has a type").to_owned();
            self.init_field(id, field::TYPE, FieldValue::Str(ty));
            if kind == Kind::Label {
                self.regroup_labels();
            }
        }
    }

    /// Drops stored-only node `id` and its fields (a local or remote delete).
    pub(crate) fn forget_stored(&mut self, id: NodeId) {
        let label = self.kind_of(id) == Kind::Label;
        self.stored_nodes.remove(&id);
        self.forget_node(id);
        if label {
            self.regroup_labels();
        }
    }

    /// Rebuilds `label_group`: live labels with the same non-empty `name_key` fold into the lowest id.
    fn regroup_labels(&mut self) {
        let mut first: HashMap<String, NodeId> = HashMap::new();
        let mut group = HashMap::new();
        for id in self.stored_ids(Kind::Label) {
            let key = crate::names::name_key(self.str_field(id, field::NAME));
            if key.is_empty() {
                continue;
            }
            match first.get(&key) {
                Some(&canonical) => {
                    group.insert(id, canonical);
                }
                None => {
                    first.insert(key, id);
                }
            }
        }
        self.label_group = group;
    }

    /// The label `id` reads as: the lowest id among live labels with its name key (itself when alone or unnamed).
    pub(crate) fn canonical_label(&self, id: NodeId) -> NodeId {
        self.label_group.get(&id).copied().unwrap_or(id)
    }

    /// `canonical` and every label folded into it, ascending.
    pub(crate) fn label_members(&self, canonical: NodeId) -> Vec<NodeId> {
        let mut all: Vec<NodeId> = self.label_group.iter().filter(|&(_, &c)| c == canonical).map(|(&m, _)| m).collect();
        all.push(canonical);
        all.sort_unstable();
        all
    }

    pub(crate) fn tree(&self, id: NodeId, no: u32) -> Option<&Tree> {
        self.trees.get(&(id, no)).map(|(tree, _)| tree)
    }

    /// 0 when there is no tree; otherwise a number that changes whenever the tree does and is never reused for this
    /// `(id, no)`, so a cache keyed on it needs no content hash.
    pub(crate) fn tree_version(&self, id: NodeId, no: u32) -> u64 {
        self.trees.get(&(id, no)).map_or(0, |(_, version)| *version)
    }

    /// Every non-empty tree of `id`.
    pub(crate) fn trees_of(&self, id: NodeId) -> Trees {
        TREE_FIELDS.iter().filter_map(|&(no, _)| Some((no, self.tree(id, no)?.clone()))).collect()
    }

    /// Replaces `id`'s tree of field `no` (an empty one removes it). True when it differed.
    pub(crate) fn set_tree(&mut self, id: NodeId, no: u32, tree: Tree) -> bool {
        let same = match self.tree(id, no) {
            Some(old) => *old == tree,
            None => tree.is_empty(),
        };
        if same {
            return false;
        }
        self.tree_clock += 1;
        if tree.is_empty() {
            self.trees.remove(&(id, no));
        } else {
            self.trees.insert((id, no), (tree, self.tree_clock));
        }
        true
    }

    /// A local write at `path` (`store::tree::edit`); returns the leaves it replaced, `None` for no change.
    pub(crate) fn edit_tree(
        &mut self,
        id: NodeId,
        no: u32,
        path: &TreePath,
        leaf: Option<Leaf>,
    ) -> Option<Vec<(TreePath, Leaf)>> {
        let mut tree = self.tree(id, no).cloned().unwrap_or_default();
        let removed = tree::edit(&mut tree, path, leaf)?;
        self.set_tree(id, no, tree);
        Some(removed)
    }

    /// Drops every entry of node `id` (fields, edges, view, Default index): it left the graph without being deleted
    /// (`App::hide_node`), or, for a stored-only node, was forgotten (`forget_stored`).
    pub(crate) fn forget_node(&mut self, id: NodeId) {
        self.nodes.remove(&id);
        self.links.remove(&id);
        self.views.remove(&id);
        self.defaults.remove(&id);
        for (no, _) in TREE_FIELDS {
            self.trees.remove(&(id, no));
        }
    }

    /// Every stored-only node of `kind` this replica knows, by id.
    pub(crate) fn stored_ids(&self, kind: Kind) -> impl Iterator<Item = NodeId> + '_ {
        self.stored_nodes.iter().copied().filter(move |&id| self.kind_of(id) == kind)
    }

    /// The live labels `card` carries, oldest first by `(added_at_ms, label id)`, each as its canonical label
    /// (`canonical_label`) and once, at its earliest stamp. A membership whose label is gone (deleted, or not yet
    /// received) shows nothing, and a value that is not an `Int` is ignored.
    pub(crate) fn card_labels(&self, card: NodeId) -> Vec<NodeId> {
        let Some(tree) = self.tree(card, field::PROPS) else { return Vec::new() };
        let mut found: Vec<(i64, NodeId)> = tree
            .iter()
            .filter_map(|(path, leaf)| match (path.segments().as_slice(), leaf) {
                ([Seg::Key(k), Seg::Id(label)], Leaf::Int(at)) if k == LABELS_KEY => Some((*at, *label)),
                _ => None,
            })
            .filter(|(_, label)| self.is_stored(*label) && self.kind_of(*label) == Kind::Label)
            .map(|(at, label)| (at, self.canonical_label(label)))
            .collect();
        found.sort_unstable();
        let mut seen = BTreeSet::new();
        found.into_iter().filter(|(_, label)| seen.insert(*label)).map(|(_, label)| label).collect()
    }

    /// Every link this replica knows, by id.
    pub(crate) fn link_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.stored_ids(Kind::Link)
    }

    /// Whether `id` is a stored-only node this replica knows.
    pub(crate) fn is_stored(&self, id: NodeId) -> bool {
        self.stored_nodes.contains(&id)
    }

    /// Whether `id` is a link this replica knows.
    pub(crate) fn is_link(&self, id: NodeId) -> bool {
        self.is_stored(id) && self.kind_of(id) == Kind::Link
    }

    /// The kind of `id` by its mirrored type string (`Unknown` when it has none).
    pub(crate) fn kind_of(&self, id: NodeId) -> Kind {
        Kind::from_type(self.str_field(id, field::TYPE))
    }

    /// A label's definition, decoded from its mirrored fields.
    pub(crate) fn label_def(&self, id: NodeId) -> LabelDef {
        LabelDef {
            name: self.str_field(id, field::NAME).to_owned(), // name rule: the stored name, as `LabelDef` holds it
            color: self.color(id),
        }
    }

    /// A link's `from` and `to` (`field::FROM`, `field::TO`), read through the plain field map as `default_jar` is.
    /// Either may name a card that no longer exists, or be `None`.
    pub(crate) fn link_ends(&self, id: NodeId) -> (Option<NodeId>, Option<NodeId>) {
        (self.get(id, field::FROM).as_id(), self.get(id, field::TO).as_id())
    }

    /// A jar's definition, decoded from its mirrored fields.
    pub(crate) fn jar_def(&self, id: NodeId) -> JarDef {
        JarDef {
            name: self.str_field(id, field::NAME).to_owned(), // name rule: the stored name, as `JarDef` holds it
            color: self.color(id),
            ephemeral: self.get(id, field::EPHEMERAL).as_bool(),
            is_default: self.get(id, field::IS_DEFAULT).as_bool(),
        }
    }

    /// The ids whose mirrored `IS_DEFAULT` is true, lowest first: what `App::default_jar` picks from.
    pub(crate) fn default_jar_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.defaults.iter().copied()
    }

    /// Gives `id` field `no` only if it holds no value for it yet: a new content node's web source, seeded after
    /// its placement (`App::insert_content`), which a stored node shown again (a reload, a restore) already has, and
    /// a materialized node's `TYPE` (`App::materialize`), which a node this replica placed already holds.
    pub(crate) fn init_field(&mut self, id: NodeId, no: u32, value: FieldValue) {
        if !self.nodes.get(&id).is_some_and(|fields| fields.contains_key(&no)) {
            self.set(id, no, value);
        }
    }

    /// A string field, borrowed from the stored value; "" when the mirror has none.
    fn str_field(&self, id: NodeId, no: u32) -> &str {
        self.nodes.get(&id).and_then(|fields| fields.get(&no)).and_then(FieldValue::as_str).unwrap_or_default()
    }

    /// The node this one was opened from (`None` for a root).
    pub(crate) fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.links.get(&id)?.parent
    }

    pub(crate) fn set_parent(&mut self, id: NodeId, parent: Option<NodeId>) {
        self.links.entry(id).or_default().parent = parent;
    }

    /// The group this node is shown in (`None` at the top level). Groups nest, so a group has one too.
    pub(crate) fn group(&self, id: NodeId) -> Option<NodeId> {
        self.links.get(&id)?.group
    }

    pub(crate) fn set_group(&mut self, id: NodeId, group: Option<NodeId>) {
        self.links.entry(id).or_default().group = group;
    }

    /// Drops the entries `keep` rejects, except a stored-only node's: it is not a graph node, and only `forget_stored` drops it.
    pub(crate) fn retain(&mut self, keep: impl Fn(NodeId) -> bool) {
        let stored_nodes = &self.stored_nodes;
        let keep = |id: NodeId| keep(id) || stored_nodes.contains(&id);
        self.nodes.retain(|&id, _| keep(id));
        self.links.retain(|&id, _| keep(id));
        self.views.retain(|&id, _| keep(id));
        self.defaults.retain(|&n| keep(n));
        self.trees.retain(|&(id, _), _| keep(id));
    }

    /// A node's document pos: where it was placed or last dropped (`App::edit_pos`), or where another replica put
    /// it. `None` when it has none; the origin is a position like any other. Where the card is drawn is `App::layout`.
    pub(crate) fn pos(&self, id: NodeId) -> Option<egui::Vec2> {
        self.nodes.get(&id).and_then(pos_in)
    }

    /// A note's text: the document's, or what the last save or blur committed (`App::edit_text`). "" when the mirror
    /// has none. Borrowed from the stored string, never a copy: a note can reach 1 MB and the card reads it every
    /// frame. While the note is typed in, the card shows its edit buffer instead (`App::note_text`).
    pub(crate) fn text(&self, id: NodeId) -> &str {
        self.str_field(id, field::TEXT)
    }

    /// A Json node's view overrides (`field::VIEW`), decoded: the cache `set` refreshes whenever the mirror's view
    /// changes, so the per-frame readers never parse. `None` when the node has no overrides.
    pub(crate) fn view(&self, id: NodeId) -> Option<&JsonView> {
        self.views.get(&id)
    }

    /// A group's stored name (`field::NAME`): the document's, or the rename about to be written
    /// (`App::edit_name`). "" when the mirror has none. Raw: drawn only through a name rule (`names.rs`). A jar's
    /// name is read through its `JarDef` (`jar_def`).
    pub(crate) fn name(&self, id: NodeId) -> &str {
        self.str_field(id, field::NAME)
    }

    pub(crate) fn color(&self, id: NodeId) -> Option<egui::Color32> {
        self.nodes.get(&id).and_then(color_in)
    }

    /// Graph view's pin toggle, on any card: the force-directed layout never moves a pinned card, as it never moves
    /// a dragged node (`graph_layout::step`); it still pushes its neighbours.
    pub(crate) fn pinned(&self, id: NodeId) -> bool {
        self.nodes.get(&id).is_some_and(pinned_in)
    }

    /// A tab's shared url: the document's, or the one a local edit is about to write. Its live url, which
    /// every engine report moves, is `TabPayload::url` / `UnloadedTabPayload::url`. For a content node (a note, image,
    /// PDF, Json file or download) it is the web source it came from (`App::source_url`), set at creation. "" when the
    /// mirror has none.
    pub(crate) fn url(&self, id: NodeId) -> &str {
        self.str_field(id, field::URL)
    }

    /// The node's type string (`field::TYPE`, `Kind::type_str`), seeded at every creation and never changed. Read by
    /// Recently Closed (`worth_reopening`, containment) and, for every card, `App::card_kind`. "" when the mirror
    /// has none.
    pub(crate) fn ty(&self, id: NodeId) -> &str {
        self.str_field(id, field::TYPE)
    }

    /// A tab's or group's `jar` edge (`field::JAR`): the jar it names, `None` to inherit. It may name a jar that no
    /// longer exists or sits out of reach (`App::resolve_jar`).
    pub(crate) fn jar(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(&id).and_then(jar_in)
    }

    /// A tab's engine: the one it runs or is switching to (`App::edit_view_kind`), the document's otherwise. The mirror
    /// holds the stored code as is, so a code from a newer build survives a save; it, and an absent one, read as
    /// Servo (`view_kind_of`). `ViewTab::kind` is the engine actually running.
    pub(crate) fn view_kind(&self, id: NodeId) -> ViewKind {
        view_kind_of(self.view_code(id))
    }

    /// A tab's engine as the stored code, kept as is (0: none).
    pub(crate) fn view_code(&self, id: NodeId) -> u64 {
        self.get(id, field::VIEW_KIND).as_view_code()
    }

    /// Whether `id` has an entry: every node placed here, and a tab mid-respawn (its entry outlives the respawn). A
    /// new spawn has none until it lands (`App::linked_ids`).
    pub(crate) fn has(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id) || self.links.contains_key(&id)
    }

    /// What `id` carries across a respawn or lands with: every field the mirror holds for it but the absent ones, and
    /// its two links. A field added to the document is carried with no change here.
    pub(crate) fn carried(&self, id: NodeId) -> Carried {
        let held = self.nodes.get(&id).into_iter().flatten();
        let carried = held.fold(Carried::default(), |carried, (&no, value)| carried.with(no, value.clone()));
        Carried { parent: self.parent(id), group: self.group(id), trees: self.trees_of(id), ..carried }
    }

    /// `init` for everything a node carries; absent values (no pos, an empty string, engine code 0, `false`, no jar) are
    /// left out, as `Carried::with` never holds them. The type alone is `init_field`: an entry made before the node
    /// was placed (a remote op naming it ahead of its `Create`, a link cascade) must not leave the placed node
    /// untyped, and the store never changes a type. The entry exists afterwards even when nothing was
    /// seeded: a placed node is known here.
    pub(crate) fn seed(&mut self, id: NodeId, carried: &Carried) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.nodes.entry(id) {
            e.insert(Fields::new());
            for (&no, value) in carried.fields.iter().filter(|(&no, _)| no != field::TYPE) {
                self.set(id, no, value.clone());
            }
            for (&no, tree) in &carried.trees {
                self.set_tree(id, no, tree.clone());
            }
        }
        if let Some(ty) = carried.fields.get(&field::TYPE) {
            self.init_field(id, field::TYPE, ty.clone());
        }
        self.links.entry(id).or_insert_with(|| Links { parent: carried.parent, group: carried.group });
    }
}

fn color_in(fields: &Fields) -> Option<egui::Color32> {
    color_of(fields.get(&field::COLOR)?)
}

fn pinned_in(fields: &Fields) -> bool {
    fields.get(&field::PINNED).is_some_and(FieldValue::as_bool)
}

fn jar_in(fields: &Fields) -> Option<NodeId> {
    fields.get(&field::JAR).and_then(FieldValue::as_id)
}

/// What a tab carries across a respawn or lands with: the mirror's values for it, as a field map (no absent value
/// is held) and the two links. `place_node` seeds a new node's links, pos, type and stored path with it too. Read
/// the fields a respawn needs through `pos`, `pinned` and `jar`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Carried {
    fields: Fields,
    /// The node this one was opened from.
    pub parent: Option<NodeId>,
    /// The group it is shown in.
    pub group: Option<NodeId>,
    /// Its tree fields' live leaves.
    pub trees: Trees,
}

impl Carried {
    /// `trees` as its tree fields' leaves.
    pub(crate) fn with_trees(mut self, trees: Trees) -> Self {
        self.trees = trees;
        self
    }

    /// `value` for field `no`; an absent value removes it, so a carried map never holds one.
    pub(crate) fn with(mut self, no: u32, value: FieldValue) -> Self {
        if value == FieldValue::absent(field_def(no).expect("a known field").ty) {
            self.fields.remove(&no);
        } else {
            self.fields.insert(no, value);
        }
        self
    }

    pub(crate) fn in_group(self, group: Option<NodeId>) -> Self {
        Carried { group, ..self }
    }

    pub(crate) fn with_str(self, no: u32, text: impl Into<String>) -> Self {
        self.with(no, FieldValue::Str(text.into()))
    }

    /// A tab landing or respawning: where it sits, what it keeps, the url it opens and its engine's stored code.
    pub(crate) fn tab(
        pos: Option<egui::Vec2>,
        pinned: bool,
        (parent, group): (Option<NodeId>, Option<NodeId>),
        url: String,
        view_kind: u64,
        jar: Option<NodeId>,
    ) -> Self {
        let carried = Carried { parent, group, ..Carried::default() };
        let carried = pos.map_or(carried.clone(), |pos| carried.with(field::POS, pos_value(pos)));
        carried
            .with(field::PINNED, FieldValue::Bool(pinned))
            .with_str(field::URL, url)
            .with(field::VIEW_KIND, FieldValue::ViewCode(view_kind))
            .with(field::JAR, FieldValue::Id(jar))
    }

    pub(crate) fn pos(&self) -> Option<egui::Vec2> {
        pos_in(&self.fields)
    }

    pub(crate) fn pinned(&self) -> bool {
        pinned_in(&self.fields)
    }

    pub(crate) fn jar(&self) -> Option<NodeId> {
        jar_in(&self.fields)
    }
}

#[cfg(test)]
mod tests {
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
            .with(field::PINNED, FieldValue::Bool(true))
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
        assert_eq!(
            (mirror.ty(node(7)), mirror.url(node(7))),
            ("text/x", "https://a/"),
            "the type lands, the rest is kept"
        );
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
    fn pinned_reads_a_bool_and_defaults_to_false() {
        let mut mirror = Mirror::default();
        assert!(!mirror.pinned(node(7)));
        mirror.set(node(7), field::PINNED, FieldValue::Bool(true));
        assert!(mirror.pinned(node(7)));
    }

    /// G2: every field a tab carries (`Kind::Tab.fields()`) survives `carried` → `seed` onto a fresh entry. A tab
    /// field added to `TYPES` but not to `Carried` fails here, rather than going unseeded on a new spawn's `Create`.
    #[test]
    fn carried_holds_every_tab_field() {
        use crate::store::model::{ActorId, FieldType};
        let sample = |ty: FieldType| match ty {
            FieldType::Pos => FieldValue::Pos(Some((1.5, -2.0))),
            FieldType::Color => FieldValue::Color(Some(0x123456)),
            FieldType::Id => FieldValue::Id(Some(NodeId { actor: ActorId(1), counter: 2 })),
            FieldType::Str => FieldValue::Str("s".into()),
            FieldType::Bool => FieldValue::Bool(true),
            FieldType::ViewCode => FieldValue::ViewCode(3),
            FieldType::Replica => FieldValue::Replica(3),
            FieldType::Held => FieldValue::Held { dest: None, members: vec![NodeId { actor: ActorId(1), counter: 2 }] },
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
}
