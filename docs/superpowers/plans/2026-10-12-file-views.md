# File Views Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the `manifold-engine-*` crates to `manifold-view-*`, then render PDFs in a separate, embedded `manifold-view-pdfium.exe` so a PDFium crash can't take the app down.

**Architecture:** Task 1 renames packages and directories but pins each `[[bin]]` name, so no exe name, pin, URL or script exe path changes. Tasks 2-3 add a one-shot render protocol (`ipc-protocol::render`: frame, child runner with timeout and cancel) and the `manifold-view-pdfium` binary. Task 4 generalizes `embedded_host` to two embedded hosts. Task 5 moves `PdfRenderer` onto the child runner and drops the PDFium dependencies from `manifold`. Task 6 updates the release and package scripts and adds a real-PDF gate step.

**Tech Stack:** Rust (nightly-2026-09-23), `pdfium-render =0.8.37`, `pdfium-auto 0.3`, std `process`/`mpsc`, bash release scripts.

**Spec:** `docs/superpowers/specs/2026-10-12-file-renderers-design.md`. Step 0b (renaming the code identifiers `engine` to `view`) is parked in the spec and is **not** in this plan; revisit after this lands.

## Global Constraints

- Work on `main`; push only when the user says so. Commits end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Before each commit: `cargo fmt --all`, then `cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p gate --all-targets -- -D warnings` (the package is `manifold-engine-webview2` until Task 1 lands; from Task 3 on the line also carries `-p manifold-view-pdfium`). Never `--no-verify`.
- Gate before the final merge: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate`, then `scripts/gate.sh LOG full` detached at BelowNormal; report CPU load and Defender state. `scripts/gate-tier.sh` says the tier; Tasks 1 and 4-6 need `full`.
- No test reads or writes the real `%APPDATA%\manifold`: use `store::testutil::temp_dir` and `profile::dir()`.
- Never let `python` read stdin: write scripts to a file, run `python -P f.py < /dev/null`.
- Exe names of the four web view hosts do not change in this plan (`[[bin]] name = "manifold-engine-*"`); only `manifold-view-pdfium` has its final exe name from the start.
- No compatibility shims; delete what a change supersedes (`render_first_page` and the PDFium dependencies leave `manifold`).
- Limit CPU: at most 12 busy threads (`-j 8`), long runs at BelowNormal, no fixed sleeps in scripts or tests (wait on a condition with a deadline).

---

### Task 1: Rename the crates to `manifold-view-*` (pinned exe names)

**Files:**
- Move: `crates/manifold-engine-{cef,servo,webkit,webview2}` to `crates/manifold-view-{cef,servo,webkit,webview2}`
- Modify: each moved `Cargo.toml`; workspace `Cargo.toml`; `Cargo.lock` (regenerated)
- Modify (package names in `-p` lists, globs, paths and comments): exactly the files `grep -rIl "manifold-engine-"` lists outside `CHANGELOG.md`, `docs/`, `dist/`, `target*/` and `Cargo.lock` (54 files at 2026-10-12, about 185 hits), among them `CLAUDE.md`, `.githooks/pre-commit`, `.github/workflows/ci.yml`, the `scripts/*.sh`, `crates/gate/src/{main.rs,fresh.rs}`, `crates/ipc-protocol/src/{messages.rs,transport.rs}`, the doc comments in the four view crates and in `crates/manifold/src`, and `README.md` (current-state text only). Most hits are exe names and stay.

**Interfaces:**
- Produces: package names `manifold-view-cef|servo|webkit|webview2`; exe names unchanged (`manifold-engine-*.exe`, `manifold-engine-webkit`).

- [ ] **Step 1: Confirm a clean tree and no running builds**

Run: `git status --short` (expect empty), `Get-Process cargo,rustc -ErrorAction SilentlyContinue` (expect none).

- [ ] **Step 2: Move the directories**

```bash
for v in cef servo webkit webview2; do git mv crates/manifold-engine-$v crates/manifold-view-$v; done
```

- [ ] **Step 3: Rename each package and pin its exe name**

In each `crates/manifold-view-X/Cargo.toml`, change `name = "manifold-engine-X"` to `name = "manifold-view-X"` and append at the end of the file (if the crate already has a `[[bin]]`, set its `name` instead):

```toml
# The exe keeps the name released builds, host_pin.rs and the scripts know; it changes with a release that
# publishes assets under the new names (spec 2026-10-12, Step 0).
[[bin]]
name = "manifold-engine-X"
path = "src/main.rs"
```

(`X` is the engine: `cef`, `servo`, `webkit`, `webview2`.) Check each crate's `src/main.rs` is its bin path; `crates/manifold-view-webview2/examples/capture_spike.rs` stays an example.

- [ ] **Step 4: Update the workspace members and comments**

In the root `Cargo.toml` set the four member paths to `crates/manifold-view-*` and fix the comment on line 24 (`crates/manifold-view-cef`).

- [ ] **Step 5: Replace package names and crate paths in the files the grep lists (not exe names)**

Write `$TEMP/rename.py` that, for each file in the list above, replaces only these exact substrings: `-p manifold-engine-` to `-p manifold-view-`, `crates/manifold-engine-` to `crates/manifold-view-`, `'crates/manifold-engine-*'` to `'crates/manifold-view-*'`, and in doc comments `manifold-engine-X/src/` to `manifold-view-X/src/`. It must NOT touch `manifold-engine-X.exe`, `manifold-engine-X` used as a file name, `HOST_FILE`, `binary: "manifold-engine-..."` in `process_manager.rs`, `host_pin.rs`, `embedded_host.rs` or the tests that build file names. Write explicit UTF-8, no newline translation, then run `git diff --stat`.

Run: `python -P $TEMP/rename.py < /dev/null`

- [ ] **Step 6: Check exactly what is left**

Run: `grep -rIn "manifold-engine-\|manifold_engine_" --exclude-dir=target --exclude-dir=target-linux --exclude-dir=target-servo --exclude-dir=.git --exclude-dir=.superpowers --exclude-dir=docs --exclude-dir=dist --exclude=CHANGELOG.md --exclude=Cargo.lock .`

Expected: only exe-name uses (`.exe` names, `binary:` strings, `HOST_FILE`, `host_pin`, the `[[bin]]` names, script paths to built exes, `release.sh` copies of built exes) and no `-p manifold-engine-` or `crates/manifold-engine-`. Read every remaining line; each must be an exe or file name.

- [ ] **Step 7: Build, lint, unit-test**

```bash
cargo build -j 8 -p manifold -p manifold-view-webview2 -p gate
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p gate --all-targets -- -D warnings
cargo test -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2
ls target/debug/manifold-engine-webview2.exe
```

Expected: all pass, and the exe is still named `manifold-engine-webview2.exe`. Also `bash scripts/gate-tools-test.sh` passes.

- [ ] **Step 8: Linux check of the other crates**

Do not compile CEF (needs the CEF download) or Servo (about 33 minutes) for this. Run `cargo metadata --format-version 1 --no-deps > "$TEMP/meta.json"` and `grep -o '"name":"manifold-view-[a-z0-9]*"' "$TEMP/meta.json"` (expect the four packages, and no `manifold-engine-` package), then delete the file. Say plainly in the status line that cef, servo and webkit were not compiled.

- [ ] **Step 9: Full gate, then commit**

Run the `full` gate (Global Constraints). Then:

```bash
git add -A
git commit -m "Rename the manifold-engine-* crates to manifold-view-*; exe names unchanged"
```

---

### Task 2: The render protocol and child runner in `ipc-protocol`

**Files:**
- Create: `crates/ipc-protocol/src/render.rs`, `crates/ipc-protocol/src/bin/render_stub.rs`, `crates/ipc-protocol/tests/render_child.rs`
- Modify: `crates/ipc-protocol/src/lib.rs` (add `pub mod render;`), `crates/ipc-protocol/Cargo.toml` (the stub `[[bin]]`)

**Interfaces:**
- Produces (`ipc_protocol::render`):

```rust
pub const RENDER_PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_SIDE: u32 = 16_384;
pub struct Frame { pub width: u32, pub height: u32, pub page_count: u32, pub rgba: Vec<u8> }
pub fn write_frame(out: &mut impl std::io::Write, frame: &Frame) -> std::io::Result<()>;
pub fn read_frame(input: &mut impl std::io::Read) -> Result<Frame, String>;
pub struct Cancel(/* private */);          // Clone; Cancel::cancel(&self)
pub struct Inbox(/* private */);           // consumed by run_child
pub fn channel() -> (Cancel, Inbox);
pub fn run_child(
    command: std::process::Command,
    label: &str,
    timeout: std::time::Duration,
    inbox: Inbox,
    spawn: impl FnOnce(&mut std::process::Command) -> std::io::Result<std::process::Child>,
) -> Result<Frame, String>;
```

`spawn` receives the command with stdio already piped and returns the started child: the app's closure retries a freshly extracted exe (`embedded_host::spawn_fresh`) and assigns the child to its kill-on-close job; tests pass `|c| c.spawn()`.

- [ ] **Step 1: Write the failing tests for the frame (unit tests in `render.rs`)**

```rust
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
```

Run: `cargo test -j 8 -p ipc-protocol render::` — Expected: FAIL (module missing).

- [ ] **Step 2: Implement the frame**

```rust
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
```

Run: `cargo test -j 8 -p ipc-protocol render::` — Expected: the three tests PASS.

- [ ] **Step 3: Add the stub child**

`crates/ipc-protocol/src/bin/render_stub.rs`, used only by `tests/render_child.rs`:

```rust
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
```

In `crates/ipc-protocol/Cargo.toml`:

```toml
[[bin]]
name = "render_stub"
path = "src/bin/render_stub.rs"
test = false
doc = false
```

- [ ] **Step 4: Write the failing integration tests**

`crates/ipc-protocol/tests/render_child.rs`:

```rust
use ipc_protocol::render::{channel, run_child};
use std::process::Command;
use std::time::{Duration, Instant};

