//! The omnibar: one field and dropdown for the Graph, Item and Settings views and the Ctrl+K popup. Its rows come from
//! `rows` (pure), its state is `OmniBar`, what Enter does is `App::omni_activate`, and `ui` draws it.

mod rows;
mod ui;

pub(crate) use rows::{default_highlight, Mode, Row};

use super::*;
use crate::card_kind::kind_caps;
use crate::store::model::NodeId;
use rows::{CardInfo, Corpus, OtherInfo};
use std::collections::HashMap;

/// Where an omnibar is drawn: the view it sits in, or the Ctrl+K popup. The Item view's is `Mode::Address`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Host {
    Graph,
    Item,
    Settings,
    Popup,
}

impl Host {
    pub(crate) fn mode(self) -> Mode {
        if self == Host::Item {
            Mode::Address
        } else {
            Mode::Query
        }
    }
}

/// One omnibar's state. `App::omni` serves whichever view is showing; `App::omni_popup` is the Ctrl+K popup's own.
pub(crate) struct OmniBar {
    pub(crate) text: String,
    pub(crate) focused: bool,
    /// Index into the rows Enter picks; `None` means none (Enter opens the typed text).
    pub(crate) highlighted: Option<usize>,
    pub(crate) mode: Mode,
    /// The host that drew this bar last frame: a different one reseeds the text.
    pub(crate) host: Option<Host>,
    /// The dropdown is showing: the text was edited since it last closed, or this is the popup.
    pub(crate) open: bool,
    /// Search suggestions, which arrive later from `fetcher`'s debounced background fetch.
    pub(crate) search_suggestions: Vec<String>,
    pub(crate) fetcher: SuggestionFetcher,
    /// Frames in a row the field hasn't had focus. The dropdown survives one (egui's focus blip as its buttons
    /// appear, see `editing`) and closes after that: a click into the page must dismiss it.
    pub(crate) unfocused_frames: u8,
}

impl OmniBar {
    pub(crate) fn new(text: String) -> Self {
        Self {
            text,
            focused: false,
            highlighted: None,
            mode: Mode::Query,
            host: None,
            open: false,
            search_suggestions: Vec::new(),
            fetcher: SuggestionFetcher::new(),
            unfocused_frames: 0,
        }
    }

    /// Records this frame's focus; dismisses the dropdown once focus has been away for more than one frame.
    pub(crate) fn note_focus(&mut self, has_focus: bool) {
        self.focused = has_focus;
        self.unfocused_frames = if has_focus { 0 } else { self.unfocused_frames.saturating_add(1) };
        if self.unfocused_frames > 1 && self.open {
            self.dismiss();
        }
    }

    /// Whether the user is mid-edit, so the text mustn't be resynced to the tab's URL. Not just `focused`: found
    /// live that the moment the dropdown's buttons first appear, egui's own keyboard-focus tracking transiently
    /// moves off the `TextEdit` for a frame, so `has_focus()` reports false for that one frame and (without this)
    /// the resync blew away what the user had just typed mid-keystroke. The dropdown being open survives that blip.
    pub(crate) fn editing(&self) -> bool {
        self.focused || self.open
    }

    /// Closes the dropdown, and cancels the search fetch in flight: its response would otherwise reopen the
    /// dropdown after an Enter or Escape.
    pub(crate) fn dismiss(&mut self) {
        self.open = false;
        self.search_suggestions.clear();
        self.highlighted = None;
        self.fetcher.cancel();
    }
}

/// The open cards and history `rows` draws from.
struct AppCorpus<'a> {
    app: &'a App,
    /// The tab the Item view shows: no use offering a jump to where the user is.
    exclude: Option<NodeId>,
}

impl Corpus for AppCorpus<'_> {
    fn offers_commands(&self) -> bool {
        self.exclude.is_none()
    }
    fn cards(&self) -> Vec<CardInfo> {
        let app = self.app;
        app.node_order
            .iter()
            .filter(|&&id| Some(id) != self.exclude)
            .filter_map(|&id| {
                let (label, url) = (app.listed_tab_label(id)?, app.listed_tab_url(id)?);
                let jar = app.effective_jar(id).map(|(jar, _)| app.doc.mirror.name(jar).to_owned()); // name rule: stored
                Some(CardInfo { id, label, url, jar, labels: app.label_names(id), closed: false })
            })
            .collect()
    }

    fn others(&self) -> Vec<OtherInfo> {
        let app = self.app;
        app.node_order
            .iter()
            .filter(|&&id| Some(id) != self.exclude)
            .filter_map(|&id| {
                // The registry says which kinds the omnibar offers (`Caps::omni_row`): a jar holds cookies and is no
                // card to open, a card of a type this build does not know has no name to search, a link or label is
                // never a card.
                let caps = kind_caps(app.doc.mirror.kind_of(id));
                if !caps.omni_row {
                    return None;
                }
                Some(OtherInfo { id, label: app.accessible_name(id)?, glyph: caps.glyph, labels: app.label_names(id) })
            })
            .collect()
    }

    fn history(&self) -> Vec<(String, String)> {
        self.app.history.iter().rev().map(|h| (h.title.clone(), h.url.clone())).collect()
    }

    fn text_hits(&self, text: &str) -> Vec<NodeId> {
        self.app.search.hits(text).ids.clone()
    }

    fn snippet(&self, id: NodeId, text: &str) -> Option<String> {
        crate::search::Index::snippet(&self.app.index_text(crate::search::DocKey::Node(id)), text, 80)
    }
}

