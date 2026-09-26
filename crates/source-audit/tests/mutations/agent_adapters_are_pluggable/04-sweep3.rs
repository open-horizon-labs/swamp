//! target: crates/cli/src/main.rs
//! mode: append
//! by: audit:ids_only_in_their_module
//! why: re-review 3 sweep -- a central `match` over adapter tool-id constants, in the CLI (blind spot: the central-dispatch rule reads agents/mod.rs, actions.rs and tui/ only; crates/cli/src is not in `dispatch_files`)
/// Sweep: adding an adapter now means editing this table.
pub fn sweep_dispatch(id: &str) -> u64 {
    match id {
        x if x == swamp_core::agents::cline::CLINE_TOOL_ID => 1,
        x if x == swamp_core::agents::codex::CODEX_TOOL_ID => 2,
        _ => 0,
    }
}
