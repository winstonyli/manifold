//! The one URL splitter: the origin, host initial, short label and query items of an address all read the same parts,
//! so port, case and userinfo mean the same thing everywhere. Hand-rolled and deliberately loose, like
//! `suggestions::looks_like_url`: no URL-syntax validation, no URL-parsing crate for the little this needs.

/// An address cut at its `://`, authority, `?` and `#`. Borrows from the string it parsed.
#[derive(Debug, PartialEq)]
pub(crate) struct UrlParts<'a> {
    /// Everything before the first `?` or `#`.
    pub(crate) address: &'a str,
    /// The text between `?` and `#`, empty if there is none.
    pub(crate) query: &'a str,
    /// The text after `#`.
    pub(crate) fragment: Option<&'a str>,
    /// The scheme, when `://` follows a run of letters, digits, `+`, `-` and `.` at the start.
    scheme: Option<&'a str>,
    /// The authority without userinfo (`host[:port]`), empty when the address has no scheme.
    host: &'a str,
    /// What follows the authority, up to the fragment: the path and query.
    tail: &'a str,
}

impl<'a> UrlParts<'a> {
    pub(crate) fn parse(url: &'a str) -> Self {
        let (before_fragment, fragment) = url.split_once('#').map_or((url, None), |(b, f)| (b, Some(f)));
        let (address, query) = before_fragment.split_once('?').unwrap_or((before_fragment, ""));
        let is_scheme =
            |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        let scheme_end = address.find("://").filter(|&i| is_scheme(&address[..i]));
        let (scheme, host, tail) = match scheme_end {
            Some(i) => {
                let rest = &before_fragment[i + 3..];
                let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
                let authority = &rest[..authority_end];
                let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
                (Some(&address[..i]), host, &rest[authority_end..])
            }
            None => (None, "", ""),
        };
        Self { address, query, fragment, scheme, host, tail }
    }

    /// `scheme://host[:port]`, lowercased and without userinfo; `None` without a scheme or a host.
    pub(crate) fn origin(&self) -> Option<String> {
        let scheme = self.scheme?;
        (!self.host.is_empty()).then(|| format!("{scheme}://{}", self.host).to_ascii_lowercase())
    }

    /// The host's first letter or digit without a leading `www.`, upper-cased: what a page with no favicon is known by.
    pub(crate) fn initial(&self) -> Option<char> {
        let host = self.host.strip_prefix("www.").unwrap_or(self.host);
        host.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().next().unwrap_or(c))
    }

    /// The host without port, `www.` or case: what two addresses on one site share.
    pub(crate) fn host(&self) -> String {
        let host = self.host.strip_prefix("www.").unwrap_or(self.host);
        host.split(':').next().unwrap_or(host).to_ascii_lowercase()
    }

    /// The path, without query, fragment or trailing slash; empty for the root.
    pub(crate) fn path(&self) -> &'a str {
        self.tail.split('?').next().unwrap_or("").trim_end_matches('/')
    }

    /// A short name for the page: its path and query, or its host when that is all there is. An address with no
    /// scheme is its own name.
    pub(crate) fn short(&self) -> &'a str {
        if self.scheme.is_none() {
            return self.address;
        }
        match self.tail {
            "" | "/" => self.host,
            tail => tail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(url: &str) -> Option<String> {
        UrlParts::parse(url).origin()
    }

    #[test]
    fn an_origin_drops_userinfo_path_and_case_and_keeps_the_port() {
        assert_eq!(origin("https://user:pass@Example.COM/x?y"), Some("https://example.com".into()));
        assert_eq!(origin("HTTP://a@b@Host:8080#f"), Some("http://host:8080".into()));
        assert_eq!(origin("https://example.com?q"), Some("https://example.com".into()));
    }

    #[test]
    fn no_scheme_or_no_host_is_no_origin() {
        for url in ["https://user@/x", "https:///x", "about:blank", "", "/just/a/path", "/x?u=http://y", "data:a://b"] {
            assert_eq!(origin(url), None, "{url:?}");
        }
    }

    #[test]
    fn the_initial_is_the_host_without_userinfo_port_or_www() {
        let initial = |url| UrlParts::parse(url).initial();
        assert_eq!(initial("https://www.example.com/a?b#c"), Some('E'));
        assert_eq!(initial("http://github.com"), Some('G'));
        assert_eq!(initial("https://user:pw@docs.rs:8080/x"), Some('D'));
        assert_eq!(initial("https://élan.fr/"), Some('É'));
        assert_eq!(initial("https://127.0.0.1:3000/"), Some('1'));
        for url in ["", "about:blank", "/just/a/path", "https:///x", "file:no-slashes"] {
            assert_eq!(initial(url), None, "{url:?}");
        }
    }

    #[test]
    fn short_is_the_path_and_query_else_the_host_else_the_whole_address() {
        let short = |url| UrlParts::parse(url).short().to_owned();
        assert_eq!(short("https://example.com/docs/intro?x=1#top"), "/docs/intro?x=1");
        assert_eq!(short("https://example.com/"), "example.com");
        assert_eq!(short("https://example.com"), "example.com");
        assert_eq!(short("https://example.com#top"), "example.com");
        assert_eq!(short("https://user:pw@Example.com:81/"), "Example.com:81");
        assert_eq!(short("/plain"), "/plain");
        assert_eq!(short("about:blank"), "about:blank");
    }

    #[test]
    fn address_query_and_fragment_split_at_the_first_marks() {
        let p = UrlParts::parse("https://h/p?a=1&b=2#f?x");
        assert_eq!((p.address, p.query, p.fragment), ("https://h/p", "a=1&b=2", Some("f?x")));
        let p = UrlParts::parse("/plain");
        assert_eq!((p.address, p.query, p.fragment), ("/plain", "", None));
    }
}
