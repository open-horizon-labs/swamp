//! target: crates/core/src/external.rs
//! mode: append
//! by: compile:E0616
//! ported: 2026-09-22 -- the original stood in a local `SweepSummary` for the real type; the same renamed-binding read against the real `scope::DetectorSummary`, whose raw candidates are now private to `scope`
//! why: re-review 3 sweep -- discovery reading raw detector output through a renamed local binding (blind spot: the forbidden shapes are token needles that pin the receiver name (`summary . locations`, `scope . detectors`); one `let s = summary;` defeats all three)
/// Sweep: raw detector candidates, one rename away from the needle.
fn sweep_raw_candidates(summary: &crate::scope::DetectorSummary) -> usize {
    let s = summary;
    s.locations.len()
}