fn stub(mode: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_render_stub"));
    command.arg(mode);
    command
}

fn run(mode: &str, timeout: Duration) -> Result<ipc_protocol::render::Frame, String> {
    let (_cancel, inbox) = channel();
    run_child(stub(mode), "PDF renderer", timeout, inbox, |c| c.spawn())
}

const LONG: Duration = Duration::from_secs(30);

#[test]
fn a_good_frame_comes_back() {
    let frame = run("ok", LONG).unwrap();
    assert_eq!((frame.width, frame.height, frame.page_count, frame.rgba.len()), (2, 1, 3, 8));
}

#[test]
fn a_failure_reports_the_last_stderr_line() {
    assert_eq!(run("fail", LONG).unwrap_err(), "Couldn't read the file");
}

#[test]
fn a_crash_is_named() {
    assert_eq!(run("crash", LONG).unwrap_err(), "PDF renderer crashed");
}

#[test]
fn garbage_and_short_output_are_a_bad_frame() {
    for mode in ["garbage", "short"] {
        let err = run(mode, LONG).unwrap_err();
        assert!(err.starts_with("PDF renderer returned a bad frame"), "{mode}: {err}");
    }
}

#[test]
fn a_hang_times_out() {
    let start = Instant::now();
    assert_eq!(run("hang", Duration::from_millis(300)).unwrap_err(), "PDF renderer timed out");
    assert!(start.elapsed() < Duration::from_secs(10), "the hung child was killed, not waited for");
}

