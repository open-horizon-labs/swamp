//! target: crates/core/src/agents/codex.rs
//! by: audit:adapters_do_not_reach_gates
//! why: sweep slip -- `writeln!(stderr())` evades an emitter list of println!/eprintln!
pub fn sweep_emit_via_writeln(session: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "session {session}");
}
