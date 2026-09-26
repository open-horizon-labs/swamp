//! discovery-owned-by-report-pipeline: external and agent discovery take
//! a `report::DiscoveryPass`, which only `report::observe_scope` mints. A
//! second discovery pass elsewhere cannot forge one.
use swamp_core::report::DiscoveryPass;

fn main() {
    let _a = DiscoveryPass { _minted_by_observe_scope: () };
}
