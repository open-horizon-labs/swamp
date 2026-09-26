//! target: crates/core/src/scope.rs
//! expect: accept
//! source: accept
//! why: writing a fact table goes through its one named writer (`growth::write_notes_table` -> `columns::write_note_rows`); no new path to the Parquet writer is minted
/// Accept: the run's notes, through the named table writer.
fn sweep_accept_notes(store: &Path, key: &str, notes: &[String]) -> anyhow::Result<()> {
    crate::growth::write_notes_table(store, key, notes, 0)
}
