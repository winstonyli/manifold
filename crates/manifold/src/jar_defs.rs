//! Named cookie jars as synced definitions (`App::jar_defs`, spec "Data model"). A jar is a `Kind::Jar` store node
//! but never a graph node: `session_sync` bridges these to the document in `shared_nodes`, `materialize` and
//! `remove_remotely_deleted`.

use crate::jar::{Jar, JarChoice, JarKey};
use crate::node::NodePayload;
use crate::process_manager::EngineKind;
use crate::session_sync::{color_of, Fields, Shared};
use crate::store::model::{field, FieldValue, Kind, NodeId};
use crate::theme::TAG_COLOR_PALETTE;
use crate::{App, JarDef};
use std::collections::{BTreeMap, BTreeSet};

/// What a tab's card and Tab view show of its jar (spec "UI", chip).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JarChip {
    /// The jar's label (`App::jar_label`), shown on the chip and in Tab view's hover text.
    pub(crate) name: JarLabel,
    pub(crate) color: egui::Color32,
    /// Unset, and the nearest group default (`inherited_jar`) is this jar: the dot is outlined. An unset tab whose
    /// group default was cleared or is another jar (spec: "Clear default" leaves existing tabs alone) follows nothing,
    /// so its dot is plain.
    pub(crate) following: bool,
    /// Names a jar that no longer exists; it uses the default jar. The dot takes the warning colour.
    pub(crate) missing: bool,
    /// The live tab still runs on the jar it spawned with (`App::will_switch`): the chip shows that jar's dot
    /// (`was_color`), an arrow, then this one.
    pub(crate) will_switch: bool,
    /// "Switch Jar Now" would act (`App::can_switch_jar_now`): not while a sleep, swap or earlier switch is on its
    /// way. Only then do the hover texts offer it and Tab view's dot take a click.
    pub(crate) can_switch_now: bool,
    pub(crate) was_color: egui::Color32,
    /// A Servo or WebKit tab on a named jar: its engine ignores jars (spec "Hosts").
    pub(crate) engine_ignores: bool,
    /// The name's hidden-character hint (`JarLabel::hidden_hint`, name rules), then the warnings above, one per line,
    /// for the card's hover text; `None` when there are none.
    pub(crate) tooltip: Option<String>,
}

impl JarChip {
    /// Tab view's jar dot hover text in `font`: "Jar: " and the label (its marker styled, `JarLabel::append_to`, in
    /// `marker_color`), whether it follows its group, then the warnings, offering a click rather than a right-click.
    pub(crate) fn hover_job(&self, font: egui::FontId, marker_color: egui::Color32) -> egui::text::LayoutJob {
        let plain = egui::TextFormat::simple(font.clone(), egui::Color32::PLACEHOLDER);
        let mut job = egui::text::LayoutJob::default();
        job.append("Jar: ", 0.0, plain.clone());
        self.name.append_to(&mut job, font, marker_color);
        let mut rest = if self.following { " (follows its group)".to_string() } else { String::new() };
        if let Some(tooltip) = &self.tooltip {
            rest = format!("{rest}\n{}", tooltip.replace(SWITCH_NOW_HINT, "click to switch now"));
        }
        if !rest.is_empty() {
            job.append(&rest, 0.0, plain);
        }
        job
    }
}

/// A Settings jar action waiting for its confirm dialog (`App::build_jar_confirm_popup`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum JarConfirm {
    Clear(JarKey),
    Delete(NodeId),
    /// Ephemeral turned on for a jar with a saved file (`App::request_jar_ephemeral`).
    Ephemeral(NodeId),
}

/// How the card's chip tooltip says to switch now; Tab view's dot, which is clicked instead, swaps it out.
pub(crate) const SWITCH_NOW_HINT: &str = "right-click for Switch Jar Now";

/// The chip's hover text while a live tab waits to switch. `cleared`: its jar's cookies were cleared under it, so its
/// pulls are dropped until it restarts (Task 6 ruling), and `target` is where it restarts. `can_switch_now`: offer
/// "Switch Jar Now" (not while another host change is on its way, when the menu hides the item too).
pub(crate) fn will_switch_tooltip(cleared: bool, target: &str, can_switch_now: bool) -> String {
    let text = if cleared {
        format!(
            "Its jar's cookies were cleared: this tab signs out and restarts on {target} when it is next out of \
             sight, and anything signed in meanwhile is not kept"
        )
    } else {
        format!("Switches to {target} when it is next out of sight")
    };
    if can_switch_now {
        format!("{text}; {SWITCH_NOW_HINT}")
    } else {
        text
    }
}

/// The SHA-256 hex (lower case) of node id `id`'s text, where a jar's marker fragment comes from (`jar_fragment`).
fn id_digest(id: NodeId) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(id.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// What makes two jar names the same name (jar name labels): their clean forms (name rules: trimmed, hidden
/// characters dropped, spaces collapsed), any case.
fn name_key(name: &str) -> String {
    crate::names::clean_name(name).to_lowercase()
}

/// The marker fragment that tells jar `id` apart from the other jars with its name; `None` when no other jar has it
/// (or there is no jar `id`). It is the start of `id_digest(id)`: 4 characters, or as many more as it takes to differ
/// from every same-named jar's. It derives from the id alone, so it never changes for the jar's life, except to grow
/// on the rare collision with a same-named jar that arrives later. Not a hex prefix of the id itself: jars made on one
/// device share the id's actor half and differ only in its decimal counter.
pub(crate) fn jar_fragment(id: NodeId, defs: &BTreeMap<NodeId, JarDef>) -> Option<String> {
    let key = name_key(&defs.get(&id)?.name);
    let twins: Vec<String> =
        defs.iter().filter(|(o, d)| **o != id && name_key(&d.name) == key).map(|(o, _)| id_digest(*o)).collect();
    if twins.is_empty() {
        return None;
    }
    let own = id_digest(id);
    let len = (4..own.len()).find(|&n| twins.iter().all(|t| t[..n] != own[..n])).unwrap_or(own.len());
    Some(own[..len].to_string())
}

/// A jar's label (spec "Names"): its name, and a marker when the name alone may not say which jar it is. Derived each
/// time, never stored or synced. Rich-text sites draw the marker in its own style (`append_to`), so a name typed to
/// look like a marker never looks like one; plain-text sites (tooltips, confirm dialogs) use `Display`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JarLabel {
    pub(crate) name: String,
    pub(crate) marker: Option<JarMarker>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum JarMarker {
    /// Another jar has this name: the start of this one's id digest (`jar_fragment`).
    Fragment(String),
    /// The built-in Default jar, which a user jar may also be called.
    BuiltIn,
}

/// A marker's size, relative to its name's.
pub(crate) const MARKER_SCALE: f32 = 0.8;

impl JarLabel {
    /// The built-in Default jar's label.
    pub(crate) fn built_in() -> Self {
        Self { name: "Default".into(), marker: Some(JarMarker::BuiltIn) }
    }

    /// The marker as rich text shows it, `#3f2a` or `built-in`: without the plain text's " · " or brackets, which the
    /// marker's own style replaces.
    pub(crate) fn marker_text(&self) -> Option<String> {
        self.marker.as_ref().map(|m| match m {
            JarMarker::Fragment(frag) => format!("#{frag}"),
            JarMarker::BuiltIn => "built-in".into(),
        })
    }

    /// The name's format in `font`: the fallback colour (`Color32::PLACEHOLDER`), so a button or label paints it in
    /// its own state's colour; italic for the built-in jar.
    pub(crate) fn name_format(&self, font: egui::FontId) -> egui::TextFormat {
        let italics = self.marker == Some(JarMarker::BuiltIn);
        egui::TextFormat { italics, ..egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER) }
    }

    /// Every marker's format, beside a name `size` points high: monospace, `MARKER_SCALE` of the size, in `color` (the
    /// weak text colour), centred on the name's line.
    pub(crate) fn marker_format(size: f32, color: egui::Color32) -> egui::TextFormat {
        egui::TextFormat {
            valign: egui::Align::Center,
            ..egui::TextFormat::simple(egui::FontId::monospace(size * MARKER_SCALE), color)
        }
    }

    /// The name as the display rule shows it (`names::display_name`): placeholders for hidden characters, the
    /// "Untitled" fallback.
    pub(crate) fn display(&self) -> Vec<crate::names::NameSeg<'_>> {
        crate::names::display_name(&self.name, crate::names::UNTITLED)
    }

    /// What a hover says about the hidden characters in the name (`names::hidden_hint`).
    pub(crate) fn hidden_hint(&self) -> Option<String> {
        crate::names::hidden_hint(&self.display())
    }

    /// Appends the name in `font` (its hidden characters as placeholders, `names::append_name`), then the marker
    /// (after a space, in its own style), both in `marker_color` where styled.
    pub(crate) fn append_to(&self, job: &mut egui::text::LayoutJob, font: egui::FontId, marker_color: egui::Color32) {
        let size = font.size;
        crate::names::append_name(job, &self.display(), &self.name_format(font), marker_color);
        if let Some(marker) = self.marker_text() {
            job.append(&format!(" {marker}"), 0.0, Self::marker_format(size, marker_color));
        }
    }
}

