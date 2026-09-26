//! target: crates/core/src/store.rs
//! by: audit:gate_paths_only_inside_gates
//! why: a Parquet writer that declares no compression -- the column store's size claim quietly stops holding
pub fn sweep_write_uncompressed(
    path: &std::path::Path,
    schema: std::sync::Arc<arrow_schema::Schema>,
    batch: &arrow_array::RecordBatch,
) -> anyhow::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, None)?;
    writer.write(batch)?;
    writer.close()?;
    Ok(())
}
