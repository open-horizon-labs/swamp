//! target: crates/core/src/growth.rs
//! by: audit:gate_paths_only_inside_gates
//! why: alias/rename variant -- the column renamed, so the minutes contract is silently dropped
pub mod sweep_renamed_column {
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    pub fn files_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("rel_path", DataType::Utf8, false),
            Field::new("mod_time", DataType::Int32, false),
        ]))
    }
}
