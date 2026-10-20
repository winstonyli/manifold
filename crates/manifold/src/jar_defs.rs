//! Named cookie jars as synced definitions (`App::jar_defs`, spec "Data model"). A jar is a `Kind::Jar` card: a
//! `App::nodes` entry with a unit `Runtime::Jar`. Its fields are mirror state (2b-5 Ruling 7): written through
//! `App::edit_field` here and applied from the document by `materialize` like any card's.

use crate::caps::{Scope, Storage, JAR_EDGE};
use crate::jar::Jar;
use crate::node::Runtime;
use crate::popups::Confirm;
use crate::session_sync::color_value;
use crate::store::model::{field, FieldValue, Kind, NodeId};
use crate::theme::TAG_COLOR_PALETTE;
use crate::App;
use std::collections::BTreeMap;

/// A named jar's synced definition (a `Kind::Jar` card), decoded from its mirrored fields (`Mirror::jar_def`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JarDef {
    pub(crate) name: String,
    pub(crate) color: Option<egui::Color32>,
    pub(crate) ephemeral: bool,
    /// The built-in Default jar (`field::IS_DEFAULT`); exactly one live jar has it once `ensure_default_jar` has run.
    pub(crate) is_default: bool,
}

/// What a tab's card and Tab view show of its jar (spec "UI", chip).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JarChip {
    /// The jar's label (`App::jar_label`), shown on the chip and in Tab view's hover text.
    pub(crate) name: JarLabel,
    pub(crate) color: egui::Color32,
    /// The tab has no jar edge of its own: it inherits (from a group's edge, or the Default), and the dot is outlined.
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
    pub(crate) view_ignores: bool,
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

/// The Default jar's name when it is made, and the name a losing duplicate takes (spec "The Default jar").
const DEFAULT_NAME: &str = "Default";
const OLD_DEFAULT_NAME: &str = "Default (old)";

