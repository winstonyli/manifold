//! Serves the `manifold://` pages (`manifold_pages`) through Servo's protocol registry.

use std::future::{ready, Future};
use std::pin::Pin;

use http::HeaderValue;
use servo::protocol_handler::{
    DoneChannel, FetchContext, HttpStatus, ProtocolHandler, ProtocolRegistry, Request, ResourceFetchTiming, Response,
    ResponseBody,
};

struct Pages;

impl ProtocolHandler for Pages {
    fn load<'a>(
        &'a self,
        request: &'a mut Request,
        _done_chan: &mut DoneChannel,
        _context: &FetchContext,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'a>> {
        let url = request.current_url();
        let (status, html) = manifold_pages::serve(url.as_str());
        let mut response = Response::new(url, ResourceFetchTiming::new(request.timing_type()));
        *response.body.lock() = ResponseBody::Done(html.as_bytes().to_vec());
        response.headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static(manifold_pages::CONTENT_TYPE));
        response.status = HttpStatus::new_raw(status, if status == 200 { "OK" } else { "Not Found" }.into());
        Box::pin(ready(response))
    }

    fn is_secure(&self) -> bool {
        true
    }
}

/// The registry holding `manifold://`, for `ServoBuilder::protocol_registry`.
pub fn registry() -> ProtocolRegistry {
    let mut registry = ProtocolRegistry::default();
    registry.register(manifold_pages::SCHEME, Pages).expect("manifold is not a forbidden or taken scheme");
    registry
}
