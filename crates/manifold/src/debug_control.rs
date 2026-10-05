use std::io::BufRead;

use ipc_protocol::KeyCode;
use winit::event::MouseButton;
use winit::event_loop::EventLoopProxy;

use super::*;
use crate::nav_history::Dir;
use crate::process_manager::EngineKind;
use crate::sleep_state::SleepWhy;
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
/// feature. Keyboard commands can't go through winit the same way:
/// `winit::event::KeyEvent` has no public constructor outside the winit
/// crate (its `platform_specific` field is `pub(crate)`). The address-bar
/// keys (`ADDRESSBAR`, `ENTER`, `ESCAPE`, `ARROWDOWN`, `ARROWUP`), and
/// `EGUITEXT`/`BACKSPACE` for any focused egui field, are
/// instead injected one level down, as `egui::Event::Key`/`Text` in the
/// next frame's `RawInput`, so the UI's own `key_pressed` checks and the
/// real `TextEdit` see them. Headless mode also marks the window as focused
/// for egui, since egui reports no widget focus in an unfocused window. The
/// remaining keyboard commands (shortcuts, page input) call the exact
/// equivalent effect directly (send the same `InputEvent`s to a tab, or run
/// the same shortcut method).
///
/// Driving egui through these, learned the hard way: use `CLICK`, not a
/// `MOUSEDOWN`/`MOUSEUP` pair spaced by a `sleep` -- egui drops a press
/// held past its max click duration (~0.8 s) as a long press, not a click.
/// A drag needs several `MOUSEMOVE`s between the down and the up, not one.
/// One `MOUSEMOVE` is enough for a `SCREENSHOT` to show hover (checked
/// headless on a tab card: ✕ and peek preview both up), and a right-click
/// needs none, since the graph finds the card under the press itself
/// rather than going by hover. Each command renders its own frame in headless mode, and a
/// pointer command also one before it, so it's hit-tested against the
/// current state (see `UserEvent::Debug`'s handler); mouse commands need
/// no `sleep` between them. Animations (menu fades, group drill-in) still take real
/// time. Open a right-click menu with `MOUSEDOWN`/`MOUSEUP ... right`, not
/// `CLICK ... right`: a menu's first frame is egui's sizing pass, which may
/// place it away from the pointer, and a release in that same frame counts
/// as a click outside it, which closes it. First-launch demo tabs spawn
/// asynchronously and can take 30 s or more headless; `WAITTABS` waits for
/// them, since until the focused tab has spawned, commands that need it
/// (e.g. `ADDDOWNLOAD`) do nothing.
#[derive(Debug, Clone)]
#[rustfmt::skip]
pub enum DebugCommand {
    MouseMove { x: f64, y: f64 },
    MouseDown { x: f64, y: f64, button: MouseButton },
    MouseUp { x: f64, y: f64, button: MouseButton },
    /// A full click (move + press + release) in one command — the common
    /// case, and avoids needing three round-trips through stdin for it.
    Click { x: f64, y: f64, button: MouseButton },
    /// A left double-click, both clicks in one frame. Two `CLICK`s only
    /// count as one when their frames fall within egui's 0.3 s
    /// double-click delay, which a slow headless frame can break.
    DoubleClick { x: f64, y: f64 },
    Scroll { x: f64, y: f64, delta_x: f64, delta_y: f64 },
    /// Focuses the address bar (Tab view only), then, once it has focus,
    /// types the text over its contents as egui `Ctrl+A` + `Text` events,
    /// so suggestions update through the real `response.changed()` path.
    AddressBar(String),
    /// An egui Enter key press: in the focused address bar, navigates using
    /// its text or the highlighted suggestion.
    Enter,
    /// An egui Escape key press: dismisses the suggestion dropdown if one
    /// is showing, otherwise leaves Tab view for Graph view.
    Escape,
    /// An egui Backspace key press: erases in the focused egui text field.
    Backspace,
    /// `EGUITEXT <text>` -- an egui `Text` event: types into whichever egui
    /// text field has focus (a jar's name in Settings, "New Jar..."'s field, a
    /// group rename), not the page (`TYPE`). Nothing focused: nothing typed.
    EguiText(String),
    /// egui Down/Up key presses: in the address bar, move the highlighted
    /// suggestion row (wrapping at both ends); `Enter` afterward accepts
    /// whichever row ends up highlighted.
    ArrowDown,
    ArrowUp,
    /// Real Right/Left key presses (a JSON level's outline open/close).
    ArrowRight,
    ArrowLeft,
    /// A real Space key press (a focused tab card toggles its selection).
    SpaceKey,
    DigitKey(u8),
    /// An egui Tab key press (Shift+Tab with `true`): moves egui's keyboard focus
    /// between widgets, for checking how focus looks and that it moves nothing.
    TabKey(bool),
    /// Shift+F10: opens the context menu of the keyboard-focused graph card.
    ShiftF10,
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
    /// `Ctrl+Shift+T` equivalent — see `App::reopen_closed_shortcut`.
    ReopenTab,
    /// Starts `App::start_engine_install` for the engine, as the picker's Download does.
    InstallEngine(EngineKind),
    /// Prints the engine's `EngineState` to stderr as `engine-state <name>: <label>`.
    EngineState(EngineKind),
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
    /// the focused tab — a direct call, not a synthetic key event (see this
    /// module's doc comment).
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
    /// Blocks the stdin reader until it's saved (or 30 s pass): frames can
    /// stall for seconds, and the app holds one pending screenshot, so an
    /// unblocked second `SCREENSHOT` (or a `QUIT`) used to drop the first.
    /// Handled by `spawn_stdin_reader`, which sends
    /// `NotifyWhenScreenshotTaken`; never reaches the app.
    Screenshot(String),
    /// Internal, sent only by `spawn_stdin_reader` for `SCREENSHOT`: the
    /// app saves the next frame to the path, then sends `()` on this.
    NotifyWhenScreenshotTaken(String, std::sync::mpsc::Sender<()>),
    Resize(u32, u32),
    Quit,
    /// `Ctrl+K` equivalent — see `App::handle_global_shortcuts`.
    QuickSwitchOpen,
    /// Sets the quick-switcher's search text directly, a direct call like
    /// `FindText`.
    QuickSwitchText(String),
    /// Up/Down/Enter in the quick-switcher — see `App::quick_switcher_
    /// move`/`quick_switcher_confirm`.
    QuickSwitchDown,
    QuickSwitchUp,
    QuickSwitchEnter,
    QuickSwitchEscape,
    /// Sets the settings popup's "Save Session As..." name field directly,
    /// same "direct call, not a synthetic key event" reason as
    /// `FindText`.
    SessionNameText(String),
    /// "Save Session As..." with an explicit name instead of reading the
    /// name field a real click would use, same "bypass the dialog/UI
    /// entirely" reason as `ExportSession`. See `App::save_named_session`.
    SaveSessionAs(String),
    /// A saved name's "Open" button with an explicit name, same reason as
    /// `SaveSessionAs`. See `App::load_named_session`.
    OpenSession(String),
    /// Sets the Graph view toolbar's filter box directly, same "direct
    /// call, not a synthetic key event" reason as `FindText`.
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
    /// `EXPORTSTATE path` -- writes UI state that the session store doesn't
    /// hold (and shouldn't: it's the user's saved session) as JSON, for a
    /// test to check: `tab_view`, `transition`, `focused_url`,
    /// `address_bar_text`, `suggestions` (the dropdown's rows: a search's
    /// query, or `tab:<id>` for an open tab), `highlighted` (a row
    /// index, or null) and `current_group` (the Graph view level's group
    /// id, or null at the top). `cards` lists the current level's cards in
    /// draw order, each as `id`, `kind` (`"Tab"`, `"Note"`, ...), `group`,
    /// `parent`, `color` (whether it has a tag color), `pinned`, `screen`
    /// (its center) and `rect` (`[left, top, right, bottom]`), in points,
    /// which are pixels headless. `selected_tabs` is the rectangle
    /// selection's ids, sorted. `toast` is the bottom-left toast's text
    /// (the one showing, else the next queued), or null. Suggestions arriving from the network show up
    /// only once a frame has drained them, and every command draws one, so
    /// a test can poll this until `suggestions` fills.
    ExportState(String),
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
    /// The toolbar's Back / Forward buttons, on the focused tab.
    GoBack,
    GoForward,
    /// `Ctrl+Shift+G` equivalent — see `App::new_group_shortcut`.
    NewGroup,
    /// The Graph toolbar's "+ Note" button — see `App::new_note_shortcut`.
    NewNote,
    /// The Graph toolbar's "+ File" button with an explicit path instead of
    /// a real file dialog — same "bypass the dialog entirely" reason as
    /// `ExportSession`/`PrintToPdf`. The point, if given, is a window point
    /// like a drop there; with none, the file is jittered around
    /// world center at the current level, the same as the picker. See
    /// `App::add_file_at`.
    AddFile(String, Option<(f32, f32)>),
    /// A download node for `path`, as if the focused tab had just started
    /// downloading it (no bytes, unknown total) -- downloads otherwise only
    /// come from a real engine download. Nothing is fetched or written.
    /// A no-op with no focused tab. See `App::insert_download_node`.
    AddDownload(String),
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
    /// "direct call, not a synthetic key event" reason as
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
    /// Select the entry at this JSON Pointer on the current JSON level
    /// (empty: the root).
    JsonSelect(String),
    /// Collapse or expand the card at this pointer.
    JsonToggle(String),
    /// Reveal the next page of the card at this pointer.
    JsonMore(String),
    /// Clear every saved card position (the bar's Relayout).
    JsonRelayout,
    /// Search the current JSON level.
    JsonSearch(String),
    /// A group card's own "Close" context-menu action, targeting an
    /// explicit group id instead of `self.focused`, since a headless test
    /// can't rely on focus. No-ops if `id` isn't actually a group. See
    /// `App::close_group`.
    CloseGroup(String),
    /// `SLEEPNODE id` -- a tab's (or a group's tabs') "Sleep" context-menu
    /// action, targeting an explicit id for the same reason as `CloseGroup`.
    /// See `App::request_sleep`.
    SleepNode(String),
    /// `IDLESWEEP secs` -- one idle auto-sleep sweep with a limit of `secs`
    /// seconds instead of the setting's minutes. See `App::sweep_idle_tabs`.
    IdleSweep(u64),
    /// `SETJAR <name|default>` -- the focused tab picks that jar (a new
    /// named jar if none has the name), as from its menu. `name` is a
    /// label ("Work · #1a2b") or a stored name; a stored name several jars
    /// share picks the lowest node id (`App::jar_named`). See
    /// `App::set_tab_jar`.
    SetJar(String),
    /// `SWITCHJARNOW` -- the focused tab's "Switch Jar Now". See
    /// `App::switch_jar_now`.
    SwitchJarNow,
    /// `CLEARJAR <name|default>` -- that jar's "Clear cookies", confirmed
    /// (`App::clear_jar_cookies`). `name` as for `SETJAR`; an unknown name
    /// does nothing.
    ClearJar(String),
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
    /// `WAITTABS [secs]` -- blocks the stdin reader (so later lines wait
    /// too) until no tab spawn is in flight, or `secs` (default 60) pass.
    /// Headless tab spawns are async and can take 30 s or more (the
    /// first-launch demo's included), so this replaces guessing a `sleep`.
    /// Handled by `spawn_stdin_reader` itself, which sends
    /// `NotifyWhenTabsSpawned`; never reaches the app.
    WaitTabs(std::time::Duration),
    /// `SLEEP secs` -- pauses the stdin reader, so the pause stays in order
    /// with the commands around it. A driver script's own `sleep` doesn't:
    /// while the reader is blocked (`WAITTABS`, `SCREENSHOT`) the script
    /// keeps writing, and the lines it buffered arrive back to back.
    /// Handled by `spawn_stdin_reader`; never reaches the app.
    Sleep(std::time::Duration),
    /// Internal, sent only by `spawn_stdin_reader` for `WAITTABS`: the app
    /// sends `()` on this once `App::tab_spawns_in_flight` is zero.
    NotifyWhenTabsSpawned(std::sync::mpsc::Sender<()>),
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
    ("DOUBLECLICK x y", "Left double-click at (x, y), both clicks in one frame"),
    ("SCROLL x y dx dy", "Scroll at (x, y) by (dx, dy)"),
    ("ADDRESSBAR text", "Set the address bar's text and focus"),
    ("ENTER", "Accept the address bar / confirm a dropdown row"),
    ("ESCAPE", "Dismiss a suggestion dropdown, or leave Tab view for Graph view"),
    ("BACKSPACE", "A Backspace key press in the focused egui text field"),
    ("EGUITEXT text", "Type into the focused egui text field (a jar or group name; TYPE goes to the page)"),
    ("ARROWRIGHT", "A Right arrow key press (JSON level: open the selected outline row)"),
    ("ARROWLEFT", "A Left arrow key press (JSON level: close it, or select its parent)"),
    ("DIGITKEY n", "A plain digit key press (0-9): in the Graph view, jumps to the card with that number"),
    ("SPACEKEY", "A Space key press (a focused tab card toggles in the selection)"),
    ("ARROWDOWN", "Move the address-bar suggestion highlight down"),
    ("ARROWUP", "Move the address-bar suggestion highlight up"),
    ("SHIFTF10", "Shift+F10 -- open the focused graph card's menu"),
    ("TABKEY [shift]", "Tab (or Shift+Tab) -- move egui keyboard focus to the next (previous) widget"),
    ("GRAPHLEFT", "Graph view: move focus left"),
    ("GRAPHRIGHT", "Graph view: move focus right"),
    ("GRAPHUP", "Graph view: move focus up"),
    ("GRAPHDOWN", "Graph view: move focus down"),
    ("GRAPHOPEN", "Open whichever Graph-view node is currently focused"),
    ("TYPE text", "Send each character to the focused tab's own page (not the address bar)"),
    ("KEYPAGE key", "Send a KeyDown+KeyUp of `key` to the focused tab's own page"),
    ("INSTALLENGINE kind", "Start downloading and installing the engine's runtime (CEF only)"),
    ("ENGINESTATE kind", "Print the engine's state (Installed, Installing n%, Failed: ..) to stderr"),
    ("REOPENTAB", "Ctrl+Shift+T -- reopen the newest Recently Closed entry"),
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
    (
        "SETVISIBLE true|false",
        "Send SetVisible directly to the focused tab (bypasses the usual resize-bundled visibility change)",
    ),
    ("SCREENSHOT path", "Save the next frame as a PNG at `path`; later commands wait for it (30 s max)"),
    ("RESIZE w h", "Resize the window to (w, h)"),
    ("QUIT", "Exit the app"),
    ("QUICKSWITCHOPEN", "Ctrl+K -- open the quick switcher"),
    ("QUICKSWITCHTEXT text", "Set the quick switcher's search text"),
    ("QUICKSWITCHDOWN", "Quick switcher: move highlight down"),
    ("QUICKSWITCHUP", "Quick switcher: move highlight up"),
    ("QUICKSWITCHENTER", "Quick switcher: confirm the highlighted row"),
    ("QUICKSWITCHESCAPE", "Close the quick switcher"),
    ("SESSIONNAMETEXT text", "Set the \"Save Session As...\" name field"),
    ("SAVESESSIONAS name", "Save the running session as `name` (sessions/<name>.mnf)"),
    ("OPENSESSION name", "Open the named save `name`, replacing the running session"),
    ("GRAPHFILTERTEXT text", "Set the Graph toolbar's own filter box"),
    ("HISTORYFILTERTEXT text", "Set the History popup's own filter box"),
    ("BOOKMARKSFILTERTEXT text", "Set the Bookmarks popup's own filter box"),
    ("TOGGLEVIEW", "Ctrl+G -- toggle Tab / Graph view"),
    ("TOGGLESETTINGSPAGE", "Settings gear -- enter/leave the Settings page"),
    ("EXPORTSESSION path", "Save the current session to `path`"),
    ("EXPORTSTATE path", "Write view, address bar and dropdown state to `path` as JSON"),
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
    ("GOBACK", "Back button on the focused tab"),
    ("GOFORWARD", "Forward button on the focused tab"),
    ("NEWGROUP", "Ctrl+Shift+G -- new group"),
    ("NEWNOTE", "Graph toolbar's \"+ Note\" button"),
    (
        "ADDFILE path [x y]",
        "Adds a file as the node its content makes (image, PDF, JSON, note, tab, or a download card); \
         x y is a window point, like a drop there",
    ),
    ("ADDDOWNLOAD path", "Add a download node for `path`, as if the focused tab started it"),
    ("WAITTABS [secs]", "Hold later commands until no tab spawn is in flight (default 60 s max)"),
    ("SLEEP secs", "Hold later commands for `secs` (in order, unlike a driver script's own sleep)"),
    ("TOGGLESHORTCUTSHELP", "F1 -- toggle the keyboard-shortcut reference popup"),
    ("SPLIT engine", "Open a Split pane running `engine` (servo|cef|webkit|webview2)"),
    ("CLOSESPLIT", "Close the Split pane"),
    ("JSONSELECT [pointer]", "Select an entry on the current JSON level (no pointer: the root)"),
    ("JSONTOGGLE [pointer]", "Collapse or expand a card on the current JSON level"),
    ("JSONMORE [pointer]", "Reveal the next page of a card on the current JSON level"),
    ("JSONRELAYOUT", "Relayout the current JSON level (clears saved card positions)"),
    ("JSONSEARCH text", "Search the current JSON level's keys and values"),
    ("CLOSEGROUP id", "Close the group `id`, same as its own \"Close\" context-menu action"),
    (
        "SLEEPNODE id",
        "Ask the tab `id` (or every tab of the group `id`) to sleep, same as its \"Sleep\" \
         context-menu action; it sleeps when the engine answers the page-state query (wait a second)",
    ),
    (
        "IDLESWEEP secs",
        "Run one idle auto-sleep sweep: tabs out of sight for `secs` seconds are asked to sleep, \
         and those with a changed form stay awake (the clock updates on every sweep)",
    ),
    ("SETJAR <name|default>", "The focused tab uses that jar, by label or name (a new named jar if none has it)"),
    ("SWITCHJARNOW", "The focused tab moves to its new jar now, if it is waiting to switch"),
    ("CLEARJAR <name|default>", "Clear that jar's cookies, as its confirmed \"Clear cookies\" does"),
    ("TRIGGERFIRSTLAUNCHDEMO", "Dev-only: re-spawn the first-launch demo graph on demand"),
    ("HELP", "Print this command list"),
];

