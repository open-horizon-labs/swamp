//! Test support: wait until data written so far is reflected in
//! `st_blocks` (#197).
//!
//! On a filesystem that reports allocation late -- the homelab fleet's
//! overlayfs over ZFS assigns `st_blocks` only when the transaction
//! group commits, measured at 1 to 5 seconds, and neither `fsync` nor
//! `sync` shortens it -- a fixture written a moment ago reads as a token
//! 512 bytes per file. A test that then asserts exact allocated bytes is
//! asserting the filesystem's timing, not swamp's accounting.
//!
//! [`settle`] writes one incompressible probe file, polls until *the
//! probe* reports its real allocation, and removes it. Commits are
//! ordered, so once the probe's allocation is visible every write made
//! before the call is too; nothing about the fixture (sparse files made
//! on purpose, empty files, symlinks) has to be classified. Where blocks
//! are assigned at write time (APFS, tmpfs, a settled ext4) the first
//! poll succeeds and the cost is one 64 KiB write.
//!
//! Compiled for tests only (`cfg(test)` or the `testing` feature, which
//! every workspace crate enables under `[dev-dependencies]`).

use std::io::Write;
use std::time::{Duration, Instant};

/// Probe size: large enough that the token allocation (one block) and
/// the real one (128 blocks) cannot be confused.
const PROBE_BYTES: usize = 64 * 1024;
/// Real allocation must reach at least this much of the probe.
const PROBE_MIN_ALLOCATED: u64 = 32 * 1024;
const POLL: Duration = Duration::from_millis(100);
const CAP: Duration = Duration::from_secs(15);

/// Blocks until everything written before this call reports its
/// allocation. Panics, never continues silently, when the filesystem
/// still reports the token allocation after the cap.
pub fn settle() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ std::process::id() as u64;
    let data: Vec<u8> = (0..PROBE_BYTES)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();
    let probe = tempfile::Builder::new()
        .prefix("swamp-settle-probe-")
        .tempfile()
        .expect("settle: create probe");
    probe
        .as_file()
        .write_all(&data)
        .expect("settle: write probe");
    probe.as_file().sync_all().expect("settle: sync probe");
    let start = Instant::now();
    loop {
        let blocks = std::os::unix::fs::MetadataExt::blocks(
            &std::fs::symlink_metadata(probe.path()).expect("settle: stat probe"),
        );
        if blocks * 512 >= PROBE_MIN_ALLOCATED {
            return;
        }
        assert!(
            start.elapsed() < CAP,
            "this filesystem did not report allocation within {}s: st_blocks lags \
             (probe of {PROBE_BYTES} bytes still reports {} bytes)",
            CAP.as_secs(),
            blocks * 512
        );
        std::thread::sleep(POLL);
    }
}
