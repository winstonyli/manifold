//! Name rules (spec `docs/superpowers/specs/2026-10-03-name-rules-design.md`). There is one input rule,
//! `clean_name`, applied when the user commits a group, jar, session or bookmark folder name, and one display rule,
//! `display_name`, for every stored name that is drawn. Stored names are never rewritten. A name from sync, an import
//! or older data is shown through the display rule, which reveals a hidden character as a placeholder.

// The callers arrive in the next commits (name-rules plan, Tasks 2-4); Task 5 removes this.
#![allow(dead_code)]

use icu_properties::props::{BinaryProperty, EnumeratedProperty, ExtendedPictographic, GeneralCategory};
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

/// The most grapheme clusters a name keeps (input) or shows (display, where a placeholder counts as one).
pub(crate) const NAME_CAP: usize = 64;
/// What a name with nothing left to show is drawn as.
pub(crate) const UNTITLED: &str = "Untitled";
/// Every character a rich placeholder (`placeholder`) can use. `tests` checks that the monospace font has them all.
#[cfg(test)]
const PLACEHOLDER_CHARS: &str = "⟦⟧0123456789ABCDEF";

const ZWJ: char = '\u{200D}';
const KEYCAP: char = '\u{20E3}';
const BLACK_FLAG: char = '\u{1F3F4}';
const CANCEL_TAG: char = '\u{E007F}';

/// A piece of a name as displayed: text drawn as it is, or a hidden character drawn as a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameSeg<'a> {
    Plain(&'a str),
    Hidden(char),
}

/// What the name rules do with one character of a name, in its place (`classify`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Keep,
    /// Whitespace, kept as one plain space per run.
    Space,
    /// Dropped by `clean_name`, a placeholder in `display_name`.
    Hidden,
}

/// A character that is hidden outside an emoji sequence: `Cc`, `Cf`, `Cs`, `Co`, `Cn`, and the variation selectors.
fn is_hidden(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
        || matches!(
            GeneralCategory::for_char(c),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
        )
}

/// Whitespace that a name keeps as one plain space: `Zs` (which includes the plain space), `Zl` and `Zp`.
fn is_space(c: char) -> bool {
    matches!(
        GeneralCategory::for_char(c),
        GeneralCategory::SpaceSeparator | GeneralCategory::LineSeparator | GeneralCategory::ParagraphSeparator
    )
}

/// An emoji (Extended_Pictographic) that an emoji sequence can start from or join to. The unassigned code points
/// that Extended_Pictographic reserves are hidden, so they are not emoji here.
fn is_pict(c: char) -> bool {
    ExtendedPictographic::for_char(c) && GeneralCategory::for_char(c) != GeneralCategory::Unassigned
}

/// The byte length of the subdivision-flag tags at the start of `rest`, the text after a 🏴: one or more tag
/// characters U+E0020-E007E, then the cancel tag U+E007F. 0 when `rest` does not start with such a run.
fn flag_tags_len(rest: &str) -> usize {
    let spec: usize = rest.chars().take_while(|c| ('\u{E0020}'..='\u{E007E}').contains(c)).map(char::len_utf8).sum();
    if spec > 0 && rest[spec..].starts_with(CANCEL_TAG) {
        spec + CANCEL_TAG.len_utf8()
    } else {
        0
    }
}

