use std::fs::OpenOptions;
use std::io;
use std::path::PathBuf;

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
}

/// Deterministic from `name` alone so manifold can re-derive the path of
/// a frame buffer the engine-host created, without the engine having to
/// send the path over the control channel.
pub fn frame_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("manifold-frame-{name}.bin"))
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

    fn try_create(name: &str, width: u32, height: u32) -> io::Result<Self> {
        let path = frame_path(name);
        let len = (width as u64) * (height as u64) * 4;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.set_len(len)?;
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self { mmap, path, width, height })
    }

    pub fn open_existing(path: PathBuf, width: u32, height: u32) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self { mmap, path, width, height })
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
