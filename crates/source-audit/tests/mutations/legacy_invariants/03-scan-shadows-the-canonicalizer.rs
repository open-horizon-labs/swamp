//! target: crates/core/src/scan.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; root canonicalization is asserted by runtime tests (root_scope.rs, multi_root_coverage.rs).
//! why: alias/shadow variant -- scan.rs defines its own `canonical_roots` that canonicalizes nothing, so the name is present and the invariant is gone
pub fn canonical_roots(roots: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    roots.to_vec()
}