impl App {
    /// A tab list row's label for `id`: its title, or its url when the title is empty (live or unloaded alike, via
    /// `listed_tab`).
    pub(crate) fn listed_tab_label(&self, id: NodeId) -> Option<String> {
        let (title, url) = self.nodes.get(&id)?.runtime.listed_tab()?;
        Some(if title.is_empty() { url.to_string() } else { title.to_string() })
    }

    pub(crate) fn listed_tab_url(&self, id: NodeId) -> Option<String> {
        self.nodes.get(&id)?.runtime.listed_tab().map(|(_, url)| url.to_owned())
    }

    /// The qualifier each listed tab needs beside its label to be told from a tab that reads the same
    /// (`disambiguate::qualifiers`), over all listed tabs so a row's qualifier does not change as the query narrows.
    pub(crate) fn listed_tab_qualifiers(&self) -> HashMap<NodeId, String> {
        let ids: Vec<NodeId> =
            self.node_order.iter().copied().filter(|&id| self.listed_tab_label(id).is_some()).collect();
        let labels: Vec<(String, String)> = ids
            .iter()
            .map(|&id| (self.listed_tab_label(id).unwrap_or_default(), self.listed_tab_url(id).unwrap_or_default()))
            .collect();
        let rows: Vec<(&str, &str)> = labels.iter().map(|(l, u)| (l.as_str(), u.as_str())).collect();
        let qualifiers = crate::disambiguate::qualifiers(&rows, true);
        ids.into_iter().zip(qualifiers).filter_map(|(id, q)| Some((id, q?))).collect()
    }

    /// Whether `text` matches card `id`: its title, url, file name or text, its stored name, or the page text the index holds. The one test the
    /// Graph view dims by, over every node and uncapped (the dropdown's rows are capped).
    pub(crate) fn node_matches(&self, text: &str, id: NodeId) -> bool {
        let Some(node) = self.nodes.get(&id) else { return false };
        let labels = self.label_names(id);
        let (tags, plain) = rows::split_query(text);
        if !rows::labels_match(&tags, &labels) {
            return false;
        }
        let mut needles = crate::card::card_filter_text(&node.runtime, self.own_text(id));
        needles.push(self.doc.mirror.name(id)); // name rule: the filter matches the stored text (a rename; a group is matched twice)
        needles.extend(labels.iter().map(String::as_str));
        crate::text::filter_matches(&plain, &needles) || self.search.hits(&plain).set.contains(&id)
    }

    /// The text the Graph view dims by: the inline omnibar's while it is the Graph's, else none (all match).
    pub(crate) fn omni_filter(&self) -> &str {
        if self.omni.host == Some(Host::Graph) {
            &self.omni.text
        } else {
            ""
        }
    }

    /// Sets the Graph's omnibar text (`GRAPHFILTERTEXT`).
    pub(crate) fn set_graph_filter(&mut self, text: &str) {
        self.omni.host = Some(Host::Graph);
        self.omni.mode = Mode::Query;
        self.omni.text = text.to_owned();
    }

    fn bar(&self, host: Host) -> &OmniBar {
        if host == Host::Popup {
            &self.omni_popup
        } else {
            &self.omni
        }
    }

    pub(crate) fn bar_mut(&mut self, host: Host) -> &mut OmniBar {
        if host == Host::Popup {
            &mut self.omni_popup
        } else {
            &mut self.omni
        }
    }

    /// The rows `host`'s bar offers for its text.
    pub(crate) fn omni_rows(&self, host: Host) -> Vec<Row> {
        let bar = self.bar(host);
        let exclude = (host.mode() == Mode::Address).then_some(self.focused).flatten();
        rows::rows(&AppCorpus { app: self, exclude }, &bar.text, &bar.search_suggestions)
    }

    /// The text of `host`'s bar changed: its highlight restarts at the default, the dropdown opens and a search
    /// fetch is queued.
    pub(crate) fn omni_text_changed(&mut self, host: Host) {
        let mode = host.mode();
        let highlight = default_highlight(mode, &self.omni_rows(host));
        let bar = self.bar_mut(host);
        bar.highlighted = highlight;
        bar.open = true;
        bar.fetcher.note_text_changed(&bar.text.clone());
    }

    /// Sets `host`'s text as typing would (`QUICKSWITCHTEXT`).
    pub(crate) fn omni_set_text(&mut self, host: Host, text: &str) {
        self.bar_mut(host).text = text.to_owned();
        self.omni_text_changed(host);
    }

