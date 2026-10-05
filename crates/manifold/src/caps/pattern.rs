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

    pub(crate) fn matches(&self, mime: &str) -> bool {
        let mime = mime.to_ascii_lowercase();
        let Some((ty, sub)) = mime.split_once('/') else { return matches!(self, Pattern::Any) };
        match self {
            Pattern::Any => true,
            Pattern::Type(t) => ty == t,
            Pattern::Suffix(s) => sub.strip_suffix(s.as_str()).is_some_and(|rest| rest.ends_with('+')),
            Pattern::Prefix { ty: t, prefix } => ty == t && sub.starts_with(prefix.as_str()),
            Pattern::Exact(e) => mime == *e,
        }
    }
}

/// A runtime fact about a node that refines which rows apply (spec "Tags"). Not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tag {
    /// A tab with a running engine.
    Loaded,
    /// A note with no text.
    Blank,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct TagSet(u16);

impl TagSet {
    pub(crate) const EMPTY: TagSet = TagSet(0);

    pub(crate) fn of(tags: &[Tag]) -> TagSet {
        TagSet(tags.iter().fold(0, |bits, &tag| bits | 1 << tag as u16))
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
        assert_eq!(TagSet::of(&[Tag::Blank, Tag::Blank]).len(), 1);
    }
}