/// A jar action waiting for its confirm dialog (`App::build_confirm_popup`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JarConfirm {
    Clear(NodeId),
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
        if !self.is_tab(id) {
            return None;
        }
        let (key, missing) = self.effective_jar(id)?;
        let default = self.default_jar();
        let will_switch = self.will_switch(id);
        let can_switch_now = self.can_switch_jar_now(id);
        let view_kind = match &self.nodes.get(id)?.runtime {
            Runtime::Tab(t) => t.tab.kind,
            Runtime::Unloaded(_) => self.mirror.view_kind(id),
            _ => return None,
        };
        let view_ignores = !view_kind.desc().jar && Some(key) != default;
        let following = self.mirror.jar(id).is_none();
        if Some(key) == default && !missing && !will_switch {
            return None;
        }
        let name = self.jar_label(key);
        let was = self.cookies.jar_tabs.get(id).map_or(key, |(k, _)| *k);
        let mut tips = Vec::new();
        if let Some(hint) = name.hidden_hint() {
            tips.push(hint);
        }
        if missing {
            tips.push("This tab's jar is gone or out of reach; it uses the Default jar".to_string());
        }
        if view_ignores {
            tips.push("This view does not use jars yet".to_string());
        }
        if will_switch {
            let cleared = self.cookies.jar_reseed.contains(id);
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
            view_ignores,
            tooltip: (!tips.is_empty()).then(|| tips.join("\n")),
        })
    }

    /// Jar `id`'s label: its stored name, marked `BuiltIn` for the Default and with a fragment when another jar
    /// shares the name (`named_jar_label`). A jar that is gone reads as the Default's label, as that is what its tabs
    /// use.
    pub(crate) fn jar_label(&self, id: NodeId) -> JarLabel {
        let defs = self.jar_defs();
        let shown = if defs.contains_key(&id) { Some(id) } else { self.default_jar() };
        let Some(shown) = shown else {
            return JarLabel { name: DEFAULT_NAME.into(), marker: Some(JarMarker::BuiltIn) };
        };
        let mut label = named_jar_label(shown, &defs).expect("a live jar");
        if defs[&shown].is_default {
            label.marker = Some(JarMarker::BuiltIn);
        }
        label
    }

    /// Jar `id`'s colour (the first palette colour if it has none); grey when it is not a live jar.
    pub(crate) fn jar_color(&self, id: NodeId) -> egui::Color32 {
        self.jar_defs().get(&id).map_or(egui::Color32::GRAY, |d| d.color.unwrap_or(TAG_COLOR_PALETTE[0]))
    }

    /// The confirm dialog's title, detail and button. `None` once a named jar it asks about is gone (deleted on
    /// another device meanwhile): there is nothing left to confirm.
    pub(crate) fn jar_confirm_text(&self, confirm: JarConfirm) -> Option<(String, String, &'static str)> {
        match confirm {
            JarConfirm::Clear(id) if self.jar_exists(id) => Some((
                format!("Clear cookies in {}?", self.jar_label(id)),
                "Tabs using it are signed out. Open ones restart on the empty jar when next out of sight; anything \
                 signed in before then is not kept."
                    .to_string(),
                "Clear",
            )),
            JarConfirm::Ephemeral(id) if self.jar_exists(id) && !self.mirror.jar_def(&id.to_string()).ephemeral => Some((
                format!("Make {} ephemeral?", self.jar_label(id)),
                "Its saved sign-ins are deleted from disk now. Tabs keep them until Manifold quits; after that this \
                 jar starts empty each time."
                    .to_string(),
                "Make Ephemeral",
            )),
            _ => None,
        }
    }

    /// The Default jar: the lowest-id live jar with `IS_DEFAULT` (`ensure_default_jar` leaves exactly one). Cheap
    /// enough per card per frame: the mirror's index holds one id, two for a moment after two replicas' first
    /// launches; shown jars only, a closed or hidden one has left the mirror. Each costs a `to_string` and a hash lookup.
    pub(crate) fn default_jar(&self) -> Option<NodeId> {
        self.mirror.default_jar_ids().find(|&id| self.jar_exists(id))
    }

    /// Whether node `id` is the Default jar.
    pub(crate) fn is_default_jar(&self, id: &str) -> bool {
        id.parse().ok().is_some_and(|id| self.default_jar() == Some(id))
    }

    /// The startup and sync invariant (spec "The Default jar"): with no Default, one is made at the top level; with
    /// several (two replicas' concurrent first launches), the lowest id keeps the flag and each other is cleared and
    /// renamed, a deterministic repair every replica makes alike. Returns whether it wrote anything; the caller
    /// saves. A Default inside a group moves to the top level.
    pub(crate) fn ensure_default_jar(&mut self) -> bool {
        // A Default never sits in a group, hidden ones included (spec decision 10): one that arrived inside a group
        // moves out. One this replica shows moves now; one hidden under a closed group is queued, and the save's
        // visibility pass shows it, so no second Default is made meanwhile.
        let grouped: Vec<(NodeId, bool)> = self
            .journal
            .as_ref()
            .map(|j| {
                let doc = j.doc();
                doc.live()
                    .filter(|&(id, n)| n.kind() == Kind::Jar && doc.effective_group(id).is_some())
                    .filter(|(_, n)| n.get(field::IS_DEFAULT) == FieldValue::Bool(true))
                    .map(|(id, _)| (id, self.nodes.contains_key(&id.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let (mut queued_hidden, mut moved_here) = (false, false);
        for (id, here) in grouped {
            if here {
                self.edit_group(&id.to_string(), None);
                moved_here = true;
            } else {
                self.writes.pending_moves.insert(id, None);
                queued_hidden = true;
            }
        }
        if queued_hidden {
            return true;
        }
        let defaults: Vec<NodeId> = self.mirror.default_jar_ids().filter(|&id| self.jar_exists(id)).collect();
        let Some((_, losers)) = defaults.split_first() else {
            let pos = crate::graph_layout::GRAPH_WORLD_CENTER + crate::graph_layout::spawn_jitter(DEFAULT_NAME);
            let id = self.create_jar(DEFAULT_NAME, crate::fetched::Placement { group: None, pos });
            self.edit_field(&id.to_string(), field::IS_DEFAULT, FieldValue::Bool(true));
            return true;
        };
        for loser in losers {
            let key = loser.to_string();
            self.edit_field(&key, field::IS_DEFAULT, FieldValue::Bool(false));
            self.edit_field(&key, field::NAME, FieldValue::Str(OLD_DEFAULT_NAME.to_owned()));
        }
        moved_here || !losers.is_empty()
    }

    /// A new named jar under `name`'s clean form (name rules), with the first palette colour no jar shows (one without
    /// a colour shows the first, `jar_color`), so deleting a jar doesn't make the next one repeat a colour; with every
    /// colour taken, they cycle by count. A card placed at `at`; its fields go through `edit_field`, and the caller's
    /// `save_session` creates it.
    pub(crate) fn create_jar(&mut self, name: &str, at: crate::fetched::Placement) -> NodeId {
        let defs = self.jar_defs();
        let shown = |c: &egui::Color32| defs.values().any(|d| d.color.unwrap_or(TAG_COLOR_PALETTE[0]) == *c);
        let free = TAG_COLOR_PALETTE.iter().find(|c| !shown(c));
        let color = *free.unwrap_or(&TAG_COLOR_PALETTE[defs.len() % TAG_COLOR_PALETTE.len()]);
        let key = self.place_node(None, Kind::Jar, Runtime::Jar, at);
        self.edit_field(&key, field::NAME, FieldValue::Str(crate::names::clean_name(name).into_owned()));
        self.edit_field(&key, field::COLOR, color_value(Some(color)));
        key.parse().expect("place_node returns a store id")
    }

    /// Where a jar made for `tab` goes: its level (group), just above it.
    pub(crate) fn jar_placement_beside(&self, tab: &str) -> crate::fetched::Placement {
        crate::fetched::Placement {
            group: self.mirror.group(tab).map(str::to_owned),
            pos: self.layout_pos(tab) + egui::vec2(0.0, -80.0),
        }
    }

    /// The jar whose label (`named_jar_label`) is `text`, else one whose stored name is: of several with that name, the
    /// lowest node id (`jar_defs` iterates in id order), so the pick is the same every time and on every device. Both
    /// compared exactly, after trimming `text`.
    pub(crate) fn jar_named(&self, text: &str) -> Option<NodeId> {
        let text = text.trim();
        let defs = self.jar_defs();
        let label = defs.keys().find(|&&id| named_jar_label(id, &defs).is_some_and(|l| l.to_string() == text));
        // name rule: SETJAR matches the stored name
        label.or_else(|| defs.iter().find(|(_, d)| d.name == text).map(|(id, _)| id)).copied()
    }

    /// `SETJAR`: the focused tab picks the jar `name` names (`jar_named`: a label or a stored name, tried as typed and
    /// then cleaned; "default", any case, for the default jar), made under the clean name first if none has it.
    /// Nothing happens, and no jar is made, when the focused node is not a tab or nothing is left of the name once
    /// cleaned (name rules). Returns whether it acted.
    pub(crate) fn set_focused_tab_jar_named(&mut self, name: &str) -> bool {
        let Some(tab) = self.focused.clone().filter(|t| self.is_tab(t)) else { return false };
        let clean = crate::names::clean_name(name).into_owned();
        if clean.is_empty() {
            return false;
        }
        let jar = if clean.eq_ignore_ascii_case("default") {
            self.default_jar()
        } else {
            let existing = self.jar_named(name).or_else(|| self.jar_named(&clean));
            Some(existing.unwrap_or_else(|| self.create_jar(&clean, self.jar_placement_beside(&tab))))
        };
        self.set_jar_edge(&tab, jar);
        true
    }

    /// Turns jar `id`'s ephemeral flag on or off (deviation 10; final review I1), a local toggle (after
    /// `request_jar_ephemeral`'s confirm): the flag goes through `edit_field`. See `switch_jar_ephemeral`.
    pub(crate) fn set_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        self.switch_jar_ephemeral(id, on);
    }

    /// Jar `id`'s ephemeral flag has just changed to `on`, locally or from another replica (the mirror already
    /// holds it). Turning it on: the jar's cookies are kept in memory for the running tabs (loaded from its file
    /// first if they were not, whatever the flag now says), then the file is deleted unless the jar is locked (an
    /// unreadable file is never touched, and a locked jar saves nothing anyway). Turning it off: the jar's cookies
    /// are written to a new file (a locked one writes nothing).
    pub(crate) fn after_jar_ephemeral_changed(&mut self, id: NodeId, on: bool) {
        if on {
            if let (false, Some(dir)) = (self.jars.contains_key(&id), self.jars_dir()) {
                let jar = Jar::load(&dir, id);
                self.publish_jar(id, jar);
            }
            if let (Some(dir), Some(false)) = (self.jars_dir(), self.jars.get(&id).map(Jar::is_locked)) {
                if let Err(e) = Jar::delete_files(&dir, id) {
                    eprintln!("jar: could not delete {id}: {e}");
                }
            }
        } else {
            self.ensure_jar_loaded(id);
            self.save_jar(id);
        }
    }

    /// A local Make/Stop Ephemeral (`set_jar_ephemeral`, after any confirm): writes the flag, then its effects.
    fn switch_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        if !self.jar_exists(id) || self.mirror.jar_def(&id.to_string()).ephemeral == on {
            return;
        }
        self.edit_field(&id.to_string(), field::EPHEMERAL, FieldValue::Bool(on));
        self.after_jar_ephemeral_changed(id, on);
    }

    /// A jar card's Make Ephemeral. Turning it on for a jar with a saved file asks first (`JarConfirm::Ephemeral`):
    /// that file, its saved logins, is deleted. Anything else acts at once.
    pub(crate) fn request_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        // A jar whose cookies are not on disk yet has logins to lose too: save them, so it asks first.
        if on && self.cookies.jar_dirty.contains_key(&id) {
            self.save_jar(id);
        }
        if on && self.has_saved_file(id) {
            self.offer_confirm(Confirm::Jar(JarConfirm::Ephemeral(id)));
        } else {
            self.set_jar_ephemeral(id, on);
        }
    }

    fn has_saved_file(&self, id: NodeId) -> bool {
        self.jars_dir().is_some_and(|dir| Jar::path(&dir, id).exists())
    }

    /// A removed jar's in-memory state: its cookies saved if dirty (a Recently Closed entry may bring it back, and
    /// its file is what comes back with it), then dropped. Files are not touched here: closing never wipes
    /// credentials (spec "Backing files"); the startup prune deletes a file nothing holds.
    pub(crate) fn forget_jar_memory(&mut self, id: NodeId) {
        if self.cookies.jar_dirty.contains_key(&id) {
            self.save_jar(id);
        }
        self.jars.remove(&id);
        if let Ok(mut snapshots) = self.cookies.jar_snapshots.lock() {
            snapshots.remove(&id);
        }
    }

    /// `profile::jars_dir()`, or a test's temp dir. Never the real profile under test (`profile::dir()` is None).
    pub(crate) fn jars_dir(&self) -> Option<std::path::PathBuf> {
        #[cfg(test)]
        if let Some(dir) = &self.jars_dir_override {
            return Some(dir.clone());
        }
        crate::profile::jars_dir()
    }

    /// The jar a source resolves to (spec "Resolution") and whether it dangles. `own` is the source's own edge and
    /// `origin` the level that edge reaches out from (`reach_origin`). The walk stops at the first edge it meets,
    /// from `own` outward through each enclosing group's edge (a group's edge reaching out from its own contents).
    /// A live jar in reach is the jar. A jar gone, or out of reach (an edge that arrived so from another device), is
    /// the Default with `true`: an explicit edge to nothing warns rather than climbing on. No edge anywhere: the
    /// Default. `None` only when there is no Default.
    ///
    /// Per card per frame (`jar_chip`, `will_switch`, `sweep_jar_switches`): one mirror lookup per enclosing group,
    /// and at the edge it meets one `to_string` and a reach walk of the same depth; `default_jar` is an index read.
    /// No other allocation.
    pub(crate) fn resolve_jar(&self, own: Option<NodeId>, origin: Option<&str>) -> Option<(NodeId, bool)> {
        let (mut edge, mut reach, mut next) = (own, origin, origin);
        for _ in 0..=self.nodes.len() {
            if let Some(jar) = edge {
                let at = self.mirror.group(&jar.to_string());
                let in_reach = self.levels_from(reach).any(|level| level == at);
                return if self.jar_exists(jar) && in_reach {
                    Some((jar, false))
                } else {
                    self.default_jar().map(|d| (d, true))
                };
            }
            let Some(g) = next.filter(|g| self.nodes.contains_key(*g)) else { break };
            edge = self.mirror.jar(g);
            reach = Some(g);
            next = self.mirror.group(g);
        }
        self.default_jar().map(|d| (d, false))
    }

    /// `resolve_jar` for a tab or group the mirror holds (live, asleep or mid-respawn).
    pub(crate) fn effective_jar(&self, id: &str) -> Option<(NodeId, bool)> {
        self.resolve_jar(self.mirror.jar(id), self.reach_origin(id))
    }

    /// Whether `id` is a tab (live, unloaded or mid-respawn: `linked_ids`); false for any other node and for a new
    /// spawn, which has no mirror entry until it lands.
    pub(crate) fn is_tab(&self, id: &str) -> bool {
        match self.nodes.get(id) {
            Some(node) => node.runtime.saved_tab().is_some(),
            None => self.is_linked(id), // only a tab respawns
        }
    }

    /// The menu's and SETJAR's pick for tab or group `id`: draw its `jar` edge to `jar`, or clear it (`None`,
    /// "Inherit"). A refused edge (out of reach, not a jar) changes nothing. Live tabs whose jar changed switch.
    pub(crate) fn set_jar_edge(&mut self, id: &str, jar: Option<NodeId>) {
        match jar {
            Some(jar) => {
                let _ = self.connect(JAR_EDGE, id, &jar.to_string());
            }
            None => {
                if let Some(old) = self.mirror.jar(id) {
                    self.disconnect(JAR_EDGE, id, &old.to_string());
                }
            }
        }
        self.sweep_jar_switches();
    }

    /// After a local move of `moved` (a card, or a group with its contents): every edge whose row reaches beyond
    /// one level (`Scope::SameLevelOrAncestor`) with an end in `moved`'s subtree is checked again, and one whose live
    /// target is now out of reach is cleared, with a toast naming it (spec "Scope"). A dangling edge (target gone)
    /// is left: it warns, and is never rewritten. Then live tabs whose jar changed switch.
    pub(crate) fn recheck_edge_scope(&mut self, moved: &str) {
        let rows: Vec<(&'static str, u32)> = crate::caps::edges()
            .user_creatable()
            .filter(|r| r.scope() == Scope::SameLevelOrAncestor)
            .filter_map(|r| match r.storage() {
                Storage::Field(no) => Some((r.id(), no)),
                _ => None,
            })
            .collect();
        let mut cleared = Vec::new();
        for (edge, no) in rows {
            let scope = crate::caps::edges().row(edge).expect("a listed row").scope();
            for source in self.linked_ids().map(str::to_owned).collect::<Vec<_>>() {
                let FieldValue::Id(Some(target)) = self.mirror.get(&source, no) else { continue };
                let to = target.to_string();
                let touched = self.within(&source, moved) || self.within(&to, moved);
                if touched && self.nodes.contains_key(&to) && !self.in_scope(scope, &source, &to) {
                    self.edit_field(&source, no, FieldValue::Id(None));
                    cleared.push(target);
                }
            }
        }
        for jar in cleared {
            let name = self.jar_label(jar);
            self.queue_toast(format!("{name} is out of reach here; the edge to it was removed"));
        }
        self.sweep_jar_switches();
    }

    /// Loads `key`'s jar the first time a tab needs it (spec "Manifold state": named jars load lazily). An ephemeral
    /// jar starts empty and is never read from disk.
    pub(crate) fn ensure_jar_loaded(&mut self, id: NodeId) {
        if self.jars.contains_key(&id) {
            return;
        }
        let jar = match self.jars_dir() {
            Some(dir) if !self.is_ephemeral(id) => Jar::load(&dir, id),
            _ => Jar::default(),
        };
        self.publish_jar(id, jar);
    }

    /// Stores `jar` under `id` and copies it to the spawn threads' snapshot map.
    fn publish_jar(&mut self, id: NodeId, jar: Jar) {
        if let Ok(mut snapshots) = self.cookies.jar_snapshots.lock() {
            snapshots.insert(id, jar.clone());
        }
        self.jars.insert(id, jar);
    }

    fn is_ephemeral(&self, id: NodeId) -> bool {
        // The mirror, not `jar_defs`: a jar being removed (`forget_jar_memory`) has left `nodes` but not yet the mirror.
        self.mirror.jar_def(&id.to_string()).ephemeral
    }

    /// Writes jar `id`'s file unless it is ephemeral. A failure is logged (never cookie values) and leaves other jars
    /// alone. Settles the jar's dirty mark (`jar_dirty`): every write goes through here.
    pub(crate) fn save_jar(&mut self, id: NodeId) {
        self.cookies.jar_dirty.remove(&id); // written now, or never (ephemeral, no dir)
        let (Some(dir), Some(jar)) = (self.jars_dir(), self.jars.get(&id)) else { return };
        if self.is_ephemeral(id) {
            return;
        }
        #[cfg(test)]
        self.jar_saves.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Err(e) = jar.save(&dir, id) {
            eprintln!("jar: could not save {id}: {e}");
        }
    }

    /// Empties jar `id` and saves it, then switches the live tabs running on it (deviation 14): their engines still
    /// hold the old cookies. A jar that no longer exists is left alone, so no file is written for it.
    pub(crate) fn clear_jar_cookies(&mut self, id: NodeId) {
        if !self.jar_exists(id) {
            return;
        }
        self.ensure_jar_loaded(id);
        let Some(jar) = self.jars.get(&id) else { return };
        // Cleared in place, so a locked jar stays locked and its unreadable file is still never overwritten.
        let mut jar = jar.clone();
        jar.cookies.clear();
        // Counted with the emptied copy, under one lock: a host still spawning whose seed was read before this is
        // caught when it lands (`note_spawned_jar`), as it has no `jar_tabs` entry to mark below.
        if let Ok(mut snapshots) = self.cookies.jar_snapshots.lock() {
            snapshots.insert_cleared(id, jar.clone());
        }
        self.jars.insert(id, jar);
        self.save_jar(id);
        // Their engines still hold the old cookies: they must be replaced although their jar is unchanged.
        let running: Vec<String> =
            self.cookies.jar_tabs.iter().filter(|(_, (j, _))| *j == id).map(|(tab, _)| tab.clone()).collect();
        self.cookies.jar_reseed.extend(running);
        self.sweep_jar_switches();
    }

    /// The live jar cards' definitions, in id order (the order labels and pickers use).
    pub(crate) fn jar_defs(&self) -> BTreeMap<NodeId, JarDef> {
        self.nodes
            .iter()
            .filter(|(_, n)| matches!(n.runtime, Runtime::Jar))
            .filter_map(|(id, _)| Some((id.parse().ok()?, self.mirror.jar_def(id))))
            .collect()
    }

    /// Whether `id` is a live jar card.
    pub(crate) fn jar_exists(&self, id: NodeId) -> bool {
        self.nodes.get(&id.to_string()).is_some_and(|n| matches!(n.runtime, Runtime::Jar))
    }
}

#[cfg(test)]
mod tests;
