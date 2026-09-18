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
    DisplayHandlerBuilder, EngineDisplayHandler, EngineLoadHandler, LoadHandlerBuilder, SharedNavState,
};
use crate::render_handler::{EngineRenderHandler, RenderHandlerBuilder};
use crate::render_process_handler::{IMAGE_PAGE_RESULT, JSON_BODY_RESULT};

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

/// Same shape as `SharedJsonBody`, for the `image-page-result` reply --
/// see `ipc_protocol::EngineToHost::ImagePage`'s own doc comment.
pub type SharedImagePage = Rc<RefCell<bool>>;

pub fn new_shared_image_page() -> SharedImagePage {
    Rc::new(RefCell::new(false))
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
        image_page: SharedImagePage,
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
            if name == IMAGE_PAGE_RESULT {
                *self.image_page.borrow_mut() = true;
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
        pending_open_tab: SharedPendingOpenTab,
        find_result: SharedFindResult,
        download_path: SharedDownloadPath,
        closed: SharedClosed,
        context_menu_link: SharedContextMenuLink,
        json_body: SharedJsonBody,
        image_page: SharedImagePage,
    ) -> Client {
        Self::new(
            RenderHandlerBuilder::build(render_handler),
            DisplayHandlerBuilder::build(EngineDisplayHandler::new(nav_state.clone())),
            LoadHandlerBuilder::build(EngineLoadHandler::new(nav_state)),
            LifeSpanHandlerBuilder::build(EngineLifeSpanHandler::new(pending_open_tab, closed)),
            FindHandlerBuilder::build(EngineFindHandler::new(find_result)),
            DownloadHandlerBuilder::build(EngineDownloadHandler::new(download_path)),
            ContextMenuHandlerBuilder::build(EngineContextMenuHandler::new(context_menu_link)),
            json_body,
            image_page,
        )
    }
}
