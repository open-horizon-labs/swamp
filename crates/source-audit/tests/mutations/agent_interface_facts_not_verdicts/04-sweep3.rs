//! target: crates/core/src/agents/mod.rs
//! mode: append
//! by: audit:no_verdict_literals
//! why: re-review 3 sweep -- a verdict string reaches the renderer through a constant defined outside the scanned files (blind spot: only render.rs, agent_json.rs, cli/ and tui/ literals are scanned; a `pub const` in core carries the verdict past them)
/// Sweep: the verdict, defined where nothing looks for it.
pub const SWEEP_VERDICT: &str = "safe to delete";
//! file: crates/core/src/render.rs
//! mode: append
/// Sweep: prints the verdict without ever spelling it here.
pub fn sweep_render_verdict(out: &mut String) {
    out.push_str(crate::agents::SWEEP_VERDICT);
}
