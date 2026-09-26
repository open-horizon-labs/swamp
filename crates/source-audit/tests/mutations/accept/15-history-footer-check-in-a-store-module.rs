//! target: crates/core/src/store.rs
//! expect: accept
//! source: accept
//! why: a column-store module using the gate's Parquet reader
/// Accept: the cheap "is this Parquet at all" check.
fn sweep_accept_is_parquet(p: &std::path::Path) -> bool {
    crate::fs_gate::columns::has_parquet_footer(p).unwrap_or(false)
}
