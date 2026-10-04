use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use memmap2::MmapMut;

/// A file-backed memory-mapped raw pixel buffer shared between a
/// same-machine engine-host and `manifold`. Backed by a temp file rather
/// than a raw Windows shared-memory section or POSIX shm — file-backed
/// mmap works identically on Windows and Linux, which keeps `wpe-sys`'s
/// eventual native (non-WSL2) port free of a second implementation later.
///
/// Pixel format is NOT fixed across engines: Servo's
/// `SoftwareRenderingContext::read_to_image()` writes RGBA8, while CEF's
/// OSR `OnPaint` writes BGRA8 natively. Each engine-host's `FrameReady`
/// consumer on the manifold side must know which format that engine
/// produces — this buffer just carries opaque bytes.
pub struct SharedFrameBuffer {
    mmap: MmapMut,
    path: PathBuf,
    width: u32,
    height: u32,
    /// Unix only: the frame file, held open with a shared `flock` for the
    /// buffer's life so another manifold's sweep can tell the file is in use
    /// (see `unused`). Windows needs none: the mapping itself refuses the
    /// sweep's exclusive open.
    #[cfg(unix)]
    _lock: std::fs::File,
}

/// Deterministic from `name` alone so manifold can re-derive the path of
/// a frame buffer the engine-host created, without the engine having to
/// send the path over the control channel.
pub fn frame_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("manifold-frame-{name}.bin"))
}

/// Deletes `name`'s frame file (errors ignored). Only call once its engine-host
/// has exited and before anything can create a new file under the same name:
/// Windows deletes a file another process still maps (measured, Windows 11
/// 26200), so a late call would take a successor's file out from under it.
pub fn remove_frame_file(name: &str) {
    let _ = std::fs::remove_file(frame_path(name));
}

/// Deletes `manifold-frame-*.bin` files in `dir` last modified more than
/// `older_than` ago and not in use. A live engine-host keeps its file
/// mapped. On Windows that makes an exclusive open fail, so a file that opens
/// with `share_mode(0)` belongs to no one. On Unix a mapping blocks nothing,
/// so every `SharedFrameBuffer` holds a shared `flock` on its file instead,
/// and a file whose exclusive lock can be taken belongs to no one.
pub fn sweep_stale_frames_in(dir: &Path, older_than: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with("manifold-frame-") && name.ends_with(".bin")) {
            continue;
        }
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= older_than);
        if old && unused(&path) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(windows)]
fn unused(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    // Dropped before the delete below; only the open's success matters.
    OpenOptions::new().read(true).share_mode(0).open(path).is_ok()
}

/// `flock` locks (`File::try_lock`) belong to an open file and go when its
/// holder exits, so a crashed host's file locks again. A file that can't be
/// opened is left alone.
#[cfg(not(windows))]
fn unused(path: &Path) -> bool {
    OpenOptions::new().read(true).open(path).is_ok_and(|file| file.try_lock().is_ok())
}

/// Takes the shared lock `unused` probes for. On a filesystem without `flock`
/// (`Unsupported`) the file stays unlocked, as before locks existed, and the
/// sweep's age limit is the only guard.
#[cfg(unix)]
fn lock_shared(file: &std::fs::File) -> io::Result<()> {
    match file.lock_shared() {
        Err(err) if err.kind() == io::ErrorKind::Unsupported => Ok(()),
        other => other,
    }
}

