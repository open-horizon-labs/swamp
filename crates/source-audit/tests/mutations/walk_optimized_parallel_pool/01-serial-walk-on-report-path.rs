//! target: crates/core/src/report.rs
//! by: compile:E0425, compile:E0432
//! note: 2026-09-22 -- the serial `attribution::attribute` is test-only code (`#[cfg(test)]`), so production has no serial walk to call
//! why: the serial, test-only walk called on the report path, so a large tree costs what the pool exists to avoid
pub fn sweep_serial_report(root: &std::path::Path) {
    let _ = crate::attribution::attribute(root, &[], 1_000);
}
