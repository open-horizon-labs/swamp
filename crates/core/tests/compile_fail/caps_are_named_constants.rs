//! agent-adapters-read-bounded-headers-only: a cap is one of the named
//! constants (or `header_at_most`, clamped to the header cap); a cap of
//! "everything" cannot be built.
use swamp_core::fs_gate::read::BoundedCap;

fn main() {
    let _ = BoundedCap(usize::MAX);
}
