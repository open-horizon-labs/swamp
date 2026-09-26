//! target: crates/core/src/consumers/history.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; it registers on its own stand-in type. The real `EventBus::register` is private (compile_fail `bus_register_is_private`).
//! why: the same runtime registration written in a different consumer file
pub struct SweepHistoryLate;

impl SweepHistoryLate {
    pub fn on_event(&self) {
        let mut registry = SweepRegistry;
        registry.register(());
    }
}

pub struct SweepRegistry;
impl SweepRegistry {
    pub fn register(&mut self, _c: ()) {}
}