    /// Moves `host`'s highlight by `delta`, wrapping at both ends. With none highlighted, Down lands on the first
    /// row and Up on the last.
    pub(crate) fn omni_move(&mut self, host: Host, delta: isize) {
        let len = self.omni_rows(host).len();
        if len == 0 || delta == 0 {
            return;
        }
        let bar = self.bar_mut(host);
        let from = bar.highlighted.map(|i| i.min(len - 1)).unwrap_or(if delta > 0 { len - 1 } else { 0 });
        bar.highlighted = Some(crate::popups::step_index(from, delta, len, true));
    }

    /// Enter: activates the highlighted row, or with none highlighted (the Item view's typed address) the typed text.
    pub(crate) fn omni_confirm(&mut self, host: Host) {
        let rows = self.omni_rows(host);
        let bar = self.bar(host);
        // Query mode always has a row Enter picks: after a dismiss the highlight is gone but the rows are not.
        let highlighted = match (bar.mode, bar.highlighted) {
            (Mode::Query, None) => rows::default_highlight(Mode::Query, &rows),
            (_, h) => h,
        };
        let picked = match (bar.mode, highlighted) {
            (_, None) => None,
            (Mode::Query, Some(i)) => rows.get(i.min(rows.len().saturating_sub(1))),
            (Mode::Address, Some(i)) => rows.get(i),
        };
        let row = match picked {
            Some(row) => Some(row.clone()),
            None if bar.text.trim().is_empty() => None,
            None => Some(Row::Typed { text: bar.text.trim().to_owned() }),
        };
        match row {
            Some(row) => self.omni_activate(row, host),
            None => self.omni_close(host),
        }
    }

    /// Escape: clears a typed text, and with none left closes. Returns whether it did either.
    pub(crate) fn omni_escape(&mut self, host: Host) -> bool {
        let bar = self.bar_mut(host);
        if !bar.text.is_empty() && host != Host::Item {
            bar.text.clear();
            bar.dismiss();
            return true;
        }
        if host == Host::Popup {
            self.omni_close(host);
            return true;
        }
        let was_open = std::mem::take(&mut self.bar_mut(host).open);
        self.bar_mut(host).dismiss();
        was_open
    }

    /// Ends `host`'s edit: the dropdown closes, and the popup goes away. An Item bar keeps its text (the tab's
    /// address resyncs it); the others restart empty.
    pub(crate) fn omni_close(&mut self, host: Host) {
        let bar = self.bar_mut(host);
        bar.dismiss();
        bar.focused = false;
        if host != Host::Item {
            bar.text.clear();
        }
        if host == Host::Popup {
            self.menus.omni_popup.open = None;
        }
    }

    /// Opens the Ctrl+K popup, empty, with its field focused (Ctrl+K and `QUICKSWITCHOPEN`).
    pub(crate) fn open_omni_popup(&mut self) {
        let mut bar = OmniBar::new(String::new());
        bar.host = Some(Host::Popup);
        bar.open = true;
        self.omni_popup = bar;
        self.omni_text_changed(Host::Popup);
        self.menus.omni_popup.open = Some(());
        self.pending_focus = Some(FocusTarget::OmniPopup);
    }

    fn omni_open_url(&mut self, url: String, host: Host) {
        match (host, self.focused) {
            (Host::Item, Some(tab)) => self.navigate_tab(tab, url),
            _ => {
                if self.current_json_node().is_some() {
                    self.leave_json_level();
                }
                let group = self.current_group;
                self.spawn_new_tab(None, group, &url);
            }
        }
    }

    /// Shows card `id` in the Graph on its level and eases the camera to it, leaving the Item or Settings view (and the
    /// image viewer) for the Graph. A card that has since gone does nothing.
    fn reveal_in_graph(&mut self, id: NodeId) {
        if !self.nodes.contains_key(&id) {
            return;
        }
        // Whatever view is leaving (the Item view, or the Settings page opened from a tab) may hold the tab showing.
        if self.viewed_image.is_none() {
            self.hide_focused_tab();
        }
        self.viewed_image = None;
        self.view_mode = ViewMode::Graph;
        self.show_level_of(id);
        self.camera.reveal = Some(id);
        self.save_session();
    }

    /// What Enter (or a click) on `row` does in `host` (spec decision 8): a tab is jumped to in the Graph and opened
    /// elsewhere, any other card is shown in the Graph from every host, a page or search opens in a new tab (the Item
    /// view navigates its own), a setting opens the Settings page at its row.
    pub(crate) fn omni_activate(&mut self, row: Row, host: Host) {
        let engine = self.settings.value.default_search_engine;
        match row {
            Row::Card { id, .. } => {
                self.show_level_of(id);
                if host == Host::Graph {
                    self.camera.reveal = Some(id);
                } else {
                    self.focus_tab(id);
                }
            }
            Row::Other { id, .. } => self.reveal_in_graph(id),
            Row::Setting { id, .. } => {
                self.open_settings_page();
                self.settings.focus = Some(id);
            }
            Row::History { url, .. } => self.omni_open_url(url, host),
            Row::Typed { text } => self.omni_open_url(resolve_navigation_url(&text, engine), host),
            Row::Suggestion { text } => self.omni_open_url(engine.search_url(&text), host),
            Row::Command(command) => (command.run)(self),
        }
        self.omni_close(host);
    }
}

#[cfg(test)]
mod tests;
