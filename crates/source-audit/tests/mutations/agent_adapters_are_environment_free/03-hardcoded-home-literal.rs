//! target: crates/core/src/agents/pi.rs
//! by: audit:adapters_do_not_reach_gates
//! why: a hardcoded home path is the same coupling written as a literal
pub fn sweep_hardcoded_home() -> std::path::PathBuf {
    std::path::PathBuf::from("/Users/dev/.pi")
}
