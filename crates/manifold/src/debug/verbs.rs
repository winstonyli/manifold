//! The `VERBS` table: one row per command, with the parsers and `UserEvent` jobs its rows use.

use super::reader::debug_key_event;
use super::*;

fn job<R>(run: impl FnOnce(&mut App, &ActiveEventLoop) -> R + Send + 'static) -> Option<Parsed> {
    Some(Parsed::Job(Box::new(move |app, event_loop| {
        run(app, event_loop);
    })))
}

/// A job for a non-empty argument.
fn with_text<R>(rest: &str, run: impl FnOnce(&mut App, String) -> R + Send + 'static) -> Option<Parsed> {
    (!rest.is_empty()).then(|| rest.to_string()).and_then(|text| job(move |app, _| run(app, text)))
}

/// A job for the rest of the line, possibly empty.
fn with_str<R>(rest: &str, run: impl FnOnce(&mut App, String) -> R + Send + 'static) -> Option<Parsed> {
    let text = rest.to_string();
    job(move |app, _| run(app, text))
}

/// An egui key press for the next frame's input.
fn key_job(key: egui::Key, modifiers: egui::Modifiers) -> Option<Parsed> {
    job(move |app, _| app.debug.debug_input_events.push(debug_key_event(key, modifiers)))
}

/// A direct-call job on a node id given as the whole argument; an argument that is no node id is a usage error.
fn with_id<R>(rest: &str, run: impl FnOnce(&mut App, NodeId) -> R + Send + 'static) -> Option<Parsed> {
    let id = rest.parse().ok()?;
    job(move |app, _| run(app, id))
}

/// The next word as a node id.
fn id_word<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Option<NodeId> {
    parts.next()?.parse().ok()
}

