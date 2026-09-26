//! target: crates/core/src/activity.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M42
//! by: compile:E0277
//! why: a dead public evidence function kept "live" by a `Drop` impl of a type nothing constructs
//! blind-spot (old model): every trait method is an entry point ("dispatched by the bus, serde or the runtime"), so any trait impl -- `Drop`, `Default`, `Display` -- makes whatever it calls live
/// Sweep 4: a capability the docs can claim and nothing reaches.
pub fn sweep4_dead_fact(observed_at: u64) -> Evidence {
    Evidence::unknown(
        FactKind::Activity,
        FactSubtype::Modified,
        EvidenceSource::FilesystemMetadata {
            detail: String::new(),
        },
        observed_at,
        "never computed",
    )
}

/// Sweep 4: never constructed; its `drop` is the "caller".
pub struct Sweep4Never;

impl Drop for Sweep4Never {
    fn drop(&mut self) {
        let _ = sweep4_dead_fact(0);
    }
}
