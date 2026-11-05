//! The app's side of an engine's control channel, off the UI thread (survey E1).
//!
//! `ViewTab::send` only queues (`SendQueue`); one writer thread per tab (`Writer`) writes the queue out, so the UI
//! thread never waits on an engine. While the writer is behind, a new message merges into the newest queued one where
//! only the result matters (`coalesce`). A tab whose writer is stuck reads as unresponsive (`Writer::is_unresponsive`)
//! and the user restarts or ends it; nothing here kills an engine.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use manifold_view_protocol::{frame_path, HostToView, InputEvent, MessageChannel, SharedFrameBuffer};

/// Messages a tab's send queue holds before it refuses more (the tab is then unresponsive).
pub(crate) const SEND_QUEUE_CAP: usize = 256;
/// How long one write may stay blocked before the tab reads as unresponsive.
pub(crate) const UNRESPONSIVE_AFTER: Duration = Duration::from_secs(2);
/// How long the write under way must have been blocked before a refused push counts: a healthy engine can fall
/// behind a burst of more than `SEND_QUEUE_CAP` unmergeable messages (a long headless `TYPE`) for a moment.
pub(crate) const REFUSED_BLOCKED_AFTER: Duration = Duration::from_millis(50);
/// How long closing a tab waits for its queue and `Shutdown` to go out, counted from the first `Writer::shutdown`.
pub(crate) const SHUTDOWN_FLUSH: Duration = Duration::from_millis(200);

/// Engine messages, frames aside, waiting for the app (`process_manager::spawn_reader_thread`). Full, the reader
/// waits and the engine backs up on its own pipe; no message is dropped (survey E3).
pub(crate) const EVENTS_CAP: usize = 256;

/// The newest frame an engine reported. Only the newest is ever drawn, so a slow app holds one per tab, never a
/// queue of them (survey E3).
pub(crate) struct Frame {
    /// The frame file the pixels are in (`FrameReady::generation`); rows merge only between frames of one generation.
    pub(crate) generation: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The rows that changed since the frame the app last took; `None` is the whole frame.
    pub(crate) rows: Option<Range<u32>>,
    /// Inline pixels (WebKit over TCP); `None` for a local engine, whose pixels are in its shared frame file.
    pub(crate) pixels: Option<Vec<u8>>,
}

/// The rows `FrameReady`'s `damage` (x, y, w, h) covers in a frame `height` rows tall. `None` is the whole frame:
/// no damage reported, or damage that is empty or reaches past the frame (an engine's bad damage degrades to a whole
/// upload, never to a panic in wgpu).
pub(crate) fn frame_rows(damage: Option<(u32, u32, u32, u32)>, height: u32) -> Option<Range<u32>> {
    let (_, y, _, h) = damage?;
    let end = y.checked_add(h)?;
    (h > 0 && end <= height).then_some(y..end)
}

/// The rows covering both `prev` and `next` (the frame in between was never uploaded); `None` if either is the
/// whole frame.
pub(crate) fn merge_rows(prev: Option<Range<u32>>, next: Option<Range<u32>>) -> Option<Range<u32>> {
    let (prev, next) = (prev?, next?);
    Some(prev.start.min(next.start)..prev.end.max(next.end))
}

/// An engine's frame file mapped once and kept (survey E2): `FrameReady` names a generation, and a file of one
/// generation is never rewritten or resized, so the app reopens only when the generation changes. It lives in
/// `ViewTab`, not in the event loop: a respawn of the same tab reuses `socket_id`, and a new `ViewTab` starts
/// empty, so a dead host's file is never read.
#[derive(Default)]
pub(crate) struct FrameMap {
    /// Boxed: `ViewTab` sits inside a node enum clippy keeps small.
    mapped: Option<Box<(u32, SharedFrameBuffer)>>,
}

impl FrameMap {
    /// The pixels of `socket_id`'s frame file of `generation`, `width` x `height`: the cached mapping while the key
    /// matches, else a fresh `open_existing` of that file. A failed open leaves the cache empty, so the next call
    /// retries.
    pub(crate) fn pixels(&mut self, socket_id: &str, generation: u32, width: u32, height: u32) -> io::Result<&[u8]> {
        let hit =
            self.mapped.as_ref().is_some_and(|m| m.0 == generation && m.1.width() == width && m.1.height() == height);
        if !hit {
            self.mapped = None;
            let buf = SharedFrameBuffer::open_existing(frame_path(socket_id, generation), width, height)?;
            self.mapped = Some(Box::new((generation, buf)));
        }
        Ok(self.mapped.as_ref().map(|m| m.1.as_slice()).unwrap_or_default())
    }

