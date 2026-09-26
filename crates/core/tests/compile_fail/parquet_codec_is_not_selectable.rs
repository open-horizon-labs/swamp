//! column-store-parquet-zstd: the one Parquet writer's properties
//! (always zstd) are built privately; no caller picks a codec.
fn main() {
    let _ = swamp_core::fs_gate::columns::zstd_properties(3);
}
