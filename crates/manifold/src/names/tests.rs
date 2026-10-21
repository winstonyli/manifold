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

/// The edges of the sequences: a joiner after a flag's tags (GB11 lets the tags sit between the emoji and the
/// joiner), and the `*` keycap.
#[test]
fn clean_name_keeps_a_joiner_after_flag_tags_and_the_star_keycap() {
    let flag_join = format!("{ENGLAND}\u{200D}💻");
    for name in [flag_join.as_str(), "*\u{FE0F}\u{20E3}", "*\u{20E3}"] {
        assert!(matches!(clean_name(name), Cow::Borrowed(n) if n == name), "{name:?}");
    }
    assert_eq!(clean_name("*\u{FE0F}"), "*", "a keycap base's selector without U+20E3");
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
    // A tag sequence is one cluster of 7 characters, so the character budget cuts first: 36 of them are 252.
    assert_eq!(clean_name(&ENGLAND.repeat(65)), ENGLAND.repeat(36), "a tag sequence is never split");
    assert_eq!(clean_name(&"e\u{0301}".repeat(65)), "e\u{0301}".repeat(64), "a combining mark stays");
    assert_eq!(clean_name(&("x".repeat(63) + " y")), "x".repeat(63), "the space left at the cut is trimmed");
    assert_eq!(clean_name(&"x".repeat(64)), "x".repeat(64));
}

/// One grapheme cluster can be any length; the character budget bounds it on input and at display.
#[test]
fn a_long_cluster_is_cut_to_the_character_budget() {
    let chain = format!("👩{}", "\u{200D}👩".repeat(500));
    let skin = format!("a{}", "🏽".repeat(500));
    let acute = format!("a{}", "\u{0301}".repeat(500));
    let tags = format!("🏴{}\u{E007F}", "\u{E0067}".repeat(500));
    // A cut inside a cluster that leaves a keycap's selector without its U+20E3 (U+0D4E is a Prepend, so the
    // run and the keycap are one cluster).
    let keycap = format!("{}1\u{FE0F}\u{20E3}{}", "\u{0D4E}".repeat(254), "\u{0301}".repeat(10));
    for raw in [&chain, &skin, &acute, &tags, &keycap] {
        let once = clean_name(raw).into_owned();
        assert!(once.chars().count() <= NAME_MAX_CHARS, "{} chars", once.chars().count());
        assert!(matches!(clean_name(&once), Cow::Borrowed(_)), "idempotent: {raw:?} -> {once:?}");
        let segs = display_name(raw, UNTITLED);
        assert!(chars(&segs) <= NAME_MAX_CHARS && units(&segs) <= NAME_CAP, "{segs:?}");
    }
    assert_eq!(clean_name(&chain), format!("👩{}", "\u{200D}👩".repeat(127)), "the joiner left at the cut goes");
    assert_eq!(clean_name(&skin), format!("a{}", "🏽".repeat(255)));
    assert_eq!(clean_name(&acute), format!("a{}", "\u{0301}".repeat(255)));
    assert_eq!(clean_name(&tags), "🏴", "500 tags are not a subdivision flag");
    assert_eq!(clean_name(&keycap), format!("{}1", "\u{0D4E}".repeat(254)), "the stranded selector goes");
}

/// The cut is at the last cluster boundary inside both budgets.
#[test]
fn the_cap_cuts_inside_both_budgets() {
    let five = "e\u{0301}\u{0301}\u{0301}\u{0301}";
    assert_eq!(clean_name(&five.repeat(64)), five.repeat(51), "51 clusters of 5 is 255 characters");
    assert_eq!(clean_name(&"x".repeat(300)), "x".repeat(64), "64 clusters come first");
    let raw = five.repeat(64) + "\u{200B}";
    let segs = display_name(&raw, UNTITLED);
    assert!(chars(&segs) <= NAME_MAX_CHARS, "{}", chars(&segs));
}

