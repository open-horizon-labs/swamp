//! json-persistence-is-allowlisted, store-data-is-parquet-not-json-sidecars:
//! JSON reaches disk only as one of the `JsonFile` variants; there is no
//! write to an arbitrary path to launder a sidecar through.
use std::path::Path;
use swamp_core::fs_gate::store;

fn main() {
    let _ = store::write_json(Path::new("/tmp/unit-sidecar.json"), &1u32);
}
