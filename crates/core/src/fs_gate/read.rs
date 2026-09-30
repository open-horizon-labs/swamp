//! Content reads. Two kinds, and only two:
//!
//! * [`bounded_read`] -- at most a [`BoundedCap`] of bytes from the start
//!   of any file. The only way to read a file swamp does not own (a
//!   session header, a manifest, a `.git` pointer file, a plist). The cap
//!   is a type, not an argument a caller can pass `usize::MAX` to: every
//!   cap is one of the named constants below.
//! * [`read_owned`] / [`read_owned_string`] -- a whole file, for swamp's
//!   own state (plans, grants, the ledger, the protect list, config, the
//!   FSEvents cursor). The audit allows only the store modules to name
//!   them.
//!
//! Nothing here returns a handle a caller could keep reading from.

use std::io::{self, Read};
use std::path::Path;

/// A named ceiling on one content read. Constructed only from the
/// constants on this type, so "how much may this read" is always one of
/// a reviewed handful of answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundedCap(usize);

impl BoundedCap {
    /// One session header line (a JSON object). 64 KiB is generous for
    /// that and far below any transcript
    /// (`.oh/guardrails/agent-adapters-read-bounded-headers-only.md`).
    pub const HEADER: BoundedCap = BoundedCap(64 * 1024);
    /// A project manifest or a tool's own small config (`package.json`,
    /// `Cargo.toml`, `.nvmrc`, `compose.yaml`, `.npmrc`, `settings.xml`).
    pub const MANIFEST: BoundedCap = BoundedCap(1024 * 1024);
    /// A build-adapter manifest read *whole* (the fields an adapter wants
    /// are scattered through it): `package.json`, `pom.xml`, a Cargo
    /// `.fingerprint` JSON, an Android `source.properties`, a Python
    /// `pyvenv.cfg`, an Xcode `Info.plist`. Smaller than [`Self::MANIFEST`]
    /// on purpose: a `node_modules` tree holds one such file per installed
    /// package, so the per-file cap stays tight even though the total read
    /// across a big tree is not.
    pub const BUILD_MANIFEST: BoundedCap = BoundedCap(256 * 1024);
    /// A dependency lockfile (`Cargo.lock`, `package-lock.json`,
    /// `pnpm-lock.yaml`, `go.sum`, `gradle.lockfile`, `pom.xml`): large in
    /// big projects, still bounded. A larger one is an explicit evidence
    /// gap, never a silently truncated parse.
    pub const LOCKFILE: BoundedCap = BoundedCap(16 * 1024 * 1024);
    /// A small pointer/config file: `.git` (gitdir line), `commondir`,
    /// `HEAD`, a git `config`.
    pub const POINTER: BoundedCap = BoundedCap(16 * 1024);
    /// A system table read whole in practice (`/proc/mounts`).
    pub const SYSTEM_TABLE: BoundedCap = BoundedCap(1024 * 1024);

    pub const fn bytes(self) -> usize {
        self.0
    }

    /// The header cap narrowed to `n` (never widened past it).
    pub fn header_at_most(n: usize) -> BoundedCap {
        BoundedCap(n.min(Self::HEADER.0))
    }
}

/// What a bounded read returned: at most the cap, and whether the file
/// had more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedBytes {
    pub bytes: Vec<u8>,
    /// The file was longer than the cap; `bytes` is a prefix.
    pub truncated: bool,
}

impl BoundedBytes {
    pub fn lossy(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    /// The whole file as UTF-8, or `None` when it was truncated or not
    /// UTF-8: a parser must never see a prefix it would take for a whole.
    pub fn complete_utf8(self) -> Option<String> {
        if self.truncated {
            return None;
        }
        String::from_utf8(self.bytes).ok()
    }
}

/// macOS `SF_DATALESS` (`sys/stat.h`): the file's contents live with a
/// file provider (iCloud Drive, a CloudStorage domain) and opening it
/// asks the provider to download them first.
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

/// Whether `meta` describes a dataless placeholder (always false off
/// macOS).
pub(crate) fn is_dataless(meta: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as _;
        meta.st_flags() & SF_DATALESS != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = meta;
        false
    }
}

/// True when `path` exists but opening it for reading could block: it is
/// a FIFO or a device, or it is a dataless
/// placeholder. For callers that hand a path to a library which opens it
/// itself (gix), so they can decline before that open (#190).
pub(crate) fn would_block_on_open(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    // A socket is not listed: `open(2)` on one fails at once (`ENXIO`),
    // and git's own fsmonitor daemon keeps one inside `.git`.
    match std::fs::metadata(path) {
        Ok(m) => {
            let ft = m.file_type();
            ft.is_fifo() || ft.is_block_device() || ft.is_char_device() || is_dataless(&m)
        }
        Err(_) => false,
    }
}

/// Why a content read was refused before `open(2)` could block (#190).
fn refused(path: &Path, what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{} is {what}; not read", path.display()),
    )
}

