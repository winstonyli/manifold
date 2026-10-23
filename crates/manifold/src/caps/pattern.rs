//! MIME patterns and the tags that refine a match.

/// A MIME pattern, parsed once. Case-insensitive.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Pattern {
    /// `*/*`.
    Any,
    /// `image/*`: any subtype of one type.
    Type(String),
    /// `*/*+json`: any type whose subtype ends in `+json`.
    Suffix(String),
    /// `application/vnd.manifold.*`: a type and a subtype prefix.
    Prefix {
        ty: String,
        prefix: String,
    },
    Exact(String),
}

impl Pattern {
    pub(crate) fn parse(text: &str) -> Pattern {
        let text = text.trim().to_ascii_lowercase();
        if text == "*/*" {
            return Pattern::Any;
        }
        if let Some(suffix) = text.strip_prefix("*/*+") {
            return Pattern::Suffix(suffix.to_owned());
        }
        if let Some((ty, sub)) = text.split_once('/') {
            if sub == "*" {
                return Pattern::Type(ty.to_owned());
            }
            if let Some(prefix) = sub.strip_suffix('*') {
                return Pattern::Prefix { ty: ty.to_owned(), prefix: prefix.to_owned() };
            }
        }
        Pattern::Exact(text)
    }

    /// Low to high: `*/*`, type wildcard, suffix wildcard, subtype prefix, exact.
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Pattern::Any => 0,
            Pattern::Type(_) => 1,
            Pattern::Suffix(_) => 2,
            Pattern::Prefix { .. } => 3,
            Pattern::Exact(_) => 4,
        }
    }

    /// Case-insensitive (ASCII), compared in place: this runs per row per card per frame (`App::level_scan`), so it
    /// never lower-cases a copy. The pattern's own text is lower-cased once by `parse`.
    pub(crate) fn matches(&self, mime: &str) -> bool {
        let mime = mime.as_bytes();
        let Some(slash) = mime.iter().position(|&b| b == b'/') else { return matches!(self, Pattern::Any) };
        let (ty, sub) = (&mime[..slash], &mime[slash + 1..]);
        match self {
            Pattern::Any => true,
            Pattern::Type(t) => ty.eq_ignore_ascii_case(t.as_bytes()),
            // `+suffix` at the end of the subtype.
            Pattern::Suffix(s) => {
                sub.len() > s.len()
                    && sub[sub.len() - s.len()..].eq_ignore_ascii_case(s.as_bytes())
                    && sub[sub.len() - s.len() - 1] == b'+'
            }
            Pattern::Prefix { ty: t, prefix } => {
                ty.eq_ignore_ascii_case(t.as_bytes())
                    && sub.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
            }
            Pattern::Exact(e) => mime.eq_ignore_ascii_case(e.as_bytes()),
        }
    }
}

/// A fact about a node that refines which rows apply (spec "Tags"), or about an engine host that its tabs inherit
/// (`ViewDesc::tags`, merged into a live tab's tags by `App::card_kind`). Not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tag {
    /// A tab with a running engine (`App::card_kind` produces it; the `TYPE_TAB` row that requires it reads it).
    Loaded,
    /// A note with no text.
    Blank,
    /// A host that renders with Blink (CEF, WebView2). Exactly one of the three family tags is on every host: the
    /// rendering engine, as against the host that embeds it.
    Blink,
    /// A host that renders with WebKit (WebKitGTK today).
    WebKit,
    /// A host that renders with Servo.
    Servo,
    /// Frames arrive as BGRA (CEF, WebView2) rather than RGBA (Servo, WebKit).
    Bgra,
    /// Takes part in the cookie jar: waits for `SetCookies` before its first navigation (`VIEW_AWAIT_COOKIES`), and
    /// answers `GetCookies` (spec 2026-10-02-jars-design). A tab on a host without it shows "This view does not use
    /// jars yet".
    Jars,
    /// Opens its own DevTools window on `OpenDevTools`. Servo has no inspector API; WebKit's inspector needs a window
    /// manager its Xvfb session doesn't have. Without it, F12 toasts instead.
    DevTools,
    /// Answers `FindStart` with `FindResult` (all but Servo, whose embedding API has no find). Without it, Ctrl+F
    /// toasts instead of opening a find bar that never gets a result.
    Find,
}