/// A subdivision flag's tags are its code: 1 to 6 lowercase letters and digits, then the cancel tag.
#[test]
fn a_flag_keeps_only_valid_subdivision_tags() {
    let tag = |s: &str| s.chars().map(|c| char::from_u32(c as u32 + 0xE0000).unwrap()).collect::<String>();
    for code in ["gbsct", "usca", "gbeng", "fr75", "a", "abcdef"] {
        let flag = format!("🏴{}\u{E007F}", tag(code));
        assert!(matches!(clean_name(&flag), Cow::Borrowed(_)), "{code}");
    }
    for code in ["abcdefg", "GBENG", "gb eng", "gb-en"] {
        assert_eq!(clean_name(&format!("🏴{}\u{E007F}", tag(code))), "🏴", "{code}");
    }
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
    // A selector after a skin-tone modifier is not directly after the emoji: revealed.
    assert_eq!(display_name("👍🏽\u{FE0F}", UNTITLED), [NameSeg::Plain("👍🏽"), NameSeg::Hidden('\u{FE0F}')]);
    let coders = CODER.repeat(70);
    let segs = display_name(&coders, UNTITLED);
    assert_eq!((units(&segs), to_plain(&segs)), (64, CODER.repeat(63) + "…"));
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

/// The two rules agree: `display_name` without its placeholders is `clean_name`, once the spaces a dropped
/// placeholder leaves side by side are collapsed (for a name under the cap).
#[test]
fn display_name_without_placeholders_is_clean_name() {
    for raw in [
        "Re\u{200B}ading",
        "  a\u{00A0}\u{00A0}b \u{200B} c ",
        "\u{200B} a",
        "a \u{202E} b\t",
        "👩\u{200B}\u{200D}💻 x",
        "a\u{200D}b \u{FE0F}",
        "🏴\u{E0067}\u{E007F}\u{E0067} z",
        "Work · #3f2a",
        "x\u{200B}",
    ] {
        let plain: String = display_name(raw, "")
            .iter()
            .filter_map(|s| match s {
                NameSeg::Plain(p) => Some(*p),
                NameSeg::Hidden(_) => None,
            })
            .collect();
        let collapsed = plain.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
        assert_eq!(collapsed, clean_name(raw), "{raw:?}");
    }
}

#[test]
fn display_name_falls_back_when_nothing_is_left() {
    assert_eq!(display_name("", UNTITLED), [NameSeg::Plain("Untitled")]);
    assert_eq!(display_name(" \u{00A0}\u{3000} ", UNTITLED), [NameSeg::Plain("Untitled")]);
}

#[test]
fn placeholders_count_against_the_cap() {
    let hidden_count = |segs: &[NameSeg]| segs.iter().filter(|s| matches!(s, NameSeg::Hidden(_))).count();
    let hidden = "\u{200B}".repeat(70);
    let segs = display_name(&hidden, UNTITLED);
    assert_eq!((segs.len(), units(&segs), hidden_count(&segs)), (64, 64, 63));
    assert_eq!(segs.last(), Some(&NameSeg::Plain("…")), "a capped name says so");
    let raw = "a".repeat(10) + &"\u{200B}".repeat(60);
    let segs = display_name(&raw, UNTITLED);
    assert_eq!(units(&segs), 64);
    assert_eq!(segs[0], NameSeg::Plain("aaaaaaaaaa"));
    assert_eq!(hidden_count(&segs), 53);
    let raw = "x".repeat(70) + "\u{200B}";
    assert_eq!(display_name(&raw, UNTITLED), [NameSeg::Plain(&raw[..63]), NameSeg::Plain("…")]);
    let raw = "x".repeat(64) + "\u{200B}";
    assert_eq!(units(&display_name(&raw, UNTITLED)), 64, "64 units and a placeholder over");
    assert_eq!(display_name(&"x".repeat(64), UNTITLED), [NameSeg::Plain(&"x".repeat(64))], "64 fit whole");
}

/// A shortened name's hover gives the whole name, past the display cap, and every hidden character in it.
#[test]
fn a_name_hover_gives_the_full_name_and_its_hidden_characters() {
    let long = "x".repeat(30);
    let jump = |f: &str| format!("Jump to this group ({f})");
    assert_eq!(name_hover(&long, 20, "Jump to this group", jump), format!("Jump to this group ({long})"));
    assert_eq!(name_hover("Work", 20, "Jump to this group", jump), "Jump to this group");
    assert_eq!(
        name_hover("Re\u{200B}ading", 20, "Click to rename (F2)", |f| format!("Click to rename, F2 ({f})")),
        "Click to rename (F2)\nHidden characters: U+200B zero width space"
    );
    // A label with no hover of its own: the hint alone, or nothing.
    assert_eq!(name_hover("Re\u{200B}ading", 20, "", str::to_string), "Hidden characters: U+200B zero width space");
    assert_eq!(name_hover("Work", 20, "", str::to_string), "");
    // Past the display cap: the hover is not the capped display, and names a hidden character past it.
    let past = "x".repeat(100) + "\u{200B}";
    assert_eq!(
        name_hover(&past, 20, "", str::to_string),
        format!("{}<U+200B>\nHidden characters: U+200B zero width space", "x".repeat(100))
    );
    // Still bounded: one cluster of 1000 characters is cut to the character budget, with an ellipsis.
    let acute = format!("a{}", "\u{0301}".repeat(1000));
    let hover = name_hover(&acute, 20, "", str::to_string);
    assert!(hover.ends_with('…') && hover.chars().count() == NAME_MAX_CHARS, "{}", hover.chars().count());
    // Over the character budget in few clusters: the hover is the cut text even at the display cap.
    assert_eq!(name_hover(&acute, NAME_CAP, "whole", str::to_string), hover);
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

/// A cut never leaves a space before the ellipsis, even when the space is the last unit kept after a hidden one.
#[test]
fn a_cut_after_a_hidden_character_and_a_space_drops_the_space() {
    for text in ["ab\u{200B} cdef", "ab\u{200B} \u{200B}cdef", "ab \u{200B}\u{200B}cdef"] {
        let segs = display_name(text, UNTITLED);
        for max in 2..=6 {
            let Some(short) = shorten(&segs, max) else { continue };
            let plain = to_plain(&short);
            assert!(!plain.ends_with(" …"), "{text:?} at {max}: {plain:?}");
            assert!(!short.contains(&NameSeg::Plain("")), "{text:?} at {max}: {short:?}");
        }
    }
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

/// Source files that may read a group, jar or session `.name`: each read there is either drawn through a name
/// rule (the line calls one of `RULE_MARKS`) or marked raw with `// name rule: <why>`, on the same line or as a
/// comment on the line just above (`ruled`). A new file that reads `.name` must be added here, after deciding
/// which rule its site gets.
const ALLOWED: [&str; 14] = [
    "card.rs",
    "closed.rs",
    "debug_export.rs",
    "graph_nav.rs",
    "groups.rs",
    "jar_defs.rs",
    "main.rs",
    "mirror.rs",
    "node.rs",
    "node_menu.rs",
    "session_sync/edits.rs",
    "settings_page.rs",
    "shortcuts.rs",
    "virtual_source.rs",
];
/// Files whose `.name` is some other name: a cookie's (`jar.rs`), a string literal (`json_level_ui.rs`), a thread's
/// (`process_manager/view_io.rs`).
const OTHER_NAMES: [&str; 3] = ["jar.rs", "json_level_ui.rs", "process_manager/view_io.rs"];
const RULE_MARKS: [&str; 8] = [
    "display_name(",
    "clean_name(",
    "name_key(",
    ".display()",
    "append_to(",
    "marker_text(",
    "hidden_hint(",
    "// name rule:",
];

/// `src` up to its first test module. Every file here keeps its test modules last (checked when the guard was
/// written, 2026-10-03).
fn production(src: &str) -> &str {
    let starts = ["#[cfg(test)]\nmod ", "#[cfg(test)]\npub(crate) mod "];
    starts.iter().filter_map(|m| src.find(m)).min().map_or(src, |i| &src[..i])
}

/// The `.name` reads on `line`: `.name` not followed by more of an identifier (`.name_format`), not assigned
/// (`.name = `), and not an engine descriptor's (`desc().name`).
fn name_reads(line: &str) -> usize {
    line.match_indices(".name")
        .filter(|(i, _)| {
            let rest = &line[i + ".name".len()..];
            !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                && !rest.starts_with(" = ")
                && !line[..*i].ends_with("desc()")
        })
        .count()
}

/// Whether the `.name` reads on `line` are ruled: the line itself holds one of `RULE_MARKS` (a rule's call, or a
/// trailing `// name rule: <why>`), or the line just above is a `// name rule: <why>` comment of its own. The
/// mark goes above where a trailing comment would push the line past the width or `cargo fmt` would move it.
fn ruled(above: &str, line: &str) -> bool {
    RULE_MARKS.iter().any(|m| line.contains(m)) || above.trim_start().starts_with("// name rule:")
}

#[test]
fn name_reads_counts_reads_only() {
    assert_eq!(name_reads("let n = group.name.clone();"), 1);
    assert_eq!(name_reads("f(&a.name, &b.name)"), 2);
    assert_eq!(name_reads("group.name = clean;"), 0);
    assert_eq!(name_reads("label.name_format(font)"), 0);
    assert_eq!(name_reads("engine.desc().name"), 0);
    assert_eq!(production("a\n#[cfg(test)]\nmod tests {\nb.name\n}"), "a\n");
}

#[test]
fn a_mark_counts_on_the_line_or_the_line_above() {
    let read = "let n = group.name.clone();";
    assert!(!ruled("", read));
    assert!(ruled("", "let n = group.name.clone(); // name rule: why"));
    assert!(ruled("    // name rule: why", read));
    assert!(ruled("", "let n = display_name(&group.name, UNTITLED);"));
    // Only a raw mark reaches down a line, not a rule's call above.
    assert!(!ruled("let s = display_name(&a, UNTITLED);", read));
    assert!(!ruled("// a comment", read));
}

/// Spec "Guard against missed sites": outside `names.rs` and the store layer, a stored name reaches UI text only
/// through a name rule.
#[test]
fn every_stored_name_reaches_the_screen_through_a_name_rule() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut seen = Vec::new();
    // `caps/`, `gpu/`, `graph_ui/`, `popups/`, `process_manager/`, `session_sync/` and `tabs/` are production code: their files are scanned as `<folder>/<file>`.
    let entries = std::fs::read_dir(&dir)
        .unwrap()
        .chain(std::fs::read_dir(dir.join("caps")).unwrap())
        .chain(std::fs::read_dir(dir.join("gpu")).unwrap())
        .chain(std::fs::read_dir(dir.join("graph_ui")).unwrap())
        .chain(std::fs::read_dir(dir.join("popups")).unwrap())
        .chain(std::fs::read_dir(dir.join("process_manager")).unwrap())
        .chain(std::fs::read_dir(dir.join("session_sync")).unwrap())
        .chain(std::fs::read_dir(dir.join("tabs")).unwrap());
    for entry in entries {
        let path = entry.unwrap().path();
        let file = path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
        if path.is_dir() {
            if file == "caps"
                || file == "gpu"
                || file == "graph_ui"
                || file == "popups"
                || file == "process_manager"
                || file == "session_sync"
                || file == "tabs"
            {
                continue;
            }
            // A folder holding only `tests.rs` is its sibling module's test module, which `production` skips anyway.
            let files: Vec<_> =
                std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
            if files == ["tests.rs"] {
                continue;
            }
            assert_eq!(file, "store", "a new source folder: scan it here too");
            continue;
        }
        // `process_manager/tests.rs`, `session_sync/tests.rs` and `tabs/tests.rs` are test modules `production` cannot see as one.
        if path.extension().is_none_or(|e| e != "rs") || file == "names.rs" || file.ends_with("/tests.rs") {
            continue;
        }
        seen.push(file.clone());
        if OTHER_NAMES.contains(&file.as_str()) {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        let mut above = "";
        for (no, line) in production(&src).lines().enumerate() {
            let reads = !line.trim_start().starts_with("//") && name_reads(line) > 0;
            if reads && (!ALLOWED.contains(&file.as_str()) || !ruled(above, line)) {
                offenders.push(format!("{file}:{}: {}", no + 1, line.trim()));
            }
            above = line;
        }
    }
    for file in ALLOWED.iter().chain(&OTHER_NAMES) {
        assert!(seen.contains(&file.to_string()), "{file} is gone: drop it from the list");
    }
    assert_eq!(
        offenders,
        Vec::<String>::new(),
        "draw a stored name through names::display_name, or mark a raw use `// name rule: <why>`, and add the file \
             to ALLOWED"
    );
}
