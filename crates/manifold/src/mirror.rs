//! The app's decoded copy of the document's fields that have moved off `GraphNode` and its payloads (node model
//! spec, 2b "Read model"). Every read goes through the accessors here, so making `Doc` the app's own state later
//! changes only where they read.
//!
//! The mirror equals the document except for fields and links with an edit still waiting for a save
//! (`App::pending`, `App::writes`.`pending_moves`); `App::edit_field`, `App::edit_parent` and `App::edit_group` are the
//! writers, and `App::mirror_remote` applies another replica's change.

use std::collections::{BTreeSet, HashMap};

use crate::jar::JarChoice;
use crate::jar_defs::JarDef;
use crate::json_view::JsonView;
use crate::process_manager::ViewKind;
use crate::session_sync::{color_of, color_value, pos_value, view_kind_of, Fields};
use crate::store::model::{field, field_def, FieldValue, Kind, NodeId};

/// Every field of a node's type (`Kind::fields`, 2b-5 Ruling 2) but the two links, `parent` and `group`, which are
/// `links`. A `Create` carries them, and on an existing node only `App::pending` writes them (nothing compares the
/// mirror with the document). `URL` is a tab's shared url
/// (2b-3 Ruling 2) and a content node's web source (`App::source_url`), seeded at creation since 2b-5. `JAR` and
/// `JAR_SET` are a tab's jar choice (`jar_choice`), written together by `App::edit_jar`. `VIEW_KIND` is a tab's engine as
/// its stored code (`view_kind`), written by `App::edit_view_kind`. `POS` is the document pos (`pos`), written by
/// `App::edit_pos`; where a card is drawn is `App::layout`, which the document never reads (2b-4 Ruling 1). `TEXT` is a
/// note's text (`text`), written by `App::edit_text` at a save or blur; the note's payload is its edit buffer. `VIEW`
/// is a Json node's view (`view`, decoded), written by `App::edit_view`. `NAME` is a group's name (`name`), written by
/// `App::edit_name`. A jar's `NAME`, `COLOR` and `EPHEMERAL` are its definition (`jar_def`), written by
/// `App::create_jar`, the generic rename (`App::commit_rename`) and `App::set_jar_ephemeral` (2b-5 Ruling 7).
/// `DEFAULT_JAR` is a group's default jar (`default_jar`), written by `App::edit_default_jar` and resolved at commit
/// like `JAR` (2b-5 Ruling 6). `PATH` is the stored, create-time path (`path`), never rewritten: the payload holds the
/// path resolved on this machine (2b-5 Ruling 5). `TYPE` is the node's type string (`ty`), seeded at every creation and never changed (Ruling 10).
#[derive(Default)]
pub(crate) struct Mirror {
    nodes: HashMap<String, Fields>,
    links: HashMap<String, Links>,
    /// `field::VIEW` of `nodes`, decoded (`view`); only non-empty views.
    views: HashMap<String, JsonView>,
    /// The ids of the nodes whose `IS_DEFAULT` is true (`default_jar_ids`), kept by `set` and `retain` like `views`. It may hold
    /// a removed jar until the next `retain`; `App::default_jar` checks liveness.
    defaults: BTreeSet<NodeId>,
    /// Every `Kind::Link` node this replica knows (`link_ids`); their ends are plain fields of `nodes` (`link_ends`).
    link_nodes: BTreeSet<NodeId>,
}

/// A node's links to other nodes, as ids: `Option<String>` so hand-built test ids work and per-frame filters
/// compare without allocating.
#[derive(Default)]
struct Links {
    parent: Option<String>,
    group: Option<String>,
}

impl Mirror {
    /// `id`'s value of field `no`; the field's absent value when the mirror has none.
    pub(crate) fn get(&self, id: &str, no: u32) -> FieldValue {
        match self.nodes.get(id).and_then(|fields| fields.get(&no)) {
            Some(value) => value.clone(),
            None => FieldValue::absent(field_def(no).expect("a known field").ty),
        }
    }

