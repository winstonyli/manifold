//! The `manifold://` pages: static HTML compiled in, and the one router every engine host calls from its own
//! custom-scheme hook (WebView2's `WebResourceRequested`, Servo's `ProtocolHandler`, CEF's scheme handler factory,
//! WebKit's `register_uri_scheme`). Only the hook differs per engine; the page table lives here.
//! See `docs/superpowers/specs/2026-09-19-manifold-scheme-design.md`.

/// The app's colors, as `[r, g, b]`: the one place they are defined. `manifold`'s `theme` builds its egui colors from
/// these, and every page below gets them as CSS variables (`--bg`, `--raised`, `--text`, `--weak`, `--accent`,
/// `--danger`), so the chrome and the pages cannot drift apart.
pub mod palette {
    pub const BG: [u8; 3] = [23, 20, 15];
    pub const SURFACE_RAISED: [u8; 3] = [41, 36, 27];
    pub const TEXT_PRIMARY: [u8; 3] = [233, 226, 211];
    pub const TEXT_WEAK: [u8; 3] = [178, 164, 130];
    pub const ACCENT: [u8; 3] = [215, 162, 74];
    pub const DANGER: [u8; 3] = [224, 112, 84];
}

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
const NOT_FOUND_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><style>:root{/*PALETTE*/}body{background:var(--bg);color:var(--text);font-family:sans-serif;padding:2em}</style></head><body><h1>Not found</h1></body></html>";

/// `:root` declarations for `palette`, substituted for each page's `/*PALETTE*/`.
fn palette_vars() -> String {
    use palette::*;
    let hex = |[r, g, b]: [u8; 3]| format!("#{r:02X}{g:02X}{b:02X}");
    [
        ("bg", BG),
        ("raised", SURFACE_RAISED),
        ("text", TEXT_PRIMARY),
        ("weak", TEXT_WEAK),
        ("accent", ACCENT),
        ("danger", DANGER),
    ]
    .map(|(name, color)| format!("--{name}:{};", hex(color)))
    .concat()
}

/// Every page with its palette filled in, built once.
fn themed(index: usize) -> &'static str {
    static PAGES: std::sync::OnceLock<[String; 4]> = std::sync::OnceLock::new();
    &PAGES.get_or_init(|| {
        [WELCOME_HTML, TIPS_HTML, START_HTML, NOT_FOUND_HTML].map(|html| html.replace("/*PALETTE*/", &palette_vars()))
    })[index]
}

/// The response for a `manifold://` request: an HTTP status and the HTML body.
pub fn serve(uri: &str) -> (u16, &'static str) {
    match page_name(uri).and_then(page_content) {
        Some(html) => (200, html),
        None => (404, themed(3)),
    }
}

/// The host component of a `manifold://<host>...` URI, `None` if the scheme differs.
fn page_name(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix(SCHEME)?.strip_prefix("://")?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or(rest))
}

fn page_content(page: &str) -> Option<&'static str> {
    match page.to_ascii_lowercase().as_str() {
        "welcome" => Some(themed(0)),
        "tips" => Some(themed(1)),
        "start" => Some(themed(2)),
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
    fn pages_take_their_colors_from_the_palette() {
        for name in ["welcome", "tips", "start", "nosuch"] {
            let html = serve(&format!("manifold://{name}")).1;
            assert!(html.contains("--bg:#17140F;") && html.contains("--accent:#D7A24A;"), "{name}");
            assert!(!html.contains("/*PALETTE*/"), "{name}");
        }
        for html in [WELCOME_HTML, TIPS_HTML, START_HTML] {
            let style = html.split("</style>").next().unwrap();
            assert!(!style.contains("#1E1F24") && !style.contains("#58A6FF"), "an old hardcoded color is left");
        }
    }

    #[test]
    fn unknown_pages_and_other_schemes_are_not_found() {
        assert_eq!(serve("manifold://nosuch").0, 404);
        assert_eq!(serve("https://start").0, 404);
        assert_eq!(serve("manifoldx://start").0, 404);
    }
}