/// Each character of `s` with its byte offset and its class. A hidden character (`is_hidden`) is `Hidden` except
/// inside an emoji sequence, which is kept whole:
/// - a zero-width joiner between two emoji, the first of which may carry skin-tone modifiers and selectors
///   (👩‍💻, 👩🏽‍💻, ❤️‍🔥; UAX #29 rule GB11);
/// - U+FE0E or U+FE0F directly after an emoji (☺️), or between a keycap base (0-9, `#`, `*`) and U+20E3 (1️⃣);
/// - the tags of a subdivision flag: 🏴, tags, U+E007F (🏴󠁧󠁢󠁥󠁮󠁧󠁿).
///
/// Skin-tone modifiers (U+1F3FB-1F3FF) and U+20E3 are not hidden characters to begin with.
fn classify(s: &str) -> impl Iterator<Item = (usize, char, Class)> + '_ {
    // After an emoji and only its modifiers, selectors or tags since: where a joiner may follow.
    let mut pict = false;
    let mut prev: Option<char> = None;
    // Where the tags of the last subdivision flag end (a byte offset).
    let mut tags_end = 0;
    s.char_indices().map(move |(i, c)| {
        let after = i + c.len_utf8();
        let next = s[after..].chars().next();
        let kept = |ok: bool| if ok { Class::Keep } else { Class::Hidden };
        let class = if i < tags_end {
            Class::Keep
        } else if c == ZWJ {
            let ok = pict && next.is_some_and(is_pict);
            pict = false;
            kept(ok)
        } else if matches!(c, '\u{FE0E}' | '\u{FE0F}') {
            let keycap = matches!(prev, Some('0'..='9' | '#' | '*')) && next == Some(KEYCAP);
            let ok = prev.is_some_and(is_pict) || keycap;
            pict &= ok;
            kept(ok)
        } else if ('\u{1F3FB}'..='\u{1F3FF}').contains(&c) {
            Class::Keep
        } else if is_pict(c) {
            pict = true;
            if c == BLACK_FLAG {
                tags_end = after + flag_tags_len(&s[after..]);
            }
            Class::Keep
        } else {
            pict = false;
            if is_hidden(c) {
                Class::Hidden
            } else if is_space(c) {
                Class::Space
            } else {
                Class::Keep
            }
        };
        prev = Some(c);
        (i, c, class)
    })
}

/// Whether `clean_name` would return `s` unchanged.
fn is_clean(s: &str) -> bool {
    !s.starts_with(' ')
        && !s.ends_with(' ')
        && !s.contains("  ")
        && classify(s).all(|(_, c, class)| class == Class::Keep || c == ' ')
        && (s.len() <= NAME_CAP || s.graphemes(true).nth(NAME_CAP).is_none())
}

/// What a cut at a cluster boundary must not leave at the end: a space, or a joiner (should the segmenter's emoji
/// data be older than `is_pict`'s, it could end a cluster at a joiner that `classify` keeps).
const CUT_TRIM: [char; 2] = [' ', ZWJ];

/// The input rule: `s` without hidden characters (`classify`: emoji sequences keep theirs), with each whitespace run
/// as one plain space, trimmed, and cut to `NAME_CAP` grapheme clusters. It borrows `s` when `s` is already clean,
/// and is idempotent.
pub(crate) fn clean_name(s: &str) -> Cow<'_, str> {
    if is_clean(s) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for (_, c, class) in classify(s) {
        match class {
            Class::Hidden => {}
            Class::Space => space = !out.is_empty(),
            Class::Keep => {
                if std::mem::take(&mut space) {
                    out.push(' ');
                }
                out.push(c);
            }
        }
    }
    if let Some((cut, _)) = out.grapheme_indices(true).nth(NAME_CAP) {
        out.truncate(cut);
        out.truncate(out.trim_end_matches(CUT_TRIM).len());
    }
    Cow::Owned(out)
}

/// The display rule: `s` as segments, with each hidden character (`classify`) a `Hidden`, each whitespace run one
/// plain space, trimmed, and at most `NAME_CAP` units (`units`). It gives `fallback` when nothing is left. A clean
/// name is one segment borrowing all of `s`: the `Vec` is the only allocation, and the name is not copied.
pub(crate) fn display_name<'a>(s: &'a str, fallback: &'a str) -> Vec<NameSeg<'a>> {
    if is_clean(s) {
        return vec![NameSeg::Plain(if s.is_empty() { fallback } else { s })];
    }
    let mut segs = Vec::new();
    let mut run: Option<usize> = None;
    let mut space = false;
    for (i, c, class) in classify(s) {
        if class != Class::Keep {
            if let Some(start) = run.take() {
                segs.push(NameSeg::Plain(&s[start..i]));
            }
            if class == Class::Space {
                space = !segs.is_empty();
                continue;
            }
        }
        if std::mem::take(&mut space) {
            segs.push(NameSeg::Plain(" "));
        }
        if class == Class::Hidden {
            segs.push(NameSeg::Hidden(c));
        } else if run.is_none() {
            run = Some(i);
        }
    }
    if let Some(start) = run {
        segs.push(NameSeg::Plain(&s[start..]));
    }
    let segs = take_units(&segs, NAME_CAP);
    if segs.is_empty() {
        vec![NameSeg::Plain(fallback)]
    } else {
        segs
    }
}

