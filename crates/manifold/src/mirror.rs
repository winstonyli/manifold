//! The app's decoded copy of the document's fields that have moved off `GraphNode` and its payloads (node model
//! spec, 2b "Read model"). Every read goes through the accessors here, so making `Doc` the app's own state later
//! changes only where they read.
//!
//! The mirror equals the document except for fields and links with an edit still waiting for a save
//! (`App::pending`, `App::pending_moves`); `App::edit_field`, `App::edit_parent` and `App::edit_group` are the
//! writers, and `App::mirror_remote` applies another replica's change.

use std::collections::HashMap;

use crate::jar::JarChoice;
use crate::json_view::JsonView;
use crate::process_manager::EngineKind;
use crate::session_sync::{color_of, color_value, engine_of, pos_value, Fields};
use crate::store::model::{field, field_def, FieldValue};

/// The fields the mirror holds. `diff` leaves them to `App::pending` on existing nodes. `URL` is mirrored for every
/// kind that has it, but only tabs seed and read it; content kinds read their payload's until 2b-5 (2b-3 Ruling 2).
/// `JAR` and `JAR_SET` are a tab's jar choice (`jar_choice`), written together by `App::edit_jar`. `ENGINE` is a tab's
/// engine as its stored code (`engine`), written by `App::edit_engine`. `POS` is the document pos (`pos`), written by
/// `App::edit_pos`; where a card is drawn is `App::layout`, which the document never reads (2b-4 Ruling 1). `TEXT` is a
/// note's text (`text`), written by `App::edit_text` at a save or blur; the note's payload is its edit buffer. `VIEW`
/// is a Json node's view (`view`, decoded), written by `App::edit_view`. `NAME` is a group's name (`name`), written by
/// `App::edit_name`; a jar's `NAME` is not mirrored (`JarDef`, `materialize_jar`), and `diff` still writes it.
pub(crate) const MIRRORED: &[u32] = &[
    field::POS,
    field::COLOR,
    field::PINNED,
    field::URL,
    field::TEXT,
    field::VIEW,
    field::NAME,
    field::ENGINE,
    field::JAR,
    field::JAR_SET,
];

#[derive(Default)]
pub(crate) struct Mirror {
    nodes: HashMap<String, Fields>,
    links: HashMap<String, Links>,
    /// `field::VIEW` of `nodes`, decoded (`view`); only non-empty views.
    views: HashMap<String, JsonView>,
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
        debug_assert!(MIRRORED.contains(&no), "field {no} is not mirrored");
        if no == field::VIEW {
            let view = match &value {
                FieldValue::Str(text) => JsonView::from_field(text),
                _ => JsonView::default(),
            };
            if view.0.is_empty() {
                self.views.remove(id);
            } else {
                self.views.insert(id.to_owned(), view);
            }
        }
        self.nodes.entry(id.to_owned()).or_default().insert(no, value);
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

