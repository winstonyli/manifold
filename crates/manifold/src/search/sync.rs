//! Keeping the index equal to the document: once a second, every card's document text is fingerprinted and the
//! ones that changed are queued, the ones that vanished removed. One pass covers a navigation, a rename, a note
//! edit (so a note is reindexed a second after it changes, not per keystroke), a closed card, a permanent delete
//! and a tab entering an ephemeral jar, where a hook for each would have been another special case.

use super::{page_doc, DocKey};
use crate::card::card_filter_text;
use crate::store::model::{field, FieldValue, NodeId};
use crate::App;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

/// The pace of `sync_search_index`.
pub(crate) const SYNC_EVERY: Duration = Duration::from_secs(1);

impl App {
    /// Whether tab `id` is in an ephemeral jar: nothing of it is stored or indexed (ruling R1). A closed tab has left
    /// `nodes` and the mirror, so its jar is read from the document: its own `jar` edge, else its nearest group's.
    pub(crate) fn in_ephemeral_jar(&self, id: NodeId) -> bool {
        if self.nodes.contains_key(&id) {
            return self.is_tab(id)
                && self.effective_jar(id).is_some_and(|(jar, _)| self.doc.mirror.jar_def(jar).ephemeral);
        }
        let Some(doc) = self.doc.journal.as_ref().map(|j| j.doc()) else { return false };
        let mut at = Some(id);
        for _ in 0..=doc.nodes.len() {
            let Some(node) = at.and_then(|n| doc.nodes.get(&n)) else { break };
            if let FieldValue::Id(Some(jar)) = node.get(field::JAR) {
                return self.jar_exists(jar) && self.doc.mirror.jar_def(jar).ephemeral;
            }
            at = node.group;
        }
        self.default_jar().is_some_and(|jar| self.doc.mirror.jar_def(jar).ephemeral)
    }

    /// The fingerprint of `key`'s document, or `None` when it has none to index.
    fn index_print(&self, key: DocKey) -> Option<u64> {
        let mut h = DefaultHasher::new();
        match key {
            DocKey::Node(id) => {
                let rec = self.search.texts.get(id);
                if let Some(node) = self.nodes.get(&id) {
                    card_filter_text(&node.runtime, self.own_text(id)).hash(&mut h);
                    self.doc.mirror.name(id).hash(&mut h); // name rule: the index holds the stored text, as the filter does
                    self.label_names(id).hash(&mut h);
                } else {
                    let rec = rec?;
                    (&rec.title, &rec.url).hash(&mut h);
                }
                if let Some(rec) = rec {
                    (rec.at, rec.text.len()).hash(&mut h);
                }
            }
            DocKey::History(i) => {
                let entry = self.history.get(i as usize)?;
                (&entry.title, &entry.url).hash(&mut h);
            }
        }
        Some(h.finish())
    }

    /// `key`'s document text: the card's filter text (title and url, a note's text, a name, a file name) and the
    /// text stored for its page.
    pub(crate) fn index_text(&self, key: DocKey) -> String {
        match key {
            DocKey::Node(id) => {
                let rec = self.search.texts.get(id);
                let labels = self.label_names(id);
                let mut parts: Vec<&str> = Vec::new();
                if let Some(node) = self.nodes.get(&id) {
                    parts.extend(card_filter_text(&node.runtime, self.own_text(id)));
                    parts.push(self.doc.mirror.name(id)); // name rule: the index holds the stored text, as the filter does
                    parts.extend(labels.iter().map(String::as_str));
                } else if let Some(rec) = rec {
                    parts.extend([rec.title.as_str(), rec.url.as_str()]);
                }
                parts.extend(rec.map(|r| r.text.as_str()));
                parts.join(" ")
            }
            DocKey::History(i) => {
                self.history.get(i as usize).map_or_else(String::new, |e| page_doc(&e.title, &e.url, ""))
            }
        }
    }

