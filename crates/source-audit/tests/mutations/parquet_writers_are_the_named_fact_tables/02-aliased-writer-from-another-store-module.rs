//! target: crates/core/src/github.rs
//! by: audit:parquet_writers_are_the_named_fact_tables
//! why: the same writer behind a `use ... as` alias, from a module that already holds one legitimate table writer
use crate::fs_gate::columns::write_parquet_atomic as publish_table;
pub fn sweep_cache_series(
    store: &Path,
    schema: std::sync::Arc<arrow_schema::Schema>,
    batch: arrow_array::RecordBatch,
) -> Result<()> {
    publish_table(
        &store.join("series.parquet"),
        schema,
        std::iter::once(Ok(batch)),
        3,
    )
}
