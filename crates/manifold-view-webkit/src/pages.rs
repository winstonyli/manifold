//! Serves the `manifold://` pages (`manifold_pages`) through the web context's URI scheme hook.
//! The status is always 200 here: setting one needs `URISchemeResponse` (webkit2gtk `v2_36`).

use gtk::gio::MemoryInputStream;
use gtk::glib;
use webkit2gtk::{URISchemeRequestExt, WebContext, WebContextExt};

/// Registers `manifold://`; call before the first load of the scheme.
pub fn register(context: &WebContext) {
    context.register_uri_scheme(manifold_pages::SCHEME, |request| {
        let uri = request.uri().unwrap_or_default();
        let (_status, html) = manifold_pages::serve(&uri);
        let stream = MemoryInputStream::from_bytes(&glib::Bytes::from_static(html.as_bytes()));
        request.finish(&stream, html.len() as i64, Some(manifold_pages::MIME_TYPE));
    });
}