    /// Runs at most once per `SYNC_EVERY`: forgets the stored text of tabs in ephemeral jars and of nodes the
    /// document no longer holds, then queues what changed since the last pass.
    pub(crate) fn sync_search_index(&mut self, now: Instant) {
        if self.search.next_sync.is_some_and(|at| now < at) {
            return;
        }
        self.search.next_sync = Some(now + SYNC_EVERY);
        let stored: Vec<NodeId> = self.search.texts.ids().collect();
        for id in stored {
            let held =
                self.nodes.contains_key(&id) || self.doc.mirror.has(id) || self.doc.hidden_local.contains_key(&id);
            let gone = !held && !self.doc.store_recovered;
            if gone || self.in_ephemeral_jar(id) {
                self.search.texts.remove(id);
            }
        }
        let mut keys: Vec<DocKey> = self.nodes.keys().map(|&id| DocKey::Node(id)).collect();
        keys.extend(self.search.texts.ids().filter(|id| !self.nodes.contains_key(id)).map(DocKey::Node));
        keys.extend((0..self.history.len() as u32).map(DocKey::History));
        let mut seen = HashSet::new();
        for key in keys {
            if matches!(key, DocKey::Node(id) if self.in_ephemeral_jar(id)) {
                continue;
            }
            let Some(print) = self.index_print(key) else { continue };
            seen.insert(key);
            if self.search.indexed.get(&key) != Some(&print) {
                self.search.service.queue_add(key, self.index_text(key));
                self.search.indexed.insert(key, print);
            }
        }
        let vanished: Vec<DocKey> = self.search.indexed.keys().filter(|k| !seen.contains(k)).copied().collect();
        for key in vanished {
            self.search.service.queue_remove(key);
            self.search.indexed.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_manager::ViewKind;
    use crate::search::text_store::TextRecord;
    use crate::session_sync::tests::{add_jar, add_note, app_on, live_tab};

    fn settle(app: &App) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.search.service.busy() {
            assert!(Instant::now() < deadline, "the worker did not finish");
            std::thread::yield_now();
        }
    }

    fn hits(app: &App, query: &str) -> Vec<NodeId> {
        settle(app);
        app.search.hits(query).ids.clone()
    }

    fn stored(app: &mut App, id: NodeId, text: &str) {
        let tab = app.nodes.get(&id).and_then(|n| n.as_tab()).unwrap();
        let rec = TextRecord { id, url: tab.url.clone(), title: tab.title.clone(), text: text.into(), at: 1 };
        app.search.texts.put(rec);
    }

    #[test]
    fn renaming_a_label_reindexes_its_cards_only() {
        let mut app = app_on(&crate::store::testutil::temp_dir("label-index").join("session.mnf"), 1);
        let (c1, c2) = (add_note(&mut app, "x"), add_note(&mut app, "y"));
        let (a, b) = (app.create_label("a", None).unwrap(), app.create_label("b", None).unwrap());
        let before = (app.index_print(DocKey::Node(c1)), app.index_print(DocKey::Node(c2)));
        app.set_label(&[c1], a, true);
        app.set_label(&[c2], b, true);
        let (p1, p2) = (app.index_print(DocKey::Node(c1)), app.index_print(DocKey::Node(c2)));
        assert_ne!(p1, before.0, "gaining a label changes the print");
        app.rename_label(a, "renamed");
        assert_ne!(app.index_print(DocKey::Node(c1)), p1);
        assert_eq!(app.index_print(DocKey::Node(c2)), p2);
        assert!(app.index_text(DocKey::Node(c1)).contains("renamed"));
    }

    #[test]
    fn a_tabs_stored_text_is_found_after_a_sync() {
        let mut app = App::default();
        let (id, _peer) = live_tab(&mut app, ViewKind::Servo, "https://a.test/");
        stored(&mut app, id, "the borrow checker");
        app.sync_search_index(Instant::now());
        assert_eq!(hits(&app, "borrow"), [id]);
    }

    #[test]
    fn an_ephemeral_jar_turning_on_purges_its_records() {
        let mut app = App::default();
        let t0 = Instant::now();
        let (id, _peer) = live_tab(&mut app, ViewKind::Servo, "https://a.test/");
        let jar = add_jar(&mut app, "Tmp");
        app.set_jar_edge(id, Some(jar));
        stored(&mut app, id, "secret words");
        app.sync_search_index(t0);
        assert_eq!(hits(&app, "secret"), [id]);
        app.set_jar_ephemeral(jar, true);
        app.sync_search_index(t0 + SYNC_EVERY);
        assert!(hits(&app, "secret").is_empty());
        assert!(app.search.texts.get(id).is_none(), "the stored text is gone too");
    }

    #[test]
    fn a_purge_also_clears_a_record_the_startup_index_holds() {
        let mut app = App::default();
        let (id, _peer) = live_tab(&mut app, ViewKind::Servo, "https://a.test/");
        let jar = add_jar(&mut app, "Tmp");
        app.set_jar_edge(id, Some(jar));
        stored(&mut app, id, "secret words");
        // As after a restart: the index was built from the text log before `sync` ever ran.
        let mut texts = crate::search::text_store::TextStore::open(None);
        let tab = app.nodes.get(&id).and_then(|n| n.as_tab()).unwrap();
        texts.put(TextRecord {
            id,
            url: tab.url.clone(),
            title: tab.title.clone(),
            text: "secret words".into(),
            at: 1,
        });
        app.search = crate::search::Search::from_texts(texts);
        assert_eq!(hits(&app, "secret"), [id]);
        app.set_jar_ephemeral(jar, true);
        app.sync_search_index(Instant::now());
        assert!(hits(&app, "secret").is_empty());
    }

    #[test]
    fn a_closed_tabs_text_is_purged_when_its_jar_turns_ephemeral() {
        let mut app = app_on(&crate::store::testutil::temp_dir("closed-ephemeral").join("session.mnf"), 1);
        let t0 = Instant::now();
        let (id, _peer) = live_tab(&mut app, ViewKind::Servo, "https://a.test/");
        let jar = add_jar(&mut app, "Tmp");
        app.set_jar_edge(id, Some(jar));
        app.save_session();
        stored(&mut app, id, "secret words");
        app.sync_search_index(t0);
        app.close_node(id);
        assert!(!app.nodes.contains_key(&id), "the tab is closed");
        app.set_jar_ephemeral(jar, true);
        app.sync_search_index(t0 + SYNC_EVERY);
        assert!(app.search.texts.get(id).is_none(), "the closed tab's stored text is gone");
        assert!(hits(&app, "secret").is_empty());
    }

    #[test]
    fn a_node_the_document_no_longer_holds_loses_its_text_and_hits() {
        let mut app = App::default();
        let t0 = Instant::now();
        let (id, _peer) = live_tab(&mut app, ViewKind::Servo, "https://a.test/");
        stored(&mut app, id, "ghost words");
        app.sync_search_index(t0);
        assert_eq!(hits(&app, "ghost"), [id]);
        app.nodes.remove(&id);
        app.doc.mirror.forget_node(id);
        app.sync_search_index(t0 + SYNC_EVERY);
        assert!(hits(&app, "ghost").is_empty());
        assert!(app.search.texts.get(id).is_none());
    }

    #[test]
    fn a_note_edit_reindexes_after_the_debounce() {
        let mut app = App::default();
        let t0 = Instant::now();
        let id = add_note(&mut app, "alpha");
        app.sync_search_index(t0);
        assert_eq!(hits(&app, "alpha"), [id]);
        app.edit_text(id, "omega");
        app.sync_search_index(t0 + SYNC_EVERY / 2);
        assert!(hits(&app, "omega").is_empty(), "inside the debounce");
        app.sync_search_index(t0 + SYNC_EVERY);
        assert_eq!(hits(&app, "omega"), [id]);
        assert!(hits(&app, "alpha").is_empty());
    }

    #[test]
    fn clear_empties_store_and_index() {
        let mut app = App::default();
        let t0 = Instant::now();
        let (id, _peer) = live_tab(&mut app, ViewKind::Servo, "https://a.test/");
        stored(&mut app, id, "remember me");
        app.sync_search_index(t0);
        assert_eq!(hits(&app, "remember"), [id]);
        app.search.clear();
        assert!(app.search.texts.get(id).is_none());
        assert!(hits(&app, "remember").is_empty());
        app.sync_search_index(t0 + SYNC_EVERY);
        assert!(hits(&app, "remember").is_empty(), "the next sync brings back titles and urls, not page text");
    }

    #[test]
    fn history_entries_are_indexed_apart_from_cards() {
        let mut app = App::default();
        app.history.push(crate::history::HistoryEntry { title: "Rust blog".into(), url: "https://blog.test/".into() });
        app.sync_search_index(Instant::now());
        settle(&app);
        assert_eq!(app.search.service.search("blog"), [(DocKey::History(0), app.search.service.search("blog")[0].1)]);
        assert!(app.search.hits("blog").ids.is_empty(), "a history entry is not a card");
    }
}
