//! Filesystem space, from the kernel rather than from `df`'s output.
//!
//! `df -k` was parsed by field index (`fields[3]` = "Available"), which
//! is a macOS layout assumption in two ways. GNU coreutils `df` prints a
//! different header and, on a long device name, *wraps the row onto a
//! second line* -- so `fields[3]` on Linux can be the capacity percentage
//! or nothing at all. It also honours `POSIXLY_CORRECT`, `BLOCK_SIZE` and
//! `DF_BLOCK_SIZE` from the environment, which a measurement must not
//! depend on. And every call was a subprocess spawn on a path that
//! already counts its spawns.
//!
//! Semantics, kept explicit because the two are not the same number:
//!
//! * **available** is what an unprivileged process may actually use --
//!   what `df`'s "Avail" column shows, and what "you would get this much
//!   back" has to be measured against.
//! * **free** includes blocks reserved for root. Reporting it as free
//!   space would overstate what a cleanup can recover.
//!
//! Only *available* is exposed: nothing here has a use for the reserved
//! blocks, and offering both invites picking the flattering one.
//!
//! ## Why two syscalls rather than one
//!
//! POSIX `statvfs` is on both targets and was the obvious single answer.
//! It is the wrong one on macOS: Darwin's `fsblkcnt_t` is `c_uint`, so
//! `statvfs`'s `f_blocks`/`f_bavail` are **32-bit**, and on a volume with
//! more than 2^32 blocks the call either overflows silently or fails with
//! `EOVERFLOW`. A 16 TB volume is an ordinary external disk. Darwin's
//! native `statfs` has 64-bit counts and no such ceiling, so macOS uses
//! that; Linux's `statvfs` counts are already 64-bit and it uses that.
//!
//! The multiplier differs with the call, which is the other reason they
//! are not interchangeable: POSIX defines `statvfs`'s counts in units of
//! `f_frsize`, while Darwin's `statfs` counts are in units of `f_bsize`.

#![allow(unsafe_code)]

use std::path::Path;

/// Bytes an unprivileged process can still write to the filesystem
/// holding `path`.
///
/// `None` when the filesystem cannot answer -- the path does not exist,
/// permission is denied, the path contains a NUL byte, or the filesystem
/// reports a zero block size. A missing measurement stays missing: no
/// caller may substitute zero, and none may read `None` as "plenty".
pub fn available_bytes(path: &Path) -> Option<u64> {
    let s = space(path)?;
    s.available_blocks.checked_mul(s.block_size)
}

/// Total bytes the filesystem holding `path` holds, for the "x of y"
/// shape a report uses when it has both. Same `None` discipline as
/// [`available_bytes`].
pub fn total_bytes(path: &Path) -> Option<u64> {
    let s = space(path)?;
    s.total_blocks.checked_mul(s.block_size)
}

/// The three numbers both backends produce, already widened to `u64`.
struct Space {
    available_blocks: u64,
    total_blocks: u64,
    block_size: u64,
}

fn cpath(path: &Path) -> Option<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).ok()
}

/// Darwin: `statfs`, whose block counts are 64-bit and whose unit is
/// `f_bsize`.
#[cfg(target_os = "macos")]
fn space(path: &Path) -> Option<Space> {
    let cpath = cpath(path)?;
    let mut buf: std::mem::MaybeUninit<libc::statfs> = std::mem::MaybeUninit::uninit();
    // SAFETY: `cpath` is a NUL-terminated C string alive for the call and
    // `buf` is a correctly sized, writable `statfs`. The return value is
    // checked before `assume_init`.
    let stat = unsafe {
        if libc::statfs(cpath.as_ptr(), buf.as_mut_ptr()) != 0 {
            return None;
        }
        buf.assume_init()
    };
    let block_size = u64::from(stat.f_bsize);
    if block_size == 0 {
        return None;
    }
    Some(Space {
        available_blocks: stat.f_bavail,
        total_blocks: stat.f_blocks,
        block_size,
    })
}

/// Linux: POSIX `statvfs`, whose counts are 64-bit and whose unit is
/// `f_frsize` -- falling back to `f_bsize` for a filesystem that reports
/// `f_frsize` as zero.
#[cfg(target_os = "linux")]
fn space(path: &Path) -> Option<Space> {
    let cpath = cpath(path)?;
    let mut buf: std::mem::MaybeUninit<libc::statvfs> = std::mem::MaybeUninit::uninit();
    // SAFETY: as above, with a `statvfs`.
    let stat = unsafe {
        if libc::statvfs(cpath.as_ptr(), buf.as_mut_ptr()) != 0 {
            return None;
        }
        buf.assume_init()
    };
    let block_size = if stat.f_frsize > 0 {
        stat.f_frsize
    } else {
        stat.f_bsize
    };
    if block_size == 0 {
        return None;
    }
    Some(Space {
        available_blocks: stat.f_bavail,
        total_blocks: stat.f_blocks,
        block_size,
    })
}

/// The identity of the filesystem a path is on, as a stamp: the device
/// number (unique per mount), whether it is mounted read-only, its
/// total size, and the root directory's inode and mtime. A read-only
/// filesystem whose stamp has not moved holds exactly the bytes it held
/// last pass -- a sealed simulator runtime volume, a mounted disk image
/// -- so a tree under it can be reused without a walk and without an
/// event window (R19; `.oh/guardrails/no-second-traversal-on-report-path.md`).
/// Deliberately not the used/available block count: on APFS a volume's
/// free space is the shared container's, which moves every second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeStamp {
    pub device: u64,
    pub read_only: bool,
    pub total_blocks: u64,
    pub root_ino: u64,
    pub root_mtime: i64,
}

