//! The TUI's one way off the event thread
//! (`.oh/guardrails/tui-actions-off-event-thread.md`).
//!
//! Every observation, review and execution the TUI starts runs in a
//! closure handed to [`spawn`], which reports back over a channel the
//! event loop polls without blocking. There is no join handle: nothing
//! on the event thread can wait for a worker. `std::thread` is named
//! nowhere else in this crate -- `crates/tui/clippy.toml` disallows
//! `thread::spawn`, `Builder::spawn` and `JoinHandle::join`, and the
//! gate audit rejects the path outside this file. That is what makes
//! `thread::spawn(|| probe()).join()` on a key handler -- "off the event
//! thread", then waited for on it -- not compile.

#![allow(clippy::disallowed_methods)]

/// Runs `job` on a new worker thread and returns immediately.
pub fn spawn<F>(job: F)
where
    F: FnOnce() + Send + 'static,
{
    // Dropping the handle detaches the thread; the job reports through
    // whatever channel it captured.
    drop(std::thread::spawn(job));
}
