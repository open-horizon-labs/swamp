//! target: crates/core/src/agents/codex.rs
//! by: compile:E0616
//! ported: 2026-09-22 -- the summary type is `scope::DetectorSummary`
//! why: the same bypass moved one adapter away, reading the scope summary's raw locations
fn sweep_homes_from_summary(summary: &crate::scope::DetectorSummary) -> usize {
    summary.locations.len()
}
