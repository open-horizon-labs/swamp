//! target: crates/core/src/growth.rs
//! mode: append
//! expect: reject
//! by: compile:E0603
//! ported: 2026-09-22 -- the artifact table's writer moved into the private `growth::columns`; the reviewer's shape, pointed at it
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M6
//! why: the delta path is honoured -- as an `if` condition that returns early -- and the current table is rewritten with no delta
//! blind-spot (old model): `used` is `honoured != Discarded`; reading the delta path in a condition is honoured, and nothing asks that it reach a write
/// Sweep 4: consults the delta path, never appends one.
pub mod sweep4_history {
    pub fn sweep4_rewrite_current(dir: &std::path::Path) -> anyhow::Result<()> {
        let cur = super::current_path(dir);
        if super::next_delta_path(dir).exists() {
            return Ok(());
        }
        super::columns::write_rows(&cur, &[])
    }
}
