//! target: crates/core/src/signals.rs
//! mode: append
//! expect: accept
//! source: accept, re-review 5 finding 5
//! why: the git-signal module asking the gate's read-only repository handle a question is the legitimate shape of every gitoxide use
/// Accept: HEAD's id, through the gate.
pub(crate) fn sweep_accept_head(dir: &Path) -> Option<String> {
    crate::fs_gate::git::Repo::open(dir)?.head_id_hex()
}