/// A label with no marker, for tests that need one. Not for app code: a stored name's label comes from
/// `named_jar_label`, which knows whether it needs a marker.
#[cfg(test)]
impl From<&str> for JarLabel {
    fn from(name: &str) -> Self {
        Self { name: name.into(), marker: None }
    }
}

/// The plain string: `"Work"`, `"Work · #3f2a"`, or `"Default (built-in)"`, with the name by the display rule
/// (`names::to_plain`: a hidden character reads `<U+200B>`).
impl std::fmt::Display for JarLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = crate::names::to_plain(&self.display());
        match &self.marker {
            None => f.write_str(&name),
            Some(JarMarker::Fragment(frag)) => write!(f, "{name} · #{frag}"),
            Some(JarMarker::BuiltIn) => write!(f, "{name} (built-in)"),
        }
    }
}

/// Jar `id`'s label, what every view shows for it (spec "Names"); `None` when there is no jar `id`. The stored name,
/// with a `Fragment` marker (`jar_fragment`) when another jar has that name.
pub(crate) fn named_jar_label(id: NodeId, defs: &BTreeMap<NodeId, JarDef>) -> Option<JarLabel> {
    let name = defs.get(&id)?.name.clone(); // name rule: drawn through JarLabel::display
    Some(JarLabel { name, marker: jar_fragment(id, defs).map(JarMarker::Fragment) })
}

impl App {
    /// The card's jar chip (spec "UI"): `None` for a tab on the default jar with nothing to warn about.
    pub(crate) fn jar_chip(&self, id: &str) -> Option<JarChip> {
        let choice = self.tab_jar(id)?;
        let (key, missing) = self.resolved_jar(&choice);
        let will_switch = self.will_switch(id);
        let can_switch_now = self.can_switch_jar_now(id);
        let engine = match &self.nodes.get(id)?.payload {
            NodePayload::Tab(t) => t.tab.kind,
            NodePayload::Unloaded(_) => self.mirror.engine(id),
            _ => return None,
        };
        let engine_ignores = !matches!(engine, EngineKind::Webview2 | EngineKind::Cef) && key != JarKey::Default;
        let group = self.nodes.get(id).and(self.mirror.group(id));
        let following = !choice.set && self.inherited_jar(group) == choice.jar;
        if key == JarKey::Default && !missing && !will_switch {
            return None;
        }
        let name = self.jar_label(key);
        let was = self.jar_tabs.get(id).map_or(key, |(k, _)| *k);
        let mut tips = Vec::new();
        if let Some(hint) = name.hidden_hint() {
            tips.push(hint);
        }
        if missing {
            tips.push("This tab's jar was deleted; it uses the default jar".to_string());
        }
        if engine_ignores {
            tips.push("This engine does not use jars yet".to_string());
        }
        if will_switch {
            let cleared = self.jar_reseed.contains(id);
            let target = if cleared && was == key { "the empty jar".to_string() } else { name.to_string() };
            tips.push(will_switch_tooltip(cleared, &target, can_switch_now));
        }
        Some(JarChip {
            color: self.jar_color(key),
            was_color: self.jar_color(was),
            name,
            following,
            missing,
            will_switch,
            can_switch_now,
            engine_ignores,
            tooltip: (!tips.is_empty()).then(|| tips.join("\n")),
        })
    }

    /// The built-in jar's label (`JarLabel::built_in`), or a named jar's (`named_jar_label`; the built-in's once it is
    /// gone, as that is what its tabs use).
    pub(crate) fn jar_label(&self, key: JarKey) -> JarLabel {
        match key {
            JarKey::Named(id) => named_jar_label(id, &self.jar_defs).unwrap_or_else(JarLabel::built_in),
            JarKey::Default => JarLabel::built_in(),
        }
    }

    /// A named jar's colour (the first palette colour if it has none); grey for the default jar.
    pub(crate) fn jar_color(&self, key: JarKey) -> egui::Color32 {
        match key {
            JarKey::Named(id) => {
                self.jar_defs.get(&id).map_or(egui::Color32::GRAY, |d| d.color.unwrap_or(TAG_COLOR_PALETTE[0]))
            }
            JarKey::Default => egui::Color32::GRAY,
        }
    }

    /// Tabs (live, asleep or mid-respawn) that resolve to `key`, for Settings' count and the delete confirm.
    pub(crate) fn jar_tab_count(&self, key: JarKey) -> usize {
        self.linked_ids().filter_map(|t| self.tab_jar(t)).filter(|c| self.resolved_jar(c).0 == key).count()
    }

    /// The confirm dialog's title, detail and button. `None` once a named jar it asks about is gone (deleted on
    /// another device meanwhile): there is nothing left to confirm.
    pub(crate) fn jar_confirm_text(&self, confirm: JarConfirm) -> Option<(String, String, &'static str)> {
        let exists = |key| !matches!(key, JarKey::Named(id) if !self.jar_defs.contains_key(&id));
        match confirm {
            JarConfirm::Clear(key) if exists(key) => Some((
                format!("Clear cookies in {}?", self.jar_label(key)),
                "Tabs using it are signed out. Open ones restart on the empty jar when next out of sight; anything \
                 signed in before then is not kept."
                    .to_string(),
                "Clear",
            )),
            JarConfirm::Delete(id) if exists(JarKey::Named(id)) => {
                let n = self.jar_tab_count(JarKey::Named(id));
                let uses =
                    if n == 1 { "1 tab uses this; it'll".to_string() } else { format!("{n} tabs use this; they'll") };
                Some((
                    format!("Delete jar {}?", self.jar_label(JarKey::Named(id))),
                    format!("{uses} use {}.", JarLabel::built_in()),
                    "Delete",
                ))
            }
            JarConfirm::Ephemeral(id) if self.jar_defs.get(&id).is_some_and(|d| !d.ephemeral) => Some((
                format!("Make {} ephemeral?", self.jar_label(JarKey::Named(id))),
                "Its saved sign-ins are deleted from disk now. Tabs keep them until Manifold quits; after that this \
                 jar starts empty each time."
                    .to_string(),
                "Make Ephemeral",
            )),
            _ => None,
        }
    }

    /// A new named jar under `name`'s clean form (name rules), with the first palette colour no jar shows (one without
    /// a colour shows the first, `jar_color`), so deleting a jar doesn't make the next one repeat a colour; with every
    /// colour taken, they cycle by count.
    /// Saved by the caller's `save_session`.
    pub(crate) fn create_jar(&mut self, name: &str) -> NodeId {
        let id: NodeId = self.alloc_node_id().parse().expect("alloc_node_id returns a store id");
        let shown = |c: &egui::Color32| self.jar_defs.values().any(|d| d.color.unwrap_or(TAG_COLOR_PALETTE[0]) == *c);
        let free = TAG_COLOR_PALETTE.iter().find(|c| !shown(c));
        let color = Some(*free.unwrap_or(&TAG_COLOR_PALETTE[self.jar_defs.len() % TAG_COLOR_PALETTE.len()]));
        self.jar_defs.insert(id, JarDef { name: crate::names::clean_name(name).into_owned(), color, ephemeral: false });
        id
    }

    /// The jar whose label (`named_jar_label`) is `text`, else one whose stored name is: of several with that name, the
    /// lowest node id (`jar_defs` iterates in id order), so the pick is the same every time and on every device. Both
    /// compared exactly, after trimming `text`.
    pub(crate) fn jar_named(&self, text: &str) -> Option<NodeId> {
        let text = text.trim();
        let label = self
            .jar_defs
            .keys()
            .find(|&&id| named_jar_label(id, &self.jar_defs).is_some_and(|l| l.to_string() == text));
        // name rule: SETJAR matches the stored name
        label.or_else(|| self.jar_defs.iter().find(|(_, d)| d.name == text).map(|(id, _)| id)).copied()
    }

    /// `SETJAR`: the focused tab picks the jar `name` names (`jar_named`: a label or a stored name, tried as typed and
    /// then cleaned; "default", any case, for the default jar), made under the clean name first if none has it.
    /// Nothing happens, and no jar is made, when the focused node is not a tab or nothing is left of the name once
    /// cleaned (name rules). Returns whether it acted.
    pub(crate) fn set_focused_tab_jar_named(&mut self, name: &str) -> bool {
        let Some(tab) = self.focused.clone().filter(|t| self.tab_jar(t).is_some()) else { return false };
        let clean = crate::names::clean_name(name).into_owned();
        if clean.is_empty() {
            return false;
        }
        let jar = if clean.eq_ignore_ascii_case("default") {
            None
        } else {
            Some(self.jar_named(name).or_else(|| self.jar_named(&clean)).unwrap_or_else(|| self.create_jar(&clean)))
        };
        self.set_tab_jar(&tab, jar);
        true
    }