impl Tag {
    /// The rendering-engine family tags, each with its display name.
    pub(crate) const FAMILIES: [(Tag, &'static str); 3] =
        [(Tag::Blink, "Blink"), (Tag::WebKit, "WebKit"), (Tag::Servo, "Servo")];
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct TagSet(u32);

impl TagSet {
    pub(crate) const EMPTY: TagSet = TagSet(0);

    pub(crate) const fn of(tags: &[Tag]) -> TagSet {
        let mut bits = 0;
        let mut i = 0;
        while i < tags.len() {
            bits |= 1 << tags[i] as u32;
            i += 1;
        }
        TagSet(bits)
    }

    /// Both sets' tags.
    pub(crate) fn union(self, other: TagSet) -> TagSet {
        TagSet(self.0 | other.0)
    }

    pub(crate) fn has(self, tag: Tag) -> bool {
        self.contains_all(TagSet::of(&[tag]))
    }

    /// Whether every tag in `other` is in `self`.
    pub(crate) fn contains_all(self, other: TagSet) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) fn len(self) -> u32 {
        self.0.count_ones()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_picks_the_pattern_kind() {
        assert_eq!(Pattern::parse("*/*"), Pattern::Any);
        assert_eq!(Pattern::parse("image/*"), Pattern::Type("image".into()));
        assert_eq!(Pattern::parse("*/*+json"), Pattern::Suffix("json".into()));
        assert_eq!(
            Pattern::parse("application/vnd.manifold.*"),
            Pattern::Prefix { ty: "application".into(), prefix: "vnd.manifold.".into() }
        );
        assert_eq!(Pattern::parse(" Text/Markdown "), Pattern::Exact("text/markdown".into()));
    }

    #[test]
    fn matches_each_pattern_kind() {
        assert!(Pattern::parse("*/*").matches("anything/at-all"));
        assert!(Pattern::parse("image/*").matches("image/png"));
        assert!(!Pattern::parse("image/*").matches("video/mp4"));
        assert!(Pattern::parse("*/*+json").matches("application/ld+json"));
        assert!(!Pattern::parse("*/*+json").matches("application/json"));
        assert!(Pattern::parse("application/vnd.manifold.*").matches("application/vnd.manifold.tab"));
        assert!(!Pattern::parse("application/vnd.manifold.*").matches("text/vnd.manifold.tab"));
        assert!(Pattern::parse("text/markdown").matches("TEXT/Markdown"));
        assert!(!Pattern::parse("text/markdown").matches("text/plain"));
    }

    /// Every pattern kind matches mixed-case input as it matches lower-case, and only then.
    #[test]
    fn matching_ignores_ascii_case_for_every_pattern_kind() {
        for (pattern, mime) in [
            ("image/*", "IMAGE/PNG"),
            ("*/*+json", "Application/LD+JSON"),
            ("application/vnd.manifold.*", "APPLICATION/VND.Manifold.Tab"),
            ("text/markdown", "Text/MARKDOWN"),
            ("Image/*", "image/png"),
        ] {
            assert!(Pattern::parse(pattern).matches(mime), "{pattern} vs {mime}");
            assert_eq!(
                Pattern::parse(pattern).matches(mime),
                Pattern::parse(pattern).matches(&mime.to_ascii_lowercase())
            );
        }
        // Not a match: a suffix with no `+`, a subtype shorter than the prefix, a different type.
        assert!(!Pattern::parse("*/*+json").matches("application/JSON"));
        assert!(!Pattern::parse("application/vnd.manifold.*").matches("application/VND"));
        assert!(!Pattern::parse("image/*").matches("IMAGES/png"));
        assert!(!Pattern::parse("text/markdown").matches("text/markdownx"));
    }

    #[test]
    fn a_type_with_no_slash_matches_only_the_catch_all() {
        assert!(Pattern::parse("*/*").matches("odd"));
        assert!(!Pattern::parse("image/*").matches("odd"));
        assert!(!Pattern::parse("odd").matches("other"));
    }

    #[test]
    fn rank_orders_the_patterns() {
        let ranks: Vec<u8> = ["*/*", "image/*", "*/*+json", "application/vnd.manifold.*", "text/markdown"]
            .iter()
            .map(|p| Pattern::parse(p).rank())
            .collect();
        assert_eq!(ranks, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn tag_sets_hold_and_count_tags() {
        let both = TagSet::of(&[Tag::Loaded, Tag::Blank]);
        assert_eq!(both.len(), 2);
        assert!(both.contains_all(TagSet::of(&[Tag::Blank])));
        assert!(both.contains_all(TagSet::EMPTY));
        assert!(!TagSet::of(&[Tag::Loaded]).contains_all(both));
        assert!(both.has(Tag::Blank) && !both.has(Tag::Servo));
        assert_eq!(both.union(TagSet::of(&[Tag::Servo])).len(), 3);
        assert_eq!(TagSet::of(&[Tag::Blank, Tag::Blank]).len(), 1);
    }
}
