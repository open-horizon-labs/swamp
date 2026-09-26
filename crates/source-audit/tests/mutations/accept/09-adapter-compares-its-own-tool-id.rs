//! target: crates/core/src/agents/cline.rs
//! expect: accept
//! source: accept
//! why: an adapter writing its own tool id literal
/// Accept: its own id, in its own module.
fn sweep_accept_is_mine(id: &str) -> bool {
    id == "cline"
}
