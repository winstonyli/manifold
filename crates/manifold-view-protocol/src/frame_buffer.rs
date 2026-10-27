use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use memmap2::MmapMut;

/// A file-backed memory-mapped raw pixel buffer shared between a
/// same-machine view-host and `manifold`. Backed by a temp file rather
/// than a raw Windows shared-memory section or POSIX shm — file-backed
/// mmap works identically on Windows and Linux, which keeps `wpe-sys`'s
/// eventual native (non-WSL2) port free of a second implementation later.
///
/// Pixel format is NOT fixed across engines: Servo's
/// `SoftwareRenderingContext::read_to_image()` writes RGBA8, while CEF's
/// OSR `OnPaint` writes BGRA8 natively. Each view-host's `FrameReady`
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

/// Deterministic from `name` and `generation` alone so manifold can re-derive the path of
/// a frame buffer the view-host created, without the engine having to
/// send the path over the control channel. A file is never rewritten at a new size:
/// the engine creates the next generation's file, so a reader's mapping of the
/// old one stays valid (Windows refuses to truncate a mapped file, error 1224).
pub fn frame_path(name: &str, generation: u32) -> PathBuf {
    std::env::temp_dir().join(format!("manifold-frame-{name}-g{generation}.bin"))
}

/// Deletes every generation's frame file of `name` (errors ignored): the files named
/// `manifold-frame-{name}-g<digits>.bin` exactly, so a name that starts with another's
/// does not match. Only call once its view-host has exited and the caller's own
/// mappings are dropped. Windows deletes a file another process still maps (measured,
/// Windows 11 26200), so a call while a successor runs would take its file away.
pub fn remove_frame_file(name: &str) {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    let prefix = format!("manifold-frame-{name}-g");
    for entry in entries.flatten() {
        let file = entry.file_name();
        let Some(rest) = file.to_str().and_then(|f| f.strip_prefix(prefix.as_str())) else { continue };
        let Some(digits) = rest.strip_suffix(".bin") else { continue };
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Deletes `manifold-frame-*.bin` files in `dir` last modified more than
/// `older_than` ago and not in use. A live view-host keeps its file
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
    OpenOptions::new().read(true).open(path).is_ok_and(|file| lock_probe_unused(file.try_lock()))
}

/// Reads a `try_lock` result: only `WouldBlock` means a live holder. On a filesystem
/// without `flock` (`Unsupported`) no file is ever locked, so the sweep's age limit
/// alone decides; any other error leaves the file alone.
#[cfg(not(windows))]
fn lock_probe_unused(result: Result<(), std::fs::TryLockError>) -> bool {
    match result {
        Ok(()) => true,
        Err(std::fs::TryLockError::WouldBlock) => false,
        Err(std::fs::TryLockError::Error(err)) => err.kind() == io::ErrorKind::Unsupported,
    }
}

/// Takes the shared lock `unused` probes for. On a filesystem without `flock`
/// (`Unsupported`) the file stays unlocked, as before locks existed, and `unused`
/// treats such files as free, so the sweep's age limit is the only guard.
#[cfg(unix)]
fn lock_shared(file: &std::fs::File) -> io::Result<()> {
    match file.lock_shared() {
        Err(err) if err.kind() == io::ErrorKind::Unsupported => Ok(()),
        other => other,
    }
}

impl SharedFrameBuffer {
    /// Creates `frame_path(name, generation)`. `name` should be unique per tab (e.g. a tab id)
    /// so concurrent tabs don't collide on the same backing file. A new size means a new
    /// generation, hence a new file: no reader's mapping can block this call.
    pub fn create(name: &str, generation: u32, width: u32, height: u32) -> io::Result<Self> {
        // A zero-length mapping is rejected by `memmap2` on every platform.
        if width == 0 || height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("SharedFrameBuffer::create: zero-sized buffer requested ({width}x{height})"),
            ));
        }
        Self::try_create(name, generation, width, height)
    }

    /// On Unix the new buffer is a new file renamed over `path`, never the old
    /// one truncated: Unix lets a file be truncated while another process has
    /// it mapped, and that reader's next access past the new end dies with
    /// SIGBUS (manifold did, in `upload_frame`, when a CEF tab resized under
    /// WSL). Names are no longer reused, but this stays (parked: untestable on Windows).
    fn try_create(name: &str, generation: u32, width: u32, height: u32) -> io::Result<Self> {
        let path = frame_path(name, generation);
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

    /// `InvalidData` when the file isn't `width * height * 4` bytes (a `FrameReady` whose
    /// file the engine has since replaced).
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
        // Best-effort: the host's own lock is what protects the file from the sweep, and
        // a reader failing here (e.g. ENOLCK) would blank the tab for nothing.
        #[cfg(unix)]
        let _ = file.lock_shared();
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

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.mmap
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
    use crate::copy_changed_rows;
    use std::time::Duration;

    #[cfg(not(windows))]
    #[test]
    fn lock_probe_treats_only_wouldblock_and_other_errors_as_in_use() {
        use std::fs::TryLockError;
        assert!(lock_probe_unused(Ok(())));
        assert!(lock_probe_unused(Err(TryLockError::Error(io::ErrorKind::Unsupported.into()))));
        assert!(!lock_probe_unused(Err(TryLockError::WouldBlock)));
        assert!(!lock_probe_unused(Err(TryLockError::Error(io::ErrorKind::PermissionDenied.into()))));
    }

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
        let first = SharedFrameBuffer::create(&name, 1, 4, 4).unwrap();
        assert!(!unused(&frame_path(&name, 1)));
        let second = SharedFrameBuffer::create(&name, 2, 2, 2).unwrap();
        drop(first);
        assert!(!unused(&frame_path(&name, 2)), "the resized file is locked");
        drop(second);
        assert!(unused(&frame_path(&name, 2)));
        remove_frame_file(&name);
    }

    #[test]
    fn frame_paths_carry_the_generation() {
        let (one, two) = (frame_path("a", 1), frame_path("a", 2));
        assert_ne!(one, two);
        assert!(one.to_str().unwrap().ends_with("-g1.bin") && two.to_str().unwrap().ends_with("-g2.bin"));
    }

    #[test]
    fn remove_frame_file_deletes_every_generation_of_exactly_that_name() {
        let name = format!("test-remove-frame-{}", std::process::id());
        let longer = format!("{name}-b");
        drop(SharedFrameBuffer::create(&name, 1, 4, 4).unwrap());
        drop(SharedFrameBuffer::create(&name, 2, 4, 4).unwrap());
        drop(SharedFrameBuffer::create(&longer, 1, 4, 4).unwrap());
        remove_frame_file(&name);
        assert!(!frame_path(&name, 1).exists() && !frame_path(&name, 2).exists());
        assert!(frame_path(&longer, 1).exists(), "a name that starts with another's is left alone");
        remove_frame_file(&longer);
        assert!(!frame_path(&longer, 1).exists());
    }

    #[test]
    fn open_existing_refuses_a_file_of_the_wrong_size() {
        let name = format!("test-open-existing-{}", std::process::id());
        let created = SharedFrameBuffer::create(&name, 1, 4, 2).unwrap();
        assert!(SharedFrameBuffer::open_existing(frame_path(&name, 1), 4, 2).is_ok());
        let err = SharedFrameBuffer::open_existing(frame_path(&name, 1), 8, 8).err().expect("size mismatch");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        drop(created);
        remove_frame_file(&name);
    }

    /// A resize is a new generation: it succeeds at once while a reader maps the old
    /// file (a rewrite of that file fails with error 1224 on Windows), and the reader's
    /// mapping stays readable after the old file is deleted.
    #[test]
    fn a_new_generation_does_not_wait_for_a_reader_and_leaves_it_whole() {
        let name = format!("test-resize-reader-{}", std::process::id());
        let mut first = SharedFrameBuffer::create(&name, 1, 64, 64).unwrap();
        first.as_mut_slice().copy_from_slice(&[7; 64 * 64 * 4]);
        let reader = SharedFrameBuffer::open_existing(frame_path(&name, 1), 64, 64).unwrap();
        let second = SharedFrameBuffer::create(&name, 2, 2, 2).unwrap();
        drop(first);
        let _ = std::fs::remove_file(frame_path(&name, 1));
        assert!(reader.as_slice().iter().all(|&b| b == 7), "the old frame is intact");
        assert!(SharedFrameBuffer::open_existing(frame_path(&name, 2), 2, 2).is_ok(), "the new file has the new size");
        drop((second, reader));
        remove_frame_file(&name);
    }

    /// The best and the median of `runs` (manifold-view-protocol cannot see `manifold::bench`).
    fn best_median(mut runs: Vec<Duration>) -> (Duration, Duration) {
        runs.sort();
        (runs[0], runs[runs.len() / 2])
    }

    fn time_it(what: &str, n: usize, mut f: impl FnMut()) {
        f(); // warm up (first touch of the mapping, caches)
        let runs: Vec<Duration> = (0..n)
            .map(|_| {
                let start = std::time::Instant::now();
                f();
                start.elapsed()
            })
            .collect();
        let (best, median) = best_median(runs);
        println!("  {what:<58} best {best:>10.1?}  median {median:>10.1?}");
    }

    /// What the engines' frame copies and the app's per-frame map cost at 1920x1080. Run: `cargo test --release -p manifold-view-protocol handoff_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing bench; run by hand"]
    fn handoff_timing() {
        use std::hint::black_box;
        const N: usize = 200;
        let (w, h) = (1920usize, 1080usize);
        let row_bytes = w * 4;
        let pitch = row_bytes + 256; // a padded D3D-style source
        let pitched_len = pitch * (h - 1) + row_bytes;
        // Two different frames, so "every row changed" can alternate between them.
        let pitched: Vec<Vec<u8>> =
            (0..2u8).map(|k| (0..pitched_len).map(|i| (i as u8).wrapping_mul(7).wrapping_add(k)).collect()).collect();
        let tight: Vec<Vec<u8>> = (0..2u8)
            .map(|k| (0..row_bytes * h).map(|i| (i as u8).wrapping_mul(13).wrapping_add(k)).collect())
            .collect();

        let name = format!("handoff-timing-{}", std::process::id());
        let mut buf = SharedFrameBuffer::create(&name, 1, w as u32, h as u32).unwrap();
        println!("handoff_timing: {w}x{h}, {} bytes/frame, pitch {pitch}, N = {N}", row_bytes * h);

        let mut k = 0usize;
        time_it("1 WebView2 today: vec + pitched row copy + file copy", N, || {
            k ^= 1;
            let mut v = vec![0u8; w * h * 4];
            for y in 0..h {
                v[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(&pitched[k][y * pitch..y * pitch + row_bytes]);
            }
            let v = black_box(v);
            let n = v.len().min(buf.mmap.len());
            buf.mmap[..n].copy_from_slice(&v[..n]);
        });
        time_it("2 CEF today: to_vec + file copy", N, || {
            k ^= 1;
            let v = black_box(tight[k].to_vec());
            let n = v.len().min(buf.mmap.len());
            buf.mmap[..n].copy_from_slice(&v[..n]);
        });
        time_it("3 direct pitched row copy into the mapping", N, || {
            k ^= 1;
            for (y, row) in buf.mmap.chunks_exact_mut(row_bytes).enumerate() {
                row.copy_from_slice(&pitched[k][y * pitch..y * pitch + row_bytes]);
            }
            black_box(&buf.mmap);
        });
        time_it("4 compare-then-copy, every row changed", N, || {
            k ^= 1;
            black_box(copy_changed_rows(&mut buf.mmap, &pitched[k], row_bytes, pitch));
        });
        black_box(copy_changed_rows(&mut buf.mmap, &pitched[0], row_bytes, pitch));
        time_it("5 compare-then-copy, no row changed", N, || {
            black_box(copy_changed_rows(&mut buf.mmap, &pitched[0], row_bytes, pitch));
        });
        let path = frame_path(&name, 1);
        time_it("6 app today: open_existing + read every byte + drop", N, || {
            let r = SharedFrameBuffer::open_existing(path.clone(), w as u32, h as u32).unwrap();
            black_box(r.as_slice().iter().fold(0u8, |a, &b| a ^ b));
        });
        let reader = SharedFrameBuffer::open_existing(path.clone(), w as u32, h as u32).unwrap();
        time_it("7 persistent mapping: read every byte", N, || {
            black_box(reader.as_slice().iter().fold(0u8, |a, &b| a ^ b));
        });
        drop((reader, buf));
        remove_frame_file(&name);
    }
}
