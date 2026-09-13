mod gpu;
mod process_manager;

use std::sync::Arc;

use gpu::GpuState;
use ipc_protocol::{frame_path, EngineToHost, HostToEngine, InputEvent, MouseButton, SharedFrameBuffer};
use process_manager::{EngineKind, EngineTab};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

const TAB_ID: &str = "tab-0";
const INITIAL_URL: &str = "https://example.com";
const STARTING_ENGINE: EngineKind = EngineKind::Servo;

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    tab: Option<EngineTab>,
    cursor_pos: (f64, f64),
    /// Set while a swap is in flight: the engine to swap *to*, once the
    /// current tab's `GetState` reply (its `StateSnapshot`) comes back.
    swap_target: Option<EngineKind>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            window: None,
            gpu: None,
            tab: None,
            cursor_pos: (0.0, 0.0),
            swap_target: None,
        }
    }
}

impl App {
    /// M2's swap sequence: GetState -> spawn new engine -> replay (url,
    /// via the new engine's initial_url) -> new engine's first FrameReady
    /// swaps the displayed texture automatically (RedrawRequested always
    /// renders whatever `self.tab` currently is) -> old engine-host is
    /// killed by `EngineTab`'s `Drop` when `self.tab` is replaced.
    ///
    /// No tab UI yet (that's M4), so this is triggered by pressing 'S' —
    /// see `window_event`'s `KeyboardInput` arm.
    fn start_swap(&mut self) {
        let Some(tab) = &mut self.tab else { return };
        if self.swap_target.is_some() {
            return; // one swap at a time
        }
        let target = match tab.kind {
            EngineKind::Servo => EngineKind::Cef,
            EngineKind::Cef => EngineKind::Webkit,
            EngineKind::Webkit => EngineKind::Servo,
        };
        self.swap_target = Some(target);
        tab.send(&HostToEngine::GetState);
    }

    fn finish_swap(&mut self, target: EngineKind, url: String) {
        let window_size = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .unwrap_or(winit::dpi::PhysicalSize::new(1024, 768));
        let dpi_scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());

        // Must drop (and thus Shutdown + kill) the old tab before spawning
        // the new one: both use the same tab_id, so the new engine-host's
        // named-pipe listeners would collide with the old ones otherwise.
        drop(self.tab.take());

