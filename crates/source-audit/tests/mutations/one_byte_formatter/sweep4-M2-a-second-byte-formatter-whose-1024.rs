//! target: crates/tui/src/model.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M2
//! by: audit:byte_units_only_in_the_formatter
//! why: a second byte formatter whose 1024 divisor is a named constant
//! blind-spot (old model): the divisor test is the token `1024`/`1024.0`/`1<<10` in the body; constants are followed for the *labels* only, so `const KIB: f64 = 1024.0` hides the divisor
const SWEEP4_KIB: f64 = 1024.0;

/// Sweep 4: binary divisor behind a constant, binary label in the macro.
pub fn sweep4_size_label(n: u64) -> String {
    let v = n as f64 / SWEEP4_KIB;
    format!("{v:.1} KiB")
}
