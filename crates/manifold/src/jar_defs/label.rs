//! A jar's label and what shows it: the chip, the marker fragment that tells same-named jars apart, rich-text label
//! drawing and the switch hints.

use super::*;

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

/// How the card's chip tooltip says to switch now; Tab view's dot, which is clicked instead, swaps it out.
pub(crate) const SWITCH_NOW_HINT: &str = "right-click for Switch Jar Now";

/// The chip's hover text while a live tab waits to switch. `cleared`: its jar's cookies were cleared under it, so its
/// pulls are dropped until it restarts, and `target` is where it restarts. `can_switch_now`: offer
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
pub(super) fn id_digest(id: NodeId) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(id.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The marker fragment that tells jar `id` apart from the other jars with its name; `None` when no other jar has it
/// (or there is no jar `id`). It is the start of `id_digest(id)`: 4 characters, or as many more as it takes to differ
/// from every same-named jar's. It derives from the id alone, so it never changes for the jar's life, except to grow
/// on the rare collision with a same-named jar that arrives later. Not a hex prefix of the id itself: jars made on one
/// device share the id's actor half and differ only in its decimal counter.
pub(crate) fn jar_fragment(id: NodeId, defs: &BTreeMap<NodeId, JarDef>) -> Option<String> {
    let key = crate::names::name_key(&defs.get(&id)?.name);
    let twins: Vec<String> = defs
        .iter()
        .filter(|(o, d)| **o != id && crate::names::name_key(&d.name) == key)
        .map(|(o, _)| id_digest(*o))
        .collect();
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
    pub(crate) fn jar_chip(&self, id: NodeId) -> Option<JarChip> {
        if !self.is_tab(id) {
            return None;
        }
        let (key, missing) = self.effective_jar(id)?;
        let default = self.default_jar();
        let will_switch = self.will_switch(id);
        let can_switch_now = self.can_switch_jar_now(id);
        let view_kind = match &self.nodes.get(&id)?.runtime {
            Runtime::Tab(t) => t.tab.kind,
            Runtime::Unloaded(_) => self.doc.mirror.view_kind(id),
            _ => return None,
        };
        let view_ignores = !view_kind.has(crate::caps::Tag::Jars) && Some(key) != default;
        let following = self.doc.mirror.jar(id).is_none();
        if Some(key) == default && !missing && !will_switch {
            return None;
        }
        let name = self.jar_label(key);
        let was = self.cookies.jar_tabs.get(&id).map_or(key, |(k, _)| *k);
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
            let cleared = self.cookies.jar_reseed.contains(&id);
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
            JarConfirm::Ephemeral(id) if self.jar_exists(id) && !self.doc.mirror.jar_def(id).ephemeral => Some((
                format!("Make {} ephemeral?", self.jar_label(id)),
                "Its saved sign-ins are deleted from disk now. Tabs keep them until Manifold quits; after that this \
                 jar starts empty each time."
                    .to_string(),
                "Make Ephemeral",
            )),
            _ => None,
        }
    }
}
