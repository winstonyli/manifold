use cef::{rc::Rc, *};

use crate::life_span_handler::{EngineLifeSpanHandler, LifeSpanHandlerBuilder, SharedPendingOpenTab};
use crate::nav_handler::{
    DisplayHandlerBuilder, EngineDisplayHandler, EngineLoadHandler, LoadHandlerBuilder,
    SharedNavState,
};
use crate::render_handler::{EngineRenderHandler, RenderHandlerBuilder};

wrap_client! {
    pub struct ClientBuilder {
        render_handler: RenderHandler,
        display_handler: DisplayHandler,
        load_handler: LoadHandler,
        life_span_handler: LifeSpanHandler,
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
    }
}

impl ClientBuilder {
    pub fn build(
        render_handler: EngineRenderHandler,
        nav_state: SharedNavState,
        pending_open_tab: SharedPendingOpenTab,
    ) -> Client {
        Self::new(
            RenderHandlerBuilder::build(render_handler),
            DisplayHandlerBuilder::build(EngineDisplayHandler::new(nav_state.clone())),
            LoadHandlerBuilder::build(EngineLoadHandler::new(nav_state)),
            LifeSpanHandlerBuilder::build(EngineLifeSpanHandler::new(pending_open_tab)),
        )
    }
}