    /// Unmaps the file (before `remove_frame_file`).
    pub(crate) fn clear(&mut self) {
        self.mapped = None;
    }
}

/// One tab's newest frame, written by its reader thread and taken by the drain.
pub(crate) type FrameSlot = Arc<Mutex<Option<Frame>>>;

/// Called by a reader after each message it hands over, and after a frame stored into an empty slot (not for the
/// frames that pile up behind it): wakes the event loop to apply it now rather than at its next tick
/// (`event_loop::view_wake`).
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;
/// The writer only serializes and writes.
const WRITER_STACK: usize = 256 * 1024;

/// `next` folded into `prev` when the engine needs only the result: the latest pointer position or size, or the sum of
/// a run of wheel steps. `None` keeps both. Only the queue's newest unwritten message is ever `prev`, so a merge never
/// moves a message past another one. A modifier change ends a run (the page may react to it).
pub(crate) fn coalesce(prev: &HostToView, next: &HostToView) -> Option<HostToView> {
    use HostToView::{Input, Resize};
    use InputEvent::{MouseMove, Scroll};
    match (prev, next) {
        (Input(MouseMove { modifiers: a, .. }), Input(MouseMove { modifiers: b, .. })) if a == b => Some(next.clone()),
        (Resize { .. }, Resize { .. }) => Some(next.clone()),
        (
            Input(Scroll { delta_x: dx, delta_y: dy, modifiers: a, .. }),
            Input(Scroll { x, y, delta_x, delta_y, modifiers: b }),
        ) if a == b => {
            Some(Input(Scroll { x: *x, y: *y, delta_x: dx + delta_x, delta_y: dy + delta_y, modifiers: *b }))
        }
        _ => None,
    }
}

/// A bounded FIFO of messages not yet written.
pub(crate) struct SendQueue {
    items: VecDeque<HostToView>,
    cap: usize,
}

impl Default for SendQueue {
    fn default() -> Self {
        Self { items: VecDeque::new(), cap: SEND_QUEUE_CAP }
    }
}

impl SendQueue {
    /// Queues `msg`, or merges it into the newest queued message (`coalesce`, allowed even when full). False when
    /// the queue is full and `msg` was dropped.
    pub(crate) fn push(&mut self, msg: HostToView) -> bool {
        if let Some(last) = self.items.back_mut() {
            if let Some(merged) = coalesce(last, &msg) {
                *last = merged;
                return true;
            }
        }
        if self.items.len() >= self.cap {
            return false;
        }
        self.items.push_back(msg);
        true
    }

    /// Queues `msg` last whatever the cap: `Shutdown`, which nothing follows.
    pub(crate) fn push_last(&mut self, msg: HostToView) {
        self.items.push_back(msg);
    }

    pub(crate) fn pop(&mut self) -> Option<HostToView> {
        self.items.pop_front()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Default)]
struct State {
    queue: SendQueue,
    /// `shutdown` was called: no more messages are taken.
    closed_at: Option<Instant>,
    /// The thread has ended: it wrote `Shutdown`, a write failed, or it panicked.
    finished: bool,
    /// Nothing will reach the engine although it may be alive: the thread panicked or never started.
    broken: bool,
    /// When the write under way started.
    writing_since: Option<Instant>,
    /// A push was refused since the last completed write.
    refused: bool,
    /// `take_newly_unresponsive` has reported the current stall.
    reported: bool,
}