/// How long `segs` is against a cap: grapheme clusters, with a `Hidden` counting as one.
pub(crate) fn units(segs: &[NameSeg]) -> usize {
    segs.iter()
        .map(|s| match s {
            NameSeg::Plain(p) => p.graphemes(true).count(),
            NameSeg::Hidden(_) => 1,
        })
        .sum()
}

/// The first `max` units of `segs` (`units`), cut at a cluster boundary, with no trailing space.
pub(crate) fn take_units<'a>(segs: &[NameSeg<'a>], max: usize) -> Vec<NameSeg<'a>> {
    let mut out = Vec::new();
    let mut left = max;
    for &seg in segs {
        if left == 0 {
            break;
        }
        match seg {
            NameSeg::Hidden(_) => {
                out.push(seg);
                left -= 1;
            }
            NameSeg::Plain(p) => match p.grapheme_indices(true).nth(left) {
                Some((cut, _)) => {
                    out.push(NameSeg::Plain(&p[..cut]));
                    left = 0;
                }
                None => {
                    left -= p.graphemes(true).count();
                    out.push(seg);
                }
            },
        }
    }
    if let Some(NameSeg::Plain(p)) = out.last_mut() {
        let text = *p;
        *p = text.trim_end_matches(CUT_TRIM);
    }
    out.retain(|s| *s != NameSeg::Plain(""));
    out
}

/// `segs` cut to `max - 1` units plus "…", or `None` when `segs` already fits in `max`.
pub(crate) fn shorten<'a>(segs: &[NameSeg<'a>], max: usize) -> Option<Vec<NameSeg<'a>>> {
    (units(segs) > max).then(|| {
        let mut short = take_units(segs, max.saturating_sub(1));
        short.push(NameSeg::Plain("…"));
        short
    })
}

/// The plain-text form, for tooltips, confirm texts and the screen reader, where a hidden character is `<U+200B>`.
pub(crate) fn to_plain(segs: &[NameSeg]) -> String {
    let mut out = String::new();
    for seg in segs {
        match *seg {
            NameSeg::Plain(p) => out.push_str(p),
            NameSeg::Hidden(c) => out.push_str(&format!("<U+{:04X}>", c as u32)),
        }
    }
    out
}

/// The rich form of a hidden character: `⟦200B⟧`, in uppercase hex of at least four digits.
pub(crate) fn placeholder(c: char) -> String {
    format!("⟦{:04X}⟧", c as u32)
}

pub(crate) fn has_hidden(segs: &[NameSeg]) -> bool {
    segs.iter().any(|s| matches!(s, NameSeg::Hidden(_)))
}

/// What a hover says about the hidden characters in `segs`, each named once, or `None` when there are none.
pub(crate) fn hidden_hint(segs: &[NameSeg]) -> Option<String> {
    let mut seen: Vec<char> = Vec::new();
    for seg in segs {
        if let NameSeg::Hidden(c) = *seg {
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
    }
    let named: Vec<String> = seen.iter().map(|&c| format!("U+{:04X} {}", c as u32, char_name(c))).collect();
    (!named.is_empty()).then(|| format!("Hidden characters: {}", named.join(", ")))
}

/// `base`, then `hidden_hint` on its own line when there is one. An empty `base` gives the hint alone.
pub(crate) fn with_hint(base: &str, segs: &[NameSeg]) -> String {
    match (base.is_empty(), hidden_hint(segs)) {
        (_, None) => base.to_string(),
        (true, Some(hint)) => hint,
        (false, Some(hint)) => format!("{base}\n{hint}"),
    }
}

/// A short name for a hidden character, falling back to its kind.
fn char_name(c: char) -> &'static str {
    match c {
        '\t' => "tab",
        '\n' => "line feed",
        '\r' => "carriage return",
        '\0' => "null",
        '\u{00AD}' => "soft hyphen",
        '\u{200B}' => "zero width space",
        '\u{200C}' => "zero width non-joiner",
        '\u{200D}' => "zero width joiner",
        '\u{200E}' => "left-to-right mark",
        '\u{200F}' => "right-to-left mark",
        '\u{202A}' => "left-to-right embedding",
        '\u{202B}' => "right-to-left embedding",
        '\u{202C}' => "pop directional formatting",
        '\u{202D}' => "left-to-right override",
        '\u{202E}' => "right-to-left override",
        '\u{2060}' => "word joiner",
        '\u{2066}' => "left-to-right isolate",
        '\u{2067}' => "right-to-left isolate",
        '\u{2068}' => "first strong isolate",
        '\u{2069}' => "pop directional isolate",
        '\u{FEFF}' => "zero width no-break space",
        '\u{FE0F}' => "emoji variation selector",
        '\u{FE00}'..='\u{FE0E}' | '\u{E0100}'..='\u{E01EF}' => "variation selector",
        '\u{E0020}'..='\u{E007F}' => "tag character",
        _ => match GeneralCategory::for_char(c) {
            GeneralCategory::Control => "control character",
            GeneralCategory::Format => "format character",
            GeneralCategory::PrivateUse => "private-use character",
            GeneralCategory::Surrogate => "surrogate",
            _ => "unassigned code point",
        },
    }
}

