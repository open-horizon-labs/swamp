//! target: crates/core/src/agents/cline.rs
//! by: audit:adapters_do_not_reach_gates
//! why: the original forbidden shape must still be rejected
pub fn sweep_emit_plain(session: &str) {
    eprintln!("session {session}");
}
