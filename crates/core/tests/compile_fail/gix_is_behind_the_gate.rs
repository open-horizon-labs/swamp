//! every-spawn-is-counted / symlinks-never-followed (re-review 5,
//! finding 5): gitoxide lives in `fs_gate::git`, behind read-only queries.
//! The repository handle is opaque, so reaching the `gix::Repository`
//! inside it -- and through it `gix_fs`, `gix_command`, temp and lock
//! files -- does not compile.
use std::path::Path;
use swamp_core::fs_gate::git::Repo;

fn main() {
    let repo = Repo::open(Path::new("/w")).unwrap();
    let _inner = &repo.0;
}
