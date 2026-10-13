//! The one-shot render protocol: a view process (`manifold-view-pdfium`) renders one file and writes one frame to
//! stdout; the app reads it with `run_child`. Not the engine control channel: a render is a function from a path to a
//! bitmap, with no input, navigation or streaming.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Bump when the frame layout changes. Separate from `PROTOCOL_VERSION`: engines and renderers move independently.
pub const RENDER_PROTOCOL_VERSION: u32 = 1;
const MAGIC: [u8; 4] = *b"MRND";
/// magic + version + width + height + page count.
pub const HEADER_LEN: usize = 20;
/// A bound on what a child may claim, so a corrupt header can't make the app allocate gigabytes.
pub const MAX_FRAME_SIDE: u32 = 16_384;

/// `Debug` because the tests call `unwrap_err` on `Result<Frame, _>`.
#[derive(Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub page_count: u32,
    pub rgba: Vec<u8>,
}

pub fn write_frame(out: &mut impl Write, frame: &Frame) -> std::io::Result<()> {
    out.write_all(&MAGIC)?;
    for field in [RENDER_PROTOCOL_VERSION, frame.width, frame.height, frame.page_count] {
        out.write_all(&field.to_le_bytes())?;
    }
    out.write_all(&frame.rgba)?;
    out.flush()
}

pub fn read_frame(input: &mut impl Read) -> Result<Frame, String> {
    let mut header = [0u8; HEADER_LEN];
    let got = read_up_to(input, &mut header).map_err(|e| format!("read failed: {e}"))?;
    if got == 0 {
        return Err("no frame".into());
    }
    if got < HEADER_LEN || header[..4] != MAGIC {
        return Err("bad magic".into());
    }
    let field = |i: usize| u32::from_le_bytes(header[4 + 4 * i..8 + 4 * i].try_into().expect("4 bytes"));
    if field(0) != RENDER_PROTOCOL_VERSION {
        return Err(format!(
            "render protocol mismatch: the view speaks {}, manifold speaks {RENDER_PROTOCOL_VERSION}",
            field(0)
        ));
    }
    let (width, height, page_count) = (field(1), field(2), field(3));
    if width == 0 || height == 0 {
        return Err("empty frame".into());
    }
    if width > MAX_FRAME_SIDE || height > MAX_FRAME_SIDE {
        return Err(format!("frame too large: {width}x{height}"));
    }
    let len = width as usize * height as usize * 4;
    let mut rgba = vec![0u8; len];
    if read_up_to(input, &mut rgba).map_err(|e| format!("read failed: {e}"))? < len {
        return Err("short frame".into());
    }
    Ok(Frame { width, height, page_count, rgba })
}

/// Fills `buf` from `input` until it is full or the stream ends; returns how many bytes arrived.
fn read_up_to(input: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match input.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

enum Event {
    Frame(Result<Frame, String>),
    Cancel,
}

/// Held by whoever may cancel a running child.
#[derive(Clone)]
pub struct Cancel(mpsc::Sender<Event>);

impl Cancel {
    /// Kills the child `run_child` is waiting on; a no-op once it has finished.
    pub fn cancel(&self) {
        let _ = self.0.send(Event::Cancel);
    }
}

/// The receiving half, consumed by `run_child`.
pub struct Inbox {
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
}

pub fn channel() -> (Cancel, Inbox) {
    let (tx, rx) = mpsc::channel();
    (Cancel(tx.clone()), Inbox { tx, rx })
}

/// Runs `command` (a view process), reads its frame from stdout and maps every way it can go wrong to one reason
/// (`label` names it, e.g. "PDF renderer"): a spawn failure is "not installed", a hang past `timeout` is
/// "timed out" (the child is killed), a `Cancel` kills it and returns "cancelled", exit 1 with stderr text is that
/// text's last line, any other failed exit is "crashed", and output that is not a valid frame is "returned a bad
/// frame". A complete valid frame is the result whatever the child does next: it is killed rather than waited for.
/// `spawn` starts the configured command: the app's closure retries a freshly extracted exe and assigns the child to
/// its kill-on-close job; tests pass `|c| c.spawn()`.
pub fn run_child(
    mut command: Command,
    label: &str,
    timeout: Duration,
    inbox: Inbox,
    spawn: impl FnOnce(&mut Command) -> std::io::Result<Child>,
) -> Result<Frame, String> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = spawn(&mut command).map_err(|e| format!("{label} is not installed ({e})"))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let tx = inbox.tx;
    // Read stdout to EOF on its own thread: waiting for exit first would deadlock on a frame larger than the pipe.
    std::thread::spawn(move || {
        let _ = tx.send(Event::Frame(read_frame(&mut stdout)));
    });
    let stderr_text = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let event = inbox.rx.recv_timeout(timeout);
    // Only a bad frame leaves the child to finish by itself, so its exit status can say why; a timeout, a cancel and a
    // good frame all end it now.
    if !matches!(event, Ok(Event::Frame(Err(_)))) {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| format!("{label} could not be waited on: {e}"))?;
    let stderr_text = stderr_text.join().unwrap_or_default();
    match event {
        Err(_) => Err(format!("{label} timed out")),
        Ok(Event::Cancel) => Err("cancelled".into()),
        Ok(Event::Frame(Ok(frame))) => Ok(frame),
        Ok(Event::Frame(Err(reason))) => {
            if status.code() == Some(1) {
                if let Some(line) = stderr_text.lines().rev().find(|l| !l.trim().is_empty()) {
                    return Err(line.trim().to_string());
                }
            }
            if !status.success() {
                return Err(format!("{label} crashed"));
            }
            Err(if reason.starts_with("render protocol mismatch") {
                reason
            } else {
                format!("{label} returned a bad frame ({reason})")
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame { width: 3, height: 2, page_count: 7, rgba: (0..24).collect() }
    }

    #[test]
    fn a_frame_round_trips() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame()).unwrap();
        let back = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!((back.width, back.height, back.page_count, back.rgba), (3, 2, 7, (0..24).collect::<Vec<u8>>()));
    }

    /// Pins the layout: magic, version, width, height, page count (u32 LE each), then the pixels.
    #[test]
    fn the_header_layout_is_pinned() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame()).unwrap();
        assert_eq!(&bytes[..4], b"MRND");
        assert_eq!(bytes[4..20], [1, 0, 0, 0, 3, 0, 0, 0, 2, 0, 0, 0, 7, 0, 0, 0]);
        assert_eq!(bytes.len(), HEADER_LEN + 24);
    }

    #[test]
    fn malformed_frames_are_named() {
        let mut good = Vec::new();
        write_frame(&mut good, &frame()).unwrap();
        let err = |bytes: &[u8]| read_frame(&mut &*bytes).unwrap_err();
        assert!(err(b"").contains("no frame"), "empty");
        assert!(err(b"XXXX0123456789012345").contains("bad magic"));
        let mut other = good.clone();
        other[4] = 2;
        assert!(err(&other).contains("protocol mismatch"));
        assert!(err(&good[..good.len() - 1]).contains("short"), "short pixels");
        let mut huge = good.clone();
        huge[8..12].copy_from_slice(&(MAX_FRAME_SIDE + 1).to_le_bytes());
        assert!(err(&huge).contains("too large"));
        let mut zero = good.clone();
        zero[8..12].copy_from_slice(&0u32.to_le_bytes());
        assert!(err(&zero).contains("empty"));
    }
}
