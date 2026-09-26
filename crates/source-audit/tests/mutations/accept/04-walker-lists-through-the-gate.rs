//! target: crates/core/src/walk.rs
//! expect: accept
//! source: accept
//! why: a walker module taking a directory listing through `fs_gate::read_dir` (its capability group allows the walker)
/// Accept: one listing, in the walker.
fn sweep_accept_entries(dir: &Path) -> usize {
    fs::read_dir(dir).map(|rd| rd.flatten().count()).unwrap_or(0)
}
