//! target: crates/core/src/agents/mod.rs
//! by: audit:no_unreferenced_public_items
//! why: alias/rename variant -- the field's declared type is renamed, so a "type mentions Evidence" needle would miss it
use crate::evidence::Evidence as SweepFact;

pub struct SweepAliasedEvidenceUnit {
    pub relative_path: String,
    pub evidence_sweep_aliased: Vec<SweepFact>,
}

pub fn sweep_build_aliased_unit(relative_path: String) -> SweepAliasedEvidenceUnit {
    SweepAliasedEvidenceUnit {
        relative_path,
        evidence_sweep_aliased: Vec::new(),
    }
}