    /// Drops the entries `keep` rejects.
    pub(crate) fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.nodes.retain(|id, _| keep(id));
        self.links.retain(|id, _| keep(id));
        self.views.retain(|id, _| keep(id));
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
        match self.nodes.get(id).and_then(|fields| fields.get(&field::TEXT)) {
            Some(FieldValue::Str(text)) => text,
            _ => "",
        }
    }

    /// A Json node's view overrides (`field::VIEW`), decoded: the cache `set` refreshes whenever the mirror's view
    /// changes (2b-4 Ruling 8), so the per-frame readers never parse. `None` when the node has no overrides.
    pub(crate) fn view(&self, id: &str) -> Option<&JsonView> {
        self.views.get(id)
    }

    /// A group's stored name (`field::NAME`, 2b-4 Ruling 9): the document's, or the rename about to be written
    /// (`App::edit_name`). "" when the mirror has none. Raw: drawn only through a name rule (`names.rs`). A jar's
    /// name is its `JarDef`'s, not this.
    pub(crate) fn name(&self, id: &str) -> &str {
        match self.nodes.get(id).and_then(|fields| fields.get(&field::NAME)) {
            Some(FieldValue::Str(name)) => name,
            _ => "",
        }
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
    /// every engine report moves, is `TabPayload::url` / `UnloadedTabPayload::url`. "" when the mirror has none.
    pub(crate) fn url(&self, id: &str) -> String {
        match self.get(id, field::URL) {
            FieldValue::Str(url) => url,
            _ => String::new(),
        }
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

    /// A tab's engine: the one it runs or is switching to (`App::edit_engine`), the document's otherwise. The mirror
    /// holds the stored code as is, so a code from a newer build survives a save; it, and an absent one, read as
    /// Servo (`engine_of`, 2b-3 Ruling 6). `EngineTab::kind` is the engine actually running.
    pub(crate) fn engine(&self, id: &str) -> EngineKind {
        engine_of(self.engine_code(id))
    }

    /// A tab's engine as the stored code, kept as is (0: none).
    pub(crate) fn engine_code(&self, id: &str) -> u64 {
        match self.get(id, field::ENGINE) {
            FieldValue::Engine(code) => code,
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
            url: self.url(id),
            text: self.text(id).to_owned(),
            engine: self.engine_code(id),
            jar: self.jar_choice(id),
        }
    }

    /// `init` for everything a node carries. No pos, an empty `url` or `text`, engine code 0 and the default, unset jar (any
    /// node but a tab) are the fields' absent values, so they are left out.
    pub(crate) fn seed(&mut self, id: &str, carried: &Carried) {
        let pos = carried.pos.map(|pos| (field::POS, pos_value(pos)));
        let url = (!carried.url.is_empty()).then(|| (field::URL, FieldValue::Str(carried.url.clone())));
        let text = (!carried.text.is_empty()).then(|| (field::TEXT, FieldValue::Str(carried.text.clone())));
        let engine = (carried.engine != 0).then_some((field::ENGINE, FieldValue::Engine(carried.engine)));
        let jar = carried.jar.jar.is_some().then_some((field::JAR, FieldValue::Id(carried.jar.jar)));
        let jar_set = carried.jar.set.then_some((field::JAR_SET, FieldValue::Bool(true)));
        let fields = [(field::COLOR, color_value(carried.color)), (field::PINNED, FieldValue::Bool(carried.pinned))];
        self.init(id, fields.into_iter().chain(pos).chain(url).chain(text).chain(engine).chain(jar).chain(jar_set));
        if !self.links.contains_key(id) {
            let links = Links { parent: carried.parent.clone(), group: carried.group.clone() };
            self.links.insert(id.to_owned(), links);
        }
    }
}

/// What a tab carries across a respawn or lands with: the mirror's values for it. `place_node` seeds a new node's
/// links and pos with it too.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Carried {
    /// The document pos (`Mirror::pos`): a new node's placement, which its `Create` carries (2b-4 Ruling 3).
    pub pos: Option<egui::Vec2>,
    pub color: Option<egui::Color32>,
    pub pinned: bool,
    pub parent: Option<String>,
    pub group: Option<String>,
    /// The shared url (`Mirror::url`); "" for a node that isn't a tab.
    pub url: String,
    /// A note's text (`Mirror::text`): what a new note starts with, which its `Create` carries; "" for other kinds.
    pub text: String,
    /// The engine's stored code (`Mirror::engine` decodes it, `session_sync::engine_code` makes one); 0, the absent
    /// value, for a node that isn't a tab.
    pub engine: u64,
    /// The jar choice (`Mirror::jar_choice`); the default for a node that isn't a tab.
    pub jar: JarChoice,
}

#[cfg(test)]
mod tests {
    use super::*;

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
            engine: 99,
            jar: JarChoice {
                jar: Some(crate::store::model::NodeId { actor: crate::store::model::ActorId(1), counter: 2 }),
                set: true,
            },
        };
        mirror.seed("7", &first);
        assert_eq!(mirror.carried("7"), first);
        mirror.seed("7", &Carried { url: "https://b/".into(), ..Carried::default() });
        assert_eq!(mirror.carried("7"), first);
        mirror.seed("8", &Carried { pos: Some(egui::Vec2::ZERO), ..Carried::default() });
        assert_eq!(mirror.pos("8"), None, "pos (0,0) is the absent value (Ruling 5)");
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
    fn an_engine_reads_back_decoded_and_keeps_an_unknown_code() {
        let mut mirror = Mirror::default();
        assert_eq!(mirror.engine("7"), EngineKind::Servo, "absent is Servo");
        mirror.seed("8", &Carried::default());
        assert!(!mirror.nodes["8"].contains_key(&field::ENGINE), "code 0 seeds nothing");
        let cef = crate::session_sync::engine_code(EngineKind::Cef);
        mirror.set("7", field::ENGINE, FieldValue::Engine(cef));
        assert_eq!(mirror.engine("7"), EngineKind::Cef);
        mirror.set("7", field::ENGINE, FieldValue::Engine(99));
        assert_eq!(mirror.engine("7"), EngineKind::Servo, "a newer build's code reads as Servo");
        assert_eq!(mirror.carried("7").engine, 99, "and is carried as is");
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

    #[test]
    fn pinned_reads_a_bool_and_defaults_to_false() {
        let mut mirror = Mirror::default();
        assert!(!mirror.pinned("7"));
        mirror.set("7", field::PINNED, FieldValue::Bool(true));
        assert!(mirror.pinned("7"));
    }
}
