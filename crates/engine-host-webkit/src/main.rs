mod input;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use gtk::prelude::*;
use ipc_protocol::{transport, EngineToHost, HostToEngine, MessageChannel, TabState};
use webkit2gtk::{
    gio, DownloadExt, FaviconDatabaseExt, FindControllerExt, FindOptions, URIRequestExt,
    WebContextExt, WebView, WebViewExt,
};

use gtk::cairo::{Format, ImageSurface};

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
            Err(_) => {
                // The control socket dropped without a graceful `Shutdown`
                // message — browser-ui.exe exited abnormally (force-killed,
                // crashed) rather than running `Drop for EngineTab`'s normal
                // teardown, which is the only other place `Shutdown` would
                // have come from. Windows still closes the TCP socket on
                // process death regardless of whether any Rust destructor
                // ran, so this `recv` reliably observes it (as an
                // `UnexpectedEof` from `read_exact`) — but before this fix,
                // nothing told the main loop, which kept polling GTK,
                // capturing frames nobody could receive, and re-attempting
                // now-dead sends forever: confirmed live via `taskkill /F
                // /IM browser-ui.exe`, the orphaned engine-host-webkit (and
                // the wsl.exe relay above it) pegged at ~45% CPU
                // indefinitely. Synthesizing `Shutdown` here reuses the
                // existing clean-exit path below for this case too, so a
                // dead parent is torn down exactly like a graceful one.
                let _ = tx.send(HostToEngine::Shutdown);
                break;
            }
        }
    });

    gtk::init().expect("gtk::init (is Xvfb running? DISPLAY set?)");

    // WebKitGTK doesn't track favicons at all — `connect_favicon_notify`
    // never fires, on any page — until a `WebContext` has a favicon
    // database directory explicitly set (confirmed live: without this,
    // silent, no error, just never fires — no signal ever arrives).
    // `WebView::new()` uses the default context, so this must happen
    // before it.
    //
    // Setting the directory alone was found (in an earlier pass) to be
    // necessary but not sufficient — the database initializes and is
    // actively written to (confirmed via the WSL2 filesystem: a real
    // `WebpageIcons.db` + WAL journal), but a real page's favicon still
    // never showed up via `WebView::favicon()`/`connect_favicon_notify`
    // alone. Root cause, found by reading `webkit2gtk`'s own generated
    // bindings rather than guessing further: `WebView::favicon`/
    // `favicon-notify` are a passive property that reflects whatever the
    // favicon database *already has cached* — they don't themselves cause
    // a fetch. The actual fetch trigger is `FaviconDatabase::favicon(uri,
    // cancellable, callback)`, a separate async API on the *database*
    // object (`WebContext::favicon_database()`), which explicitly
    // requests (and if needed, downloads) the icon for a given page URI.
    // Nothing in this codebase ever called it — every prior attempt only
    // ever waited passively for a signal that had nothing to actively
    // trigger it. Kept alive for the process lifetime (not just this
    // block) so it can be used below once a page finishes loading.
    let web_context = webkit2gtk::WebContext::default();
    let favicon_database = web_context.as_ref().and_then(|context| {
        let dir = std::env::temp_dir().join("multiengine-browser-favicons");
        context.set_favicon_database_directory(dir.to_str());
        context.favicon_database()
    });

    // Downloads: WebKitGTK has no default save location of its own (unlike
    // WebView2) -- `decide-destination` fires with the server's suggested
    // filename and MUST call `set_destination` or the download silently
    // never starts, same requirement as CEF's `on_before_download`/
    // `BeforeDownloadCallback::cont`. `$HOME/Downloads` mirrors CEF's
    // `%USERPROFILE%\Downloads` choice for the same reason: this project
    // never shows its own save-as dialog. `connect_finished` (not
    // `received_data_length`/`estimated_progress` polling) is the actual
    // completion signal; `destination()` returns back whatever URI was set
    // above, stripped of its `file://` prefix to match the plain
    // filesystem paths WebView2/CEF report over the same
    // `EngineToHost::DownloadCompleted` message.
    // A queue, not a single slot -- two downloads finishing within the
    // same ~16ms tick would otherwise silently clobber each other.
    let pending_download: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    if let Some(context) = &web_context {
        let pending_download = pending_download.clone();
        context.connect_download_started(move |_context, download| {
            download.connect_decide_destination(|download, suggested_filename| {
                let dir = std::env::var_os("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_default()
                    .join("Downloads");
                let _ = std::fs::create_dir_all(&dir);
                // Server-controlled -- only its final path component is
                // ever used, same `..`/absolute-path guard as CEF's
                // `on_before_download`.
                let name = std::path::Path::new(suggested_filename)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("download");
                let dest = dir.join(name);
                download.set_destination(&format!("file://{}", dest.display()));
                true
            });
            let pending_download = pending_download.clone();
            download.connect_finished(move |download| {
                if let Some(uri) = download.destination() {
                    let path = uri.strip_prefix("file://").unwrap_or(&uri).to_string();
                    pending_download.borrow_mut().push(path);
                }
            });
        });
    }

    let webview = WebView::new();
    webview.set_size_request(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    webview.load_uri(&initial_url);

    let offscreen = gtk::OffscreenWindow::new();
    offscreen.add(&webview);
    offscreen.set_default_size(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    offscreen.show_all();

    // Round 2 on synthetic input (see input.rs's doc comment and project
    // memory for round 1's revert): resolved once here, not per-event —
    // the offscreen widget's default seat's keyboard/pointer devices are
    // attached to every synthetic event `input::send_input` builds.
    let seat = webview.display().default_seat();
    let keyboard_device = seat.as_ref().and_then(|s| s.keyboard());
    let pointer_device = seat.as_ref().and_then(|s| s.pointer());

    // Page-triggered new tabs (target=_blank/window.open()/ctrl+click):
    // "create" is WebKitGTK's popup-interception signal, handed the
    // request's URL *before* anything is created - grab it and always
    // return None (never actually create the popup webview; browser-ui
    // spawns a real child tab for `url` instead, once drained below).
    // A queue, not a single slot -- a page opening several tabs
    // synchronously would otherwise silently lose all but the last one.
    let pending_open_tab: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let pending_open_tab = pending_open_tab.clone();
        // Fully-qualified: `connect_create` also exists on gtk::atk's
        // AtkWindowExt (an unrelated accessibility signal) via the same
        // glob-imported gtk::prelude::*, and plain method-call syntax
        // can't disambiguate which trait's version to use here.
        <WebView as webkit2gtk::WebViewExt>::connect_create(&webview, move |_webview, nav_action| {
            if let Some(uri) = nav_action.request().and_then(|r| r.uri()) {
                pending_open_tab.borrow_mut().push(uri.to_string());
            }
            None
        });
    }

    // Favicon: WebKitGTK only tells us it changed (a GObject property
    // notify, not a poll-friendly value) — stash the decoded pixels here
    // and drain them in the main loop below, same pattern as
    // `pending_open_tab` above.
    let pending_favicon: Rc<RefCell<Option<(Vec<u8>, u32, u32)>>> = Rc::new(RefCell::new(None));
    {
        let pending_favicon = pending_favicon.clone();
        webview.connect_favicon_notify(move |webview| {
            if let Some(surface) = webview.favicon() {
                if let Some(rgba) = favicon_to_rgba(&surface) {
                    *pending_favicon.borrow_mut() = Some(rgba);
                }
            }
        });
    }

    // Audio-playing indicator: `is-playing-audio` is a real GObject notify
    // property (same passive-read shape as `favicon`), unlike CEF's only
    // equivalent (a full `AudioHandler` that takes over raw audio output
    // entirely) -- see `ipc_protocol::EngineToHost::AudioPlayingChanged`'s
    // doc comment.
    let pending_audio_playing: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
    {
        let pending_audio_playing = pending_audio_playing.clone();
        webview.connect_is_playing_audio_notify(move |webview| {
            *pending_audio_playing.borrow_mut() = Some(webview.is_playing_audio());
        });
    }

    // Minimal context menu: a right-click already dispatches a genuine
    // native `ButtonRelease` (input.rs, unchanged), which makes WebKitGTK
    // fire its own real `context-menu` signal -- suppressed here (`true` =
    // "handled, don't show your own") unconditionally, same
    // don't-risk-a-real-popup-window caution `HostToEngine::OpenDevTools`
    // already has to (a real GTK popup here would be override-redirect,
    // not a toplevel, so likely wouldn't hit that exact hang -- but this
    // project has no way to verify that live in this sandboxed session,
    // and suppressing it costs nothing since `input.rs`'s own
    // script-driven href query below is what `browser-ui`'s minimal
    // "Open in new tab" menu actually needs). See `input.rs`'s own
    // `PendingContextMenu`.
    webview.connect_context_menu(|_, _, _, _| true);
    let pending_context_menu: input::PendingContextMenu = Default::default();

    // Find-in-page: WebKitGTK's own `WebKitFindController`, one per
    // `WebView` (`find_controller()` never returns `None` in practice for a
    // real `WebView`, but the type is `Option` so this degrades gracefully
    // to a silent no-op rather than a panic if it ever did). `found-text`/
    // `failed-to-find-text` fire from every `search`/`search_next`/
    // `search_previous` call with the current total match count — WebKit's
    // API has no notion of "which match is currently active" the way CEF's
    // `active_match_ordinal`/WebView2's `ActiveMatchIndex` do, so
    // `active_index` is always reported as 0 here; see
    // `ipc_protocol::EngineToHost::FindResult`'s own doc comment.
    let find_controller = webview.find_controller();
    let pending_find_result: Rc<RefCell<Option<(u32, u32)>>> = Rc::new(RefCell::new(None));
    if let Some(controller) = &find_controller {
        let pending = pending_find_result.clone();
        controller.connect_found_text(move |_controller, count| {
            *pending.borrow_mut() = Some((count, 0));
        });
        let pending = pending_find_result.clone();
        controller.connect_failed_to_find_text(move |_controller| {
            *pending.borrow_mut() = Some((0, 0));
        });
    }

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
                    webview.set_size_request(w as i32, h as i32);
                    offscreen.resize(w as i32, h as i32);
                }
                HostToEngine::Input(event) => {
                    input::send_input(
                        &webview,
                        event,
                        keyboard_device.as_ref(),
                        pointer_device.as_ref(),
                        &pending_context_menu,
                    );
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
                HostToEngine::FindStart { text, match_case } => {
                    if let Some(controller) = &find_controller {
                        let mut options = FindOptions::WRAP_AROUND;
                        if !match_case {
                            options |= FindOptions::CASE_INSENSITIVE;
                        }
                        controller.search(&text, options.bits(), 0);
                    }
                }
                HostToEngine::FindNext => {
                    if let Some(controller) = &find_controller {
                        controller.search_next();
                    }
                }
                HostToEngine::FindPrevious => {
                    if let Some(controller) = &find_controller {
                        controller.search_previous();
                    }
                }
                HostToEngine::FindStop => {
                    if let Some(controller) = &find_controller {
                        controller.search_finish();
                    }
                }
                // Confirmed live: `WebInspector::show()` opens a real,
                // separate top-level `GtkWindow` (unlike the main
                // `WebView`, which only ever lives inside our own
                // `GtkOffscreenWindow`) -- and this project's Xvfb session
                // (`xvfb-run -a`, no `-wm`/window-manager flag) has no
                // window manager running at all. A real toplevel shown with
                // no WM to map it never receives the response GTK's show
                // path waits on, which pegged engine-host-webkit at ~45%
                // CPU indefinitely (confirmed via a live 8-minute hang,
                // process never returned to servicing IPC) rather than
                // just failing visibly. No safe way to open a real window
                // in this specific hosting setup without adding a window
                // manager to the Xvfb session (a bigger change than this
                // warranted) -- silent no-op instead, same per-engine gap
                // as Servo's own missing DevTools API.
                HostToEngine::OpenDevTools => {}
                HostToEngine::SetMuted { muted } => {
                    webview.set_is_muted(muted);
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
            // Fetch the favicon once a load actually finishes (loading
            // true -> false), not on every nav-state tick — see
            // `favicon_database`'s doc comment for why this explicit
            // request is the real trigger, not something that happens on
            // its own just because the page loaded.
            let just_finished_loading = last_nav.4 && !nav_now.4;
            let loaded_url = nav_now.0.clone();
            last_nav = nav_now.clone();
            let _ = to_host.send(&EngineToHost::NavigationStateChanged {
                url: nav_now.0,
                title: nav_now.1,
                can_go_back: nav_now.2,
                can_go_forward: nav_now.3,
                loading: nav_now.4,
            });
            if just_finished_loading {
                if let Some(db) = &favicon_database {
                    let pending_favicon = pending_favicon.clone();
                    db.favicon(&loaded_url, gio::Cancellable::NONE, move |result| {
                        if let Ok(surface) = result {
                            if let Some(rgba) = favicon_to_rgba(&surface) {
                                *pending_favicon.borrow_mut() = Some(rgba);
                            }
                        }
                    });
                }
            }
        }

        for url in pending_open_tab.borrow_mut().drain(..) {
            let _ = to_host.send(&EngineToHost::OpenTabRequested { url });
        }

        if let Some((rgba, width, height)) = pending_favicon.borrow_mut().take() {
            let _ = to_host.send(&EngineToHost::FaviconChanged { rgba, width, height });
        }

        if let Some((count, active_index)) = pending_find_result.borrow_mut().take() {
            let _ = to_host.send(&EngineToHost::FindResult { count, active_index });
        }

        if let Some(playing) = pending_audio_playing.borrow_mut().take() {
            let _ = to_host.send(&EngineToHost::AudioPlayingChanged { playing });
        }

        if let Some((x, y, href)) = pending_context_menu.borrow_mut().take() {
            let _ = to_host.send(&EngineToHost::ContextMenuLink { x, y, href });
        }

        for path in pending_download.borrow_mut().drain(..) {
            let _ = to_host.send(&EngineToHost::DownloadCompleted { path });
        }

        if visible {
            // Send whatever size the offscreen widget's pixbuf actually is
            // — not gated on it matching the last `Resize` message's
            // width/height. Found live: after a tab is respawned (engine
            // swap or crash recovery, as opposed to the very first spawn),
            // `offscreen.resize()`/`webview.set_size_request()` below never
            // actually change `GtkOffscreenWindow`'s real backing surface
            // size away from its `set_default_size` at construction —
            // `pixbuf.width()/height()` stayed at `DEFAULT_WIDTH`/
            // `DEFAULT_HEIGHT` forever in testing, regardless of how many
            // `Resize` messages arrived, so gating on an exact match meant
            // *zero* frames were ever sent post-respawn (confirmed via
            // debug logging: `frame_id` stuck at 0 indefinitely). Sending
            // the pixbuf's own real dimensions sidesteps needing to
            // understand or fix GTK's offscreen-resize negotiation at all
            // — browser-ui's reader already takes width/height from each
            // `FrameReady` message directly (no separate shared-memory
            // sizing contract for this RemoteTransport-based engine-host),
            // so there is no requirement that it match what was requested.
            if let Some(pixbuf) = offscreen.pixbuf() {
                let (pw, ph) = (pixbuf.width() as u32, pixbuf.height() as u32);
                if pw > 0 && ph > 0 {
                    let pixels = tightly_packed_rgba(&pixbuf);
                    frame_id += 1;
                    let _ = to_host.send(&EngineToHost::FrameReady {
                        frame_id,
                        width: pw,
                        height: ph,
                        damage: None,
                        pixels: Some(pixels),
                    });
                }
            }
        }

        std::thread::sleep(Duration::from_millis(16));
    }
}

