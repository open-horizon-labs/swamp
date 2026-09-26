//! target: crates/core/src/agents/claude_code.rs
//! expect: accept
//! source: accept
//! why: an adapter listing one level through `IdentifyCtx::list` (capped, symlink-refusing)
/// Accept: the context's listing, not a traversal.
fn sweep_accept_count(ctx: &crate::agents::IdentifyCtx<'_>, dir: &std::path::Path) -> usize {
    ctx.list(dir).len()
}
