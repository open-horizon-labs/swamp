//! target: crates/core/src/consumers/docker.rs
//! by: audit:bus_static_registration
//! why: the composed guardrail's first half -- a consumer that knows the bus it runs on
pub fn sweep_docker_consumer_knows_the_bus(bus: &mut crate::bus::EventBus) {
    let _ = bus;
}
