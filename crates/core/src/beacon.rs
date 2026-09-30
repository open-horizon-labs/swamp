//! Progress beacon for a running `observe` pass (#190).
//!
//! A pass once sat for 15+ minutes at 0% CPU with the writer lock held
//! and nothing said where. Every unit of work that touches a user path
//! (a walk directory, a repository's git signals, its ignore lens, an
//! agent session store, an external unit) now [`enter`]s the beacon with
//! a phase and a path, and long loops [`beat`]. The `observe` watchdog
//! reads [`idle_for`] (time since anything progressed) and [`stuck`] (the
//! step that has been running longest) to stop a pass that has stopped
//! moving, and to name where.
//!
//! Cost on the walk's hot path: each thread owns one slot, so entering
//! and leaving takes only that thread's own, uncontended lock; progress
//! is one relaxed atomic store.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Slots for concurrently tracked threads; a thread past this many shares
/// the last slot's progress but is not individually named.
const SLOTS: usize = 256;

type Slot = Mutex<Option<(&'static str, PathBuf, Instant)>>;

static TABLE: [Slot; SLOTS] = [const { Mutex::new(None) }; SLOTS];
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);
/// Milliseconds since [`epoch`] at the last progress anywhere.
static LAST: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static E: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *E.get_or_init(Instant::now)
}

/// Slots released by threads that exited (walk pools spawn fresh scoped
/// threads per pass, so slots must be recycled).
static FREE: Mutex<Vec<usize>> = Mutex::new(Vec::new());

struct SlotId(usize);

impl SlotId {
    fn claim() -> Self {
        let reused = FREE.lock().unwrap_or_else(|e| e.into_inner()).pop();
        Self(reused.unwrap_or_else(|| NEXT_SLOT.fetch_add(1, Ordering::Relaxed).min(SLOTS - 1)))
    }
}

impl Drop for SlotId {
    fn drop(&mut self) {
        if self.0 < SLOTS - 1 {
            *TABLE[self.0].lock().unwrap_or_else(|e| e.into_inner()) = None;
            FREE.lock().unwrap_or_else(|e| e.into_inner()).push(self.0);
        }
    }
}

thread_local! {
    static MINE: SlotId = SlotId::claim();
}

/// Records progress now.
pub fn beat() {
    LAST.store(epoch().elapsed().as_millis() as u64, Ordering::Relaxed);
}

/// How long since anything progressed.
pub fn idle_for() -> Duration {
    let last = Duration::from_millis(LAST.load(Ordering::Relaxed));
    epoch().elapsed().saturating_sub(last)
}

/// Held while one step runs on this thread; dropping it leaves the slot
/// (restoring an enclosing step, if any) and counts as progress.
pub struct Entered {
    slot: usize,
    outer: Option<(&'static str, PathBuf, Instant)>,
}

/// This thread starts `phase` on `path`.
pub fn enter(phase: &'static str, path: &Path) -> Entered {
    beat();
    let slot = MINE.with(|s| s.0);
    let mut s = TABLE[slot].lock().unwrap_or_else(|e| e.into_inner());
    let outer = s.replace((phase, path.to_path_buf(), Instant::now()));
    Entered { slot, outer }
}

impl Drop for Entered {
    fn drop(&mut self) {
        *TABLE[self.slot].lock().unwrap_or_else(|e| e.into_inner()) = self.outer.take();
        beat();
    }
}

/// Test hook for the end-to-end watchdog test: under `SWAMP_TEST_MODE=1`,
/// a walk entering the directory named by the park variable parks there
/// without progressing, as a worker in a blocking `open(2)` did in #190.
///
/// Compiled only with swamp-core's `testing` feature, which only
/// `[dev-dependencies]` enable (resolver 2 never unifies it into a
/// shipped build; `scripts/check.sh` proves that for the release graph and
/// `scripts/release-smoke.sh` checks the packaged binary for the variable
/// name). A release binary has no hook at all.
#[cfg(feature = "testing")]
pub(crate) fn test_park(path: &Path) {
    static PARK: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    let park = PARK.get_or_init(|| {
        if std::env::var("SWAMP_TEST_MODE").is_ok_and(|v| v == "1") {
            std::env::var_os("SWAMP_TEST_PARK_DIR").map(PathBuf::from)
        } else {
            None
        }
    });
    if park.as_deref() == Some(path) {
        loop {
            std::thread::park();
        }
    }
}

/// Release builds: no hook.
#[cfg(not(feature = "testing"))]
#[inline(always)]
pub(crate) fn test_park(_path: &Path) {}

/// The innermost step still running: of the steps in flight, the one
/// entered last. At a stall every running step is stuck, and the latest
/// one is the deepest (an external unit waits on the walk workers inside
/// it; the walk directory a worker parked in names the path).
pub fn stuck() -> Option<(&'static str, PathBuf, Duration)> {
    let used = NEXT_SLOT.load(Ordering::Relaxed).min(SLOTS);
    TABLE[..used]
        .iter()
        .filter_map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone())
        .max_by_key(|(_, _, at)| *at)
        .map(|(ph, p, at)| (ph, p, at.elapsed()))
}

/// Every step in flight, for tests that run beside other walks (the
/// innermost one [`stuck`] names may belong to another test's thread).
#[cfg(test)]
pub(crate) fn running() -> Vec<(&'static str, PathBuf)> {
    let used = NEXT_SLOT.load(Ordering::Relaxed).min(SLOTS);
    TABLE[..used]
        .iter()
        .filter_map(|s| s.lock().unwrap_or_else(|e| e.into_inner()).clone())
        .map(|(ph, p, _)| (ph, p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_named_while_it_runs_and_restores_the_outer_one() {
        let p = PathBuf::from("/beacon-test/outer");
        let q = PathBuf::from("/beacon-test/inner");
        std::thread::spawn(move || {
            let _o = enter("git signals", &p);
            {
                let _i = enter("ignore lens", &q);
                assert!(running().contains(&("ignore lens", q.clone())));
            }
            let mine = MINE.with(|s| s.0);
            let slot = TABLE[mine].lock().unwrap().clone();
            assert_eq!(
                slot.map(|(ph, path, _)| (ph, path)),
                Some(("git signals", p))
            );
        })
        .join()
        .unwrap();
        assert!(idle_for() < Duration::from_secs(5));
    }
}
