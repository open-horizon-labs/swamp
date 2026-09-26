//! target: crates/core/src/agents/opencode.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M34
//! by: audit:gate_paths_only_inside_gates, compile:unsafe_code
//! why: an adapter truncating a tool's session index with `libc::truncate`
//! blind-spot (old model): destruction is `std::fs::*`, `File::create*`, a writing `OpenOptions`, `trash::`, `persist` or a spawn; `libc` is a dependency of core and `truncate(2)`/`unlink(2)`/`rename(2)` are on no list (the `File::options()` spelling *was* rejected -- but only because every `.open(x)` is "possibly" `Store::open`/`Ledger::open`, which create directories: rejected for the wrong reason)
/// Sweep 4: identification truncates the user's session index.
pub fn sweep4_reset_index(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::truncate(c.as_ptr(), 0) == 0 }
}
