//! target: crates/core/src/agents/continue_dev.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M33
//! by: compile:clippy::disallowed_methods
//! why: an unbounded recursive walk of a tool home through `Path::read_dir` (UFCS)
//! blind-spot (old model): as for the report path: the associated-function spelling of `read_dir` is not a traversal primitive
/// Sweep 4: an adapter that walks the whole tool home.
pub fn sweep4_walk_home(home: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = Path::read_dir(home) {
        for e in rd.flatten() {
            n += 1;
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                n += sweep4_walk_home(&e.path());
            }
        }
    }
    n
}
