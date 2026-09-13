use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;
use ipc_protocol::{transport, EngineToHost, HostToEngine, MessageChannel, TabState};
use webkit2gtk::{WebView, WebViewExt};

const DEFAULT_WIDTH: i32 = 1024;
const DEFAULT_HEIGHT: i32 = 768;

fn main() {
    // Read (and required) so a missing env var fails fast with a clear
    // message, matching the other engine-hosts' contract with
    // process_manager.rs — but this engine-host has no actual use for it:
    // RemoteTransport is keyed by TCP port, not by tab_id (there's no
    // shared memory path here to name a file after).
    let _tab_id = std::env::var("ENGINE_TAB_ID").expect("ENGINE_TAB_ID env var required");
    let initial_url = std::env::var("ENGINE_INITIAL_URL")
        .unwrap_or_else(|_| "https://example.com".to_string());
    let port: u16 = std::env::var("ENGINE_TCP_PORT")
        .expect("ENGINE_TCP_PORT env var required")
        .parse()
        .expect("ENGINE_TCP_PORT must be a u16");
    // The Windows host's own gateway IP, computed by process_manager.rs's
    // spawn command (`ip route`'s default gateway) — not 127.0.0.1. WSL2's
    // NAT networking refuses a guest-to-127.0.0.1-on-host connection
    // outright (ECONNREFUSED), so the host is only reachable this way. See
    // ipc_protocol::transport::listen_remote's doc comment.
    let host_ip =
        std::env::var("ENGINE_HOST_IP").expect("ENGINE_HOST_IP env var required");

    // Single TCP connection back to browser-ui, used for both directions
    // via a cloned handle — mirrors process_manager.rs's spawn_webkit_tab
    // on the Windows side (see that file's comment for why: WSL2 has no
    // shared memory with the Windows host process, so this engine-host
    // uses RemoteTransport, not the SharedFrameBuffer the other engines
    // use).
    let control =
        transport::connect_remote(&host_ip, port).expect("connect to browser-ui");
    let stream = control.into_inner();
    let read_stream = stream.try_clone().expect("clone control stream");
    let mut to_host = MessageChannel::new(stream);
    let mut from_host = MessageChannel::new(read_stream);

    let (tx, rx) = mpsc::channel::<HostToEngine>();
    std::thread::spawn(move || loop {
        match from_host.recv::<HostToEngine>() {
            Ok(msg) => {
                let is_shutdown = matches!(msg, HostToEngine::Shutdown);
                if tx.send(msg).is_err() || is_shutdown {
                    break;
                }
            }
            Err(_) => break,
        }
    });

    gtk::init().expect("gtk::init (is Xvfb running? DISPLAY set?)");

    let webview = WebView::new();
    webview.set_size_request(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    webview.load_uri(&initial_url);

    let offscreen = gtk::OffscreenWindow::new();
    offscreen.add(&webview);
    offscreen.set_default_size(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    offscreen.show_all();

    let mut width = DEFAULT_WIDTH as u32;
    let mut height = DEFAULT_HEIGHT as u32;
    let mut frame_id: u64 = 0;
    // M5: background tabs stay alive but stop sending frames; see
    // HostToEngine::SetVisible's doc comment.
    let mut visible = true;
    // Last-sent NavigationStateChanged fields, compared each tick against
    // the WebView's live properties (`webview.uri()`/`title()`/
    // `is_loading()`/`can_go_back()`/`can_go_forward()`) rather than
    // hooking GObject "notify::" signals — WebKitGTK has no natural poll
    // point other than the same main-loop tick already driving frame
    // capture, so diffing here reuses that instead of adding signal
    // plumbing for what's still just a once-per-16ms check.
    let mut last_nav = (
        initial_url,
        String::new(),
        false,
        false,
        false, // (url, title, can_go_back, can_go_forward, loading)
    );

    loop {
        // Capped, not `while gtk::events_pending()` unconditionally: an
        // earlier attempt at synthesizing input here (see project memory)
        // found that *something* can make GTK/WebKitGTK keep generating
        // more pending events faster than they drain, pegging CPU and
        // starving message draining/frame capture below indefinitely. A
        // hard cap guarantees this loop always reaches those every tick
        // regardless of how many GTK events are actually pending, even
        // though it didn't fully explain or fix that specific case.
        let mut pumped = 0;
        while pumped < 200 && gtk::events_pending() {
            gtk::main_iteration_do(false);
            pumped += 1;
        }

        while let Ok(msg) = rx.try_recv() {
            match msg {
                HostToEngine::Navigate { url, .. } => {
                    webview.load_uri(&url);
                }
                HostToEngine::GoBack => {
                    if webview.can_go_back() {
                        webview.go_back();
                    }
                }
                HostToEngine::GoForward => {
                    if webview.can_go_forward() {
                        webview.go_forward();
                    }
                }
                HostToEngine::Resize {
                    width: w,
                    height: h,
                    ..
                } => {
                    width = w;
                    height = h;
                    webview.set_size_request(w as i32, h as i32);
                    offscreen.resize(w as i32, h as i32);
                }
                HostToEngine::Input(_event) => {
                    // TODO: mouse/keyboard input isn't wired up here. A
                    // synthetic-GDK-event approach (constructing raw
                    // GdkEventKey/EventButton/EventMotion structs by hand
                    // via gdk::Event::new + gdk::Event::put, since the
                    // offscreen widget gets no real X11/Wayland input of
                    // its own) was attempted and reverted: sending
                    // synthetic keyboard events reliably triggered a
                    // runaway CPU loop in GTK's event pump (frame capture
                    // and message draining stalled indefinitely; capping
                    // the `gtk::events_pending()` pump loop's iteration
                    // count did not fix it, only confirmed something keeps
                    // regenerating pending events faster than they drain).
                    // Root cause not identified — see project memory
                    // before re-attempting this.
                }
                HostToEngine::SetZoom { factor } => {
                    webview.set_zoom_level(factor);
                }
                HostToEngine::SetVisible(v) => {
                    visible = v;
                }
                HostToEngine::GetState => {
                    let url = webview
                        .uri()
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| last_nav.0.clone());
                    let _ = to_host.send(&EngineToHost::StateSnapshot(TabState {
                        url,
                        zoom: webview.zoom_level(),
                    }));
                }
                HostToEngine::Shutdown => {
                    return;
                }
            }
        }

        let nav_now = (
            webview
                .uri()
                .map(|u| u.to_string())
                .unwrap_or_else(|| last_nav.0.clone()),
            webview.title().map(|t| t.to_string()).unwrap_or_default(),
            webview.can_go_back(),
            webview.can_go_forward(),
            webview.is_loading(),
        );
        if nav_now != last_nav {
            last_nav = nav_now.clone();
            let _ = to_host.send(&EngineToHost::NavigationStateChanged {
                url: nav_now.0,
                title: nav_now.1,
                can_go_back: nav_now.2,
                can_go_forward: nav_now.3,
                loading: nav_now.4,
            });
        }

        if visible {
            if let Some(pixbuf) = offscreen.pixbuf() {
                if pixbuf.width() as u32 == width && pixbuf.height() as u32 == height {
                    let pixels = tightly_packed_rgba(&pixbuf);
                    frame_id += 1;
                    let _ = to_host.send(&EngineToHost::FrameReady {
                        frame_id,
                        width,
                        height,
                        damage: None,
                        pixels: Some(pixels),
                    });
                }
            }
        }

        std::thread::sleep(Duration::from_millis(16));
    }
}

/// `GdkPixbuf` rows can be padded to a stride wider than `width * 4` for
/// alignment; our IPC contract assumes tightly-packed rows (`width * height
/// * 4` bytes, no padding — see `ipc_protocol::frame_buffer`'s doc comment
/// and `browser-ui`'s texture upload), so copy row-by-row rather than
/// sending the raw buffer as-is.
fn tightly_packed_rgba(pixbuf: &gtk::gdk_pixbuf::Pixbuf) -> Vec<u8> {
    let width = pixbuf.width() as usize;
    let height = pixbuf.height() as usize;
    let stride = pixbuf.rowstride() as usize;
    let channels = pixbuf.n_channels() as usize;
    let src = unsafe { pixbuf.pixels() };

    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        let src_row = &src[y * stride..y * stride + width * channels];
        let dst_row = &mut out[y * width * 4..(y + 1) * width * 4];
        if channels == 4 {
            dst_row.copy_from_slice(src_row);
        } else {
            // No alpha channel: fill with opaque.
            for x in 0..width {
                dst_row[x * 4] = src_row[x * channels];
                dst_row[x * 4 + 1] = src_row[x * channels + 1];
                dst_row[x * 4 + 2] = src_row[x * channels + 2];
                dst_row[x * 4 + 3] = 255;
            }
        }
    }
    out
}
