//! The omnibar's rows: what each source offers for a typed text, in the fixed order the dropdown shows them. Pure over
//! a `Corpus`, so the tests need no `App`.

use crate::store::model::NodeId;
use crate::App;

/// Rows each section offers at most; the dimming set (`App::omni_matches`) is not capped.
pub(crate) const ROW_CAP: usize = 5;

/// `Query` (Graph, Settings, Ctrl+K) starts empty and preselects the first row; `Address` (Item) starts as the tab's
/// url and preselects none, so Enter navigates to what is typed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Query,
    Address,
}

#[rustfmt::skip]
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Row {
    Card { id: NodeId, label: String, url: String, jar: Option<String>, snippet: Option<String> },
    History { url: String, title: String },
    Setting { id: &'static str, title: &'static str, section: &'static str },
    /// The typed text: a url to open, or a search with the default engine.
    Typed { text: String },
    /// A search suggestion from the engine.
    Suggestion { text: String },
    /// An action the typed words name.
    Command(&'static Command),
}

/// An action the omnibar offers by name: what it is called, the phrases whose prefix (two letters or more) offers
/// it, and what it does. A new command is one more entry in `COMMANDS`.
#[derive(Debug)]
pub(crate) struct Command {
    pub(crate) title: &'static str,
    phrases: &'static [&'static str],
    pub(crate) run: fn(&mut App),
}

/// Titles are unique, so they are the identity.
impl PartialEq for Command {
    fn eq(&self, other: &Self) -> bool {
        self.title == other.title
    }
}

pub(crate) static SUGGEST_GROUPS: Command = Command {
    title: "Suggest groups",
    phrases: &["suggest groups", "group by topic"],
    run: |app| {
        app.suggest_groups();
    },
};
pub(crate) static ARRANGE: Command = Command {
    title: "Arrange by topic",
    phrases: &["arrange by topic", "lay out by topic"],
    run: App::arrange_by_topic,
};
pub(crate) static COMMANDS: [&Command; 2] = [&SUGGEST_GROUPS, &ARRANGE];

/// One open card the omnibar may offer.
pub(crate) struct CardInfo {
    pub(crate) id: NodeId,
    pub(crate) label: String,
    pub(crate) url: String,
    pub(crate) jar: Option<String>,
    /// The names of the card's live labels.
    pub(crate) labels: Vec<String>,
    pub(crate) closed: bool,
}

pub(crate) trait Corpus {
    fn cards(&self) -> Vec<CardInfo>;
    /// `(title, url)`, newest first.
    fn history(&self) -> Vec<(String, String)>;
    /// The cards whose page text (or other document) holds every word of `text`, best first.
    fn text_hits(&self, text: &str) -> Vec<NodeId>;
    /// About a line of `id`'s document around `text`.
    fn snippet(&self, id: NodeId, text: &str) -> Option<String>;
    /// Whether commands are offered (not in the address bar, where the text is a url or a search).
    fn offers_commands(&self) -> bool;
}

/// Whether `text` (trimmed) is a case-insensitive substring of any of `haystacks`.
fn contains(text: &str, haystacks: &[&str]) -> bool {
    let text = text.trim().to_lowercase();
    haystacks.iter().any(|h| h.to_lowercase().contains(&text))
}

/// `text` split into its `#label` words (each as the `name_key` of what follows the `#`) and the rest, the plain words
/// joined by a space.
pub(crate) fn split_query(text: &str) -> (Vec<String>, String) {
    let (mut tags, mut plain) = (Vec::new(), Vec::new());
    for word in text.split_whitespace() {
        match word.strip_prefix('#') {
            Some(rest) => tags.push(crate::names::name_key(rest)),
            None => plain.push(word),
        }
    }
    (tags, plain.join(" "))
}

/// Whether a card with label `names` has, for every one of `tags`, a label whose key starts with it (a bare `#` asks
/// for any label).
pub(crate) fn labels_match(tags: &[String], names: &[String]) -> bool {
    tags.iter().all(|tag| names.iter().any(|n| crate::names::name_key(n).starts_with(tag.as_str())))
}

/// The rows for `text`: open cards, history, settings, the typed text, then search `suggestions`, each section at
/// most `ROW_CAP`. An empty text offers the first open cards and nothing else (the Ctrl+K list before a keystroke).
pub(crate) fn rows(corpus: &impl Corpus, text: &str, suggestions: &[String]) -> Vec<Row> {
    let text = text.trim();
    let (tags, plain) = split_query(text);
    let cards = corpus.cards();
    let mut out: Vec<Row> = cards
        .iter()
        .filter(|c| !c.closed && labels_match(&tags, &c.labels))
        .filter(|c| {
            let mut haystacks: Vec<&str> = vec![&c.label, &c.url];
            haystacks.extend(c.labels.iter().map(String::as_str));
            contains(&plain, &haystacks)
        })
        .take(ROW_CAP)
        .map(|c| Row::Card { id: c.id, label: c.label.clone(), url: c.url.clone(), jar: c.jar.clone(), snippet: None })
        .collect();
    if !plain.is_empty() {
        for id in corpus.text_hits(&plain) {
            if out.len() >= ROW_CAP {
                break;
            }
            let Some(c) = cards.iter().find(|c| c.id == id && !c.closed && labels_match(&tags, &c.labels)) else {
                continue;
            };
            if out.iter().any(|r| matches!(r, Row::Card { id: seen, .. } if *seen == id)) {
                continue;
            }
            let snippet = corpus.snippet(id, &plain);
            out.push(Row::Card { id, label: c.label.clone(), url: c.url.clone(), jar: c.jar.clone(), snippet });
        }
    }
    // Only `#label` words: the cards with those labels, and nothing else to offer.
    if plain.is_empty() {
        return out;
    }
    out.extend(
        corpus
            .history()
            .into_iter()
            .filter(|(title, url)| contains(text, &[title, url]))
            .take(ROW_CAP)
            .map(|(title, url)| Row::History { url, title }),
    );
    out.extend(crate::settings_rows::matching(text).into_iter().take(ROW_CAP).map(|r| Row::Setting {
        id: r.id,
        title: r.title,
        section: r.section,
    }));
    if corpus.offers_commands() && text.chars().count() >= 2 {
        let lower = text.to_lowercase();
        out.extend(COMMANDS.into_iter().filter(|c| c.phrases.iter().any(|p| p.starts_with(&lower))).map(Row::Command));
    }
    out.push(Row::Typed { text: text.to_string() });
    out.extend(suggestions.iter().take(ROW_CAP).map(|s| Row::Suggestion { text: s.clone() }));
    out
}

/// The row Enter picks before the user moves: the first in `Query` mode, none in `Address` mode.
pub(crate) fn default_highlight(mode: Mode, rows: &[Row]) -> Option<usize> {
    (mode == Mode::Query && !rows.is_empty()).then_some(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::model::ActorId;

    fn id(n: u64) -> NodeId {
        NodeId { actor: ActorId(1), counter: n }
    }

    #[derive(Default)]
    struct Fake {
        cards: Vec<(u64, &'static str, &'static str, bool)>,
        history: Vec<(&'static str, &'static str)>,
        /// `(card, snippet)` for the cards a page-text search finds.
        text_hits: Vec<(u64, &'static str)>,
        commands: bool,
    }

    impl Corpus for Fake {
        fn cards(&self) -> Vec<CardInfo> {
            self.cards
                .iter()
                .map(|&(n, label, url, closed)| CardInfo {
                    id: id(n),
                    label: label.into(),
                    url: url.into(),
                    jar: None,
                    labels: Vec::new(),
                    closed,
                })
                .collect()
        }
        fn history(&self) -> Vec<(String, String)> {
            self.history.iter().map(|&(t, u)| (t.into(), u.into())).collect()
        }
        fn text_hits(&self, _text: &str) -> Vec<NodeId> {
            self.text_hits.iter().map(|&(n, _)| id(n)).collect()
        }
        fn snippet(&self, card: NodeId, _text: &str) -> Option<String> {
            self.text_hits.iter().find(|&&(n, _)| id(n) == card).map(|&(_, s)| s.to_owned())
        }
        fn offers_commands(&self) -> bool {
            self.commands
        }
    }

    fn kinds(rows: &[Row]) -> Vec<&'static str> {
        rows.iter()
            .map(|r| match r {
                Row::Card { .. } => "card",
                Row::History { .. } => "history",
                Row::Setting { .. } => "setting",
                Row::Typed { .. } => "typed",
                Row::Suggestion { .. } => "suggestion",
                Row::Command(_) => "command",
            })
            .collect()
    }

    #[test]
    fn a_command_is_offered_for_a_prefix_of_its_phrase_and_only_where_commands_are() {
        let on = Fake { commands: true, ..Default::default() };
        let offered = |fake: &Fake, text| rows(fake, text, &[]).contains(&Row::Command(&SUGGEST_GROUPS));
        for text in ["su", "Suggest gr", "group by", "GRO"] {
            assert!(offered(&on, text), "{text}");
        }
        for text in ["s", "rust", "suggestions"] {
            assert!(!offered(&on, text), "{text}");
        }
        assert!(!offered(&Fake::default(), "suggest"), "the address bar offers none");
        let arrange = |text| rows(&on, text, &[]).contains(&Row::Command(&ARRANGE));
        assert!(arrange("arrange by") && arrange("ARR") && !arrange("su"));
    }

    #[test]
    fn card_rows_come_before_history_before_settings_before_typed() {
        let fake = Fake {
            cards: vec![(1, "Rust book", "https://doc.test/", false)],
            history: vec![("Rust blog", "https://blog.test/")],
            ..Default::default()
        };
        let rows = rows(&fake, "e", &["engine room".into()]);
        let mut order = kinds(&rows);
        order.dedup();
        assert_eq!(order, ["card", "history", "setting", "typed", "suggestion"]);
    }

    #[test]
    fn each_section_is_capped_at_row_cap() {
        let cards = (0..ROW_CAP as u64 + 3).map(|n| (n, "a card", "https://a.test/", false)).collect();
        let fake = Fake { cards, ..Default::default() };
        let shown = rows(&fake, "a", &[]);
        assert_eq!(shown.iter().filter(|r| matches!(r, Row::Card { .. })).count(), ROW_CAP);
    }

    #[test]
    fn query_mode_preselects_the_first_row_and_address_mode_preselects_none() {
        let fake = Fake { cards: vec![(1, "Rust", "https://r.test/", false)], ..Default::default() };
        let shown = rows(&fake, "rust", &[]);
        assert_eq!(default_highlight(Mode::Query, &shown), Some(0));
        assert_eq!(default_highlight(Mode::Address, &shown), None);
        assert_eq!(default_highlight(Mode::Query, &[]), None, "nothing to pick");
    }

    #[test]
    fn substring_of_title_or_url_matches_a_card_case_insensitively() {
        let fake = Fake { cards: vec![(1, "Rust Book", "https://x.test/Guide", false)], ..Default::default() };
        let cards = |query| rows(&fake, query, &[]).into_iter().filter(|r| matches!(r, Row::Card { .. })).count();
        for query in ["rust", "BOOK", "guide"] {
            assert_eq!(cards(query), 1, "{query}");
        }
        assert_eq!(cards("zebra"), 0);
    }

    #[test]
    fn closed_cards_are_not_offered() {
        let fake = Fake { cards: vec![(1, "Rust", "https://r.test/", true)], ..Default::default() };
        assert!(!rows(&fake, "rust", &[]).iter().any(|r| matches!(r, Row::Card { .. })));
    }

    #[test]
    fn settings_rows_appear_for_a_settings_word_and_not_for_a_page_word() {
        let fake = Fake::default();
        let settings = |text| rows(&fake, text, &[]).into_iter().filter(|r| matches!(r, Row::Setting { .. })).count();
        assert!(settings("engine") > 0);
        assert_eq!(settings("rust"), 0);
    }

    #[test]
    fn an_empty_text_offers_only_the_open_cards() {
        let fake = Fake {
            cards: vec![(1, "Rust", "https://r.test/", false)],
            history: vec![("Blog", "https://b.test/")],
            ..Default::default()
        };
        assert_eq!(kinds(&rows(&fake, "  ", &["x".into()])), ["card"]);
    }

    #[test]
    fn a_page_text_word_finds_the_card_with_a_snippet() {
        let fake = Fake {
            cards: vec![(1, "Rust book", "https://doc.test/", false), (2, "Other", "https://o.test/", false)],
            text_hits: vec![(2, "…the borrow checker…")],
            ..Default::default()
        };
        let shown = rows(&fake, "borrow", &[]);
        assert_eq!(
            shown.first(),
            Some(&Row::Card {
                id: id(2),
                label: "Other".into(),
                url: "https://o.test/".into(),
                jar: None,
                snippet: Some("…the borrow checker…".into()),
            })
        );
    }

    #[test]
    fn a_closed_card_text_hit_is_not_offered() {
        let fake = Fake {
            cards: vec![(2, "Other", "https://o.test/", true)],
            text_hits: vec![(2, "…borrow…"), (9, "…borrow…")],
            ..Default::default()
        };
        assert!(!rows(&fake, "borrow", &[]).iter().any(|r| matches!(r, Row::Card { .. })));
    }

    #[test]
    fn a_card_matching_by_title_and_by_text_is_offered_once() {
        let fake = Fake {
            cards: vec![(1, "Borrow book", "https://b.test/", false)],
            text_hits: vec![(1, "…borrow…")],
            ..Default::default()
        };
        assert_eq!(rows(&fake, "borrow", &[]).iter().filter(|r| matches!(r, Row::Card { .. })).count(), 1);
    }
}
