//! target: crates/core/src/store.rs
//! by: audit:gate_paths_only_inside_gates
//! why: the same omission in the other half of the store, where a single-file audit would not look
pub fn sweep_write_plain_rows(
    path: &std::path::Path,
    schema: std::sync::Arc<arrow_schema::Schema>,
    batch: &arrow_array::RecordBatch,
) -> anyhow::Result<()> {
    let f = std::fs::File::create(path)?;
    let mut w = ArrowWriter::try_new(f, schema, Option::<WriterProperties>::None)?;
    w.write(batch)?;
    w.close()?;
    Ok(())
}
