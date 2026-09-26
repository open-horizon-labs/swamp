//! target: crates/core/src/agents/cline.rs
//! expect: accept
//! source: accept, review-4 A4 ported to the gate
//! why: A4 -- a read-only, no-follow look at a session header. Under the gate an adapter does this through its `IdentifyCtx` (a raw `OpenOptions` is a gate path anywhere outside `fs_gate` by design); the legitimate shape must pass every rule.
/// Accept: stat without following, then a capped header read.
fn sweep_accept_header(ctx: &crate::agents::IdentifyCtx<'_>, p: &std::path::Path) -> Option<String> {
    let meta = ctx.stat(p).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    ctx.read_header(p, 256)
}
