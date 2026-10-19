//! Serves the `manifold://` pages (`manifold_pages`) through a CEF scheme handler factory. The scheme is declared
//! to every CEF process (`on_register_custom_schemes`, from `app.rs`); the factory is browser-process only.

use cef::wrapper::stream_resource_handler::StreamResourceHandler;
use cef::*;

/// `manifold` behaves like https: a host (`manifold://welcome`), a secure context, CORS and `fetch`.
pub fn register_scheme(registrar: &mut SchemeRegistrar) {
    let options = SchemeOptions::STANDARD.get_raw()
        | SchemeOptions::SECURE.get_raw()
        | SchemeOptions::CORS_ENABLED.get_raw()
        | SchemeOptions::FETCH_ENABLED.get_raw();
    registrar.add_custom_scheme(Some(&manifold_pages::SCHEME.into()), options);
}

/// Registers the factory; call in the browser process after `initialize`.
pub fn register_factory() {
    let mut factory = PagesFactory::new();
    register_scheme_handler_factory(Some(&manifold_pages::SCHEME.into()), None, Some(&mut factory));
}

wrap_scheme_handler_factory! {
    struct PagesFactory;

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let url = CefString::from(&request?.url()).to_string();
            let (status, html) = manifold_pages::serve(&url);
            // `html` is 'static; the reader only reads it.
            let stream = stream_reader_create_for_data(html.as_ptr().cast_mut(), html.len())?;
            let reason = if status == 200 { "OK" } else { "Not Found" };
            Some(StreamResourceHandler::new(
                i32::from(status),
                reason.to_string(),
                manifold_pages::MIME_TYPE.to_string(),
                None,
                Some(stream),
            ))
        }
    }
}
