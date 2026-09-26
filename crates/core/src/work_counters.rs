//! Counters for the work an observation actually did: directories
//! listed, files statted, session-header bytes read.
//!
//! These exist because "incremental" is otherwise unfalsifiable. The
//! handoff requires unchanged work to scale with roots and changed
//! containers rather than with all files; the only way to *test* that is
//! to count the syscalls an unchanged pass makes and assert it is zero.
//! `crates/core/tests/incremental_external_and_agent_measurement.rs`
//! does exactly that.
//!
//! # Why there are two sinks
//!
//! The counters began as process-global `AtomicU64`s, which made every
//! assertion in this crate's own tests a race: `cargo test` runs lib
//! tests in parallel threads, so one adapter's `header_bytes_read == 0`
//! could be falsified by a different adapter's fixture reading a header
//! at the same moment. They were then made `thread_local!`, which fixed
//! the race and broke the instrument: `walk.rs` does its listing and
//! stat'ing on a worker pool (`Pool::drain` spawns `std::thread::scope`
//! workers), so the measuring thread saw none of the traversal it was
//! measuring. The 2026-09-22 re-review measured a 20,000-file traversal
//! reporting "2 dirs listed, 0 files statted" and called it a P1 on the
//! audit itself: "any future incrementality claim measured with this
//! instrument will pass vacuously".
//!
//! Neither scope alone is right, so there are two, and every record
//! writes to both:
//!
//! * a **process-global** sink, read by [`snapshot`]/[`since`]/[`reset`].
//!   It sees every thread, which is what an integration test measuring a
//!   whole observation wants. Such a test serializes itself (its own
//!   mutex, or `--test-threads=1`).
//! * a **scoped** sink installed by [`measured`] on the calling thread
//!   and *inherited by the pool workers that thread spawns*
//!   ([`current`]/[`install`], called from `Pool::drain`). Two tests
//!   calling `measured` at the same time cannot see each other's work,
//!   and each still sees its own pool.
//!
//! Relaxed ordering throughout: these are counters, not synchronization.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// One set of counters. The process-global sink is one of these; each
/// [`measured`] scope allocates another.
#[derive(Debug, Default)]
pub struct Counters {
    dirs_listed: AtomicU64,
    files_statted: AtomicU64,
    header_bytes: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    containers_reused: AtomicU64,
    containers_identified: AtomicU64,
    spawns: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> WorkCounters {
        WorkCounters {
            dirs_listed: self.dirs_listed.load(Ordering::Relaxed),
            files_statted: self.files_statted.load(Ordering::Relaxed),
            header_bytes_read: self.header_bytes.load(Ordering::Relaxed),
            identification_cache_hits: self.cache_hits.load(Ordering::Relaxed),
            identification_cache_misses: self.cache_misses.load(Ordering::Relaxed),
            containers_reused: self.containers_reused.load(Ordering::Relaxed),
            containers_identified: self.containers_identified.load(Ordering::Relaxed),
            subprocess_spawns: self.spawns.load(Ordering::Relaxed),
        }
    }
}

static GLOBAL: Counters = Counters {
    dirs_listed: AtomicU64::new(0),
    files_statted: AtomicU64::new(0),
    header_bytes: AtomicU64::new(0),
    cache_hits: AtomicU64::new(0),
    cache_misses: AtomicU64::new(0),
    containers_reused: AtomicU64::new(0),
    containers_identified: AtomicU64::new(0),
    spawns: AtomicU64::new(0),
};

thread_local! {
    static SCOPE: std::cell::RefCell<Option<Arc<Counters>>> =
        const { std::cell::RefCell::new(None) };
}

/// The scoped sink this thread is recording into, if any. `walk.rs`'s
/// pool captures this on the spawning thread and [`install`]s it on each
/// worker, so a [`measured`] scope sees the traversal it started.
pub fn current() -> Option<Arc<Counters>> {
    SCOPE.with(|s| s.borrow().clone())
}

/// Installs `scope` as this thread's scoped sink. Called by pool workers
/// with the value [`current`] returned on the thread that spawned them.
pub fn install(scope: Option<Arc<Counters>>) {
    SCOPE.with(|s| *s.borrow_mut() = scope);
}

fn add(pick: fn(&Counters) -> &AtomicU64, n: u64) {
    pick(&GLOBAL).fetch_add(n, Ordering::Relaxed);
    SCOPE.with(|s| {
        if let Some(c) = s.borrow().as_ref() {
            pick(c).fetch_add(n, Ordering::Relaxed);
        }
    });
}

