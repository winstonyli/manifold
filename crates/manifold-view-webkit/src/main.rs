mod input;
mod pages;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use javascriptcore::ValueExt;
use manifold_view_protocol::{transport, HostToView, MessageChannel, TabState, ViewToHost};
use webkit2gtk::{
    BackForwardListExt, BackForwardListItemExt, DownloadExt, FindControllerExt, FindOptions, URIRequestExt,
    URIResponseExt, WebContextExt, WebView, WebViewExt,
};

const DEFAULT_WIDTH: i32 = 1024;
const DEFAULT_HEIGHT: i32 = 768;

/// At most one frame per this, as WebView2's own `FRAME_INTERVAL`: about 30 fps.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

fn main() {
    manifold_pages::set_dark(std::env::var_os(manifold_view_protocol::host_env::ENV_DARK).is_some());
    // Hex of the UTF-8 URL (process_manager.rs's webkit_shell_command): page text crosses the WSL shells
    // only as hex. A missing variable keeps the old default; a malformed one is a bug in the host.
    let initial_url = match std::env::var("VIEW_INITIAL_URL_HEX") {
        Ok(hex) => String::from_utf8(transport::from_hex(&hex).expect("VIEW_INITIAL_URL_HEX must be hex"))
            .expect("VIEW_INITIAL_URL_HEX must spell UTF-8"),
        Err(_) => "https://example.com".to_string(),
    };
    let port: u16 = std::env::var("VIEW_TCP_PORT")
        .expect("VIEW_TCP_PORT env var required")
        .parse()
        .expect("VIEW_TCP_PORT must be a u16");
    // The Windows host's own gateway IP, computed by process_manager.rs's
    // spawn command (`ip route`'s default gateway) — not 127.0.0.1. WSL2's
    // NAT networking refuses a guest-to-127.0.0.1-on-host connection
    // outright (ECONNREFUSED), so the host is only reachable this way. See
    // manifold_view_protocol::transport::listen_remote's doc comment.
    let host_ip = std::env::var("VIEW_HOST_IP").expect("VIEW_HOST_IP env var required");

    // Single TCP connection back to manifold, used for both directions
    // via a cloned handle — mirrors process_manager.rs's spawn_webkit_tab
    // on the Windows side (see that file's comment for why: WSL2 has no
    // shared memory with the Windows host process, so this view-host
    // uses RemoteTransport, not the SharedFrameBuffer the other engines
    // use).
    // The secret process_manager.rs generated for this spawn; manifold drops any
    // connection that doesn't send it first.
    let token = transport::token_from_hex(&std::env::var("VIEW_TOKEN").expect("VIEW_TOKEN env var required"))
        .expect("VIEW_TOKEN must be 32 hex digits");
    let control = transport::connect_remote(&host_ip, port, &token).expect("connect to manifold");
    let stream = control.into_inner();
    let read_stream = stream.try_clone().expect("clone control stream");
    let mut to_host = MessageChannel::new(stream);
    let from_host = MessageChannel::new(read_stream);

    let rx = manifold_view_protocol::transport::spawn_host_reader(from_host);

    gtk::init().expect("gtk::init (is Xvfb running? DISPLAY set?)");

    // `WebView::new()` uses the default context, so this must be created
    // before it -- kept for the download handling below, which needs a
    // `WebContext` to attach `connect_download_started` to. Favicon
    // fetching (which used to also live on this context, via WebKitGTK's
    // own `FaviconDatabase` API) has been replaced by `manifold`'s own
    // uniform HTTP fetch of `{origin}/favicon.ico`, independent of any
    // engine -- see `favicon_fetch` in that crate. That per-engine
    // approach was inconsistent across engines anyway (this one needed an
    // extra "actively request the fetch" step CEF's own favicon plumbing
    // never worked at all under, a confirmed platform limitation).
    let web_context = webkit2gtk::WebContext::default();

    // Downloads: WebKitGTK has no default save location of its own (unlike
    // WebView2) -- `decide-destination` fires with the server's suggested
    // filename and MUST call `set_destination` or the download silently
    // never starts, same requirement as CEF's `on_before_download`/
    // `BeforeDownloadCallback::cont`. The folder and name come from
    // `manifold_view_protocol::downloads`, the rule CEF uses too: `~/Downloads`, the
    // suggested name's last component, numbered if taken (WebKit doesn't
    // overwrite, so a second download of one name would otherwise fail).
    // `reserved` holds names handed out and not yet finished, so concurrent
    // same-name downloads get distinct names before either file exists. The
    // destination is set as a properly escaped `file://` URI
    // (`glib::filename_to_uri`), so a `#` or space in a name works.
    //
    // `decide-destination` also reads `total_bytes` (the response's
    // `Content-Length`, via `WebKitURIResponse::content_length` -- 0 means
    // unknown, per WebKit's own convention) and stores the plain filesystem
    // path in `dest_for_events`. `received-data`, `failed` and `finished`
    // report that stored string, never `destination()` (the escaped URI),
    // so every message about one download carries the exact `path` its
    // `Started` did -- the host keys downloads by it. `received-data` fires
    // repeatedly as bytes arrive; `received_data_length()` is WebKit's own
    // running total. `finished` (not `estimated-progress` polling) is the
    // completion signal.
    //
    // A queue, not a single slot -- two downloads progressing within the
    // same ~16ms tick would otherwise silently clobber each other.
    let pending_download: Rc<RefCell<Vec<ViewToHost>>> = Rc::new(RefCell::new(Vec::new()));
    let reserved: Rc<RefCell<std::collections::HashSet<std::path::PathBuf>>> = Rc::default();
    if let Some(context) = &web_context {
        pages::register(context);
        let pending_for_started = pending_download.clone();
        context.connect_download_started(move |_context, download| {
            let dest_for_events: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
            let pending_for_destination = pending_for_started.clone();
            let dest_for_destination = dest_for_events.clone();
            let reserved_for_destination = reserved.clone();
            download.connect_decide_destination(move |download, suggested_filename| {
                let dir = manifold_view_protocol::downloads::downloads_dir();
                let _ = std::fs::create_dir_all(&dir);
                let dest = manifold_view_protocol::downloads::unique_path(
                    &dir,
                    &manifold_view_protocol::downloads::safe_name(suggested_filename),
                    |p| p.exists() || reserved_for_destination.borrow().contains(p),
                );
                let Ok(uri) = gtk::glib::filename_to_uri(&dest, None) else { return false };
                download.set_destination(&uri);
                reserved_for_destination.borrow_mut().insert(dest.clone());
                let path = dest.to_string_lossy().to_string();
                *dest_for_destination.borrow_mut() = Some(path.clone());
                let total_bytes = download.response().map(|r| r.content_length()).filter(|&n| n > 0);
                let url = download.request().and_then(|r| r.uri()).map(|u| u.to_string());
                pending_for_destination.borrow_mut().push(ViewToHost::DownloadStarted { path, total_bytes, url });
                true
            });

            let pending_for_progress = pending_for_started.clone();
            let dest_for_progress = dest_for_events.clone();
            download.connect_received_data(move |download, _data_length_delta| {
                if let Some(path) = dest_for_progress.borrow().clone() {
                    let bytes_received = download.received_data_length();
                    pending_for_progress.borrow_mut().push(ViewToHost::DownloadProgress { path, bytes_received });
                }
            });

            // `failed` fires before `finished` (which follows either outcome), so it marks the download for it.
            let failed = Rc::new(std::cell::Cell::new(false));
            let pending_for_failed = pending_for_started.clone();
            let failed_for_handler = failed.clone();
            let dest_for_failed = dest_for_events.clone();
            download.connect_failed(move |_download, _error| {
                failed_for_handler.set(true);
                if let Some(path) = dest_for_failed.borrow().clone() {
                    pending_for_failed.borrow_mut().push(ViewToHost::DownloadFailed { path });
                }
            });

            let pending_for_finished = pending_for_started.clone();
            let reserved_for_finished = reserved.clone();
            download.connect_finished(move |_download| {
                // `finished` follows either outcome, so the name is freed here.
                let dest = dest_for_events.borrow().clone();
                if let Some(path) = &dest {
                    reserved_for_finished.borrow_mut().remove(std::path::Path::new(path));
                }
                if failed.get() {
                    return;
                }
                if let Some(path) = dest {
                    pending_for_finished.borrow_mut().push(ViewToHost::DownloadCompleted { path });
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
    // return None (never actually create the popup webview; manifold
    // spawns a real child tab for `url` instead, once drained below).
    // A queue, not a single slot -- a page opening several tabs
    // synchronously would otherwise silently lose all but the last one.
    // Each entry is `(url, user_gesture)`; the host drops popups without a
    // gesture (WebKit already blocks script popups by default, as
    // `javascript-can-open-windows-automatically` is false).
    let pending_open_tab: Rc<RefCell<Vec<(String, bool)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let pending_open_tab = pending_open_tab.clone();
        // Fully-qualified: `connect_create` also exists on gtk::atk's
        // AtkWindowExt (an unrelated accessibility signal) via the same
        // glob-imported gtk::prelude::*, and plain method-call syntax
        // can't disambiguate which trait's version to use here.
        <WebView as webkit2gtk::WebViewExt>::connect_create(&webview, move |_webview, nav_action| {
            if let Some(uri) = nav_action.request().and_then(|r| r.uri()) {
                pending_open_tab.borrow_mut().push((uri.to_string(), nav_action.is_user_gesture()));
            }
            None
        });
    }

    // Audio-playing indicator: `is-playing-audio` is a real GObject notify
    // property, unlike CEF's only
    // equivalent (a full `AudioHandler` that takes over raw audio output
    // entirely) -- see `manifold_view_protocol::ViewToHost::AudioPlayingChanged`'s
    // doc comment.
    let pending_audio_playing: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
    {
        let pending_audio_playing = pending_audio_playing.clone();
        webview.connect_is_playing_audio_notify(move |webview| {
            *pending_audio_playing.borrow_mut() = Some(webview.is_playing_audio());
        });
    }

    // See `manifold_view_protocol::ViewToHost::JsonBody`/`RedirectableContent`'s own
    // doc comments -- filled in once per navigation, right after `loading`
    // transitions true -> false below, by the same `document.contentType`
    // script-eval check every engine that sends either message uses.
    let pending_json_body: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let pending_redirect_content_type: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    // Filled by the `QueryPageState` script callback, drained and sent each tick.
    let pending_page_state: Rc<RefCell<Option<manifold_view_protocol::PageState>>> = Rc::new(RefCell::new(None));

    // Minimal context menu: a right-click already dispatches a genuine
    // native `ButtonRelease` (input.rs, unchanged), which makes WebKitGTK
    // fire its own real `context-menu` signal -- suppressed here (`true` =
    // "handled, don't show your own") unconditionally, same
    // don't-risk-a-real-popup-window caution `HostToView::OpenDevTools`
    // already has to (a real GTK popup here would be override-redirect,
    // not a toplevel, so likely wouldn't hit that exact hang -- but this
    // project has no way to verify that live in this sandboxed session,
    // and suppressing it costs nothing since `input.rs`'s own
    // script-driven href query below is what `manifold`'s minimal
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
    // `manifold_view_protocol::ViewToHost::FindResult`'s own doc comment.
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

    let mut sink = manifold_view_protocol::FrameSink::inline();
    // M5: background tabs stay alive but stop sending frames; see
    // HostToView::SetVisible's doc comment.
    let mut visible = true;
    // When the last frame was sent: `FRAME_INTERVAL` paces `take`.
    let mut sent_at = Instant::now();
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
                HostToView::Navigate { url, .. } => {
                    webview.load_uri(&url);
                }
                HostToView::GoBack => {
                    if webview.can_go_back() {
                        webview.go_back();
                    }
                }
                HostToView::GoForward => {
                    if webview.can_go_forward() {
                        webview.go_forward();
                    }
                }
                HostToView::Reload => {
                    webview.reload();
                }
                HostToView::Resize { width: w, height: h, .. } => {
                    webview.set_size_request(w as i32, h as i32);
                    offscreen.resize(w as i32, h as i32);
                }
                HostToView::Input(event) => {
                    input::send_input(
                        &webview,
                        event,
                        keyboard_device.as_ref(),
                        pointer_device.as_ref(),
                        &pending_context_menu,
                    );
                }
                HostToView::SetZoom { factor } => {
                    webview.set_zoom_level(factor);
                }
                HostToView::SetVisible(v) => {
                    visible = v;
                    if v {
                        // A tab shown again gets a frame at once, changed or not.
                        sink.show();
                    }
                }
                HostToView::GetState => {
                    let url = webview.uri().map(|u| u.to_string()).unwrap_or_else(|| last_nav.0.clone());
                    let _ = to_host.send(&ViewToHost::StateSnapshot(TabState { url }));
                }
                // This engine has no cookie jar to seed or read; stays silent.
                HostToView::SetCookies(_) | HostToView::GetCookies => {}
                HostToView::QueryPageState => {
                    let pending = pending_page_state.clone();
                    webview.run_javascript(
                        manifold_view_protocol::PAGE_STATE_SCRIPT,
                        gtk::gio::Cancellable::NONE,
                        move |result| {
                            if let Ok(js_result) = result {
                                if let Some(value) = js_result.js_value() {
                                    if value.is_string() {
                                        if let Some(state) = manifold_view_protocol::PageState::parse(&value.to_str()) {
                                            *pending.borrow_mut() = Some(state);
                                        }
                                    }
                                }
                            }
                        },
                    );
                }
                HostToView::RestoreScroll { x, y } => {
                    webview.run_javascript(
                        &manifold_view_protocol::restore_scroll_script(x, y),
                        gtk::gio::Cancellable::NONE,
                        |_| {},
                    );
                }
                HostToView::FindStart { text, match_case } => {
                    if let Some(controller) = &find_controller {
                        let mut options = FindOptions::WRAP_AROUND;
                        if !match_case {
                            options |= FindOptions::CASE_INSENSITIVE;
                        }
                        controller.search(&text, options.bits(), 0);
                    }
                }
                HostToView::FindNext => {
                    if let Some(controller) = &find_controller {
                        controller.search_next();
                    }
                }
                HostToView::FindPrevious => {
                    if let Some(controller) = &find_controller {
                        controller.search_previous();
                    }
                }
                HostToView::FindStop => {
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
                // path waits on, which pegged manifold-engine-webkit at ~45%
                // CPU indefinitely (confirmed via a live 8-minute hang,
                // process never returned to servicing IPC) rather than
                // just failing visibly. No safe way to open a real window
                // in this specific hosting setup without adding a window
                // manager to the Xvfb session (a bigger change than this
                // warranted) -- silent no-op instead, same per-engine gap
                // as Servo's own missing DevTools API.
                HostToView::OpenDevTools => {}
                HostToView::SetMuted { muted } => {
                    webview.set_is_muted(muted);
                }
                HostToView::SetTheme { dark } => manifold_pages::set_dark(dark),
                HostToView::Shutdown => {
                    return;
                }
            }
        }

        // The history's current item, not `uri()`: `uri()` is the pending page from the moment a load starts, while
        // `can_go_back`/`can_go_forward` follow the history at commit. The item moves with them, so a poll never
        // pairs a new url with the old page's flags (a page-script Back read as a fork on that).
        let url =
            webview.back_forward_list().and_then(|l| l.current_item()).and_then(|i| i.uri()).or_else(|| webview.uri());
        let nav_now = (
            url.map(|u| u.to_string()).unwrap_or_else(|| last_nav.0.clone()),
            webview.title().map(|t| t.to_string()).unwrap_or_default(),
            webview.can_go_back(),
            webview.can_go_forward(),
            webview.is_loading(),
        );
        if nav_now != last_nav {
            let was_loading = last_nav.4;
            last_nav = nav_now.clone();
            let _ = to_host.send(&ViewToHost::NavigationStateChanged {
                url: nav_now.0,
                title: nav_now.1,
                can_go_back: nav_now.2,
                can_go_forward: nav_now.3,
                loading: nav_now.4,
                // Polled state carries no cause; `decide-policy`'s
                // `NavigationAction` (is_user_gesture, navigation_type)
                // would.
                user_started: None,
            });
            // Loading just finished -- check once whether the page that
            // loaded is a raw JSON response, or anything else non-HTML (see
            // `ViewToHost::JsonBody`/`RedirectableContent`'s own doc
            // comments for why this checks `document.contentType` rather
            // than guessing from the url). The `"json\n"`/`"type\n"`
            // prefixes disambiguate the two outcomes over one script-eval
            // round trip -- safe since they're prepended here before any
            // real page content, never something a real JSON body could
            // itself start with by coincidence.
            if was_loading && !nav_now.4 {
                let pending_json = pending_json_body.clone();
                let pending_redirect = pending_redirect_content_type.clone();
                webview.run_javascript(
                    manifold_view_protocol::PAGE_KIND_SCRIPT,
                    gtk::gio::Cancellable::NONE,
                    move |result| {
                        if let Ok(js_result) = result {
                            if let Some(value) = js_result.js_value() {
                                if value.is_string() {
                                    match manifold_view_protocol::PageKindReply::parse(&value.to_str()) {
                                        Some(manifold_view_protocol::PageKindReply::Json(body)) => {
                                            *pending_json.borrow_mut() = Some(body)
                                        }
                                        Some(manifold_view_protocol::PageKindReply::Other(t)) => {
                                            *pending_redirect.borrow_mut() = Some(t)
                                        }
                                        None => {}
                                    }
                                }
                            }
                        }
                    },
                );
            }
        }

        for (url, user_gesture) in pending_open_tab.borrow_mut().drain(..) {
            let _ = to_host.send(&ViewToHost::OpenTabRequested { url, user_gesture });
        }

        if let Some((count, active_index)) = pending_find_result.borrow_mut().take() {
            let _ = to_host.send(&ViewToHost::FindResult { count, active_index });
        }

        if let Some(body) = pending_json_body.borrow_mut().take() {
            let _ = to_host.send(&ViewToHost::JsonBody { body });
        }

        if let Some(state) = pending_page_state.borrow_mut().take() {
            let _ = to_host.send(&ViewToHost::PageState(state));
        }

        if let Some(content_type) = pending_redirect_content_type.borrow_mut().take() {
            let _ = to_host.send(&ViewToHost::RedirectableContent { content_type });
        }

        if let Some(playing) = pending_audio_playing.borrow_mut().take() {
            let _ = to_host.send(&ViewToHost::AudioPlayingChanged { playing });
        }

        if let Some((x, y, href)) = pending_context_menu.borrow_mut().take() {
            let _ = to_host.send(&ViewToHost::ContextMenuLink { x, y, href });
        }

        let download_msgs = std::mem::take(&mut *pending_download.borrow_mut());
        for msg in manifold_view_protocol::downloads::coalesce_progress(download_msgs) {
            let _ = to_host.send(&msg);
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
            // debug logging: no frame was ever sent). Sending
            // the pixbuf's own real dimensions sidesteps needing to
            // understand or fix GTK's offscreen-resize negotiation at all
            // — manifold's reader already takes width/height from each
            // `FrameReady` message directly (no separate shared-memory
            // sizing contract for this RemoteTransport-based view-host),
            // so there is no requirement that it match what was requested.
            // Unchanged frames are not sent (`FrameSink`).
            if let Some(pixbuf) = offscreen.pixbuf() {
                let (pw, ph) = (pixbuf.width() as u32, pixbuf.height() as u32);
                // A 4-channel pixbuf is offered as is, its rowstride the pitch (GdkPixbuf leaves the last row
                // unpadded, which `offer` accepts); anything else is packed to RGBA first.
                // SAFETY: the pixbuf is not modified while this slice is alive.
                if pixbuf.n_channels() == 4 {
                    sink.offer(unsafe { pixbuf.pixels() }, pixbuf.rowstride() as usize, pw, ph);
                } else {
                    sink.offer(&tightly_packed_rgba(&pixbuf), pw as usize * 4, pw, ph);
                }
                if sent_at.elapsed() >= FRAME_INTERVAL {
                    if let Some(msg) = sink.take() {
                        let _ = to_host.send(&msg);
                        sent_at = Instant::now();
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_millis(16));
    }
}

/// `GdkPixbuf` rows can be padded to a stride wider than `width * 4` for
/// alignment; our IPC contract assumes tightly-packed rows
/// (`width * height * 4` bytes, no padding — see `manifold_view_protocol::frame_buffer`'s doc comment
/// and `manifold`'s texture upload), so copy row-by-row rather than
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
