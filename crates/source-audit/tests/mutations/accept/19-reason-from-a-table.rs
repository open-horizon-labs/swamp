//! target: crates/core/src/occupancy.rs
//! expect: accept
//! source: accept
//! why: a reason held in a static table, through `Reason::fixed`
const SWEEP_ACCEPT_WHY: &[&str] = &["the probe timed out", "lsof is not installed"];

/// Accept: a table reason, checked at run time.
fn sweep_accept_table_reason(i: usize) -> crate::evidence::Reason {
    crate::evidence::Reason::fixed(SWEEP_ACCEPT_WHY[i % SWEEP_ACCEPT_WHY.len()])
}
