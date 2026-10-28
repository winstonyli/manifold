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
        bg: [41, 39, 35],
        toolbar_fill: [47, 45, 40],
        surface_raised: [58, 55, 49],
        surface_hover: [70, 67, 60],
        outline: [152, 147, 136],
        text_primary: [236, 233, 226],
        text_weak: [186, 181, 170],
        accent: [143, 176, 255],
        danger: [240, 132, 112],
    };

    pub const LIGHT: Colors = Colors {
        bg: [235, 232, 225],
        toolbar_fill: [245, 243, 238],
        surface_raised: [255, 255, 255],
        surface_hover: [240, 237, 230],
        outline: [135, 130, 120],
        text_primary: [34, 32, 28],
        text_weak: [100, 95, 86],
        accent: [40, 90, 215],
        danger: [190, 50, 30],
    };

    pub fn colors(dark: bool) -> &'static Colors {
        if dark {
            &DARK
        } else {
            &LIGHT
        }
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

/// `:root` declarations for the `dark` or light `palette`, substituted for each page's `/*PALETTE*/`.
fn palette_vars(dark: bool) -> String {
    let c = palette::colors(dark);
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

static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The palette `serve` currently fills in.
pub fn is_dark() -> bool {
    DARK.load(std::sync::atomic::Ordering::Relaxed)
}

/// Which palette `serve` fills in: each host sets it from the app's settings
/// (`manifold_view_protocol::host_env::apply`); a page already open keeps the colors it was served with until reloaded.
pub fn set_dark(dark: bool) {
    DARK.store(dark, std::sync::atomic::Ordering::Relaxed);
}

/// Every page with the base stylesheet and the current palette filled in, built once per theme.
fn themed(index: usize) -> &'static str {
    static PAGES: [std::sync::OnceLock<[String; 4]>; 2] = [std::sync::OnceLock::new(), std::sync::OnceLock::new()];
    let dark = DARK.load(std::sync::atomic::Ordering::Relaxed);
    &PAGES[usize::from(dark)].get_or_init(|| {
        [WELCOME_HTML, TIPS_HTML, START_HTML, NOT_FOUND_HTML]
            .map(|html| html.replace("/*BASE*/", BASE_CSS).replace("/*PALETTE*/", &palette_vars(dark)))
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

    /// `set_dark` is process-global: tests that read page colors hold this and set the theme they expect.
    static THEME: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _theme = THEME.lock().unwrap_or_else(|e| e.into_inner());
        set_dark(true);
        for name in ["welcome", "tips", "start", "nosuch"] {
            let html = serve(&format!("manifold://{name}")).1;
            assert!(html.contains("--bg:#292723;") && html.contains("--accent:#8FB0FF;"), "{name}");
            assert!(!html.contains("/*PALETTE*/") && !html.contains("/*BASE*/"), "{name}");
        }
        for html in [WELCOME_HTML, TIPS_HTML, START_HTML] {
            let style = html.split("</style>").next().unwrap();
            assert!(!style.contains("#1E1F24") && !style.contains("#58A6FF"), "an old hardcoded color is left");
        }
    }

    #[test]
    fn pages_follow_set_dark() {
        let _theme = THEME.lock().unwrap_or_else(|e| e.into_inner());
        set_dark(false);
        assert!(serve("manifold://start").1.contains("--bg:#EBE8E1;"));
        set_dark(true);
        assert!(serve("manifold://start").1.contains("--bg:#292723;"));
    }

    #[test]
    fn dark_table_has_the_soft_values() {
        assert_eq!(palette::DARK.bg, [41, 39, 35]);
        assert_eq!(palette::DARK.accent, [143, 176, 255]);
        assert_eq!(palette::colors(true), &palette::DARK);
    }

    #[test]
    fn unknown_pages_and_other_schemes_are_not_found() {
        assert_eq!(serve("manifold://nosuch").0, 404);
        assert_eq!(serve("https://start").0, 404);
        assert_eq!(serve("manifoldx://start").0, 404);
    }
}
