//! Name rules (spec `docs/superpowers/specs/2026-10-03-name-rules-design.md`). There is one input rule,
//! `clean_name`, applied when the user commits a group, jar, session or bookmark folder name, and one display rule,
//! `display_name`, for every stored name that is drawn. Stored names are never rewritten. A name from sync, an import
//! or older data is shown through the display rule, which reveals a hidden character as a placeholder.

use icu_properties::props::{BinaryProperty, EnumeratedProperty, ExtendedPictographic, GeneralCategory};
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

/// The most grapheme clusters a name keeps (input) or shows (display, where a placeholder counts as one).
pub(crate) const NAME_CAP: usize = 64;
/// The most characters a name keeps or shows, as a second budget beside `NAME_CAP`: one grapheme cluster can hold
/// any number of characters (a combining mark or skin-tone modifier repeated, a long joiner chain).
pub(crate) const NAME_MAX_CHARS: usize = 256;
/// The most units (`shorten`) a name shows in a menu or list row: a "Move to" row, a Settings saved-session button.
pub(crate) const ROW_UNITS: usize = 32;
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

/// The most tags a subdivision flag's code has: a region (2) and a subdivision (1-4), as in `gbeng`.
const FLAG_TAGS_MAX: usize = 6;

/// The byte length of the subdivision-flag tags at the start of `rest`, the text after a 🏴: 1 to `FLAG_TAGS_MAX`
/// tags for a lowercase letter or digit (U+E0030-E0039, U+E0061-E007A), then the cancel tag U+E007F. 0 when `rest`
/// does not start with such a run, so a 🏴 cannot carry invisible tag text.
fn flag_tags_len(rest: &str) -> usize {
    let valid = |c: &char| matches!(c, '\u{E0030}'..='\u{E0039}' | '\u{E0061}'..='\u{E007A}');
    let tags: Vec<char> = rest.chars().take_while(valid).take(FLAG_TAGS_MAX + 1).collect();
    let spec: usize = tags.iter().map(|c| c.len_utf8()).sum();
    if (1..=FLAG_TAGS_MAX).contains(&tags.len()) && rest[spec..].starts_with(CANCEL_TAG) {
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
        && (s.len() <= NAME_CAP || fits(&[NameSeg::Plain(s)], NAME_CAP, NAME_MAX_CHARS))
}

/// What a cut at a cluster boundary must not leave at the end: a space, or a joiner (should the segmenter's emoji
/// data be older than `is_pict`'s, it could end a cluster at a joiner that `classify` keeps).
const CUT_TRIM: [char; 2] = [' ', ZWJ];

/// The input rule: `s` without hidden characters (`classify`: emoji sequences keep theirs), with each whitespace run
/// as one plain space, trimmed, and cut to `NAME_CAP` grapheme clusters and `NAME_MAX_CHARS` characters
/// (`take_units`). It borrows `s` when `s` is already clean, and is idempotent.
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
    if !fits(&[NameSeg::Plain(&out)], NAME_CAP, NAME_MAX_CHARS) {
        let kept = match take_units(&[NameSeg::Plain(&out)], NAME_CAP, NAME_MAX_CHARS)[..] {
            [NameSeg::Plain(p)] => p.len(),
            _ => 0,
        };
        out.truncate(kept);
        // A cut inside one long cluster can strand a character whose context it took (a keycap's selector without
        // its U+20E3): clean what is left again. It only gets shorter, so this ends.
        if !is_clean(&out) {
            out = clean_name(&out).into_owned();
        }
    }
    Cow::Owned(out)
}

/// The display rule: `s` as segments, with each hidden character (`classify`) a `Hidden`, each whitespace run one
/// plain space, trimmed, and at most `NAME_CAP` units (`units`) and `NAME_MAX_CHARS` characters: a longer name is
/// cut by `shorten`, so it ends in "…". It gives `fallback` when nothing is left. A clean name is one segment
/// borrowing all of `s`: the `Vec` is the only allocation, and the name is not copied.
pub(crate) fn display_name<'a>(s: &'a str, fallback: &'a str) -> Vec<NameSeg<'a>> {
    let segs = reveal(s, fallback);
    shorten(&segs, NAME_CAP).unwrap_or(segs)
}

