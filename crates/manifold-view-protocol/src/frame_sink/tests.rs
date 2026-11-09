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

#[test]
fn frame_label_tells_a_frame_from_a_poll() {
    let frame = ViewToHost::FrameReady { generation: 0, width: 1, height: 1, damage: None, pixels: None };
    assert_eq!(frame_label(&Some(frame)), "1");
    assert_eq!(frame_label(&None), "0");
}
