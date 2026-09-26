//! target: crates/core/src/actions.rs
//! mode: append
//! expect: accept
//! source: accept, re-review 5 finding 6
//! why: lowering a lint that is not one of the gate's (`too_many_arguments`) outside the gate is ordinary code; only the gate's lints and their groups are pinned
/// Accept: a long signature, allowed.
#[allow(clippy::too_many_arguments)]
fn sweep_accept_many(a: u8, b: u8, c: u8, d: u8, e: u8, f: u8, g: u8, h: u8) -> u8 {
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h
}
