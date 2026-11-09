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

    /// The `FrameReady` to send now, if anything changed since the last one. With `host_env::frame_stats()` on, each
    /// call logs `[frame] 1` or `[frame] 0`, so `gate hostrate` can count frames and polls.
    pub fn take(&mut self) -> Option<ViewToHost> {
        let frame = self.take_inner();
        if crate::host_env::frame_stats() {
            eprintln!("[frame] {}", frame_label(&frame));
        }
        frame
    }

    fn take_inner(&mut self) -> Option<ViewToHost> {
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

/// The `[frame]` line's label for what `take` returned: `1` for a frame sent, `0` for a poll that found nothing.
fn frame_label(frame: &Option<ViewToHost>) -> &'static str {
    if frame.is_some() {
        "1"
    } else {
        "0"
    }
}

#[cfg(test)]
mod tests;
