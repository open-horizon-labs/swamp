//! target: crates/core/src/agents/windsurf.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M38
//! by: audit:adapters_do_not_reach_gates
//! why: an adapter naming a detector id through a glob import of `crate::locations::*`
//! blind-spot (old model): detector identity is a path with a `locations` segment; after `use crate::locations::*;` the path is `windsurf::WINDSURF_DETECTOR_ID`, and glob imports are recorded but never resolved
/// Sweep 4: detector identity, one glob away.
pub mod sweep4_ids {
    use crate::locations::*;
    pub fn sweep4_detector_id() -> &'static str {
        windsurf::WINDSURF_DETECTOR_ID
    }
}
