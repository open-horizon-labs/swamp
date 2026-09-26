//! execution-sinks-recheck-live-state (re-review 5, finding 4): `git
//! worktree prune` follows only a licensed Trash move, through that
//! move's receipt. The receipt has private fields, so claiming a move
//! that did not happen does not compile.
use std::path::PathBuf;
use swamp_core::fs_gate::destroy::Trashed;

fn main() {
    let _moved = Trashed {
        anchor: PathBuf::from("/w/wt"),
        dest: PathBuf::from("/tmp/trash/wt"),
    };
}
