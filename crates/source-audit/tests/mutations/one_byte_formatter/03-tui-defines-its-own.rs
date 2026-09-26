//! target: crates/tui/src/model.rs
//! by: audit:byte_units_only_in_the_formatter
//! ported: 2026-09-22 -- the original's `format!("{n} B")` collided with the imported `human_bytes` (an incidental compile error); the same second formatter under its own name, with a unit
//! why: the TUI defining its own byte formatter
fn sweep_tui_human_bytes(n: u64) -> String {
    format!("{:.1} KiB", n as f64 / 1024.0)
}