fn on_off(word: &str) -> Option<bool> {
    match word {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

fn coords(parts: &mut SplitWhitespace) -> Option<(f64, f64)> {
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// The cursor at a window point, as a real pointer move.
fn cursor_to(app: &mut App, event_loop: &ActiveEventLoop, x: f64, y: f64) {
    app.dispatch_window_event(
        event_loop,
        WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
    );
}

fn button_at(
    app: &mut App,
    event_loop: &ActiveEventLoop,
    (x, y): (f64, f64),
    button: MouseButton,
    state: ElementState,
) {
    cursor_to(app, event_loop, x, y);
    app.dispatch_window_event(event_loop, WindowEvent::MouseInput { device_id: DeviceId::dummy(), state, button });
}

/// `MOUSEDOWN`/`MOUSEUP`: `x y [button]`.
fn mouse_button(rest: &str, state: ElementState) -> Option<Parsed> {
    let mut parts = rest.split_whitespace();
    let at = coords(&mut parts)?;
    let button = parts.next().map(parse_mouse_button).unwrap_or(MouseButton::Left);
    job(move |app, el| button_at(app, el, at, button, state))
}

/// `CLICK x y [button] [ctrl]`.
pub(super) fn parse_click(rest: &str) -> Option<((f64, f64), MouseButton, bool)> {
    let mut parts = rest.split_whitespace();
    let at = coords(&mut parts)?;
    let (mut button, mut ctrl) = (MouseButton::Left, false);
    for word in parts {
        if word.eq_ignore_ascii_case("ctrl") {
            ctrl = true;
        } else {
            button = parse_mouse_button(word);
        }
    }
    Some((at, button, ctrl))
}

/// `MOVENODE id x y`: finite coordinates and nothing after them.
pub(super) fn parse_move_node(rest: &str) -> Option<(NodeId, f32, f32)> {
    let mut parts = rest.split_whitespace();
    let id = id_word(&mut parts)?;
    let mut coord = || parts.next()?.parse::<f32>().ok().filter(|v| v.is_finite());
    let (x, y) = (coord()?, coord()?);
    parts.next().is_none().then_some((id, x, y))
}

/// `GRAPHZOOM pct`.
pub(super) fn parse_zoom_pct(rest: &str) -> Option<f32> {
    rest.parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// `RENAMEGROUP [id name]`: `Some(None)` alone (F2), `Some(Some((id, name)))` with both, `None` for an id with no name.
pub(super) fn parse_rename(rest: &str) -> Option<Option<(NodeId, String)>> {
    if rest.is_empty() {
        return Some(None);
    }
    let (id, name) = split_id(rest)?;
    (!name.is_empty()).then(|| Some((id, name.to_string())))
}

/// `USEJAR <id> <jar|none>`.
pub(super) fn parse_use_jar(rest: &str) -> Option<(NodeId, Option<NodeId>)> {
    let mut parts = rest.split_whitespace();
    let from = id_word(&mut parts)?;
    let jar = parts.next()?;
    let jar = if jar.eq_ignore_ascii_case("none") { None } else { Some(jar.parse().ok()?) };
    Some((from, jar))
}

/// `WAITTABS`/`WAITIDLE`'s optional timeout: a minute by default.
fn wait_timeout(rest: &str) -> Option<std::time::Duration> {
    if rest.is_empty() {
        Some(std::time::Duration::from_secs(60))
    } else {
        parse_secs(rest)
    }
}

/// `GOBACK`/`GOFORWARD`: the focused tab's trail, one step.
fn go(dir: Dir) -> Option<Parsed> {
    job(move |app, _| {
        if let Some(id) = app.focused {
            app.navigate_history(id, dir);
        }
    })
}

/// An action on the focused tab's own view, if it is a tab with one.
fn on_focused_tab(app: &mut App, run: impl FnOnce(&mut crate::node::TabPayload)) {
    if let Some(id) = app.focused {
        if let Some(node) = app.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
            run(node);
        }
    }
}

/// `QUIT`, and the exit the reader sends at end of input.
pub(super) fn quit(app: &mut App, event_loop: &ActiveEventLoop) {
    // Exiting right now could drop a pending Enter/Escape/arrow command, or a just-requested Screenshot that hasn't
    // been captured yet (`pending_screenshot` is only consumed inside a `RedrawRequested`: a `Screenshot`
    // immediately followed by `Quit`, with no frame in between, would otherwise silently lose the screenshot the
    // same way an unconsumed Enter could). `quit_after_redraws` holds how many frames to wait.
    if app.debug_input_pending() {
        app.debug.quit_after_redraws = Some(3);
        if let Some(window) = &app.window {
            window.request_redraw();
        }
    } else {
        event_loop.exit();
    }
}

/// The longest SLEEP/WAITTABS a script may ask for: a day.
pub(crate) const MAX_WAIT_SECS: f64 = 86_400.0;

/// SLEEP/WAITTABS seconds: finite and within `0..=MAX_WAIT_SECS`.
/// `Duration::from_secs_f64` panics on infinity or overflow, which killed
/// the stdin reader and left headless runs hanging.
pub(crate) fn parse_secs(s: &str) -> Option<std::time::Duration> {
    let secs: f64 = s.parse().ok()?;
    (secs.is_finite() && (0.0..=MAX_WAIT_SECS).contains(&secs)).then(|| std::time::Duration::from_secs_f64(secs))
}

fn parse_mouse_button(name: &str) -> MouseButton {
    match name.to_ascii_lowercase().as_str() {
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::Left,
    }
}

/// Maps a debug command's key name (`Enter`, `ShiftLeft`, `KeyA`, ...: the DOM
/// code, `KeyCode::dom_code`) to `manifold_view_protocol::KeyCode`. A bare letter or
/// digit (`"a"`, `"5"`) is accepted too, for convenience.
fn parse_key_code(name: &str) -> Option<KeyCode> {
    if let Some(key) = KeyCode::ALL.iter().copied().find(|k| k.dom_code() == name) {
        return Some(key);
    }
    let mut chars = name.chars();
    let ch = chars.next().filter(char::is_ascii_alphanumeric)?;
    if chars.next().is_some() {
        return None;
    }
    let code = if ch.is_ascii_digit() { format!("Digit{ch}") } else { format!("Key{}", ch.to_ascii_uppercase()) };
    KeyCode::ALL.iter().copied().find(|k| k.dom_code() == code)
}

/// `ADDFILE <path>` or `ADDFILE <path> <x> <y>` -- `rest` is non-empty. The
/// path may itself contain spaces, so the trailing `x y` is only peeled off
/// when the last two whitespace-separated words both parse as `f32`;
/// otherwise the whole of `rest` is the path.
pub(super) fn parse_add_file(rest: &str) -> (String, Option<(f32, f32)>) {
    let words: Vec<&str> = rest.split_whitespace().collect();
    if words.len() >= 3 {
        if let (Ok(x), Ok(y)) = (words[words.len() - 2].parse::<f32>(), words[words.len() - 1].parse::<f32>()) {
            return (words[..words.len() - 2].join(" "), Some((x, y)));
        }
    }
    (rest.to_string(), None)
}

/// `<id> <text...>` -- `rest` is trimmed and non-empty: the first word as a node id, and the rest of the line after
/// it, trimmed (empty when there is none). The text may itself contain spaces.
pub(super) fn split_id(rest: &str) -> Option<(NodeId, &str)> {
    let (id, text) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    Some((id.parse().ok()?, text.trim()))
}

/// The command table: one row per verb, in `HELP`'s order.
#[rustfmt::skip]
pub(super) const VERBS: &[Verb] = &[
    pointer_row("MOUSEMOVE x y", "Move the mouse to (x, y)", |rest| {
        let (x, y) = coords(&mut rest.split_whitespace())?;
        job(move |app, el| cursor_to(app, el, x, y))
    }),
    pointer_row("MOUSEDOWN x y [button]", "Press a mouse button at (x, y) -- button defaults to left", |rest| mouse_button(rest, ElementState::Pressed)),
    pointer_row("MOUSEUP x y [button]", "Release a mouse button at (x, y)", |rest| mouse_button(rest, ElementState::Released)),
    pointer_row("CLICK x y [button] [ctrl]", "Full click (move+press+release) at (x, y); ctrl holds Ctrl down", |rest| {
        let (at, button, ctrl) = parse_click(rest)?;
        job(move |app, el| app.debug_click(el, at.0, at.1, button, 1, ctrl))
    }),
    pointer_row("DOUBLECLICK x y", "Left double-click at (x, y), both clicks in one frame", |rest| {
        // Two `CLICK`s only count as one when their frames fall within egui's 0.3 s double-click delay, which a slow
        // headless frame can break.
        let (x, y) = coords(&mut rest.split_whitespace())?;
        job(move |app, el| app.debug_click(el, x, y, MouseButton::Left, 2, false))
    }),
    pointer_row("TRIPLECLICK x y", "Left triple-click at (x, y), the three clicks in one frame", |rest| {
        let (x, y) = coords(&mut rest.split_whitespace())?;
        job(move |app, el| app.debug_click(el, x, y, MouseButton::Left, 3, false))
    }),
    pointer_row("SCROLL x y dx dy", "Scroll at (x, y) by (dx, dy)", |rest| {
        let mut parts = rest.split_whitespace();
        let (x, y) = coords(&mut parts)?;
        let (delta_x, delta_y) = coords(&mut parts)?;
        job(move |app, el| {
            cursor_to(app, el, x, y);
            app.dispatch_window_event(
                el,
                WindowEvent::MouseWheel {
                    device_id: DeviceId::dummy(),
                    delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(delta_x, delta_y)),
                    phase: TouchPhase::Moved,
                },
            );
        })
    }),
    // A click on the address bar, then select-all and typing: focus is requested now, and the text is typed as egui
    // `Text` input in the first frame the bar has it (see `debug_address_bar_typing`).
    row("ADDRESSBAR text", "Set the address bar's text and focus (Tab view only)", |rest| with_str(rest, |app, text| {
        if app.showing_tab_view() {
            app.pending_focus = Some(FocusTarget::OmniBar);
            app.debug.debug_address_bar_typing = Some(text);
            if let Some(window) = &app.window {
                window.request_redraw();
            }
        } else {
            eprintln!("debug: ADDRESSBAR ignored: not in Tab view");
        }
    })),
    // Real key presses for the next frame's egui input, so they go through the same checks a keyboard does, in any
    // view.
    row("ENTER", "Accept the address bar / confirm a dropdown row", |_| key_job(egui::Key::Enter, egui::Modifiers::NONE)),
    row("ESCAPE", "Dismiss a suggestion dropdown, or leave Tab view for Graph view", |_| key_job(egui::Key::Escape, egui::Modifiers::NONE)),
    row("BACKSPACE", "A Backspace key press in the focused egui text field", |_| key_job(egui::Key::Backspace, egui::Modifiers::NONE)),
    // The focused egui field takes it, as from a keyboard (`TYPE` goes to the page instead). Nothing focused: nothing typed.
    row("EGUITEXT text", "Type into the focused egui text field (a jar or group name; TYPE goes to the page)", |rest| {
        with_text(rest, |app, text| app.debug.debug_input_events.push(egui::Event::Text(text)))
    }),
    row("ARROWRIGHT", "A Right arrow key press (JSON level: open the selected outline row)", |_| key_job(egui::Key::ArrowRight, egui::Modifiers::NONE)),
    row("ARROWLEFT", "A Left arrow key press (JSON level: close it, or select its parent)", |_| key_job(egui::Key::ArrowLeft, egui::Modifiers::NONE)),
    row("DIGITKEY n", "A plain digit key press (0-9): in the Graph view, jumps to the card with that number", |rest| {
        let digit: usize = rest.parse().ok().filter(|d| *d <= 9)?;
        let key = [
            egui::Key::Num0, egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4,
            egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9,
        ][digit];
        key_job(key, egui::Modifiers::NONE)
    }),
    row("SPACEKEY", "A Space key press (a focused tab card toggles in the selection)", |_| key_job(egui::Key::Space, egui::Modifiers::NONE)),
    row("ARROWDOWN", "Move the address-bar suggestion highlight down", |_| key_job(egui::Key::ArrowDown, egui::Modifiers::NONE)),
    row("ARROWUP", "Move the address-bar suggestion highlight up", |_| key_job(egui::Key::ArrowUp, egui::Modifiers::NONE)),
    row("SHIFTF10", "Shift+F10 -- open the focused graph card's menu", |_| key_job(egui::Key::F10, egui::Modifiers::SHIFT)),
    row("UNDOKEY [shift]", "Ctrl+Z (or Ctrl+Shift+Z) -- undo (redo) the last drop or fold in the Graph view", |rest| {
        key_job(egui::Key::Z, egui::Modifiers { command: true, ctrl: true, shift: rest == "shift", ..Default::default() })
    }),
    row("TABKEY [shift]", "Tab (or Shift+Tab) -- move egui keyboard focus to the next (previous) widget", |rest| {
        key_job(egui::Key::Tab, if rest == "shift" { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE })
    }),
    row("GRAPHLEFT", "Graph view: move focus left", |_| job(|app, _| app.move_graph_focus(egui::vec2(-1.0, 0.0)))),
    row("GRAPHRIGHT", "Graph view: move focus right", |_| job(|app, _| app.move_graph_focus(egui::vec2(1.0, 0.0)))),
    row("GRAPHDOWN", "Graph view: move focus down", |_| job(|app, _| app.move_graph_focus(egui::vec2(0.0, 1.0)))),
    row("GRAPHOPEN", "Open whichever Graph-view node is currently focused", |_| job(|app, _| app.open_focused_graph_node())),
    row("TYPE text", "Send each character to the focused tab's own page (not the address bar)", |rest| with_str(rest, |app, text| {
        on_focused_tab(app, |node| {
            for ch in text.chars() {
                node.tab.send(&HostToView::Input(InputEvent::Char { codepoint: ch as u32 }));
            }
        });
    })),
    row("KEYPAGE key", "Send a KeyDown+KeyUp of `key` to the focused tab's own page", |rest| {
        let key = parse_key_code(rest.split_whitespace().next()?)?;
        job(move |app, _| {
            let modifiers = app.page_input.key_modifiers;
            on_focused_tab(app, |node| {
                node.tab.send(&HostToView::Input(InputEvent::KeyDown { key, modifiers }));
                node.tab.send(&HostToView::Input(InputEvent::KeyUp { key, modifiers }));
            });
        })
    }),
    row("INSTALLVIEW kind", "Start downloading and installing the view's host files (and CEF's runtime)", |rest| {
        let kind = ViewKind::from_name(rest)?;
        job(move |app, _| app.start_view_install(kind))
    }),
    row("VIEWSTATE kind", "Print the view's state (Installed, Installing n%, Failed: ..) to stderr", |rest| {
        let kind = ViewKind::from_name(rest)?;
        job(move |app, _| {
            eprintln!(
                "view-state {}: {} [{}]",
                kind.desc().host.id,
                crate::view_info::view_status_label(&app.view_status(kind)).0,
                crate::view_info::on_disk_text(kind, crate::process_manager::view_on_disk(kind))
            )
        })
    }),
    row("REOPENTAB", "Ctrl+Shift+T -- reopen the newest Recently Closed entry", |_| job(|app, _| app.reopen_closed_shortcut())),
    row("REOPENNODE id", "Reopen closed node `id`, as the Recently Closed popup does, whichever replica closed it", |rest| {
        with_id(rest, |app, id| app.reopen_node(id))
    }),
    row("ZOOMIN", "Ctrl+=/Ctrl++ -- zoom in", |_| job(|app, _| app.zoom_in_shortcut())),
    row("ZOOMOUT", "Ctrl+- -- zoom out", |_| job(|app, _| app.zoom_out_shortcut())),
    row("ZOOMRESET", "Ctrl+0 -- reset zoom", |_| job(|app, _| app.zoom_reset_shortcut())),
    row("GRAPHZOOM pct", "Graph view: set the camera zoom to pct percent (ZOOMIN/ZOOMOUT zoom the focused page there)", |rest| {
        let pct = parse_zoom_pct(rest)?;
        job(move |app, _| app.graph.pending_zoom = Some(pct / 100.0))
    }),
    row("MARK label", "Write `label` into the timing trace (`MANIFOLD_TRACE`), to tell a script's phases apart", |rest| {
        with_text(rest, |_, label| tracing::info!(target: "mark", label = label.as_str()))
    }),
    row("OPENDEVTOOLS", "F12 -- open devtools", |_| job(|app, _| app.open_devtools_shortcut())),
    row("NEWTAB", "Ctrl+T -- new tab", |_| job(|app, _| app.new_tab_shortcut())),
    row("CLOSETAB", "Ctrl+W -- close the focused tab", |_| job(|app, _| app.close_focused_tab_shortcut())),
    row("FOCUSADDRESS", "Ctrl+L -- focus the address bar", |_| job(|app, _| app.focus_address_bar_shortcut())),
    row("NEXTTAB", "Ctrl+Tab -- cycle to the next tab", |_| job(|app, _| app.cycle_focused_tab(true))),
    row("PREVTAB", "Ctrl+Shift+Tab -- cycle to the previous tab", |_| job(|app, _| app.cycle_focused_tab(false))),
    row("OPENFINDBAR", "Ctrl+F -- open find-in-page", |_| job(|app, _| app.open_find_bar_shortcut())),
    row("FINDTEXT text", "Set the find bar's search text and search", |rest| with_str(rest, |app, text| {
        app.find.text = text;
        app.send_find_start();
    })),
    row("CLOSEFINDBAR", "Close the find bar", |_| job(|app, _| app.close_find_bar())),
    row("SCREENSHOT path", "Save the next frame as a PNG at `path`; later commands wait for it (30 s max)", |rest| {
        (!rest.is_empty()).then(|| Parsed::Reader(ReaderOp::Screenshot(rest.to_string())))
    }),
    row("RESIZE w h", "Resize the window to (w, h)", |rest| {
        let mut parts = rest.split_whitespace();
        let (width, height): (u32, u32) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
        job(move |app, _| {
            if let Some(window) = &app.window {
                let _ = window.request_inner_size(PhysicalSize::new(width, height));
            }
        })
    }),
    row("SURFACE outdated|lost", "Make the next frame see an outdated or lost window surface (the recovery paths)", |rest| {
        let fault = crate::gpu::SurfaceFault::parse(rest.trim())?;
        job(move |app, _| {
            if let Some(gpu) = app.gpu.as_mut() {
                gpu.force_surface_fault(fault);
            }
            if let Some(window) = &app.window {
                window.request_redraw();
            }
        })
    }),
    row("QUIT", "Exit the app", |_| job(quit)),
    row("QUICKSWITCHOPEN", "Ctrl+K -- open the omnibar popup", |_| job(|app, _| app.open_omni_popup())),
    row("QUICKSWITCHTEXT text", "Set the omnibar popup's search text", |rest| with_str(rest, |app, text| {
        if app.menus.omni_popup.open.is_some() {
            app.omni_set_text(Host::Popup, &text);
        }
    })),
    row("QUICKSWITCHENTER", "Omnibar popup: confirm the highlighted row", |_| job(|app, _| app.omni_confirm(Host::Popup))),
    row("QUICKSWITCHESCAPE", "Close the omnibar popup", |_| job(|app, _| app.omni_close(Host::Popup))),
    row("SESSIONNAMETEXT text", "Set the \"Save Session As...\" name field", |rest| with_str(rest, |app, text| app.session_name_draft = text)),
    row("SAVESESSIONAS name", "Save the running session as `name` (sessions/<name>.mnf)", |rest| with_text(rest, |app, name| app.save_named_session(&name))),
    row("OPENSESSION name", "Open the named save `name`, replacing the running session", |rest| {
        with_text(rest, |app, name| {
            app.load_named_session(&name);
            app.close_settings_page();
        })
    }),
    row("GRAPHFILTERTEXT text", "Set the Graph omnibar's text (dims the cards it misses)", |rest| with_str(rest, |app, text| app.set_graph_filter(&text))),
    row("HISTORYFILTERTEXT text", "Set the History popup's own filter box", |rest| with_str(rest, |app, text| app.list_popups.history_filter = text)),
    row("BOOKMARKSFILTERTEXT text", "Set the Bookmarks popup's own filter box", |rest| with_str(rest, |app, text| app.list_popups.bookmarks_filter = text)),
    row("TOGGLEVIEW", "Ctrl+G -- toggle Tab / Graph view", |_| job(|app, _| app.toggle_view_mode_shortcut())),
    row("TOGGLESETTINGSPAGE", "Settings gear -- enter/leave the Settings page", |_| job(|app, _| app.toggle_settings_page())),
    row("CLEARTOASTS", "Drop every toast: shown, queued and held (a toast expires on the clock, so a shot can't wait one out)", |_| job(|app, _| app.toasts.clear())),
    row("HOLDTOAST text", "Stop the clock for the toast holding `text`, showing or still to come, until CLEARTOASTS (a shot of a toast can't race its expiry)", |rest| {
        with_text(rest, |app, text| app.toasts.hold = Some(text))
    }),
    // The file commands take an explicit path instead of a real file dialog: headless mode has no human to answer
    // one, so they call the same underlying save/load directly.
    row("EXPORTSESSION path", "Save the current session to `path`", |rest| {
        with_text(rest, |app, path| app.export_session_to(std::path::Path::new(&path)))
    }),
    row("EXPORTSTATE path", "Write view, address bar and dropdown state to `path` as JSON (see debug/export.rs)", |rest| {
        with_text(rest, |app, path| app.write_export_state(std::path::Path::new(&path)))
    }),
    row("TEXTOF id path", "Write the page text stored for tab `id` to `path`, or `none` (see tabs/page_text.rs)", |rest| {
        let (id, path) = rest.split_once(' ')?;
        let (id, path) = (id.parse().ok()?, path.to_owned());
        job(move |app, _| app.write_text_of(id, std::path::Path::new(&path)))
    }),
    row("IMPORTSESSION path", "Load a session from `path`", |rest| {
        with_text(rest, |app, path| match crate::profile::file::load(std::path::Path::new(&path), "session") {
            Ok(json) => app.confirm = Some(Confirm::Import(Box::new(json))),
            Err(err) => app.queue_toast(err),
        })
    }),
    row("EXPORTBOOKMARKS path", "Save bookmarks to `path`", |rest| {
        with_text(rest, |app, path| bookmarks::save_to(&app.bookmarks, std::path::Path::new(&path)))
    }),
    row("IMPORTBOOKMARKS path", "Load bookmarks from `path`", |rest| {
        with_text(rest, |app, path| match bookmarks::load_from(std::path::Path::new(&path)) {
            Ok(imported) => {
                app.bookmarks = imported;
                bookmarks::save(&app.bookmarks);
            }
            Err(err) => app.queue_toast(err),
        })
    }),
    row("CLOSEALLLEVEL", "Ctrl+Shift+W -- close every tab at the current level", |_| job(|app, _| app.close_all_at_current_level())),
    row("CONFIRMIMPORT", "Click the \"Import Session...\" confirm popup's Import button", |_| {
        job(|app, _| {
            if let Some(Confirm::Import(json)) = app.confirm.take_if(|c| matches!(c, Confirm::Import(_))) {
                app.replace_running_session(&json, crate::store::json::Trust::Foreign);
            }
        })
    }),
    row("CANCELIMPORT", "Click the confirm popup's Cancel button", |_| {
        job(|app, _| {
            app.confirm.take_if(|c| matches!(c, Confirm::Import(_)));
        })
    }),
    // `id name`: node `id`'s rename field committed with `name` (`NodeAction::CommitRename`: the clean name; a group
    // ignores an empty one, any other node clears its name with it), by id since a headless test can't rely on focus.
    row("RENAMEGROUP [id name]", "F2 -- rename the current group; with `id name`, commit `name` as node `id`'s new name", |rest| {
        match parse_rename(rest)? {
            None => job(|app, _| app.rename_current_group_shortcut()),
            Some((id, name)) => job(move |app, _| {
                if app.nodes.contains_key(&id) {
                    app.commit_rename(id, &name);
                    app.save_session();
                } else {
                    eprintln!("RENAMEGROUP: {id} isn't a node; ignored");
                }
            }),
        }
    }),
    row("BOOKMARKFOLDERTEXT text", "Set the bookmarks popup's own folder-rename draft", |rest| {
        with_str(rest, |app, text| {
            if let Some((_, draft)) = &mut app.list_popups.renaming_bookmark_folder {
                *draft = text;
            }
        })
    }),
    row("DISMISSPEEK", "Escape-dismisses-peek equivalent in Graph view", |_| job(|app, _| app.dismiss_hover_peek())),
    row("EXPORTGROUP path", "Export whichever group's context menu is currently open", |rest| {
        with_text(rest, |app, path| {
            if let Some((group_id, _)) = app.menus.node_menu.open.filter(|(id, _)| app.is_container(*id)) {
                if let Some(json) = app.export_group_json(group_id) {
                    write_json_export(&json, std::path::Path::new(&path), "group");
                }
            }
        })
    }),
    row("EXPORTJSON id path", "Write inline Json card `id`'s document to `path`", |rest| {
        let (id, path) = rest.split_once(' ')?;
        let (id, path) = (id.parse::<NodeId>().ok()?, path.trim().to_owned());
        job(move |app, _| {
            if let Some(json) = app.export_inline_json(id) {
                write_json_export(&json, std::path::Path::new(&path), "json");
            }
        })
    }),
    row("IMPORTGROUP path", "Import a group from `path`", |rest| {
        with_text(rest, |app, path| match crate::profile::file::load::<store::json::JsonDoc>(std::path::Path::new(&path), "group") {
            Ok(json) => app.import_group_json(&json),
            Err(err) => app.queue_toast(err),
        })
    }),
    row("DUPLICATETAB", "Ctrl+D -- duplicate the focused tab", |_| job(|app, _| app.duplicate_focused_tab_shortcut())),
    row("DUPLICATENODE id", "Duplicate on card `id`: the whole selection when `id` is in it (any kind)", |rest| {
        with_id(rest, |app, id| {
            app.duplicate_selection(id);
            app.save_session();
        })
    }),
    row("PRINTTOPDF path", "Print the focused tab to a PDF at `path`", |rest| with_text(rest, |app, path| app.start_print_to_pdf(path))),
    row("TOGGLEPIN", "Ctrl+P -- pin/unpin the selected cards, or the focused one", |_| job(|app, _| app.pin_shortcut())),
    row("SELECTCARDS [id...]", "Select exactly these cards (none: clear the selection)", |rest| {
        let ids: Vec<crate::card_id::CardId> = rest.split_whitespace().map(str::parse).collect::<Result<_, _>>().ok()?;
        job(move |app, _| app.interaction.selected = ids.into_iter().collect())
    }),
    row("LABEL name", "Make a label (or find the one with that name) and put it on the selected cards", |rest| {
        with_text(rest, |app, name| {
            let ctx = app.egui_ctx.clone();
            app.apply_graph_action(&ctx, GraphAction::NewLabel(name));
        })
    }),
    row("LABELSELECTED name on|off", "Put the named label on the selected cards, or take it off", |rest| {
        let (name, on) = rest.rsplit_once(' ')?;
        let on = match on {
            "on" => true,
            "off" => false,
            _ => return None,
        };
        with_text(name.trim(), move |app, name| {
            if let Some(label) = app.label_named(&name, None) {
                let ctx = app.egui_ctx.clone();
                app.apply_graph_action(&ctx, GraphAction::ToggleLabel { label, on });
            }
        })
    }),
    row("LABELRENAME old new", "Rename the label `old` (one word) to `new`", |rest| {
        let (old, new) = rest.split_once(' ')?;
        let (old, new) = (old.to_owned(), new.trim().to_owned());
        job(move |app, _| {
            if let Some(label) = app.label_named(&old, None) {
                let ctx = app.egui_ctx.clone();
                app.apply_graph_action(&ctx, GraphAction::RenameLabel { label, name: new });
            }
        })
    }),
    row("LABELDELETE name", "Delete the named label", |rest| {
        with_text(rest, |app, name| {
            if let Some(label) = app.label_named(&name, None) {
                let ctx = app.egui_ctx.clone();
                app.apply_graph_action(&ctx, GraphAction::DeleteLabel(label));
            }
        })
    }),
    row("RELOAD", "Ctrl+R/F5 -- reload the focused tab", |_| job(|app, _| app.reload_focused_tab_shortcut())),
    row("GOBACK", "Back button on the focused tab", |_| go(Dir::Back)),
    row("GOFORWARD", "Forward button on the focused tab", |_| go(Dir::Forward)),
    row("NEWGROUP", "Ctrl+Shift+G -- new group", |_| job(|app, _| app.new_group_shortcut())),
    row("NEWJSON [x y]", "A new inline JSON card (its document lives in the synced document); x y is a window point", |rest| {
        let at = match rest.split_whitespace().collect::<Vec<_>>()[..] {
            [] => None,
            [x, y] => Some(egui::pos2(x.parse().ok()?, y.parse().ok()?)),
            _ => return None,
        };
        job(move |app, _| app.new_json_shortcut(at))
    }),
    row("DETACHJSON id", "Move a file JSON card's document into the synced document", |rest| {
        let (id, _) = split_id(rest)?;
        job(move |app, _| {
            if app.detach_json(id).is_ok() {
                app.save_session();
            }
        })
    }),
    row("JSONEDIT op pointer [args]", "Edit the open inline JSON level: set <ptr> <json>, add <ptr> <key> <json>, insert <ptr> <index> <json>, remove <ptr>, rename <ptr> <to>, move <ptr> <to>; `root` is the root pointer", |rest| {
        match crate::json::edit::parse_json_edit(rest) {
            Ok(op) => job(move |app, _| {
                let Some(id) = app.json_level.as_ref().map(|level| level.node_id) else {
                    return eprintln!("JSONEDIT: no JSON level is open");
                };
                if app.edit_json(id, op).is_ok() {
                    app.save_session();
                }
            }),
            Err(message) => {
                eprintln!("JSONEDIT: {message}");
                None
            }
        }
    }),
    row("NEWNOTE", "A new note near the graph's centre, in the level in view", |_| job(|app, _| app.new_note_shortcut())),
    // With a point, placed like a real drop at that window point; no `save_session` there either -- same reasoning as
    // `WindowEvent::DroppedFile`, so a script sending several `ADDFILE`s with a point still gets one save per batch,
    // not one per file. With none, the file is jittered around world center at the current level, as the picker does.
    row("ADDFILE path [x y]", "Adds a file as the node its content makes (image, PDF, JSON, note, tab, or a download card); x y is a window point, like a drop there", |rest| {
        if rest.is_empty() {
            return None;
        }
        match parse_add_file(rest) {
            (path, None) => job(move |app, _| {
                if app.add_file_at(path.into(), None).is_ok() {
                    app.save_session();
                }
            }),
            (path, Some((x, y))) => job(move |app, _| app.drop_file(path.into(), egui::pos2(x, y))),
        }
    }),
    // No bytes, unknown total, nothing fetched or written; a no-op with no focused tab.
    row("ADDDOWNLOAD path", "Add a download node for `path`, as if the focused tab started it", |rest| {
        with_text(rest, |app, path| {
            if let Some(tab_id) = app.focused {
                app.insert_download_node(tab_id, path, None, None);
            }
        })
    }),
    // Headless tab spawns are async and can take 30 s or more (the first-launch demo's included), so WAITTABS
    // replaces guessing a `sleep`.
    row("WAITTABS [secs]", "Hold later commands until no tab spawn is in flight (default 60 s max)", |rest| {
        wait_timeout(rest).map(|timeout| Parsed::Reader(ReaderOp::WaitTabs(timeout)))
    }),
    // Replaces the `SLEEP 1` between `ADDRESSBAR` and `ENTER` (they land in one frame otherwise, Enter first) and the
    // `SLEEP 3` for a new tab's first load. A tab that hasn't reported its first load yet counts as not loading.
    row("WAITIDLE [secs]", "Hold later commands until typed input has landed, no view transition runs and neither the focused tab nor the split's pane is loading, waking or short of its first frame (default 60 s max)", |rest| {
        wait_timeout(rest).map(|timeout| Parsed::Reader(ReaderOp::WaitIdle(timeout)))
    }),
    row("SUGGEST", "Toolbar's Suggest groups: cluster this level's cards by page text (read the offers in EXPORTSTATE `group_suggestions`)", |_| {
        job(|app, _| {
            app.suggest_groups();
        })
    }),
    row("TIDY", "Toolbar's Tidy: release this level's drop anchors, pins stay (one undo step)", |_| {
        job(|app, _| app.tidy_level())
    }),
    row("ACCEPTSUGGESTION n", "Accept the n-th (from 0) suggested group", |rest| {
        let index: usize = rest.parse().ok()?;
        job(move |app, _| app.accept_suggestion(index))
    }),
    row("ACCEPTSUGGESTIONLABEL n", "Accept the n-th (from 0) suggestion as a label on its cards instead of a group", |rest| {
        let index: usize = rest.parse().ok()?;
        job(move |app, _| app.accept_suggestion_as_label(index))
    }),
    row("WAITINDEX [secs]", "Hold later commands until the page-text index has caught up with the loaded pages (default 60 s max)", |rest| {
        wait_timeout(rest).map(|timeout| Parsed::Reader(ReaderOp::WaitIndex(timeout)))
    }),
    // A driver script's own `sleep` doesn't stay in order: while the reader is blocked (`WAITTABS`, `SCREENSHOT`) the
    // script keeps writing, and the lines it buffered arrive back to back.
    row("SLEEP secs", "Hold later commands for `secs` seconds, not milliseconds (fractions too: `SLEEP 0.2`; in order, unlike a driver script's own sleep)", |rest| {
        parse_secs(rest).map(|duration| Parsed::Reader(ReaderOp::Sleep(duration)))
    }),
    row("TOGGLESHORTCUTSHELP", "F1 -- toggle the keyboard-shortcut reference popup", |_| {
        job(|app, _| app.menus.shortcuts_help_open = !app.menus.shortcuts_help_open)
    }),
    row("CONFIRM kind", "Open a confirm dialog: destructive (delete session) or safe (download Servo)", |rest| {
        // Nothing runs unless the dialog's button is clicked, and nothing opens while another confirm is up.
        match rest {
            "destructive" => job(|app, _| app.offer_confirm(Confirm::SessionDelete("probe".to_string()))),
            "safe" => job(|app, _| app.offer_confirm(Confirm::View(crate::view_info::ViewConfirm::Download(ViewKind::Servo)))),
            _ => None,
        }
    }),
    row("MENUPAGE name", "Show the open card menu's jar page, or its items for none", |rest| {
        let page = match rest {
            "jar" => Some(crate::node_menu::MenuPage::Jar),
            "none" => None,
            _ => return None,
        };
        job(move |app, _| {
            app.menus.node_menu.show_page(page);
        })
    }),
    row("TOGGLEPOPUP name", "Toggle the Settings page's bookmarks|history|downloads popup (opens the page), the graph toolbar's recentlyclosed list, or the Back|Forward chip's branch list (needs its marker)", |rest| {
        let which = match rest {
            "bookmarks" => AnchoredPopup::Bookmarks,
            "history" => AnchoredPopup::History,
            "downloads" => AnchoredPopup::Downloads,
            "back" => AnchoredPopup::BackBranches,
            "forward" => AnchoredPopup::ForwardBranches,
            "recentlyclosed" => AnchoredPopup::RecentlyClosed,
            _ => return None,
        };
        job(move |app, _| {
            if !matches!(which, AnchoredPopup::BackBranches | AnchoredPopup::ForwardBranches | AnchoredPopup::RecentlyClosed) {
                app.open_settings_page();
            }
            popups::toggle_popup(&mut app.menus.open_popup, which);
        })
    }),
    row("SPLIT kind", "Open a Split pane running `kind` (servo|cef|webkit|webview2)", |rest| {
        let kind = ViewKind::from_name(rest)?;
        job(move |app, _| app.start_split(kind))
    }),
    row("CLOSESPLIT", "Close the Split pane", |_| job(|app, _| app.close_split())),
    row("JSONSELECT [pointer]", "Select an entry on the current JSON level (no pointer: the root)", |rest| {
        with_str(rest, |app, pointer| app.apply_json_edit(JsonEdit::Select(pointer)))
    }),
    row("JSONTOGGLE [pointer]", "Collapse or expand a card on the current JSON level", |rest| {
        with_str(rest, |app, pointer| app.apply_json_edit(JsonEdit::Toggle(pointer)))
    }),
    row("JSONMORE [pointer]", "Reveal the next page of a card on the current JSON level", |rest| {
        with_str(rest, |app, pointer| app.apply_json_edit(JsonEdit::More(pointer)))
    }),
    row("JSONRELAYOUT", "Relayout the current JSON level (clears saved card positions)", |_| job(|app, _| app.apply_json_edit(JsonEdit::Relayout))),
    row("JSONSEARCH text", "Search the current JSON level's keys and values", |rest| with_str(rest, |app, text| app.set_json_search(text))),
    // Node commands take explicit ids, since a headless test can't rely on focus.
    row("CLOSENODE id", "Close node `id` (any kind), as its card's Close does, and save", |rest| {
        with_id(rest, |app, id| {
            if !app.debug_close_node(id) {
                eprintln!("CLOSENODE: no node {id}; ignored");
            }
        })
    }),
    row("MOVETOGROUP id group", "Move node `id` into the sibling group `group`, same as its \"Move into\" menu row", |rest| {
        let mut parts = rest.split_whitespace();
        let (id, group) = (id_word(&mut parts)?, id_word(&mut parts)?);
        job(move |app, _| {
            if !app.debug_move_to_group(id, group) {
                eprintln!("MOVETOGROUP: {group} isn't a sibling group of {id}; ignored");
            }
        })
    }),
    row("LINK a b", "Link card `a` to card `b`, same as `a`'s \"Link to...\" row and a click on `b`", |rest| {
        let mut parts = rest.split_whitespace();
        let (from, to) = (id_word(&mut parts)?, id_word(&mut parts)?);
        job(move |app, _| match app.connect(crate::caps::LINK, from, to) {
            Ok(()) => {
                app.save_session();
            }
            Err(err) => eprintln!("LINK: {from} -> {to} refused: {err:?}"),
        })
    }),
    row("UNLINK a b", "Remove the link between cards `a` and `b`, in either order", |rest| {
        let mut parts = rest.split_whitespace();
        let (a, b) = (id_word(&mut parts)?, id_word(&mut parts)?);
        job(move |app, _| {
            app.disconnect(crate::caps::LINK, a, b);
            app.save_session();
        })
    }),
    row("OPENEDFROM child parent", "Set the card `child` was opened from, same as its \"Opened from...\" row", |rest| {
        let mut parts = rest.split_whitespace();
        let (child, parent) = (id_word(&mut parts)?, id_word(&mut parts)?);
        job(move |app, _| match app.connect(crate::caps::OPENED_FROM, child, parent) {
            Ok(()) => {
                app.save_session();
            }
            Err(err) => eprintln!("OPENEDFROM: {child} -> {parent} refused: {err:?}"),
        })
    }),
    // `text` is the rest of the line, possibly empty.
    row("SETNOTETEXT id text", "Type `text` into note `id`'s box (replacing it) and blur it, which commits and saves", |rest| {
        let (id, text) = split_id(rest)?;
        let text = text.to_string();
        job(move |app, _| {
            if !app.debug_set_note_text(id, text) {
                eprintln!("SETNOTETEXT: {id} isn't a note; ignored");
            }
        })
    }),
    row("MOVENODE id x y", "Drag card `id` to world point (x, y) and release it there: its document pos and drop anchor are written as a drag release writes them, and saved", |rest| {
        let (id, x, y) = parse_move_node(rest)?;
        job(move |app, _| {
            if !app.debug_move_node(id, egui::vec2(x, y)) {
                eprintln!("MOVENODE: no node {id}; ignored");
            }
        })
    }),
    row("SLEEPNODE id", "Ask the tab `id` (or every tab of the group `id`) to sleep, same as its \"Sleep\" context-menu action; it sleeps when the engine answers the page-state query (wait a second)", |rest| {
        with_id(rest, |app, id| {
            if app.is_container(id) {
                app.sleep_group(id);
            } else {
                app.request_sleep(id, SleepWhy::Manual);
            }
        })
    }),
    row("RESTARTVIEW id", "Start a new view host for live tab `id` at its URL, same as its \"Restart View\" row (offered while it is not responding)", |rest| {
        with_id(rest, |app, id| {
            if !app.restart_view(id) {
                eprintln!("RESTARTVIEW: no live tab {id}; ignored");
            }
        })
    }),
    row("ENDVIEW id", "End live tab `id`'s view host and leave a failed card, same as its \"End View\" row", |rest| {
        with_id(rest, |app, id| {
            if !app.end_view(id) {
                eprintln!("ENDVIEW: no live tab {id}; ignored");
            }
        })
    }),
    row("IDLESWEEP secs", "Run one idle auto-sleep sweep: tabs out of sight for `secs` seconds are asked to sleep, and those with a changed form stay awake (the clock updates on every sweep)", |rest| {
        let secs: u64 = rest.parse().ok()?;
        job(move |app, _| app.sweep_idle_tabs(Some(std::time::Duration::from_secs(secs))))
    }),
    // `name` is a label ("Work · #1a2b") or a stored name; a stored name several jars share picks the lowest node id
    // (`App::jar_named`).
    row("SETJAR <name|default>", "The focused tab's jar edge to that jar, by label or name (a new jar card at its level if none has it)", |rest| {
        with_text(rest, |app, name| {
            if app.set_focused_tab_jar_named(&name) {
                app.save_session();
            }
        })
    }),
    row("SWITCHJARNOW", "The focused tab moves to its new jar now, if it is waiting to switch", |_| {
        job(|app, _| {
            if let Some(tab) = app.focused {
                app.switch_jar_now(tab);
            }
        })
    }),
    row("CLEARJAR <name|default>", "Clear that jar's cookies, as its confirmed \"Clear cookies\" does", |rest| {
        with_text(rest, |app, name| {
            let jar = if name.trim().eq_ignore_ascii_case("default") { app.default_jar() } else { app.jar_named(&name) };
            match jar {
                Some(jar) => app.clear_jar_cookies(jar),
                None => eprintln!("CLEARJAR: no jar named {name:?}"),
            }
        })
    }),
    row("NEWJAR <name>", "A new jar card at the current level, as \"New Jar...\" makes (no edge)", |rest| {
        with_text(rest, |app, name| {
            // `placement` jitters by the id it is given and leaves a JSON level first, as a new card does.
            let jitter = app.alloc_node_id();
            let at = app.placement(jitter, None);
            let id = app.create_jar(&name, at);
            eprintln!("NEWJAR: {id}");
            app.save_session();
        })
    }),
    row("USEJAR <id> <jar|none>", "Tab or group `id` uses jar card `jar`, or inherits with `none`, as a Jar page row", |rest| {
        let (from, jar) = parse_use_jar(rest)?;
        job(move |app, _| {
            app.set_jar_edge(from, jar);
            app.save_session();
        })
    }),
    row("JAREPHEMERAL <id> on|off", "Jar card `id`'s Make/Stop Ephemeral, confirmed", |rest| {
        let mut parts = rest.split_whitespace();
        let id = id_word(&mut parts)?;
        let on = on_off(parts.next()?)?;
        job(move |app, _| {
            if app.jar_exists(id) {
                app.set_jar_ephemeral(id, on);
                app.save_session();
            } else {
                eprintln!("JAREPHEMERAL: {id} isn't a jar card; ignored");
            }
        })
    }),
    // Adds a fresh copy of the demo layout to whatever's open, never touching `has_shown_first_launch_demo`: purely
    // a look-at-it aid, not a way to simulate real first-launch persistence.
    row("TRIGGERFIRSTLAUNCHDEMO", "Dev-only: re-spawn the first-launch demo graph on demand", |_| job(|app, _| app.spawn_first_launch_demo())),
    row("HELP", "Print this command list (`?` too)", |_| job(|_, _| print_help())),
];

impl App {
    /// `MOVETOGROUP`: what the node menu's "Move into <group>" row does (`NodeAction::MoveToGroup`, a Join drop,
    /// which saves), only when the menu would offer `group`. `false` when it wouldn't.
    pub(super) fn debug_move_to_group(&mut self, id: NodeId, group: NodeId) -> bool {
        let offered = self.node_menu_view(id).is_some_and(|v| v.move_targets.iter().any(|(g, _)| *g == group));
        if offered {
            self.apply_drop(CardId::from(id), Some((DropEffect::Join, Some(CardId::from(group)))));
        }
        offered
    }

    /// `SETNOTETEXT`: what a click into note `id`, typing that leaves its box holding `text`, and a click away do.
    /// The box is the payload's buffer and `editing` is on while typed in (`paint_note_body`), so a remote text
    /// arriving meanwhile would be held; the blur (`blur_note`) then resolves it and saves. `false` when `id` isn't a
    /// note.
    pub(super) fn debug_set_note_text(&mut self, id: NodeId, text: String) -> bool {
        let Some(note) = self.nodes.get_mut(&id).and_then(|n| n.as_note_mut()) else { return false };
        note.editing = true;
        note.text = text;
        self.blur_note(id);
        true
    }

    /// `MOVENODE`: card `id` dragged to world point `pos` and released. The drag moves the layout (`card.rs`), the
    /// release drops it there (`drop_at`: its document pos and anchor, as `release_drag` writes them), and the drop
    /// saves (`apply_drop` with no target). `false` when there is no such node.
    pub(super) fn debug_move_node(&mut self, id: NodeId, pos: egui::Vec2) -> bool {
        if !self.nodes.contains_key(&id) {
            return false;
        }
        self.snap_layout(id, pos);
        self.drop_at(id);
        self.save_session();
        true
    }

    /// `CLOSENODE`: `close_node` on node `id`. `false` when there is no such node.
    pub(super) fn debug_close_node(&mut self, id: NodeId) -> bool {
        if !self.nodes.contains_key(&id) {
            return false;
        }
        self.close_node(id);
        true
    }
}
