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

fn url(n: usize) -> HostToView {
    HostToView::Navigate { url: n.to_string(), referrer: None }
}

fn urls(msgs: &[HostToView]) -> Vec<usize> {
    msgs.iter()
        .filter_map(|m| match m {
            HostToView::Navigate { url, .. } => url.parse().ok(),
            _ => None,
        })
        .collect()
}

fn mouse(x: f64, modifiers: u8) -> HostToView {
    HostToView::Input(InputEvent::MouseMove { x, y: 0.0, modifiers })
}

fn scroll(delta_y: f64) -> HostToView {
    HostToView::Input(InputEvent::Scroll { x: 1.0, y: 2.0, delta_x: 0.0, delta_y, modifiers: 0 })
}

fn resize(width: u32) -> HostToView {
    HostToView::Resize { width, height: 10, dpi_scale: 1.0 }
}

fn drain(queue: &mut SendQueue) -> Vec<HostToView> {
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
    assert!(matches!(out[0], HostToView::Input(InputEvent::MouseMove { x, .. }) if x == 9.0));
    assert!(matches!(out[1], HostToView::Resize { width: 9, .. }));
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
        HostToView::Input(InputEvent::Scroll { x, y, delta_y, .. }) if delta_y == -6.0 && x == 1.0 && y == 2.0
    ));
}

#[test]
fn nothing_merges_across_another_message_or_a_modifier_change() {
    let mut queue = SendQueue::default();
    for msg in [mouse(1.0, 0), url(0), mouse(2.0, 0), mouse(3.0, 1), HostToView::Shutdown, resize(1), resize(2)] {
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
    assert!(matches!(out[1], HostToView::Input(InputEvent::MouseMove { x, .. }) if x == 2.0));
}

#[test]
fn shutdown_goes_last_even_when_full() {
    let mut queue = SendQueue { items: VecDeque::new(), cap: 1 };
    assert!(queue.push(resize(1)));
    queue.push_last(HostToView::Shutdown);
    assert!(matches!(drain(&mut queue).as_slice(), [HostToView::Resize { .. }, HostToView::Shutdown]));
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
fn decode(bytes: Vec<u8>) -> Vec<HostToView> {
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
fn a_burst_past_the_cap_to_a_view_that_keeps_reading_is_not_unresponsive() {
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
    // That holds at once (the first write is young), so wait for the queue to drain: sent into a full queue, the
    // message below would be refused too.
    let drained = || {
        let bytes = written.lock().unwrap_or_else(PoisonError::into_inner).clone();
        urls(&decode(bytes)).len() >= SEND_QUEUE_CAP
    };
    assert!(eventually(Duration::from_secs(5), drained), "the queue drains");
    writer.send(&url(sent));
    writer.shutdown();
    assert!(writer.wait_finished(Duration::from_secs(5)));
    let out = decode(std::mem::take(&mut *written.lock().unwrap_or_else(PoisonError::into_inner)));
    let got = urls(&out);
    assert!(got.windows(2).all(|w| w[0] < w[1]), "order kept: {got:?}");
    assert!(got.len() < sent, "some were refused while full");
    assert_eq!(got.last(), Some(&sent), "a message sent after the recovery arrives");
    assert!(matches!(out.last(), Some(HostToView::Shutdown)));
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
    // The old write started before `probe`, a new one after it: only the old one is blocked for
    // `UNRESPONSIVE_AFTER` at `probe + UNRESPONSIVE_AFTER`. (`!is_unresponsive(now)` would hold at once, the old
    // write being young, and the refill below could race the writer's reset of `reported`.)
    let probe = Instant::now();
    permit.send(()).unwrap();
    assert!(eventually(Duration::from_secs(5), || !writer.is_unresponsive(probe + UNRESPONSIVE_AFTER)));
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
fn a_dead_view_ends_the_writer_and_is_not_unresponsive() {
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
    manifold_view_protocol::remove_frame_file(&id);
    assert_eq!(map.pixels(&id, 1, 2, 2).unwrap(), &[7; 16][..]);
    // A new generation reopens.
    let mut second = SharedFrameBuffer::create(&id, 2, 2, 2).unwrap();
    second.as_mut_slice().fill(9);
    assert_eq!(map.pixels(&id, 2, 2, 2).unwrap(), &[9; 16][..]);
    // A failed open leaves the cache empty (generation 2 is not served again), and the next call retries.
    assert!(map.pixels(&id, 3, 2, 2).is_err());
    manifold_view_protocol::remove_frame_file(&id);
    assert!(map.pixels(&id, 2, 2, 2).is_err(), "the failed open dropped the old mapping");
    let mut third = SharedFrameBuffer::create(&id, 3, 2, 2).unwrap();
    third.as_mut_slice().fill(5);
    assert_eq!(map.pixels(&id, 3, 2, 2).unwrap(), &[5; 16][..]);
    map.clear();
    drop((first, second, third));
    manifold_view_protocol::remove_frame_file(&id);
}
