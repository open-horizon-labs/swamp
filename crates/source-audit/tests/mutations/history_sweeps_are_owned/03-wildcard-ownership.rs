//! target: crates/core/src/report.rs
//! by: audit:gate_paths_only_inside_gates
//! why: an ownership window constructed to cover everything is no window at all
pub fn sweep_wildcard_ownership() -> crate::growth::ObservationOwnership {
    crate::growth::ObservationOwnership::new(
        crate::growth::KeyFamily::External,
        vec![std::path::PathBuf::from("/")],
    )
}
