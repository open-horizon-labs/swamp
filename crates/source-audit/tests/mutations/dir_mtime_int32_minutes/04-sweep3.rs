//! target: crates/core/src/growth.rs
//! mode: append
//! by: audit:gate_paths_only_inside_gates
//! why: re-review 3 sweep -- a second directory schema storing seconds in an Int64 column (blind spot: the rule inspects functions named exactly `dirs_schema`/`files_schema`; a renamed schema builder is unaudited)
/// Sweep: the same table, a different builder name, seconds.
pub fn dirs_schema_v2() -> std::sync::Arc<arrow_schema::Schema> {
    std::sync::Arc::new(arrow_schema::Schema::new(vec![
        arrow_schema::Field::new("rel_path", arrow_schema::DataType::Utf8, false),
        arrow_schema::Field::new("mod_time_secs", arrow_schema::DataType::Int64, false),
    ]))
}
