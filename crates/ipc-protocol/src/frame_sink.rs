use std::ops::Range;

use crate::{SharedFrameBuffer, ViewToHost};

/// Copies `src` (`dst.len() / row_bytes` rows, `pitch` bytes apart) over `dst` (the same rows, tightly packed),
/// writing only the rows that differ. Returns the span of rows that differed, or `None` if none did.
pub fn copy_changed_rows(dst: &mut [u8], src: &[u8], row_bytes: usize, pitch: usize) -> Option<Range<u32>> {
    let mut span: Option<Range<u32>> = None;
    for (y, row) in dst.chunks_exact_mut(row_bytes).enumerate() {
        let s = &src[y * pitch..y * pitch + row_bytes];
        if row != s {
            row.copy_from_slice(s);
            let y = y as u32;
            span = Some(span.map_or(y..y + 1, |r| r.start..y + 1));
        }
    }
    span
}

/// Rows read on each side of an engine's row hint (CEF's rect fell 1 px outside the changed rows now and then).
pub const HINT_PAD: u32 = 8;
/// Every this-many'th hinted offer compares the whole frame, bounding a row the hint missed to that many offers.
pub const HINT_FULL_EVERY: u32 = 30;

/// The rows to compare for a hint on a `height`-row frame: the hint clamped to the frame and padded by `HINT_PAD`
/// each side. `None` for an empty, reversed or wholly out-of-frame hint, which means compare everything.
fn hint_window(hint: &Range<u32>, height: u32) -> Option<Range<u32>> {
    let (start, end) = (hint.start.min(height), hint.end.min(height));
    (start < end).then(|| start.saturating_sub(HINT_PAD)..end.saturating_add(HINT_PAD).min(height))
}

enum Store {
    Shared(SharedFrameBuffer),
    Inline(Vec<u8>),
}

impl Store {
    fn bytes_mut(&mut self) -> &mut [u8] {
        match self {
            Store::Shared(buf) => buf.as_mut_slice(),
            Store::Inline(vec) => vec,
        }
    }
}

/// A frame the shared store could not be created for yet, tightly packed.
struct Pending {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
}

/// An engine's frame store and what it has not yet reported (survey E2). Every engine offers each frame it gets
/// and sends what `take` returns; unchanged frames cost a compare and nothing else.
pub struct FrameSink {
    /// `Some(socket_id)` for a shared frame file, `None` for an in-memory store.
    socket_id: Option<String>,
    store: Option<Store>,
    width: u32,
    height: u32,
    /// Rows written since the last `take`; meaningless while `whole`.
    changed: Option<Range<u32>>,
    /// The next report covers the whole frame.
    whole: bool,
    /// The generation of the current shared frame file; 0 before the first create and always for inline.
    generation: u32,
    pending: Option<Pending>,
    create_failure_logged: bool,
    /// Hinted offers since the last one that ignored its hint, modulo `HINT_FULL_EVERY`.
    hinted_offers: u32,
}

impl FrameSink {
    /// A sink writing the shared frame file of `socket_id` (`frame_path`), created at the first offered size.
    pub fn shared(socket_id: &str) -> Self {
        Self::new(Some(socket_id.to_owned()))
    }

    /// A sink keeping frames in memory and sending them inline (WebKit over TCP).
    pub fn inline() -> Self {
        Self::new(None)
    }

    fn new(socket_id: Option<String>) -> Self {
        Self {
            socket_id,
            store: None,
            width: 0,
            height: 0,
            changed: None,
            whole: false,
            generation: 0,
            pending: None,
            create_failure_logged: false,
            hinted_offers: 0,
        }
    }

    /// Offers a frame of `width` x `height` BGRA/RGBA rows `pitch` bytes apart. Returns false (and keeps nothing)
    /// for a malformed frame: zero size, `pitch < width * 4`, or `src` shorter than `pitch * (height - 1) + width * 4`.
    pub fn offer(&mut self, src: &[u8], pitch: usize, width: u32, height: u32) -> bool {
        self.offer_hinted(src, pitch, width, height, None)
    }

