//! target: crates/core/src/consumer_wiring.rs
//! by: compile:E0277
//! ported: 2026-09-22 -- to the current five-argument `Evidence::unavailable`
//! why: the same defect written differently -- the constructor is called, with an empty reason
fn sweep_unavailable_with_no_reason(
    kind: crate::evidence::FactKind,
    subtype: crate::evidence::FactSubtype,
    source: crate::evidence::EvidenceSource,
) -> crate::evidence::Evidence {
    crate::evidence::Evidence::unavailable(kind, subtype, source, 0, "")
}
