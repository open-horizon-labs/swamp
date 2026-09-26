//! target: crates/core/src/growth.rs
//! by: audit:gate_paths_only_inside_gates
//! why: the column widened to a timestamp, which is the same store growth the Int32-minutes decision exists to prevent
pub mod sweep_widened_column {
    use arrow_schema::{DataType, Field, Schema, TimeUnit};
    use std::sync::Arc;

    pub fn dirs_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![Field::new(
            "mod_time_min",
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            false,
        )]))
    }
}
