//! The few `libc` calls swamp makes: `statfs(2)` (volume flags and
//! filesystem type), a non-blocking `flock(2)` probe, and an
//! `O_NOFOLLOW` open of a regular file (Cargo's build lock, a reviewed
//! Cargo member). `unsafe` exists in this crate only under `fs_gate`
//! (`#![deny(unsafe_code)]` at the crate root, allowed here).

#![allow(unsafe_code)]

use std::io;
use std::path::Path;

/// What `statfs(2)` says about the volume holding a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// `f_flags` (macOS `MNT_*` bits; Linux `ST_*` bits).
    pub flags: u64,
    /// `f_fstypename` on macOS (`"apfs"`, `"hfs"`, …); empty elsewhere.
    pub type_name: String,
    /// `f_type` magic on Linux; `0` elsewhere.
    pub type_magic: u64,
}

impl VolumeInfo {
    /// A supported local filesystem (never a network or unknown mount).
    pub fn is_local(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.flags & libc::MNT_LOCAL as u64 != 0
        }
        #[cfg(target_os = "linux")]
        {
            matches!(
                self.type_magic,
                0xef53 | 0x9123683e | 0x58465342 | 0x01021994
            )
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            false
        }
    }

    /// An APFS volume (copy-on-write extent sharing possible).
    pub fn is_apfs(&self) -> bool {
        self.type_name.eq_ignore_ascii_case("apfs")
    }
}

#[cfg(unix)]
pub fn volume_info(path: &Path) -> io::Result<VolumeInfo> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    let mut buf = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `c` is a valid NUL-terminated path and `buf` is a properly
    // sized, writable `statfs`; on success the kernel initialized it.
    if unsafe { libc::statfs(c.as_ptr(), buf.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statfs returned 0, so `buf` is initialized.
    let sf = unsafe { buf.assume_init() };
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `f_fstypename` is a NUL-terminated C string.
        let name = unsafe { std::ffi::CStr::from_ptr(sf.f_fstypename.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        Ok(VolumeInfo {
            flags: sf.f_flags as u64,
            type_name: name,
            type_magic: 0,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(VolumeInfo {
            flags: 0,
            type_name: String::new(),
            type_magic: sf.f_type as u64,
        })
    }
}

#[cfg(not(unix))]
pub fn volume_info(_path: &Path) -> io::Result<VolumeInfo> {
    Err(io::Error::other("statfs is unix-only"))
}

/// Advisory-lock probe: attempts (and immediately releases) a
/// non-blocking exclusive `flock` on `lock_path`. `Ok(true)` means the
/// lock was free (this call briefly held and released it); `Ok(false)`
/// means another process currently holds it. Never writes; never blocks.
#[cfg(unix)]
pub fn flock_is_free(lock_path: &Path) -> io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new().read(true).open(lock_path)?;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is an open descriptor owned by `file` for this scope.
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        // SAFETY: as above; releasing the lock this call just took.
        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }
        Ok(true)
    } else {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(false)
        } else {
            Err(err)
        }
    }
}

/// A regular file opened with `O_NOFOLLOW | O_NONBLOCK`: never a symlink,
/// never a FIFO that would block. Exposes only what Cargo cleanup needs:
/// its `fstat`, a whole-content digest (the member's identity, never
/// returned as bytes), and Cargo's advisory build lock.
#[derive(Debug)]
pub struct RegularFile(std::fs::File);

impl RegularFile {
    #[cfg(unix)]
    pub fn open_nofollow(path: &Path) -> io::Result<RegularFile> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other(format!(
                "not a regular file: {}",
                path.display()
            )));
        }
        Ok(RegularFile(file))
    }

    pub fn metadata(&self) -> io::Result<std::fs::Metadata> {
        self.0.metadata()
    }

    /// blake3 of the whole content. The bytes never leave this function.
    pub fn digest(&mut self) -> io::Result<String> {
        use std::io::Read;
        let mut hash = blake3::Hasher::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = self.0.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        Ok(hash.finalize().to_hex().to_string())
    }

    /// Non-blocking exclusive advisory lock (Cargo's build lock).
    pub fn try_lock(&self) -> io::Result<()> {
        self.0.try_lock().map_err(|e| match e {
            std::fs::TryLockError::Error(e) => e,
            std::fs::TryLockError::WouldBlock => {
                io::Error::new(io::ErrorKind::WouldBlock, "lock held by another process")
            }
        })
    }

    pub fn unlock(&self) -> io::Result<()> {
        self.0.unlock()
    }

    pub fn try_clone(&self) -> io::Result<RegularFile> {
        Ok(RegularFile(self.0.try_clone()?))
    }
}

/// This process's real uid, for a `loginctl show-user <uid>` argument
/// (`systemd_user::linger`).
#[cfg(unix)]
pub fn current_uid() -> u32 {
    // SAFETY: getuid cannot fail and reads no memory.
    unsafe { libc::getuid() }
}

/// Opens (creating if missing, and creating its parent directory) a
/// file meant only to be `flock`ed -- `crate::continuity`'s per-root
/// lock files. The lock itself is then a safe, non-`libc` call
/// (`File::try_lock`/`try_lock_shared`/`unlock`, stable since the
/// pinned toolchain) the caller makes directly on the handle this
/// returns; only the *open* is gated.
pub fn open_for_lock(path: &Path) -> io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
}

/// Opens an existing file read-only, for a lock probe that must never
/// create the file (`crate::continuity::collector_alive`: asking must
/// leave no state behind). `Ok(None)` when it does not exist.
pub fn open_for_lock_probe(path: &Path) -> io::Result<Option<std::fs::File>> {
    match std::fs::OpenOptions::new().read(true).open(path) {
        Ok(f) => Ok(Some(f)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Set by SIGINT/SIGTERM once [`install_stop_signal_handlers`] installed
/// the handler (`crate::continuity::stop_on_signals`, the collector's
/// clean-shutdown flag).
#[cfg(target_os = "linux")]
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "linux")]
extern "C" fn on_stop_signal(_sig: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Installs `on_stop_signal` for SIGINT and SIGTERM and returns the flag
/// it sets.
#[cfg(target_os = "linux")]
pub fn install_stop_signal_handlers() -> &'static std::sync::atomic::AtomicBool {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_stop_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            on_stop_signal as *const () as libc::sighandler_t,
        );
    }
    &STOP
}

/// Whether `e` is `ENOSPC` -- on Linux, `inotify_add_watch` returning it
/// means `fs.inotify.max_user_watches` is exhausted. The one named
/// `libc::` constant a caller outside the gate (`live_watch::LiveTree`,
/// portable over its `Kernel` trait) needs, so it does not have to name
/// `libc` itself.
#[cfg(unix)]
pub fn is_enospc(e: &io::Error) -> bool {
    e.raw_os_error() == Some(libc::ENOSPC)
}

#[cfg(not(unix))]
pub fn is_enospc(_e: &io::Error) -> bool {
    false
}
