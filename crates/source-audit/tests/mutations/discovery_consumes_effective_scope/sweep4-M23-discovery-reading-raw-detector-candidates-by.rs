//! target: crates/core/src/external.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M23
//! by: compile:E0451
//! why: discovery reading raw detector candidates by destructuring `DetectorSummary`
//! blind-spot (old model): raw detector output is recognised as a `.field` access; `let DetectorSummary { locations, .. } = s;` reads the same field with no field access
/// Sweep 4: raw candidates, destructured.
pub fn sweep4_raw_candidates(summary: &crate::scope::DetectorSummary) -> Vec<PathBuf> {
    let crate::scope::DetectorSummary { locations, .. } = summary;
    locations.iter().filter_map(|l| l.path.clone()).collect()
}
