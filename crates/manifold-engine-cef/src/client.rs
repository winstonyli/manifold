use std::cell::RefCell;
use std::rc::Rc;

use cef::{rc::Rc as CefRc, *};

use crate::context_menu_handler::{
    ContextMenuHandlerBuilder, EngineContextMenuHandler, SharedContextMenuLink,
};
use crate::download_handler::{DownloadHandlerBuilder, EngineDownloadHandler, SharedDownloadPath};
use crate::find_handler::{EngineFindHandler, FindHandlerBuilder, SharedFindResult};
use crate::life_span_handler::{
    EngineLifeSpanHandler, LifeSpanHandlerBuilder, SharedClosed, SharedPendingOpenTab,
};
use crate::nav_handler::{
    DisplayHandlerBuilder, EngineDisplayHandler, EngineLoadHandler, LoadHandlerBuilder, SharedMuted,
    SharedNavState,
};
use crate::render_handler::{EngineRenderHandler, RenderHandlerBuilder};
use crate::render_process_handler::{AUDIO_PLAYING_RESULT, JSON_BODY_RESULT, REDIRECT_RESULT};

/// The renderer subprocess's own answer to a `check-json-body`
/// `ProcessMessage` (see `render_process_handler.rs`) -- `Client::
/// on_process_message_received` below is where a reply *arrives* in the
/// browser process; `main.rs`'s own poll loop drains it into
/// `EngineToHost::JsonBody`, same "shared-RefCell-drained-by-the-main-loop"
/// shape `find_handler.rs`/`download_handler.rs` already use.
pub type SharedJsonBody = Rc<RefCell<Option<String>>>;

pub fn new_shared_json_body() -> SharedJsonBody {
    Rc::new(RefCell::new(None))
}

/// Same shape as `SharedJsonBody`, for the `redirect-content-result` reply
/// -- see `ipc_protocol::EngineToHost::RedirectableContent`'s own doc
/// comment.
pub type SharedRedirectContentType = Rc<RefCell<Option<String>>>;

pub fn new_shared_redirect_content_type() -> SharedRedirectContentType {
    Rc::new(RefCell::new(None))
}

/// The renderer's latest `check-audio-playing` reply -- unlike
/// `SharedJsonBody`/`SharedImagePage`, this is always overwritten with
/// every reply (never cleared to `None` in between), since `nav_handler.rs`'s
/// polling timer sends a request continuously rather than once per
/// navigation; `main.rs`'s own drain loop is what dedupes against the
/// last-known value before ever emitting `EngineToHost::AudioPlayingChanged`.
pub type SharedAudioPlaying = Rc<RefCell<Option<bool>>>;

pub fn new_shared_audio_playing() -> SharedAudioPlaying {
    Rc::new(RefCell::new(None))
}

wrap_client! {
    pub struct ClientBuilder {
        render_handler: RenderHandler,
        display_handler: DisplayHandler,
        load_handler: LoadHandler,
        life_span_handler: LifeSpanHandler,
        find_handler: FindHandler,
        download_handler: DownloadHandler,
        context_menu_handler: ContextMenuHandler,
        json_body: SharedJsonBody,
        redirect_content_type: SharedRedirectContentType,
        audio_playing: SharedAudioPlaying,
    }

    impl Client {
        fn on_process_message_received(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            let Some(message) = message else { return 0 };
            let name = CefString::from(&message.name()).to_string();
            if name == JSON_BODY_RESULT {
                if let Some(args) = message.argument_list() {
                    *self.json_body.borrow_mut() = Some(CefString::from(&args.string(0)).to_string());
                }
                return 1;
            }
            if name == REDIRECT_RESULT {
                if let Some(args) = message.argument_list() {
                    *self.redirect_content_type.borrow_mut() =
                        Some(CefString::from(&args.string(0)).to_string());
                }
                return 1;
            }
            if name == AUDIO_PLAYING_RESULT {
                if let Some(args) = message.argument_list() {
                    *self.audio_playing.borrow_mut() = Some(args.bool(0) != 0);
                }
                return 1;
            }
            0
        }

        fn render_handler(&self) -> Option<cef::RenderHandler> {
            Some(self.render_handler.clone())
        }

        fn display_handler(&self) -> Option<cef::DisplayHandler> {
            Some(self.display_handler.clone())
        }

        fn load_handler(&self) -> Option<cef::LoadHandler> {
            Some(self.load_handler.clone())
        }

        fn life_span_handler(&self) -> Option<cef::LifeSpanHandler> {
            Some(self.life_span_handler.clone())
        }

        fn find_handler(&self) -> Option<cef::FindHandler> {
            Some(self.find_handler.clone())
        }

        fn download_handler(&self) -> Option<cef::DownloadHandler> {
            Some(self.download_handler.clone())
        }

        fn context_menu_handler(&self) -> Option<cef::ContextMenuHandler> {
            Some(self.context_menu_handler.clone())
        }
    }
}

impl ClientBuilder {
    pub fn build(
        render_handler: EngineRenderHandler,
        nav_state: SharedNavState,
        muted: SharedMuted,
        pending_open_tab: SharedPendingOpenTab,
        find_result: SharedFindResult,
        download_path: SharedDownloadPath,
        closed: SharedClosed,
        context_menu_link: SharedContextMenuLink,
        json_body: SharedJsonBody,
        redirect_content_type: SharedRedirectContentType,
        audio_playing: SharedAudioPlaying,
    ) -> Client {
        Self::new(
            RenderHandlerBuilder::build(render_handler),
            DisplayHandlerBuilder::build(EngineDisplayHandler::new(nav_state.clone())),
            LoadHandlerBuilder::build(EngineLoadHandler::new(nav_state, muted)),
            LifeSpanHandlerBuilder::build(EngineLifeSpanHandler::new(pending_open_tab, closed)),
            FindHandlerBuilder::build(EngineFindHandler::new(find_result)),
            DownloadHandlerBuilder::build(EngineDownloadHandler::new(download_path)),
            ContextMenuHandlerBuilder::build(EngineContextMenuHandler::new(context_menu_link)),
            json_body,
            redirect_content_type,
            audio_playing,
        )
    }
}