#[test]
fn a_valid_frame_wins_over_a_lingering_child() {
    let start = Instant::now();
    assert_eq!(run("linger", LONG).unwrap().page_count, 3);
    assert!(start.elapsed() < Duration::from_secs(10), "the child that kept running was killed, not waited for");
}

#[test]
fn a_cancel_kills_a_running_child() {
    let (cancel, inbox) = channel();
    let started = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let flag = started.clone();
    let worker = std::thread::spawn(move || {
        run_child(stub("hang"), "PDF renderer", LONG, inbox, move |c| {
            let child = c.spawn()?;
            *flag.0.lock().unwrap() = true;
            flag.1.notify_all();
            Ok(child)
        })
    });
    let (lock, cvar) = &*started;
    let guard = cvar.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(10), |s| !*s).unwrap().0;
    assert!(*guard, "the child started");
    drop(guard);
    cancel.cancel();
    assert_eq!(worker.join().unwrap().unwrap_err(), "cancelled");
}

#[test]
fn a_missing_binary_is_not_installed() {
    let (_cancel, inbox) = channel();
    let err = run_child(Command::new("definitely-not-here-xyz"), "PDF renderer", LONG, inbox, |c| c.spawn()).unwrap_err();
    assert!(err.starts_with("PDF renderer is not installed"), "{err}");
}
```

Run: `cargo test -j 8 -p ipc-protocol --test render_child` — Expected: FAIL (`channel`/`run_child` missing).

- [ ] **Step 5: Implement `channel` and `run_child`**

Append to `render.rs` (above the tests):

```rust
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
/// `spawn` starts the configured command (see the interface notes above).
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
        Err(_) => return Err(format!("{label} timed out")),
        Ok(Event::Cancel) => return Err("cancelled".into()),
        Ok(Event::Frame(Ok(frame))) => return Ok(frame),
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
```

Register the module: add `pub mod render;` to `lib.rs`.

Run: `cargo test -j 8 -p ipc-protocol` — Expected: all PASS, including the seven integration tests.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p ipc-protocol --all-targets -- -D warnings
git add -A
git commit -m "ipc-protocol: the one-shot render protocol and child runner"
```

---

### Task 3: The `manifold-view-pdfium` binary

**Files:**
- Create: `crates/manifold-view-pdfium/Cargo.toml`, `crates/manifold-view-pdfium/src/main.rs`, `crates/manifold-view-pdfium/tests/fixture.pdf`, `crates/manifold-view-pdfium/tests/render.rs`
- Modify: root `Cargo.toml` (member)

**Interfaces:**
- Consumes: `ipc_protocol::render::{Frame, write_frame}`
- Produces: `manifold-view-pdfium.exe <pdf-path>`; success writes one frame (first page, longest side capped at 1000 px, `page_count` set); failure prints one stderr line and exits 1.

- [ ] **Step 1: Create the crate**

`Cargo.toml`:

```toml
[package]
name = "manifold-view-pdfium"
version.workspace = true
edition.workspace = true
description = "Renders the first page of a PDF for manifold, in its own process so a PDFium crash can't take the app down."

[lints]
workspace = true

[dependencies]
ipc-protocol.workspace = true
pdfium-render.workspace = true
pdfium-auto.workspace = true
```

Add `"crates/manifold-view-pdfium"` to the root workspace `members`.

- [ ] **Step 2: Move the render code into `main.rs`**

```rust
//! One-shot PDF view: `manifold-view-pdfium <path>` renders the first page and writes one frame to stdout
//! (`ipc_protocol::render`). A failure prints one line on stderr and exits 1; a PDFium crash is the process dying,
//! which the app reports as a crashed renderer instead of dying itself.

use ipc_protocol::render::{write_frame, Frame};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium, PdfiumError};

/// Capped well above the card's size so the page still looks sharp in the enlarged peek preview, without paying for a
/// full-resolution render of every page.
const PDF_RENDER_MAX_DIMENSION: i32 = 1000;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: manifold-view-pdfium <path>");
        std::process::exit(1);
    };
    let result = pdfium_auto::bind_pdfium_silent()
        .map_err(|e| e.to_string())
        .and_then(|pdfium| render_first_page(&pdfium, &path));
    match result {
        Ok(frame) => {
            if let Err(e) = write_frame(&mut std::io::stdout().lock(), &frame) {
                eprintln!("couldn't write the frame: {e}");
                std::process::exit(1);
            }
        }
        Err(reason) => {
            eprintln!("{reason}");
            std::process::exit(1);
        }
    }
}

/// `PdfiumError`'s own `Display` is its pretty-printed `Debug` (a missing file comes out as a nine-line
/// `IoError(Os { .. })`), too much for a toast.
fn pdfium_error(err: PdfiumError) -> String {
    match err {
        PdfiumError::IoError(err) => err.to_string(),
        other => format!("{other:?}"),
    }
}

fn render_first_page(pdfium: &Pdfium, path: &str) -> Result<Frame, String> {
    let document = pdfium.load_pdf_from_file(path, None).map_err(pdfium_error)?;
    let pages = document.pages();
    let page_count = pages.len() as u32;
    let page = pages.get(0).map_err(pdfium_error)?;
    let render_config =
        PdfRenderConfig::new().set_target_width(PDF_RENDER_MAX_DIMENSION).set_maximum_height(PDF_RENDER_MAX_DIMENSION);
    let bitmap = page.render_with_config(&render_config).map_err(pdfium_error)?;
    Ok(Frame { width: bitmap.width() as u32, height: bitmap.height() as u32, page_count, rgba: bitmap.as_rgba_bytes() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfium_error_is_one_line() {
        let err = PdfiumError::IoError(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert_eq!(pdfium_error(err), "gone");
        assert!(!pdfium_error(PdfiumError::PageIndexOutOfBounds).contains('\n'));
    }
}
```

