//! target: crates/core/src/agents/claude_code.rs
//! by: audit:adapters_do_not_reach_gates
//! why: an adapter deciding scope from LocationStatus::Resolved, so an excluded tool home is scanned anyway
pub fn sweep_resolved_homes(
    proposals: &[crate::locations::LocationProposal],
) -> Vec<std::path::PathBuf> {
    proposals
        .iter()
        .filter(|p| p.status == crate::locations::LocationStatus::Resolved)
        .filter_map(|p| p.path.clone())
        .collect()
}
