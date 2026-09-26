//! target: crates/core/src/growth.rs
//! mode: append
//! by: compile:E0616
//! why: re-review 3 sweep -- an unguarded tombstone whose right-hand side is a constant rather than the literal `false` (blind spot: a tombstone is recognised as `lhs.ends_with(". present") && rhs.trim() == "false"`; naming the constant defeats it)
const SWEEP_ABSENT: bool = false;

/// Sweep: tombstones every row it is handed, owned or not.
fn sweep_tombstone_all(rows: &mut [StoredRow]) {
    for row in rows.iter_mut() {
        row.present = SWEEP_ABSENT;
    }
}
