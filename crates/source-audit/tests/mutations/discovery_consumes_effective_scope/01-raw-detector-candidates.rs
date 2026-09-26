//! target: crates/core/src/external.rs
//! by: compile:E0616
//! ported: 2026-09-22 -- `EffectiveScope::detectors` is a list of `DetectorSummary`; its raw candidate locations are the private field
//! why: the review's excluded_agent_home_must_not_be_scanned counterexample -- discovery reading raw detector output instead of the authorized scope
fn sweep_candidates_from_detectors(scope: &crate::scope::EffectiveScope) -> usize {
    scope.detectors.iter().map(|d| d.locations.len()).sum()
}