`bind_once`, `take_cancelled` and their tests stay behind in `pdf_render.rs` (Task 5 decides their fate: `bind_once` is dead here because the process binds once per run).

- [ ] **Step 3: Add the fixture**

Write `tests/fixture.pdf`, a valid one-page PDF (about 200 bytes). Generate it with a file-writing script, not by hand, so the xref offsets are correct:

```python
# $TEMP/mkpdf.py
content = b"0 0 1 rg 20 20 160 60 re f"
objs = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>",
    b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
]
out = bytearray(b"%PDF-1.4\n")
offsets = []
for i, body in enumerate(objs, 1):
    offsets.append(len(out))
    out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for off in offsets:
    out += b"%010d 00000 n \n" % off
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
open("crates/manifold-view-pdfium/tests/fixture.pdf", "wb").write(out)
```

Run: `python -P $TEMP/mkpdf.py < /dev/null`

- [ ] **Step 4: Write the test against the built exe**

`tests/render.rs`:

```rust
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
```

Run: `cargo test -j 8 -p manifold-view-pdfium` — Expected: PASS on a machine that can fetch PDFium once (first run downloads the DLL, so allow a minute). If the first run is offline, the render test fails with the bind error; say so and rerun online.

- [ ] **Step 5: Put the crate on the lint lines**

Add `-p manifold-view-pdfium` to the clippy line in `CLAUDE.md` ("Formatting and lints"), `.githooks/pre-commit` and the clippy step of `.github/workflows/ci.yml`. Not to CI's `cargo test` lines: the render tests download PDFium and belong to local runs and the gate.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
git add -A
git commit -m "manifold-view-pdfium: render a PDF's first page in its own process"
```

---

### Task 4: Embed more than one host

**Files:**
- Modify: `crates/manifold/build.rs`, `crates/manifold/src/embedded_host.rs`, `crates/manifold/src/process_manager.rs` (the one `embedded_host::ensure` call, ~line 798, and its spawn site, ~line 852), `crates/manifold/src/process_manager/availability.rs` (the `host_path` call, ~line 54), `crates/manifold/src/process_manager/tests.rs` (~line 864)

**Interfaces:**
- Produces:

```rust
pub(crate) enum Embedded { Webview2, Pdfium }
impl Embedded {
    pub(crate) fn host_path(self, hosts_dir: &Path) -> PathBuf; // hosts_dir/<webview2|pdfium>/<exe>
    pub(crate) fn ensure(self, hosts_dir: &Path) -> io::Result<Option<PathBuf>>;
}
```

`ensure_with` becomes `ensure_with(host: Embedded, hosts_dir, bytes, sha256)`; user-facing error texts take the host's `label()` ("the WebView2 host", "the PDF view") instead of the literal. Private helpers `file()`, `label()` and `bytes() -> (&'static [u8], &'static str)` replace the statics `EMBEDDED`, `EMBEDDED_SHA256` and the const `HOST_FILE`.

- [ ] **Step 1: Make the existing tests parametric first (they must keep passing)**

In `embedded_host.rs` tests, change each call `ensure_with(dir, bytes, sha)` to `ensure_with(Embedded::Webview2, dir, bytes, sha)`, `host_path(dir)` to `Embedded::Webview2.host_path(dir)`, every `HOST_FILE` to `Embedded::Webview2.file()` (lines ~161-238), and the two tests that read the real embed (`EMBEDDED`, `EMBEDDED_SHA256`, ~lines 149 and 272-281) to `let (embedded, embedded_sha) = Embedded::Webview2.bytes();`. Add one test that the two entries extract side by side without touching each other:

```rust
#[test]
fn two_embedded_hosts_extract_side_by_side() {
    let dir = temp_dir("embedded-two");
    let a = ensure_with(Embedded::Webview2, &dir, b"AAAA", &hash(b"AAAA")).unwrap().unwrap();
    let b = ensure_with(Embedded::Pdfium, &dir, b"BBBB", &hash(b"BBBB")).unwrap().unwrap();
    assert_ne!(a, b);
    assert_eq!(std::fs::read(&a).unwrap(), b"AAAA");
    assert_eq!(std::fs::read(&b).unwrap(), b"BBBB");
    assert!(b.ends_with("pdfium/manifold-view-pdfium.exe"), "{}", b.display());
}
```

Run: `cargo test -j 8 -p manifold embedded_host` — Expected: FAIL to compile.

- [ ] **Step 2: Implement the table**

In `embedded_host.rs`:

```rust
/// A host carried inside `manifold.exe`. Each has a folder of its own under `engine-hosts/`, so a host's lock file,
/// leftover `.old` and (WebView2) user-data folder stay apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Embedded {
    Webview2,
    Pdfium,
}

static WEBVIEW2: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/webview2-host.bin"));
static PDFIUM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pdfium-host.bin"));