        match process_manager::spawn_tab(target, TAB_ID, &url) {
            Ok(mut new_tab) => {
                new_tab.send(&HostToEngine::Resize {
                    width: window_size.width,
                    height: window_size.height,
                    dpi_scale,
                });
                // Dropping the old tab (replaced here) sends it Shutdown
                // and kills the process — see EngineTab's Drop impl.
                self.tab = Some(new_tab);
            }
            Err(err) => {
                eprintln!("swap to {target:?} failed, keeping current engine: {err}");
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("multiengine-browser"))
                .expect("create window"),
        );
        self.gpu = Some(GpuState::new(window.clone()));
        self.tab = Some(
            process_manager::spawn_tab(STARTING_ENGINE, TAB_ID, INITIAL_URL)
                .expect("spawn engine-host"),
        );
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                if let Some(tab) = &mut self.tab {
                    tab.send(&HostToEngine::Resize {
                        width: size.width,
                        height: size.height,
                        dpi_scale: self.window.as_ref().map_or(1.0, |w| w.scale_factor()),
                    });
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                if let Some(tab) = &mut self.tab {
                    tab.send(&HostToEngine::Input(InputEvent::MouseMove {
                        x: position.x,
                        y: position.y,
                    }));
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let (Some(tab), Some(button)) = (&mut self.tab, translate_mouse_button(button))
                {
                    let (x, y) = self.cursor_pos;
                    let event = match state {
                        ElementState::Pressed => InputEvent::MouseDown { x, y, button },
                        ElementState::Released => InputEvent::MouseUp { x, y, button },
                    };
                    tab.send(&HostToEngine::Input(event));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(tab) = &mut self.tab {
                    let (dx, dy) = match delta {
                        winit::event::MouseScrollDelta::LineDelta(x, y) => {
                            (x as f64 * 20.0, y as f64 * 20.0)
                        }
                        winit::event::MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                    };
                    let (x, y) = self.cursor_pos;
                    tab.send(&HostToEngine::Input(InputEvent::Scroll {
                        x,
                        y,
                        delta_x: dx,
                        delta_y: dy,
                    }));
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && event.physical_key
                        == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyS)
                {
                    self.start_swap();
                }
            }
            WindowEvent::RedrawRequested => {
                // Drain to the most recent FrameReady (older ones are stale
                // by the time we get to render) and watch for the
                // StateSnapshot reply to an in-flight swap. Scoped so the
                // borrow of `self.tab` ends before any `&mut self` call
                // below (finish_swap needs one).
                let mut latest = None;
                let mut swap_state = None;
                let mut tab_id_for_frame = None;
                let mut tab_kind_for_frame = None;
                if let Some(tab) = &self.tab {
                    while let Ok(msg) = tab.events_rx.try_recv() {
                        match msg {
                            EngineToHost::FrameReady {
                                width,
                                height,
                                pixels,
                                ..
                            } => {
                                latest = Some((width, height, pixels));
                                tab_id_for_frame = Some(tab.tab_id.clone());
                                tab_kind_for_frame = Some(tab.kind);
                            }
                            EngineToHost::StateSnapshot(state) => {
                                swap_state = Some(state);
                            }
                            _ => {}
                        }
                    }
                }

                if swap_state.is_some() && self.swap_target.is_some() {
                    // .take() only now that we know both are Some — doing
                    // it unconditionally inside the tuple pattern below
                    // would clear swap_target as a side effect even on a
                    // frame where no StateSnapshot arrived yet, discarding
                    // the in-flight swap before its reply ever showed up.
                    let target = self.swap_target.take().unwrap();
                    let state = swap_state.unwrap();
                    self.finish_swap(target, state.url);
                } else if let (Some((width, height, pixels)), Some(tab_id), Some(kind)) =
                    (latest, tab_id_for_frame, tab_kind_for_frame)
                {
                    let format = match kind {
                        EngineKind::Servo | EngineKind::Webkit => wgpu::TextureFormat::Rgba8UnormSrgb,
                        EngineKind::Cef => wgpu::TextureFormat::Bgra8UnormSrgb,
                    };
                    if let Some(pixels) = pixels {
                        // RemoteTransport engine-host (WPE/WebKit over
                        // WSL2): no shared memory with this process, so the
                        // pixels travel inline in the message itself.
                        if let Some(gpu) = &mut self.gpu {
                            gpu.upload_frame(&pixels, width, height, format);
                        }
                    } else if let Ok(buf) =
                        SharedFrameBuffer::open_existing(frame_path(&tab_id), width, height)
                    {
                        // The engine-host truncates-then-grows this file
                        // when (re)creating it (see SharedFrameBuffer::
                        // create), which briefly exposes a 0-byte or
                        // otherwise wrong-sized file to a concurrent
                        // reader. Skip this frame rather than panic on a
                        // mismatched slice — the next redraw tries again.
                        let expected_len = (width as usize) * (height as usize) * 4;
                        if buf.as_slice().len() == expected_len {
                            if let Some(gpu) = &mut self.gpu {
                                gpu.upload_frame(buf.as_slice(), width, height, format);
                            }
                        }
                    }
                }

                if let Some(gpu) = &mut self.gpu {
                    gpu.render();
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn translate_mouse_button(button: WinitMouseButton) -> Option<MouseButton> {
    match button {
        WinitMouseButton::Left => Some(MouseButton::Left),
        WinitMouseButton::Right => Some(MouseButton::Right),
        WinitMouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("run event loop");
}
