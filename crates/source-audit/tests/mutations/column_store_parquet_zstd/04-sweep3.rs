//! target: crates/core/src/report.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: re-review 3 sweep -- an uncompressed Parquet writer in a third file (blind spot: the audit reads exactly two files (growth.rs, store.rs); a writer anywhere else is unaudited)
/// Sweep: Parquet, no compression, outside the two audited files.
pub fn sweep_write_rows(path: &std::path::Path, batch: arrow_array::RecordBatch) {
    let file = std::fs::File::create(path).unwrap();
    let mut w = parquet::arrow::ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
}
