//! target: crates/core/src/consumers/growth.rs
//! by: audit:bus_static_registration
//! why: a consumer naming the bus -- the coupling the bus exists to remove
pub fn sweep_register_self(bus: &mut crate::bus::EventBus) {
    let _ = bus;
}