/// Appends `segs` to `job`: plain text in `format`, and each hidden character as its placeholder in the jar-marker
/// style (`JarLabel::marker_format`) in `hidden_color`.
pub(crate) fn append_name(
    job: &mut egui::text::LayoutJob,
    segs: &[NameSeg],
    format: &egui::TextFormat,
    hidden_color: egui::Color32,
) {
    for seg in segs {
        match *seg {
            NameSeg::Plain(p) => job.append(p, 0.0, format.clone()),
            NameSeg::Hidden(c) => job.append(
                &placeholder(c),
                0.0,
                crate::jar_defs::JarLabel::marker_format(format.font_id.size, hidden_color),
            ),
        }
    }
}

/// `prefix`, the name and `suffix` in `ui`'s widget font (`FontSelection::Default`, as egui widgets resolve it) and
/// the fallback colour (`Color32::PLACEHOLDER`, so a button paints it in its own state's colour), with placeholders
/// in the weak colour. Used for a button or label whose text includes a stored name.
pub(crate) fn name_job(ui: &egui::Ui, prefix: &str, segs: &[NameSeg], suffix: &str) -> egui::text::LayoutJob {
    let font = egui::FontSelection::Default.resolve(ui.style());
    let plain = egui::TextFormat::simple(font, egui::Color32::PLACEHOLDER);
    let mut job = egui::text::LayoutJob::default();
    if !prefix.is_empty() {
        job.append(prefix, 0.0, plain.clone());
    }
    append_name(&mut job, segs, &plain, ui.visuals().weak_text_color());
    if !suffix.is_empty() {
        job.append(suffix, 0.0, plain);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    /// 👩‍💻, 🏴󠁧󠁢󠁥󠁮󠁧󠁿 and other sequences written out, so the hidden characters in them are visible in the source.
    const CODER: &str = "👩\u{200D}💻";
    const ENGLAND: &str = "🏴\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";

    #[test]
    fn clean_name_drops_hidden_characters() {
        for (raw, want) in [
            ("Re\u{200B}ading", "Reading"),
            ("a\tb\nc\r", "abc"),
            ("\u{0}nul", "nul"),
            ("\u{202E}evil\u{202C}", "evil"),
            ("\u{2066}iso\u{2069}", "iso"),
            ("soft\u{00AD}hyphen", "softhyphen"),
            ("\u{FEFF}bom", "bom"),
            ("x\u{FE0F}", "x"),
            ("y\u{E0100}", "y"),
            ("\u{E000}p", "p"),
            ("\u{0378}q", "q"),
            ("tag\u{E0041}", "tag"),
        ] {
            assert_eq!(clean_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn clean_name_makes_odd_spaces_one_plain_space_and_trims() {
        for (raw, want) in [
            ("a\u{00A0}b", "a b"),
            ("a\u{3000}b", "a b"),
            ("a\u{2028}b\u{2029}c", "a b c"),
            ("  a \u{2009}\u{2009} b  ", "a b"),
            ("a \u{200B} b", "a b"),
            ("\u{200B} a", "a"),
            (" \u{00A0} ", ""),
            ("\u{200B}\u{202E}", ""),
        ] {
            assert_eq!(clean_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn clean_name_borrows_a_clean_name_and_leaves_it_alone() {
        for name in ["Work", "Café", "東京", "Работа", "👍🏽 Team", "🇯🇵", "Work · #3f2a", "A-b_c (2)!", "", "x"]
        {
            assert!(matches!(clean_name(name), Cow::Borrowed(n) if n == name), "{name:?}");
        }
    }

    /// An emoji sequence keeps its joiners, selectors and tags: the name is already clean.
    #[test]
    fn clean_name_keeps_emoji_sequences_whole() {
        for name in [
            CODER,
            "👩🏽\u{200D}💻",
            "❤\u{FE0F}\u{200D}🔥",
            "🏳\u{FE0F}\u{200D}🌈",
            "☺\u{FE0F}",
            "✈\u{FE0E}",
            "1\u{FE0F}\u{20E3}",
            "#\u{FE0F}\u{20E3}",
            "1\u{20E3}",
            ENGLAND,
            "Team 👩\u{200D}💻 2",
        ] {
            assert!(matches!(clean_name(name), Cow::Borrowed(n) if n == name), "{name:?}");
        }
    }

    /// The same characters outside an emoji sequence are dropped.
    #[test]
    fn clean_name_drops_emoji_formatting_outside_a_sequence() {
        for (raw, want) in [
            ("a\u{200D}b", "ab"),
            ("\u{200D}", ""),
            ("👩\u{200D}", "👩"),
            ("\u{200D}💻", "💻"),
            ("👩 \u{200D}💻", "👩 💻"),
            ("👩\u{200B}\u{200D}💻", "👩💻"),
            ("👩\u{200D}\u{200D}💻", "👩💻"),
            ("a\u{FE0F}", "a"),
            ("\u{FE0F}", ""),
            ("👍🏽\u{FE0F}", "👍🏽"),
            ("1\u{FE0F}", "1"),
            ("🏴\u{E0067}\u{E0062}", "🏴"),
            ("🏴\u{E007F}", "🏴"),
            ("x\u{E0067}\u{E007F}", "x"),
            ("👍\u{E0067}\u{E007F}", "👍"),
            ("🏴\u{E0067}\u{E007F}\u{E0067}", "🏴\u{E0067}\u{E007F}"),
        ] {
            assert_eq!(clean_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn clean_name_is_idempotent() {
        let long = "x".repeat(63) + " y";
        let coders = CODER.repeat(70);
        let inputs = [
            "Re\u{200B}ading",
            "  a \u{2009}\u{2009} b  ",
            "e\u{200B}\u{0301}",
            "\u{202E}..\\",
            "a\tb",
            long.as_str(),
            "a\u{200D}b",
            "👩\u{200B}\u{200D}💻",
            "👩\u{FE0E}\u{FE0F}\u{200D}💻",
            "🏴\u{E0067}\u{E007F}\u{E0067}",
            coders.as_str(),
        ];
        for raw in inputs {
            let once = clean_name(raw).into_owned();
            assert!(matches!(clean_name(&once), Cow::Borrowed(_)), "{raw:?} -> {once:?}");
        }
    }

    #[test]
    fn clean_name_caps_at_64_clusters_on_a_cluster_boundary() {
        assert_eq!(clean_name(&"é".repeat(65)), "é".repeat(64));
        assert_eq!(clean_name(&"👍🏽".repeat(65)), "👍🏽".repeat(64), "a skin-tone emoji is never cut from its modifier");
        assert_eq!(clean_name(&"🇯🇵".repeat(65)), "🇯🇵".repeat(64), "a flag is never split");
        assert_eq!(clean_name(&CODER.repeat(65)), CODER.repeat(64), "a ZWJ sequence is one cluster");
        assert_eq!(clean_name(&ENGLAND.repeat(65)), ENGLAND.repeat(64), "a tag sequence is one cluster");
        assert_eq!(clean_name(&"e\u{0301}".repeat(65)), "e\u{0301}".repeat(64), "a combining mark stays");
        assert_eq!(clean_name(&("x".repeat(63) + " y")), "x".repeat(63), "the space left at the cut is trimmed");
        assert_eq!(clean_name(&"x".repeat(64)), "x".repeat(64));
    }

    #[test]
    fn display_name_borrows_a_clean_name_whole() {
        for name in ["Work Home", CODER, ENGLAND] {
            let segs = display_name(name, UNTITLED);
            assert!(matches!(segs[..], [NameSeg::Plain(p)] if std::ptr::eq(p, name)), "{segs:?}");
        }
    }

    #[test]
    fn display_name_reveals_hidden_characters() {
        let segs = display_name("Re\u{200B}ading", UNTITLED);
        assert_eq!(segs, [NameSeg::Plain("Re"), NameSeg::Hidden('\u{200B}'), NameSeg::Plain("ading")]);
        assert_eq!(to_plain(&segs), "Re<U+200B>ading");
        assert!(has_hidden(&segs));
        assert_eq!(display_name("\u{200B}", UNTITLED), [NameSeg::Hidden('\u{200B}')], "revealed, not the fallback");
    }

    /// Emoji formatting outside a sequence is revealed like any hidden character; a sequence counts as one unit.
    #[test]
    fn display_name_reveals_emoji_formatting_outside_a_sequence() {
        let joined = display_name("a\u{200D}b", UNTITLED);
        assert_eq!(joined, [NameSeg::Plain("a"), NameSeg::Hidden('\u{200D}'), NameSeg::Plain("b")]);
        assert_eq!(display_name("a\u{FE0F}", UNTITLED), [NameSeg::Plain("a"), NameSeg::Hidden('\u{FE0F}')]);
        let segs = display_name("👩\u{200B}\u{200D}💻", UNTITLED);
        assert_eq!(to_plain(&segs), "👩<U+200B><U+200D>💻");
        let coders = CODER.repeat(70);
        let segs = display_name(&coders, UNTITLED);
        assert_eq!((units(&segs), to_plain(&segs)), (64, CODER.repeat(64)));
    }

    #[test]
    fn display_name_collapses_spaces_and_trims() {
        let segs = display_name("  a\u{00A0}\u{00A0}b \u{200B} c ", UNTITLED);
        assert_eq!(
            segs,
            [
                NameSeg::Plain("a"),
                NameSeg::Plain(" "),
                NameSeg::Plain("b"),
                NameSeg::Plain(" "),
                NameSeg::Hidden('\u{200B}'),
                NameSeg::Plain(" "),
                NameSeg::Plain("c"),
            ]
        );
        assert_eq!(to_plain(&segs), "a b <U+200B> c");
    }

    #[test]
    fn display_name_falls_back_when_nothing_is_left() {
        assert_eq!(display_name("", UNTITLED), [NameSeg::Plain("Untitled")]);
        assert_eq!(display_name(" \u{00A0}\u{3000} ", UNTITLED), [NameSeg::Plain("Untitled")]);
    }

    #[test]
    fn placeholders_count_against_the_cap() {
        let hidden = "\u{200B}".repeat(70);
        let segs = display_name(&hidden, UNTITLED);
        assert_eq!((segs.len(), units(&segs)), (64, 64));
        let raw = "a".repeat(10) + &"\u{200B}".repeat(60);
        let segs = display_name(&raw, UNTITLED);
        assert_eq!(units(&segs), 64);
        assert_eq!(segs[0], NameSeg::Plain("aaaaaaaaaa"));
        assert_eq!(segs.iter().filter(|s| matches!(s, NameSeg::Hidden(_))).count(), 54);
        let raw = "x".repeat(70) + "\u{200B}";
        assert_eq!(display_name(&raw, UNTITLED), [NameSeg::Plain(&raw[..64])]);
    }

    #[test]
    fn shorten_keeps_max_minus_one_units_and_an_ellipsis() {
        let long = "x".repeat(30);
        let segs = display_name(&long, UNTITLED);
        let short = shorten(&segs, 20).expect("30 units is over 20");
        assert_eq!(to_plain(&short), "x".repeat(19) + "…");
        assert_eq!(shorten(&display_name(&"x".repeat(20), UNTITLED), 20), None);
        let hidden = display_name("ab\u{200B}\u{200B}cd", UNTITLED);
        assert_eq!(to_plain(&shorten(&hidden, 4).unwrap()), "ab<U+200B>…");
    }

    #[test]
    fn placeholders_are_uppercase_hex_of_at_least_four_digits() {
        assert_eq!(placeholder('\u{200B}'), "⟦200B⟧");
        assert_eq!(placeholder('\t'), "⟦0009⟧");
        assert_eq!(placeholder('\u{E0100}'), "⟦E0100⟧");
        assert_eq!(to_plain(&[NameSeg::Hidden('\u{10FFFD}')]), "<U+10FFFD>");
        for c in ['\0', '\u{200B}', '\u{E01EF}', '\u{10FFFD}'] {
            assert!(placeholder(c).chars().all(|p| PLACEHOLDER_CHARS.contains(p)), "{c:?}");
            assert!(to_plain(&[NameSeg::Hidden(c)]).is_ascii(), "{c:?}");
        }
    }

    #[test]
    fn the_hint_names_each_hidden_character_once() {
        let segs = display_name("a\u{200B}b\u{200B}\u{202E}c\u{E000}\u{200D}", UNTITLED);
        assert_eq!(
            hidden_hint(&segs).as_deref(),
            Some(
                "Hidden characters: U+200B zero width space, U+202E right-to-left override, \
                 U+E000 private-use character, U+200D zero width joiner"
            )
        );
        assert_eq!(hidden_hint(&display_name("Work", UNTITLED)), None);
        assert_eq!(with_hint("Open", &display_name("Work", UNTITLED)), "Open");
        assert_eq!(with_hint("Open", &segs).lines().count(), 2);
        assert_eq!(with_hint("", &segs), hidden_hint(&segs).unwrap());
    }

    #[test]
    fn a_hidden_character_is_appended_in_the_marker_style() {
        let format = egui::TextFormat::simple(egui::FontId::proportional(13.0), egui::Color32::RED);
        let mut job = egui::text::LayoutJob::default();
        append_name(&mut job, &display_name("Re\u{200B}ading", UNTITLED), &format, egui::Color32::GRAY);
        assert_eq!(job.text, "Re⟦200B⟧ading");
        assert_eq!(job.sections.len(), 3);
        assert_eq!(job.sections[0].format, format);
        assert_eq!(job.sections[2].format, format);
        let hidden = &job.sections[1].format;
        assert_eq!(hidden.font_id, egui::FontId::monospace(13.0 * crate::jar_defs::MARKER_SCALE));
        assert_eq!(hidden.color, egui::Color32::GRAY);
    }

    #[test]
    fn name_job_wraps_the_name_in_plain_text() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = name_job(ui, "Move to ", &display_name("Re\u{200B}ading", UNTITLED), " (3)");
            assert_eq!(job.text, "Move to Re⟦200B⟧ading (3)");
            // `LayoutJob::append` merges a section into the one before it when their formats match.
            let colors: Vec<_> = job.sections.iter().map(|s| s.format.color).collect();
            let plain = egui::Color32::PLACEHOLDER;
            assert_eq!(colors, [plain, ui.visuals().weak_text_color(), plain]);
            assert_eq!(name_job(ui, "", &display_name("Work", UNTITLED), "").sections.len(), 1);
        });
        output.textures_delta.clear();
    }

    /// Spec: every character of the rich placeholder is in the monospace font, so a font change cannot turn it into
    /// tofu unnoticed. Checked against the font file's character map (egui's `has_glyph` cannot tell: Hack has the
    /// replacement glyph ◻, so egui reports every character Hack lacks as present).
    #[test]
    fn the_placeholder_characters_are_in_the_monospace_font() {
        use skrifa::MetadataProvider;
        let defs = crate::theme::font_definitions();
        let first = &defs.families[&egui::FontFamily::Monospace][0];
        let data = &defs.font_data[first];
        let font = skrifa::FontRef::from_index(&data.font, data.index).expect("a readable font file");
        let missing: Vec<char> = PLACEHOLDER_CHARS.chars().filter(|&c| font.charmap().map(c).is_none()).collect();
        assert_eq!(missing, Vec::<char>::new(), "{first} lacks these, so a placeholder would draw tofu");
    }
}