impl State {
    fn unresponsive(&self, now: Instant) -> bool {
        let blocked_for_at_least = |min: Duration| self.writing_since.is_some_and(|t| now.duration_since(t) >= min);
        self.broken
            || (self.closed_at.is_none()
                && (blocked_for_at_least(UNRESPONSIVE_AFTER)
                    || (self.refused && blocked_for_at_least(REFUSED_BLOCKED_AFTER))))
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled on every change: a push (the thread waits for work) and a finish (`wait_finished`).
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One tab's send queue and the thread that writes it out.
#[derive(Clone)]
pub(crate) struct Writer(Arc<Shared>);

impl Writer {
    /// Starts the thread writing to `channel`. If it cannot start, the writer is `broken` from the outset (the tab
    /// reads as unresponsive) rather than panicking on the spawn thread.
    pub(crate) fn spawn<S: Read + Write + Send + 'static>(channel: MessageChannel<S>, name: &str) -> Writer {
        let writer = Writer(Arc::default());
        let shared = Arc::clone(&writer.0);
        let started = std::thread::Builder::new()
            .name(name.to_owned())
            .stack_size(WRITER_STACK)
            .spawn(move || write_queued(channel, &shared));
        if let Err(e) = started {
            eprintln!("{name}: could not start: {e}");
            let mut state = writer.0.lock();
            state.finished = true;
            state.broken = true;
        }
        writer
    }

    /// Queues a copy of `msg` (none when dropped for being closed); never blocks. Dropped once closed or finished, and when the queue is full (the tab then reads as
    /// unresponsive once a write is stuck).
    pub(crate) fn send(&self, msg: &HostToView) {
        let mut state = self.0.lock();
        if state.finished || state.closed_at.is_some() {
            return;
        }
        if !state.queue.push(msg.clone()) {
            state.refused = true;
        }
        drop(state);
        self.0.changed.notify_all();
    }

    /// A refused push while the write in flight has been blocked for `REFUSED_BLOCKED_AFTER`, or one write blocked
    /// for `UNRESPONSIVE_AFTER`, since the last completed write; or a broken writer. Never while closing (`shutdown`).
    pub(crate) fn is_unresponsive(&self, now: Instant) -> bool {
        self.0.lock().unresponsive(now)
    }

    /// True once per stall: the first call that finds the tab unresponsive since its last completed write (the toast).
    pub(crate) fn take_newly_unresponsive(&self, now: Instant) -> bool {
        let mut state = self.0.lock();
        let newly = state.unresponsive(now) && !state.reported;
        if newly {
            state.reported = true;
        }
        newly
    }

    /// Queues `Shutdown` after everything queued and takes nothing more. Repeat calls do nothing.
    pub(crate) fn shutdown(&self) {
        let mut state = self.0.lock();
        if state.closed_at.is_some() {
            return;
        }
        state.closed_at = Some(Instant::now());
        state.queue.push_last(HostToView::Shutdown);
        drop(state);
        self.0.changed.notify_all();
    }

    /// Waits until the thread has ended, at most until `flush` after the first `shutdown` (or after now, if none).
    /// True when it ended.
    pub(crate) fn wait_finished(&self, flush: Duration) -> bool {
        let mut state = self.0.lock();
        let deadline = state.closed_at.unwrap_or_else(Instant::now) + flush;
        while !state.finished {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self.0.changed.wait_timeout(state, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }

    /// Waits until everything pushed so far has been written (or the thread ended), at most `within`. True when so.
    #[cfg(test)]
    pub(crate) fn wait_idle(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let mut state = self.0.lock();
        // A write's end signals nothing, so look again every few milliseconds.
        while !(state.finished || (state.queue.is_empty() && state.writing_since.is_none())) {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let tick = (deadline - now).min(Duration::from_millis(2));
            state = self.0.changed.wait_timeout(state, tick).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }

    /// Wedges a writer whose engine end is never read: pushes until a push is refused while a write is blocked, so it
    /// reads as unresponsive (`REFUSED_BLOCKED_AFTER` passes once the queue is full).
    #[cfg(test)]
    pub(crate) fn wedge(&self) {
        let big = HostToView::SetCookies("x".repeat(64 * 1024));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.is_unresponsive(Instant::now()) {
            assert!(Instant::now() < deadline, "the writer never wedged: is the engine end being read?");
            self.send(&big);
            if self.0.lock().refused {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// Marks the writer finished when its thread ends, a panic included (then also `broken`).
struct Finished<'a>(&'a Shared);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.finished = true;
        state.writing_since = None;
        if std::thread::panicking() {
            state.broken = true;
        }
        drop(state);
        self.0.changed.notify_all();
    }
}

/// The writer thread: writes the queue in order, never holding the lock while writing. Ends after `Shutdown` or on
/// the first failed write (the engine is gone; its reader reports the crash).
fn write_queued<S: Read + Write>(mut channel: MessageChannel<S>, shared: &Shared) {
    let _finished = Finished(shared);
    loop {
        let msg = {
            let mut state = shared.lock();
            loop {
                if let Some(msg) = state.queue.pop() {
                    state.writing_since = Some(Instant::now());
                    break msg;
                }
                state = shared.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let last = matches!(msg, HostToView::Shutdown);
        let written = channel.send(&msg).is_ok();
        {
            let mut state = shared.lock();
            state.writing_since = None;
            state.refused = false;
            state.reported = false;
        }
        if !written || last {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