/// A snapshot of the work counters, for a test or a `--json` work block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WorkCounters {
    pub dirs_listed: u64,
    pub files_statted: u64,
    pub header_bytes_read: u64,
    pub identification_cache_hits: u64,
    pub identification_cache_misses: u64,
    /// Container directories replayed from the store without a listing
    /// (`crate::agents::ContainerCache`). Counted separately from the
    /// per-file derivation cache above so neither number can be read as
    /// the other: a pass that reuses every container makes *no* per-file
    /// derivation at all, so its `identification_cache_hits` is zero and
    /// that is the right answer, not a regression.
    pub containers_reused: u64,
    /// Container directories identified file by file this pass.
    pub containers_identified: u64,
    /// Child processes this crate spawned.
    ///
    /// The 2026-09-21 review's CE6 -- a disabled `docker-desktop`
    /// detector still asking the daemon to enumerate the user's images,
    /// five spawns per observation, forever -- was caught with
    /// process-wide `PATH` shims, which cannot run under the default
    /// test harness: one test's shimmed `PATH` and shared counter file
    /// are every concurrent test's too. Counting the spawn where it
    /// happens makes the same assertion, through the scoped sink
    /// [`measured`] installs, with no process-wide state at all.
    pub subprocess_spawns: u64,
}

pub fn record_dir_listed() {
    add(|c| &c.dirs_listed, 1);
}

pub fn record_files_statted(n: u64) {
    add(|c| &c.files_statted, n);
}

pub fn record_header_bytes(n: u64) {
    add(|c| &c.header_bytes, n);
}

pub fn record_cache_hit() {
    add(|c| &c.cache_hits, 1);
}

pub fn record_cache_miss() {
    add(|c| &c.cache_misses, 1);
}

pub fn record_container_reused() {
    add(|c| &c.containers_reused, 1);
}

pub fn record_container_identified() {
    add(|c| &c.containers_identified, 1);
}

/// One child process, recorded at the call that spawns it. Every
/// `std::process::Command` in this crate goes through a site that calls
/// this, so "this observation spawned nothing" is a measurement rather
/// than an inference from a `PATH` shim.
pub fn record_spawn() {
    add(|c| &c.spawns, 1);
}

/// The process-global counters. Sees every thread; a caller that wants
/// an exact number either serializes itself or uses [`measured`].
pub fn snapshot() -> WorkCounters {
    GLOBAL.snapshot()
}

/// Runs `f` with a fresh scoped sink installed on this thread, returning
/// its value and exactly the work `f` did -- including the work of any
/// `walk.rs` pool `f` started, and excluding every other thread's.
pub fn measured<T>(f: impl FnOnce() -> T) -> (T, WorkCounters) {
    let previous = current();
    let scope = Arc::new(Counters::default());
    install(Some(Arc::clone(&scope)));
    let out = f();
    install(previous);
    (out, scope.snapshot())
}

/// Zeroes the process-global counters. Tests that measure through
/// [`snapshot`] call this immediately before the pass they are
/// measuring; nothing in production resets them.
pub fn reset() {
    for c in [
        &GLOBAL.dirs_listed,
        &GLOBAL.files_statted,
        &GLOBAL.header_bytes,
        &GLOBAL.cache_hits,
        &GLOBAL.cache_misses,
        &GLOBAL.containers_reused,
        &GLOBAL.containers_identified,
        &GLOBAL.spawns,
    ] {
        c.store(0, Ordering::Relaxed);
    }
}

/// The global work done between `before` and now.
pub fn since(before: WorkCounters) -> WorkCounters {
    let now = snapshot();
    WorkCounters {
        dirs_listed: now.dirs_listed.saturating_sub(before.dirs_listed),
        files_statted: now.files_statted.saturating_sub(before.files_statted),
        header_bytes_read: now
            .header_bytes_read
            .saturating_sub(before.header_bytes_read),
        identification_cache_hits: now
            .identification_cache_hits
            .saturating_sub(before.identification_cache_hits),
        identification_cache_misses: now
            .identification_cache_misses
            .saturating_sub(before.identification_cache_misses),
        containers_reused: now
            .containers_reused
            .saturating_sub(before.containers_reused),
        containers_identified: now
            .containers_identified
            .saturating_sub(before.containers_identified),
        subprocess_spawns: now
            .subprocess_spawns
            .saturating_sub(before.subprocess_spawns),
    }
}

#[cfg(test)]
mod tests {
    /// The scoped sink is what makes a `== 0` assertion in a parallel
    /// test suite mean anything: work another thread does outside this
    /// scope must not land in it.
    #[test]
    fn a_measured_scope_does_not_see_another_thread() {
        let (_, counted) = super::measured(|| {
            std::thread::spawn(super::record_dir_listed).join().unwrap();
            super::record_dir_listed();
        });
        assert_eq!(counted.dirs_listed, 1);
    }
}