    /// `offer` with the engine's claim of which rows changed. Against a same-size store only the hint's rows plus
    /// `HINT_PAD` each side are read, so the rest of `src` is never touched (a cold engine buffer costs a page fault
    /// per page read). Every `HINT_FULL_EVERY`th hinted offer ignores the hint, since the engine can be wrong. A new
    /// or resized store takes the whole frame regardless; an empty or out-of-frame hint is no hint.
    pub fn offer_hinted(
        &mut self,
        src: &[u8],
        pitch: usize,
        width: u32,
        height: u32,
        hint: Option<Range<u32>>,
    ) -> bool {
        let row_bytes = width as usize * 4;
        if width == 0 || height == 0 || pitch < row_bytes || src.len() < pitch * (height as usize - 1) + row_bytes {
            return false;
        }
        let same_size = (self.width, self.height) == (width, height);
        if let (Some(store), true) = (self.store.as_mut(), same_size) {
            let window = hint.and_then(|h| hint_window(&h, height)).filter(|_| {
                self.hinted_offers = (self.hinted_offers + 1) % HINT_FULL_EVERY;
                self.hinted_offers != 0
            });
            let start = window.as_ref().map_or(0, |w| w.start);
            let end = window.as_ref().map_or(height, |w| w.end);
            let dst = &mut store.bytes_mut()[start as usize * row_bytes..end as usize * row_bytes];
            if let Some(rows) = copy_changed_rows(dst, &src[start as usize * pitch..], row_bytes, pitch) {
                let rows = rows.start + start..rows.end + start;
                let span = self.changed.take().map_or(rows.clone(), |c| c.start.min(rows.start)..c.end.max(rows.end));
                self.changed = Some(span);
            }
            return true;
        }
        // A new or resized store: keep this frame tightly packed until a store exists to take it.
        let mut bytes = Vec::with_capacity(row_bytes * height as usize);
        for y in 0..height as usize {
            bytes.extend_from_slice(&src[y * pitch..y * pitch + row_bytes]);
        }
        self.pending = Some(Pending { bytes, width, height });
        self.install_pending();
        true
    }

    /// Creates the store for the pending frame and writes it in; leaves it pending if the create fails.
    fn install_pending(&mut self) {
        let Some(pending) = self.pending.take() else { return };
        let store = match &self.socket_id {
            Some(id) => match SharedFrameBuffer::create(id, self.generation + 1, pending.width, pending.height) {
                Ok(buf) => {
                    // The previous generation's file goes (best effort); a reader still mapping it keeps its view.
                    self.store = None;
                    if self.generation > 0 {
                        let _ = std::fs::remove_file(crate::frame_path(id, self.generation));
                    }
                    self.generation += 1;
                    Store::Shared(buf)
                }
                Err(err) => {
                    // The create failed (a full disk, say): `take` retries.
                    if !self.create_failure_logged {
                        eprintln!(
                            "frame buffer: create at {}x{} failed, retrying each frame: {err}",
                            pending.width, pending.height
                        );
                        self.create_failure_logged = true;
                    }
                    self.pending = Some(pending);
                    return;
                }
            },
            None => Store::Inline(vec![0; pending.bytes.len()]),
        };
        let mut store = store;
        store.bytes_mut().copy_from_slice(&pending.bytes);
        self.store = Some(store);
        self.width = pending.width;
        self.height = pending.height;
        // Compared against nothing: the changed span means nothing, the whole frame is new.
        self.whole = true;
        self.changed = None;
    }

    /// The tab was shown: the next `take` reports the whole stored frame, changed or not.
    pub fn show(&mut self) {
        self.whole = true;
    }

