//! The `manifold://` pages: static HTML compiled in, and the one router every engine host calls from its own
//! custom-scheme hook (WebView2's `WebResourceRequested`, Servo's `ProtocolHandler`, CEF's scheme handler factory,
//! WebKit's `register_uri_scheme`). Only the hook differs per engine; the page table lives here.
//! See `docs/superpowers/specs/2026-09-19-manifold-scheme-design.md`.

/// The scheme's name, without `://`. Each engine registers it as standard (so `manifold://welcome` has host
/// `welcome`) and secure.
pub const SCHEME: &str = "manifold";

/// The MIME type of every response, for APIs that take it apart from the charset (CEF, WebKit; the pages carry
/// `<meta charset>`).
pub const MIME_TYPE: &str = "text/html";

/// The `Content-Type` header value of every response, for APIs that take headers (WebView2, Servo).
pub const CONTENT_TYPE: &str = "text/html; charset=utf-8";

const WELCOME_HTML: &str = include_str!("../assets/welcome.html");
const TIPS_HTML: &str = include_str!("../assets/tips.html");
const START_HTML: &str = include_str!("../assets/start.html");
const NOT_FOUND_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><style>body{background:#1E1F24;color:#E6E7EB;font-family:sans-serif;padding:2em}</style></head><body><h1>Not found</h1></body></html>";

/// The response for a `manifold://` request: an HTTP status and the HTML body.
pub fn serve(uri: &str) -> (u16, &'static str) {
    match page_name(uri).and_then(page_content) {
        Some(html) => (200, html),
        None => (404, NOT_FOUND_HTML),
    }
}

/// The host component of a `manifold://<host>...` URI, `None` if the scheme differs.
fn page_name(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix(SCHEME)?.strip_prefix("://")?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or(rest))
}

fn page_content(page: &str) -> Option<&'static str> {
    match page.to_ascii_lowercase().as_str() {
        "welcome" => Some(WELCOME_HTML),
        "tips" => Some(TIPS_HTML),
        "start" => Some(START_HTML),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_name_serves_its_page() {
        for name in ["welcome", "tips", "start"] {
            let (status, html) = serve(&format!("manifold://{name}"));
            assert_eq!((status, html.contains("<title>")), (200, true), "{name}");
        }
        assert_eq!(serve("manifold://START/?x#y").0, 200);
    }

    #[test]
    fn unknown_pages_and_other_schemes_are_not_found() {
        assert_eq!(serve("manifold://nosuch").0, 404);
        assert_eq!(serve("https://start").0, 404);
        assert_eq!(serve("manifoldx://start").0, 404);
    }
}
