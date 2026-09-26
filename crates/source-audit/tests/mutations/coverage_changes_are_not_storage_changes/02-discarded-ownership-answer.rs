//! target: crates/core/src/growth.rs
//! by: compile:E0616
//! ported: 2026-09-22 -- the real stored row, beside the ownership whose answer is discarded
//! why: sweep slip -- `let _owned = ownership.owns(key);` then an unguarded regrowth bump
fn sweep_regrow(ownership: &ObservationOwnership, key: &str, row: &mut columns::StoredExternalRow) {
    let _owned = ownership.owns(key);
    row.regrowth_count = row.regrowth_count + 1;
}
