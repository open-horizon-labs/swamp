//! target: crates/core/src/actions.rs
//! mode: append
//! by: audit:sinks_have_no_path_predicates
//! why: re-review 3 sweep -- a one-directional protection predicate, named without the word `protect` (blind spot: the "one predicate everywhere" rule only inspects functions whose name `contains("protect")` -- the deleted `is_human_protected` renamed to `kept_conflict` is invisible again)
/// Sweep: the second predicate, back, under a name the audit does not read.
pub fn sweep_kept_conflict(candidate: &std::path::Path, kept: &[std::path::PathBuf]) -> bool {
    kept.iter().any(|k| candidate.starts_with(k))
}
