//! target: crates/core/src/bus/mod.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M18
//! by: compile:E0624
//! why: a second static registrar that registers a consumer again with `Box::from`
//! blind-spot (old model): the registration count is the number of `Box :: new (` texts naming the consumer; `Box::from(X)` registers without being counted
impl EventBus {
    /// Sweep 4: every builtin, plus the projects consumer a second time.
    pub fn sweep4_with_builtins_and_more() -> Self {
        let mut bus = Self::with_builtins();
        let again: Box<crate::consumers::ProjectsConsumer> =
            Box::from(crate::consumers::ProjectsConsumer);
        let _ = bus.register(again);
        bus
    }
}