impl SharedFrameBuffer {
    /// `name` should be unique per tab (e.g. a tab id) so concurrent tabs
    /// don't collide on the same backing file.
    ///
    /// Retries a few times on Windows error 1224 ("a file with a
    /// user-mapped section open"): manifold opens and drops its own
    /// mapping of this same file on every redraw to read the latest frame,
    /// so an engine-host resizing/recreating the file (a new size means a
    /// new call here — see `open_existing`'s width/height contract) can
    /// transiently race a concurrent read from the other process. This is
    /// cheaper than a cross-process locking protocol for how rarely a
    /// resize actually happens.
    pub fn create(name: &str, width: u32, height: u32) -> io::Result<Self> {
        // A zero-length mapping is rejected by `memmap2` on every platform,
        // deterministically — not the transient Windows error 1224 this
        // retry loop exists to ride out, so retrying it 20 times would
        // just waste ~190ms before returning the same error anyway.
        if width == 0 || height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("SharedFrameBuffer::create: zero-sized buffer requested ({width}x{height})"),
            ));
        }
        let mut last_err = None;
        for attempt in 0..20 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            match Self::try_create(name, width, height) {
                Ok(buf) => return Ok(buf),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.unwrap())
    }

    /// On Unix the new buffer is a new file renamed over `path`, never the old
    /// one truncated: Unix lets a file be truncated while another process has
    /// it mapped, and that reader's next access past the new end dies with
    /// SIGBUS (manifold did, in `upload_frame`, when a CEF tab resized under
    /// WSL). A reader keeps the old file until it drops its mapping. Windows
    /// refuses to truncate a mapped file instead (error 1224, retried above).
    fn try_create(name: &str, width: u32, height: u32) -> io::Result<Self> {
        let path = frame_path(name);
        let len = (width as u64) * (height as u64) * 4;
        #[cfg(unix)]
        let open_path = path.with_extension(format!("bin.{}.tmp", std::process::id()));
        #[cfg(not(unix))]
        let open_path = path.clone();
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&open_path)?;
        file.set_len(len)?;
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        // Locked before the rename, so the file is never at `path` unlocked.
        #[cfg(unix)]
        if let Err(err) = lock_shared(&file).and_then(|()| std::fs::rename(&open_path, &path)) {
            let _ = std::fs::remove_file(&open_path);
            return Err(err);
        }
        Ok(Self {
            mmap,
            path,
            width,
            height,
            #[cfg(unix)]
            _lock: file,
        })
    }

    /// `InvalidData` when the file isn't `width * height * 4` bytes: on resize the
    /// engine-host truncates and regrows it (Windows) or replaces it (Unix), so a
    /// reader can briefly see the wrong size.
    pub fn open_existing(path: PathBuf, width: u32, height: u32) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let expected = u64::from(width) * u64::from(height) * 4;
        if file.metadata()?.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame buffer file is not width * height * 4 bytes",
            ));
        }
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        #[cfg(unix)]
        lock_shared(&file)?;
        Ok(Self {
            mmap,
            path,
            width,
            height,
            #[cfg(unix)]
            _lock: file,
        })
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn write_frame(&mut self, bgra: &[u8]) {
        let n = bgra.len().min(self.mmap.len());
        self.mmap[..n].copy_from_slice(&bgra[..n]);
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.mmap
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A unique directory per test, so no test sweeps the real temp dir, where
    /// a running manifold's frame files live.
    fn sweep_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manifold-frame-sweep-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_sweep_removes_only_old_frame_files() {
        let dir = sweep_dir("old");
        std::fs::write(dir.join("manifold-frame-a.bin"), [0u8; 8]).unwrap();
        std::fs::write(dir.join("manifold-store-x"), [0u8; 8]).unwrap();
        std::fs::write(dir.join("other.bin"), [0u8; 8]).unwrap();
        assert_eq!(sweep_stale_frames_in(&dir, Duration::from_secs(3600)), 0, "too new");
        assert_eq!(sweep_stale_frames_in(&dir, Duration::ZERO), 1);
        assert!(!dir.join("manifold-frame-a.bin").exists());
        assert!(dir.join("manifold-store-x").exists() && dir.join("other.bin").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A live engine keeps its frame file mapped; Windows refuses an exclusive
    /// open of a mapped file, which is how the sweep tells it apart. The file is
    /// mapped the way `SharedFrameBuffer::create` maps it (the file handle
    /// dropped, only the mapping kept), but in a private directory.
    #[cfg(windows)]
    #[test]
    fn the_sweep_spares_a_mapped_frame_file() {
        let dir = sweep_dir("live");
        let path = dir.join("manifold-frame-live.bin");
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        file.set_len(64).unwrap();
        let live = unsafe { MmapMut::map_mut(&file).unwrap() };
        drop(file);
        assert_eq!(sweep_stale_frames_in(&dir, Duration::ZERO), 0);
        assert!(path.exists(), "a mapped file survives");
        drop(live);
        assert_eq!(sweep_stale_frames_in(&dir, Duration::ZERO), 1);
        assert!(!path.exists(), "an unmapped one goes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// On Unix a mapping blocks nothing, so the sweep goes by the shared lock a
    /// live `SharedFrameBuffer` holds: while one is open the file stays, once it
    /// drops the file goes. `unused` opens its own handle, as another
    /// manifold's sweep would.
    #[cfg(unix)]
    #[test]
    fn the_sweep_spares_a_locked_frame_file() {
        let dir = sweep_dir("locked");
        let path = dir.join("manifold-frame-locked.bin");
        std::fs::write(&path, [0u8; 64]).unwrap();
        let live = SharedFrameBuffer::open_existing(path.clone(), 4, 4).unwrap();
        assert!(!unused(&path), "held");
        assert_eq!(sweep_stale_frames_in(&dir, Duration::ZERO), 0);
        assert!(path.exists(), "a locked file survives");
        drop(live);
        assert!(unused(&path), "released");
        assert_eq!(sweep_stale_frames_in(&dir, Duration::ZERO), 1);
        assert!(!path.exists(), "an unlocked one goes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `create` locks the file it renames into place, and a resize's new file
    /// is locked by its own buffer.
    #[cfg(unix)]
    #[test]
    fn a_created_buffer_holds_its_lock_across_a_resize() {
        let name = format!("test-create-lock-{}", std::process::id());
        let first = SharedFrameBuffer::create(&name, 4, 4).unwrap();
        assert!(!unused(&frame_path(&name)));
        let second = SharedFrameBuffer::create(&name, 2, 2).unwrap();
        drop(first);
        assert!(!unused(&frame_path(&name)), "the resized file is locked");
        drop(second);
        assert!(unused(&frame_path(&name)));
        let _ = std::fs::remove_file(frame_path(&name));
    }

    #[test]
    fn remove_frame_file_deletes_a_released_buffer() {
        let name = format!("test-remove-frame-{}", std::process::id());
        drop(SharedFrameBuffer::create(&name, 4, 4).unwrap());
        assert!(frame_path(&name).exists());
        remove_frame_file(&name);
        assert!(!frame_path(&name).exists());
    }

    #[test]
    fn open_existing_refuses_a_file_of_the_wrong_size() {
        let name = format!("test-open-existing-{}", std::process::id());
        let created = SharedFrameBuffer::create(&name, 4, 2).unwrap();
        assert!(SharedFrameBuffer::open_existing(frame_path(&name), 4, 2).is_ok());
        let err = SharedFrameBuffer::open_existing(frame_path(&name), 8, 8).err().expect("size mismatch");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        drop(created);
        let _ = std::fs::remove_file(frame_path(&name));
    }

    /// A resize (a second `create` under the same name) must leave a reader's
    /// existing mapping readable to its end. Before the fix this failed on Linux:
    /// the reader's bytes read back zeroed, and past the new end they fault.
    #[cfg(unix)]
    #[test]
    fn a_resize_leaves_an_open_reader_whole() {
        let name = format!("test-resize-reader-{}", std::process::id());
        let mut first = SharedFrameBuffer::create(&name, 64, 64).unwrap();
        first.write_frame(&[7; 64 * 64 * 4]);
        let reader = SharedFrameBuffer::open_existing(frame_path(&name), 64, 64).unwrap();
        let second = SharedFrameBuffer::create(&name, 2, 2).unwrap();
        assert!(reader.as_slice().iter().all(|&b| b == 7), "the old frame is intact");
        assert!(SharedFrameBuffer::open_existing(frame_path(&name), 2, 2).is_ok(), "the path has the new size");
        drop((first, second, reader));
        let _ = std::fs::remove_file(frame_path(&name));
    }
}
