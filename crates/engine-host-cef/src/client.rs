use cef::{rc::Rc, *};

use crate::render_handler::{EngineRenderHandler, RenderHandlerBuilder};

wrap_client! {
    pub struct ClientBuilder {
        render_handler: RenderHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<cef::RenderHandler> {
            Some(self.render_handler.clone())
        }
    }
}

impl ClientBuilder {
    pub fn build(render_handler: EngineRenderHandler) -> Client {
        Self::new(RenderHandlerBuilder::build(render_handler))
    }
}
