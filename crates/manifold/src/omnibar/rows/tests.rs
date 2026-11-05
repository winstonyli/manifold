use super::*;
use crate::store::model::ActorId;

fn id(n: u64) -> NodeId {
    NodeId { actor: ActorId(1), counter: n }
}

#[derive(Default)]
struct Fake {
    cards: Vec<(u64, &'static str, &'static str, bool)>,
    /// `(id, label, label names)` of the live cards that are not tabs.
    others: Vec<(u64, &'static str, Vec<&'static str>)>,
    history: Vec<(&'static str, &'static str)>,
    /// `(card, snippet)` for the cards a page-text search finds.
    text_hits: Vec<(u64, &'static str)>,
    commands: bool,
}

fn other(n: u64, label: &'static str) -> (u64, &'static str, Vec<&'static str>) {
    (n, label, Vec::new())
}

fn other_ids(shown: Vec<Row>) -> Vec<NodeId> {
    shown.into_iter().filter_map(|r| if let Row::Other { id, .. } = r { Some(id) } else { None }).collect()
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
    fn others(&self) -> Vec<OtherInfo> {
        self.others
            .iter()
            .map(|(n, label, names)| OtherInfo {
                id: id(*n),
                label: (*label).into(),
                glyph: "g",
                labels: names.iter().map(|s| (*s).into()).collect(),
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
            Row::Other { .. } => "other",
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
    let tidy = |text| rows(&on, text, &[]).contains(&Row::Command(&TIDY));
    assert!(tidy("tidy") && tidy("TID") && !tidy("su"));
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
fn other_rows_sit_after_tab_rows_and_before_history() {
    let fake = Fake {
        cards: vec![(1, "Rust book", "https://doc.test/", false)],
        others: vec![other(5, "Note: rust ideas")],
        history: vec![("Rust blog", "https://blog.test/")],
        ..Default::default()
    };
    let mut order = kinds(&rows(&fake, "rust", &[]));
    order.dedup();
    order.truncate(3);
    assert_eq!(order, ["card", "other", "history"]);
}

#[test]
fn a_card_matches_by_label_word_by_label_name_and_by_hash_label() {
    let fake =
        Fake { others: vec![(5, "Note: milk", vec!["Urgent"]), other(6, "Group: Cheese")], ..Default::default() };
    let ids = |text: &str| other_ids(rows(&fake, text, &[]));
    assert_eq!(ids("milk"), [id(5)]);
    assert_eq!(ids("urgent"), [id(5)], "a label name matches");
    assert_eq!(ids("#urg"), [id(5)], "only the tag filters");
    assert!(ids("#urg chee").is_empty(), "a tag and a plain word are both needed");
    assert_eq!(ids("group"), [id(6)], "the kind word is part of the label");
    assert!(ids("zebra").is_empty());
}

#[test]
fn other_rows_are_capped_and_an_empty_text_offers_none() {
    let others = (0..ROW_CAP as u64 + 3).map(|n| other(n, "Note: a")).collect();
    let fake = Fake { others, ..Default::default() };
    assert_eq!(other_ids(rows(&fake, "a", &[])).len(), ROW_CAP);
    assert!(rows(&fake, "  ", &[]).is_empty());
}

#[test]
fn a_text_hit_on_a_non_tab_card_carries_its_snippet_and_is_listed_once() {
    let fake = Fake {
        others: vec![other(5, "Note: other words"), other(6, "Note: borrow list")],
        text_hits: vec![(5, "…the borrow checker…"), (6, "…borrow…")],
        ..Default::default()
    };
    let shown: Vec<Row> = rows(&fake, "borrow", &[]).into_iter().filter(|r| matches!(r, Row::Other { .. })).collect();
    assert_eq!(shown.len(), 2, "the label match and the text hit, not three");
    assert_eq!(shown[0], Row::Other { id: id(6), label: "Note: borrow list".into(), glyph: "g", snippet: None });
    assert_eq!(
        shown[1],
        Row::Other {
            id: id(5),
            label: "Note: other words".into(),
            glyph: "g",
            snippet: Some("…the borrow checker…".into())
        }
    );
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