/// Opens `path` for a content read without ever blocking in `open(2)`.
///
/// `File::open` on a FIFO waits until some other process opens the write
/// end, which may be never: one `.git/config` FIFO parked a discovery
/// worker in `__open` for good while every other worker idled in
/// `Pool::next` at 0% CPU (#190). A dataless file provider placeholder
/// blocks the same way while the provider downloads it.
///
/// So the refusal is a `stat`, never an open: opening a FIFO's read end,
/// even with `O_NONBLOCK`, completes the rendezvous for a writer parked
/// on it, and when swamp closes it that writer's next write fails with
/// `EPIPE` and its data is lost. Only a path `stat` calls a regular,
/// non-dataless file is opened, with `O_NONBLOCK` and an `fstat` as the
/// backstop for a swap between the two calls. Symlinks are followed: a
/// symlinked manifest is read, a symlink to a FIFO is not.
fn open_regular(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(refused(path, "not a regular file"));
    }
    if is_dataless(&meta) {
        return Err(refused(path, "a dataless file provider placeholder"));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(refused(path, "not a regular file"));
    }
    Ok(file)
}

/// Reads at most `cap` bytes from the start of `path`. Only a regular
/// file is read: a FIFO, socket, device or dataless placeholder is an
/// error, never a wait (see [`open_regular`]).
pub fn bounded_read(path: impl AsRef<Path>, cap: BoundedCap) -> io::Result<BoundedBytes> {
    let file = open_regular(path.as_ref())?;
    // The length the `fstat` reports tells a complete file from a
    // truncated one without reading a byte past the cap.
    let len = file.metadata()?.len();
    let mut buf = Vec::with_capacity((cap.0 as u64).min(len).min(1 << 20) as usize);
    file.take(cap.0 as u64).read_to_end(&mut buf)?;
    let truncated = len > buf.len() as u64;
    Ok(BoundedBytes {
        bytes: buf,
        truncated,
    })
}

/// A whole small file (a manifest, a tool's own config, a `.git`
/// pointer) as UTF-8, through [`bounded_read`]: `Err(InvalidData)` when
/// the file is larger than `cap` or not UTF-8, so a parser never sees a
/// prefix it would take for the whole.
pub fn bounded_string(path: impl AsRef<Path>, cap: BoundedCap) -> io::Result<String> {
    let path = path.as_ref();
    let read = bounded_read(path, cap)?;
    if read.truncated {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is larger than the {}-byte read bound",
                path.display(),
                cap.bytes()
            ),
        ));
    }
    String::from_utf8(read.bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// [`bounded_read`] of an agent session header, counted as header bytes
/// by `work_counters` (what "zero header reads on an unchanged pass"
/// measures).
pub fn bounded_read_header(path: impl AsRef<Path>, cap: BoundedCap) -> io::Result<BoundedBytes> {
    let read = bounded_read(path, cap)?;
    crate::work_counters::record_header_bytes(read.bytes.len() as u64);
    Ok(read)
}

/// What a byte scanner says after each byte of a streamed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    /// Feed the next byte.
    More,
    /// This byte completed the field; read nothing after it.
    Done,
}

/// How a streamed header read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOutcome {
    /// Bytes fetched from the file: exactly the bytes the scanner saw.
    pub bytes_read: usize,
    /// The scanner said [`Scan::Done`] (else the cap or the file ended).
    pub done: bool,
}

/// Streams a header to `scan` one byte per read, and stops at the byte
/// `scan` marks [`Scan::Done`] -- or at `cap`, or at the end of the file.
///
/// The difference from [`bounded_read_header`] is what is *in memory*:
/// that reads the whole cap into a buffer before a parser sees any of
/// it, so a field 300 bytes in costs holding 8 KiB of whatever follows
/// it. Here no byte past the stop is fetched from the file and no byte
/// is retained by this function; only what `scan` chooses to keep exists
/// afterwards. The price is one `read(2)` per byte, which is why it is
/// for a field that sits near the start of a record, never for a whole
/// header. Counted as header bytes by `work_counters`, so a test can
/// assert the read ended at the field, not merely under the cap.
pub fn bounded_scan_header(
    path: impl AsRef<Path>,
    cap: BoundedCap,
    scan: &mut dyn FnMut(u8) -> Scan,
) -> io::Result<ScanOutcome> {
    let mut file = open_regular(path.as_ref())?;
    let mut byte = [0u8; 1];
    let mut bytes_read = 0usize;
    let mut done = false;
    while bytes_read < cap.0 {
        if file.read(&mut byte)? == 0 {
            break;
        }
        bytes_read += 1;
        if scan(byte[0]) == Scan::Done {
            done = true;
            break;
        }
    }
    crate::work_counters::record_header_bytes(bytes_read as u64);
    Ok(ScanOutcome { bytes_read, done })
}

/// [`read_owned`], as UTF-8.
pub fn read_owned_string(path: impl AsRef<Path>) -> io::Result<String> {
    std::fs::read_to_string(path)
}
