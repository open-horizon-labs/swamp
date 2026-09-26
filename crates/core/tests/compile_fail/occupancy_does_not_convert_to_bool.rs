//! occupancy-is-tristate-at-sinks: nor a conversion to one.
use swamp_core::occupancy::OccupancyState;

fn as_bool(state: OccupancyState) -> bool {
    state.into()
}

fn main() {}
