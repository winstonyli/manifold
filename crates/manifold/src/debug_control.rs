use std::io::BufRead;

use ipc_protocol::KeyCode;
use winit::event::MouseButton;
use winit::event_loop::EventLoopProxy;

use crate::process_manager::EngineKind;
use crate::UserEvent;

/// A headless-mode automation command, one per stdin line (see `parse_line`
/// for the text grammar). Exists so live GUI testing on this project can
/// stop fighting Windows' foreground-lock/focus contention (see the M3/M5
/// gotchas in project memory: `SetForegroundWindow` can be silently refused
/// while the user is active on their own machine, making `SendKeys`/
/// `mouse_event`-based automation unreliable) — these commands drive the
/// exact same code paths real OS input would (see `main.rs`'s
/// `handle_debug_command`), just via a channel instead of the OS, so a
/// hidden/headless window never needs real focus at all.
///
/// Mouse commands are dispatched as genuine synthetic `winit::event::
/// WindowEvent`s (see `App::dispatch_window_event`) — deliberately not a
/// logic-bypassing shortcut — since that's the only way to actually exercise
/// egui's real hit-testing/rendering-timing, which is exactly the code path
/// that had the click-swallowing dropdown bug fixed in the autocomplete
/// feature. Keyboard/text commands are the one exception: `winit::event::
/// KeyEvent` has no public constructor outside the winit crate itself (its
/// `platform_specific` field is `pub(crate)`), so there is no way to
/// synthesize a real `WindowEvent::KeyboardInput` from application code at
/// all. Those commands instead call the exact equivalent effect directly
/// (send the same `InputEvent`s to a tab, or run the same address-bar
/// update/navigate logic) — this still exercises all of *this project's*
/// logic, just not winit/egui's own keyboard event plumbing (which was never
/// the source of a bug here).
#[derive(Debug, Clone)]
pub enum DebugCommand {
    MouseMove { x: f64, y: f64 },
    MouseDown { x: f64, y: f64, button: MouseButton },
    MouseUp { x: f64, y: f64, button: MouseButton },
    /// A full click (move + press + release) in one command — the common
    /// case, and avoids needing three round-trips through stdin for it.
    Click { x: f64, y: f64, button: MouseButton },
    Scroll { x: f64, y: f64, delta_x: f64, delta_y: f64 },
    /// Sets the address bar's text and focus state directly, then recomputes
    /// suggestions exactly as `response.changed()` would — see this module's
    /// doc comment for why address-bar *text entry* is a direct call rather
    /// than a synthetic `KeyboardInput` event.
    AddressBar(String),
    /// Simulates pressing Enter in the address bar (navigates using its
    /// current text), regardless of whether it has "real" egui focus —
    /// headless address-bar focus is set directly by `AddressBar`, not
    /// through a real click-driven egui focus transfer.
    Enter,
    /// Simulates pressing Escape — dismisses the suggestion dropdown if one
    /// is showing, otherwise leaves Tab view for Graph view (same behavior
    /// as a real Escape keypress).
    Escape,
    /// Simulates pressing Down/Up in the address bar — moves the
    /// highlighted suggestion row (wrapping at both ends) the same way a
    /// real arrow-key press would; `Enter` afterward accepts whichever row
    /// ends up highlighted.
    ArrowDown,
    ArrowUp,
    /// Graph-view keyboard navigation equivalents -- see
    /// `App::move_graph_focus`/`open_focused_graph_node`. Separate from
    /// `ArrowDown`/`ArrowUp` above (those are address-bar-dropdown-specific)
    /// since Graph view's arrow keys move in all four directions and do a
    /// different thing entirely.
    GraphFocusLeft,
    GraphFocusRight,
    GraphFocusUp,
    GraphFocusDown,
    GraphOpenFocused,
    /// Sends each character as an `InputEvent::Char` to the focused tab's
    /// page — for typing into a page's own text field, not the address bar.
    TypeIntoPage(String),
    /// Sends a KeyDown+KeyUp pair of the named key to the focused tab's page.
    KeyPage(KeyCode),
    /// `Ctrl+Shift+T` equivalent — see `App::reopen_closed_tab_shortcut`.
    ReopenTab,
    /// `Ctrl+=`/`Ctrl++` equivalent — see `App::zoom_in_shortcut`.
    ZoomIn,
    /// `Ctrl+-` equivalent — see `App::zoom_out_shortcut`.
    ZoomOut,
    /// `Ctrl+0` equivalent — see `App::zoom_reset_shortcut`.
    ZoomReset,
    /// `F12` equivalent — see `App::open_devtools_shortcut`.
    OpenDevTools,
    /// `Ctrl+T` equivalent — see `App::new_tab_shortcut`. A real
    /// `WindowEvent::KeyboardInput` can't be synthesized from application
    /// code (see this module's own doc comment), so this and the four
    /// shortcut commands below call the exact same effect directly instead,
    /// same as `TypeIntoPage`/`KeyPage` already do for page-level input.
    NewTab,
    /// `Ctrl+W` equivalent — see `App::close_focused_tab_shortcut`.
    CloseTab,
    /// `Ctrl+L` equivalent — see `App::focus_address_bar_shortcut`.
    FocusAddressBar,
    /// `Ctrl+Tab` equivalent — see `App::cycle_focused_tab`.
    NextTab,
    /// `Ctrl+Shift+Tab` equivalent — see `App::cycle_focused_tab`.
    PrevTab,
    /// `Ctrl+F` equivalent — see `App::open_find_bar_shortcut`.
    OpenFindBar,
    /// Sets the find bar's search text directly and sends `FindStart` to
    /// the focused tab — same "direct call, not a synthetic key event"
    /// reason as `AddressBar`.
    FindText(String),
    /// Sets the find bar's case-sensitivity toggle directly, re-sending
    /// `FindStart` if a search is already in progress — headless equivalent
    /// of clicking the bar's "Aa" toggle.
    FindMatchCase(bool),
    /// `Enter`/`Shift+Enter` in the find bar.
    FindNext,
    FindPrevious,
    /// `Escape` (or the find bar's own close button) — sends `FindStop` and
    /// closes the bar.
    CloseFindBar,
    /// Sends `HostToEngine::SetVisible` to the focused tab directly —
    /// bypassing `focus_tab`/`leave_to_graph`'s own visibility changes,
    /// which always come bundled with a `Resize` (Tab vs. Graph view's
    /// chrome heights differ). Exists for testing the `SetVisible(false)`→
    /// `true` repaint path in isolation, without a same-transaction resize
    /// able to mask a bug in it (see the M5 `SetVisible`-resume fix's own
    /// verification-gap note in project memory/README: a real user's
    /// Escape-then-refocus always resizes too, so a clean repro needs a
    /// way to toggle visibility *without* one).
    SetVisible(bool),
    /// Renders the next frame and saves it as a PNG at this path — includes
    /// chrome and any open dropdown, not just tab content, since it reads
    /// back the same composited frame a real screenshot would show.
    Screenshot(String),
    Resize(u32, u32),
    Quit,
    /// `Ctrl+K` equivalent — see `App::handle_global_shortcuts`.
    QuickSwitchOpen,
    /// Sets the quick-switcher's search text directly, same "direct call,
    /// not a synthetic key event" reason as `AddressBar`/`FindText`.
    QuickSwitchText(String),
    /// Up/Down/Enter in the quick-switcher — see `App::quick_switcher_
    /// move`/`quick_switcher_confirm`.
    QuickSwitchDown,
    QuickSwitchUp,
    QuickSwitchEnter,
    QuickSwitchEscape,
    /// Sets the settings popup's "Save Session As..." name field directly,
    /// same "direct call, not a synthetic key event" reason as
    /// `AddressBar`/`FindText`.
    SessionNameText(String),
    /// Sets the Graph view toolbar's filter box directly, same "direct
    /// call, not a synthetic key event" reason as `AddressBar`/`FindText`.
    GraphFilterText(String),
    /// `Ctrl+G` equivalent — see `App::toggle_view_mode_shortcut`.
    ToggleView,
    /// Settings gear (Tab or Graph toolbar) — see `App::toggle_settings_page`.
    /// Documented as a headless command (`App::open_settings_page`'s own
    /// doc comment already claimed this) since before it actually existed
    /// here -- added for real once that gap got in the way of testing the
    /// Settings toolbar directly.
    ToggleSettingsPage,
    /// "Export..." with an explicit path instead of a real file dialog —
    /// headless mode has no OS/human to answer one, so this bypasses it
    /// entirely and calls the same underlying save directly.
    ExportSession(String),
    /// "Import..." with an explicit path, same "bypass the real file
    /// dialog" reason as `ExportSession`.
    ImportSession(String),
    /// "Print to PDF" with an explicit path instead of a real file dialog —
    /// same "bypass the dialog entirely" reason as `ExportSession`.
    PrintToPdf(String),
    /// `Ctrl+P` equivalent — see `App::pin_focused_tab_shortcut`.
    TogglePin,
    /// `Ctrl+R`/`F5` equivalent — see `App::reload_focused_tab_shortcut`.
    Reload,
    /// `Ctrl+Shift+G` equivalent — see `App::new_group_shortcut`.
    NewGroup,
    /// The Graph toolbar's "+ Note" button — see `App::new_note_shortcut`.
    NewNote,
    /// The Graph toolbar's "+ Image" button with an explicit path instead
    /// of a real file dialog — same "bypass the dialog entirely" reason as
    /// `ExportSession`/`PrintToPdf`. See `App::insert_image_node`.
    AddImage(String),
    /// The Graph toolbar's "+ PDF" button with an explicit path instead of
    /// a real file dialog — same reasoning as `AddImage`. See
    /// `App::insert_pdf_node`.
    AddPdf(String),
    /// The Graph toolbar's "+ JSON" button with an explicit path instead of
    /// a real file dialog — same reasoning as `AddImage`. See
    /// `App::insert_json_node`.
    AddJson(String),
    /// `F1` equivalent — toggles the keyboard-shortcut reference popup.
    ToggleShortcutsHelp,
    /// Sets the History popup's own filter box directly, same "direct call,
    /// not a synthetic key event" reason as `GraphFilterText`.
    HistoryFilterText(String),
    /// Sets the Bookmarks popup's own filter box directly, same reason as
    /// `HistoryFilterText`.
    BookmarksFilterText(String),
    /// "Export Bookmarks..." with an explicit path instead of a real file
    /// dialog, same "bypass the dialog entirely" reason as `ExportSession`.
    ExportBookmarks(String),
    /// "Import Bookmarks..." with an explicit path, same reason as
    /// `ExportBookmarks`.
    ImportBookmarks(String),
    /// `Ctrl+Shift+W` equivalent — see `App::close_all_at_current_level`.
    CloseAllLevel,
    /// Clicks the "Import Session..." confirm popup's own Import button —
    /// see `App::pending_import`.
    ConfirmImport,
    /// Clicks the confirm popup's Cancel button, same reason as
    /// `ConfirmImport`.
    CancelImport,
    /// `F2` equivalent — see `App::rename_current_group_shortcut`.
    RenameGroup,
    /// Sets the bookmarks popup's own folder-rename draft directly, same
    /// "direct call, not a synthetic key event" reason as `AddressBar`/
    /// `GraphFilterText` — a no-op if nothing is currently being renamed.
    BookmarkFolderText(String),
    /// Graph view's `Escape`-dismisses-peek equivalent — see
    /// `App::dismiss_hover_peek`. Separate from plain `Escape` since that
    /// command is Tab-view-only (see its own doc comment).
    DismissPeek,
    /// "Export Group..." with an explicit path instead of a real file
    /// dialog, same "bypass the dialog entirely" reason as
    /// `ExportSession` — exports only when the open node menu belongs to a
    /// Group (see `App::node_menu`).
    ExportGroup(String),
    /// "Import Group..." with an explicit path, same reason as
    /// `ExportGroup`.
    ImportGroup(String),
    /// `Ctrl+D` equivalent — see `App::duplicate_focused_tab_shortcut`.
    DuplicateTab,
    /// The Tab-view toolbar's "Split" picker with an explicit engine
    /// instead of a real click-through dropdown — see `App::start_split`.
    Split(EngineKind),
    /// The Tab-view toolbar's "Close Split" button — see `App::close_split`.
    CloseSplit,
    /// The JSON node context menu's "Expand as Graph" button, targeting an
    /// explicit node id instead of `self.focused` (a plain `ADDJSON` doesn't
    /// focus the node it creates, so a headless test can't rely on focus
    /// here) — see `App::expand_json_as_graph`.
    ExpandJsonAsGraph(String),
    /// A group card's own "Close" context-menu action, targeting an
    /// explicit group id instead of `self.focused` -- same "deterministic
    /// headless test" reasoning as `ExpandJsonAsGraph`. No-ops if `id`
    /// isn't actually a group. See `App::close_group`.
    CloseGroup(String),
    /// Dev-only: re-runs the one-time first-launch demo graph on demand
    /// (see `Settings::has_shown_first_launch_demo`'s own doc comment for
    /// why it otherwise only ever fires once) -- adds a fresh copy of the
    /// demo layout to whatever's currently open rather than clearing
    /// anything first, same "acts on current state, never auto-resets"
    /// posture every other debug command already has. Never touches the
    /// `has_shown_first_launch_demo` flag itself -- purely a look-at-it
    /// aid, not a way to simulate real first-launch persistence. See
    /// `App::spawn_first_launch_demo`.
    TriggerFirstLaunchDemo,
    /// `HELP` (or `?`) -- prints `COMMAND_HELP` to stderr. See that table's
    /// own doc comment for why this exists and its one maintenance cost.
    Help,
}

