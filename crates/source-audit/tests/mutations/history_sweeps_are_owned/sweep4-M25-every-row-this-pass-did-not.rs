//! target: crates/core/src/growth.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M25
//! by: compile:E0616
//! why: every row this pass did not see is tombstoned, guarded by `!seen.contains(..)` on a parameter
//! blind-spot (old model): `ownership_condition` accepts "a negated membership test on a parameter" as ownership -- which is the exact shape of the cross-family sweep (CE4) the rule exists to forbid
/// Sweep 4: absent from this pass == deleted.
#[allow(dead_code)]
fn sweep4_tombstone_unseen(rows: &mut [StoredRow], seen: &HashSet<String>) {
    for row in rows.iter_mut() {
        if !seen.contains(&row.rel_path) {
            row.present = false;
        }
    }
}
