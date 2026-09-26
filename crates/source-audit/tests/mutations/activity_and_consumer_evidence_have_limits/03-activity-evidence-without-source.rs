//! target: crates/core/src/activity.rs
//! by: compile:E0599
//! note: 2026-09-22 -- `Evidence` has no `Default`: a fact is built only with its source and time
//! why: an Activity fact with no EvidenceSource -- "when was this last used" with nothing saying how we know
pub fn sweep_last_used_evidence(at: u64) -> crate::evidence::Evidence {
    let mut e = crate::evidence::Evidence::default();
    e.observed_at = at;
    e
}
