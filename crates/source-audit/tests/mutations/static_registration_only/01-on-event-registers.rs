//! target: crates/core/src/consumers/tracking.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; it registers on its own stand-in type. The real `EventBus::register` is private (compile_fail `bus_register_is_private`).
//! why: sweep slip -- consumer registration happening at event time instead of statically
pub struct SweepLateConsumer;

impl SweepLateConsumer {
    pub fn on_event(&self, bus: &mut SweepBus) {
        bus.register(SweepLateConsumer);
    }
}

pub struct SweepBus;
impl SweepBus {
    pub fn register(&mut self, _c: SweepLateConsumer) {}
}
