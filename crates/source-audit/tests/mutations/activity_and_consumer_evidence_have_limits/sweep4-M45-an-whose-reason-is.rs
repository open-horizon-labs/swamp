//! target: crates/core/src/activity.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M45
//! by: compile:E0277
//! why: an `Evidence::unknown` whose reason is `String::from("")`
//! blind-spot (old model): `empty_text` is a fixed list of spellings (`""`, `String::new()`, `"".into()`, ...) plus constants; `String::from("")`, `" "` and `format!("")` are not on it
/// Sweep 4: "not observed", with nothing saying why.
pub fn sweep4_unknown_fact(observed_at: u64) -> Evidence {
    Evidence::unknown(
        FactKind::Activity,
        FactSubtype::Modified,
        EvidenceSource::FilesystemMetadata {
            detail: String::new(),
        },
        observed_at,
        String::from(""),
    )
}