impl Embedded {
    fn folder(self) -> &'static str {
        match self {
            Self::Webview2 => "webview2",
            Self::Pdfium => "pdfium",
        }
    }
    /// Unchanged for WebView2: the exe name is how released builds and scripts know it.
    fn file(self) -> &'static str {
        match self {
            Self::Webview2 => "manifold-engine-webview2.exe",
            Self::Pdfium => "manifold-view-pdfium.exe",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Webview2 => "the WebView2 host",
            Self::Pdfium => "the PDF view",
        }
    }
    fn bytes(self) -> (&'static [u8], &'static str) {
        match self {
            Self::Webview2 => (WEBVIEW2, env!("MANIFOLD_WEBVIEW2_SHA256")),
            Self::Pdfium => (PDFIUM, env!("MANIFOLD_PDFIUM_SHA256")),
        }
    }
    pub(crate) fn host_path(self, hosts_dir: &Path) -> PathBuf {
        hosts_dir.join(self.folder()).join(self.file())
    }
    pub(crate) fn ensure(self, hosts_dir: &Path) -> io::Result<Option<PathBuf>> {
        let (bytes, sha256) = self.bytes();
        ensure_with(self, hosts_dir, bytes, sha256)
    }
}
```

Delete the old `EMBEDDED`, `EMBEDDED_SHA256`, `HOST_FILE`, free `host_path` and `ensure`. In `ensure_with`, add the `host: Embedded` first parameter, set `dest = host.host_path(hosts_dir)`, and replace the two literal strings `"could not write the WebView2 host for"`, `"the WebView2 host written for"`, `"could not move the new WebView2 host to"` with `format!("could not write {} for", host.label())`, `format!("{} written for", host.label())`, `format!("could not move the new {} to", host.label())` (capitalise the first letter where the message starts a sentence; check the tests' expected texts).

- [ ] **Step 3: Update `build.rs`**

Replace the single-host body with a loop over `[("MANIFOLD_WEBVIEW2_HOST", "webview2-host.bin", "MANIFOLD_WEBVIEW2_SHA256"), ("MANIFOLD_PDFIUM_HOST", "pdfium-host.bin", "MANIFOLD_PDFIUM_SHA256")]` doing exactly what the current body does for the first (read the file when the variable names one and the target is Windows, hash it, write `OUT_DIR/<bin>`, emit `cargo:rustc-env=<SHA>=<hash or empty>`, `rerun-if-env-changed` and `rerun-if-changed`). Update the file's doc comment.

- [ ] **Step 4: Fix the call sites**

`process_manager.rs`: `embedded_host::ensure(&dir)?` becomes `embedded_host::Embedded::Webview2.ensure(&dir)?`. `availability.rs`: `crate::embedded_host::host_path(&fallback)` becomes `crate::embedded_host::Embedded::Webview2.host_path(&fallback)`. `tests.rs`: the same for its `embedded_host::host_path(hosts)`.

- [ ] **Step 5: Run and commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold --all-targets -- -D warnings
cargo test -j 8 -p manifold
git add -A
git commit -m "embedded_host: a table of embedded hosts, WebView2 and the PDF view"
```

Expected: all tests pass; the existing embedded-host tests behave as before.

---

### Task 5: `PdfRenderer` on the child runner

**Files:**
- Modify: `crates/manifold/src/pdf_render.rs`, `crates/manifold/src/process_manager.rs` (re-export or a small `pdf_view_path` function), `crates/manifold/Cargo.toml` (drop `pdfium-render`, `pdfium-auto`), root `Cargo.toml` (the two pdfium entries stay as workspace dependencies for `manifold-view-pdfium`)
- Test: unit tests in `pdf_render.rs`

**Interfaces:**
- Consumes: `ipc_protocol::render::{channel, run_child, Cancel, Frame}`, `Embedded::Pdfium`, `sibling_binary`, `assign_to_engine_job`, `engine_hosts_dir`
- Produces: `PdfRenderer::{new, request, cancel, tick}` with unchanged signatures; `PdfRenderResult` keeps its fields except `page_count: usize` stays `usize`.

- [ ] **Step 1: Add the view locator to `process_manager`**

In `process_manager.rs` (it can see `sibling_binary`, `assign_to_engine_job` and `engine_hosts_dir`):

```rust
/// The PDF view exe: beside `manifold.exe` when built there (dev trees, the gate), else the copy embedded in this
/// build, extracted on first use. `None` when neither exists.
pub(crate) fn pdf_view_exe() -> std::io::Result<Option<std::path::PathBuf>> {
    let sibling = sibling_binary("manifold-view-pdfium");
    if sibling.exists() {
        return Ok(Some(sibling));
    }
    match engine_hosts_dir() {
        Some(dir) => embedded_host::Embedded::Pdfium.ensure(&dir),
        None => Ok(None),
    }
}

/// Puts a spawned view process in the kill-on-close job, so it dies with manifold.
pub(crate) fn contain_view_process(child: &std::process::Child) {
    assign_to_engine_job(child);
}
```

- [ ] **Step 2: Write the failing tests for the new `PdfRenderer` internals**

In `pdf_render.rs` tests, replace `a_failed_bind_is_retried_and_a_good_one_kept` (the bind now lives in the child) with tests of the shared state:

```rust
#[test]
fn a_cancel_before_the_render_starts_skips_it() {
    let state = State::default();
    state.cancel("gone");
    assert!(!state.begin("gone", ipc_protocol::render::channel().0), "skipped");
    assert!(state.begin("gone", ipc_protocol::render::channel().0), "the mark was taken once");
}

#[test]
fn a_cancel_while_running_kills_instead_of_marking_and_a_finish_clears_it() {
    let state = State::default();
    assert!(state.begin("a", ipc_protocol::render::channel().0));
    state.cancel("a");
    assert!(!state.is_cancelled("a"), "a running render is killed, not marked for skipping");
    state.finish("a");
    assert!(state.running_ids().is_empty());
    assert!(state.begin("a", ipc_protocol::render::channel().0), "a later request for the same id still renders");
}

#[test]
fn a_new_request_clears_a_stale_cancel() {
    let state = State::default();
    state.cancel("a");
    state.request("a");
    assert!(state.begin("a", ipc_protocol::render::channel().0));
}
```

