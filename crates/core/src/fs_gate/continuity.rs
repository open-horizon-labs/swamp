//! The two file families `crate::continuity`'s Linux collector owns:
//! its per-root checkpoint (`continuity/<root-id>.json`) and the
//! one-shot sync-token file an observation drops to ask it to drain its
//! queue. Every write is atomic (sibling temp file + rename); every
//! read returns the bytes or the error, never silently absent content.
//! Narrowly purposed rather than a general `write(path, bytes)` escape
//! hatch, the same discipline `fs_gate::store`'s typed writers hold for
//! swamp's other own files (`.oh/guardrails/json-persistence-is-allowlisted.md`).

use std::io;
use std::path::Path;

/// Writes `bytes` to `target` atomically (`tmp` renamed onto it),
/// creating `dir` first. The caller computes `tmp`'s name (a pure
/// `Path` operation, not I/O) so two processes racing to publish the
/// same `target` never share one.
pub fn write_atomic(dir: &Path, tmp: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(tmp, bytes)?;
    std::fs::rename(tmp, target)
}

/// Creates `dir` (and its parents) if it is missing -- the continuity
/// directory under the store, before the collector's checkpoint, alive
/// lock, dirty lock or sync file are first written into it.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Reads a whole small file swamp owns here (a checkpoint, a sync
/// token, `/proc/sys/kernel/random/boot_id`) as UTF-8.
pub fn read_text(path: impl AsRef<Path>) -> io::Result<String> {
    std::fs::read_to_string(path)
}

/// An `flock` held for as long as this value lives.
#[derive(Debug)]
pub struct FileLock {
    file: std::fs::File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Tries to take `path`'s lock without blocking. `Ok(None)` when
/// another process holds it.
pub fn try_lock(path: &Path, exclusive: bool) -> io::Result<Option<FileLock>> {
    let file = super::sys::open_for_lock(path)?;
    let result = if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    match result {
        Ok(()) => Ok(Some(FileLock { file })),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Whether a collector holds `alive_path`'s lock right now. Never
/// creates the lock file: asking must leave no state behind.
pub fn collector_alive(alive_path: &Path) -> bool {
    let Ok(Some(file)) = super::sys::open_for_lock_probe(alive_path) else {
        return false;
    };
    // A shared lock we can take means nobody holds the exclusive one;
    // released (best-effort) right after, since this call only asks.
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }
}

/// Makes SIGINT and SIGTERM stop the collector cleanly and returns the
/// flag they set. The signal handler itself lives in `fs_gate::sys`.
#[cfg(target_os = "linux")]
pub fn stop_on_signals() -> &'static std::sync::atomic::AtomicBool {
    super::sys::install_stop_signal_handlers()
}
