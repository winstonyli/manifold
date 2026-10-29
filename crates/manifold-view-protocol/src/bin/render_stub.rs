//! Test double for a view process: behaves as its first argument says.
use manifold_view_protocol::render::{HEADER_LEN, RENDER_PROTOCOL_VERSION};
use std::io::Write;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let frame = manifold_view_protocol::render::Frame {
        width: 2,
        height: 1,
        page_count: 3,
        rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
    };
    match mode.as_str() {
        "ok" => manifold_view_protocol::render::write_frame(&mut std::io::stdout(), &frame).unwrap(),
        "fail" => {
            eprintln!("noise line\nCouldn't read the file");
            std::process::exit(1);
        }
        "crash" => std::process::exit(0xC0000005u32 as i32),
        "garbage" => std::io::stdout().write_all(b"this is not a frame at all......").unwrap(),
        "short" => {
            let mut bytes = Vec::new();
            manifold_view_protocol::render::write_frame(&mut bytes, &frame).unwrap();
            std::io::stdout().write_all(&bytes[..bytes.len() - 3]).unwrap();
        }
        "hang" => loop {
            std::thread::park();
        },
        "linger" => {
            manifold_view_protocol::render::write_frame(&mut std::io::stdout(), &frame).unwrap();
            loop {
                std::thread::park();
            }
        }
        // A frame of an older render protocol. "mismatch" makes it larger than a pipe buffer: the app stops reading
        // after the header, so the rest can't be written, and this exits 1 the way pdfium does ("couldn't write the
        // frame"). "mismatch-linger" writes the header alone and keeps running, so only a kill ends it soon; it gives
        // up after 20 s, so a run_child that waits for it fails a test instead of hanging it.
        "mismatch" | "mismatch-linger" => {
            let mut bytes = b"MRND".to_vec();
            for field in [RENDER_PROTOCOL_VERSION - 1, 1024, 1024, 1] {
                bytes.extend_from_slice(&field.to_le_bytes());
            }
            if mode == "mismatch" {
                bytes.resize(HEADER_LEN + 4 * 1024 * 1024, 0x7f);
            }
            let mut out = std::io::stdout().lock();
            if let Err(e) = out.write_all(&bytes).and_then(|()| out.flush()) {
                eprintln!("couldn't write the frame: {e}");
                std::process::exit(1);
            }
            if mode == "mismatch-linger" {
                std::thread::sleep(std::time::Duration::from_secs(20));
            }
        }
        other => panic!("unknown mode {other}"),
    }
}
