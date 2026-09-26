//! target: crates/core/src/external.rs
//! mode: append
//! by: compile:E0425, compile:E0432
//! note: 2026-09-22 -- the serial `attribution::attribute` is test-only code (`#[cfg(test)]`), so production has no serial walk to call
//! why: re-review 3 sweep -- the serial, test-only walk called from the external measurement pass (blind spot: the caller list is report.rs plus consumers/; external.rs is on the report path and is not in it)
/// Sweep: the serial walk, on the report path, outside the audited list.
pub fn sweep_serial_fold(root: &std::path::Path) {
    let _ = crate::attribution::attribute(root, &[], 0);
}
