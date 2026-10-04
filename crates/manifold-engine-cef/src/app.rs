use cef::{self, rc::Rc, BrowserProcessHandler, ImplBrowserProcessHandler, WrapBrowserProcessHandler, *};

use crate::render_process_handler::{EngineRenderProcessHandler, RenderProcessHandlerBuilder};

/// Top-level CEF app callbacks. Mirrors tauri-apps/cef-rs's `examples/osr`
/// reference: append the switches that make windowless (OSR) rendering
/// behave in a headless-friendly way (no crash-restore UI, no error
/// dialogs — this process has no window at all).
#[derive(Clone)]
pub struct EngineApp {}

impl EngineApp {
    pub fn new() -> Self {
        Self {}
    }
}

wrap_app! {
    pub struct AppBuilder {
        app: EngineApp,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&cef::CefStringUtf16>,
            command_line: Option<&mut cef::CommandLine>,
        ) {
            let Some(command_line) = command_line else {
                return;
            };
            command_line.append_switch(Some(&"no-sandbox".into()));
            command_line.append_switch(Some(&"no-startup-window".into()));
            command_line.append_switch(Some(&"noerrdialogs".into()));
            command_line.append_switch(Some(&"hide-crash-restore-bubble".into()));
            command_line.append_switch(Some(&"use-mock-keychain".into()));
            // Chromium 151 made a first-run EULA required on Linux; with no
            // dialog to show, initialize fails with exit code 28
            // (CHROME_RESULT_CODE_EULA_REFUSED). An embedded engine has no
            // first run at all.
            command_line.append_switch(Some(&"no-first-run".into()));
            command_line.append_switch_with_value(Some(&"enable-logging".into()), Some(&"stderr".into()));
            // Chromium's verbose log is a debugging aid, and per-tab logs append across spawns:
            // only on request.
            if std::env::var_os("MANIFOLD_CEF_VERBOSE").is_some_and(|v| v == "1") {
                command_line.append_switch_with_value(Some(&"v".into()), Some(&"1".into()));
            }
        }

        fn browser_process_handler(&self) -> Option<cef::BrowserProcessHandler> {
            Some(BrowserProcessHandlerBuilder::build(
                EngineBrowserProcessHandler::new(),
            ))
        }

        // Only ever actually invoked by CEF inside a renderer subprocess --
        // this same `App` is also handed to the browser-process `initialize`
        // call (see `main.rs`), where CEF never calls this getter at all, so
        // returning a real handler unconditionally is harmless there.
        fn render_process_handler(&self) -> Option<cef::RenderProcessHandler> {
            Some(RenderProcessHandlerBuilder::build(
                EngineRenderProcessHandler::new(),
            ))
        }
    }
}

impl AppBuilder {
    pub fn build(app: EngineApp) -> cef::App {
        Self::new(app)
    }
}

#[derive(Clone)]
pub struct EngineBrowserProcessHandler {}

impl EngineBrowserProcessHandler {
    pub fn new() -> Self {
        Self {}
    }
}

wrap_browser_process_handler! {
    pub struct BrowserProcessHandlerBuilder {
        handler: EngineBrowserProcessHandler,
    }

    impl BrowserProcessHandler {}
}

impl BrowserProcessHandlerBuilder {
    pub fn build(handler: EngineBrowserProcessHandler) -> BrowserProcessHandler {
        Self::new(handler)
    }
}