pub fn volume_stamp(path: &Path) -> Option<VolumeStamp> {
    #[cfg(any(test, feature = "testing"))]
    if let Some(stamp) = testing::override_for(path) {
        return Some(stamp);
    }
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    let (space, read_only) = space_and_flag(path)?;
    Some(VolumeStamp {
        device: meta.dev(),
        read_only,
        total_blocks: space.total_blocks,
        root_ino: meta.ino(),
        root_mtime: meta.mtime(),
    })
}

/// `MNT_RDONLY` from `<sys/mount.h>`.
#[cfg(target_os = "macos")]
const MNT_RDONLY: u32 = 0x0000_0001;

#[cfg(target_os = "macos")]
fn space_and_flag(path: &Path) -> Option<(Space, bool)> {
    let cpath = cpath(path)?;
    let mut buf: std::mem::MaybeUninit<libc::statfs> = std::mem::MaybeUninit::uninit();
    // SAFETY: as in `space`.
    let stat = unsafe {
        if libc::statfs(cpath.as_ptr(), buf.as_mut_ptr()) != 0 {
            return None;
        }
        buf.assume_init()
    };
    let block_size = u64::from(stat.f_bsize);
    if block_size == 0 {
        return None;
    }
    Some((
        Space {
            available_blocks: stat.f_bavail,
            total_blocks: stat.f_blocks,
            block_size,
        },
        stat.f_flags & MNT_RDONLY != 0,
    ))
}

#[cfg(target_os = "linux")]
fn space_and_flag(path: &Path) -> Option<(Space, bool)> {
    let cpath = cpath(path)?;
    let mut buf: std::mem::MaybeUninit<libc::statvfs> = std::mem::MaybeUninit::uninit();
    // SAFETY: as in `space`.
    let stat = unsafe {
        if libc::statvfs(cpath.as_ptr(), buf.as_mut_ptr()) != 0 {
            return None;
        }
        buf.assume_init()
    };
    let block_size = if stat.f_frsize > 0 {
        stat.f_frsize
    } else {
        stat.f_bsize
    };
    if block_size == 0 {
        return None;
    }
    Some((
        Space {
            available_blocks: stat.f_bavail,
            total_blocks: stat.f_blocks,
            block_size,
        },
        stat.f_flag & libc::ST_RDONLY != 0,
    ))
}

/// Test seam: a stamp to answer for a path instead of asking the kernel,
/// so a sealed read-only mount can be staged in a temp dir.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use super::VolumeStamp;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static OVERRIDES: Mutex<Option<HashMap<PathBuf, VolumeStamp>>> = Mutex::new(None);

    pub fn override_for(path: &Path) -> Option<VolumeStamp> {
        OVERRIDES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()?
            .get(path)
            .copied()
    }

    /// Answers `stamp` for `path` until [`clear`].
    pub fn set(path: &Path, stamp: VolumeStamp) {
        OVERRIDES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(path.to_path_buf(), stamp);
    }

    pub fn clear() {
        *OVERRIDES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_directory_answers_with_a_plausible_figure() {
        let tmp = tempfile::tempdir().unwrap();
        let available = available_bytes(tmp.path()).expect("statvfs on a temp dir");
        let total = total_bytes(tmp.path()).expect("statvfs on a temp dir");
        assert!(available > 0, "a writable temp dir has some space");
        assert!(
            available <= total,
            "available {available} cannot exceed total {total}"
        );
        // Sanity against an absurd unit mistake: no developer machine has
        // a filesystem smaller than a megabyte or larger than an exabyte.
        assert!(
            (1 << 20..1 << 60).contains(&total),
            "implausible total {total}"
        );
    }

    #[test]
    fn a_missing_path_is_none_not_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(available_bytes(&tmp.path().join("does-not-exist")), None);
    }

    #[test]
    fn a_path_with_an_interior_nul_is_none_not_a_panic() {
        use std::os::unix::ffi::OsStrExt;
        let path = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/a\0b"));
        assert_eq!(available_bytes(&path), None);
    }

    /// The reason this module exists: the old implementation read
    /// `df -k`'s fourth whitespace-separated field, which is the
    /// available figure only on macOS's layout and only when the row does
    /// not wrap. The kernel answer must not depend on any of that.
    #[test]
    fn the_answer_does_not_depend_on_df_environment_variables() {
        let tmp = tempfile::tempdir().unwrap();
        let before = available_bytes(tmp.path()).unwrap();
        // SAFETY: single-threaded test-local mutation, restored below.
        unsafe {
            std::env::set_var("DF_BLOCK_SIZE", "1");
            std::env::set_var("POSIXLY_CORRECT", "1");
        }
        let after = available_bytes(tmp.path()).unwrap();
        unsafe {
            std::env::remove_var("DF_BLOCK_SIZE");
            std::env::remove_var("POSIXLY_CORRECT");
        }
        // Space genuinely moves between the two calls on a busy machine;
        // the unit must not. A block-size mistake changes the answer by
        // a factor of at least 512.
        let ratio = before.max(after) as f64 / before.min(after).max(1) as f64;
        assert!(ratio < 2.0, "before {before}, after {after}");
    }
}
