//! target: crates/core/src/report.rs
//! mode: append
//! expect: reject
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M5
//! by: audit:gate_paths_only_inside_gates, compile:clippy::disallowed_methods, compile:clippy::disallowed_types
//! why: an uncompressed Parquet writer built with `ArrowWriter::try_new_with_options`
//! blind-spot (old model): a Parquet writer is a call to `ArrowWriter::try_new`/`ArrowWriter::new`; the options constructor (default properties: UNCOMPRESSED) is not a writer at all
/// Sweep 4: Parquet with the default (uncompressed) properties.
pub fn sweep4_write_rows(path: &Path, batch: arrow_array::RecordBatch) -> anyhow::Result<()> {
    let file = std::fs::File::create(path)?;
    let opts = parquet::arrow::arrow_writer::ArrowWriterOptions::new();
    let mut w = parquet::arrow::ArrowWriter::try_new_with_options(file, batch.schema(), opts)?;
    w.write(&batch)?;
    w.close()?;
    Ok(())
}
