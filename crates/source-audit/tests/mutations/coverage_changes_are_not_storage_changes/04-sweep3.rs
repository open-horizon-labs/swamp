//! target: crates/core/src/growth.rs
//! mode: append
//! by: compile:E0616
//! why: re-review 3 sweep -- an unguarded regrowth bump written with `+=` (blind spot: the tombstone needle is the literal `regrowth_count + 1`; the token text of `+=` is `regrowth_count += 1`, which does not contain it)
/// Sweep: scores a regrowth for every row, covered or not.
fn sweep_score_regrowth(rows: &mut [StoredRow]) {
    for row in rows.iter_mut() {
        row.regrowth_count += 1;
    }
}
