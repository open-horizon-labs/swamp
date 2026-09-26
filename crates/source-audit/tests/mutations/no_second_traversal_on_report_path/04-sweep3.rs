//! target: crates/core/src/external.rs
//! mode: append
//! by: compile:E0061
//! why: re-review 3 sweep -- external.rs re-walking every root through the walker's own entry point (blind spot: `TRAVERSAL_CALLS` is a five-needle list (`read_dir`, `walkdir`, `jwalk`, `resize_artifact`); `walk::discover_and_attribute` is a full traversal and is not on it)
/// Sweep: a full re-walk from a file that must never traverse.
pub fn sweep_remeasure(root: &std::path::Path) -> u64 {
    let _ = crate::walk::discover_and_attribute(root, 0, 0, &[]);
    0
}
