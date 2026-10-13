use std::process::Command;

fn run(path: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_manifold-view-pdfium")).arg(path).output().unwrap()
}

#[test]
fn the_fixture_renders_to_a_frame() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture.pdf");
    let out = run(&path);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let frame = ipc_protocol::render::read_frame(&mut out.stdout.as_slice()).unwrap();
    assert_eq!((frame.width, frame.height, frame.page_count), (1000, 500, 1));
    assert!(frame.rgba.chunks(4).any(|px| px[2] > 200 && px[0] < 60), "the blue rectangle is there");
}

#[test]
fn a_garbage_file_exits_one_with_one_line() {
    let dir = std::env::temp_dir().join(format!("view-pdfium-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.pdf");
    std::fs::write(&path, b"%PDF-1.4\nnot really").unwrap();
    let out = run(&path);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim().lines().count(), 1);
}
