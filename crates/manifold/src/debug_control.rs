//! The headless-mode automation protocol, one command per stdin line (or `--script` file line). It exists so live GUI
//! testing needn't fight Windows' foreground lock (`SetForegroundWindow` can be silently refused while the user is
//! active, making `SendKeys`/`mouse_event` automation unreliable): each command drives the same code path real OS
//! input would, through a channel, so a hidden window never needs real focus.
//!
//! `VERBS` is the one table: each row is a verb's usage, its help line, whether it is pointer input, and a parser
//! from the rest of the line to a `Parsed`. `HELP`, the line parser and the sync test all read it, so adding a
//! command is adding a row. A row's parser yields either a `Job` (a closure run on the UI thread, with the already
//! parsed arguments captured) or a `ReaderOp` (`WAITTABS`, `WAITIDLE`, `WAITINDEX`, `SLEEP`, `SCREENSHOT`: the stdin reader
//! blocks on those itself, so later lines stay in order behind them).
//!
//! Mouse commands are genuine synthetic `winit::event::WindowEvent`s (`App::dispatch_window_event`), deliberately
//! not a logic-bypassing shortcut, since that is the only way to exercise egui's real hit-testing and render timing.
//! Keyboard commands can't go through winit (`winit::event::KeyEvent` has no public constructor): the address-bar
//! keys (`ADDRESSBAR`, `ENTER`, `ESCAPE`, `ARROWDOWN`, `ARROWUP`) and `EGUITEXT`/`BACKSPACE` for any focused egui
//! field are injected one level down, as `egui::Event::Key`/`Text` in the next frame's `RawInput`, so the UI's own
//! `key_pressed` checks and the real `TextEdit` see them. Headless mode also marks the window as focused for egui,
//! since egui reports no widget focus in an unfocused window. The remaining keyboard commands (shortcuts, page
//! input) call the equivalent effect directly.
//!
//! Driving egui through these, learned the hard way: use `CLICK`, not a `MOUSEDOWN`/`MOUSEUP` pair spaced by a
//! `sleep` -- egui drops a press held past its max click duration (~0.8 s) as a long press, not a click. A drag
//! needs several `MOUSEMOVE`s between the down and the up, not one. One `MOUSEMOVE` is enough for a `SCREENSHOT` to
//! show hover (checked headless on a tab card: ✕ and peek preview both up), and a right-click needs none, since the
//! graph finds the card under the press itself rather than going by hover. Each command renders its own frame in
//! headless mode, and a pointer command also one before it, so it's hit-tested against the current state (see
//! `UserEvent::Debug`'s handler); mouse commands need no `sleep` between them. Animations (menu fades, group
//! drill-in) still take real time. Open a right-click menu with `MOUSEDOWN`/`MOUSEUP ... right`, not
//! `CLICK ... right`: a menu's first frame is egui's sizing pass, which may place it away from the pointer, and a
//! release in that same frame counts as a click outside it, which closes it. First-launch demo tabs spawn
//! asynchronously and can take 30 s or more headless; `WAITTABS` waits for them, since until the focused tab has
//! spawned, commands that need it (e.g. `ADDDOWNLOAD`) do nothing.

use std::io::BufRead;
use std::str::SplitWhitespace;

use manifold_view_protocol::KeyCode;
use winit::event::MouseButton;
use winit::event_loop::EventLoopProxy;

use super::*;
use crate::event_loop::UserEvent;
use crate::nav_history::Dir;
use crate::process_manager::ViewKind;
use crate::sleep_state::SleepWhy;

/// What one parsed line does on the UI thread.
pub(crate) type Job = Box<dyn FnOnce(&mut App, &ActiveEventLoop) + Send>;

/// A `Job` as it crosses the channel (`UserEvent::Debug`), with its row's pointer tag.
pub(crate) struct DebugJob {
    /// Pointer input: egui hit-tests it against the last frame's widgets, which may predate whatever changed since
    /// the previous command (a tab that just spawned, suggestions that just arrived; a CLICK on a node right after
    /// WAITTABS missed 3 runs out of 3), so a headless run draws the current state first.
    pub(crate) pointer: bool,
    run: Job,
}

impl DebugJob {
    fn new(pointer: bool, run: Job) -> Self {
        Self { pointer, run }
    }

    /// The exit the reader sends at end of input.
    pub(crate) fn quit() -> Self {
        Self::new(false, Box::new(quit))
    }
}

/// What the stdin reader does itself, blocking, so the lines after it wait.
pub(crate) enum ReaderOp {
    WaitTabs(std::time::Duration),
    WaitIdle(std::time::Duration),
    WaitIndex(std::time::Duration),
    Sleep(std::time::Duration),
    Screenshot(String),
}

/// One row's parse result.
enum Parsed {
    Job(Job),
    Reader(ReaderOp),
}

/// `parse_line`'s result.
pub(crate) enum Line {
    Run(DebugJob),
    Reader(ReaderOp),
}

struct Verb {
    /// `VERB args`, as `HELP` shows it; the first word is the verb.
    usage: &'static str,
    doc: &'static str,
    pointer: bool,
    /// From the line's text after the verb (trimmed), or `None` for malformed arguments.
    parse: fn(&str) -> Option<Parsed>,
}

impl Verb {
    fn word(&self) -> &'static str {
        self.usage.split_whitespace().next().unwrap_or(self.usage)
    }
}

const fn row(usage: &'static str, doc: &'static str, parse: fn(&str) -> Option<Parsed>) -> Verb {
    Verb { usage, doc, pointer: false, parse }
}

