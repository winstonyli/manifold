//! Asking a tab's engine for its page text (`QueryPageText`): a short while after a load ends, and before the tab
//! sleeps, so the text log (`search::text_store`) holds what each card showed. Pages of other schemes and tabs in
//! an ephemeral jar are never asked.

use super::*;
use std::time::{Duration, Instant};

/// How long after a load ends the text is read (late scripts have rendered by then).
pub(crate) const TEXT_DELAY: Duration = Duration::from_secs(2);
/// How long an unanswered `QueryPageText` is awaited. A page with no text (empty, not HTML) never answers.
pub(crate) const ASK_TIMEOUT: Duration = Duration::from_secs(10);

/// Timers armed by a load end, and the queries whose answer is awaited, by tab.
#[derive(Default)]
pub(crate) struct PageTextTracker {
    /// Tab -> (url the load ended on, when to ask).
    timers: HashMap<NodeId, (String, Instant)>,
    /// Tab -> (the url it showed when asked, when to stop waiting); the answer is kept only if the tab still shows
    /// that url.
    asked: HashMap<NodeId, (String, Instant)>,
}

impl PageTextTracker {
    /// Whether no text request is armed or awaited.
    pub(crate) fn is_quiet(&self) -> bool {
        self.timers.is_empty() && self.asked.is_empty()
    }
}

impl App {
    /// A load on tab `id` ended on `url`: read its text after `TEXT_DELAY`.
    pub(crate) fn note_load_end(&mut self, id: NodeId, url: &str, now: Instant) {
        self.page_text.timers.insert(id, (url.to_owned(), now + TEXT_DELAY));
    }

    /// Asks the tabs whose timer is due and whose url is still the one the load ended on.
    pub(crate) fn poll_text_timers(&mut self, now: Instant) {
        self.sync_search_index(now);
        let nodes = &self.nodes;
        self.page_text.timers.retain(|id, _| nodes.contains_key(id));
        self.page_text.asked.retain(|id, (_, until)| nodes.contains_key(id) && now < *until);
        let due: Vec<NodeId> =
            self.page_text.timers.iter().filter(|(_, (_, at))| now >= *at).map(|(id, _)| *id).collect();
        for id in due {
            let Some((url, _)) = self.page_text.timers.remove(&id) else { continue };
            if self.nodes.get(&id).and_then(|n| n.as_tab()).is_some_and(|t| t.url == url) {
                self.ask_page_text(id);
            }
        }
    }

    /// Whether tab `id`'s text may be read: an http(s) page, in a jar that is not ephemeral (ruling R1).
    pub(crate) fn text_wanted(&self, id: NodeId) -> bool {
        let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_tab()) else { return false };
        let url = tab.url.to_ascii_lowercase();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return false;
        }
        !self.in_ephemeral_jar(id)
    }

    /// Sends `QueryPageText` to tab `id` when its text is wanted, remembering the url it was asked at.
    pub(crate) fn ask_page_text(&mut self, id: NodeId) {
        if !self.text_wanted(id) {
            return;
        }
        let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_tab()) else { return };
        self.page_text.asked.insert(id, (tab.url.clone(), Instant::now() + ASK_TIMEOUT));
        tab.tab.send(&HostToView::QueryPageText);
    }

    /// Whether the stored text for tab `id` is for another page than the one it shows now (or there is none).
    pub(crate) fn text_is_stale(&self, id: NodeId) -> bool {
        let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_tab()) else { return false };
        self.search.texts.get(id).is_none_or(|r| r.url != tab.url)
    }

    /// The headless `TEXTOF`: writes the text stored for `id` (or `none`) to `path`.
    pub(crate) fn write_text_of(&self, id: NodeId, path: &std::path::Path) {
        let text = self.search.texts.get(id).map_or("none", |r| r.text.as_str());
        if let Err(e) = crate::store::atomic::replace(path, text.as_bytes()) {
            eprintln!("TEXTOF: could not write {}: {e}", path.display());
        }
    }

    /// The engine's answer: kept if the tab still shows the page it was asked at and its text is still wanted.
    pub(crate) fn accept_page_text(&mut self, id: NodeId, text: String) {
        let Some((asked, _)) = self.page_text.asked.remove(&id) else { return };
        let Some(tab) = self.nodes.get(&id).and_then(|n| n.as_tab()) else { return };
        if tab.url != asked || !self.text_wanted(id) {
            return;
        }
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let rec = crate::search::text_store::TextRecord { id, url: asked, title: tab.title.clone(), text, at };
        self.search.texts.put(rec);
        self.search.next_sync = None; // index it now, not at the next tick of the pace
    }
}
