//! event-bus-pluggable-consumers: there is no empty bus to assemble a
//! private pipeline on; `EventBus::with_builtins` is the one constructor.
fn main() {
    let _ = swamp_core::bus::EventBus::new();
}
