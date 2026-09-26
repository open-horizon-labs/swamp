//! target: crates/core/src/growth.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M44
//! by: compile:E0616
//! why: an unguarded regrowth bump written `saturating_add(1)`
//! blind-spot (old model): a bump is `+=` or a right-hand side containing `regrowth_count+`; `x.regrowth_count.saturating_add(1)` is neither
/// Sweep 4: scores a regrowth for every row, covered or not.
#[allow(dead_code)]
fn sweep4_score_regrowth(rows: &mut [StoredRow]) {
    for row in rows.iter_mut() {
        row.regrowth_count = row.regrowth_count.saturating_add(1);
    }
}
