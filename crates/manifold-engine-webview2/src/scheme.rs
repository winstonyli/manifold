//! The `manifold://` custom URL scheme -- registered at environment
//! creation via `environment_options()`, then served via `register()`'s
//! `WebResourceRequested` handler. Both are called from `main.rs` (see
//! `create_environment`/the webview-setup block) since scheme registration
//! is genuinely split across two different WebView2 lifecycle points: the
//! scheme name itself must be known before the environment is created, but
//! actually serving requests needs a live `ICoreWebView2` to attach the
//! event handler to. See the design doc
//! (`docs/superpowers/specs/2026-09-19-manifold-scheme-design.md`) for why
//! this is a real custom scheme (not an `https://` virtual host) and why
//! pages are compiled in rather than supplied dynamically by `manifold.exe`.

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Environment, ICoreWebView2CustomSchemeRegistration,
    ICoreWebView2EnvironmentOptions, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
};
use webview2_com::{
    take_pwstr, CoreWebView2CustomSchemeRegistration, CoreWebView2EnvironmentOptions,
    WebResourceRequestedEventHandler,
};
use windows::core::HSTRING;
use windows::Win32::UI::Shell::SHCreateMemStream;

/// This scheme's name, without the `://` -- matches the design spec's own
/// `manifold://` naming. `HasAuthorityComponent` (set below) means a URL
/// like `manifold://welcome` parses with `host = "welcome"`, the same
/// shape `http`/`https` URLs already have.
const SCHEME_NAME: &str = "manifold";

/// The real `manifold://welcome` page -- a feature tour, embedded at
/// compile time (same `include_str!`/`include_bytes!` convention the
/// `manifold` crate's own icon/font assets already use).
const WELCOME_HTML: &str = include_str!("../assets/welcome.html");

/// `manifold://tips` -- a short keyboard-shortcut reference, used as one
/// of the first-launch demo group's two member tabs (see `App::
/// spawn_first_launch_demo` in the `manifold` crate).
const TIPS_HTML: &str = include_str!("../assets/tips.html");

/// Builds the `ICoreWebView2EnvironmentOptions` that `create_environment`
/// passes to `CreateCoreWebView2EnvironmentWithOptions`, with the
/// `"manifold"` custom scheme registered on it. Called once, before the
/// environment exists.
pub fn environment_options() -> windows::core::Result<ICoreWebView2EnvironmentOptions> {
    let scheme_registration = CoreWebView2CustomSchemeRegistration::new(SCHEME_NAME.to_string());
    unsafe {
        scheme_registration.set_has_authority_component(true);
        // Local, static content -- no network trust boundary to actually
        // secure, but this avoids WebView2 flagging the page as insecure/
        // mixed content the way a real custom scheme without this flag
        // otherwise would.
        scheme_registration.set_treat_as_secure(true);
    }
    let scheme_registration: ICoreWebView2CustomSchemeRegistration = scheme_registration.into();

    let options = CoreWebView2EnvironmentOptions::default();
    unsafe {
        options.set_scheme_registrations(vec![Some(scheme_registration)]);
    }
    Ok(options.into())
}

/// Attaches the `WebResourceRequested` handler that actually serves
/// `manifold://` pages. Called once per tab, right after the webview is
/// created -- same call shape as `find::register`/`download::register`
/// elsewhere in this crate. Best-effort: if this fails (e.g. an older
/// WebView2 Runtime missing the interface Task 1's environment-options
/// path needs), `manifold://` pages simply won't render for this tab,
/// same soft-degradation posture `find::register`'s own caller already
/// gives its own best-effort registration.
pub fn register(
    environment: &ICoreWebView2Environment,
    webview: &ICoreWebView2,
) -> windows::core::Result<()> {
    unsafe {
        webview.AddWebResourceRequestedFilter(
            &HSTRING::from(format!("{SCHEME_NAME}://*")),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;
    }

    let environment = environment.clone();
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else { return Ok(()) };
        let Ok(request) = (unsafe { args.Request() }) else { return Ok(()) };
        let mut uri = windows::core::PWSTR::null();
        if unsafe { request.Uri(&mut uri) }.is_err() {
            return Ok(());
        }
        let uri = take_pwstr(uri);

        let page = page_name(&uri).and_then(page_content);
        let (status, reason, body) = match page {
            Some(html) => (200, "OK", html),
            None => (
                404,
                "Not Found",
                "<!doctype html><html><head><meta charset=\"utf-8\"><style>body{background:#1E1F24;color:#E6E7EB;font-family:sans-serif;padding:2em}</style></head><body><h1>Not found</h1></body></html>",
            ),
        };
        let Some(stream) = (unsafe { SHCreateMemStream(Some(body.as_bytes())) }) else {
            return Ok(());
        };
        if let Ok(response) = unsafe {
            environment.CreateWebResourceResponse(
                &stream,
                status,
                &HSTRING::from(reason),
                &HSTRING::from("Content-Type: text/html; charset=utf-8"),
            )
        } {
            let _ = unsafe { args.SetResponse(&response) };
        }
        Ok(())
    }));
    let mut token = Default::default();
    unsafe {
        webview.add_WebResourceRequested(&handler, &mut token)?;
    }
    Ok(())
}

/// Extracts the host component from a `manifold://<host>...` URI (`None`
/// if it doesn't even match the scheme) -- same
/// `split(['/', '?', '#']).next()` shape `crates/manifold/src/suggestions.rs`'s
/// own `looks_like_url` already uses to isolate a URL's host part.
fn page_name(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix(&format!("{SCHEME_NAME}://"))?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or(rest))
}

/// The static page router. One `match` arm per known page -- no registry
/// abstraction for what is, for now, a single page (see the design spec's
/// own reasoning for why this is deliberately not over-built).
fn page_content(page: &str) -> Option<&'static str> {
    match page.to_ascii_lowercase().as_str() {
        "welcome" => Some(WELCOME_HTML),
        "tips" => Some(TIPS_HTML),
        _ => None,
    }
}
