//! discovery-owned-by-report-pipeline: `DiscoveryPass::begin` is private
//! to the report module, and the test constructor exists only under the
//! `testing` feature.
use swamp_core::report::DiscoveryPass;

fn main() {
    let _b = DiscoveryPass::begin();
}
