//! target: crates/core/src/growth.rs
//! by: compile:E0616
//! ported: 2026-09-22 -- the old public row type is gone; the tombstone written on the real stored rows, from the history module's own parent
//! why: sweep slip -- a `present = false` sweep with no ObservationOwnership guard in the same function
fn sweep_mark_missing_external(rows: &mut [columns::StoredExternalRow]) {
    for row in rows.iter_mut() {
        row.present = false;
    }
}
