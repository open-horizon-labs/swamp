//! folding-only-for-artifacts: a sized (folded) walk job carries the
//! `attribution::Classified` witness that `classify_at` found an artifact.
//! The witness is private to the crate and its field to its module; it
//! cannot be conjured to fold a directory that did not classify.
use swamp_core::attribution::Classified;

fn main() {
    let _w: Classified = todo!();
}
