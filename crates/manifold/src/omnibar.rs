//! The omnibar: one field and dropdown for the Graph, Item and Settings views and the Ctrl+K popup. Its rows come from
//! `rows` (pure), its state is `OmniBar`, what Enter does is `App::omni_activate`, and `ui` draws it.

mod rows;
mod ui;

pub(crate) use rows::{default_highlight, Command, Mode, Row};

use super::*;
use crate::store::model::NodeId;
use rows::{CardInfo, Corpus};
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
                Some(CardInfo { id, label, url, jar, closed: false })
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
        let mut needles = crate::card::card_filter_text(&node.runtime, self.own_text(id));
        needles.push(self.doc.mirror.name(id)); // name rule: the filter matches the stored text (a rename; a group is matched twice)
        crate::text::filter_matches(text, &needles) || self.search.hits(text).set.contains(&id)
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
        let picked = match (bar.mode, bar.highlighted) {
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

    /// What Enter (or a click) on `row` does in `host` (spec decision 8): a card is jumped to in the Graph and opened
    /// elsewhere, a page or search opens in a new tab (the Item view navigates its own), a setting opens the
    /// Settings page at its row.
    pub(crate) fn omni_activate(&mut self, row: Row, host: Host) {
        let engine = self.settings.default_search_engine;
        match row {
            Row::Card { id, .. } => {
                self.show_level_of(id);
                if host == Host::Graph {
                    self.camera.reveal = Some(id);
                } else {
                    self.focus_tab(id);
                }
            }
            Row::Setting { id, .. } => {
                self.open_settings_page();
                self.settings_focus = Some(id);
            }
            Row::History { url, .. } => self.omni_open_url(url, host),
            Row::Typed { text } => self.omni_open_url(resolve_navigation_url(&text, engine), host),
            Row::Suggestion { text } => self.omni_open_url(engine.search_url(&text), host),
            Row::Command(Command::SuggestGroups) => {
                self.suggest_groups();
            }
        }
        self.omni_close(host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_sync::tests::{add, app_on, live_tab, sleeping_tab};
    use crate::store::model::Kind;

    fn tab(app: &mut App, url: &str, title: &str) -> NodeId {
        add(app, Kind::Tab, sleeping_tab(url, title))
    }

    fn card_row(app: &App, id: NodeId) -> Row {
        Row::Card {
            id,
            label: app.listed_tab_label(id).unwrap(),
            url: app.listed_tab_url(id).unwrap(),
            jar: None,
            snippet: None,
        }
    }

    #[test]
    fn the_dropdown_closes_after_focus_stays_away() {
        let mut bar = OmniBar::new(String::new());
        bar.open = true;
        bar.note_focus(false);
        assert!(bar.editing(), "one frame is the known blip");
        bar.note_focus(true);
        bar.note_focus(false);
        assert!(bar.editing(), "regaining focus starts the count again");
        bar.note_focus(false);
        assert!(!bar.editing(), "dismissed");
    }

    #[test]
    fn enter_on_a_card_row_in_graph_jumps_to_the_card_and_sets_reveal() {
        let mut app = App::default();
        let near = tab(&mut app, "https://near.test/", "Near");
        let far = add(&mut app, Kind::Tab, sleeping_tab("https://far.test/", "Far"));
        app.view_mode = ViewMode::Graph;
        app.layout.entry(far).or_default().pos = egui::vec2(5000.0, 0.0);
        app.camera.jump_to(egui::Rect::from_center_size(egui::pos2(100.0, 100.0), egui::vec2(900.0, 700.0)));
        app.omni_activate(card_row(&app, far), Host::Graph);
        assert_eq!(app.camera.reveal, Some(far));
        assert_eq!(app.view_mode, ViewMode::Graph, "a jump, not an open");
        app.apply_camera_requests(false);
        assert!(app.camera.is_moving(), "eased to the far card");
        let _ = near;
    }

    #[test]
    fn enter_on_a_typed_row_in_graph_opens_a_new_tab() {
        let mut app = App::default();
        app.omni_activate(Row::Typed { text: "rust lang".into() }, Host::Graph);
        let (spawn, _) = app.test_spawns.last().expect("a tab was spawned");
        assert_eq!(spawn.url, app.settings.default_search_engine.search_url("rust lang"));
    }

    #[test]
    fn enter_on_a_typed_row_in_item_navigates_the_current_tab() {
        let mut app = App::default();
        let (id, mut peer) = live_tab(&mut app, crate::ViewKind::Servo, "https://a.test/");
        app.focused = Some(id);
        app.view_mode = ViewMode::Item;
        let nodes = app.nodes.len();
        app.omni_activate(Row::Typed { text: "https://b.test/".into() }, Host::Item);
        assert_eq!(app.nodes.len(), nodes);
        assert!(app.test_spawns.is_empty());
        assert!(peer.sent().iter().any(|m| matches!(m, HostToView::Navigate { url, .. } if url == "https://b.test/")));
    }

    #[test]
    fn enter_on_a_settings_row_sets_settings_focus_and_opens_settings() {
        let mut app = App::default();
        let setting = Row::Setting { id: "theme", title: "Theme", section: "General" };
        app.omni_activate(setting, Host::Graph);
        assert_eq!(app.view_mode, ViewMode::Settings);
        assert_eq!(app.settings_focus, Some("theme"));
    }

    #[test]
    fn escape_clears_the_text_then_closes() {
        let mut app = App::default();
        app.open_omni_popup();
        app.omni_set_text(Host::Popup, "abc");
        assert!(app.omni_escape(Host::Popup));
        assert!(app.omni_popup.text.is_empty());
        assert!(app.menus.omni_popup.open.is_some(), "the first Escape only clears");
        assert!(app.omni_escape(Host::Popup));
        assert!(app.menus.omni_popup.open.is_none());
    }

    #[test]
    fn dimming_matches_every_card_past_the_row_cap() {
        let mut app = App::default();
        let ids: Vec<NodeId> =
            (0..rows::ROW_CAP + 3).map(|i| tab(&mut app, &format!("https://a{i}.test/"), "alpha")).collect();
        let other = tab(&mut app, "https://z.test/", "zebra");
        assert!(ids.iter().all(|&id| app.node_matches("alpha", id)), "all of them, past the row cap");
        assert!(!app.node_matches("alpha", other));
        app.omni.text = "alpha".into();
        let shown = app.omni_rows(Host::Graph).into_iter().filter(|r| matches!(r, Row::Card { .. })).count();
        assert_eq!(shown, rows::ROW_CAP);
    }

    #[test]
    fn the_item_bar_offers_other_tabs_not_the_one_shown() {
        let mut app = App::default();
        let here = tab(&mut app, "https://here.test/", "Here");
        let there = tab(&mut app, "https://there.test/", "Here too");
        app.focused = Some(here);
        app.omni.text = "here".into();
        let ids: Vec<NodeId> = app
            .omni_rows(Host::Item)
            .into_iter()
            .filter_map(|r| if let Row::Card { id, .. } = r { Some(id) } else { None })
            .collect();
        assert_eq!(ids, [there]);
    }

    /// A sleeping tab's row shows its title, or its url when untitled.
    #[test]
    fn a_sleeping_tabs_label_is_its_title_or_its_url() {
        let mut app = app_on(&crate::store::testutil::temp_dir("omni-label").join("session.mnf"), 1);
        let titled = tab(&mut app, "https://a/", "Alpha");
        let untitled = tab(&mut app, "https://b/", "");
        assert_eq!(app.listed_tab_label(titled).as_deref(), Some("Alpha"));
        assert_eq!(app.listed_tab_label(untitled).as_deref(), Some("https://b/"));
    }

    /// Tabs that read the same are told apart by what differs; a tab of its own label gets nothing.
    #[test]
    fn tabs_that_read_the_same_get_a_qualifier_and_others_none() {
        let mut app = app_on(&crate::store::testutil::temp_dir("omni-qualifiers").join("session.mnf"), 1);
        let one = tab(&mut app, "https://one.test/", "Home");
        let two = tab(&mut app, "https://two.test/", "Home");
        let other = tab(&mut app, "https://one.test/x", "Docs");
        let q = app.listed_tab_qualifiers();
        assert_eq!(q.get(&one).map(String::as_str), Some("one.test"));
        assert_eq!(q.get(&two).map(String::as_str), Some("two.test"));
        assert_eq!(q.get(&other), None);
    }

    /// Review C-B2: a query narrowed after moving down confirms its first match, not a stale row.
    #[test]
    fn a_narrowed_query_confirms_its_first_match() {
        let mut app = app_on(&crate::store::testutil::temp_dir("omni-narrowed").join("session.mnf"), 1);
        let ids: Vec<NodeId> = (0..4).map(|i| tab(&mut app, &format!("https://{i}/"), &format!("Tab {i}"))).collect();
        app.open_omni_popup();
        for _ in 0..3 {
            app.omni_move(Host::Popup, 1);
        }
        assert_eq!(app.omni_popup.highlighted, Some(3));
        app.omni_set_text(Host::Popup, "tab 2");
        app.omni_confirm(Host::Popup);
        assert!(app.menus.omni_popup.open.is_none());
        assert_eq!(app.focused, Some(ids[2]), "jumped to the one match");
    }

    /// Nothing open matches: Enter searches for it (a new tab), as the Graph's typed row does.
    #[test]
    fn enter_with_no_matching_card_searches_for_the_text() {
        let mut app = App::default();
        app.open_omni_popup();
        app.omni_set_text(Host::Popup, "no such tab");
        app.omni_confirm(Host::Popup);
        assert!(app.menus.omni_popup.open.is_none());
        let (spawn, _) = app.test_spawns.last().expect("a search tab");
        assert_eq!(spawn.url, app.settings.default_search_engine.search_url("no such tab"));
    }
}
