//! protection-fails-closed: the protect list is opaque; its only query is
//! `conflict` (both directions, one predicate). Reading the paths to write
//! a second, one-directional predicate does not compile.
use std::path::Path;
use swamp_core::protection::ProtectList;

fn my_own_predicate(list: &ProtectList, candidate: &Path) -> bool {
    list.paths.iter().any(|p| candidate.starts_with(p))
}

fn main() {}
