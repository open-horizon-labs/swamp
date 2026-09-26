//! target: crates/core/src/growth.rs
//! by: audit:parquet_writers_are_the_named_fact_tables
//! why: a new table that stores a Report view (summary totals) -- exactly the R17-R18a-3 shape R20 removed; the writer is not one of the named fact-table writers
pub fn sweep_write_summary_table(
    swamp_dir: &Path,
    schema: std::sync::Arc<arrow_schema::Schema>,
    batch: arrow_array::RecordBatch,
) -> Result<()> {
    crate::fs_gate::columns::write_parquet_atomic(
        &swamp_dir.join("summary.parquet"),
        schema,
        std::iter::once(Ok(batch)),
        3,
    )
}
