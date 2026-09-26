//! target: crates/core/src/growth.rs
//! by: audit:gate_paths_only_inside_gates
//! why: a second writer choosing a different codec, so "parquet + zstd" becomes true of only some tables
pub fn sweep_write_snappy(
    path: &std::path::Path,
    schema: std::sync::Arc<arrow_schema::Schema>,
    batch: &arrow_array::RecordBatch,
) -> anyhow::Result<()> {
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let file = std::fs::File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(batch)?;
    writer.close()?;
    Ok(())
}
