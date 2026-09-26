//! target: crates/core/src/external.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (U group, reviewer_counterexamples_stack4_sweep.rs), U7
//! by: audit:gate_paths_only_inside_gates, audit:no_unreferenced_public_items
//! why: U7: the only code that fills a delivered field lives in a file no `mod` declares
//! blind-spot (old model): the model is every `.rs` file under `src/`, not the module tree the compiler builds; an orphan file is never compiled and still counts as a writer, a caller or a registration
/// Sweep 4 U7: promised, never computed.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Sweep4OrphanRow {
    pub path: PathBuf,
    pub decision_facts: Vec<crate::evidence::FactKind>,
}

pub fn sweep4_orphan_row(path: PathBuf) -> Sweep4OrphanRow {
    Sweep4OrphanRow {
        path,
        decision_facts: Vec::new(),
    }
}
//! file: crates/core/src/sweep4_never_compiled.rs
//! mode: create
// No `mod sweep4_never_compiled;` anywhere: this file is not part of the crate.
pub fn sweep4_fill(r: &mut crate::external::Sweep4OrphanRow) {
    r.decision_facts.push(crate::evidence::FactKind::Activity);
}
