//! target: crates/cli/src/main.rs
//! by: audit:gate_paths_only_inside_gates
//! why: a CLI verb that runs a tool-managed removal: no Trash, so only the human's `Y` on the TUI confirm may reach `tool_removal::execute`
pub fn sweep_remove_with_mise(
    host: &swamp_core::tool_removal::Host,
    preview: &swamp_core::tool_removal::Preview,
    ledger: &swamp_core::ledger::Ledger,
) -> bool {
    matches!(
        swamp_core::tool_removal::execute(host, preview, &[], ledger).status,
        swamp_core::tool_removal::Status::Removed
    )
}