/// The longest SLEEP/WAITTABS a script may ask for: a day.
pub(crate) const MAX_WAIT_SECS: f64 = 86_400.0;

/// SLEEP/WAITTABS seconds: finite and within `0..=MAX_WAIT_SECS`.
/// `Duration::from_secs_f64` panics on infinity or overflow, which killed
/// the stdin reader and left headless runs hanging.
pub(crate) fn parse_secs(s: &str) -> Option<std::time::Duration> {
    let secs: f64 = s.parse().ok()?;
    (secs.is_finite() && (0.0..=MAX_WAIT_SECS).contains(&secs)).then(|| std::time::Duration::from_secs_f64(secs))
}

/// Case-insensitive, matching `engine_kind_serde`'s own lowercase
/// convention (`settings.json` and session exports use the same strings) --
/// `SPLIT`'s one argument.
fn parse_engine_kind(name: &str) -> Option<EngineKind> {
    EngineKind::from_name(name)
}

fn parse_mouse_button(name: &str) -> MouseButton {
    match name.to_ascii_lowercase().as_str() {
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::Left,
    }
}

/// Maps a debug command's key name (`Enter`, `ShiftLeft`, `KeyA`, ...: the DOM
/// code, `KeyCode::dom_code`) to `ipc_protocol::KeyCode`. A bare letter or
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
fn parse_add_file(rest: &str) -> Option<DebugCommand> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    if words.len() >= 3 {
        if let (Ok(x), Ok(y)) = (words[words.len() - 2].parse::<f32>(), words[words.len() - 1].parse::<f32>()) {
            return Some(DebugCommand::AddFile(words[..words.len() - 2].join(" "), Some((x, y))));
        }
    }
    Some(DebugCommand::AddFile(rest.to_string(), None))
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
    // Commands whose whole argument is a path or text, which must not be empty.
    let non_empty = |command: fn(String) -> DebugCommand| (!rest.is_empty()).then(|| command(rest.to_string()));
    match cmd.to_ascii_uppercase().as_str() {
        "MOUSEMOVE" => Some(DebugCommand::MouseMove { x: parts.next()?.parse().ok()?, y: parts.next()?.parse().ok()? }),
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
        "DOUBLECLICK" => {
            Some(DebugCommand::DoubleClick { x: parts.next()?.parse().ok()?, y: parts.next()?.parse().ok()? })
        }
        "SCROLL" => Some(DebugCommand::Scroll {
            x: parts.next()?.parse().ok()?,
            y: parts.next()?.parse().ok()?,
            delta_x: parts.next()?.parse().ok()?,
            delta_y: parts.next()?.parse().ok()?,
        }),
        "ADDRESSBAR" => Some(DebugCommand::AddressBar(rest.to_string())),
        "ENTER" => Some(DebugCommand::Enter),
        "ESCAPE" => Some(DebugCommand::Escape),
        "BACKSPACE" => Some(DebugCommand::Backspace),
        "EGUITEXT" => non_empty(DebugCommand::EguiText),
        "ARROWRIGHT" => Some(DebugCommand::ArrowRight),
        "ARROWLEFT" => Some(DebugCommand::ArrowLeft),
        "SPACEKEY" => Some(DebugCommand::SpaceKey),
        "DIGITKEY" => rest.trim().parse().ok().filter(|d| *d <= 9).map(DebugCommand::DigitKey),
        "ARROWDOWN" => Some(DebugCommand::ArrowDown),
        "ARROWUP" => Some(DebugCommand::ArrowUp),
        "SHIFTF10" => Some(DebugCommand::ShiftF10),
        "TABKEY" => Some(DebugCommand::TabKey(rest.trim() == "shift")),
        "GRAPHLEFT" => Some(DebugCommand::GraphFocusLeft),
        "GRAPHRIGHT" => Some(DebugCommand::GraphFocusRight),
        "GRAPHUP" => Some(DebugCommand::GraphFocusUp),
        "GRAPHDOWN" => Some(DebugCommand::GraphFocusDown),
        "GRAPHOPEN" => Some(DebugCommand::GraphOpenFocused),
        "TYPE" => Some(DebugCommand::TypeIntoPage(rest.to_string())),
        "KEYPAGE" => Some(DebugCommand::KeyPage(parse_key_code(parts.next()?)?)),
        "REOPENTAB" => Some(DebugCommand::ReopenTab),
        "INSTALLENGINE" => parse_engine_kind(rest).map(DebugCommand::InstallEngine),
        "ENGINESTATE" => parse_engine_kind(rest).map(DebugCommand::EngineState),
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
        "SCREENSHOT" => non_empty(DebugCommand::Screenshot),
        "RESIZE" => Some(DebugCommand::Resize(parts.next()?.parse().ok()?, parts.next()?.parse().ok()?)),
        "QUIT" => Some(DebugCommand::Quit),
        "QUICKSWITCHOPEN" => Some(DebugCommand::QuickSwitchOpen),
        "QUICKSWITCHTEXT" => Some(DebugCommand::QuickSwitchText(rest.to_string())),
        "QUICKSWITCHDOWN" => Some(DebugCommand::QuickSwitchDown),
        "QUICKSWITCHUP" => Some(DebugCommand::QuickSwitchUp),
        "QUICKSWITCHENTER" => Some(DebugCommand::QuickSwitchEnter),
        "QUICKSWITCHESCAPE" => Some(DebugCommand::QuickSwitchEscape),
        "SESSIONNAMETEXT" => Some(DebugCommand::SessionNameText(rest.to_string())),
        "SAVESESSIONAS" => non_empty(DebugCommand::SaveSessionAs),
        "OPENSESSION" => non_empty(DebugCommand::OpenSession),
        "GRAPHFILTERTEXT" => Some(DebugCommand::GraphFilterText(rest.to_string())),
        "HISTORYFILTERTEXT" => Some(DebugCommand::HistoryFilterText(rest.to_string())),
        "BOOKMARKSFILTERTEXT" => Some(DebugCommand::BookmarksFilterText(rest.to_string())),
        "TOGGLEVIEW" => Some(DebugCommand::ToggleView),
        "TOGGLESETTINGSPAGE" => Some(DebugCommand::ToggleSettingsPage),
        "EXPORTSESSION" => non_empty(DebugCommand::ExportSession),
        "EXPORTSTATE" => non_empty(DebugCommand::ExportState),
        "IMPORTSESSION" => non_empty(DebugCommand::ImportSession),
        "EXPORTBOOKMARKS" => non_empty(DebugCommand::ExportBookmarks),
        "IMPORTBOOKMARKS" => non_empty(DebugCommand::ImportBookmarks),
        "CLOSEALLLEVEL" => Some(DebugCommand::CloseAllLevel),
        "CONFIRMIMPORT" => Some(DebugCommand::ConfirmImport),
        "CANCELIMPORT" => Some(DebugCommand::CancelImport),
        "RENAMEGROUP" => Some(DebugCommand::RenameGroup),
        "BOOKMARKFOLDERTEXT" => Some(DebugCommand::BookmarkFolderText(rest.to_string())),
        "DISMISSPEEK" => Some(DebugCommand::DismissPeek),
        "EXPORTGROUP" => non_empty(DebugCommand::ExportGroup),
        "IMPORTGROUP" => non_empty(DebugCommand::ImportGroup),
        "DUPLICATETAB" => Some(DebugCommand::DuplicateTab),
        "PRINTTOPDF" => non_empty(DebugCommand::PrintToPdf),
        "TOGGLEPIN" => Some(DebugCommand::TogglePin),
        "RELOAD" => Some(DebugCommand::Reload),
        "GOBACK" => Some(DebugCommand::GoBack),
        "GOFORWARD" => Some(DebugCommand::GoForward),
        "NEWGROUP" => Some(DebugCommand::NewGroup),
        "NEWNOTE" => Some(DebugCommand::NewNote),
        "ADDFILE" => (!rest.is_empty()).then(|| parse_add_file(rest)).flatten(),
        "ADDDOWNLOAD" => non_empty(DebugCommand::AddDownload),
        "WAITTABS" => Some(DebugCommand::WaitTabs(if rest.is_empty() {
            std::time::Duration::from_secs(60)
        } else {
            parse_secs(rest)?
        })),
        "SLEEP" => Some(DebugCommand::Sleep(parse_secs(rest)?)),
        "TOGGLESHORTCUTSHELP" => Some(DebugCommand::ToggleShortcutsHelp),
        "SPLIT" => parse_engine_kind(rest).map(DebugCommand::Split),
        "CLOSESPLIT" => Some(DebugCommand::CloseSplit),
        "CLOSEGROUP" => non_empty(DebugCommand::CloseGroup),
        "SLEEPNODE" => non_empty(DebugCommand::SleepNode),
        "IDLESWEEP" => rest.parse().ok().map(DebugCommand::IdleSweep),
        "SETJAR" => non_empty(DebugCommand::SetJar),
        "SWITCHJARNOW" => Some(DebugCommand::SwitchJarNow),
        "CLEARJAR" => non_empty(DebugCommand::ClearJar),
        "JSONSELECT" => Some(DebugCommand::JsonSelect(rest.to_string())),
        "JSONTOGGLE" => Some(DebugCommand::JsonToggle(rest.to_string())),
        "JSONMORE" => Some(DebugCommand::JsonMore(rest.to_string())),
        "JSONRELAYOUT" => Some(DebugCommand::JsonRelayout),
        "JSONSEARCH" => Some(DebugCommand::JsonSearch(rest.to_string())),
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
        // `--script <file>` reads the commands from a file instead of stdin,
        // for a run that has no stdin to pipe (the elevated `Samply` task).
        let args: Vec<String> = std::env::args().collect();
        let script = args.iter().position(|a| a == "--script").and_then(|i| args.get(i + 1));
        let input: Box<dyn std::io::BufRead> = match script {
            Some(path) => match std::fs::File::open(path) {
                Ok(file) => Box::new(std::io::BufReader::new(file)),
                Err(e) => {
                    eprintln!("--script {path}: {e}");
                    let _ = proxy.send_event(UserEvent::Debug(DebugCommand::Quit));
                    return;
                }
            },
            None => Box::new(std::io::stdin().lock()),
        };
        for line in input.lines() {
            let Ok(line) = line else { break };
            match parse_line(&line) {
                Some(DebugCommand::WaitTabs(timeout)) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    if proxy.send_event(UserEvent::Debug(DebugCommand::NotifyWhenTabsSpawned(tx))).is_err() {
                        break;
                    }
                    if rx.recv_timeout(timeout).is_err() {
                        eprintln!("WAITTABS: tab spawns still in flight after {timeout:?}; continuing");
                    }
                }
                Some(DebugCommand::Sleep(duration)) => std::thread::sleep(duration),
                Some(DebugCommand::Screenshot(path)) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let cmd = DebugCommand::NotifyWhenScreenshotTaken(path.clone(), tx);
                    if proxy.send_event(UserEvent::Debug(cmd)).is_err() {
                        break;
                    }
                    if rx.recv_timeout(std::time::Duration::from_secs(30)).is_err() {
                        eprintln!("SCREENSHOT: {path} not taken after 30 s; continuing");
                    }
                }
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

impl App {
    /// Applies one headless `DebugCommand` (see `debug_control`'s doc
    /// comment for the overall design: mouse commands are real synthetic
    /// `WindowEvent`s, keyboard/text commands call the equivalent effect
    /// directly since `winit::event::KeyEvent` has no public constructor).
    pub(crate) fn handle_debug_command(&mut self, event_loop: &ActiveEventLoop, cmd: DebugCommand) {
        // A transition only ends in a frame, and frames can stall for
        // seconds on a busy GPU, so one that ran out long ago could still
        // be `Some` here and make the ENTER/ESCAPE/arrow guards below drop
        // their command. It's timed by the clock, so end it by the clock.
        self.finish_transition_if_done();
        match cmd {
            DebugCommand::MouseMove { x, y } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
                );
            }
            DebugCommand::MouseDown { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput { device_id: DeviceId::dummy(), state: ElementState::Pressed, button },
                );
            }
            DebugCommand::MouseUp { x, y, button } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseInput { device_id: DeviceId::dummy(), state: ElementState::Released, button },
                );
            }
            DebugCommand::Click { x, y, button } => self.debug_click(event_loop, x, y, button, 1),
            DebugCommand::DoubleClick { x, y } => self.debug_click(event_loop, x, y, WinitMouseButton::Left, 2),
            DebugCommand::Scroll { x, y, delta_x, delta_y } => {
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) },
                );
                self.dispatch_window_event(
                    event_loop,
                    WindowEvent::MouseWheel {
                        device_id: DeviceId::dummy(),
                        delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(delta_x, delta_y)),
                        phase: TouchPhase::Moved,
                    },
                );
            }
            // A click on the address bar, then select-all and typing: focus
            // is requested now, and the text is typed as egui `Text` input in
            // the first frame the bar has it (see `debug_address_bar_typing`).
            DebugCommand::AddressBar(text) if self.showing_tab_view() => {
                self.pending_focus = Some(FocusTarget::AddressBar);
                self.debug_address_bar_typing = Some(text);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            DebugCommand::AddressBar(_) => eprintln!("debug: ADDRESSBAR ignored: not in Tab view"),
            // Real key presses for the next frame's egui input, so they go
            // through the same checks a keyboard does, in any view.
            DebugCommand::Enter => {
                self.debug_input_events.push(debug_key_event(egui::Key::Enter, egui::Modifiers::NONE))
            }
            DebugCommand::Escape => {
                self.debug_input_events.push(debug_key_event(egui::Key::Escape, egui::Modifiers::NONE))
            }
            DebugCommand::Backspace => {
                self.debug_input_events.push(debug_key_event(egui::Key::Backspace, egui::Modifiers::NONE))
            }
            // The focused egui field takes it, as from a keyboard (`TYPE` goes to the page instead).
            DebugCommand::EguiText(text) => self.debug_input_events.push(egui::Event::Text(text)),
            DebugCommand::ArrowDown => {
                self.debug_input_events.push(debug_key_event(egui::Key::ArrowDown, egui::Modifiers::NONE))
            }
            DebugCommand::ArrowRight => {
                self.debug_input_events.push(debug_key_event(egui::Key::ArrowRight, egui::Modifiers::NONE))
            }
            DebugCommand::DigitKey(d) => {
                let key = [
                    egui::Key::Num0,
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                    egui::Key::Num8,
                    egui::Key::Num9,
                ][d as usize];
                self.debug_input_events.push(debug_key_event(key, egui::Modifiers::NONE))
            }
            DebugCommand::SpaceKey => {
                self.debug_input_events.push(debug_key_event(egui::Key::Space, egui::Modifiers::NONE))
            }
            DebugCommand::ArrowLeft => {
                self.debug_input_events.push(debug_key_event(egui::Key::ArrowLeft, egui::Modifiers::NONE))
            }
            DebugCommand::ArrowUp => {
                self.debug_input_events.push(debug_key_event(egui::Key::ArrowUp, egui::Modifiers::NONE))
            }
            DebugCommand::TabKey(shift) => {
                let modifiers = if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
                self.debug_input_events.push(debug_key_event(egui::Key::Tab, modifiers))
            }
            DebugCommand::ShiftF10 => {
                self.debug_input_events.push(debug_key_event(egui::Key::F10, egui::Modifiers::SHIFT))
            }
            DebugCommand::GraphFocusLeft => self.move_graph_focus(egui::vec2(-1.0, 0.0)),
            DebugCommand::GraphFocusRight => self.move_graph_focus(egui::vec2(1.0, 0.0)),
            DebugCommand::GraphFocusUp => self.move_graph_focus(egui::vec2(0.0, -1.0)),
            DebugCommand::GraphFocusDown => self.move_graph_focus(egui::vec2(0.0, 1.0)),
            DebugCommand::GraphOpenFocused => self.open_focused_graph_node(),
            DebugCommand::TypeIntoPage(text) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        for ch in text.chars() {
                            node.tab.send(&HostToEngine::Input(InputEvent::Char { codepoint: ch as u32 }));
                        }
                    }
                }
            }
            DebugCommand::KeyPage(key) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        node.tab.send(&HostToEngine::Input(InputEvent::KeyDown { key, modifiers: self.key_modifiers }));
                        node.tab.send(&HostToEngine::Input(InputEvent::KeyUp { key, modifiers: self.key_modifiers }));
                    }
                }
            }
            DebugCommand::ReopenTab => self.reopen_closed_shortcut(),
            DebugCommand::InstallEngine(kind) => {
                self.start_engine_install(kind);
            }
            DebugCommand::EngineState(kind) => {
                eprintln!(
                    "engine-state {}: {}",
                    kind.desc().name,
                    crate::engine_info::engine_state_label(&self.engine_state(kind)).0
                )
            }
            DebugCommand::ZoomIn => self.zoom_in_shortcut(),
            DebugCommand::ZoomOut => self.zoom_out_shortcut(),
            DebugCommand::ZoomReset => self.zoom_reset_shortcut(),
            DebugCommand::OpenDevTools => self.open_devtools_shortcut(),
            DebugCommand::NewTab => self.new_tab_shortcut(),
            DebugCommand::CloseTab => self.close_focused_tab_shortcut(),
            DebugCommand::FocusAddressBar => self.focus_address_bar_shortcut(),
            DebugCommand::NextTab => self.cycle_focused_tab(true),
            DebugCommand::PrevTab => self.cycle_focused_tab(false),
            DebugCommand::OpenFindBar => self.open_find_bar_shortcut(),
            DebugCommand::FindText(text) => {
                self.find.text = text;
                self.send_find_start();
            }
            DebugCommand::FindMatchCase(match_case) => {
                self.find.match_case = match_case;
                self.send_find_start();
            }
            DebugCommand::FindNext => self.send_find_navigate(true),
            DebugCommand::FindPrevious => self.send_find_navigate(false),
            DebugCommand::CloseFindBar => self.close_find_bar(),
            DebugCommand::SetVisible(visible) => {
                if let Some(id) = self.focused.clone() {
                    if let Some(node) = self.nodes.get_mut(&id).and_then(|n| n.as_tab_mut()) {
                        node.tab.set_visible(visible);
                    }
                }
            }
            // Only ever intercepted by the stdin reader itself.
            DebugCommand::Screenshot(_) | DebugCommand::Sleep(_) => {}
            DebugCommand::NotifyWhenScreenshotTaken(path, waiter) => {
                self.pending_screenshot = Some(path);
                self.screenshot_waiter = Some(waiter);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            DebugCommand::Resize(width, height) => {
                if let Some(window) = &self.window {
                    let _ = window.request_inner_size(PhysicalSize::new(width, height));
                }
            }
            DebugCommand::QuickSwitchOpen => self.open_quick_switcher(),
            DebugCommand::QuickSwitchText(text) => {
                if let Some(switcher) = &mut self.quick_switcher {
                    switcher.query = text;
                    switcher.selected = 0;
                }
            }
            DebugCommand::QuickSwitchDown => self.quick_switcher_move(1),
            DebugCommand::QuickSwitchUp => self.quick_switcher_move(-1),
            DebugCommand::QuickSwitchEnter => self.quick_switcher_confirm(),
            DebugCommand::QuickSwitchEscape => self.quick_switcher = None,
            DebugCommand::SessionNameText(text) => self.session_name_draft = text,
            DebugCommand::SaveSessionAs(name) => self.save_named_session(&name),
            DebugCommand::OpenSession(name) => {
                self.load_named_session(&name);
                self.close_settings_page();
            }
            DebugCommand::GraphFilterText(text) => self.graph_filter = text,
            DebugCommand::HistoryFilterText(text) => self.history_filter = text,
            DebugCommand::BookmarksFilterText(text) => self.bookmarks_filter = text,
            DebugCommand::ToggleView => self.toggle_view_mode_shortcut(),
            DebugCommand::ToggleSettingsPage => self.toggle_settings_page(),
            DebugCommand::ExportSession(path) => {
                self.export_session_to(std::path::Path::new(&path));
            }
            DebugCommand::ExportState(path) => {
                let focused_url = self
                    .focused
                    .as_ref()
                    .and_then(|id| self.nodes.get(id))
                    .and_then(|n| n.as_tab())
                    .map(|t| t.url.clone());
                let suggestions: Vec<String> = self
                    .address_bar
                    .combined()
                    .into_iter()
                    .map(|s| match s {
                        Suggestion::OpenTab { tab_id, .. } => format!("tab:{tab_id}"),
                        Suggestion::Search { query } => query,
                    })
                    .collect();
                // The current level's cards, in draw order, with where each
                // one's center is on screen: what a headless test needs to
                // aim a click or drag at a card.
                let cards: Vec<serde_json::Value> = self
                    .level_items()
                    .into_iter()
                    .filter_map(|item| {
                        let screen = self.graph_world_rect_to_screen(item.rect);
                        if item.kind == LevelItemKind::JsonCard {
                            return Some(serde_json::json!({
                                "id": item.id,
                                "kind": "JsonCard",
                                "screen": [screen.center().x, screen.center().y],
                                "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                            }));
                        }
                        let LevelItemKind::Node(kind) = item.kind else { return None };
                        let node = self.nodes.get(&item.id)?;
                        Some(serde_json::json!({
                            "id": item.id,
                            "kind": format!("{kind:?}"),
                            "group": node.group,
                            "parent": node.parent,
                            "color": node.color.is_some(),
                            "pinned": node.payload.pinned(),
                            "editing": node.as_note().is_some_and(|n| n.editing),
                            "screen": [screen.center().x, screen.center().y],
                            "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                        }))
                    })
                    .collect();
                let mut selected_tabs: Vec<&String> = self.selected_tabs.iter().collect();
                selected_tabs.sort();
                let state = serde_json::json!({
                    "tab_view": self.showing_tab_view(),
                    "transition": self.transition.is_some(),
                    "focused_url": focused_url,
                    "address_bar_text": self.address_bar.text,
                    "suggestions": suggestions,
                    "highlighted": self.address_bar.highlighted,
                    "current_group": self.current_group,
                    "cards": cards,
                    "json_level": self.json_level_state(),
                    "selected_tabs": selected_tabs,
                    // The toast showing, or else the next queued one.
                    "toast": self.save_toast.as_ref().map(|(message, _)| message).or(self.save_toast_queue.front()),
                });
                if let Err(err) = std::fs::write(&path, state.to_string()) {
                    eprintln!("EXPORTSTATE: couldn't write {path}: {err}");
                }
            }
            DebugCommand::ImportSession(path) => match crate::json_file::load(std::path::Path::new(&path), "session") {
                Ok(json) => self.pending_import = Some(json),
                Err(err) => self.queue_toast(err),
            },
            DebugCommand::ConfirmImport => {
                if let Some(json) = self.pending_import.take() {
                    self.replace_running_session(&json, crate::store::json::Trust::Foreign);
                }
            }
            DebugCommand::CancelImport => {
                self.pending_import = None;
            }
            DebugCommand::RenameGroup => self.rename_current_group_shortcut(),
            DebugCommand::BookmarkFolderText(text) => {
                if let Some((_, draft)) = &mut self.renaming_bookmark_folder {
                    *draft = text;
                }
            }
            DebugCommand::DismissPeek => self.dismiss_hover_peek(),
            DebugCommand::ExportGroup(path) => {
                if let Some((group_id, _)) = self
                    .node_menu
                    .clone()
                    .filter(|(id, _)| self.nodes.get(id).is_some_and(|node| node.as_group().is_some()))
                {
                    if let Some(json) = self.export_group_json(&group_id) {
                        write_json_export(&json, std::path::Path::new(&path), "group");
                    }
                }
            }
            DebugCommand::ImportGroup(path) => {
                match crate::json_file::load::<store::json::JsonDoc>(std::path::Path::new(&path), "group") {
                    Ok(json) => self.import_group_json(&json),
                    Err(err) => self.queue_toast(err),
                }
            }
            DebugCommand::DuplicateTab => self.duplicate_focused_tab_shortcut(),
            DebugCommand::ExportBookmarks(path) => {
                bookmarks::save_to(&self.bookmarks, std::path::Path::new(&path));
            }
            DebugCommand::ImportBookmarks(path) => match bookmarks::load_from(std::path::Path::new(&path)) {
                Ok(imported) => {
                    self.bookmarks = imported;
                    bookmarks::save(&self.bookmarks);
                }
                Err(err) => self.queue_toast(err),
            },
            DebugCommand::PrintToPdf(path) => self.start_print_to_pdf(path),
            DebugCommand::TogglePin => self.pin_focused_tab_shortcut(),
            DebugCommand::Reload => self.reload_focused_tab_shortcut(),
            DebugCommand::GoBack | DebugCommand::GoForward => {
                if let Some(id) = self.focused.clone() {
                    let dir = if matches!(cmd, DebugCommand::GoBack) { Dir::Back } else { Dir::Forward };
                    self.navigate_history(&id, dir);
                }
            }
            DebugCommand::NewGroup => self.new_group_shortcut(),
            DebugCommand::NewNote => self.new_note_shortcut(),
            DebugCommand::AddFile(path, None) => {
                if self.add_file_at(path.into(), None).is_ok() {
                    self.save_session();
                }
            }
            // Placed like a real drop at that window point. No
            // `save_session` here either -- same reasoning as
            // `WindowEvent::DroppedFile`, so a script sending several
            // `ADDFILE`s with a point still gets one save per batch, not
            // one per file.
            DebugCommand::AddFile(path, Some((x, y))) => {
                self.drop_file(path.into(), egui::pos2(x, y));
            }
            // Only ever intercepted by the stdin reader itself.
            DebugCommand::WaitTabs(_) => {}
            DebugCommand::NotifyWhenTabsSpawned(waiter) => {
                if self.tab_spawns_in_flight == 0 {
                    let _ = waiter.send(());
                } else {
                    self.tabs_spawned_waiters.push(waiter);
                }
            }
            DebugCommand::AddDownload(path) => {
                if let Some(tab_id) = self.focused.clone() {
                    self.insert_download_node(tab_id, path, None, None);
                }
            }
            DebugCommand::Split(kind) => self.start_split(kind),
            DebugCommand::CloseSplit => self.close_split(),
            DebugCommand::JsonSelect(pointer) => self.apply_json_edit(JsonEdit::Select(pointer)),
            DebugCommand::JsonToggle(pointer) => self.apply_json_edit(JsonEdit::Toggle(pointer)),
            DebugCommand::JsonMore(pointer) => self.apply_json_edit(JsonEdit::More(pointer)),
            DebugCommand::JsonRelayout => self.apply_json_edit(JsonEdit::Relayout),
            DebugCommand::JsonSearch(text) => self.set_json_search(text),
            DebugCommand::CloseGroup(id) => {
                if self.nodes.get(&id).is_some_and(|n| n.as_group().is_some()) {
                    self.close_node(&id);
                }
            }
            DebugCommand::IdleSweep(secs) => self.sweep_idle_tabs(Some(std::time::Duration::from_secs(secs))),
            DebugCommand::SleepNode(id) => {
                if self.nodes.get(&id).is_some_and(|n| n.as_group().is_some()) {
                    self.sleep_group(&id);
                } else {
                    self.request_sleep(&id, SleepWhy::Manual);
                }
            }
            DebugCommand::SetJar(name) => {
                if self.set_focused_tab_jar_named(&name) {
                    self.save_session();
                }
            }
            DebugCommand::SwitchJarNow => {
                if let Some(tab) = self.focused.clone() {
                    self.switch_jar_now(&tab);
                }
            }
            DebugCommand::ClearJar(name) => {
                let key = if name.trim().eq_ignore_ascii_case("default") {
                    Some(crate::jar::JarKey::Default)
                } else {
                    self.jar_named(&name).map(crate::jar::JarKey::Named)
                };
                match key {
                    Some(key) => self.clear_jar_cookies(key),
                    None => eprintln!("CLEARJAR: no jar named {name:?}"),
                }
            }
            DebugCommand::TriggerFirstLaunchDemo => self.spawn_first_launch_demo(),
            DebugCommand::Help => debug_control::print_help(),
            DebugCommand::CloseAllLevel => self.close_all_at_current_level(),
            DebugCommand::ToggleShortcutsHelp => {
                self.shortcuts_help_open = !self.shortcuts_help_open;
            }
            DebugCommand::Quit => {
                // See `quit_after_redraws`'s doc comment: exiting right
                // now could drop a pending Enter/Escape/arrow command, or a
                // just-requested Screenshot that hasn't been captured yet
                // (`pending_screenshot` is only consumed inside a
                // `RedrawRequested` — a `Screenshot` immediately followed by
                // `Quit`, with no frame in between, would otherwise silently
                // lose the screenshot the same way an unconsumed Enter
                // could).
                if self.debug_input_pending() {
                    self.quit_after_redraws = Some(3);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                } else {
                    event_loop.exit();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fix batch A5: `EGUITEXT` types into the focused egui field (a jar name, a group rename) and `BACKSPACE` erases
    /// there; `TYPE` still goes to the page.
    #[test]
    fn egui_typing_commands_parse() {
        assert!(matches!(parse_line("EGUITEXT Work Stuff"), Some(DebugCommand::EguiText(t)) if t == "Work Stuff"));
        assert!(parse_line("EGUITEXT").is_none(), "text is required");
        assert!(matches!(parse_line("backspace"), Some(DebugCommand::Backspace)));
        assert!(matches!(parse_line("TYPE abc"), Some(DebugCommand::TypeIntoPage(t)) if t == "abc"));
        for usage in ["EGUITEXT text", "BACKSPACE"] {
            assert!(COMMAND_HELP.iter().any(|(u, _)| *u == usage), "{usage} is in HELP");
        }
    }

    #[test]
    fn wait_seconds_must_be_finite_and_bounded() {
        assert_eq!(parse_secs("1.5"), Some(std::time::Duration::from_millis(1500)));
        assert_eq!(parse_secs("0"), Some(std::time::Duration::ZERO));
        for bad in ["inf", "-inf", "NaN", "1e30", "-1", "86401", "x", ""] {
            assert_eq!(parse_secs(bad), None, "{bad}");
        }
        assert!(parse_line("SLEEP inf").is_none());
        assert!(parse_line("WAITTABS 1e30").is_none());
        assert!(matches!(parse_line("WAITTABS"), Some(DebugCommand::WaitTabs(d)) if d.as_secs() == 60));
    }

    /// Every HELP usage parses with sample arguments, and every sampled verb is in HELP.
    #[test]
    fn help_and_parser_agree() {
        // One sample line per parser verb, written from `parse_line`'s match arms
        // (`?` is `HELP`'s alias, checked separately below).
        const SAMPLES: &[&str] = &[
            "MOUSEMOVE 1 2",
            "MOUSEDOWN 1 2",
            "MOUSEUP 1 2 right",
            "CLICK 1 2",
            "DOUBLECLICK 1 2",
            "SCROLL 1 2 0 -3",
            "ADDRESSBAR example.com",
            "ENTER",
            "ESCAPE",
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
            "GRAPHLEFT",
            "GRAPHRIGHT",
            "GRAPHUP",
            "GRAPHDOWN",
            "GRAPHOPEN",
            "TYPE abc",
            "KEYPAGE Enter",
            "REOPENTAB",
            "INSTALLENGINE cef",
            "ENGINESTATE cef",
            "ZOOMIN",
            "ZOOMOUT",
            "ZOOMRESET",
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
            "SLEEP 0.5",
            "TOGGLESHORTCUTSHELP",
            "SPLIT cef",
            "CLOSESPLIT",
            "CLOSEGROUP g1",
            "SLEEPNODE t1",
            "IDLESWEEP 30",
            "SETJAR default",
            "SWITCHJARNOW",
            "CLEARJAR Work",
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
            assert!(parse_line(sample).is_some(), "sample {sample:?} parses");
            assert!(COMMAND_HELP.iter().any(|(usage, _)| verb(usage) == verb(sample)), "{sample:?} has HELP");
        }
        for (usage, _) in COMMAND_HELP {
            assert!(SAMPLES.iter().any(|s| verb(s) == verb(usage)), "HELP verb {usage:?} has a sample");
        }
        assert!(matches!(parse_line("?"), Some(DebugCommand::Help)));
    }

    #[test]
    fn jar_commands_parse() {
        assert!(matches!(parse_line("SETJAR Work"), Some(DebugCommand::SetJar(n)) if n == "Work"));
        assert!(matches!(parse_line("SETJAR default"), Some(DebugCommand::SetJar(n)) if n == "default"));
        assert!(parse_line("SETJAR").is_none(), "a jar name is required");
        assert!(matches!(parse_line("SWITCHJARNOW"), Some(DebugCommand::SwitchJarNow)));
        assert!(matches!(parse_line("CLEARJAR Work"), Some(DebugCommand::ClearJar(n)) if n == "Work"));
        assert!(parse_line("CLEARJAR").is_none(), "a jar name is required");
    }

    #[test]
    fn parses_json_level_commands_with_an_optional_pointer() {
        assert!(matches!(parse_line("JSONSELECT /users/0"), Some(DebugCommand::JsonSelect(p)) if p == "/users/0"));
        assert!(matches!(parse_line("JSONTOGGLE"), Some(DebugCommand::JsonToggle(p)) if p.is_empty()));
        assert!(matches!(parse_line("JSONSEARCH two words"), Some(DebugCommand::JsonSearch(q)) if q == "two words"));
        assert!(matches!(parse_line("JSONRELAYOUT"), Some(DebugCommand::JsonRelayout)));
    }

    #[test]
    fn parses_engine_install_commands() {
        assert!(matches!(parse_line("INSTALLENGINE cef"), Some(DebugCommand::InstallEngine(EngineKind::Cef))));
        assert!(matches!(parse_line("ENGINESTATE CEF"), Some(DebugCommand::EngineState(EngineKind::Cef))));
        assert!(parse_line("INSTALLENGINE nope").is_none());
    }

    #[test]
    fn parses_addfile_with_an_optional_trailing_point() {
        assert!(matches!(
            parse_line("ADDFILE C:\\a b\\c.png"),
            Some(DebugCommand::AddFile(p, None)) if p == "C:\\a b\\c.png"
        ));
        assert!(matches!(
            parse_line("ADDFILE C:\\a b\\c.png 10 20.5"),
            Some(DebugCommand::AddFile(p, Some((x, y)))) if p == "C:\\a b\\c.png" && x == 10.0 && y == 20.5
        ));
    }
}
