//! Test double for a view process: behaves as its first argument says.
use std::io::Write;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let frame = ipc_protocol::render::Frame { width: 2, height: 1, page_count: 3, rgba: vec![1, 2, 3, 4, 5, 6, 7, 8] };
    match mode.as_str() {
        "ok" => ipc_protocol::render::write_frame(&mut std::io::stdout(), &frame).unwrap(),
        "fail" => {
            eprintln!("noise line\nCouldn't read the file");
            std::process::exit(1);
        }
        "crash" => std::process::exit(0xC0000005u32 as i32),
        "garbage" => std::io::stdout().write_all(b"this is not a frame at all......").unwrap(),
        "short" => {
            let mut bytes = Vec::new();
            ipc_protocol::render::write_frame(&mut bytes, &frame).unwrap();
            std::io::stdout().write_all(&bytes[..bytes.len() - 3]).unwrap();
        }
        "hang" => loop {
            std::thread::park();
        },
        "linger" => {
            ipc_protocol::render::write_frame(&mut std::io::stdout(), &frame).unwrap();
            loop {
                std::thread::park();
            }
        }
        other => panic!("unknown mode {other}"),
    }
}