Run: `cargo test -j 8 -p manifold pdf_render` — Expected: FAIL (no `State`).

- [ ] **Step 3: Rewrite `pdf_render.rs`**

Replace the file body (keep the file's opening doc comment, edited to say the render runs in `manifold-view-pdfium`):

```rust
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use ipc_protocol::render::Cancel;

const WORKERS: usize = 2;
/// Covers the one-time PDFium download as well as a render; a hung PDF costs a worker this long at most.
const RENDER_TIMEOUT: Duration = Duration::from_secs(120);
const LABEL: &str = "PDF renderer";

pub struct PdfRenderResult {
    pub width: u32,
    pub height: u32,
    pub page_count: usize,
    pub rgba: Vec<u8>,
}

/// Which renders are queued-but-cancelled and which child is running, under one lock so a cancel can't fall between
/// a worker's check and its registration.
#[derive(Default)]
struct State(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    cancelled: HashSet<String>,
    running: HashMap<String, Cancel>,
}

impl State {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    /// Node `id` was deleted: skip its render if queued, kill it if running.
    fn cancel(&self, id: &str) {
        let mut inner = self.lock();
        match inner.running.get(id) {
            Some(running) => running.cancel(),
            None => {
                inner.cancelled.insert(id.to_string());
            }
        }
    }
    /// A fresh request: a mark left by deleting an already-rendered PDF must not skip it.
    fn request(&self, id: &str) {
        self.lock().cancelled.remove(id);
    }
    /// A worker takes `id`: false when it was cancelled while queued, else registers `cancel` for it.
    fn begin(&self, id: &str, cancel: Cancel) -> bool {
        let mut inner = self.lock();
        if inner.cancelled.remove(id) {
            return false;
        }
        inner.running.insert(id.to_string(), cancel);
        true
    }
    fn finish(&self, id: &str) {
        self.lock().running.remove(id);
    }
    #[cfg(test)]
    fn running_ids(&self) -> Vec<String> {
        self.lock().running.keys().cloned().collect()
    }
    #[cfg(test)]
    fn is_cancelled(&self, id: &str) -> bool {
        self.lock().cancelled.contains(id)
    }
}

pub struct PdfRenderer {
    tx: mpsc::Sender<(String, String)>,
    rx: mpsc::Receiver<(String, Result<PdfRenderResult, String>)>,
    state: Arc<State>,
}

impl PdfRenderer {
    pub fn new() -> Self {
        let (tx, queue) = mpsc::channel::<(String, String)>();
        let (result_tx, rx) = mpsc::channel();
        let queue = Arc::new(Mutex::new(queue));
        let state = Arc::new(State::default());
        for _ in 0..WORKERS {
            let (queue, result_tx, state) = (Arc::clone(&queue), result_tx.clone(), Arc::clone(&state));
            thread::spawn(move || loop {
                // The lock is held only while waiting for the next request, never while rendering.
                let Ok((id, path)) = queue.lock().map(|q| q.recv()).unwrap_or(Err(mpsc::RecvError)) else { return };
                let (cancel, inbox) = ipc_protocol::render::channel();
                if !state.begin(&id, cancel) {
                    continue;
                }
                let result = render(&path, inbox);
                state.finish(&id);
                if result_tx.send((id, result)).is_err() {
                    return;
                }
            });
        }
        Self { tx, rx, state }
    }

    /// Queues a render of `path`'s first page for node `id`.
    pub fn request(&self, id: String, path: String) {
        self.state.request(&id);
        let _ = self.tx.send((id, path));
    }

    /// Node `id` was deleted: skip its render if it hasn't started, kill it if it has.
    pub fn cancel(&self, id: &str) {
        self.state.cancel(id);
    }

    /// Whether `id`'s render is marked to be skipped (`session_sync` tests).
    #[cfg(test)]
    pub(crate) fn is_cancelled(&self, id: &str) -> bool {
        self.state.is_cancelled(id)
    }

    /// Every render finished since the last call. A result for a node deleted meanwhile is the caller's to ignore.
    pub fn tick(&self) -> Vec<(String, Result<PdfRenderResult, String>)> {
        self.rx.try_iter().collect()
    }
}

/// Runs the PDF view on `path` in its own process.
fn render(path: &str, inbox: ipc_protocol::render::Inbox) -> Result<PdfRenderResult, String> {
    let exe = crate::process_manager::pdf_view_exe()
        .map_err(|e| format!("{LABEL} could not be extracted: {e}"))?
        .ok_or_else(|| format!("{LABEL} is not installed: build manifold-view-pdfium"))?;
    let mut command = std::process::Command::new(exe);
    command.arg(path);
    let frame = ipc_protocol::render::run_child(command, LABEL, RENDER_TIMEOUT, inbox, |command| {
        // A just-extracted exe can be held for a moment by a scanner: the same retry the WebView2 spawn has.
        let child = crate::embedded_host::spawn_fresh(command)?;
        crate::process_manager::contain_view_process(&child);
        Ok(child)
    })?;
    Ok(PdfRenderResult { width: frame.width, height: frame.height, page_count: frame.page_count as usize, rgba: frame.rgba })
}
```

Remove the `pdfium_render` imports, `Request`, `CancelSet`, `bind_once`, `take_cancelled`, `pdfium_error`, `render_first_page`, `PDF_RENDER_MAX_DIMENSION` and their tests (now in the view crate or replaced above). Fix `node_store.rs:97` and `session_sync.rs:2044`: they call `self.pdf_renderer.cancel(id)`, unchanged. `PdfRenderer::is_cancelled` stays (test-only) for `session_sync/tests.rs:2976`, now answered by `State::is_cancelled`. If `sibling_binary` is not in the `use availability::{...}` list at `process_manager.rs:909-916`, add it.

- [ ] **Step 4: Drop the dependencies from `manifold`**

In `crates/manifold/Cargo.toml` delete `pdfium-render.workspace = true` and `pdfium-auto.workspace = true`. Keep the workspace entries (and their comments) in the root `Cargo.toml`; they now serve `manifold-view-pdfium`.

Run: `cargo build -j 8 -p manifold -p manifold-view-pdfium && cargo test -j 8 -p manifold pdf_render`
Expected: the three new tests PASS; `grep -rn "pdfium" crates/manifold/src crates/manifold/Cargo.toml` shows only the exe name `manifold-view-pdfium`, doc comments and the new `pdf_view_exe`.

- [ ] **Step 5: Log a finished render**

The success path logs nothing today, so nothing outside the app can tell a rendered PDF from a pending one (`package.sh` needs the signal, Task 6). In `crates/manifold/src/event_loop.rs`, in the `Ok(rendered)` arm of `for (id, result) in self.pdf_renderer.tick()` (~line 1263), make the node update:

```rust
if let Some(pdf) = self.nodes.get_mut(&id).and_then(|n| n.as_pdf_mut()) {
    eprintln!("pdf: rendered {}: {}x{}, {} page(s)", pdf.path, rendered.width, rendered.height, rendered.page_count);
    pdf.outcome = Some(Ok(PdfOutcome {
        width: rendered.width,
        height: rendered.height,
        page_count: rendered.page_count,
    }));
}
```

It pairs with the existing `pdf: couldn't open {path}: {err}` line in the `Err` arm.

- [ ] **Step 6: Lint, tests, commit**

```bash
cargo fmt --all
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo test -j 8 -p manifold -p ipc-protocol -p manifold-view-pdfium
git add -A
git commit -m "Render PDFs in the manifold-view-pdfium process (D-R9)"
```

The glue (`pdf_view_exe`, `render`) is covered end to end by `package.sh` in Task 6, on the shipped bare exe. A `manifold` unit test would have to find a built view exe beside `target/debug/deps/`, which needs a test-only lookup branch; not worth it.

---

### Task 6: Ship it: release, package, gate, docs

**Files:**
- Modify: `scripts/release.sh`, `scripts/package.sh`, `scripts/gate.sh` (header build line), `scripts/fresh-exe.sh`, `scripts/gate-tools-test.sh`, `crates/gate/src/{main.rs,fresh.rs}`, `CLAUDE.md` ("Gate" build line), `README.md`, `docs/superpowers/plans/2026-10-04-review-scan-fixes.md` (D-R9 shipped), the spec (status, and the record-file wording in "Shipping")

- [ ] **Step 1: Release script**

Read `scripts/release.sh` lines 1-30 (header), 76-110 (`--verify`) and 136-245 first. Then:
- `win_pkgs=(-p manifold-view-webview2 -p manifold-view-pdfium)` (Task 1 already renamed the first).
- After the WebView2 host copy: `pdf_host="$stage/manifold-view-pdfium.exe"; cp "$tdir/manifold-view-pdfium.exe" "$pdf_host" || die "$tdir/manifold-view-pdfium.exe is missing"`.
- In the `manifold.exe` build subshell: `export MANIFOLD_PDFIUM_HOST; MANIFOLD_PDFIUM_HOST="$(wp "$pdf_host")"` beside the WebView2 one.
- After the existing "the WebView2 host changed while manifold.exe was built" line: `[ "$(sha "$tdir/manifold-view-pdfium.exe")" = "$(sha "$pdf_host")" ] || die "the PDF view changed while manifold.exe was built"`.
- In the `dist/` block: `cp "$pdf_host" dist/embedded/manifold-view-pdfium.exe` and `echo "$(sha "$pdf_host")  manifold-view-pdfium.exe" >"dist/manifold-$tag-windows-x64.embedded-pdf-view.sha256"`; add `echo "embedded PDF view sha256: $(sha "$pdf_host")"` beside the WebView2 one.
- `--verify`: a second record, in its own file because the first is read as one hash and the tags already shipped (v0.4.1, v0.5.0, v0.5.1) have only that one. After the WebView2 record check:

```bash
    if git cat-file -e "$verify_tag:crates/manifold-view-pdfium/Cargo.toml" 2>/dev/null; then
        rec="dist/manifold-$verify_tag-windows-x64.embedded-pdf-view.sha256"
        if [ -f "$rec" ] && [ "$(cut -d' ' -f1 "$rec")" = "$(sha dist/embedded/manifold-view-pdfium.exe)" ]; then
            ok "the embedded PDF view copy matches the recorded hash"
        else bad "$rec is missing or does not match dist/embedded/manifold-view-pdfium.exe"; fi
    fi
```

  (A tag whose tree has the crate must have the record; an older tag is not asked for it.)
- Update the header comments (steps list, `--verify` description) to mention the second embedded file.

Run: `bash scripts/release.sh --verify v0.5.1` — Expected: the same results as before the edit (the v0.5.1 tree has no PDF view crate, so the new check is skipped). `bash -n scripts/release.sh` passes. Do not run a real release.

- [ ] **Step 2: Package script**

`package.sh` drives the bare exe headless through a `cmd` file. Edit its flow (read lines 1-30 and 60-85 first):
1. After `exe=...`, copy the fixture: `cp crates/manifold-view-pdfium/tests/fixture.pdf "$out/fixture.pdf"`.
2. In the first command block, replace `echo QUIT` with `echo "ADDFILE $(cygpath -w "$out/fixture.pdf") 600 500"; echo WAITTABS` (the block ends there; QUIT is sent later).
3. Wait for the render on a deadline of 180 s (the first run downloads PDFium), stopping early on a failure line, then QUIT:

```bash
for _ in $(seq 900); do grep -qE '^pdf: (rendered|couldn.t open) ' "$profile/log" && break; sleep 0.2; done
echo QUIT >>"$cmd"
```

4. After the existing exit wait and extraction check, assert both outcomes:

```bash
grep -qE '^pdf: rendered .*fixture\.pdf: 1000x500, 1 page' "$profile/log" && ok "the PDF view rendered the fixture (1000x500, 1 page)" ||
    { bad "the fixture PDF did not render"; grep '^pdf: ' "$profile/log" | sed 's/^/    log: /'; }
[ -f "$profile/manifold/manifold/data/engine-hosts/pdfium/manifold-view-pdfium.exe" ] &&
    ok "the PDF view was extracted under the redirected APPDATA" || bad "no extracted PDF view under the redirected APPDATA"
```

The assertion that only `manifold.exe` sits beside the exe stays and must still pass. Update the header comment (lines 6-13).

- [ ] **Step 3: Gate staleness and build lines**

`crates/gate/src/fresh.rs` judges the WebView2 host by its own dep-info because `-p manifold` does not build it (~line 66): generalize that loop to `[exe, webview2 host, pdf view]`, each skipped when absent (same rule), and change the "Build first" text to `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium`. There is no new gate stage: the smoke's `doc.pdf` already makes the app spawn the view (an invalid PDF, so it ends in the existing error path), and the real render is `package.sh`'s. Update the build line to `-p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate` in `crates/gate/src/main.rs`, `scripts/gate.sh`, `scripts/fresh-exe.sh`, `scripts/gate-tools-test.sh` (and its list of required exes if it has one), `CLAUDE.md` ("Gate" section) and the README's build text. `bash scripts/gate-tools-test.sh` is the check for the script side; the `fresh.rs` change keeps its existing unit tests green.

- [ ] **Step 4: Measure and document**

Build release with `MANIFOLD_PDFIUM_HOST` set to the built view exe, record the `manifold.exe` size with and without it in the README packaging notes. Update the README current-state text: PDFs render in `manifold-view-pdfium` (embedded, extracted to `engine-hosts/pdfium/`), the crates are `manifold-view-*`, the four web view exes keep their `manifold-engine-*` names until a release publishes assets under new ones. Mark D-R9 shipped in `docs/superpowers/plans/2026-10-04-review-scan-fixes.md`. In the spec: set the status to shipped with the date, change "second line in the existing record" to "a second record file, `...embedded-pdf-view.sha256`, because tags already shipped have only the first", leave step 0b parked.

- [ ] **Step 5: Final checks**

```bash
cargo fmt --all --check
cargo clippy -j 8 -p manifold -p ipc-protocol -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings
cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate
```

Then `scripts/gate.sh LOG full` detached at BelowNormal; report CPU load and Defender state. Run `bash scripts/package.sh` against a release build made with both embeds (it needs network once for PDFium). Check `Get-Process python,manifold*,gate*` for leftovers.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "Ship the PDF view: release and package scripts, gate staleness, docs"
```

---

## Self-review

- **Spec coverage:** step 0 rename (Task 1), render protocol and failure mapping (Task 2), the PDF crate (Task 3), shipping by embedding (Tasks 4 and 6), `PdfRenderer` with unchanged API, cancel-kills and two workers (Task 5), end-to-end proof on the shipped exe (Task 6 `package.sh`), docs and D-R9 (Task 6). Step 0b is explicitly out. The one deviation from the spec: the second embedded hash goes in its own record file (Task 6 Step 1); the spec is amended in Step 4.
- **Type consistency:** `Frame`, `Cancel`, `Inbox`, `channel`, `run_child(command, label, timeout, inbox, spawn)` and `Embedded::{host_path, ensure}` are defined in Tasks 2 and 4 and used with the same signatures in Task 5. `PdfRenderResult.page_count` stays `usize` for `event_loop.rs`.
- **Executor reads first:** the `release.sh` and `package.sh` edits give the code to add, but the exact insertion lines come from reading the script (both steps say so).

## Stress-test record

- **Round 1** (every factual claim checked against the code and scripts): 12 defects. `PdfRenderer::is_cancelled` is used by `session_sync` tests; the extracted exe needs `spawn_fresh`'s retry, so `run_child` takes a spawn closure instead of `on_spawn`; a valid frame must not wait on a lingering child; `release.sh --verify` reads one hash per record, so the PDF view gets its own record file; the existing `embedded_host` tests use `HOST_FILE`/`EMBEDDED`; `package.sh` has no signal for a finished render, so the app logs one; the in-crate end-to-end test needed a test-only lookup branch, so it was dropped for `package.sh`; the fixture's stream `/Length` was wrong; `fresh.rs` judges staleness, it does not require the exe; the lint lists belong in Task 3, not 6; Task 1's file list was incomplete (now "whatever the grep lists"); a muddled cancel test.
- **Round 2** (re-read of every changed passage): 3 defects. A stale instruction to delete `is_cancelled`; `sibling_binary` may not be imported in `process_manager.rs`; Task 1's Linux-check step still carried a forbidden stdin pipe.
- **Round 3** (full pass, nothing material found): see the status line of the session that wrote this plan.