/// `display_name` without its cap.
fn reveal<'a>(s: &'a str, fallback: &'a str) -> Vec<NameSeg<'a>> {
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

/// How many characters `segs` holds, with a `Hidden` counting as one (the character it stands for).
fn chars(segs: &[NameSeg]) -> usize {
    segs.iter()
        .map(|s| match s {
            NameSeg::Plain(p) => p.chars().count(),
            NameSeg::Hidden(_) => 1,
        })
        .sum()
}

/// Whether `segs` is within both budgets: `max_units` units (`units`) and `max_chars` characters (`chars`).
fn fits(segs: &[NameSeg], max_units: usize, max_chars: usize) -> bool {
    units(segs) <= max_units && chars(segs) <= max_chars
}

/// The characters `shorten` allows for `max` units: `NAME_MAX_CHARS` for `NAME_CAP`, in proportion for a shorter
/// cap, so a cluster of many characters cannot widen a short row either.
fn chars_for(max: usize) -> usize {
    max.saturating_mul(NAME_MAX_CHARS) / NAME_CAP
}

/// The longest start of `segs` within both budgets (`fits`), cut at a cluster boundary, with no trailing space or
/// joiner. When the first cluster alone is over `max_chars`, it is cut by character.
fn take_units<'a>(segs: &[NameSeg<'a>], max_units: usize, max_chars: usize) -> Vec<NameSeg<'a>> {
    let mut out = Vec::new();
    let (mut units, mut chars) = (0, 0);
    'segs: for &seg in segs {
        match seg {
            NameSeg::Hidden(_) => {
                if units == max_units || chars == max_chars {
                    break;
                }
                out.push(seg);
                units += 1;
                chars += 1;
            }
            NameSeg::Plain(p) => {
                for (i, g) in p.grapheme_indices(true) {
                    let len = g.chars().count();
                    let full = units == max_units;
                    if full || chars + len > max_chars {
                        let end = if !full && units == 0 {
                            i + g.char_indices().nth(max_chars).map_or(g.len(), |(j, _)| j)
                        } else {
                            i
                        };
                        out.push(NameSeg::Plain(&p[..end]));
                        break 'segs;
                    }
                    units += 1;
                    chars += len;
                }
                out.push(seg);
            }
        }
    }
    // The break can leave an empty `Plain` after a lone space, so drop empties before trimming the end.
    out.retain(|s| *s != NameSeg::Plain(""));
    while let Some(NameSeg::Plain(p)) = out.last_mut() {
        *p = p.trim_end_matches(CUT_TRIM);
        if !p.is_empty() {
            break;
        }
        out.pop();
    }
    out
}

/// `segs` cut to `max - 1` units plus "…", or `None` when `segs` already fits in `max`. The characters are bounded
/// too (`chars_for`), the "…" counting within both budgets.
pub(crate) fn shorten<'a>(segs: &[NameSeg<'a>], max: usize) -> Option<Vec<NameSeg<'a>>> {
    cut(segs, max, chars_for(max))
}

/// `segs` cut within `max_units - 1` units and `max_chars - 1` characters plus "…", or `None` when it fits.
fn cut<'a>(segs: &[NameSeg<'a>], max_units: usize, max_chars: usize) -> Option<Vec<NameSeg<'a>>> {
    (!fits(segs, max_units, max_chars)).then(|| {
        let mut short = take_units(segs, max_units.saturating_sub(1), max_chars.saturating_sub(1));
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

/// The hover for a stored name `s` drawn by `display_name` and cut to `max` units (`shorten`): `whole` when the name
/// is drawn whole, else `cut(full name)`; then `hidden_hint` on its own line (`with_hint`). The full name is `s` by
/// the display rule without its cluster cap, in the plain form (`to_plain`), so the hover shows what the cut, or the
/// display cap, left out, and names every hidden character in it. It is still cut, with "…", at `NAME_MAX_CHARS`
/// characters, which only a name that never went through `clean_name` can reach. Empty when there is nothing to
/// say: no hover then.
pub(crate) fn name_hover(s: &str, max: usize, whole: &str, cut: impl Fn(&str) -> String) -> String {
    let whole_name = reveal(s, UNTITLED);
    let full = self::cut(&whole_name, usize::MAX, NAME_MAX_CHARS).unwrap_or_else(|| whole_name.clone());
    let max = max.min(NAME_CAP);
    // Fit is judged on the whole name: the hover's own cut ends in "…", which would count as a unit.
    let base = if fits(&whole_name, max, chars_for(max)) { whole.to_string() } else { cut(&to_plain(&full)) };
    with_hint(&base, &full)
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
mod tests;
