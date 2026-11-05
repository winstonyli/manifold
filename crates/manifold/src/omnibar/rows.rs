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
    /// A live card that is not a tab (a note, group, download, image, PDF or Json): chosen, it is shown in the Graph.
    Other { id: NodeId, label: String, glyph: &'static str, snippet: Option<String> },
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
pub(crate) static TIDY: Command =
    Command { title: "Tidy", phrases: &["tidy", "tidy dropped cards"], run: App::tidy_level };
pub(crate) static COMMANDS: [&Command; 2] = [&SUGGEST_GROUPS, &TIDY];

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

/// One live card that is not a tab the omnibar may offer.
pub(crate) struct OtherInfo {
    pub(crate) id: NodeId,
    /// What the card is called (`App::accessible_name`): "Note: first line", "Group: Research".
    pub(crate) label: String,
    /// The icon of its kind.
    pub(crate) glyph: &'static str,
    /// The names of the card's live labels.
    pub(crate) labels: Vec<String>,
}

pub(crate) trait Corpus {
    fn cards(&self) -> Vec<CardInfo>;
    /// The live cards that are not tabs, in document order.
    fn others(&self) -> Vec<OtherInfo>;
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

/// The rows for the live cards that are not tabs: the label matches, then the page-text `hits`, at most `ROW_CAP` in
/// all. Nothing for an empty `text`.
fn other_rows(corpus: &impl Corpus, text: &str, tags: &[String], plain: &str, hits: &[NodeId]) -> Vec<Row> {
    if text.is_empty() {
        return Vec::new();
    }
    let others = corpus.others();
    let mut out: Vec<Row> = others
        .iter()
        .filter(|o| labels_match(tags, &o.labels))
        .filter(|o| {
            let mut haystacks: Vec<&str> = vec![&o.label];
            haystacks.extend(o.labels.iter().map(String::as_str));
            contains(plain, &haystacks)
        })
        .take(ROW_CAP)
        .map(|o| Row::Other { id: o.id, label: o.label.clone(), glyph: o.glyph, snippet: None })
        .collect();
    for &id in hits {
        if out.len() >= ROW_CAP {
            break;
        }
        let Some(o) = others.iter().find(|o| o.id == id && labels_match(tags, &o.labels)) else { continue };
        if out.iter().any(|r| matches!(r, Row::Other { id: seen, .. } if *seen == id)) {
            continue;
        }
        out.push(Row::Other { id, label: o.label.clone(), glyph: o.glyph, snippet: corpus.snippet(id, plain) });
    }
    out
}

/// The rows for `text`: open cards, the other cards, history, settings, the typed text, then search `suggestions`, each
/// section at most `ROW_CAP`. An empty text offers the first open cards and nothing else (the Ctrl+K list before a
/// keystroke).
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
    let hits = if plain.is_empty() { Vec::new() } else { corpus.text_hits(&plain) };
    for &id in &hits {
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
    out.extend(other_rows(corpus, text, &tags, &plain, &hits));
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
    out.extend(crate::settings::rows::matching(text).into_iter().take(ROW_CAP).map(|r| Row::Setting {
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
mod tests;
