//! target: crates/core/src/growth.rs
//! by: compile:E0616
//! why: sweep slip -- `let _owned = ownership.owns(key);` then an unguarded tombstone
pub fn sweep_unowned_tombstone(
    ownership: &ObservationOwnership,
    current: &mut HashMap<String, StoredExternalRow>,
    observed_at: u64,
) {
    for (key, row) in current.iter_mut() {
        let _owned = ownership.owns(key);
        row.present = false;
        row.bytes = 0;
        row.observed_at = observed_at;
    }
}
