//! target: crates/core/src/growth/columns.rs
//! by: audit:parquet_writers_are_the_named_fact_tables
//! why: a generic "write any batch to any path" helper inside the columns module itself, which would let every caller mint a table without a named writer
pub(super) fn write_view_rows(path: &Path, schema: Arc<Schema>, batch: RecordBatch) -> Result<()> {
    crate::fs_gate::columns::write_parquet_atomic(
        path,
        schema,
        std::iter::once(Ok(batch)),
        super::ARTIFACT_ZSTD_LEVEL,
    )
}
