//! event-bus-pluggable-consumers, extractors-are-pluggable: consumers are
//! registered only in the bus's static registrar (`with_builtins`).
use swamp_core::bus::{Consumer, EventBus};

fn add(bus: &mut EventBus, extra: Box<dyn Consumer>) {
    let _ = bus.register(extra);
}

fn main() {}
