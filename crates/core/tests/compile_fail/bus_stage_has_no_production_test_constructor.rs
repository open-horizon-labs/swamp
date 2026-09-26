//! no-second-traversal-on-report-path: the test constructor for a bus
//! stage exists only under the `testing` feature.
fn main() {
    let _ = swamp_core::bus::Stage::for_tests();
}
