//! The app's side of an engine's control channel, off the UI thread (survey E1).
//!
//! `EngineTab::send` only queues (`SendQueue`); one writer thread per tab (`Writer`) writes the queue out, so the UI
//! thread never waits on an engine. While the writer is behind, a new message merges into the newest queued one where
//! only the result matters (`coalesce`). A tab whose writer is stuck reads as unresponsive (`Writer::is_unresponsive`)
//! and the user restarts or ends it; nothing here kills an engine.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ipc_protocol::{frame_path, HostToEngine, InputEvent, MessageChannel, SharedFrameBuffer};

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
/// `EngineTab`, not in the event loop: a respawn of the same tab reuses `socket_id`, and a new `EngineTab` starts
/// empty, so a dead host's file is never read.
#[derive(Default)]
pub(crate) struct FrameMap {
    /// Boxed: `EngineTab` sits inside a node enum clippy keeps small.
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

/// Called by a reader after each message it hands over (frames aside): wakes the event loop to apply it now rather
/// than at its next tick (`event_loop::engine_wake`).
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;
/// The writer only serializes and writes.
const WRITER_STACK: usize = 256 * 1024;

/// `next` folded into `prev` when the engine needs only the result: the latest pointer position or size, or the sum of
/// a run of wheel steps. `None` keeps both. Only the queue's newest unwritten message is ever `prev`, so a merge never
/// moves a message past another one. A modifier change ends a run (the page may react to it).
pub(crate) fn coalesce(prev: &HostToEngine, next: &HostToEngine) -> Option<HostToEngine> {
    use HostToEngine::{Input, Resize};
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
    items: VecDeque<HostToEngine>,
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
    pub(crate) fn push(&mut self, msg: HostToEngine) -> bool {
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
    pub(crate) fn push_last(&mut self, msg: HostToEngine) {
        self.items.push_back(msg);
    }

    pub(crate) fn pop(&mut self) -> Option<HostToEngine> {
        self.items.pop_front()
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
    pub(crate) fn send(&self, msg: &HostToEngine) {
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
        state.queue.push_last(HostToEngine::Shutdown);
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

    /// Wedges a writer whose engine end is never read: pushes until a push is refused while a write is blocked, so it
    /// reads as unresponsive (`REFUSED_BLOCKED_AFTER` passes once the queue is full).
    #[cfg(test)]
    pub(crate) fn wedge(&self) {
        let big = HostToEngine::SetCookies("x".repeat(64 * 1024));
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
        let last = matches!(msg, HostToEngine::Shutdown);
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
mod tests {
    use super::*;

    #[test]
    fn frame_rows_are_whole_rows_or_none() {
        assert_eq!(frame_rows(None, 10), None);
        assert_eq!(frame_rows(Some((0, 3, 8, 2)), 10), Some(3..5));
        assert_eq!(frame_rows(Some((0, 8, 8, 2)), 10), Some(8..10));
        assert_eq!(frame_rows(Some((0, 9, 8, 2)), 10), None, "past the bottom");
        assert_eq!(frame_rows(Some((0, 3, 8, 0)), 10), None, "empty");
        assert_eq!(frame_rows(Some((0, u32::MAX, 8, 2)), 10), None, "overflow");
    }

    #[test]
    fn merged_rows_cover_both_or_are_whole() {
        assert_eq!(merge_rows(Some(2..4), Some(7..9)), Some(2..9));
        assert_eq!(merge_rows(Some(2..8), Some(3..4)), Some(2..8));
        assert_eq!(merge_rows(None, Some(1..2)), None);
        assert_eq!(merge_rows(Some(1..2), None), None);
    }
    use std::io;
    use std::sync::mpsc;

    fn url(n: usize) -> HostToEngine {
        HostToEngine::Navigate { url: n.to_string(), referrer: None }
    }

    fn urls(msgs: &[HostToEngine]) -> Vec<usize> {
        msgs.iter()
            .filter_map(|m| match m {
                HostToEngine::Navigate { url, .. } => url.parse().ok(),
                _ => None,
            })
            .collect()
    }

    fn mouse(x: f64, modifiers: u8) -> HostToEngine {
        HostToEngine::Input(InputEvent::MouseMove { x, y: 0.0, modifiers })
    }

    fn scroll(delta_y: f64) -> HostToEngine {
        HostToEngine::Input(InputEvent::Scroll { x: 1.0, y: 2.0, delta_x: 0.0, delta_y, modifiers: 0 })
    }

    fn resize(width: u32) -> HostToEngine {
        HostToEngine::Resize { width, height: 10, dpi_scale: 1.0 }
    }

    fn drain(queue: &mut SendQueue) -> Vec<HostToEngine> {
        std::iter::from_fn(|| queue.pop()).collect()
    }

    #[test]
    fn messages_leave_in_the_order_they_were_sent() {
        let mut queue = SendQueue::default();
        for n in 0..5 {
            assert!(queue.push(url(n)));
        }
        assert_eq!(urls(&drain(&mut queue)), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_run_of_mouse_moves_or_resizes_keeps_only_the_latest() {
        let mut queue = SendQueue::default();
        for x in 0..10u8 {
            queue.push(mouse(f64::from(x), 0));
        }
        for width in 0..10 {
            queue.push(resize(width));
        }
        let out = drain(&mut queue);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], HostToEngine::Input(InputEvent::MouseMove { x, .. }) if x == 9.0));
        assert!(matches!(out[1], HostToEngine::Resize { width: 9, .. }));
    }

    #[test]
    fn a_run_of_scrolls_adds_up() {
        let mut queue = SendQueue::default();
        for _ in 0..4 {
            queue.push(scroll(-1.5));
        }
        let out = drain(&mut queue);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            HostToEngine::Input(InputEvent::Scroll { x, y, delta_y, .. }) if delta_y == -6.0 && x == 1.0 && y == 2.0
        ));
    }

    #[test]
    fn nothing_merges_across_another_message_or_a_modifier_change() {
        let mut queue = SendQueue::default();
        for msg in [mouse(1.0, 0), url(0), mouse(2.0, 0), mouse(3.0, 1), HostToEngine::Shutdown, resize(1), resize(2)] {
            queue.push(msg);
        }
        // Only the two resizes at the end merge.
        assert_eq!(drain(&mut queue).len(), 6);
    }

    #[test]
    fn a_full_queue_refuses_new_messages_but_still_merges_into_its_last() {
        let mut queue = SendQueue { items: VecDeque::new(), cap: 2 };
        assert!(queue.push(url(0)) && queue.push(mouse(1.0, 0)));
        assert!(!queue.push(url(1)), "full");
        assert!(queue.push(mouse(2.0, 0)), "a move still merges into the newest");
        let out = drain(&mut queue);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[1], HostToEngine::Input(InputEvent::MouseMove { x, .. }) if x == 2.0));
    }

    #[test]
    fn shutdown_goes_last_even_when_full() {
        let mut queue = SendQueue { items: VecDeque::new(), cap: 1 };
        assert!(queue.push(resize(1)));
        queue.push_last(HostToEngine::Shutdown);
        assert!(matches!(drain(&mut queue).as_slice(), [HostToEngine::Resize { .. }, HostToEngine::Shutdown]));
    }

    /// An engine end whose every write waits for a permit: no permits, a stuck engine; the sender dropped, a dead one.
    struct Gate {
        permits: mpsc::Receiver<()>,
        written: Arc<Mutex<Vec<u8>>>,
        panic_on_write: bool,
    }

    impl Write for Gate {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            assert!(!self.panic_on_write, "a writer bug");
            self.permits.recv().map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
            self.written.lock().unwrap_or_else(PoisonError::into_inner).extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for Gate {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }

    fn gated(panic_on_write: bool) -> (Writer, mpsc::Sender<()>, Arc<Mutex<Vec<u8>>>) {
        let (permit, permits) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let gate = Gate { permits, written: Arc::clone(&written), panic_on_write };
        (Writer::spawn(MessageChannel::new(gate), "test writer"), permit, written)
    }

    /// Every message in `bytes`, in order.
    fn decode(bytes: Vec<u8>) -> Vec<HostToEngine> {
        let mut channel = MessageChannel::new(io::Cursor::new(bytes));
        std::iter::from_fn(|| channel.recv().ok()).collect()
    }

    /// Polls `done` until it holds or `within` passes.
    fn eventually(within: Duration, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        done()
    }

    #[test]
    fn sends_never_block_and_a_write_stuck_for_unresponsive_after_is_reported() {
        let (writer, _permit, _) = gated(false);
        let start = Instant::now();
        for n in 0..10 {
            writer.send(&url(n));
        }
        assert!(start.elapsed() < Duration::from_secs(1), "ten sends to a stuck engine returned at once");
        assert!(
            eventually(Duration::from_secs(5), || writer.is_unresponsive(Instant::now() + UNRESPONSIVE_AFTER)),
            "the first write is under way and stuck"
        );
        assert!(!writer.is_unresponsive(Instant::now()), "not before UNRESPONSIVE_AFTER: the queue is not full");
    }

    #[test]
    fn a_burst_past_the_cap_to_an_engine_that_keeps_reading_is_not_unresponsive() {
        let (writer, permit, _) = gated(false);
        let sent = SEND_QUEUE_CAP + 10;
        for n in 0..sent {
            writer.send(&url(n));
        }
        // Some pushes were refused, but the write in flight has only just started.
        assert!(!writer.is_unresponsive(Instant::now()), "a refusal alone is not enough");
        for _ in 0..sent + 2 {
            permit.send(()).unwrap();
        }
        assert!(eventually(Duration::from_secs(5), || !writer.is_unresponsive(Instant::now())));
        assert!(!writer.take_newly_unresponsive(Instant::now()), "no toast for a burst that drained");
    }

    #[test]
    fn a_full_queue_is_unresponsive_once_its_write_has_been_blocked_and_recovers_in_order() {
        let (writer, permit, written) = gated(false);
        let sent = SEND_QUEUE_CAP + 10;
        for n in 0..sent {
            writer.send(&url(n));
        }
        assert!(
            eventually(Duration::from_secs(5), || writer.is_unresponsive(Instant::now() + REFUSED_BLOCKED_AFTER)),
            "a refused message plus a write blocked for REFUSED_BLOCKED_AFTER"
        );
        for _ in 0..sent + 2 {
            permit.send(()).unwrap();
        }
        assert!(eventually(Duration::from_secs(5), || !writer.is_unresponsive(Instant::now())), "a write clears it");
        writer.send(&url(sent));
        writer.shutdown();
        assert!(writer.wait_finished(Duration::from_secs(5)));
        let out = decode(std::mem::take(&mut *written.lock().unwrap_or_else(PoisonError::into_inner)));
        let got = urls(&out);
        assert!(got.windows(2).all(|w| w[0] < w[1]), "order kept: {got:?}");
        assert!(got.len() < sent, "some were refused while full");
        assert_eq!(got.last(), Some(&sent), "a message sent after the recovery arrives");
        assert!(matches!(out.last(), Some(HostToEngine::Shutdown)));
    }

    #[test]
    fn a_stall_is_reported_once_until_a_write_completes() {
        let (writer, permit, _) = gated(false);
        for n in 0..SEND_QUEUE_CAP + 2 {
            writer.send(&url(n));
        }
        let later = || Instant::now() + REFUSED_BLOCKED_AFTER;
        assert!(eventually(Duration::from_secs(5), || writer.is_unresponsive(later())), "the first write is under way");
        assert!(writer.take_newly_unresponsive(later()));
        assert!(!writer.take_newly_unresponsive(later()), "once per stall");
        permit.send(()).unwrap();
        assert!(eventually(Duration::from_secs(5), || !writer.is_unresponsive(Instant::now())));
        for n in 0..SEND_QUEUE_CAP + 2 {
            writer.send(&url(n)); // the writer is stuck on its next write again, and the queue refills
        }
        let later = || Instant::now() + REFUSED_BLOCKED_AFTER;
        assert!(eventually(Duration::from_secs(5), || writer.is_unresponsive(later())), "the next write is under way");
        assert!(writer.take_newly_unresponsive(later()), "a new stall is reported again");
    }

    #[test]
    fn closing_waits_at_most_its_deadline() {
        let (writer, permit, _) = gated(false);
        writer.send(&url(0));
        writer.shutdown();
        let start = Instant::now();
        assert!(!writer.wait_finished(Duration::from_millis(100)), "the engine is stuck");
        assert!(start.elapsed() < Duration::from_secs(2));
        writer.send(&url(1)); // after shutdown: dropped, and never blocks
        drop(permit);
        assert!(writer.wait_finished(Duration::from_secs(5)), "the write failed, so the thread ended");
    }

    #[test]
    fn a_dead_engine_ends_the_writer_and_is_not_unresponsive() {
        let (writer, permit, _) = gated(false);
        writer.send(&url(0));
        // The engine died: the write fails. No `shutdown`: `wait_finished` then waits `flush` from now.
        drop(permit);
        assert!(writer.wait_finished(Duration::from_secs(5)), "the write failed, so the thread ended");
        assert!(
            !writer.is_unresponsive(Instant::now() + UNRESPONSIVE_AFTER),
            "a dead engine is a crash for its reader to report, not a stuck one"
        );
        writer.send(&url(1)); // a finished writer drops it
    }

    #[test]
    fn a_writer_that_panicked_is_unresponsive() {
        let (writer, _permit, _) = gated(true);
        writer.send(&url(0));
        assert!(eventually(Duration::from_secs(5), || writer.is_unresponsive(Instant::now())));
    }

    #[test]
    fn the_frame_map_reuses_reopens_and_retries() {
        let id = format!("framemap-test-{}", std::process::id());
        let mut map = FrameMap::default();
        let mut first = SharedFrameBuffer::create(&id, 1, 2, 2).unwrap();
        first.as_mut_slice().fill(7);
        assert_eq!(map.pixels(&id, 1, 2, 2).unwrap(), &[7; 16][..]);
        // Reused: the file is gone, the mapping answers.
        ipc_protocol::remove_frame_file(&id);
        assert_eq!(map.pixels(&id, 1, 2, 2).unwrap(), &[7; 16][..]);
        // A new generation reopens.
        let mut second = SharedFrameBuffer::create(&id, 2, 2, 2).unwrap();
        second.as_mut_slice().fill(9);
        assert_eq!(map.pixels(&id, 2, 2, 2).unwrap(), &[9; 16][..]);
        // A failed open leaves the cache empty (generation 2 is not served again), and the next call retries.
        assert!(map.pixels(&id, 3, 2, 2).is_err());
        ipc_protocol::remove_frame_file(&id);
        assert!(map.pixels(&id, 2, 2, 2).is_err(), "the failed open dropped the old mapping");
        let mut third = SharedFrameBuffer::create(&id, 3, 2, 2).unwrap();
        third.as_mut_slice().fill(5);
        assert_eq!(map.pixels(&id, 3, 2, 2).unwrap(), &[5; 16][..]);
        map.clear();
        drop((first, second, third));
        ipc_protocol::remove_frame_file(&id);
    }
}