    /// Every write of a mirrored field lands here, so the decoded view (`view`) is refreshed in this one place.
    pub(crate) fn set(&mut self, id: &str, no: u32, value: FieldValue) {
        debug_assert!(!matches!(no, field::GROUP | field::PARENT), "a link: `set_group` / `set_parent`");
        if no == field::VIEW {
            let view = match &value {
                FieldValue::Str(text) => JsonView::from_field(text),
                _ => JsonView::default(),
            };
            if view.is_empty() {
                self.views.remove(id);
            } else {
                self.views.insert(id.to_owned(), view);
            }
        }
        if no == field::IS_DEFAULT {
            if let Ok(node) = id.parse::<NodeId>() {
                if value == FieldValue::Bool(true) {
                    self.defaults.insert(node);
                } else {
                    self.defaults.remove(&node);
                }
            }
        }
        self.nodes.entry(id.to_owned()).or_default().insert(no, value);
    }

    /// Makes `id` a link this replica knows, and seeds its type. A link is never a graph node.
    pub(crate) fn add_link(&mut self, id: NodeId) {
        if self.link_nodes.insert(id) {
            let ty = Kind::Link.type_str().expect("a link type").to_owned();
            self.init_field(&id.to_string(), field::TYPE, FieldValue::Str(ty));
        }
    }

    /// Drops link `id` and its fields (a local or remote delete).
    pub(crate) fn forget_link(&mut self, id: NodeId) {
        self.link_nodes.remove(&id);
        let key = id.to_string();
        self.nodes.remove(&key);
        self.links.remove(&key);
        self.views.remove(&key);
    }