/// The full command reference `HELP`/`?` prints, and the single source of
/// truth for it -- `(usage, one-line description)` pairs, one per
/// `parse_line` match arm below, in the same order. There is no way to
/// generate this from the match arms themselves (Rust has no runtime
/// reflection over a `match`'s own string patterns), so this is a second,
/// hand-maintained list that can drift from `parse_line` if one is updated
/// without the other -- accepted as the cheapest way to give this protocol
/// any discoverability at all, given ~70 commands and no existing registry
/// to hang help text off of. **Adding, renaming, or removing a command?
/// Update this table in the same commit.**
const COMMAND_HELP: &[(&str, &str)] = &[
    ("MOUSEMOVE x y", "Move the mouse to (x, y)"),
    ("MOUSEDOWN x y [button]", "Press a mouse button at (x, y) -- button defaults to left"),
    ("MOUSEUP x y [button]", "Release a mouse button at (x, y)"),
    ("CLICK x y [button]", "Full click (move+press+release) at (x, y)"),
    ("SCROLL x y dx dy", "Scroll at (x, y) by (dx, dy)"),
    ("ADDRESSBAR text", "Set the address bar's text and focus"),
    ("ENTER", "Accept the address bar / confirm a dropdown row"),
    ("ESCAPE", "Dismiss a suggestion dropdown, or leave Tab view for Graph view"),
    ("ARROWDOWN", "Move the address-bar suggestion highlight down"),
    ("ARROWUP", "Move the address-bar suggestion highlight up"),
    ("GRAPHLEFT", "Graph view: move focus left"),
    ("GRAPHRIGHT", "Graph view: move focus right"),
    ("GRAPHUP", "Graph view: move focus up"),
    ("GRAPHDOWN", "Graph view: move focus down"),
    ("GRAPHOPEN", "Open whichever Graph-view node is currently focused"),
    ("TYPE text", "Send each character to the focused tab's own page (not the address bar)"),
    ("KEYPAGE key", "Send a KeyDown+KeyUp of `key` to the focused tab's own page"),
    ("REOPENTAB", "Ctrl+Shift+T -- reopen the last closed tab"),
    ("ZOOMIN", "Ctrl+=/Ctrl++ -- zoom in"),
    ("ZOOMOUT", "Ctrl+- -- zoom out"),
    ("ZOOMRESET", "Ctrl+0 -- reset zoom"),
    ("OPENDEVTOOLS", "F12 -- open devtools"),
    ("NEWTAB", "Ctrl+T -- new tab"),
    ("CLOSETAB", "Ctrl+W -- close the focused tab"),
    ("FOCUSADDRESS", "Ctrl+L -- focus the address bar"),
    ("NEXTTAB", "Ctrl+Tab -- cycle to the next tab"),
    ("PREVTAB", "Ctrl+Shift+Tab -- cycle to the previous tab"),
    ("OPENFINDBAR", "Ctrl+F -- open find-in-page"),
    ("FINDTEXT text", "Set the find bar's search text and search"),
    ("FINDMATCHCASE true|false", "Toggle the find bar's case-sensitivity"),
    ("FINDNEXT", "Find bar: next match"),
    ("FINDPREVIOUS", "Find bar: previous match"),
    ("CLOSEFINDBAR", "Close the find bar"),
    ("SETVISIBLE true|false", "Send SetVisible directly to the focused tab (bypasses the usual resize-bundled visibility change)"),
    ("SCREENSHOT path", "Render the next frame and save it as a PNG at `path`"),
    ("RESIZE w h", "Resize the window to (w, h)"),
    ("QUIT", "Exit the app"),
    ("QUICKSWITCHOPEN", "Ctrl+K -- open the quick switcher"),
    ("QUICKSWITCHTEXT text", "Set the quick switcher's search text"),
    ("QUICKSWITCHDOWN", "Quick switcher: move highlight down"),
    ("QUICKSWITCHUP", "Quick switcher: move highlight up"),
    ("QUICKSWITCHENTER", "Quick switcher: confirm the highlighted row"),
    ("QUICKSWITCHESCAPE", "Close the quick switcher"),
    ("SESSIONNAMETEXT text", "Set the \"Save Session As...\" name field"),
    ("GRAPHFILTERTEXT text", "Set the Graph toolbar's own filter box"),
    ("HISTORYFILTERTEXT text", "Set the History popup's own filter box"),
    ("BOOKMARKSFILTERTEXT text", "Set the Bookmarks popup's own filter box"),
    ("TOGGLEVIEW", "Ctrl+G -- toggle Tab / Graph view"),
    ("TOGGLESETTINGSPAGE", "Settings gear -- enter/leave the Settings page"),
    ("EXPORTSESSION path", "Save the current session to `path`"),
    ("IMPORTSESSION path", "Load a session from `path`"),
    ("EXPORTBOOKMARKS path", "Save bookmarks to `path`"),
    ("IMPORTBOOKMARKS path", "Load bookmarks from `path`"),
    ("CLOSEALLLEVEL", "Ctrl+Shift+W -- close every tab at the current level"),
    ("CONFIRMIMPORT", "Click the \"Import Session...\" confirm popup's Import button"),
    ("CANCELIMPORT", "Click the confirm popup's Cancel button"),
    ("RENAMEGROUP", "F2 -- rename the current group"),
    ("BOOKMARKFOLDERTEXT text", "Set the bookmarks popup's own folder-rename draft"),
    ("DISMISSPEEK", "Escape-dismisses-peek equivalent in Graph view"),
    ("EXPORTGROUP path", "Export whichever group's context menu is currently open"),
    ("IMPORTGROUP path", "Import a group from `path`"),
    ("DUPLICATETAB", "Ctrl+D -- duplicate the focused tab"),
    ("PRINTTOPDF path", "Print the focused tab to a PDF at `path`"),
    ("TOGGLEPIN", "Ctrl+P -- pin/unpin the focused tab"),
    ("RELOAD", "Ctrl+R/F5 -- reload the focused tab"),
    ("NEWGROUP", "Ctrl+Shift+G -- new group"),
    ("NEWNOTE", "Graph toolbar's \"+ Note\" button"),
    ("ADDIMAGE path", "Add an image node from `path`"),
    ("ADDPDF path", "Add a PDF node from `path`"),
    ("ADDJSON path", "Add a JSON node from `path`"),
    ("TOGGLESHORTCUTSHELP", "F1 -- toggle the keyboard-shortcut reference popup"),
    ("SPLIT engine", "Open a Split pane running `engine` (servo|cef|webkit|webview2)"),
    ("CLOSESPLIT", "Close the Split pane"),
    ("EXPANDJSONASGRAPH id", "\"Expand as Graph\" on the JSON node `id`"),
    ("CLOSEGROUP id", "Close the group `id`, same as its own \"Close\" context-menu action"),
    ("TRIGGERFIRSTLAUNCHDEMO", "Dev-only: re-spawn the first-launch demo graph on demand"),
    ("HELP", "Print this command list"),
];

