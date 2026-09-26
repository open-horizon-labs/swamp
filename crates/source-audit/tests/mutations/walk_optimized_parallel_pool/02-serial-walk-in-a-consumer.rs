//! target: crates/core/src/consumers/walk.rs
//! by: compile:E0425, compile:E0432
//! note: 2026-09-22 -- the serial `attribution::attribute` is test-only code (`#[cfg(test)]`), so production has no serial walk to call
//! why: the same serial walk moved into a consumer, where a hand-written file list would not look
pub fn sweep_serial_consumer(root: &std::path::Path) {
    let _ = crate::attribution::attribute(root, &[], 1_000);
}
