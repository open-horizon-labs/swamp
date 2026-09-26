//! json-persistence-is-allowlisted (re-review 5, finding 4): a store
//! file is a fixed name inside a typed `StoreDir`, never a directory the
//! caller names -- so `JsonFile::UiState { store: <anywhere> }` does not
//! compile, and there is no free listing of an arbitrary directory.
use std::path::Path;
use swamp_core::fs_gate::store;

fn main() {
    let _ = store::write_json(
        store::JsonFile::UiState {
            store: Path::new("/Users/me/project"),
        },
        &1u32,
    );
    let _ = store::list_owned(Path::new("/Users/me"));
}
