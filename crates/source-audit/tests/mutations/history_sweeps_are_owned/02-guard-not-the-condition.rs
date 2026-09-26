//! target: crates/core/src/growth.rs
//! by: compile:E0616
//! why: the ownership guard is earlier in the body but not the condition of the write
pub fn sweep_guard_elsewhere(
    ownership: &ObservationOwnership,
    current: &mut HashMap<String, StoredExternalRow>,
    observed_at: u64,
) {
    let mut any = false;
    for (key, _row) in current.iter() {
        if ownership.owns(key) {
            any = true;
        }
    }
    if any {
        for (_key, row) in current.iter_mut() {
            row.present = false;
            row.observed_at = observed_at;
        }
    }
}
