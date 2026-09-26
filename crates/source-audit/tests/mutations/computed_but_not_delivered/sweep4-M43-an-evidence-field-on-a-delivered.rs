//! target: crates/core/src/external.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M43
//! by: audit:no_unreferenced_public_items
//! why: an evidence field on a delivered surface that is only ever empty; the one "mutation" is a `retain`
//! blind-spot (old model): a field counts as computed if anything calls `push|extend|insert|..|retain|sort|iter_mut` on it; filtering or sorting an empty Vec computes nothing
/// Sweep 4: promised, never computed, "mutated" by a filter.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Sweep4NestedRow {
    pub path: PathBuf,
    pub decision_facts: Vec<crate::evidence::FactKind>,
}

pub fn sweep4_nested_row(path: PathBuf) -> Sweep4NestedRow {
    let mut row = Sweep4NestedRow {
        path,
        decision_facts: Vec::new(),
    };
    row.decision_facts.retain(|_| true);
    row
}
