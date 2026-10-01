//! The few `libc` calls swamp makes: `statfs(2)` (volume flags and
//! filesystem type), a non-blocking `flock(2)` probe, and an
//! `O_NOFOLLOW` open of a regular file (Cargo's build lock, a reviewed
//! Cargo member). `unsafe` exists in this crate only under `fs_gate`
//! (`#![deny(unsafe_code)]` at the crate root, allowed here).

#![allow(unsafe_code)]

use std::io;
use std::path::Path;

/// Whether a per-entry I/O error means the entry was gone (or its name
/// no longer resolved) by the time it was looked at: `ENOENT`, or
/// `ESTALE` (overlayfs and network filesystems return it for an entry
/// whose lower-layer or server-side object went away between the
/// directory listing and the `lstat`). A walker skips such an entry; any
/// other per-entry error is "not measured" for that entry only.
pub fn is_vanished_entry(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESTALE)
}

/// A raw OS error for a fault-injection test, by errno name. Test-only:
/// the tests outside `fs_gate` must not name `libc`.
#[cfg(test)]
pub(crate) fn errno_for_test(name: &str) -> io::Error {
    let code = match name {
        "ENOENT" => libc::ENOENT,
        "ESTALE" => libc::ESTALE,
        "EIO" => libc::EIO,
        "EACCES" => libc::EACCES,
        "ENOTEMPTY" => libc::ENOTEMPTY,
        other => panic!("unknown errno {other}"),
    };
    io::Error::from_raw_os_error(code)
}

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
        // Refuse by `lstat` first, never by opening: opening a FIFO's read
        // end, even non-blocking, releases a writer parked on it, whose
        // next write then fails with EPIPE once this end closes (#190).
        // The O_NOFOLLOW|O_NONBLOCK open and `fstat` below stay as the
        // backstop for a swap between the two calls.
        if !std::fs::symlink_metadata(path)?.is_file() {
            return Err(io::Error::other(format!(
                "not a regular file: {}",
                path.display()
            )));
        }
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

/// This process's real uid (`fs_gate::current_uid`; also the
/// `loginctl show-user <uid>` argument in `systemd_user::linger`).
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
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
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

#[cfg(all(test, unix))]
mod open_nofollow_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Tempting wrong patch (the old code): "O_NONBLOCK makes the open
    /// safe, then fstat refuses". Safe for swamp; the writer parked on
    /// the FIFO is released and loses its data.
    #[test]
    fn refusing_a_fifo_does_not_wake_a_waiting_writer() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("member");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let (tx, rx) = mpsc::channel::<()>();
        let w = fifo.clone();
        std::thread::spawn(move || {
            let _f = std::fs::OpenOptions::new().write(true).open(&w);
            let _ = tx.send(());
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(RegularFile::open_nofollow(&fifo).is_err());
        let woke = rx.recv_timeout(Duration::from_secs(1)).is_ok();
        if !woke {
            use std::os::unix::fs::OpenOptionsExt;
            let _r = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo);
        }
        assert!(!woke, "the refusal opened the FIFO and released its writer");
    }
}
/// Lowers the calling thread to background scheduling: on macOS the
/// Darwin background band (`PRIO_DARWIN_BG` on this thread: CPU and disk
/// I/O yield to foreground work), on Linux nice 10. The volume pass's
/// workers call it first so a whole-disk measurement never competes with
/// what the person is doing. Best effort: a refusal changes nothing else.
#[cfg(target_os = "macos")]
pub fn lower_current_thread_priority() {
    // SAFETY: setpriority on this thread takes no pointers; failure is
    // reported through errno and deliberately ignored.
    unsafe {
        libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG);
    }
}

#[cfg(target_os = "linux")]
pub fn lower_current_thread_priority() {
    // SAFETY: gettid and setpriority take no pointers; failure is ignored.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn lower_current_thread_priority() {}

/// Test fixture: a named pipe at `path` (nothing ever opens it). The
/// walker must measure a tree holding one without blocking on it.
#[cfg(all(unix, any(test, feature = "testing")))]
pub fn make_fifo_for_test(path: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
    // SAFETY: `c` is a valid NUL-terminated pathname for the call.
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The home directory the account database names for this process's uid
/// (`getpwuid_r`), which is not `$HOME`: a sandbox or a test points
/// `$HOME` somewhere else. `None` when the database has no entry.
#[cfg(unix)]
pub fn account_home() -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut buf = vec![0 as libc::c_char; 8192];
    // SAFETY: an all-zero `passwd` is a valid out-parameter for getpwuid_r.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call and `buf.len()` is its size.
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
    if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // SAFETY: on success `pw_dir` is a NUL-terminated string inside `buf`.
    let dir = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) };
    Some(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
        dir.to_bytes(),
    )))
}

