//! target: crates/core/src/external.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M11
//! by: compile:E0425, compile:E0432
//! note: 2026-09-22 -- the serial `attribution::attribute` is test-only code (`#[cfg(test)]`), so production has no serial walk to call
//! why: the serial, test-only walk called on the report path through a locally defined `macro_rules!`
//! blind-spot (old model): `visit_macro` skips `macro_rules` bodies ("a declarative macro body is not a call site") and the call site's tokens are only the argument; a local macro is a call the graph never sees
macro_rules! sweep4_serial {
    ($root:expr) => {
        crate::attribution::attribute($root, &[], 0)
    };
}

/// Sweep 4: the serial walk, one macro away.
pub fn sweep4_serial_fold(root: &Path) -> usize {
    let r = sweep4_serial!(root);
    r.unowned.len()
}
