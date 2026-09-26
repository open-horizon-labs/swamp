//! target: crates/core/src/growth.rs
//! by: compile:E0616
//! ported: 2026-09-22 -- the real stored row type
//! why: alias/rename variant -- the tombstone written through a renamed local binding, one function away from any guard
fn sweep_tombstone_agent_rows(rows: &mut Vec<columns::StoredExternalRow>) {
    for r in rows.iter_mut() {
        let present = &mut r.present;
        *present = false;
    }
}
