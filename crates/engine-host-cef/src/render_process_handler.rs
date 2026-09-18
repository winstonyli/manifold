use cef::{rc::Rc, *};

/// Runs inside the renderer subprocess -- this same exe, re-launched by CEF
/// itself with `--type=renderer` (see `main.rs`'s `execute_process` call,
/// which now hands over a real `App` for exactly this reason). Answers
/// `browser-ui`'s "is this page JSON?" question the way `ipc_protocol::
/// EngineToHost::JsonBody`'s own doc comment describes for the other three
/// engines, but through `V8Context::eval` + a `ProcessMessage` reply rather
/// than a script-eval-with-a-result-callback API -- `Frame::execute_java_script`
/// (used for `ContextMenuLink` on other engines) has no way to return a
/// value at all; a `ProcessMessage` round-trip is CEF's real mechanism for a
/// renderer to hand a result back to the browser process. See
/// `nav_handler.rs`'s `on_loading_state_change` for the request side, and
/// `client.rs`'s `on_process_message_received` for where the reply below
/// gets picked up.
pub const CHECK_JSON_BODY: &str = "check-json-body";
pub const JSON_BODY_RESULT: &str = "json-body-result";
/// See `ipc_protocol::EngineToHost::ImagePage`'s own doc comment -- same
/// request/`document.contentType` check as `JSON_BODY_RESULT`, just the
/// other branch; carries no argument list of its own since `browser-ui`
/// doesn't need anything back beyond "yes, this is an image."
pub const IMAGE_PAGE_RESULT: &str = "image-page-result";

#[derive(Clone)]
pub struct EngineRenderProcessHandler {}

impl EngineRenderProcessHandler {
    pub fn new() -> Self {
        Self {}
    }
}

wrap_render_process_handler! {
    pub struct RenderProcessHandlerBuilder {
        handler: EngineRenderProcessHandler,
    }

    impl RenderProcessHandler {
        fn on_process_message_received(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            let (Some(frame), Some(message)) = (frame, message) else {
                return 0;
            };
            if CefString::from(&message.name()).to_string() != CHECK_JSON_BODY {
                return 0;
            }

            // Only reply for an actual outcome -- `browser-ui` already
            // clears a tab's `json_body` on any real URL change regardless
            // of which engine it's on (see `EngineToHost::JsonBody`'s own
            // doc comment), and simply never gets an `ImagePage` for a page
            // that isn't one, so a "neither" page needs no reply at all.
            match frame.v8_context().map(|context| eval_page_kind(&context)) {
                Some(Some(PageKind::Json(body))) => {
                    if let Some(mut reply) = process_message_create(Some(&JSON_BODY_RESULT.into())) {
                        if let Some(args) = reply.argument_list() {
                            args.set_string(0, Some(&body.as_str().into()));
                        }
                        frame.send_process_message(ProcessId::BROWSER, Some(&mut reply));
                    }
                }
                Some(Some(PageKind::Image)) => {
                    if let Some(mut reply) = process_message_create(Some(&IMAGE_PAGE_RESULT.into())) {
                        frame.send_process_message(ProcessId::BROWSER, Some(&mut reply));
                    }
                }
                _ => {}
            }
            1
        }
    }
}

impl RenderProcessHandlerBuilder {
    pub fn build(handler: EngineRenderProcessHandler) -> RenderProcessHandler {
        Self::new(handler)
    }
}

enum PageKind {
    Json(String),
    Image,
}

/// `V8Context::eval` must run inside an entered context when called outside
/// a `cef_v8handler_t`/`cef_v8accessor_t` callback -- CEF's own documented
/// contract for `cef_v8context_t::eval`. `enter`/`exit` bracket every eval
/// this makes (the `contentType` check and, only if it's JSON, the real
/// body read) so a mid-way early return can't ever leave the context
/// entered.
fn eval_page_kind(context: &V8Context) -> Option<PageKind> {
    context.enter();
    let content_type = eval_string(context, "document.contentType");
    let kind = match content_type.as_deref() {
        Some("application/json") => eval_string(context, "document.body.innerText").map(PageKind::Json),
        Some(t) if t.starts_with("image/") => Some(PageKind::Image),
        _ => None,
    };
    context.exit();
    kind
}

fn eval_string(context: &V8Context, code: &str) -> Option<String> {
    let mut retval: Option<V8Value> = None;
    let mut exception: Option<V8Exception> = None;
    let ok = context.eval(Some(&code.into()), None, 0, Some(&mut retval), Some(&mut exception));
    if ok == 0 {
        return None;
    }
    retval.map(|v| CefString::from(&v.string_value()).to_string())
}
