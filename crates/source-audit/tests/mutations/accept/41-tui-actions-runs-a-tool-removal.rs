//! target: crates/tui/src/actions.rs
//! expect: accept
//! source: accept
//! why: the one allowed caller of `tool_removal::execute` is the TUI's actions module (on the human's `Y`)
fn sweep_accept_tool_removal(
    host: &swamp_core::tool_removal::Host,
    preview: &swamp_core::tool_removal::Preview,
    ledger: &swamp_core::ledger::Ledger,
) -> bool {
    swamp_core::tool_removal::execute(host, preview, &[], ledger)
        .recorded
        .is_ok()
}