/// Case-insensitive, matching `engine_kind_serde`'s own lowercase
/// convention (`settings.json`/`session.json` use the same strings) --
/// `SPLIT`'s one argument.
fn parse_engine_kind(name: &str) -> Option<EngineKind> {
    match name.to_ascii_lowercase().as_str() {
        "servo" => Some(EngineKind::Servo),
        "cef" => Some(EngineKind::Cef),
        "webkit" => Some(EngineKind::Webkit),
        "webview2" => Some(EngineKind::Webview2),
        _ => None,
    }
}

fn parse_mouse_button(name: &str) -> MouseButton {
    match name.to_ascii_lowercase().as_str() {
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::Left,
    }
}

/// Maps a debug command's key name to `ipc_protocol::KeyCode` — the same
/// engine-agnostic key set every real engine-host translation table already
/// covers (see that type's own doc comment), not winit's own `KeyCode`.
fn parse_key_code(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Enter" => KeyCode::Enter,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        "Space" => KeyCode::Space,
        "Escape" => KeyCode::Escape,
        "Delete" => KeyCode::Delete,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "ControlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        _ => return parse_single_char_key(name),
    })
}

/// `KeyA`..`KeyZ` and `Digit0`..`Digit9` follow a regular naming scheme, so
/// generate the match from the single character instead of spelling out 36
/// arms — also accepts a bare letter/digit (`"a"`, `"5"`) for convenience.
fn parse_single_char_key(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match ch.to_ascii_uppercase() {
        c @ 'A'..='Z' => Some(match c {
            'A' => KeyCode::KeyA, 'B' => KeyCode::KeyB, 'C' => KeyCode::KeyC, 'D' => KeyCode::KeyD,
            'E' => KeyCode::KeyE, 'F' => KeyCode::KeyF, 'G' => KeyCode::KeyG, 'H' => KeyCode::KeyH,
            'I' => KeyCode::KeyI, 'J' => KeyCode::KeyJ, 'K' => KeyCode::KeyK, 'L' => KeyCode::KeyL,
            'M' => KeyCode::KeyM, 'N' => KeyCode::KeyN, 'O' => KeyCode::KeyO, 'P' => KeyCode::KeyP,
            'Q' => KeyCode::KeyQ, 'R' => KeyCode::KeyR, 'S' => KeyCode::KeyS, 'T' => KeyCode::KeyT,
            'U' => KeyCode::KeyU, 'V' => KeyCode::KeyV, 'W' => KeyCode::KeyW, 'X' => KeyCode::KeyX,
            'Y' => KeyCode::KeyY, 'Z' => KeyCode::KeyZ,
            _ => unreachable!(),
        }),
        c @ '0'..='9' => Some(match c {
            '0' => KeyCode::Digit0, '1' => KeyCode::Digit1, '2' => KeyCode::Digit2, '3' => KeyCode::Digit3,
            '4' => KeyCode::Digit4, '5' => KeyCode::Digit5, '6' => KeyCode::Digit6, '7' => KeyCode::Digit7,
            '8' => KeyCode::Digit8, '9' => KeyCode::Digit9,
            _ => unreachable!(),
        }),
        _ => None,
    }
}

