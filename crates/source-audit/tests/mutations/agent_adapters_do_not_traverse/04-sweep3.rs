//! target: crates/core/src/agents/bounded_io.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- an unbounded recursive walk of a tool home, written in the shared module every adapter reads through (blind spot: `AGENT_NON_ADAPTERS` exempts bounded_io.rs -- the shared capped reader every adapter reads through -- so a recursive walk written there is not an adapter traversal)
/// Sweep: an unbounded recursive scan, in the shared module every
/// adapter's bounded reads go through.
pub fn sweep_walk_home(home: &std::path::Path, n: &mut usize) {
    let Ok(rd) = std::fs::read_dir(home) else { return };
    for e in rd.flatten() {
        *n += 1;
        if e.path().is_dir() {
            sweep_walk_home(&e.path(), n);
        }
    }
}
