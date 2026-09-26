//! target: crates/core/src/consumer_wiring.rs
//! by: audit:ids_only_in_their_module
//! ported: 2026-09-22 -- the constant's current path (`locations::cargo_home`)
//! why: wiring matching on a detector id constant, so adding a detector means editing a table
fn sweep_wire_by_id(id: &str) -> bool {
    id == crate::locations::cargo_home::CARGO_HOME_DETECTOR_ID
}
