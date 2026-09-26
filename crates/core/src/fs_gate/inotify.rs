//! Linux inotify: the one kernel backend `crate::live_watch::LiveTree`
//! drives through its `Kernel` trait. Every raw syscall a live watch
//! makes (`inotify_init1`, `poll`, `read`, `inotify_add_watch`,
//! `inotify_rm_watch`, `close`) lives here, inside the capability gate
//! (`docs/architecture.md`, "Capability gates"), rather than in
//! `live_watch.rs` itself -- which stays pure over the `Kernel` trait so
//! its state machine runs as a unit test on either platform.

#![allow(unsafe_code)]

use crate::live_watch::{Kernel, Limits, RawEvent};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::io::RawFd;
use std::path::Path;

/// An inotify instance. Non-blocking, close-on-exec.
pub struct Inotify {
    fd: RawFd,
}

impl Inotify {
    pub fn new() -> std::io::Result<Self> {
        // SAFETY: plain syscall, no pointers.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { fd })
    }

    pub fn fd(&self) -> RawFd {
        self.fd
    }

    /// Waits up to `timeout_ms` for events, then reads every event
    /// available. An empty vec means nothing arrived.
    pub fn read(&self, timeout_ms: i32) -> std::io::Result<Vec<RawEvent>> {
        let mut pfd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                return Ok(Vec::new());
            }
            return Err(e);
        }
        let mut out = Vec::new();
        if n == 0 {
            return Ok(out);
        }
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: `buf` is valid for its length.
            let r = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if r < 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EAGAIN) {
                    break;
                }
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if r == 0 {
                break;
            }
            parse(&buf[..r as usize], &mut out);
        }
        Ok(out)
    }
}

/// Parses a buffer of `struct inotify_event`s.
pub fn parse(mut buf: &[u8], out: &mut Vec<RawEvent>) {
    const HEADER: usize = 16;
    while buf.len() >= HEADER {
        let wd = i32::from_ne_bytes(buf[0..4].try_into().unwrap());
        let mask = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
        let cookie = u32::from_ne_bytes(buf[8..12].try_into().unwrap());
        let len = u32::from_ne_bytes(buf[12..16].try_into().unwrap()) as usize;
        let end = (HEADER + len).min(buf.len());
        let raw = &buf[HEADER..end];
        let name_end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        let name = (name_end > 0).then(|| OsString::from_vec(raw[..name_end].to_vec()));
        out.push(RawEvent {
            wd,
            mask,
            cookie,
            name,
        });
        buf = &buf[end..];
    }
}

impl Inotify {
    /// A watch for swamp's own control files (a collector's sync
    /// requests): created or renamed-in files only. Same instance as
    /// the tree, so its events are ordered with the tree's.
    pub fn add_control(&mut self, dir: &Path) -> std::io::Result<i32> {
        let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let m = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_ONLYDIR;
        // SAFETY: valid fd and NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), m) };
        if wd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(wd)
    }
}

impl Kernel for Inotify {
    fn add(&mut self, dir: &Path) -> std::io::Result<i32> {
        let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: valid fd and NUL-terminated path.
        let wd =
            unsafe { libc::inotify_add_watch(self.fd, c.as_ptr(), crate::live_watch::mask::WATCH) };
        if wd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(wd)
    }

    fn remove(&mut self, wd: i32) {
        // SAFETY: plain syscall; an already-removed wd is EINVAL,
        // which is harmless here.
        unsafe {
            libc::inotify_rm_watch(self.fd, wd);
        }
    }
}

impl Drop for Inotify {
    fn drop(&mut self) {
        // SAFETY: we own the fd.
        unsafe {
            libc::close(self.fd);
        }
    }
}

/// `fs.inotify.*` as this user sees them.
pub fn limits() -> Limits {
    let read = |p: &str| -> Option<u64> {
        match std::fs::read_to_string(p) {
            Ok(s) => s.trim().parse().ok(),
            Err(_) => None,
        }
    };
    Limits {
        max_user_watches: read("/proc/sys/fs/inotify/max_user_watches"),
        max_queued_events: read("/proc/sys/fs/inotify/max_queued_events"),
    }
}

#[cfg(test)]
mod tests {
    use crate::live_watch::mask;

    #[test]
    fn the_declared_bits_are_the_kernels() {
        assert_eq!(mask::IN_MODIFY, libc::IN_MODIFY);
        assert_eq!(mask::IN_ATTRIB, libc::IN_ATTRIB);
        assert_eq!(mask::IN_CLOSE_WRITE, libc::IN_CLOSE_WRITE);
        assert_eq!(mask::IN_MOVED_FROM, libc::IN_MOVED_FROM);
        assert_eq!(mask::IN_MOVED_TO, libc::IN_MOVED_TO);
        assert_eq!(mask::IN_CREATE, libc::IN_CREATE);
        assert_eq!(mask::IN_DELETE, libc::IN_DELETE);
        assert_eq!(mask::IN_DELETE_SELF, libc::IN_DELETE_SELF);
        assert_eq!(mask::IN_MOVE_SELF, libc::IN_MOVE_SELF);
        assert_eq!(mask::IN_UNMOUNT, libc::IN_UNMOUNT);
        assert_eq!(mask::IN_Q_OVERFLOW, libc::IN_Q_OVERFLOW);
        assert_eq!(mask::IN_IGNORED, libc::IN_IGNORED);
        assert_eq!(mask::IN_ONLYDIR, libc::IN_ONLYDIR);
        assert_eq!(mask::IN_DONT_FOLLOW, libc::IN_DONT_FOLLOW);
        assert_eq!(mask::IN_EXCL_UNLINK, libc::IN_EXCL_UNLINK);
        assert_eq!(mask::IN_ISDIR, libc::IN_ISDIR);
    }
}
