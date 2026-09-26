//! target: crates/core/src/growth.rs
//! mode: append
//! expect: reject
//! by: audit:gate_paths_only_inside_gates
//! ported: 2026-09-22 -- the Arrow imports moved out of growth.rs with the column store (`growth::columns`); the reviewer's schema, written with its own paths
//! source: review-4 sweep (M group, reviewer_counterexamples_stack4_sweep.rs), M12
//! why: a modification-time column named with `format!`, stored as Int64 seconds
//! blind-spot (old model): the column name must `eval_literal` to a string starting `mod_time`; a `format!` evaluates to nothing and the field is skipped
/// Sweep 4: seconds, under a name the audit cannot evaluate.
pub fn sweep4_dirs_schema() -> std::sync::Arc<arrow_schema::Schema> {
    use arrow_schema::{DataType, Field, Schema};
    std::sync::Arc::new(Schema::new(vec![
        Field::new("rel_path", DataType::Utf8, false),
        Field::new(format!("mod_time_{}", "secs"), DataType::Int64, false),
    ]))
}
