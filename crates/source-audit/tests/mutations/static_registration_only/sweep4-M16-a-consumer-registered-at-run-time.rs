//! target: crates/core/src/report.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M16
//! by: compile:E0624
//! why: a consumer registered at run time on a bus reached through `Option::unwrap()`
//! blind-spot (old model): only an *exactly resolved* call to the registration sink counts; a receiver whose type the model cannot name (`bus.unwrap()`) makes the call "possibly" `register`, and possible calls are skipped
/// Sweep 4: late registration, through an untyped receiver.
pub fn sweep4_register_late(
    bus: Option<&mut crate::bus::EventBus>,
    c: Box<dyn crate::bus::Consumer>,
) -> anyhow::Result<()> {
    bus.unwrap().register(c)
}