#[cfg(not(unix))]
pub fn account_home() -> Option<std::path::PathBuf> {
    None
}

/// One mounted filesystem, from the OS's own mount table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPoint {
    pub path: std::path::PathBuf,
    pub fs_type: String,
    /// On this machine's own storage (not a network share).
    pub local: bool,
}

/// The mount table: `getmntinfo` on macOS (a snapshot, no I/O to the
/// mounted volumes), `/proc/self/mounts` on Linux. Empty when the table
/// cannot be read: the caller then knows of no mounts, and says so.
#[cfg(target_os = "macos")]
pub fn mount_points() -> Vec<MountPoint> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: `buf` receives a pointer to libc-owned memory that stays
    // valid until the next `getmntinfo` call on this thread; entries are
    // copied out before returning and never freed here.
    let n = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    if n <= 0 || buf.is_null() {
        return Vec::new();
    }
    // SAFETY: `getmntinfo` returned `n` initialized entries at `buf`.
    let entries = unsafe { std::slice::from_raw_parts(buf, n as usize) };
    entries
        .iter()
        .map(|e| {
            // SAFETY: the name fields are NUL-terminated C strings.
            let on = unsafe { std::ffi::CStr::from_ptr(e.f_mntonname.as_ptr()) };
            let ty = unsafe { std::ffi::CStr::from_ptr(e.f_fstypename.as_ptr()) };
            MountPoint {
                path: std::path::PathBuf::from(std::ffi::OsStr::from_bytes(on.to_bytes())),
                fs_type: ty.to_string_lossy().into_owned(),
                local: e.f_flags & libc::MNT_LOCAL as u32 != 0,
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub fn mount_points() -> Vec<MountPoint> {
    let Ok(table) =
        super::read::bounded_read("/proc/self/mounts", super::read::BoundedCap::SYSTEM_TABLE)
    else {
        return Vec::new();
    };
    table
        .lossy()
        .lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let _dev = f.next()?;
            let path = f.next()?.replace("\\040", " ");
            let ty = f.next()?.to_string();
            let network = matches!(
                ty.as_str(),
                "nfs" | "nfs4" | "cifs" | "smbfs" | "fuse.sshfs"
            );
            Some(MountPoint {
                path: std::path::PathBuf::from(path),
                fs_type: ty,
                local: !network,
            })
        })
        .collect()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn mount_points() -> Vec<MountPoint> {
    Vec::new()
}

/// A group's id by name (`getgrnam`); `None` when there is no such group.
pub(crate) fn group_id(name: &str) -> Option<u32> {
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: getgrnam returns null or a pointer into static storage; one
    // field is read at once and nothing is kept.
    let g = unsafe { libc::getgrnam(c.as_ptr()) };
    (!g.is_null()).then(|| unsafe { (*g).gr_gid })
}

/// A group's name by id (`getgrgid`), for plain refusal text; `gid N` when
/// the id has no name.
pub(crate) fn group_name(gid: u32) -> String {
    // SAFETY: as in `group_id`.
    let g = unsafe { libc::getgrgid(gid) };
    if g.is_null() {
        return format!("gid {gid}");
    }
    unsafe { std::ffi::CStr::from_ptr((*g).gr_name) }
        .to_string_lossy()
        .into_owned()
}

/// Whether a process with this pid exists: `kill(pid, 0)`. `EPERM` (it
/// exists but belongs to someone else) is alive; `ESRCH` is gone. A pid
/// that does not fit a `pid_t`, or 0 (this process group), is not alive.
pub fn pid_alive(pid: u32) -> bool {
    let Ok(p) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if p <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks; nothing is delivered.
    if unsafe { libc::kill(p, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod pid_alive_tests {
    /// A live child, the same child after it is reaped, and pid 1 (alive,
    /// not ours: `EPERM`). Tempting wrong patch: `kill(pid, 0) == 0` only,
    /// which reads another user's live lock holder as dead and reclaims it.
    #[test]
    fn live_dead_and_eperm() {
        assert!(super::pid_alive(std::process::id()));
        assert!(super::pid_alive(1), "pid 1 exists (EPERM for a user)");
        assert!(!super::pid_alive(0));
        assert!(!super::pid_alive(u32::MAX));
        // A pid that existed and is gone: a short-lived thread is not a
        // process, so use a forked child via libc, reaped before the check.
        // SAFETY: the child calls only _exit.
        let child = unsafe { libc::fork() };
        if child == 0 {
            unsafe { libc::_exit(0) };
        }
        assert!(child > 0);
        let mut status = 0;
        // SAFETY: waiting for our own child.
        unsafe { libc::waitpid(child, &mut status, 0) };
        assert!(!super::pid_alive(child as u32), "a reaped child is gone");
    }
}