    /// Every link this replica knows, by id.
    pub(crate) fn link_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.link_nodes.iter().copied()
    }

    /// Whether `id` is a link this replica knows.
    pub(crate) fn is_link(&self, id: NodeId) -> bool {
        self.link_nodes.contains(&id)
    }

    /// A link's `from` and `to` (`field::FROM`, `field::TO`), read through the plain field map as `default_jar` is.
    /// Either may name a card that no longer exists, or be `None`.
    pub(crate) fn link_ends(&self, id: &str) -> (Option<NodeId>, Option<NodeId>) {
        let end = |no| match self.get(id, no) {
            FieldValue::Id(end) => end,
            _ => None,
        };
        (end(field::FROM), end(field::TO))
    }

    /// A jar's definition, decoded from its mirrored fields (2b-5 Ruling 7).
    pub(crate) fn jar_def(&self, id: &str) -> JarDef {
        JarDef {
            name: self.str_field(id, field::NAME).to_owned(), // name rule: the stored name, as `JarDef` holds it
            color: self.color(id),
            ephemeral: matches!(self.get(id, field::EPHEMERAL), FieldValue::Bool(true)),
            is_default: matches!(self.get(id, field::IS_DEFAULT), FieldValue::Bool(true)),
        }
    }

    /// The ids whose mirrored `IS_DEFAULT` is true, lowest first: what `App::default_jar` picks from.
    pub(crate) fn default_jar_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.defaults.iter().copied()
    }

    /// Gives `id` these values only if it has no entry yet (a spawn landing: a new tab takes what it was started
    /// with, an existing node keeps what the mirror already holds).
    pub(crate) fn init(&mut self, id: &str, fields: impl IntoIterator<Item = (u32, FieldValue)>) {
        if !self.nodes.contains_key(id) {
            for (no, value) in fields {
                self.set(id, no, value);
            }
        }
    }

    /// Gives `id` field `no` only if it holds no value for it yet: a new content node's web source, seeded after
    /// its placement (`App::insert_content`), which a stored node shown again (a reload, a restore) already has, and
    /// a materialized node's `TYPE` (`App::materialize`), which a node this replica placed already holds.
    pub(crate) fn init_field(&mut self, id: &str, no: u32, value: FieldValue) {
        if !self.nodes.get(id).is_some_and(|fields| fields.contains_key(&no)) {
            self.set(id, no, value);
        }
    }

    /// A string field, borrowed from the stored value; "" when the mirror has none.
    fn str_field(&self, id: &str, no: u32) -> &str {
        match self.nodes.get(id).and_then(|fields| fields.get(&no)) {
            Some(FieldValue::Str(text)) => text,
            _ => "",
        }
    }

    /// The node this one was opened from (`None` for a root).
    pub(crate) fn parent(&self, id: &str) -> Option<&str> {
        self.links.get(id)?.parent.as_deref()
    }

    pub(crate) fn set_parent(&mut self, id: &str, parent: Option<String>) {
        self.links.entry(id.to_owned()).or_default().parent = parent;
    }

    /// The group this node is shown in (`None` at the top level). Groups nest, so a group has one too.
    pub(crate) fn group(&self, id: &str) -> Option<&str> {
        self.links.get(id)?.group.as_deref()
    }

    pub(crate) fn set_group(&mut self, id: &str, group: Option<String>) {
        self.links.entry(id.to_owned()).or_default().group = group;
    }

    /// Drops the entries `keep` rejects, except a link's: a link is not a graph node, and only `forget_link` drops it.
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        let link_nodes = &self.link_nodes;
        let keep = |id: &str| keep(id) || id.parse::<NodeId>().is_ok_and(|n| link_nodes.contains(&n));
        self.nodes.retain(|id, _| keep(id));
        self.links.retain(|id, _| keep(id));
        self.views.retain(|id, _| keep(id));
        self.defaults.retain(|n| keep(&n.to_string()));
    }

    /// A node's document pos: where it was placed or last dropped (`App::edit_pos`), or where another replica put
    /// it. `None` when it has none. Pos (0,0) is the field's absent value in the encoding, so a node at the exact
    /// origin reads as `None` too (2b-4 Ruling 5). Where the card is drawn is `App::layout`.
    pub(crate) fn pos(&self, id: &str) -> Option<egui::Vec2> {
        match self.nodes.get(id)?.get(&field::POS)? {
            FieldValue::Pos(x, y) if (*x, *y) != (0.0, 0.0) => Some(egui::vec2(*x, *y)),
            _ => None,
        }
    }

    /// A note's text: the document's, or what the last save or blur committed (`App::edit_text`). "" when the mirror
    /// has none. Borrowed from the stored string, never a copy: a note can reach 1 MB and the card reads it every
    /// frame (2b-4 Ruling 6). While the note is typed in, the card shows its edit buffer instead (`App::note_text`).
    pub(crate) fn text(&self, id: &str) -> &str {
        self.str_field(id, field::TEXT)
    }

    /// A Json node's view overrides (`field::VIEW`), decoded: the cache `set` refreshes whenever the mirror's view
    /// changes (2b-4 Ruling 8), so the per-frame readers never parse. `None` when the node has no overrides.
    pub(crate) fn view(&self, id: &str) -> Option<&JsonView> {
        self.views.get(id)
    }

    /// A group's stored name (`field::NAME`, 2b-4 Ruling 9): the document's, or the rename about to be written
    /// (`App::edit_name`). "" when the mirror has none. Raw: drawn only through a name rule (`names.rs`). A jar's
    /// name is read through its `JarDef` (`jar_def`).
    pub(crate) fn name(&self, id: &str) -> &str {
        self.str_field(id, field::NAME)
    }

    pub(crate) fn color(&self, id: &str) -> Option<egui::Color32> {
        color_of(&self.get(id, field::COLOR))
    }

    /// Graph view's pin toggle (tabs only): the force-directed layout never moves a pinned tab, as it never moves
    /// a dragged node (`graph_layout::step`); it still pushes its neighbours. Only tabs carry the field.
    pub(crate) fn pinned(&self, id: &str) -> bool {
        matches!(self.get(id, field::PINNED), FieldValue::Bool(true))
    }

    /// A tab's shared url: the document's, or the one a local edit is about to write (Ruling 1). Its live url, which
    /// every engine report moves, is `TabPayload::url` / `UnloadedTabPayload::url`. For a content node (a note, image,
    /// PDF, Json file or download) it is the web source it came from (`App::source_url`), set at creation. "" when the
    /// mirror has none.
    pub(crate) fn url(&self, id: &str) -> &str {
        self.str_field(id, field::URL)
    }

    /// A content node's stored path (`field::PATH`): the one it was created with, never rewritten (2b-5 Ruling 5).
    /// The file it opens on this machine is its payload's `path` (`ContentIndex::resolve`, or a reload's new file).
    /// "" when the mirror has none.
    pub(crate) fn path(&self, id: &str) -> &str {
        self.str_field(id, field::PATH)
    }

    /// A group's default jar (`field::DEFAULT_JAR`): the jar new tabs in it start with (`App::inherited_jar`). `None`
    /// inherits from the group above. It may name a jar that no longer exists (2b-5 Ruling 6).
    pub(crate) fn default_jar(&self, id: &str) -> Option<NodeId> {
        match self.get(id, field::DEFAULT_JAR) {
            FieldValue::Id(jar) => jar,
            _ => None,
        }
    }

    /// The node's type string (`field::TYPE`, `Kind::type_str`), seeded at every creation and never changed. Read by
    /// Recently Closed (`worth_reopening`, containment) and, for every card, `App::card_kind`. "" when the mirror
    /// has none.
    pub(crate) fn ty(&self, id: &str) -> &str {
        self.str_field(id, field::TYPE)
    }

    /// A tab's jar choice (`field::JAR`, `field::JAR_SET`): the document's, or the one a local edit is about to write.
    /// The default jar, unset, when the mirror has none. It may name a jar that no longer exists (`App::resolved_jar`).
    pub(crate) fn jar_choice(&self, id: &str) -> JarChoice {
        let jar = match self.get(id, field::JAR) {
            FieldValue::Id(jar) => jar,
            _ => None,
        };
        JarChoice { jar, set: matches!(self.get(id, field::JAR_SET), FieldValue::Bool(true)) }
    }

    /// A tab's engine: the one it runs or is switching to (`App::edit_view_kind`), the document's otherwise. The mirror
    /// holds the stored code as is, so a code from a newer build survives a save; it, and an absent one, read as
    /// Servo (`view_kind_of`, 2b-3 Ruling 6). `ViewTab::kind` is the engine actually running.
    pub(crate) fn view_kind(&self, id: &str) -> ViewKind {
        view_kind_of(self.view_code(id))
    }

    /// A tab's engine as the stored code, kept as is (0: none).
    pub(crate) fn view_code(&self, id: &str) -> u64 {
        match self.get(id, field::VIEW_KIND) {
            FieldValue::ViewCode(code) => code,
            _ => 0,
        }
    }

    /// Whether `id` has an entry: every node placed here, and a tab mid-respawn (its entry outlives the respawn). A
    /// new spawn has none until it lands (`App::linked_ids`).
    pub(crate) fn has(&self, id: &str) -> bool {
        self.nodes.contains_key(id) || self.links.contains_key(id)
    }

    pub(crate) fn carried(&self, id: &str) -> Carried {
        Carried {
            pos: self.pos(id),
            color: self.color(id),
            pinned: self.pinned(id),
            parent: self.parent(id).map(str::to_owned),
            group: self.group(id).map(str::to_owned),
            url: self.url(id).to_owned(),
            text: self.text(id).to_owned(),
            view_kind: self.view_code(id),
            jar: self.jar_choice(id),
            ty: self.ty(id).to_owned(),
            path: self.path(id).to_owned(),
            default_jar: self.default_jar(id),
            // name rule: carried as stored
            name: self.name(id).to_owned(),
        }
    }

    /// `init` for everything a node carries. No pos, an empty string, engine code 0, `false` and no jar (the fields a
    /// node's kind lacks, or holds unset) are the fields' absent values, so they are left out. The type alone is
    /// `init_field`: an entry made before the node was placed (a remote op naming it ahead of its `Create`, a
    /// link cascade) must not leave the placed node untyped, and the store never changes a type (2b-5 Ruling 10).
    pub(crate) fn seed(&mut self, id: &str, carried: &Carried) {
        let str = |no: u32, s: &String| (!s.is_empty()).then(|| (no, FieldValue::Str(s.clone())));
        let pos = carried.pos.map(|pos| (field::POS, pos_value(pos)));
        let view_kind = (carried.view_kind != 0).then_some((field::VIEW_KIND, FieldValue::ViewCode(carried.view_kind)));
        let jar = carried.jar.jar.is_some().then_some((field::JAR, FieldValue::Id(carried.jar.jar)));
        let jar_set = carried.jar.set.then_some((field::JAR_SET, FieldValue::Bool(true)));
        let default_jar = carried.default_jar.map(|jar| (field::DEFAULT_JAR, FieldValue::Id(Some(jar))));
        let strings = [
            str(field::URL, &carried.url),
            str(field::TEXT, &carried.text),
            str(field::PATH, &carried.path),
            // name rule: seeded as stored
            str(field::NAME, &carried.name),
        ];
        let fields = [(field::COLOR, color_value(carried.color)), (field::PINNED, FieldValue::Bool(carried.pinned))];
        let optional = [pos, view_kind, jar, jar_set, default_jar];
        self.init(id, fields.into_iter().chain(optional.into_iter().flatten()).chain(strings.into_iter().flatten()));
        // After `init`, which seeds nothing once the entry exists: the type is the one field that still lands.
        if !carried.ty.is_empty() {
            self.init_field(id, field::TYPE, FieldValue::Str(carried.ty.clone()));
        }
        if !self.links.contains_key(id) {
            let links = Links { parent: carried.parent.clone(), group: carried.group.clone() };
            self.links.insert(id.to_owned(), links);
        }
    }
}

