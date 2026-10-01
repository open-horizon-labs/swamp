//! target: crates/core/src/growth/columns.rs
//! by: audit:parquet_writers_are_the_named_fact_tables
//! why: the encoding-aware writer must not let an unnamed helper mint a persisted report view
pub(super) fn write_encoded_view_rows(path: &Path, schema: Arc<Schema>, batch: RecordBatch) -> Result<()> {
    crate::fs_gate::columns::write_parquet_atomic_with_dictionary_disabled(
        path,
        schema,
        std::iter::once(Ok(batch)),
        super::ARTIFACT_ZSTD_LEVEL,
        &[],
    )
}
