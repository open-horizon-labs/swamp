//! agent-adapters-read-bounded-headers-only: the gate's only content read
//! takes a `BoundedCap`. An uncapped read does not compile.
use swamp_core::fs_gate::read::bounded_read;

fn main() {
    let _ = bounded_read("/tmp/transcript.jsonl");
}