    /// Renames jar `id` to `name`'s clean form (name rules); nothing left once cleaned leaves the name as it is.
    pub(crate) fn rename_jar(&mut self, id: NodeId, name: &str) {
        let clean = crate::names::clean_name(name);
        if let (false, Some(def)) = (clean.is_empty(), self.jar_defs.get_mut(&id)) {
            def.name = clean.into_owned();
        }
    }

    /// Turns jar `id`'s ephemeral flag on or off (deviation 10; final review I1), the same for a local toggle (after
    /// `request_jar_ephemeral`'s confirm) and a remote one (`materialize_jar`). The jar is loaded from its file first,
    /// while it still counts as saved, so nothing on disk is dropped unread:
    /// - on: its files are deleted now (`Jar::delete_files`); its cookies stay in memory until quit. Not a locked
    ///   jar's: an unreadable file is never touched, and a locked jar saves nothing anyway.
    /// - off: the loaded jar is saved (a locked one writes nothing).
    pub(crate) fn set_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        if self.jar_defs.get(&id).is_none_or(|d| d.ephemeral == on) {
            return;
        }
        let key = JarKey::Named(id);
        if on {
            self.ensure_jar_loaded(key);
        }
        if let Some(def) = self.jar_defs.get_mut(&id) {
            def.ephemeral = on;
        }
        if !on {
            self.ensure_jar_loaded(key);
            self.save_jar(key);
        } else if let (Some(dir), Some(false)) = (self.jars_dir(), self.jars.get(&key).map(Jar::is_locked)) {
            if let Err(e) = Jar::delete_files(&dir, key) {
                eprintln!("jar: could not delete {}: {e}", key.file_stem());
            }
        }
    }

    /// Settings' Ephemeral checkbox. Turning it on for a jar with a saved file asks first (`JarConfirm::Ephemeral`):
    /// that file, its saved logins, is deleted. Anything else acts at once.
    pub(crate) fn request_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        if on && self.has_saved_file(JarKey::Named(id)) {
            self.offer_jar_confirm(JarConfirm::Ephemeral(id));
        } else {
            self.set_jar_ephemeral(id, on);
        }
    }

    fn has_saved_file(&self, key: JarKey) -> bool {
        self.jars_dir().is_some_and(|dir| Jar::path(&dir, key).exists())
    }

    /// The user deleted jar `id`: tabs (a respawning one too, `linked_ids`) and groups that named it fall back to the
    /// default jar (deviation 9), then it is forgotten and the live tabs that used it switch. Its document node is
    /// deleted by the next save (`diff`: live no longer has it). A new spawn in flight is not reached: it has no
    /// mirror entry until it lands (`tab_jar`), and its landing clears the jar instead (`seed_landing`).
    pub(crate) fn delete_jar(&mut self, id: NodeId) {
        let tabs: Vec<String> = self
            .linked_ids()
            .filter(|t| self.tab_jar(t).is_some_and(|c| c.jar == Some(id)))
            .map(str::to_owned)
            .collect();
        for t in tabs {
            self.edit_jar(&t, JarChoice::default());
        }
        let groups: Vec<String> = self
            .nodes
            .iter()
            .filter(|(g, node)| node.as_group().is_some() && self.mirror.default_jar(g) == Some(id))
            .map(|(g, _)| g.clone())
            .collect();
        for g in groups {
            self.edit_default_jar(&g, None);
        }
        self.forget_jar(id);
        self.sweep_jar_switches();
    }

    /// Drops jar `id` (a local or remote delete): its definition, its cookies and its files, unreadable copies and
    /// crashed saves' temp files included (`Jar::delete_files`). A tab still running on it has its pulls dropped
    /// (`apply_pulled_cookies`) and now resolves to the default jar, so it switches: the caller sweeps (`delete_jar`,
    /// or `apply_remote` once the whole remote batch is in). Its `last_shared` entry stays: after a local delete it
    /// is what keeps a remote edit from recreating the jar before the save (`materialize_jar`).
    pub(crate) fn forget_jar(&mut self, id: NodeId) {
        self.jar_defs.remove(&id);
        let key = JarKey::Named(id);
        self.jars.remove(&key);
        if let Ok(mut snapshots) = self.jar_snapshots.lock() {
            snapshots.remove(&key);
        }
        if let Some(dir) = self.jars_dir() {
            if let Err(e) = Jar::delete_files(&dir, key) {
                eprintln!("jar: could not delete {}: {e}", key.file_stem());
            }
        }
    }

    /// Startup, after `restore_session` (final review I2b): a jar deleted on another device while this one was not
    /// running never reaches `forget_jar` here, so its files (`jars\<id>.json`, unreadable copies, crashed saves'
    /// temp files) are deleted now: the file goes with the delete. Kept: every jar in `jar_defs` or live in the
    /// document. Skipped when the document may not name every jar: no journal, a repaired one (`store_recovered`,
    /// which includes a lost snapshot rebuilt as a new document), one a newer build wrote (read-only), or one with
    /// no live node at all (a new or replaced session file).
    pub(crate) fn prune_jar_files(&self) {
        let (Some(journal), Some(dir)) = (&self.journal, self.jars_dir()) else { return };
        let doc = journal.doc();
        if !self.may_prune_content() || journal.read_only() || doc.live().next().is_none() {
            return;
        }
        let in_doc = doc.live().filter(|(_, n)| n.kind() == Kind::Jar).map(|(id, _)| id);
        let keep: BTreeSet<NodeId> = self.jar_defs.keys().copied().chain(in_doc).collect();
        crate::jar::prune_orphans(&dir, &keep);
    }

    /// `profile::jars_dir()`, or a test's temp dir. Never the real profile under test (`profile::dir()` is None).
    pub(crate) fn jars_dir(&self) -> Option<std::path::PathBuf> {
        #[cfg(test)]
        if let Some(dir) = &self.jars_dir_override {
            return Some(dir.clone());
        }
        crate::profile::jars_dir()
    }

    /// The only place a `jar` field becomes a key (spec "Resolution"). `true` when it names a jar that no longer
    /// exists (deleted, or not synced yet): the tab uses the default jar and its chip warns.
    pub(crate) fn resolved_jar(&self, choice: &JarChoice) -> (JarKey, bool) {
        match choice.jar {
            None => (JarKey::Default, false),
            Some(id) if self.jar_defs.contains_key(&id) => (JarKey::Named(id), false),
            Some(_) => (JarKey::Default, true),
        }
    }

    /// The nearest default jar walking up from `group` (spec "Inheritance").
    pub(crate) fn inherited_jar(&self, group: Option<&str>) -> Option<NodeId> {
        let mut at = group.map(str::to_string);
        for _ in 0..=self.nodes.len() {
            let node = self.nodes.get(at.as_deref()?)?;
            if let Some(jar) = node.as_group().and(self.mirror.default_jar(at.as_deref()?)) {
                return Some(jar);
            }
            at = at.as_deref().and_then(|a| self.mirror.group(a)).map(str::to_owned);
        }
        None // a group cycle (never built, but the loop must end)
    }

    /// What a new tab in `group` starts with: the inherited jar, unset, so a later cascade or move updates it.
    pub(crate) fn new_tab_jar(&self, group: Option<&str>) -> JarChoice {
        JarChoice { jar: self.inherited_jar(group), set: false }
    }

    /// Tab `id`'s jar choice (`Mirror::jar_choice`), live, unloaded or mid-respawn (`linked_ids`); `None` for any
    /// other node, and for a new spawn, which has no mirror entry until it lands.
    pub(crate) fn tab_jar(&self, id: &str) -> Option<JarChoice> {
        let is_tab = match self.nodes.get(id) {
            Some(node) => node.payload.saved_tab().is_some(),
            None => self.is_linked(id), // only a tab respawns
        };
        is_tab.then(|| self.mirror.jar_choice(id))
    }

    /// The user picked a jar for tab `id` (`None`: the default jar). Cascades and moves leave it alone from now on.
    pub(crate) fn set_tab_jar(&mut self, id: &str, jar: Option<NodeId>) {
        if self.tab_jar(id).is_some() {
            self.edit_jar(id, JarChoice { jar, set: true });
        }
        self.sweep_jar_switches();
    }

    /// "Follow group's jar": clears the pick and re-snapshots from where the tab sits.
    pub(crate) fn follow_group_jar(&mut self, id: &str) {
        self.resnapshot(id);
        self.sweep_jar_switches();
    }

    /// `follow_group_jar` without the sweep, for `refollow`'s loop.
    fn resnapshot(&mut self, id: &str) {
        if self.tab_jar(id).is_none() {
            return;
        }
        let group = self.mirror.group(id).map(str::to_owned);
        self.edit_jar(id, self.new_tab_jar(group.as_deref()));
    }

    /// Re-snapshots every unset tab at `id` (a tab, or each tab in a group's subtree) from its new place, a tab
    /// mid-respawn included (`linked_ids`; 2b-3 Ruling 9). Called after a local move between groups (deviation 12); a
    /// remote move's jar edits arrive with it.
    pub(crate) fn refollow(&mut self, id: &str) {
        let tabs: Vec<String> = self
            .linked_ids()
            .filter(|t| self.tab_jar(t).is_some_and(|c| !c.set) && (*t == id || self.is_inside(t, id)))
            .map(str::to_owned)
            .collect();
        for t in tabs {
            self.resnapshot(&t);
        }
        self.sweep_jar_switches();
    }

    /// The group `at` is in, when `at` is one of `linked_ids` (the mirror may still hold a removed node's entry).
    fn linked_group(&self, at: &str) -> Option<String> {
        self.mirror.group(at).filter(|_| self.is_linked(at)).map(str::to_owned)
    }

    /// Whether `at` sits somewhere inside group `group`.
    fn is_inside(&self, at: &str, group: &str) -> bool {
        let mut cur = self.linked_group(at);
        for _ in 0..=self.nodes.len() {
            match cur {
                Some(g) if g == group => return true,
                Some(g) => cur = self.nodes.get(&g).and(self.mirror.group(&g)).map(str::to_owned),
                None => return false,
            }
        }
        false
    }

    /// Whether `group` governs tab-or-group `at`'s jar: `group` is `at`'s group or an ancestor, and no group between
    /// them has a default of its own (spec: "nested groups with no default of their own included").
    fn governed_by(&self, at: &str, group: &str) -> bool {
        let mut cur = self.linked_group(at);
        for _ in 0..=self.nodes.len() {
            let Some(g) = cur else { return false };
            if g == group {
                return true;
            }
            let node = self.nodes.get(&g);
            if node.and_then(|n| n.as_group()).is_some() && self.mirror.default_jar(&g).is_some() {
                return false; // a nearer default wins
            }
            cur = node.and(self.mirror.group(&g)).map(str::to_owned);
        }
        false
    }

    /// The unset tabs a change of `group`'s default to `jar` would rewrite, a tab mid-respawn included
    /// (`linked_ids`).
    fn cascade_targets(&self, group: &str, jar: NodeId) -> Vec<String> {
        self.linked_ids()
            .filter(|t| self.tab_jar(t).is_some_and(|c| !c.set && c.jar != Some(jar)))
            .filter(|t| self.governed_by(t, group))
            .map(str::to_owned)
            .collect()
    }

    /// How many tabs `set_group_default_jar(group, jar)` would change, shown on the group menu's entry for `jar`
    /// (spec "UI": no confirm dialog).
    pub(crate) fn cascade_count(&self, group: &str, jar: NodeId) -> usize {
        self.cascade_targets(group, jar).len()
    }

    /// Writes `group`'s default, then the new jar into every unset tab it governs (spec "Changing a group's
    /// `default_jar`"). The default goes through `edit_default_jar`, the tabs through `edit_jar`. The next save writes
    /// them. Returns how many tabs changed.
    pub(crate) fn set_group_default_jar(&mut self, group: &str, jar: NodeId) -> usize {
        if self.nodes.get(group).and_then(|n| n.as_group()).is_none() {
            return 0;
        }
        self.edit_default_jar(group, Some(jar));
        // After the write, so `governed_by` sees it; harmless, as it stops only at groups strictly between.
        let targets = self.cascade_targets(group, jar);
        for t in &targets {
            self.edit_jar(t, JarChoice { jar: Some(jar), set: false });
        }
        self.sweep_jar_switches();
        targets.len()
    }

    /// "Clear default": new tabs stop inheriting it; existing tabs keep their jar.
    pub(crate) fn clear_group_default_jar(&mut self, group: &str) {
        if self.nodes.get(group).and_then(|n| n.as_group()).is_some() {
            self.edit_default_jar(group, None);
        }
    }

    /// Loads `key`'s jar the first time a tab needs it (spec "Manifold state": named jars load lazily). An ephemeral
    /// jar starts empty and is never read from disk.
    pub(crate) fn ensure_jar_loaded(&mut self, key: JarKey) {
        if self.jars.contains_key(&key) {
            return;
        }
        let jar = match self.jars_dir() {
            Some(dir) if !self.is_ephemeral(key) => Jar::load(&dir, key),
            _ => Jar::default(),
        };
        self.publish_jar(key, jar);
    }

    /// Stores `jar` under `key` and copies it to the spawn threads' snapshot map.
    fn publish_jar(&mut self, key: JarKey, jar: Jar) {
        if let Ok(mut snapshots) = self.jar_snapshots.lock() {
            snapshots.insert(key, jar.clone());
        }
        self.jars.insert(key, jar);
    }

    fn is_ephemeral(&self, key: JarKey) -> bool {
        matches!(key, JarKey::Named(id) if self.jar_defs.get(&id).is_some_and(|d| d.ephemeral))
    }

    /// Writes `key`'s file unless it is ephemeral. A failure is logged (never cookie values) and leaves other jars
    /// alone.
    pub(crate) fn save_jar(&self, key: JarKey) {
        let (Some(dir), Some(jar)) = (self.jars_dir(), self.jars.get(&key)) else { return };
        if self.is_ephemeral(key) {
            return;
        }
        if let Err(e) = jar.save(&dir, key) {
            eprintln!("jar: could not save {}: {e}", key.file_stem());
        }
    }

    /// Empties `key`'s jar and saves it, then switches the live tabs running on it (deviation 14): their engines still
    /// hold the old cookies. A named jar that no longer exists is left alone, so no file is written for it.
    pub(crate) fn clear_jar_cookies(&mut self, key: JarKey) {
        if matches!(key, JarKey::Named(id) if !self.jar_defs.contains_key(&id)) {
            return;
        }
        self.ensure_jar_loaded(key);
        let Some(jar) = self.jars.get(&key) else { return };
        // Cleared in place, so a locked jar stays locked and its unreadable file is still never overwritten.
        let mut jar = jar.clone();
        jar.cookies.clear();
        // Counted with the emptied copy, under one lock: a host still spawning whose seed was read before this is
        // caught when it lands (`note_spawned_jar`), as it has no `jar_tabs` entry to mark below.
        if let Ok(mut snapshots) = self.jar_snapshots.lock() {
            snapshots.insert_cleared(key, jar.clone());
        }
        self.jars.insert(key, jar);
        self.save_jar(key);
        // Their engines still hold the old cookies: they must be replaced although their key is unchanged.
        let running: Vec<String> =
            self.jar_tabs.iter().filter(|(_, (k, _))| *k == key).map(|(id, _)| id.clone()).collect();
        self.jar_reseed.extend(running);
        self.sweep_jar_switches();
    }

    /// A `Kind::Jar` change from the document: applies the fields that differ from what this replica last saw.
    /// A jar this replica knew (`seen`) but no longer has was deleted here, and the next save's diff deletes its node:
    /// another device's edit to it is dropped rather than bringing it back, nameless from a partial change or whole
    /// (final review M1). `forget_jar` leaves the `last_shared` entry that marks it; a remote delete removes that entry
    /// first (`apply_remote`).
    pub(crate) fn materialize_jar(&mut self, id: NodeId, fields: &Fields, seen: &Option<Shared>, now: &Shared) {
        if seen.is_some() && !self.jar_defs.contains_key(&id) {
            return;
        }
        let changed = |no: u32| seen.as_ref().is_none_or(|s| s.fields.get(&no) != now.fields.get(&no));
        let def =
            self.jar_defs.entry(id).or_insert_with(|| JarDef { name: String::new(), color: None, ephemeral: false });
        if changed(field::NAME) {
            def.name = match fields.get(&field::NAME) {
                Some(FieldValue::Str(s)) => s.clone(),
                _ => String::new(),
            };
        }
        if changed(field::COLOR) {
            def.color = fields.get(&field::COLOR).and_then(color_of);
        }
        if changed(field::EPHEMERAL) {
            let on = matches!(fields.get(&field::EPHEMERAL), Some(FieldValue::Bool(true)));
            self.set_jar_ephemeral(id, on);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{id_digest, jar_fragment, named_jar_label, JarConfirm, JarLabel, JarMarker};
    use crate::card::DropEffect;
    use crate::jar::{JarChoice, JarKey};
    use crate::session_sync::tests::{add, sleeping_tab};
    use crate::store::model::NodeId;
    use crate::App;
    use std::collections::{BTreeMap, BTreeSet};

    fn tab_in(app: &mut App, group: Option<&str>) -> String {
        let id = add(app, sleeping_tab("https://a.com/", "A"));
        app.edit_group(&id, group.map(str::to_string));
        id
    }
    fn group_in(app: &mut App, parent: Option<&str>) -> String {
        let id = crate::session_sync::tests::add_group(app, "G");
        app.edit_group(&id, parent.map(str::to_string));
        id
    }
    fn jar_of(app: &App, tab: &str) -> JarChoice {
        app.tab_jar(tab).unwrap()
    }

    #[test]
    fn a_new_tab_snapshots_the_nearest_group_default() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let outer = group_in(&mut app, None);
        let inner = group_in(&mut app, Some(&outer));
        assert_eq!(app.new_tab_jar(Some(&inner)), JarChoice::default(), "no default anywhere: no jar");
        app.set_group_default_jar(&outer, work);
        assert_eq!(app.new_tab_jar(Some(&inner)), JarChoice { jar: Some(work), set: false });
        assert_eq!(app.new_tab_jar(None), JarChoice::default());
    }

    #[test]
    fn a_cascade_reaches_unset_tabs_in_nested_groups_without_their_own_default() {
        let mut app = App::default();
        let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
        let outer = group_in(&mut app, None);
        let plain = group_in(&mut app, Some(&outer)); // no default of its own
        let own = group_in(&mut app, Some(&outer));
        let a = tab_in(&mut app, Some(&outer));
        let b = tab_in(&mut app, Some(&plain));
        let c = tab_in(&mut app, Some(&own));
        // Set after `c` exists, so the cascade gives `c` its group's own jar.
        app.set_group_default_jar(&own, home);
        let picked = tab_in(&mut app, Some(&outer));
        app.set_tab_jar(&picked, None);
        assert_eq!(app.cascade_count(&outer, work), 2);
        assert_eq!(app.set_group_default_jar(&outer, work), 2);
        assert_eq!(jar_of(&app, &a), JarChoice { jar: Some(work), set: false });
        assert_eq!(jar_of(&app, &b), JarChoice { jar: Some(work), set: false });
        assert_eq!(jar_of(&app, &c).jar, Some(home), "a nested group with its own default is skipped");
        assert_eq!(jar_of(&app, &picked), JarChoice { jar: None, set: true }, "a hand-picked tab is skipped");
        app.clear_group_default_jar(&outer);
        assert_eq!(jar_of(&app, &a).jar, Some(work), "clearing the default leaves tabs alone");
        assert_eq!(app.new_tab_jar(Some(&outer)), JarChoice::default(), "but new tabs no longer inherit it");
    }

    #[test]
    fn moving_an_unset_tab_re_snapshots_and_a_set_tab_keeps_its_jar() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let g = group_in(&mut app, None);
        app.set_group_default_jar(&g, work);
        let unset = tab_in(&mut app, None);
        let set = tab_in(&mut app, None);
        app.set_tab_jar(&set, None);
        for t in [&unset, &set] {
            app.edit_group(t, Some(g.clone()));
            app.refollow(t);
        }
        assert_eq!(jar_of(&app, &unset).jar, Some(work));
        assert_eq!(jar_of(&app, &set), JarChoice { jar: None, set: true });
        app.edit_group(&unset, None);
        app.refollow(&unset);
        assert_eq!(jar_of(&app, &unset), JarChoice::default(), "moved out: no default, no jar");
    }

    #[test]
    fn follow_group_jar_clears_set_and_re_snapshots() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let g = group_in(&mut app, None);
        app.set_group_default_jar(&g, work);
        let t = tab_in(&mut app, Some(&g));
        app.set_tab_jar(&t, None);
        app.follow_group_jar(&t);
        assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: false });
    }

    #[test]
    fn a_new_tab_inherits_and_an_opened_tab_copies_its_opener() {
        let mut app = App::default();
        let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
        let g = group_in(&mut app, None);
        app.set_group_default_jar(&g, work);
        app.spawn_new_tab(None, Some(g.clone()), "https://new/");
        let (spawn, _) = app.test_spawns.last().unwrap();
        assert_eq!(spawn.jar, JarChoice { jar: Some(work), set: false });
        let opener = tab_in(&mut app, Some(&g));
        app.set_tab_jar(&opener, Some(home));
        app.spawn_tab_from(&opener, "https://opened/");
        let (spawn, _) = app.test_spawns.last().unwrap();
        assert_eq!(spawn.jar, JarChoice { jar: Some(home), set: true }, "deviation 3");
        assert_eq!(spawn.parent.as_deref(), Some(opener.as_str()));
        assert_eq!(spawn.group.as_deref(), Some(g.as_str()));
        app.duplicate_tab(&opener);
        let (spawn, _) = app.test_spawns.last().unwrap();
        assert_eq!(spawn.jar, JarChoice { jar: Some(home), set: true }, "a duplicate copies too");
        app.spawn_tab_at("https://dropped/", egui::Vec2::ZERO, Some(g.clone()), None);
        let (spawn, _) = app.test_spawns.last().unwrap();
        assert_eq!(spawn.jar, JarChoice { jar: Some(work), set: false }, "a dropped page inherits");
    }

    /// A merge re-snapshots only the members that moved: a tab already in the target keeps its jar, even one that
    /// differs from what it would inherit now (spec: "Clear default" leaves existing tabs alone).
    #[test]
    fn a_merge_leaves_the_targets_own_tabs_alone() {
        let mut app = App::default();
        let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
        let h = group_in(&mut app, None);
        let stayed = tab_in(&mut app, Some(&h));
        app.set_group_default_jar(&h, home);
        app.clear_group_default_jar(&h);
        let w = group_in(&mut app, None);
        let moved = tab_in(&mut app, Some(&w));
        app.set_group_default_jar(&w, work);
        app.apply_drop(&w, Some((DropEffect::Merge, Some(h.clone()))));
        assert_eq!(app.mirror.group(&moved), Some(h.as_str()));
        assert_eq!(jar_of(&app, &moved), JarChoice::default(), "a moved member re-snapshots: `h` has no default");
        assert_eq!(jar_of(&app, &stayed), JarChoice { jar: Some(home), set: false }, "a tab that never moved");
    }

    /// The real move paths, not `refollow` called by hand: each re-snapshots unset tabs and leaves set ones.
    #[test]
    fn every_move_between_groups_re_snapshots_unset_tabs() {
        let mut app = App::default();
        let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
        let w = group_in(&mut app, None);
        app.set_group_default_jar(&w, work);
        let h = group_in(&mut app, None);
        app.set_group_default_jar(&h, home);
        let unset = tab_in(&mut app, None);
        let set = tab_in(&mut app, None);
        app.set_tab_jar(&set, None);
        let unset_jar = |app: &App| jar_of(app, &unset);

        // Join: dropped onto group card `w`.
        app.apply_drop(&unset, Some((DropEffect::Join, Some(w.clone()))));
        app.apply_drop(&set, Some((DropEffect::Join, Some(w.clone()))));
        assert_eq!(unset_jar(&app), JarChoice { jar: Some(work), set: false }, "join");
        assert_eq!(jar_of(&app, &set), JarChoice { jar: None, set: true }, "join leaves a set tab");

        // Eject: out of `w` to the top level, where nothing has a default.
        app.eject(&unset);
        assert_eq!(unset_jar(&app), JarChoice::default(), "eject");

        // Merge: `w` (holding the tab again) dropped onto `h`; its members now take `h`'s default.
        app.apply_drop(&unset, Some((DropEffect::Join, Some(w.clone()))));
        app.apply_drop(&w, Some((DropEffect::Merge, Some(h.clone()))));
        assert_eq!(app.mirror.group(&unset), Some(h.as_str()));
        assert_eq!(unset_jar(&app), JarChoice { jar: Some(home), set: false }, "merge");
        assert_eq!(jar_of(&app, &set), JarChoice { jar: None, set: true }, "merge leaves a set tab");

        // Dissolve: `inner` (no default) inside `h` is dissolved into `h`, then `h` itself to the top level.
        let inner = group_in(&mut app, Some(&h));
        app.edit_group(&unset, Some(inner.clone()));
        app.edit_jar(&unset, JarChoice { jar: Some(work), set: false }); // stale, so the dissolve must fix it
        app.dissolve_group(&inner);
        assert_eq!(unset_jar(&app), JarChoice { jar: Some(home), set: false }, "dissolve into a defaulted group");
        app.dissolve_group(&h);
        assert_eq!(unset_jar(&app), JarChoice::default(), "dissolve to the top level");

        // GroupWith: viewing group `level` (default Work), the tab, moved in by hand, is dropped onto `other`. The
        // new group has no default of its own, so the tab takes `level`'s.
        let level = group_in(&mut app, None);
        app.set_group_default_jar(&level, work);
        let other = tab_in(&mut app, Some(&level));
        app.edit_group(&unset, Some(level.clone()));
        app.current_group = Some(level.clone());
        app.apply_drop(&unset, Some((DropEffect::GroupWith, Some(other.clone()))));
        let made = app.mirror.group(&unset).unwrap().to_owned();
        assert_eq!(app.mirror.group(&made), Some(level.as_str()), "the new group sits inside `level`");
        assert_eq!(unset_jar(&app), JarChoice { jar: Some(work), set: false }, "group with");
    }

    #[test]
    fn the_chip_shows_only_non_default_or_warning_states() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let t = tab_in(&mut app, None);
        assert!(app.jar_chip(&t).is_none(), "default jar: no chip");
        app.set_tab_jar(&t, Some(work));
        let chip = app.jar_chip(&t).unwrap();
        assert_eq!((chip.name.to_string().as_str(), chip.following, chip.missing), ("Work", false, false));
        assert!(chip.engine_ignores, "`sleeping_tab` is a Servo tab");
        assert_eq!(chip.tooltip.as_deref(), Some("This engine does not use jars yet"));
        app.follow_group_jar(&t); // no group default: back to the default jar
        assert!(app.jar_chip(&t).is_none());
        app.set_tab_jar(&t, Some(work));
        app.jar_defs.remove(&work); // a remote delete arrives before the tab's own field edit
        let chip = app.jar_chip(&t).unwrap();
        assert!(chip.missing);
        assert_eq!(chip.tooltip.as_deref(), Some("This tab's jar was deleted; it uses the default jar"));
        assert!(app.jar_chip("nobody").is_none());
    }

    /// Fix batch A2: a new jar takes the first palette colour no jar uses, so a delete doesn't make the next jar repeat
    /// one; with every colour taken it still gets one.
    #[test]
    fn a_new_jar_takes_a_colour_no_jar_uses() {
        use crate::theme::TAG_COLOR_PALETTE;
        let mut app = App::default();
        let colour = |app: &App, id| app.jar_defs[&id].color.unwrap();
        let ids: Vec<_> = ["A", "B", "C"].iter().map(|n| app.create_jar(n)).collect();
        assert_eq!(ids.iter().map(|&id| colour(&app, id)).collect::<Vec<_>>(), TAG_COLOR_PALETTE[..3]);
        app.delete_jar(ids[1]);
        let d = app.create_jar("D");
        assert_eq!(colour(&app, d), TAG_COLOR_PALETTE[1], "the freed colour");
        let distinct = |app: &App| app.jar_defs.values().map(|j| j.color.unwrap().to_array()).collect::<BTreeSet<_>>();
        assert_eq!(distinct(&app).len(), app.jar_defs.len(), "no colour repeats");
        while app.jar_defs.len() < TAG_COLOR_PALETTE.len() {
            app.create_jar("More");
        }
        assert_eq!(distinct(&app).len(), TAG_COLOR_PALETTE.len(), "every palette colour once");
        let extra = app.create_jar("Extra");
        assert!(TAG_COLOR_PALETTE.contains(&colour(&app, extra)), "all taken: still a palette colour");
    }

    /// Fix batch A4: the dot is outlined ("following") only while the tab's nearest group default is its jar. After
    /// "Clear Default Jar" the tab keeps its jar, unset, but follows nothing, so the dot is plain.
    #[test]
    fn the_chip_follows_only_while_a_group_default_names_its_jar() {
        let mut app = App::default();
        let (work, home) = (app.create_jar("Work"), app.create_jar("Home"));
        let g = group_in(&mut app, None);
        app.set_group_default_jar(&g, work);
        let t = tab_in(&mut app, Some(&g));
        app.refollow(&t);
        assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: false });
        assert!(app.jar_chip(&t).unwrap().following, "follows its group's Work");
        app.clear_group_default_jar(&g);
        assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: false }, "stored values unchanged");
        assert!(!app.jar_chip(&t).unwrap().following, "no group default: a plain dot");
        app.mirror.set(&g, crate::store::model::field::DEFAULT_JAR, crate::store::model::FieldValue::Id(Some(home))); // remote, uncascaded
        assert!(!app.jar_chip(&t).unwrap().following, "the group's default is another jar");
        let outer = group_in(&mut app, None);
        app.mirror.set(&g, crate::store::model::field::DEFAULT_JAR, crate::store::model::FieldValue::Id(None));
        app.edit_group(&g, Some(outer.clone()));
        app.mirror.set(
            &outer,
            crate::store::model::field::DEFAULT_JAR,
            crate::store::model::FieldValue::Id(Some(work)),
        );
        assert!(app.jar_chip(&t).unwrap().following, "an outer group's default counts when it is the nearest");
    }

    /// `SETJAR` (Task 8 review): makes a jar only for a focused tab, reuses one by name, and "default" picks Default.
    #[test]
    fn setjar_acts_only_on_a_focused_tab() {
        let mut app = App::default();
        let g = group_in(&mut app, None);
        app.focused = Some(g);
        assert!(!app.set_focused_tab_jar_named("Stray"));
        assert!(app.jar_defs.is_empty(), "a focused group makes no jar");
        app.focused = None;
        assert!(!app.set_focused_tab_jar_named("Stray"));
        assert!(app.jar_defs.is_empty());
        let t = tab_in(&mut app, None);
        app.focused = Some(t.clone());
        assert!(app.set_focused_tab_jar_named("Work"));
        let work = app.jar_named("Work").expect("made for the tab");
        assert_eq!(jar_of(&app, &t), JarChoice { jar: Some(work), set: true });
        assert!(app.set_focused_tab_jar_named("Work"));
        assert_eq!(app.jar_defs.len(), 1, "an existing name is reused");
        assert!(app.set_focused_tab_jar_named("DEFAULT"));
        assert_eq!(jar_of(&app, &t).jar, None);
    }

    /// Jar name labels: `SETJAR` takes a label first, then a stored name; a stored name several jars share picks the
    /// lowest node id, and no jar is made.
    #[test]
    fn setjar_takes_a_label_or_an_ambiguous_stored_name() {
        let mut app = App::default();
        let (w1, w2, home) = (app.create_jar("Work"), app.create_jar("Work"), app.create_jar("Home"));
        assert!(w1 < w2, "made in order");
        let t = tab_in(&mut app, None);
        app.focused = Some(t.clone());
        let l2 = app.jar_label(JarKey::Named(w2)).to_string();
        assert_eq!(app.jar_named(&l2), Some(w2));
        assert!(app.set_focused_tab_jar_named(&l2));
        assert_eq!(jar_of(&app, &t).jar, Some(w2), "by label");
        assert!(app.set_focused_tab_jar_named("Work"));
        assert_eq!(jar_of(&app, &t).jar, Some(w1), "an ambiguous stored name: the lowest id");
        assert!(app.set_focused_tab_jar_named("Home"));
        assert_eq!(jar_of(&app, &t).jar, Some(home), "a unique name is its own label");
        assert_eq!(app.jar_defs.len(), 3, "no jar made");
        // A jar literally named like another's label: the label wins, as it is matched first.
        app.create_jar(&l2);
        assert_eq!(app.jar_named(&l2), Some(w2));
    }

    #[test]
    fn the_will_switch_tooltip_says_where_and_what_a_clear_loses() {
        let tip = |cleared, target: &str| super::will_switch_tooltip(cleared, target, true);
        assert_eq!(tip(false, "Work"), "Switches to Work when it is next out of sight; right-click for Switch Jar Now");
        assert_eq!(
            super::will_switch_tooltip(false, "Work", false),
            "Switches to Work when it is next out of sight",
            "no Switch Jar Now hint while a sleep, swap or earlier switch is on its way"
        );
        assert!(!super::will_switch_tooltip(true, "the empty jar", false).contains("Switch Jar Now"));
        let cleared = tip(true, "the empty jar");
        assert!(cleared.contains("signs out and restarts on the empty jar"), "{cleared}");
        assert!(cleared.contains("anything signed in meanwhile is not kept"), "{cleared}");
    }

    #[test]
    fn jar_tab_count_counts_live_and_sleeping_tabs_by_resolved_jar() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let (a, b) = (tab_in(&mut app, None), tab_in(&mut app, None));
        group_in(&mut app, None);
        app.set_tab_jar(&b, Some(work));
        assert_eq!((app.jar_tab_count(JarKey::Default), app.jar_tab_count(JarKey::Named(work))), (1, 1));
        app.set_tab_jar(&a, Some(work));
        assert_eq!((app.jar_tab_count(JarKey::Default), app.jar_tab_count(JarKey::Named(work))), (0, 2));
        app.jar_defs.remove(&work);
        assert_eq!(app.jar_tab_count(JarKey::Default), 2, "a missing jar's tabs use the default jar");
    }

    #[test]
    fn the_confirm_names_the_jar_and_what_it_costs() {
        let mut app = App::default();
        let work = app.create_jar("Work");
        let t = tab_in(&mut app, None);
        app.set_tab_jar(&t, Some(work));
        let (title, detail, button) = app.jar_confirm_text(JarConfirm::Clear(JarKey::Named(work))).unwrap();
        assert_eq!((title.as_str(), button), ("Clear cookies in Work?", "Clear"));
        assert!(detail.starts_with("Tabs using it are signed out."), "{detail}");
        assert!(detail.contains("restart on the empty jar") && detail.contains("not kept"), "{detail}");
        let (title, _, _) = app.jar_confirm_text(JarConfirm::Clear(JarKey::Default)).unwrap();
        assert_eq!(title, "Clear cookies in Default (built-in)?");
        let (title, detail, button) = app.jar_confirm_text(JarConfirm::Delete(work)).unwrap();
        assert_eq!(
            (title.as_str(), detail.as_str(), button),
            ("Delete jar Work?", "1 tab uses this; it'll use Default (built-in).", "Delete")
        );
        tab_in(&mut app, None);
        let other = tab_in(&mut app, None);
        app.set_tab_jar(&other, Some(work));
        assert_eq!(
            app.jar_confirm_text(JarConfirm::Delete(work)).unwrap().1,
            "2 tabs use this; they'll use Default (built-in)."
        );
        app.jar_defs.remove(&work);
        assert!(
            app.jar_confirm_text(JarConfirm::Delete(work)).is_none(),
            "deleted elsewhere meanwhile: nothing to ask"
        );
        assert!(app.jar_confirm_text(JarConfirm::Clear(JarKey::Named(work))).is_none());
    }

    /// A jar def named `name`, for tests that need chosen ids.
    fn def(name: &str) -> crate::JarDef {
        crate::JarDef { name: name.into(), color: None, ephemeral: false }
    }

    fn id(counter: u64) -> NodeId {
        NodeId { actor: crate::store::model::ActorId(0xabcd), counter }
    }

    /// Jar name labels: a name no other jar has (trimmed, any case) is shown as stored; a shared one gets a marker.
    #[test]
    fn a_label_is_the_name_unless_another_jar_shares_it() {
        let mut defs = BTreeMap::new();
        defs.insert(id(1), def("Work"));
        defs.insert(id(2), def("Home"));
        let plain = |n, defs: &BTreeMap<NodeId, crate::JarDef>| named_jar_label(id(n), defs).map(|l| l.to_string());
        assert_eq!(plain(1, &defs).as_deref(), Some("Work"));
        assert_eq!(jar_fragment(id(1), &defs), None);
        assert_eq!(named_jar_label(id(9), &defs), None, "no such jar");
        defs.insert(id(3), def(" work "));
        defs.insert(id(4), def("WORK"));
        let labels: Vec<String> = [1, 3, 4].map(|n| plain(n, &defs).unwrap()).to_vec();
        // The plain label shows the name by the display rule (trimmed), so " work " reads "work · #frag".
        for (n, (label, name)) in labels.iter().zip(["Work", "work", "WORK"]).enumerate() {
            let frag = label.strip_prefix(&format!("{name} · #")).unwrap_or_else(|| panic!("{n}: {label}"));
            assert_eq!(frag.len(), 4, "{label}");
            assert!(frag.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{label}");
        }
        assert_eq!(labels.iter().collect::<BTreeSet<_>>().len(), 3, "every clashing label differs: {labels:?}");
        assert_eq!(plain(2, &defs).as_deref(), Some("Home"), "an unrelated jar stays plain");
    }

    /// Name rules: clash detection compares clean names, so a hidden character cannot make two same-looking jars
    /// look distinct; the plain label shows it as `<U+200B>`.
    #[test]
    fn a_hidden_character_does_not_make_a_name_distinct() {
        let mut defs = BTreeMap::new();
        defs.insert(id(1), def("Work"));
        defs.insert(id(2), def("Wo\u{200B}rk"));
        defs.insert(id(3), def("Work\u{00A0}"));
        for n in [1, 2, 3] {
            assert!(jar_fragment(id(n), &defs).is_some(), "{n} clashes");
        }
        let label = named_jar_label(id(2), &defs).unwrap().to_string();
        assert!(label.starts_with("Wo<U+200B>rk · #"), "{label}");
        assert_eq!(JarLabel::built_in().to_string(), "Default (built-in)");
    }

    /// Styled jar label markers: a label is a name and an optional marker, and its plain string is the text every view
    /// showed before the split ("Work", "Work · #frag").
    #[test]
    fn a_label_is_a_name_and_an_optional_marker() {
        let mut defs = BTreeMap::new();
        defs.insert(id(1), def("Work"));
        defs.insert(id(2), def("Home"));
        let unique = named_jar_label(id(1), &defs).unwrap();
        assert_eq!(unique, JarLabel { name: "Work".into(), marker: None });
        assert_eq!(unique.to_string(), "Work");
        defs.insert(id(3), def("Work"));
        let frag = jar_fragment(id(1), &defs).unwrap();
        let clash = named_jar_label(id(1), &defs).unwrap();
        assert_eq!(clash, JarLabel { name: "Work".into(), marker: Some(JarMarker::Fragment(frag.clone())) });
        assert_eq!(clash.to_string(), format!("Work · #{frag}"));
        assert_eq!(JarLabel::built_in(), JarLabel { name: "Default".into(), marker: Some(JarMarker::BuiltIn) });
        assert_eq!(JarLabel::built_in().to_string(), "Default (built-in)");
    }

    /// A user jar named "Default" is shown as stored; the built-in jar (and a deleted jar's tabs, which use it) is
    /// marked, and plain-text sites say "Default (built-in)".
    #[test]
    fn the_built_in_jar_is_marked_apart_from_a_user_jar_named_default() {
        let mut app = App::default();
        let user = app.create_jar("Default");
        assert_eq!(app.jar_label(JarKey::Named(user)), JarLabel { name: "Default".into(), marker: None });
        assert_eq!(app.jar_label(JarKey::Default), JarLabel::built_in());
        assert_eq!(
            app.jar_confirm_text(JarConfirm::Clear(JarKey::Named(user))).unwrap().0,
            "Clear cookies in Default?"
        );
        assert_eq!(
            app.jar_confirm_text(JarConfirm::Clear(JarKey::Default)).unwrap().0,
            "Clear cookies in Default (built-in)?"
        );
        app.jar_defs.remove(&user);
        assert_eq!(app.jar_label(JarKey::Named(user)), JarLabel::built_in(), "gone: its tabs use the built-in jar");
    }

    /// Styled jar label markers: rich text draws a marker after its name in monospace, smaller, in the weak colour and
    /// without the plain text's " · ", so a name typed to look like a marker never looks like one; the built-in jar's
    /// name is italic too. The name keeps the fallback colour, so a button paints it in its own state's colour.
    #[test]
    fn a_marker_is_drawn_smaller_monospace_and_weak_after_its_name() {
        let font = egui::FontId::proportional(14.0);
        let weak = egui::Color32::from_gray(120);
        let marker_format = egui::TextFormat {
            font_id: egui::FontId::monospace(14.0 * super::MARKER_SCALE),
            color: weak,
            valign: egui::Align::Center,
            ..Default::default()
        };
        let parts = |job: &egui::text::LayoutJob| {
            job.sections
                .iter()
                .map(|s| (job.text[s.byte_range.start.0..s.byte_range.end.0].to_string(), s.format.clone()))
                .collect::<Vec<_>>()
        };
        let job = |label: &JarLabel| {
            let mut job = egui::text::LayoutJob::default();
            label.append_to(&mut job, font.clone(), weak);
            job
        };
        let name_format = egui::TextFormat::simple(font.clone(), egui::Color32::PLACEHOLDER);
        assert_eq!(parts(&job(&JarLabel::from("Work"))), [("Work".into(), name_format.clone())]);
        let typed = job(&JarLabel::from("Work · #3f2a"));
        assert_eq!(parts(&typed), [("Work · #3f2a".into(), name_format.clone())], "a typed look-alike is all name");
        let clash = JarLabel { name: "Work".into(), marker: Some(JarMarker::Fragment("3f2a".into())) };
        assert_eq!(clash.marker_text().as_deref(), Some("#3f2a"));
        assert_eq!(
            parts(&job(&clash)),
            [("Work".into(), name_format.clone()), (" #3f2a".into(), marker_format.clone())]
        );
        let built_in = JarLabel::built_in();
        assert_eq!(built_in.marker_text().as_deref(), Some("built-in"));
        let italic = egui::TextFormat { italics: true, ..name_format };
        assert_eq!(parts(&job(&built_in)), [("Default".into(), italic), (" built-in".into(), marker_format)]);
        assert_eq!(JarLabel::from("Default").marker_text(), None);
    }

    /// Tab view's jar dot hover: "Jar: " and the styled label, whether it follows its group, then the chip's warnings
    /// with the click hint in place of the right-click one.
    #[test]
    fn the_tab_view_hover_styles_the_label() {
        let mut app = App::default();
        let (_, w2) = (app.create_jar("Work"), app.create_jar("Work"));
        let t = tab_in(&mut app, None);
        app.set_tab_jar(&t, Some(w2));
        let chip = app.jar_chip(&t).unwrap();
        let font = egui::FontId::proportional(14.0);
        let weak = egui::Color32::from_gray(120);
        let job = chip.hover_job(font.clone(), weak);
        let mut want = egui::text::LayoutJob::default();
        want.append("Jar: ", 0.0, egui::TextFormat::simple(font.clone(), egui::Color32::PLACEHOLDER));
        chip.name.append_to(&mut want, font.clone(), weak);
        want.append(
            "\nThis engine does not use jars yet",
            0.0,
            egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER),
        );
        assert_eq!(job, want);
        let mut chip = chip;
        chip.following = true;
        chip.tooltip = Some(format!("Switches to Work; {}", super::SWITCH_NOW_HINT));
        let text = chip.hover_job(egui::FontId::proportional(14.0), weak).text;
        assert!(text.ends_with(" (follows its group)\nSwitches to Work; click to switch now"), "{text}");
    }

    /// Two ids whose 4-character fragments collide get longer ones, just long enough to differ; a third jar with the
    /// same name whose 4 characters already differ keeps them.
    #[test]
    fn colliding_fragments_lengthen_just_enough() {
        let mut seen = std::collections::HashMap::new();
        let (a, b) = (0..)
            .find_map(|n| seen.insert(id_digest(id(n))[..4].to_string(), id(n)).map(|other| (other, id(n))))
            .unwrap();
        let (da, db) = (id_digest(a), id_digest(b));
        let differ = (0..).find(|&i| da.as_bytes()[i] != db.as_bytes()[i]).unwrap();
        let c = (0..).map(id).find(|&c| c != a && c != b && id_digest(c)[..4] != da[..4]).unwrap();
        let mut defs = BTreeMap::new();
        for n in [a, b, c] {
            defs.insert(n, def("Work"));
        }
        assert_eq!(jar_fragment(a, &defs), Some(da[..=differ].to_string()));
        assert_eq!(jar_fragment(b, &defs), Some(db[..=differ].to_string()));
        assert_ne!(jar_fragment(a, &defs), jar_fragment(b, &defs));
        assert_eq!(jar_fragment(c, &defs), Some(id_digest(c)[..4].to_string()));
    }

    /// A label is derived from the jar's own id: adding or deleting a jar with another name never changes it, and
    /// it is back to the plain name once its twin goes.
    #[test]
    fn a_label_is_stable_while_other_jars_come_and_go() {
        let mut app = App::default();
        let (w1, w2) = (app.create_jar("Work"), app.create_jar("Work"));
        let label = |app: &App, j| app.jar_label(JarKey::Named(j)).to_string();
        let (l1, l2) = (label(&app, w1), label(&app, w2));
        assert_ne!(l1, l2);
        let home = app.create_jar("Home");
        assert_eq!((label(&app, w1), label(&app, w2)), (l1.clone(), l2.clone()), "a third jar added elsewhere");
        app.delete_jar(home);
        assert_eq!((label(&app, w1), label(&app, w2)), (l1.clone(), l2), "and deleted");
        app.delete_jar(w2);
        assert_eq!(label(&app, w1), "Work", "no twin left: plain again");
        let w3 = app.create_jar("work");
        // The same marker when a twin comes back (longer only in the 1-in-65536 case that the two collide).
        assert!(label(&app, w1).starts_with(&l1), "{} vs {l1}", label(&app, w1));
        assert!(label(&app, w3).starts_with("work · #"));
    }

    /// The chip, its will-switch text and the confirm dialogs all name a clashing jar by its label.
    #[test]
    fn the_chip_and_confirms_show_the_label() {
        let mut app = App::default();
        let (w1, w2) = (app.create_jar("Work"), app.create_jar("Work"));
        let l2 = app.jar_label(JarKey::Named(w2));
        assert!(l2.to_string().starts_with("Work · #"), "{l2}");
        let t = tab_in(&mut app, None);
        app.set_tab_jar(&t, Some(w2));
        assert_eq!(app.jar_chip(&t).unwrap().name, l2);
        assert_eq!(
            app.jar_confirm_text(JarConfirm::Clear(JarKey::Named(w2))).unwrap().0,
            format!("Clear cookies in {l2}?")
        );
        assert_eq!(app.jar_confirm_text(JarConfirm::Delete(w2)).unwrap().0, format!("Delete jar {l2}?"));
        assert_eq!(app.jar_confirm_text(JarConfirm::Ephemeral(w2)).unwrap().0, format!("Make {l2} ephemeral?"));
        let l1 = app.jar_label(JarKey::Named(w1));
        assert_eq!(app.jar_confirm_text(JarConfirm::Delete(w1)).unwrap().0, format!("Delete jar {l1}?"));
    }

    /// Task 6 review M3: "Switch Jar Now" waits out an engine swap or a sleep already on its way, since either
    /// replaces the host anyway and a second hold would cancel the first.
    #[test]
    fn a_pending_sleep_or_hold_blocks_switch_jar_now() {
        let mut app = App::default();
        assert!(!app.host_change_pending("t"));
        app.pending_sleeps.insert(
            "t".into(),
            crate::sleep_state::PendingSleep {
                why: crate::sleep_state::SleepWhy::JarSwitch,
                sent: std::time::Instant::now(),
            },
        );
        assert!(app.host_change_pending("t"));
        app.pending_sleeps.clear();
        app.jar_pulls.insert("t".into(), std::time::Instant::now());
        let held = crate::tabs::Held::Snapshot(crate::process_manager::EngineKind::Cef, "http://x/".into());
        assert!(app.hold_for_cookies("t", held).is_none());
        assert!(app.host_change_pending("t"), "a swap or switch parked waiting for its cookies");
    }

    /// Name rules: a jar is made and renamed under the clean name; a rename with nothing left is ignored.
    #[test]
    fn a_jar_is_made_and_renamed_with_the_clean_name() {
        let mut app = App::default();
        let jar = app.create_jar(" Wo\u{200B}rk\n");
        assert_eq!(app.jar_defs[&jar].name, "Work");
        app.rename_jar(jar, "\u{00A0}Home\u{2028}Office");
        assert_eq!(app.jar_defs[&jar].name, "Home Office");
        app.rename_jar(jar, "\u{200B}");
        assert_eq!(app.jar_defs[&jar].name, "Home Office");
    }

    /// Name rules: `SETJAR` with nothing left of the name does nothing; a name that is clean only after cleaning
    /// finds the jar it cleans to rather than making a twin.
    #[test]
    fn setjar_matches_and_makes_by_the_clean_name() {
        let mut app = App::default();
        let t = crate::session_sync::tests::add(&mut app, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        app.focused = Some(t.clone());
        assert!(!app.set_focused_tab_jar_named("\u{200B} "));
        assert!(app.jar_defs.is_empty());
        let work = app.create_jar("Work");
        assert!(app.set_focused_tab_jar_named("Wo\u{200B}rk"));
        assert_eq!(app.jar_defs.len(), 1, "no twin made");
        assert_eq!(app.tab_jar(&t).map(|c| c.jar), Some(Some(work)));
        assert!(app.set_focused_tab_jar_named(" New\u{00A0}\u{00A0}one "));
        assert!(app.jar_defs.values().any(|d| d.name == "New one"));
    }

    /// Name rules: a label draws a hidden character as a placeholder in the marker style, between the plain name
    /// parts and before a marker, and its hint names it.
    #[test]
    fn a_label_shows_a_hidden_character_in_the_marker_style() {
        let label = JarLabel { name: "Wo\u{200B}rk".into(), marker: Some(JarMarker::Fragment("3f2a".into())) };
        let mut job = egui::text::LayoutJob::default();
        label.append_to(&mut job, egui::FontId::proportional(14.0), egui::Color32::GRAY);
        assert_eq!(job.text, "Wo⟦200B⟧rk #3f2a");
        let parts: Vec<(String, bool)> = job
            .sections
            .iter()
            .map(|s| {
                let text = job.text[s.byte_range.start.0..s.byte_range.end.0].to_string();
                (text, s.format.font_id.family == egui::FontFamily::Monospace)
            })
            .collect();
        let want = [("Wo", false), ("⟦200B⟧", true), ("rk", false), (" #3f2a", true)];
        assert_eq!(parts, want.map(|(t, m)| (t.to_string(), m)));
        assert_eq!(label.hidden_hint().as_deref(), Some("Hidden characters: U+200B zero width space"));
        assert_eq!(JarLabel::built_in().hidden_hint(), None);
    }

    /// Name rules: a chip on a jar whose synced name holds a hidden character says so in its hover.
    #[test]
    fn a_jar_chip_hover_names_a_hidden_character() {
        let mut app = App::default();
        let t = crate::session_sync::tests::add(&mut app, crate::session_sync::tests::sleeping_tab("https://a/", "A"));
        let jar = app.create_jar("x");
        app.jar_defs.get_mut(&jar).unwrap().name = "Wo\u{200B}rk".into();
        app.set_tab_jar(&t, Some(jar));
        let tip = app.jar_chip(&t).and_then(|c| c.tooltip).unwrap_or_default();
        assert!(tip.contains("Hidden characters: U+200B zero width space"), "{tip}");
    }
}
