//! json-persistence-is-allowlisted: the atomic byte writer behind the
//! typed store files is private to the gate.
use std::path::Path;

fn main() {
    let _ = swamp_core::fs_gate::store::write_atomic(Path::new("/tmp/unit-sidecar.json"), b"{}");
}
