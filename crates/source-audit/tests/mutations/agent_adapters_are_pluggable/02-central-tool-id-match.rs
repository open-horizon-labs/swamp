//! target: crates/core/src/agents/mod.rs
//! by: audit:ids_only_in_their_module
//! why: a central match over adapter tool-id constants -- adding an adapter would mean editing a dispatch table
pub fn sweep_dispatch(tool_id: &str) -> &'static str {
    match tool_id {
        crate::agents::codex::CODEX_TOOL_ID => "codex",
        _ => "other",
    }
}
