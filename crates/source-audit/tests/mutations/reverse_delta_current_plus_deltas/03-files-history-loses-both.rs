//! target: crates/core/src/growth.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; the history writers are private to `growth::columns` (compile_fail `history_rows_are_private_to_the_store`), and current-plus-delta behaviour is asserted by report_growth.rs and dirs_and_files.rs.
//! why: the files table silently stops keeping history at all while the function keeps its name
pub mod sweep_files_no_history {
    pub fn observe_and_annotate_files(_dir: &std::path::Path) -> anyhow::Result<()> {
        Ok(())
    }
}
