//! target: crates/core/src/consumers/signals.rs
//! by: audit:bus_static_registration
//! why: one consumer reaching into another's type, so a change to one silently changes the other
pub fn sweep_reuse_growth_consumer() -> Option<crate::consumers::growth::GrowthConsumer> {
    None
}
