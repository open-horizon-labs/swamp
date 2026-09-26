//! target: crates/core/src/recovery.rs
//! mode: append
//! by: audit:no_unreferenced_public_items, compile:clippy::disallowed_methods
//! why: re-review 3 sweep -- a dead public evidence function kept alive by a caller that is itself dead (blind spot: "has a non-test caller" is a one-hop question; nothing asks whether the caller is reachable from the pipeline)
/// Sweep: a capability the docs can claim and nothing reaches.
pub fn sweep_unreachable_recovery(p: &std::path::Path) -> bool {
    p.exists()
}
//! file: crates/core/src/render.rs
//! mode: append
/// Sweep: the caller that makes it look wired. Nothing calls this either.
pub fn sweep_dead_caller(p: &std::path::Path) -> bool {
    crate::recovery::sweep_unreachable_recovery(p)
}
