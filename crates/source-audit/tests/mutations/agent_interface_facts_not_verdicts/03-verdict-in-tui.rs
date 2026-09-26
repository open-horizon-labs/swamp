//! target: crates/tui/src/ui.rs
//! by: audit:no_verdict_literals
//! why: the verdict vocabulary is banned in every user-facing surface, not only core's renderer
pub fn sweep_verdict_status() -> String {
    "safe to remove".to_string()
}
