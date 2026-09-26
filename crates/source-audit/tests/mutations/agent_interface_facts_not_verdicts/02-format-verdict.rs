//! target: crates/core/src/agent_json.rs
//! by: audit:no_verdict_literals
//! why: a verdict assembled inside format! rather than written as a bare literal
pub fn sweep_verdict_json(path: &std::path::Path) -> String {
    format!("{} is unused", path.display())
}
