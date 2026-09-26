//! occupancy-is-tristate-at-sinks: there is no boolean "is free" to
//! collapse Unknown into Free.
use swamp_core::occupancy::OccupancyState;

fn proceed(state: &OccupancyState) -> bool {
    state.is_free()
}

fn main() {}