/// Parses one line of the headless debug-command text protocol. Unknown
/// commands or malformed arguments are silently ignored (returns `None`) —
/// good enough for a testing tool where a bad line should be a no-op, not a
/// crash.
pub fn parse_line(line: &str) -> Option<DebugCommand> {
    // Strip a leading BOM — several Windows-side tools that can drive this
    // protocol (PowerShell's default `StandardInput` stream encoding among
    // them, confirmed live: a `Process.StandardInput.WriteLine` from
    // PowerShell prepends `\u{FEFF}` to the very first line) write one
    // unconditionally, and it isn't whitespace so `trim()` alone leaves it
    // stuck to the command name.
    let line = line.strip_prefix('\u{feff}').unwrap_or(line);
    let line = line.trim();
    let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim();
    let mut parts = rest.split_whitespace();
    match cmd.to_ascii_uppercase().as_str() {
        "MOUSEMOVE" => Some(DebugCommand::MouseMove {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
        }),
        "MOUSEDOWN" => Some(DebugCommand::MouseDown {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            button: parts.next().map(parse_mouse_button).unwrap_or(MouseButton::Left),
        }),
        "MOUSEUP" => Some(DebugCommand::MouseUp {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            button: parts.next().map(parse_mouse_button).unwrap_or(MouseButton::Left),
        }),
        "CLICK" => Some(DebugCommand::Click {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            button: parts.next().map(parse_mouse_button).unwrap_or(MouseButton::Left),
        }),
        "SCROLL" => Some(DebugCommand::Scroll {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            delta_x: parts.next()?.parse().ok()?,
            delta_y: parts.next()?.parse().ok()?,
        }),
        "ADDRESSBAR" => Some(DebugCommand::AddressBar(rest.to_string())),
        "ENTER" => Some(DebugCommand::Enter),
        "ESCAPE" => Some(DebugCommand::Escape),
        "ARROWDOWN" => Some(DebugCommand::ArrowDown),
        "ARROWUP" => Some(DebugCommand::ArrowUp),
        "GRAPHLEFT" => Some(DebugCommand::GraphFocusLeft),
        "GRAPHRIGHT" => Some(DebugCommand::GraphFocusRight),
        "GRAPHUP" => Some(DebugCommand::GraphFocusUp),
        "GRAPHDOWN" => Some(DebugCommand::GraphFocusDown),
        "GRAPHOPEN" => Some(DebugCommand::GraphOpenFocused),
        "TYPE" => Some(DebugCommand::TypeIntoPage(rest.to_string())),
        "KEYPAGE" => Some(DebugCommand::KeyPage(parse_key_code(parts.next()?)?)),
        "REOPENTAB" => Some(DebugCommand::ReopenTab),
        "ZOOMIN" => Some(DebugCommand::ZoomIn),
        "ZOOMOUT" => Some(DebugCommand::ZoomOut),
        "ZOOMRESET" => Some(DebugCommand::ZoomReset),
        "OPENDEVTOOLS" => Some(DebugCommand::OpenDevTools),
        "NEWTAB" => Some(DebugCommand::NewTab),
        "CLOSETAB" => Some(DebugCommand::CloseTab),
        "FOCUSADDRESS" => Some(DebugCommand::FocusAddressBar),
        "NEXTTAB" => Some(DebugCommand::NextTab),
        "PREVTAB" => Some(DebugCommand::PrevTab),
        "OPENFINDBAR" => Some(DebugCommand::OpenFindBar),
        "FINDTEXT" => Some(DebugCommand::FindText(rest.to_string())),
        "FINDMATCHCASE" => Some(DebugCommand::FindMatchCase(match parts.next()? {
            "true" => true,
            "false" => false,
            _ => return None,
        })),
        "FINDNEXT" => Some(DebugCommand::FindNext),
        "FINDPREVIOUS" => Some(DebugCommand::FindPrevious),
        "CLOSEFINDBAR" => Some(DebugCommand::CloseFindBar),
        "SETVISIBLE" => Some(DebugCommand::SetVisible(match parts.next()? {
            "true" => true,
            "false" => false,
            _ => return None,
        })),
        "SCREENSHOT" => Some(DebugCommand::Screenshot(rest.to_string())).filter(|_| !rest.is_empty()),
        "RESIZE" => Some(DebugCommand::Resize(parts.next()?.parse().ok()?, parts.next()?.parse().ok()?)),
        "QUIT" => Some(DebugCommand::Quit),
        "QUICKSWITCHOPEN" => Some(DebugCommand::QuickSwitchOpen),
        "QUICKSWITCHTEXT" => Some(DebugCommand::QuickSwitchText(rest.to_string())),
        "QUICKSWITCHDOWN" => Some(DebugCommand::QuickSwitchDown),
        "QUICKSWITCHUP" => Some(DebugCommand::QuickSwitchUp),
        "QUICKSWITCHENTER" => Some(DebugCommand::QuickSwitchEnter),
        "QUICKSWITCHESCAPE" => Some(DebugCommand::QuickSwitchEscape),
        "SESSIONNAMETEXT" => Some(DebugCommand::SessionNameText(rest.to_string())),
        "GRAPHFILTERTEXT" => Some(DebugCommand::GraphFilterText(rest.to_string())),
        "HISTORYFILTERTEXT" => Some(DebugCommand::HistoryFilterText(rest.to_string())),
        "BOOKMARKSFILTERTEXT" => Some(DebugCommand::BookmarksFilterText(rest.to_string())),
        "TOGGLEVIEW" => Some(DebugCommand::ToggleView),
        "TOGGLESETTINGSPAGE" => Some(DebugCommand::ToggleSettingsPage),
        "EXPORTSESSION" => Some(DebugCommand::ExportSession(rest.to_string())).filter(|_| !rest.is_empty()),
        "IMPORTSESSION" => Some(DebugCommand::ImportSession(rest.to_string())).filter(|_| !rest.is_empty()),
        "EXPORTBOOKMARKS" => Some(DebugCommand::ExportBookmarks(rest.to_string())).filter(|_| !rest.is_empty()),
        "IMPORTBOOKMARKS" => Some(DebugCommand::ImportBookmarks(rest.to_string())).filter(|_| !rest.is_empty()),
        "CLOSEALLLEVEL" => Some(DebugCommand::CloseAllLevel),
        "CONFIRMIMPORT" => Some(DebugCommand::ConfirmImport),
        "CANCELIMPORT" => Some(DebugCommand::CancelImport),
        "RENAMEGROUP" => Some(DebugCommand::RenameGroup),
        "BOOKMARKFOLDERTEXT" => Some(DebugCommand::BookmarkFolderText(rest.to_string())),
        "DISMISSPEEK" => Some(DebugCommand::DismissPeek),
        "EXPORTGROUP" => Some(DebugCommand::ExportGroup(rest.to_string())).filter(|_| !rest.is_empty()),
        "IMPORTGROUP" => Some(DebugCommand::ImportGroup(rest.to_string())).filter(|_| !rest.is_empty()),
        "DUPLICATETAB" => Some(DebugCommand::DuplicateTab),
        "PRINTTOPDF" => Some(DebugCommand::PrintToPdf(rest.to_string())).filter(|_| !rest.is_empty()),
        "TOGGLEPIN" => Some(DebugCommand::TogglePin),
        "RELOAD" => Some(DebugCommand::Reload),
        "NEWGROUP" => Some(DebugCommand::NewGroup),
        "NEWNOTE" => Some(DebugCommand::NewNote),
        "ADDIMAGE" => Some(DebugCommand::AddImage(rest.to_string())).filter(|_| !rest.is_empty()),
        "ADDPDF" => Some(DebugCommand::AddPdf(rest.to_string())).filter(|_| !rest.is_empty()),
        "ADDJSON" => Some(DebugCommand::AddJson(rest.to_string())).filter(|_| !rest.is_empty()),
        "TOGGLESHORTCUTSHELP" => Some(DebugCommand::ToggleShortcutsHelp),
        "SPLIT" => parse_engine_kind(rest).map(DebugCommand::Split),
        "CLOSESPLIT" => Some(DebugCommand::CloseSplit),
        "EXPANDJSONASGRAPH" => {
            Some(DebugCommand::ExpandJsonAsGraph(rest.to_string())).filter(|_| !rest.is_empty())
        }
        "CLOSEGROUP" => Some(DebugCommand::CloseGroup(rest.to_string())).filter(|_| !rest.is_empty()),
        "TRIGGERFIRSTLAUNCHDEMO" => Some(DebugCommand::TriggerFirstLaunchDemo),
        "HELP" | "?" => Some(DebugCommand::Help),
        _ => None,
    }
}

/// Prints `COMMAND_HELP` to stderr -- `eprintln!`, not `println!`, matching
/// this protocol's own established convention of surfacing diagnostics on
/// stderr (see e.g. the crash reporter) while stdout stays free for
/// anything a driving script might want to parse on its own.
pub fn print_help() {
    eprintln!("Manifold debug commands:");
    for (usage, description) in COMMAND_HELP {
        eprintln!("  {usage:<32} {description}");
    }
}

/// Spawns the stdin-reading thread for headless mode — one command per
/// line, forwarded to the app via `proxy` (the same `EventLoopProxy`
/// mechanism winit itself uses for user events, so commands land on the
/// real event loop like any other event). Stdin EOF (the driving script/
/// process closed its write end) sends `Quit` so a headless run driven by a
/// piped script exits cleanly instead of hanging forever.
pub fn spawn_stdin_reader(proxy: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            match parse_line(&line) {
                Some(cmd) => {
                    if proxy.send_event(UserEvent::Debug(cmd)).is_err() {
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
        let _ = proxy.send_event(UserEvent::Debug(DebugCommand::Quit));
    });
}
