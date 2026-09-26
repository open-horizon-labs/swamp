//! target: crates/core/src/activity.rs
//! mode: append
//! by: compile:E0277
//! why: re-review 3 sweep -- an `Evidence::unknown` whose reason is an empty constant (blind spot: the empty-reason rule looks for the literal `""` in the call's arguments; a named constant holding the empty string is invisible)
const SWEEP_NO_REASON: &str = "";

/// Sweep: "not observed", with nothing saying why.
pub fn sweep_unknown_fact(observed_at: u64) -> Evidence {
    Evidence::unknown(
        FactKind::Activity,
        FactSubtype::Modified,
        EvidenceSource::FilesystemMetadata {
            detail: String::new(),
        },
        observed_at,
        SWEEP_NO_REASON,
    )
}
