//! agent-adapters-read-bounded-headers-only: the gate has no whole-file
//! read of user content to reach for.
fn main() {
    let _ = swamp_core::fs_gate::read::read_to_end("/tmp/transcript.jsonl");
}
