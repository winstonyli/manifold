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

mod export;
mod input;
pub(crate) mod reader;
mod verbs;

use std::io::BufRead;
use std::str::SplitWhitespace;

use manifold_view_protocol::KeyCode;
use winit::event::MouseButton;
use winit::event_loop::EventLoopProxy;

use super::*;
use crate::event_loop::UserEvent;
use crate::process_manager::ViewKind;
use crate::sleep_state::SleepWhy;
use crate::trail::history::Dir;
use verbs::{quit, VERBS};

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

/// Parses one line of the headless debug-command text protocol. Unknown commands or malformed arguments are `None`
/// (the reader reports them).
pub(crate) fn parse_line(line: &str) -> Option<Line> {
    // Strip a leading BOM — several Windows-side tools that can drive this
    // protocol (PowerShell's default `StandardInput` stream encoding among
    // them: a `Process.StandardInput.WriteLine` from
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

#[cfg(test)]
mod tests;
