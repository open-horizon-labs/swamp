//! target: crates/core/src/bus/mod.rs
//! mode: append
//! by: compile:E0624
//! why: re-review 3 sweep -- consumers registered at run time, from a helper that is not called `on_event` (blind spot: the rule inspects only functions *named* `on_event`)
impl EventBus {
    /// Sweep: the registered set decided while events are flowing.
    pub fn sweep_register_late(&mut self, c: Box<dyn Consumer>) -> anyhow::Result<()> {
        self.register(c)
    }
}