/// `WebView::favicon()` hands over a cairo `Surface`, not a `GdkPixbuf` —
/// downcast to `ImageSurface` (the only variant with CPU-readable pixel
/// data) and convert its native `ARgb32`/`Rgb24` format to tightly-packed
/// RGBA. `ARgb32`'s pixels are alpha-*premultiplied* 32-bit words in
/// native-endian order, which on this x86-64 target means byte order
/// B,G,R,A in memory — both the channel swap and the alpha-unmultiply are
/// needed, unlike `tightly_packed_rgba`'s pixbuf source which is neither
/// premultiplied nor channel-swapped. Returns `None` for any other format
/// (favicons only ever come back as one of these two in practice) or if
/// the surface can't be downcast/borrowed.
fn favicon_to_rgba(surface: &gtk::cairo::Surface) -> Option<(Vec<u8>, u32, u32)> {
    let mut image = ImageSurface::try_from(surface.clone()).ok()?;
    let format = image.format();
    let width = image.width() as usize;
    let height = image.height() as usize;
    let stride = image.stride() as usize;
    let data = image.data().ok()?;

    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        let src_row = &data[y * stride..y * stride + width * 4];
        let dst_row = &mut out[y * width * 4..(y + 1) * width * 4];
        for x in 0..width {
            let b = src_row[x * 4] as u32;
            let g = src_row[x * 4 + 1] as u32;
            let r = src_row[x * 4 + 2] as u32;
            let a = src_row[x * 4 + 3] as u32;
            let (r, g, b, a) = match format {
                Format::ARgb32 if a > 0 => (r * 255 / a, g * 255 / a, b * 255 / a, a),
                Format::ARgb32 => (0, 0, 0, 0),
                Format::Rgb24 => (r, g, b, 255),
                _ => return None,
            };
            dst_row[x * 4] = r as u8;
            dst_row[x * 4 + 1] = g as u8;
            dst_row[x * 4 + 2] = b as u8;
            dst_row[x * 4 + 3] = a as u8;
        }
    }
    Some((out, width as u32, height as u32))
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
