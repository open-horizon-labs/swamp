//! target: crates/core/src/walk.rs
//! expect: accept
//! source: accept
//! why: a size taken with the non-following stat the gate names
/// Accept: `lstat`, spelled out.
fn sweep_accept_len(p: &Path) -> u64 {
    fs::symlink_metadata(p).map(|m| m.len()).unwrap_or(0)
}
