//! target: crates/core/src/scan.rs
//! mode: replace
//! expect: retired
//! retired: 2026-09-22 -- a whole-file replacement of scan.rs with an older API breaks unrelated code, so it proves nothing about the invariant; root canonicalization is asserted by root_scope.rs.
//! why: roots no longer canonicalized at the boundary, so `~/src` and its `/private` spelling become two different roots
use std::path::PathBuf;

pub fn roots_from(config: &[String]) -> Vec<PathBuf> {
    config.iter().map(PathBuf::from).collect()
}
