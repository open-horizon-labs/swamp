//! target: crates/core/src/actions.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M22
//! by: audit:sinks_have_no_path_predicates
//! why: a one-directional protection predicate in actions.rs, written with `strip_prefix`
//! blind-spot (old model): a second containment predicate is recognised only as a `.starts_with(<non-literal>)` method call; `strip_prefix(k).is_ok()` is the same one-way test
/// Sweep 4: the deleted one-way predicate, spelled another way.
pub fn sweep4_kept(candidate: &Path, kept: &[PathBuf]) -> bool {
    kept.iter().any(|k| candidate.strip_prefix(k).is_ok())
}
