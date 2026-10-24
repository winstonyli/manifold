//! Splitting text into lowercase search tokens.

/// Kana, Han and Hangul have no spaces between words, so each character is its own token.
fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF)
}

/// The lowercase alphanumeric runs of `text`, each CJK character alone.
pub(crate) fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    for ch in text.chars() {
        if is_cjk(ch) {
            flush(&mut cur, &mut out);
            out.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            cur.extend(ch.to_lowercase());
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_lowercase_and_split_on_punctuation() {
        assert_eq!(tokens("Hello, World-wide web!"), ["hello", "world", "wide", "web"]);
    }

    #[test]
    fn cjk_characters_are_single_tokens() {
        assert_eq!(tokens("日本語abc"), ["日", "本", "語", "abc"]);
    }

    #[test]
    fn odd_unicode_never_panics() {
        let _ = tokens("a\u{FFFD}\u{200B}\u{0}𝒳e\u{0301}");
    }
}