    /// The `FrameReady` to send now, if anything changed since the last one.
    pub fn take(&mut self) -> Option<ViewToHost> {
        self.install_pending();
        let store = self.store.as_ref()?;
        let damage = if self.whole {
            None
        } else {
            let rows = self.changed.take()?;
            Some((0, rows.start, self.width, rows.end - rows.start))
        };
        self.whole = false;
        self.changed = None;
        let pixels = match store {
            Store::Inline(vec) => Some(vec.clone()),
            Store::Shared(_) => None,
        };
        Some(ViewToHost::FrameReady {
            generation: self.generation,
            width: self.width,
            height: self.height,
            damage,
            pixels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{frame_path, remove_frame_file, SharedFrameBuffer};

    fn frame(w: usize, h: usize, fill: u8) -> Vec<u8> {
        vec![fill; w * h * 4]
    }

    fn damage_of(msg: Option<ViewToHost>) -> Option<Option<(u32, u32, u32, u32)>> {
        match msg? {
            ViewToHost::FrameReady { damage, .. } => Some(damage),
            other => panic!("not a FrameReady: {other:?}"),
        }
    }

    #[test]
    fn copy_changed_rows_reports_the_changed_span() {
        let (row, h) = (8usize, 10usize);
        let mut dst = vec![0u8; row * h];
        let mut src = dst.clone();
        assert_eq!(copy_changed_rows(&mut dst, &src, row, row), None);
        src[3 * row] = 1;
        src[7 * row + 2] = 9;
        assert_eq!(copy_changed_rows(&mut dst, &src, row, row), Some(3..8));
        assert_eq!(dst, src);
        assert_eq!(copy_changed_rows(&mut dst, &src, row, row), None);

        // A pitched source: only `row` bytes of each `pitch` are copied; the padding is ignored.
        let pitch = row + 4;
        let mut pitched = vec![0xEEu8; pitch * (h - 1) + row];
        for y in 0..h {
            pitched[y * pitch..y * pitch + row].fill(y as u8 + 1);
        }
        let mut dst = vec![0u8; row * h];
        assert_eq!(copy_changed_rows(&mut dst, &pitched, row, pitch), Some(0..10));
        for y in 0..h {
            assert!(dst[y * row..(y + 1) * row].iter().all(|&b| b == y as u8 + 1), "row {y}");
        }
    }

    #[test]
    fn hint_window_pads_and_clamps() {
        assert_eq!(hint_window(&(20..30), 100), Some(20 - HINT_PAD..30 + HINT_PAD));
        assert_eq!(hint_window(&(2..5), 100), Some(0..5 + HINT_PAD), "clamped at the top");
        assert_eq!(hint_window(&(95..99), 100), Some(95 - HINT_PAD..100), "clamped at the bottom");
        assert_eq!(hint_window(&(5..1000), 10), Some(0..10), "a hint past the frame clamps");
        assert_eq!(hint_window(&(1000..2000), 10), None, "wholly outside: no hint");
        assert_eq!(hint_window(&(7..7), 10), None, "empty: no hint");
        #[allow(clippy::reversed_empty_ranges, reason = "the case under test")]
        let reversed = 9..3;
        assert_eq!(hint_window(&reversed, 10), None, "reversed: no hint");
    }

    /// A 1-column sink of `h` rows (one pixel per row) already holding `base`.
    fn hinted_sink(h: usize, base: &[u8]) -> FrameSink {
        let mut sink = FrameSink::inline();
        assert!(sink.offer(base, 4, 1, h as u32));
        assert!(sink.take().is_some());
        sink
    }

    fn rows(h: usize, fill: u8) -> Vec<u8> {
        frame(1, h, fill)
    }

    fn row_span(msg: Option<ViewToHost>) -> Option<(u32, u32)> {
        damage_of(msg).map(|d| d.map(|(_, y, _, n)| (y, n)).expect("not whole"))
    }

    #[test]
    fn a_hinted_offer_reads_only_the_hint_and_its_pad() {
        let h = 200usize;
        let mut sink = hinted_sink(h, &rows(h, 1));
        let mut px = rows(h, 1);
        px[50 * 4] = 9; // inside the hint
        px[150 * 4] = 9; // far outside hint + pad: must not be read
        assert!(sink.offer_hinted(&px, 4, 1, h as u32, Some(48..53)));
        assert_eq!(row_span(sink.take()), Some((50, 1)), "only the hinted change is reported");
        // The far row was never read: the store still holds the old byte (another offer, unhinted, finds it).
        assert!(sink.offer(&px, 4, 1, h as u32));
        assert_eq!(row_span(sink.take()), Some((150, 1)));
    }

    #[test]
    fn a_change_within_the_pad_is_still_found() {
        let h = 200usize;
        let mut sink = hinted_sink(h, &rows(h, 1));
        let mut px = rows(h, 1);
        px[(50 - HINT_PAD as usize) * 4] = 9; // HINT_PAD rows above the hint
        px[(52 + HINT_PAD as usize) * 4] = 9; // HINT_PAD rows below its last row (52)
        assert!(sink.offer_hinted(&px, 4, 1, h as u32, Some(50..53)));
        let (top, bottom) = (50 - HINT_PAD, 52 + HINT_PAD);
        assert_eq!(row_span(sink.take()), Some((top, bottom - top + 1)));
    }

    #[test]
    fn every_nth_hinted_offer_compares_everything() {
        let h = 200usize;
        let mut sink = hinted_sink(h, &rows(h, 1));
        let mut px = rows(h, 1);
        px[150 * 4] = 9; // far outside the hint
        for n in 1..HINT_FULL_EVERY {
            assert!(sink.offer_hinted(&px, 4, 1, h as u32, Some(10..12)), "offer {n}");
            assert!(sink.take().is_none(), "offer {n} must not see the far row");
        }
        assert!(sink.offer_hinted(&px, 4, 1, h as u32, Some(10..12)));
        assert_eq!(row_span(sink.take()), Some((150, 1)), "the full compare finds it");
        // The count starts over.
        px[160 * 4] = 9;
        assert!(sink.offer_hinted(&px, 4, 1, h as u32, Some(10..12)));
        assert!(sink.take().is_none());
    }

    #[test]
    fn a_hint_on_a_new_or_resized_store_stores_the_whole_frame() {
        let mut sink = FrameSink::inline();
        let px: Vec<u8> = (0..40u8).collect(); // 1 x 10
        assert!(sink.offer_hinted(&px, 4, 1, 10, Some(4..5)));
        match sink.take() {
            Some(ViewToHost::FrameReady { damage: None, pixels: Some(p), .. }) => assert_eq!(p, px),
            other => panic!("{other:?}"),
        }
        let big: Vec<u8> = (0..80u8).collect(); // 1 x 20
        assert!(sink.offer_hinted(&big, 4, 1, 20, Some(4..5)));
        match sink.take() {
            Some(ViewToHost::FrameReady { damage: None, pixels: Some(p), height: 20, .. }) => assert_eq!(p, big),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_hint_beyond_the_frame_clamps() {
        let h = 20usize;
        let mut sink = hinted_sink(h, &rows(h, 1));
        let mut px = rows(h, 1);
        px[19 * 4] = 9;
        assert!(sink.offer_hinted(&px, 4, 1, h as u32, Some(15..5000)));
        assert_eq!(row_span(sink.take()), Some((19, 1)));
        px[2 * 4] = 9;
        assert!(sink.offer_hinted(&px, 4, 1, h as u32, Some(5000..6000)), "wholly outside: compares everything");
        assert_eq!(row_span(sink.take()), Some((2, 1)));
    }

    #[test]
    fn an_unhinted_offer_still_compares_everything() {
        let h = 200usize;
        let mut sink = hinted_sink(h, &rows(h, 1));
        let mut px = rows(h, 1);
        px[3 * 4] = 9;
        px[180 * 4] = 9;
        assert!(sink.offer(&px, 4, 1, h as u32));
        assert_eq!(row_span(sink.take()), Some((3, 178)));
    }

    #[test]
    fn an_inline_sink_reports_whole_then_bands_then_nothing() {
        let (w, h) = (4usize, 6usize);
        let mut sink = FrameSink::inline();
        let mut px = frame(w, h, 1);
        assert!(sink.offer(&px, w * 4, w as u32, h as u32));
        match sink.take() {
            Some(ViewToHost::FrameReady { damage: None, pixels: Some(p), width: 4, height: 6, .. }) => {
                assert_eq!(p, px)
            }
            other => panic!("{other:?}"),
        }
        assert!(sink.offer(&px, w * 4, w as u32, h as u32));
        assert!(sink.take().is_none(), "unchanged");
        px[2 * w * 4] = 9;
        assert!(sink.offer(&px, w * 4, w as u32, h as u32));
        assert_eq!(damage_of(sink.take()), Some(Some((0, 2, 4, 1))));
        // Two offers between takes report the union.
        px[w * 4] = 5;
        sink.offer(&px, w * 4, w as u32, h as u32);
        px[4 * w * 4] = 5;
        sink.offer(&px, w * 4, w as u32, h as u32);
        assert_eq!(damage_of(sink.take()), Some(Some((0, 1, 4, 4))));
        // A new size is whole.
        let px2 = frame(2, 2, 3);
        assert!(sink.offer(&px2, 8, 2, 2));
        assert_eq!(damage_of(sink.take()), Some(None));
        // Shown: whole again with no new offer, once.
        sink.show();
        assert_eq!(damage_of(sink.take()), Some(None));
        assert!(sink.take().is_none());
    }

    #[test]
    fn show_waits_for_a_frame() {
        let mut sink = FrameSink::inline();
        sink.show();
        assert!(sink.take().is_none(), "no stored frame yet");
        let px = frame(2, 2, 1);
        sink.offer(&px, 8, 2, 2);
        assert_eq!(generation_of(sink.take()), 0, "an inline sink has no file");
        sink.show();
        assert_eq!(damage_of(sink.take()), Some(None));
    }

    #[test]
    fn malformed_frames_are_refused() {
        let mut sink = FrameSink::inline();
        let px = frame(4, 4, 1);
        assert!(!sink.offer(&px, 16, 0, 4));
        assert!(!sink.offer(&px, 16, 4, 0));
        assert!(!sink.offer(&px[..px.len() - 1], 16, 4, 4), "short src");
        assert!(!sink.offer(&px, 15, 4, 4), "pitch < width * 4");
        assert!(sink.take().is_none(), "nothing kept");
    }

    #[test]
    fn a_shared_sink_round_trips_through_the_frame_file() {
        let id = format!("sink-test-{}", std::process::id());
        let mut sink = FrameSink::shared(&id);
        let px: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
        assert!(sink.offer(&px, 16, 4, 3));
        match sink.take() {
            Some(ViewToHost::FrameReady { damage: None, pixels: None, width: 4, height: 3, .. }) => {}
            other => panic!("{other:?}"),
        }
        let reader = SharedFrameBuffer::open_existing(frame_path(&id, 1), 4, 3).unwrap();
        assert_eq!(reader.as_slice(), &px[..]);
        drop(reader);
        drop(sink);
        remove_frame_file(&id);
    }

    fn generation_of(msg: Option<ViewToHost>) -> u32 {
        match msg {
            Some(ViewToHost::FrameReady { generation, .. }) => generation,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn generations_follow_the_size_and_old_files_go() {
        let id = format!("sink-gen-{}", std::process::id());
        let mut sink = FrameSink::shared(&id);
        assert!(sink.offer(&frame(2, 2, 1), 8, 2, 2));
        assert_eq!(generation_of(sink.take()), 1);
        assert!(frame_path(&id, 1).exists());
        assert!(sink.offer(&frame(2, 2, 2), 8, 2, 2));
        assert_eq!(generation_of(sink.take()), 1, "same size, same file");
        assert!(sink.offer(&frame(4, 4, 3), 16, 4, 4));
        assert_eq!(generation_of(sink.take()), 2);
        assert!(!frame_path(&id, 1).exists(), "the old generation's file is gone");
        assert!(frame_path(&id, 2).exists());
        sink.show();
        assert_eq!(generation_of(sink.take()), 2);
        drop(sink);
        remove_frame_file(&id);
    }

    #[test]
    fn a_resize_does_not_wait_for_a_reader() {
        let id = format!("sink-resize-{}", std::process::id());
        let mut sink = FrameSink::shared(&id);
        assert!(sink.offer(&frame(2, 2, 1), 8, 2, 2));
        assert!(sink.take().is_some());
        let held = SharedFrameBuffer::open_existing(frame_path(&id, 1), 2, 2).unwrap();
        let big: Vec<u8> = (0..4 * 4 * 4).map(|i| i as u8).collect();
        assert!(sink.offer(&big, 16, 4, 4));
        match sink.take() {
            Some(ViewToHost::FrameReady { damage: None, width: 4, height: 4, generation: 2, .. }) => {}
            other => panic!("{other:?}"),
        }
        let reader = SharedFrameBuffer::open_existing(frame_path(&id, 2), 4, 4).unwrap();
        assert_eq!(reader.as_slice(), &big[..]);
        drop((held, reader, sink));
        remove_frame_file(&id);
    }
}
