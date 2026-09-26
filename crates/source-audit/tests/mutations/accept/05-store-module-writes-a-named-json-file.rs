//! target: crates/core/src/scope.rs
//! expect: accept
//! source: accept
//! ported: 2026-09-23 -- store files take a typed `StoreDir` (re-review 5, finding 4); a store module builds one from its path. 2026-09-25 (R18b): the one `JsonFile` variant left is the TUI's `ui_state.json`.
//! why: a store module persisting the one `JsonFile` variant
/// Accept: the UI state file, through the allow-listed writer.
fn sweep_accept_persist(store: &Path, roots: &[String]) -> std::io::Result<()> {
    let store = crate::fs_gate::store::StoreDir::at(store)?;
    crate::fs_gate::store::write_json(
        crate::fs_gate::store::JsonFile::UiState { store: &store },
        roots,
    )
}
