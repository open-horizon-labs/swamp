//! target: crates/core/src/agents/opencode.rs
//! by: audit:adapters_do_not_reach_gates
//! why: an adapter that reaches the action layer is not inspection-only
pub fn sweep_adapter_plans(units: &[crate::agents::AgentUnit], paths: &[std::path::PathBuf]) {
    let _ = crate::actions::propose_agents(units, paths, "adapter");
}
