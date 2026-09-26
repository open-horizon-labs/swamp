//! target: crates/core/src/growth.rs
//! by: audit:gate_paths_only_inside_gates
//! why: sweep slip -- seconds stored in the minutes column, so every stored directory mtime is 60x wrong
pub mod sweep_seconds_schema {
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;

    pub fn dirs_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("rel_path", DataType::Utf8, false),
            Field::new("mod_time_min", DataType::Int64, false),
        ]))
    }
}
