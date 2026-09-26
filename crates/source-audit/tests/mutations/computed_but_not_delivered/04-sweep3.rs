//! target: crates/core/src/external.rs
//! mode: append
//! by: audit:no_unreferenced_public_items
//! why: re-review 3 sweep -- an evidence field on a delivered surface, typed so the word `Evidence` never appears (blind spot: the rule is scoped to fields whose declared type *mentions* `Evidence`; the same promise typed `Vec<Fact>` is unaudited)
/// Sweep: promised in the docs, never computed, never rendered.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SweepNestedRow {
    pub path: std::path::PathBuf,
    pub decision_facts: Vec<crate::evidence::FactKind>,
}

pub fn sweep_nested_row(path: std::path::PathBuf) -> SweepNestedRow {
    SweepNestedRow {
        path,
        decision_facts: Vec::new(),
    }
}
