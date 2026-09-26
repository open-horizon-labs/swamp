//! target: crates/core/src/consumers/gate.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; it "registers" on a bus type the fixture itself defines. The real `EventBus::register` is private to the registrar (compile_fail `bus_register_is_private`; sweep fixtures 04-sweep3, M16, M18 exercise the real one).
//! why: the composed guardrail's second half -- registration decided at event time
pub struct SweepGateLate;

impl SweepGateLate {
    pub fn on_event(&self, bus: &mut SweepGateBus) {
        bus.register(());
    }
}

pub struct SweepGateBus;
impl SweepGateBus {
    pub fn register(&mut self, _c: ()) {}
}