const fn pointer_row(usage: &'static str, doc: &'static str, parse: fn(&str) -> Option<Parsed>) -> Verb {
    Verb { usage, doc, pointer: true, parse }
}

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

fn true_false(word: &str) -> Option<bool> {
    match word {
        "true" => Some(true),
        "false" => Some(false),
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
fn parse_click(rest: &str) -> Option<((f64, f64), MouseButton, bool)> {
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
fn parse_move_node(rest: &str) -> Option<(NodeId, f32, f32)> {
    let mut parts = rest.split_whitespace();
    let id = id_word(&mut parts)?;
    let mut coord = || parts.next()?.parse::<f32>().ok().filter(|v| v.is_finite());
    let (x, y) = (coord()?, coord()?);
    parts.next().is_none().then_some((id, x, y))
}

/// `GRAPHZOOM pct`.
fn parse_zoom_pct(rest: &str) -> Option<f32> {
    rest.parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// `RENAMEGROUP [id name]`: `Some(None)` alone (F2), `Some(Some((id, name)))` with both, `None` for an id with no name.
fn parse_rename(rest: &str) -> Option<Option<(NodeId, String)>> {
    if rest.is_empty() {
        return Some(None);
    }
    let (id, name) = split_id(rest)?;
    (!name.is_empty()).then(|| Some((id, name.to_string())))
}

/// `USEJAR <id> <jar|none>`.
fn parse_use_jar(rest: &str) -> Option<(NodeId, Option<NodeId>)> {
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
fn quit(app: &mut App, event_loop: &ActiveEventLoop) {
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
fn parse_add_file(rest: &str) -> (String, Option<(f32, f32)>) {
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
fn split_id(rest: &str) -> Option<(NodeId, &str)> {
    let (id, text) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    Some((id.parse().ok()?, text.trim()))
}

/// The command table: one row per verb, in `HELP`'s order.
#[rustfmt::skip]
const VERBS: &[Verb] = &[
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
    row("GRAPHUP", "Graph view: move focus up", |_| job(|app, _| app.move_graph_focus(egui::vec2(0.0, -1.0)))),
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
            let modifiers = app.key_modifiers;
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
                kind.desc().name,
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
        job(move |app, _| app.pending_graph_zoom = Some(pct / 100.0))
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
    row("FINDMATCHCASE true|false", "Toggle the find bar's case-sensitivity", |rest| {
        let match_case = true_false(rest.split_whitespace().next()?)?;
        job(move |app, _| {
            app.find.match_case = match_case;
            app.send_find_start();
        })
    }),
    row("FINDNEXT", "Find bar: next match", |_| job(|app, _| app.send_find_navigate(true))),
    row("FINDPREVIOUS", "Find bar: previous match", |_| job(|app, _| app.send_find_navigate(false))),
    row("CLOSEFINDBAR", "Close the find bar", |_| job(|app, _| app.close_find_bar())),
    // Bypasses `focus_tab`/`leave_to_graph`'s own visibility changes, which always come bundled with a `Resize`: for
    // testing the `SetVisible(false)` -> `true` repaint path in isolation.
    row("SETVISIBLE true|false", "Send SetVisible directly to the focused tab (bypasses the usual resize-bundled visibility change)", |rest| {
        let visible = true_false(rest.split_whitespace().next()?)?;
        job(move |app, _| on_focused_tab(app, |node| node.tab.set_visible(visible)))
    }),
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
    row("QUICKSWITCHDOWN", "Omnibar popup: move highlight down", |_| job(|app, _| app.omni_move(Host::Popup, 1))),
    row("QUICKSWITCHUP", "Omnibar popup: move highlight up", |_| job(|app, _| app.omni_move(Host::Popup, -1))),
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
    row("HISTORYFILTERTEXT text", "Set the History popup's own filter box", |rest| with_str(rest, |app, text| app.history_ui.history_filter = text)),
    row("BOOKMARKSFILTERTEXT text", "Set the Bookmarks popup's own filter box", |rest| with_str(rest, |app, text| app.history_ui.bookmarks_filter = text)),
    row("TOGGLEVIEW", "Ctrl+G -- toggle Tab / Graph view", |_| job(|app, _| app.toggle_view_mode_shortcut())),
    row("TOGGLESETTINGSPAGE", "Settings gear -- enter/leave the Settings page", |_| job(|app, _| app.toggle_settings_page())),
    // The file commands take an explicit path instead of a real file dialog: headless mode has no human to answer
    // one, so they call the same underlying save/load directly.
    row("EXPORTSESSION path", "Save the current session to `path`", |rest| {
        with_text(rest, |app, path| app.export_session_to(std::path::Path::new(&path)))
    }),
    row("EXPORTSTATE path", "Write view, address bar and dropdown state to `path` as JSON (see debug_export.rs)", |rest| {
        with_text(rest, |app, path| app.write_export_state(std::path::Path::new(&path)))
    }),
    row("TEXTOF id path", "Write the page text stored for tab `id` to `path`, or `none` (see tabs/page_text.rs)", |rest| {
        let (id, path) = rest.split_once(' ')?;
        let (id, path) = (id.parse().ok()?, path.to_owned());
        job(move |app, _| app.write_text_of(id, std::path::Path::new(&path)))
    }),
    row("IMPORTSESSION path", "Load a session from `path`", |rest| {
        with_text(rest, |app, path| match crate::json_file::load(std::path::Path::new(&path), "session") {
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
            if let Some((_, draft)) = &mut app.history_ui.renaming_bookmark_folder {
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
    row("IMPORTGROUP path", "Import a group from `path`", |rest| {
        with_text(rest, |app, path| match crate::json_file::load::<store::json::JsonDoc>(std::path::Path::new(&path), "group") {
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
    row("RELOAD", "Ctrl+R/F5 -- reload the focused tab", |_| job(|app, _| app.reload_focused_tab_shortcut())),
    row("GOBACK", "Back button on the focused tab", |_| go(Dir::Back)),
    row("GOFORWARD", "Forward button on the focused tab", |_| go(Dir::Forward)),
    row("NEWGROUP", "Ctrl+Shift+G -- new group", |_| job(|app, _| app.new_group_shortcut())),
    row("NEWNOTE", "Graph toolbar's \"+ Note\" button", |_| job(|app, _| app.new_note_shortcut())),
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
    row("WAITIDLE [secs]", "Hold later commands until typed input has landed, no view transition runs and the focused tab isn't loading (default 60 s max)", |rest| {
        wait_timeout(rest).map(|timeout| Parsed::Reader(ReaderOp::WaitIdle(timeout)))
    }),
    row("SUGGEST", "Toolbar's Suggest groups: cluster this level's cards by page text (read the offers in EXPORTSTATE `group_suggestions`)", |_| {
        job(|app, _| {
            app.suggest_groups();
        })
    }),
    row("ARRANGE", "Toolbar's Arrange by topic: lay this level's cards out by page text (one undo step)", |_| {
        job(|app, _| app.arrange_by_topic())
    }),
    row("ACCEPTSUGGESTION n", "Accept the n-th (from 0) suggested group", |rest| {
        let index: usize = rest.parse().ok()?;
        job(move |app, _| app.accept_suggestion(index))
    }),
    row("WAITINDEX [secs]", "Hold later commands until the page-text index has caught up with the loaded pages (default 60 s max)", |rest| {
        wait_timeout(rest).map(|timeout| Parsed::Reader(ReaderOp::WaitIndex(timeout)))
    }),
    // A driver script's own `sleep` doesn't stay in order: while the reader is blocked (`WAITTABS`, `SCREENSHOT`) the
    // script keeps writing, and the lines it buffered arrive back to back.
    row("SLEEP secs", "Hold later commands for `secs` (in order, unlike a driver script's own sleep)", |rest| {
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
    row("MENUPAGE name", "Show the open card menu's color|jar page, or its items for none", |rest| {
        let page = match rest {
            "color" => Some(crate::node_menu::MenuPage::Color),
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
    row("MOVENODE id x y", "Drag card `id` to world point (x, y) and release it there, which saves", |rest| {
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

/// Parses one line of the headless debug-command text protocol. Unknown commands or malformed arguments are `None`
/// (the reader reports them).
pub(crate) fn parse_line(line: &str) -> Option<Line> {
    // Strip a leading BOM — several Windows-side tools that can drive this
    // protocol (PowerShell's default `StandardInput` stream encoding among
    // them, confirmed live: a `Process.StandardInput.WriteLine` from
    // PowerShell prepends `\u{FEFF}` to the very first line) write one
    // unconditionally, and it isn't whitespace so `trim()` alone leaves it
    // stuck to the command name.
    let line = line.strip_prefix('\u{feff}').unwrap_or(line);
    let line = line.trim();
    let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let cmd = if cmd == "?" { "HELP" } else { cmd };
    let verb = VERBS.iter().find(|v| v.word().eq_ignore_ascii_case(cmd))?;
    match (verb.parse)(rest.trim())? {
        Parsed::Job(run) => Some(Line::Run(DebugJob::new(verb.pointer, run))),
        Parsed::Reader(op) => Some(Line::Reader(op)),
    }
}

/// Prints `VERBS` to stderr -- `eprintln!`, not `println!`, matching
/// this protocol's own established convention of surfacing diagnostics on
/// stderr (see e.g. the crash reporter) while stdout stays free for
/// anything a driving script might want to parse on its own.
pub fn print_help() {
    eprintln!("Manifold debug commands:");
    for verb in VERBS {
        eprintln!("  {:<32} {}", verb.usage, verb.doc);
    }
}

/// Runs one reader-thread op. `false` when the event loop is gone.
fn run_reader_op(proxy: &EventLoopProxy<UserEvent>, op: ReaderOp) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let (timeout, waiting_for, job): (_, String, Job) = match op {
        ReaderOp::Sleep(duration) => {
            std::thread::sleep(duration);
            return true;
        }
        // The app sends `()` on `tx` once `tab_spawns_in_flight` is zero.
        ReaderOp::WaitTabs(timeout) => (
            timeout,
            format!("WAITTABS: tab spawns still in flight after {timeout:?}; continuing"),
            Box::new(move |app, _| {
                if app.spawns.tab_spawns_in_flight == 0 {
                    let _ = tx.send(());
                } else {
                    app.spawns.tabs_spawned_waiters.push(tx);
                }
            }),
        ),
        // The app sends `()` on `tx` once `App::debug_idle` holds, checked when it arrives and at the end of every
        // frame.
        ReaderOp::WaitIdle(timeout) => (
            timeout,
            format!("WAITIDLE: input or a load still pending after {timeout:?}; continuing"),
            Box::new(move |app, _| {
                app.debug.idle_waiters.push(tx);
                app.debug.idle_frames = 0;
                // Pending input needs frames to land, and the check runs at the end of each.
                app.release_idle_waiters();
            }),
        ),
        // The app sends `()` on `tx` once `App::index_settled` holds, checked when it arrives and at the end of every
        // frame. An engine that never answers `QueryPageText` (Servo, WebKitGTK and CEF have no arm yet) keeps it from
        // holding until the timeout.
        ReaderOp::WaitIndex(timeout) => (
            timeout,
            format!("WAITINDEX: the index still behind after {timeout:?}; continuing"),
            Box::new(move |app, _| {
                app.debug.index_waiters.push(tx);
                app.release_index_waiters();
            }),
        ),
        // Frames can stall for seconds and the app holds one pending screenshot, so an unblocked second
        // `SCREENSHOT` (or a `QUIT`) would drop the first.
        ReaderOp::Screenshot(path) => (
            std::time::Duration::from_secs(30),
            format!("SCREENSHOT: {path} not taken after 30 s; continuing"),
            Box::new(move |app, _| {
                app.debug.pending_screenshot = Some(path);
                app.debug.screenshot_waiter = Some(tx);
                if let Some(window) = &app.window {
                    window.request_redraw();
                }
            }),
        ),
    };
    if proxy.send_event(UserEvent::Debug(DebugJob::new(false, job))).is_err() {
        return false;
    }
    if rx.recv_timeout(timeout).is_err() {
        eprintln!("{waiting_for}");
    }
    true
}

/// Spawns the stdin-reading thread for headless mode — one command per
/// line, forwarded to the app via `proxy` (the same `EventLoopProxy`
/// mechanism winit itself uses for user events, so commands land on the
/// real event loop like any other event). Stdin EOF (the driving script/
/// process closed its write end) sends `Quit` so a headless run driven by a
/// piped script exits cleanly instead of hanging forever.
pub fn spawn_stdin_reader(proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        // `--script <file>` reads the commands from a file instead of stdin,
        // for a run that has no stdin to pipe (the elevated `Samply` task).
        let args: Vec<String> = std::env::args().collect();
        let script = args.iter().position(|a| a == "--script").and_then(|i| args.get(i + 1));
        let input: Box<dyn std::io::BufRead> = match script {
            Some(path) => match std::fs::File::open(path) {
                Ok(file) => Box::new(std::io::BufReader::new(file)),
                Err(e) => {
                    eprintln!("--script {path}: {e}");
                    let _ = proxy.send_event(UserEvent::Debug(DebugJob::quit()));
                    return;
                }
            },
            None => Box::new(std::io::stdin().lock()),
        };
        for line in input.lines() {
            let Ok(line) = line else { break };
            match parse_line(&line) {
                Some(Line::Reader(op)) => {
                    if !run_reader_op(&proxy, op) {
                        break;
                    }
                }
                Some(Line::Run(job)) => {
                    if proxy.send_event(UserEvent::Debug(job)).is_err() {
                        break;
                    }
                }
                // A blank line (an artifact of the stream itself, not
                // something anyone typed) stays silent; anything else that
                // failed to parse gets a visible error instead of a
                // same-as-success no-op, so a typo doesn't read as "this
                // command does nothing."
                None if !line.trim().is_empty() => {
                    eprintln!("Unrecognized debug command: {line:?} -- try HELP");
                }
                None => {}
            }
        }
        let _ = proxy.send_event(UserEvent::Debug(DebugJob::quit()));
    });
}

/// The headless debug channel's pending work (`App::debug`): synthetic input, deferred quits and the capture/idle waiters.
#[derive(Default)]
pub(crate) struct HeadlessDebug {
    /// Key events from headless `ENTER`/`ESCAPE`/`ARROWDOWN`/`ARROWUP`,
    /// added to the next frame's egui input exactly like real key presses
    /// (see `run_egui_frame`), so they take the same code paths and, like
    /// a real key, last one frame whether or not anything reads them.
    pub(crate) debug_input_events: Vec<egui::Event>,
    /// Text from a headless `ADDRESSBAR`, typed (select-all, then the text)
    /// once the address bar actually has egui focus: the command requests
    /// focus through `pending_focus`, which only takes hold in
    /// the frame after, so typing waits for a frame where
    /// `address_bar.focused` is true.
    pub(crate) debug_address_bar_typing: Option<String>,
    /// Set by a headless `QUIT` that arrived while debug input above (or
    /// `pending_screenshot`) was still waiting for a frame — e.g. `ENTER`+`QUIT`,
    /// or `SCREENSHOT`+`QUIT`, sent back-to-back with no delay. Exiting
    /// immediately in that case could drop the pending command/screenshot
    /// before any frame ever consumed it (`event_loop.exit()` gives no
    /// guarantee a `RedrawRequested` runs first). Holds how many more frames
    /// to wait: `headless_end_of_frame` exits once nothing is pending, or when
    /// this runs out (an `ADDRESSBAR` needs two frames, focus then typing;
    /// a screenshot on a stalled surface might never land).
    pub(crate) quit_after_redraws: Option<u8>,
    /// Set by a headless `SCREENSHOT` command; consumed on the next
    /// `RedrawRequested` and passed into `gpu::GpuState::render` (`render_frame`).
    pub(crate) pending_screenshot: Option<String>,
    /// The stdin reader waiting on `pending_screenshot` (see
    /// `ReaderOp::Screenshot`); sent `()` once it's been attempted.
    pub(crate) screenshot_waiter: Option<std::sync::mpsc::Sender<()>>,
    /// "Print to PDF": (temp PNG, PDF): the capture `start_print_to_pdf`
    /// asked for, and where its PDF goes. Paired so an unrelated screenshot
    /// can't take the PDF target. Set alongside `pending_screenshot` so the
    /// GPU capture needn't know PDF exists; once that capture completes
    /// (`handle_screenshot_result`), the temp PNG is converted via
    /// `pdf::write_image_as_pdf` and deleted. `None` the rest of the time,
    /// including for an ordinary "Screenshot" button click.
    pub(crate) pending_print_to_pdf: Option<(String, String)>,
    /// Senders of the headless `WAITIDLE` commands waiting for `debug_idle`.
    pub(crate) idle_waiters: Vec<std::sync::mpsc::Sender<()>>,
    /// Senders of the headless `WAITINDEX` commands waiting for `index_settled`.
    pub(crate) index_waiters: Vec<std::sync::mpsc::Sender<()>>,
    /// Consecutive frame ends at which `debug_idle` held, while `idle_waiters` waits; `QUIET_FRAMES` releases them.
    pub(crate) idle_frames: u8,
}

impl App {
    /// `MOVETOGROUP`: what the node menu's "Move into <group>" row does (`NodeAction::MoveToGroup`, a Join drop,
    /// which saves), only when the menu would offer `group`. `false` when it wouldn't.
    fn debug_move_to_group(&mut self, id: NodeId, group: NodeId) -> bool {
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
    fn debug_set_note_text(&mut self, id: NodeId, text: String) -> bool {
        let Some(note) = self.nodes.get_mut(&id).and_then(|n| n.as_note_mut()) else { return false };
        note.editing = true;
        note.text = text;
        self.blur_note(id);
        true
    }

    /// `MOVENODE`: card `id` dragged to world point `pos` and released. The drag moves the layout (`card.rs`), the
    /// release writes the document pos (`release_drag`: `edit_pos` with the layout pos), and the drop saves
    /// (`apply_drop` with no target). `false` when there is no such node.
    fn debug_move_node(&mut self, id: NodeId, pos: egui::Vec2) -> bool {
        if !self.nodes.contains_key(&id) {
            return false;
        }
        self.layout.entry(id).or_default().pos = pos;
        self.edit_pos(id, Some(self.layout_pos(id)));
        self.save_session();
        true
    }

    /// `CLOSENODE`: `close_node` on node `id`. `false` when there is no such node.
    fn debug_close_node(&mut self, id: NodeId) -> bool {
        if !self.nodes.contains_key(&id) {
            return false;
        }
        self.close_node(id);
        true
    }

    /// Runs one parsed debug line (`DebugJob`), after letting a finished view transition end: a transition only ends
    /// in a frame, and frames can stall for seconds on a busy GPU, so one that ran out long ago could still be `Some`
    /// here and make the ENTER/ESCAPE/arrow guards drop their key.
    pub(crate) fn run_debug_job(&mut self, event_loop: &ActiveEventLoop, job: DebugJob) {
        self.finish_transition_if_done();
        (job.run)(self, event_loop);
    }
}

/// A key press as egui receives it from a keyboard, for headless key
/// commands (see `App::debug_input_events`).
pub(crate) fn debug_key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::testutil::node;

    fn parses(line: &str) -> bool {
        parse_line(line).is_some()
    }

    /// Fix batch A5: `EGUITEXT` types into the focused egui field (a jar name, a group rename) and `BACKSPACE` erases
    /// there; `TYPE` still goes to the page.
    #[test]
    fn egui_typing_commands_parse() {
        assert!(parses("EGUITEXT Work Stuff"));
        assert!(!parses("EGUITEXT"), "text is required");
        assert!(parses("backspace"));
        assert!(parses("TYPE abc"));
    }

    #[test]
    fn click_takes_an_optional_button_and_ctrl() {
        let click = |rest| parse_click(rest).map(|((x, y), button, ctrl)| (x, y, button, ctrl));
        assert_eq!(click("1 2"), Some((1.0, 2.0, MouseButton::Left, false)));
        assert_eq!(click("1 2 right"), Some((1.0, 2.0, MouseButton::Right, false)));
        assert_eq!(click("1 2 ctrl"), Some((1.0, 2.0, MouseButton::Left, true)));
        assert_eq!(click("1 2 right CTRL"), Some((1.0, 2.0, MouseButton::Right, true)));
        assert_eq!(click("1"), None);
        assert!(parses("click 1 2 right CTRL") && !parses("CLICK 1"));
    }

    #[test]
    fn wait_seconds_must_be_finite_and_bounded() {
        assert_eq!(parse_secs("1.5"), Some(std::time::Duration::from_millis(1500)));
        assert_eq!(parse_secs("0"), Some(std::time::Duration::ZERO));
        for bad in ["inf", "-inf", "NaN", "1e30", "-1", "86401", "x", ""] {
            assert_eq!(parse_secs(bad), None, "{bad}");
        }
        assert!(!parses("SLEEP inf"));
        assert!(!parses("WAITTABS 1e30"));
        assert!(!parses("WAITIDLE 1e30"));
        assert!(parses("WAITINDEX") && parses("WAITINDEX 5"));
        let waits = |line| match parse_line(line) {
            Some(Line::Reader(ReaderOp::WaitTabs(d) | ReaderOp::WaitIdle(d))) => Some(d.as_secs()),
            _ => None,
        };
        assert_eq!(waits("WAITTABS"), Some(60));
        assert_eq!(waits("WAITIDLE"), Some(60));
        assert_eq!(waits("WAITIDLE 5"), Some(5));
    }

    /// Reader-thread verbs block the stdin reader; every other verb is a UI-thread job, and only the mouse verbs are
    /// pointer input.
    #[test]
    fn rows_are_reader_ops_or_jobs_and_only_mouse_rows_are_pointer() {
        for line in ["WAITTABS", "WAITIDLE", "SLEEP 1", "SCREENSHOT x.png"] {
            assert!(matches!(parse_line(line), Some(Line::Reader(_))), "{line}");
        }
        for line in ["ENTER", "QUIT", "HELP", "?"] {
            assert!(matches!(parse_line(line), Some(Line::Run(job)) if !job.pointer), "{line}");
        }
        let pointer: Vec<&str> = VERBS.iter().filter(|v| v.pointer).map(Verb::word).collect();
        assert_eq!(pointer, ["MOUSEMOVE", "MOUSEDOWN", "MOUSEUP", "CLICK", "DOUBLECLICK", "TRIPLECLICK", "SCROLL"]);
    }

    /// Every verb has a sample line that parses, and every sampled verb is in the table.
    #[test]
    fn every_verb_has_a_parsing_sample() {
        const SAMPLES: &[&str] = &[
            "MOUSEMOVE 1 2",
            "MOUSEDOWN 1 2",
            "MOUSEUP 1 2 right",
            "CLICK 1 2",
            "DOUBLECLICK 1 2",
            "TRIPLECLICK 1 2",
            "SCROLL 1 2 0 -3",
            "ADDRESSBAR example.com",
            "ENTER",
            "ESCAPE",
            "CONFIRM destructive",
            "BACKSPACE",
            "EGUITEXT Work",
            "ARROWRIGHT",
            "ARROWLEFT",
            "SPACEKEY",
            "DIGITKEY 3",
            "ARROWDOWN",
            "ARROWUP",
            "SHIFTF10",
            "TABKEY shift",
            "UNDOKEY shift",
            "GRAPHLEFT",
            "GRAPHRIGHT",
            "GRAPHUP",
            "GRAPHDOWN",
            "GRAPHOPEN",
            "TYPE abc",
            "KEYPAGE Enter",
            "REOPENTAB",
            "REOPENNODE 0000000000000001-1",
            "INSTALLVIEW cef",
            "VIEWSTATE cef",
            "ZOOMIN",
            "ZOOMOUT",
            "ZOOMRESET",
            "GRAPHZOOM 60",
            "MARK phase one",
            "OPENDEVTOOLS",
            "NEWTAB",
            "CLOSETAB",
            "FOCUSADDRESS",
            "NEXTTAB",
            "PREVTAB",
            "OPENFINDBAR",
            "FINDTEXT needle",
            "FINDMATCHCASE true",
            "FINDNEXT",
            "FINDPREVIOUS",
            "CLOSEFINDBAR",
            "SETVISIBLE false",
            r"SCREENSHOT C:\x.png",
            "RESIZE 800 600",
            "SURFACE lost",
            "QUIT",
            "QUICKSWITCHOPEN",
            "QUICKSWITCHTEXT abc",
            "QUICKSWITCHDOWN",
            "QUICKSWITCHUP",
            "QUICKSWITCHENTER",
            "QUICKSWITCHESCAPE",
            "SESSIONNAMETEXT work",
            "SAVESESSIONAS work",
            "OPENSESSION work",
            "GRAPHFILTERTEXT abc",
            "HISTORYFILTERTEXT abc",
            "BOOKMARKSFILTERTEXT abc",
            "TOGGLEVIEW",
            "TOGGLESETTINGSPAGE",
            r"EXPORTSESSION C:\x.json",
            r"EXPORTSTATE C:\x.json",
            r"IMPORTSESSION C:\x.json",
            r"EXPORTBOOKMARKS C:\x.json",
            r"IMPORTBOOKMARKS C:\x.json",
            "CLOSEALLLEVEL",
            "CONFIRMIMPORT",
            "CANCELIMPORT",
            "RENAMEGROUP",
            "BOOKMARKFOLDERTEXT Reading",
            "DISMISSPEEK",
            r"EXPORTGROUP C:\x.json",
            r"IMPORTGROUP C:\x.json",
            "DUPLICATETAB",
            "DUPLICATENODE 0000000000000001-1",
            r"PRINTTOPDF C:\x.pdf",
            "TOGGLEPIN",
            "RELOAD",
            "GOBACK",
            "GOFORWARD",
            "NEWGROUP",
            "NEWNOTE",
            r"ADDFILE C:\x.json 10 20",
            r"ADDDOWNLOAD C:\x.zip",
            "WAITTABS 5",
            "WAITIDLE 5",
            "WAITINDEX 5",
            "SUGGEST",
            "ARRANGE",
            "ACCEPTSUGGESTION 0",
            "SLEEP 0.5",
            "TOGGLESHORTCUTSHELP",
            "TOGGLEPOPUP history",
            "TOGGLEPOPUP back",
            "MENUPAGE jar",
            "SPLIT cef",
            "CLOSESPLIT",
            "CLOSENODE 0000000000000001-3",
            "MOVETOGROUP 0000000000000001-1 0000000000000001-3",
            "LINK 0000000000000001-1 0000000000000001-2",
            "UNLINK 0000000000000001-1 0000000000000001-2",
            "OPENEDFROM 0000000000000001-1 0000000000000001-2",
            "SETNOTETEXT 0000000000000001-1 hello",
            "MOVENODE 0000000000000001-1 10 -20",
            "SLEEPNODE 0000000000000001-4",
            r"TEXTOF 0000000000000001-4 C:\x.txt",
            "RESTARTVIEW 0000000000000001-4",
            "ENDVIEW 0000000000000001-4",
            "IDLESWEEP 30",
            "SETJAR default",
            "SWITCHJARNOW",
            "CLEARJAR Work",
            "NEWJAR Work",
            "USEJAR 0000000000000001-4 none",
            "JAREPHEMERAL 0000000000000001-5 on",
            "JSONSELECT /a/0",
            "JSONTOGGLE",
            "JSONMORE /a",
            "JSONRELAYOUT",
            "JSONSEARCH abc",
            "TRIGGERFIRSTLAUNCHDEMO",
            "HELP",
        ];
        let verb = |line: &str| line.split_whitespace().next().unwrap().to_ascii_uppercase();
        for sample in SAMPLES {
            assert!(parses(sample), "sample {sample:?} parses");
            assert!(VERBS.iter().any(|v| v.word() == verb(sample)), "{sample:?} is in the table");
        }
        for v in VERBS {
            assert!(SAMPLES.iter().any(|s| verb(s) == v.word()), "{} has a sample", v.word());
        }
        for (i, v) in VERBS.iter().enumerate() {
            assert!(
                v.word().chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
                "{}: verbs are upper case",
                v.usage
            );
            assert!(VERBS[..i].iter().all(|w| w.word() != v.word()), "{} is listed twice", v.word());
        }
        assert!(parses("?"));
    }

    /// `LINK`, `UNLINK` and `OPENEDFROM` each take two card ids.
    #[test]
    fn link_unlink_and_openedfrom_take_two_ids() {
        for two in [
            "LINK 0000000000000001-1 0000000000000001-2",
            "UNLINK 0000000000000001-1 0000000000000001-2",
            "OPENEDFROM 0000000000000001-6 0000000000000001-7",
        ] {
            assert!(parses(two), "{two}");
        }
        for one in [
            "LINK 0000000000000001-1",
            "UNLINK 0000000000000001-1",
            "OPENEDFROM 0000000000000001-6",
            "MOVETOGROUP 0000000000000001-1",
        ] {
            assert!(!parses(one), "{one}: both ids are required");
        }
    }

    /// `MOVETOGROUP` takes two ids and moves only where the node menu's "Move into" would offer.
    #[test]
    fn move_to_group_moves_only_into_an_offered_sibling() {
        assert!(parse_line("MOVETOGROUP 0000000000000001-1 0000000000000001-3").is_some());
        assert!(parse_line("MOVETOGROUP 0000000000000001-1").is_none(), "both ids are required");
        let mut app = App::default();
        let (g, h, note) = (app.alloc_node_id(), app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(g, None);
        app.insert_group_at(h, None);
        app.insert_note_at(note, None);
        assert!(!app.debug_move_to_group(note, note), "not a group");
        assert!(!app.debug_move_to_group(note, node(9999)));
        assert!(app.debug_move_to_group(note, g));
        assert_eq!(app.doc.mirror.group(note), Some(g));
        assert!(!app.debug_move_to_group(note, h), "h is no longer a sibling of the note");
        assert_eq!(app.doc.mirror.group(note), Some(g));
        assert!(app.debug_move_to_group(g, h), "a group nests, as the menu row does");
        assert_eq!(app.doc.mirror.group(g), Some(h));
    }

    /// `CLOSENODE id` closes any node by id, as its card's Close does, and saves.
    #[test]
    fn close_node_takes_any_node() {
        assert!(parse_line("CLOSENODE 0000000000000001-1").is_some());
        assert!(parse_line("CLOSENODE").is_none(), "the id is required");
        let mut app = App::default();
        let (g, note) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(g, None);
        app.insert_note_at(note, None);
        assert!(app.debug_close_node(note));
        assert!(app.debug_close_node(g), "a group closes too");
        assert!(!app.nodes.contains_key(&note) && !app.nodes.contains_key(&g));
        assert!(!app.debug_close_node(node(9999)));
    }

    /// `REOPENNODE id` reopens a closed node by id, and `EXPORTSTATE` lists the Recently Closed history.
    #[test]
    fn reopennode_reopens_by_id_and_exportstate_lists_the_history() {
        assert!(parse_line("REOPENNODE 0000000000000001-1").is_some());
        assert!(parse_line("REOPENNODE").is_none(), "the id is required");
        let path = crate::store::testutil::temp_dir("debug-reopennode").join("session.mnf");
        let mut app = crate::session_sync::tests::app_on(&path, 1);
        let g = crate::session_sync::tests::add_group(&mut app, "G");
        assert!(app.save_session());
        assert!(app.debug_close_node(g));
        assert_eq!(app.export_state()["closed"], serde_json::json!([g.to_string()]));
        app.reopen_node(g);
        assert!(app.nodes.contains_key(&g));
        assert_eq!(app.export_state()["closed"], serde_json::json!([]));
    }

    /// The three edits reach the document through their writers: the rename's clean name (groups only), the note's
    /// typed text at the blur (beating a remote text held meanwhile, as typing does), and the released card's pos.
    #[test]
    fn name_text_and_pos_commands_write_the_document() {
        use crate::session_sync::tests::{add, add_group, app_on, remote_set, stored};
        use crate::store::model::{field, ActorId, FieldValue, Kind};
        let path = crate::store::testutil::temp_dir("debug-edits").join("session.mnf");
        let mut app = app_on(&path, 1);
        let g = add_group(&mut app, "G");
        let note = add(&mut app, Kind::Note, Runtime::Note(NotePayload { text: "v1".into(), editing: false }));
        assert!(app.save_session());

        app.commit_rename(g, " Errands\u{200B} ");
        assert!(app.save_session());
        assert_eq!(stored(&app, g, field::NAME), FieldValue::Str("Errands".into()));
        app.commit_rename(note, "Not a group");
        assert_eq!(app.doc.mirror.name(note), "Not a group", "any node takes a rename");

        assert!(app.debug_set_note_text(note, "mine".into()));
        assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("mine".into()), "the blur saved it");
        assert!(!app.nodes[&note].as_note().unwrap().editing);
        // A remote text held while typed in loses to what is typed.
        let mut other = crate::store::journal::Journal::open(&path, ActorId(2)).unwrap();
        app.nodes.get_mut(&note).unwrap().as_note_mut().unwrap().editing = true;
        remote_set(&mut other, note, field::TEXT, FieldValue::Str("theirs".into()));
        assert!(app.save_session());
        assert!(app.doc.writes.held_text.contains_key(&note), "held");
        assert!(app.debug_set_note_text(note, "mine again".into()));
        assert_eq!(stored(&app, note, field::TEXT), FieldValue::Str("mine again".into()));
        assert_eq!(app.doc.mirror.text(note), "mine again");
        assert!(!app.debug_set_note_text(g, "x".into()), "not a note");

        assert!(app.debug_move_node(note, egui::vec2(12.0, -34.0)));
        assert_eq!(stored(&app, note, field::POS), FieldValue::Pos(12.0, -34.0));
        assert_eq!(app.layout_pos(note), egui::vec2(12.0, -34.0), "the card is where it was dropped");
        assert!(!app.debug_move_node(node(9999), egui::vec2(1.0, 1.0)), "no such node");
    }

    #[test]
    fn name_text_and_pos_commands_parse() {
        let (g, n) = (node(3), node(1));
        assert_eq!(parse_rename(""), Some(None));
        assert_eq!(parse_rename(&format!("{g}  Reading list")), Some(Some((g, "Reading list".to_string()))));
        assert_eq!(parse_rename(&g.to_string()), None, "a name is required with an id");
        assert_eq!(parse_rename("g1 Reading list"), None, "a word that is no node id is a usage error");
        assert!(parses("RENAMEGROUP") && parses(&format!("RENAMEGROUP {g} x")) && !parses(&format!("RENAMEGROUP {g}")));
        assert_eq!(split_id(&format!("{n} two  words")), Some((n, "two  words")));
        assert_eq!(split_id(&n.to_string()), Some((n, "")));
        assert_eq!(split_id("n1 two  words"), None);
        assert!(parses(&format!("SETNOTETEXT {n} two  words")) && parses(&format!("SETNOTETEXT {n}")));
        assert!(!parses("SETNOTETEXT"), "an id is required");
        assert!(
            !parses("SETNOTETEXT n1 text") && !parses("CLOSENODE n1"),
            "a word that is no node id is a usage error"
        );
        assert_eq!(parse_move_node(&format!("{n} 12 -34.5")), Some((n, 12.0, -34.5)));
        for bad in ["12", "inf 0", "NaN 0", "1 2 3"] {
            assert!(parse_move_node(&format!("{n} {bad}")).is_none(), "{bad}");
        }
        assert!(parse_move_node("").is_none() && parse_move_node("n1 1 2").is_none());
        assert_eq!(parse_zoom_pct("62.5"), Some(62.5));
        for bad in ["", "0", "-5", "inf", "x"] {
            assert!(parse_zoom_pct(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn jar_commands_parse() {
        for ok in ["SETJAR Work", "SETJAR default", "SWITCHJARNOW", "CLEARJAR Work", "NEWJAR Work Stuff"] {
            assert!(parses(ok), "{ok}");
        }
        let (tab, jar) = (node(2), node(5));
        for bad in ["SETJAR", "CLEARJAR", "NEWJAR", "USEJAR 1-2", "JAREPHEMERAL 1-5 maybe", "JAREPHEMERAL 1-5"] {
            assert!(!parses(bad), "{bad}: a jar name or both words are required");
        }
        assert!(!parses(&format!("JAREPHEMERAL {jar} maybe")) && !parses(&format!("JAREPHEMERAL {jar}")));
        assert_eq!(parse_use_jar(&format!("{tab} {jar}")), Some((tab, Some(jar))));
        assert_eq!(parse_use_jar(&format!("{tab} none")), Some((tab, None)));
        assert_eq!(parse_use_jar(&tab.to_string()), None);
        assert_eq!(parse_use_jar(&format!("{tab} Work")), None, "a jar word that is no node id is a usage error");
        assert!(parses(&format!("JAREPHEMERAL {jar} on")) && parses(&format!("JAREPHEMERAL {jar} off")));
    }

    #[test]
    fn parses_json_level_commands_with_an_optional_pointer() {
        for ok in ["JSONSELECT /users/0", "JSONTOGGLE", "JSONSEARCH two words", "JSONRELAYOUT"] {
            assert!(parses(ok), "{ok}");
        }
    }

    #[test]
    fn parses_view_install_commands() {
        assert!(parses("INSTALLVIEW cef") && parses("VIEWSTATE CEF"));
        assert!(!parses("INSTALLVIEW nope"));
    }

    #[test]
    fn parses_addfile_with_an_optional_trailing_point() {
        assert_eq!(parse_add_file("C:\\a b\\c.png"), ("C:\\a b\\c.png".to_string(), None));
        assert_eq!(parse_add_file("C:\\a b\\c.png 10 20.5"), ("C:\\a b\\c.png".to_string(), Some((10.0, 20.5))));
        assert!(!parses("ADDFILE"));
    }
}