/// What a tab carries across a respawn or lands with: the mirror's values for it. `place_node` seeds a new node's
/// links, pos, type and stored path with it too.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Carried {
    /// The document pos (`Mirror::pos`): a new node's placement, which its `Create` carries (2b-4 Ruling 3).
    pub pos: Option<egui::Vec2>,
    pub color: Option<egui::Color32>,
    pub pinned: bool,
    pub parent: Option<String>,
    pub group: Option<String>,
    /// The url (`Mirror::url`): a tab's shared url, or a content node's web source. `place_node` passes only a tab's
    /// (from `saved_tab`); a content node's is seeded by its caller (`App::seed_source_url`). "" for none.
    pub url: String,
    /// A note's text (`Mirror::text`): what a new note starts with, which its `Create` carries; "" for other kinds.
    pub text: String,
    /// The engine's stored code (`Mirror::engine` decodes it, `session_sync::view_code` makes one); 0, the absent
    /// value, for a node that isn't a tab.
    pub view_kind: u64,
    /// The jar choice (`Mirror::jar_choice`); the default for a node that isn't a tab.
    pub jar: JarChoice,
    /// The type string (`Mirror::ty`); "" for none.
    pub ty: String,
    /// The stored path (`Mirror::path`); "" for a node without a file.
    pub path: String,
    /// `Mirror::default_jar`; `None` for a node that isn't a group.
    pub default_jar: Option<NodeId>,
    /// `Mirror::name`: a rename of any node, or a group's name; "" for none.
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_index_follows_is_default_writes_and_retain() {
        let mut m = Mirror::default();
        let (a, b) = (
            NodeId { actor: crate::store::model::ActorId(1), counter: 1 },
            NodeId { actor: crate::store::model::ActorId(1), counter: 2 },
        );
        let (sa, sb) = (a.to_string(), b.to_string());
        m.set(&sb, field::IS_DEFAULT, FieldValue::Bool(true));
        m.set(&sa, field::IS_DEFAULT, FieldValue::Bool(true));
        assert_eq!(m.default_jar_ids().collect::<Vec<_>>(), [a, b], "lowest first");
        m.set(&sa, field::IS_DEFAULT, FieldValue::Bool(false));
        m.retain(|id| id != sb);
        assert_eq!(m.default_jar_ids().count(), 0);
    }

    #[test]
    fn seed_then_carried_round_trips_and_never_overwrites() {
        let mut mirror = Mirror::default();
        let first = Carried {
            pos: Some(egui::vec2(3.0, -4.0)),
            color: Some(egui::Color32::RED),
            pinned: true,
            parent: Some("p".into()),
            group: Some("g".into()),
            url: "https://a/".into(),
            text: "note".into(),
            view_kind: 99,
            jar: JarChoice {
                jar: Some(crate::store::model::NodeId { actor: crate::store::model::ActorId(1), counter: 2 }),
                set: true,
            },
            ty: "text/x".into(),
            path: "C:/a.json".into(),
            default_jar: Some(NodeId { actor: crate::store::model::ActorId(1), counter: 3 }),
            name: "Mine".into(),
        };
        mirror.seed("7", &first);
        assert_eq!(mirror.carried("7"), first);
        mirror.seed("7", &Carried { url: "https://b/".into(), ..Carried::default() });
        assert_eq!(mirror.carried("7"), first);
        mirror.seed("8", &Carried { pos: Some(egui::Vec2::ZERO), ..Carried::default() });
        assert_eq!(mirror.pos("8"), None, "pos (0,0) is the absent value (Ruling 5)");
    }

    /// An entry that exists without a type (a remote op named the node before it was placed) takes the placed
    /// node's type; one with a type keeps it.
    #[test]
    fn seed_types_an_entry_that_has_none() {
        let mut mirror = Mirror::default();
        mirror.set("7", field::URL, FieldValue::Str("https://a/".into()));
        mirror.seed("7", &Carried { ty: "text/x".into(), url: "https://b/".into(), ..Carried::default() });
        assert_eq!((mirror.ty("7"), mirror.url("7")), ("text/x", "https://a/"), "the type lands, the rest is kept");
        mirror.seed("7", &Carried { ty: "text/y".into(), ..Carried::default() });
        assert_eq!(mirror.ty("7"), "text/x");
        mirror.seed("8", &Carried::default());
        assert!(!mirror.nodes["8"].contains_key(&field::TYPE), "no type seeds nothing");
    }

    #[test]
    fn a_jar_choice_reads_back_and_a_node_without_one_has_the_default() {
        let mut mirror = Mirror::default();
        assert_eq!(mirror.jar_choice("7"), JarChoice::default());
        mirror.seed("8", &Carried::default());
        assert!(!mirror.nodes["8"].contains_key(&field::JAR), "the default jar seeds nothing");
        assert!(!mirror.nodes["8"].contains_key(&field::JAR_SET));
        assert!(mirror.has("8") && !mirror.has("7"));
        mirror.set("7", field::JAR_SET, FieldValue::Bool(true));
        assert_eq!(mirror.jar_choice("7"), JarChoice { jar: None, set: true });
    }

    #[test]
    fn a_view_reads_back_decoded_and_keeps_an_unknown_code() {
        let mut mirror = Mirror::default();
        assert_eq!(mirror.view_kind("7"), ViewKind::Servo, "absent is Servo");
        mirror.seed("8", &Carried::default());
        assert!(!mirror.nodes["8"].contains_key(&field::VIEW_KIND), "code 0 seeds nothing");
        let cef = crate::session_sync::view_code(ViewKind::Cef);
        mirror.set("7", field::VIEW_KIND, FieldValue::ViewCode(cef));
        assert_eq!(mirror.view_kind("7"), ViewKind::Cef);
        mirror.set("7", field::VIEW_KIND, FieldValue::ViewCode(99));
        assert_eq!(mirror.view_kind("7"), ViewKind::Servo, "a newer build's code reads as Servo");
        assert_eq!(mirror.carried("7").view_kind, 99, "and is carried as is");
    }

    #[test]
    fn a_url_reads_back_and_a_node_without_one_has_none() {
        let mut mirror = Mirror::default();
        assert_eq!(mirror.url("7"), "");
        mirror.seed("8", &Carried::default());
        assert!(!mirror.nodes["8"].contains_key(&field::URL), "an empty url seeds nothing");
        mirror.set("7", field::URL, FieldValue::Str("https://a/".into()));
        assert_eq!(mirror.url("7"), "https://a/");
    }

    #[test]
    fn a_node_without_group_or_file_fields_seeds_none_and_reads_them_as_absent() {
        let mut mirror = Mirror::default();
        mirror.seed("8", &Carried::default());
        for no in [field::TYPE, field::PATH, field::DEFAULT_JAR] {
            assert!(!mirror.nodes["8"].contains_key(&no), "field {no} seeds nothing");
        }
        assert_eq!((mirror.ty("8"), mirror.path("8")), ("", ""));
        assert!(mirror.default_jar("8").is_none());
    }

    #[test]
    fn init_field_never_overwrites_a_value() {
        let mut mirror = Mirror::default();
        mirror.init_field("7", field::URL, FieldValue::Str("https://a/".into()));
        mirror.init_field("7", field::URL, FieldValue::Str("https://b/".into()));
        assert_eq!(mirror.url("7"), "https://a/");
        mirror.set("8", field::URL, FieldValue::Str(String::new()));
        mirror.init_field("8", field::URL, FieldValue::Str("https://b/".into()));
        assert_eq!(mirror.url("8"), "", "an absent value held is a value");
    }

    #[test]
    fn a_text_reads_back_borrowed_and_a_node_without_one_has_none() {
        let mut mirror = Mirror::default();
        assert_eq!(mirror.text("7"), "");
        mirror.seed("8", &Carried::default());
        assert!(!mirror.nodes["8"].contains_key(&field::TEXT), "an empty text seeds nothing");
        mirror.set("7", field::TEXT, FieldValue::Str("hello".into()));
        assert_eq!(mirror.text("7"), "hello");
        let FieldValue::Str(stored) = &mirror.nodes["7"][&field::TEXT] else { panic!("a Str") };
        assert_eq!(mirror.text("7").as_ptr(), stored.as_ptr(), "the stored string, not a copy (2b-4 Ruling 6)");
    }

    #[test]
    fn a_view_is_decoded_on_every_set_and_dropped_with_its_entry() {
        let mut mirror = Mirror::default();
        assert!(mirror.view("7").is_none());
        mirror.set("7", field::VIEW, FieldValue::Str(r#"{"/a":{"collapsed":true}}"#.into()));
        assert_eq!(mirror.view("7").and_then(|v| v.get("/a")).and_then(|o| o.collapsed), Some(true));
        mirror.set("7", field::VIEW, FieldValue::Str(r#"{"/b":{"collapsed":false}}"#.into()));
        let view = mirror.view("7").expect("a view");
        assert!(view.get("/a").is_none() && view.get("/b").is_some(), "refreshed, not merged");
        mirror.set("7", field::VIEW, FieldValue::Str(String::new()));
        assert!(mirror.view("7").is_none(), "an empty view is none");
        mirror.set("8", field::VIEW, FieldValue::Str(r#"{"/a":{"collapsed":true}}"#.into()));
        mirror.retain(|id| id != "8");
        assert!(mirror.view("8").is_none(), "the cache goes with the entry");
    }

    #[test]
    fn a_name_reads_back_borrowed_and_a_node_without_one_has_none() {
        let mut mirror = Mirror::default();
        assert_eq!(mirror.name("7"), "");
        mirror.set("7", field::NAME, FieldValue::Str("Errands".into()));
        assert_eq!(mirror.name("7"), "Errands");
        let FieldValue::Str(stored) = &mirror.nodes["7"][&field::NAME] else { panic!("a Str") };
        assert_eq!(mirror.name("7").as_ptr(), stored.as_ptr(), "the stored string, not a copy");
    }

    #[test]
    fn an_unknown_node_reads_as_absent() {
        let mirror = Mirror::default();
        assert_eq!(mirror.get("7", field::COLOR), FieldValue::Color(None));
        assert_eq!(mirror.color("7"), None);
    }

    #[test]
    fn a_set_value_reads_back() {
        let mut mirror = Mirror::default();
        mirror.set("7", field::COLOR, color_value(Some(egui::Color32::RED)));
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
        mirror.set("7", field::COLOR, color_value(None));
        assert_eq!(mirror.color("7"), None);
    }

    #[test]
    fn init_never_overwrites_an_entry() {
        let mut mirror = Mirror::default();
        mirror.init("7", [(field::COLOR, color_value(Some(egui::Color32::RED)))]);
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
        mirror.init("7", [(field::COLOR, color_value(Some(egui::Color32::BLUE)))]);
        assert_eq!(mirror.color("7"), Some(egui::Color32::RED));
    }

    #[test]
    fn retain_drops_rejected_entries() {
        let mut mirror = Mirror::default();
        mirror.set("a", field::COLOR, color_value(Some(egui::Color32::RED)));
        mirror.set("b", field::COLOR, color_value(Some(egui::Color32::RED)));
        mirror.retain(|id| id == "a");
        assert_eq!(mirror.color("a"), Some(egui::Color32::RED));
        assert_eq!(mirror.color("b"), None);
    }

    /// 2b-5 Ruling 7: a jar's definition is decoded from its mirrored fields on every read.
    #[test]
    fn a_jar_def_reads_its_mirrored_fields() {
        let mut mirror = Mirror::default();
        assert_eq!(
            mirror.jar_def("5"),
            JarDef { name: String::new(), color: None, ephemeral: false, is_default: false }
        );
        mirror.set("5", field::NAME, FieldValue::Str("Job".into()));
        mirror.set("5", field::COLOR, color_value(Some(egui::Color32::RED)));
        mirror.set("5", field::EPHEMERAL, FieldValue::Bool(true));
        let job = JarDef { name: "Job".into(), color: Some(egui::Color32::RED), ephemeral: true, is_default: false };
        assert_eq!(mirror.jar_def("5"), job);
    }

    #[test]
    fn pinned_reads_a_bool_and_defaults_to_false() {
        let mut mirror = Mirror::default();
        assert!(!mirror.pinned("7"));
        mirror.set("7", field::PINNED, FieldValue::Bool(true));
        assert!(mirror.pinned("7"));
    }

    /// G2: every field a tab carries (`Kind::Tab.fields()`) survives `carried` → `seed` onto a fresh entry. A tab
    /// field added to `TYPES` but not to `Carried` fails here, rather than going unseeded on a new spawn's `Create`.
    #[test]
    fn carried_holds_every_tab_field() {
        use crate::store::model::{ActorId, FieldType};
        let sample = |ty: FieldType| match ty {
            FieldType::Pos => FieldValue::Pos(1.5, -2.0),
            FieldType::Color => FieldValue::Color(Some(0x123456)),
            FieldType::Id => FieldValue::Id(Some(NodeId { actor: ActorId(1), counter: 2 })),
            FieldType::Str => FieldValue::Str("s".into()),
            FieldType::Bool => FieldValue::Bool(true),
            FieldType::ViewCode => FieldValue::ViewCode(3),
        };
        let fields: Vec<u32> = Kind::Tab.fields().iter().copied().filter(|&no| no != field::PARENT).collect();
        assert!(!fields.is_empty());
        let mut a = Mirror::default();
        for &no in &fields {
            a.set("a", no, sample(field_def(no).unwrap().ty));
        }
        a.set_parent("a", Some("p".into()));
        a.set_group("a", Some("g".into()));
        let mut b = Mirror::default();
        b.seed("b", &a.carried("a"));
        for &no in &fields {
            assert_eq!(b.get("b", no), a.get("a", no), "field {no} is not carried");
        }
        assert_eq!((b.parent("b"), b.group("b")), (Some("p"), Some("g")));
    }
}
