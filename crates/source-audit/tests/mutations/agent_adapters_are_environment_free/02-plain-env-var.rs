//! target: crates/core/src/agents/opencode.rs
//! by: audit:adapters_do_not_reach_gates
//! why: the original forbidden shape must still be rejected
pub fn sweep_plain_env() -> String {
    std::env::var("XDG_STATE_HOME").unwrap_or_default()
}
