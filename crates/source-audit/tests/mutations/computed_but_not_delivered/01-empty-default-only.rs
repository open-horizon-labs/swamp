//! target: crates/core/src/external.rs
//! by: audit:no_unreferenced_public_items
//! why: the guardrail's own defect shape -- a pub evidence field on a delivered surface type that is only ever an empty default
pub struct SweepNestedSummary {
    pub path: std::path::PathBuf,
    pub evidence_sweep_nested: Vec<crate::evidence::Evidence>,
}

pub fn sweep_build_nested_summary(path: std::path::PathBuf) -> SweepNestedSummary {
    SweepNestedSummary {
        path,
        evidence_sweep_nested: Vec::new(),
    }
}
