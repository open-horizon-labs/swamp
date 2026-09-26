//! target: crates/core/src/growth.rs
//! expect: retired
//! retired: 2026-09-22 -- only defines a function (in a stand-in module) that nothing calls; the history writers are private to `growth::columns` (compile_fail `history_rows_are_private_to_the_store`), and current-plus-delta behaviour is asserted by report_growth.rs and dirs_and_files.rs.
//! why: the other half dropped -- deltas appended but the current table never rewritten, so every read replays the whole log
pub mod sweep_delta_only {
    pub fn observe_and_annotate_dirs(dir: &std::path::Path) -> anyhow::Result<()> {
        let _ = super::next_seq_path(dir, "dirs-");
        Ok(())
    }
}
