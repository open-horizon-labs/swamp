//! target: crates/core/src/agents/codex.rs
//! by: audit:adapters_do_not_reach_gates
//! why: one adapter reusing another's parsing, so a Claude Code format change silently changes Codex identification
pub fn sweep_reuse_claude_parser(line: &str) -> Option<String> {
    crate::agents::claude_code::parse_session_header(line)
}
