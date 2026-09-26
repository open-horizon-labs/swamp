//! no-second-traversal-on-report-path, walk-optimized-parallel-pool,
//! fsevents-before-full-walk: the walk takes a `bus::Stage`, which only
//! `EventBus::run` mints. A walk from outside the report pipeline -- a
//! second traversal -- has no stage to pass, and cannot forge one.
use swamp_core::bus::Stage;

fn main() {
    let stage = Stage { _minted_by_the_bus: () };
    let _ = swamp_core::walk::discover_and_attribute(&stage, std::path::Path::new("/"), 0, 0, &[]);
}
