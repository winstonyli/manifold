//! The `manifold://` pages: static HTML compiled in, and the one router every engine host calls from its own
//! custom-scheme hook (WebView2's `WebResourceRequested`, Servo's `ProtocolHandler`, CEF's scheme handler factory,
//! WebKit's `register_uri_scheme`). Only the hook differs per engine; the page table lives here.
//! See `docs/superpowers/specs/2026-09-19-manifold-scheme-design.md`.

/// The app's colors, as `[r, g, b]`: the one place they are defined. `manifold`'s `theme` builds its egui colors from
/// these, and every page below gets them as CSS variables (`--bg`, `--panel`, `--raised`, `--hover`, `--outline`, `--text`,
/// `--weak`, `--accent`, `--danger`), so the chrome and the pages cannot drift apart.
pub mod palette {
    #[derive(Debug, PartialEq, Eq)]
    pub struct Colors {
        pub bg: [u8; 3],
        pub toolbar_fill: [u8; 3],
        pub surface_raised: [u8; 3],
        pub surface_hover: [u8; 3],
        /// A resting-but-visible border: a hovered control's outline (the accent is kept for selected and focused).
        pub outline: [u8; 3],
        pub text_primary: [u8; 3],
        pub text_weak: [u8; 3],
        pub accent: [u8; 3],
        pub danger: [u8; 3],
    }

    pub const DARK: Colors = Colors {
        bg: [23, 20, 15],
        toolbar_fill: [29, 26, 20],
        surface_raised: [41, 36, 27],
        surface_hover: [58, 50, 37],
        outline: [140, 127, 100],
        text_primary: [233, 226, 211],
        text_weak: [178, 164, 130],
        accent: [215, 162, 74],
        danger: [224, 112, 84],
    };

    /// One table today; `LIGHT` arrives with the light theme and `colors` then picks by `dark`.
    pub fn colors(_dark: bool) -> &'static Colors {
        &DARK
    }
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
const NOT_FOUND_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><style>/*BASE*/</style></head><body><h1>Not found</h1></body></html>";
/// The stylesheet every page starts from (the palette variables, the body, headings and key caps).
const BASE_CSS: &str = include_str!("../assets/base.css");

/// `:root` declarations for `palette`, substituted for each page's `/*PALETTE*/`.
fn palette_vars() -> String {
    let c = palette::colors(true);
    let hex = |[r, g, b]: [u8; 3]| format!("#{r:02X}{g:02X}{b:02X}");
    [
        ("bg", c.bg),
        ("panel", c.toolbar_fill),
        ("raised", c.surface_raised),
        ("hover", c.surface_hover),
        ("outline", c.outline),
        ("text", c.text_primary),
        ("weak", c.text_weak),
        ("accent", c.accent),
        ("danger", c.danger),
    ]
    .map(|(name, color)| format!("--{name}:{};", hex(color)))
    .concat()
}

/// Every page with the base stylesheet and its palette filled in, built once.
fn themed(index: usize) -> &'static str {
    static PAGES: std::sync::OnceLock<[String; 4]> = std::sync::OnceLock::new();
    &PAGES.get_or_init(|| {
        [WELCOME_HTML, TIPS_HTML, START_HTML, NOT_FOUND_HTML]
            .map(|html| html.replace("/*BASE*/", BASE_CSS).replace("/*PALETTE*/", &palette_vars()))
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
            assert!(!html.contains("/*PALETTE*/") && !html.contains("/*BASE*/"), "{name}");
        }
        for html in [WELCOME_HTML, TIPS_HTML, START_HTML] {
            let style = html.split("</style>").next().unwrap();
            assert!(!style.contains("#1E1F24") && !style.contains("#58A6FF"), "an old hardcoded color is left");
        }
    }

    #[test]
    fn dark_table_keeps_todays_values() {
        assert_eq!(palette::DARK.bg, [23, 20, 15]);
        assert_eq!(palette::DARK.accent, [215, 162, 74]);
        assert_eq!(palette::colors(true), &palette::DARK);
    }

    #[test]
    fn unknown_pages_and_other_schemes_are_not_found() {
        assert_eq!(serve("manifold://nosuch").0, 404);
        assert_eq!(serve("https://start").0, 404);
        assert_eq!(serve("manifoldx://start").0, 404);
    }
}
