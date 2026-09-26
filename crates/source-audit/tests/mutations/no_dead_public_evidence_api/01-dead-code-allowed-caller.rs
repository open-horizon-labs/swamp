//! target: crates/core/src/activity.rs
//! by: audit:no_unreferenced_public_items
//! why: sweep slip -- an `#[allow(dead_code)]` caller counted as a caller for a dead public API
pub fn sweep_unreachable_evidence(observed_at: u64) -> u64 {
    observed_at
}

#[allow(dead_code)]
fn sweep_fake_caller() {
    let _ = sweep_unreachable_evidence(0);
}
