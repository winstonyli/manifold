use cef::{rc::Rc, *};

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

wrap_client! {
    pub struct ClientBuilder {
        render_handler: RenderHandler,
        display_handler: DisplayHandler,
        load_handler: LoadHandler,
        life_span_handler: LifeSpanHandler,
        find_handler: FindHandler,
        download_handler: DownloadHandler,
        context_menu_handler: ContextMenuHandler,
    }

    impl Client {
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
    ) -> Client {
        Self::new(
            RenderHandlerBuilder::build(render_handler),
            DisplayHandlerBuilder::build(EngineDisplayHandler::new(nav_state.clone())),
            LoadHandlerBuilder::build(EngineLoadHandler::new(nav_state)),
            LifeSpanHandlerBuilder::build(EngineLifeSpanHandler::new(pending_open_tab, closed)),
            FindHandlerBuilder::build(EngineFindHandler::new(find_result)),
            DownloadHandlerBuilder::build(EngineDownloadHandler::new(download_path)),
            ContextMenuHandlerBuilder::build(EngineContextMenuHandler::new(context_menu_link)),
        )
    }
}
