use cef::{rc::Rc, *};

/// Runs inside the renderer subprocess -- this same exe, re-launched by CEF
/// itself with `--type=renderer` (see `main.rs`'s `execute_process` call,
/// which now hands over a real `App` for exactly this reason). Answers
/// `manifold`'s "is this page JSON?" question the way `ipc_protocol::
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
/// See `ipc_protocol::EngineToHost::RedirectableContent`'s own doc comment
/// -- same request/`document.contentType` check as `JSON_BODY_RESULT`, just
/// the other branch; carries the raw content-type string as its one
/// argument, same as every other engine forwards it, so `manifold`'s own
/// allowlist (not this handler) decides what it means.
pub const REDIRECT_RESULT: &str = "redirect-content-result";
/// See `ipc_protocol::EngineToHost::AudioPlayingChanged`'s own doc comment
/// for why CEF never sent this before -- a real native read would need the
/// same all-or-nothing `AudioHandler` `HostToEngine::SetMuted`'s own doc
/// comment already rules out. This is a *different*, lighter mechanism:
/// `nav_handler.rs`'s own polling timer sends `CHECK_AUDIO_PLAYING`
/// periodically (not once per navigation, unlike `CHECK_JSON_BODY` --
/// there's no single event to hook, audio can start/stop at any time), and
/// the reply is a real boolean argument, not a string.
pub const CHECK_AUDIO_PLAYING: &str = "check-audio-playing";
pub const AUDIO_PLAYING_RESULT: &str = "audio-playing-result";

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
            let name = CefString::from(&message.name()).to_string();
            if name == CHECK_AUDIO_PLAYING {
                if let Some(mut reply) = process_message_create(Some(&AUDIO_PLAYING_RESULT.into())) {
                    if let Some(args) = reply.argument_list() {
                        let playing = frame.v8_context().is_some_and(|context| eval_audio_playing(&context));
                        args.set_bool(0, playing as i32);
                    }
                    frame.send_process_message(ProcessId::BROWSER, Some(&mut reply));
                }
                return 1;
            }
            if name != CHECK_JSON_BODY {
                return 0;
            }

            // Only reply for an actual outcome -- `manifold` already
            // clears a tab's `json_body` on any real URL change regardless
            // of which engine it's on (see `EngineToHost::JsonBody`'s own
            // doc comment), and a missing/unreadable `document.contentType`
            // needs no reply at all (unlike the other three engines, which
            // always forward whatever content-type string they got -- CEF's
            // own `eval` can genuinely come back empty if the V8 context
            // isn't in a state to run it, which isn't "this page has no
            // content-type", so treating that as silence rather than an
            // empty-string report is deliberate here).
            match frame.v8_context().map(|context| eval_page_kind(&context)) {
                Some(Some(PageKind::Json(body))) => {
                    if let Some(mut reply) = process_message_create(Some(&JSON_BODY_RESULT.into())) {
                        if let Some(args) = reply.argument_list() {
                            args.set_string(0, Some(&body.as_str().into()));
                        }
                        frame.send_process_message(ProcessId::BROWSER, Some(&mut reply));
                    }
                }
                Some(Some(PageKind::Other(content_type))) => {
                    if let Some(mut reply) = process_message_create(Some(&REDIRECT_RESULT.into())) {
                        if let Some(args) = reply.argument_list() {
                            args.set_string(0, Some(&content_type.as_str().into()));
                        }
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
    /// Any other (non-JSON) content-type, carried through verbatim --
    /// `manifold`'s own `redirect_fetch::classify` allowlist is what
    /// actually decides whether it means anything.
    Other(String),
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
    let kind = match content_type {
        Some(t) if t == "application/json" => eval_string(context, "document.body.innerText").map(PageKind::Json),
        Some(t) => Some(PageKind::Other(t)),
        None => None,
    };
    context.exit();
    kind
}

/// Same media-element-only approximation `HostToEngine::SetMuted`'s own CEF
/// handler uses for the write side: true if any `<video>`/`<audio>` element
/// is currently playing and not muted. Misses audio the page produces
/// through the Web Audio API alone, with no media element behind it (some
/// games/synths) -- a real native read (WebView2/WebKit's own passive
/// property) has no such gap, but doesn't exist for CEF at all without the
/// full `AudioHandler` `ipc_protocol::EngineToHost::AudioPlayingChanged`'s
/// own doc comment already rules out as disproportionate. This covers the
/// common case (YouTube, most players) without that cost.
fn eval_audio_playing(context: &V8Context) -> bool {
    context.enter();
    let mut retval: Option<V8Value> = None;
    let mut exception: Option<V8Exception> = None;
    let ok = context.eval(
        Some(
            &"Array.from(document.querySelectorAll('video,audio')).some(function(el){return !el.paused&&!el.muted;});"
                .into(),
        ),
        None,
        0,
        Some(&mut retval),
        Some(&mut exception),
    );
    let playing = ok != 0 && retval.is_some_and(|v| v.is_bool() != 0 && v.bool_value() != 0);
    context.exit();
    playing
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
