//! target: crates/core/src/external.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M26
//! by: compile:clippy::disallowed_methods
//! why: external.rs re-walking a root recursively through `Path::read_dir` (UFCS)
//! blind-spot (old model): `traversal_call` is `fs::read_dir`, walkdir/jwalk, or a *method* named `read_dir`; the associated-function spelling `Path::read_dir(dir)` is none of them
/// Sweep 4: a second traversal, spelled as an associated function.
pub fn sweep4_remeasure(dir: &Path) -> u64 {
    let mut n = 0;
    if let Ok(rd) = Path::read_dir(dir) {
        for e in rd.flatten() {
            n += 1;
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                n += sweep4_remeasure(&e.path());
            }
        }
    }
    n
}
