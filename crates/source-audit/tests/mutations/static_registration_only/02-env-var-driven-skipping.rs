//! target: crates/core/src/bus/mod.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; `SweepGate` is not a consumer and nothing runs it; the registered set is `with_builtins`, whose `register` is private.
//! why: sweep slip -- env-var-driven consumer skipping, i.e. the registered set decided at run time
pub struct SweepGate;

impl SweepGate {
    pub fn on_event(&self, bus: &mut EventBus) {
        if std::env::var("SWAMP_SKIP_CONSUMERS").is_ok() {
            return;
        }
        *bus = EventBus::with_builtins();
    }
}
