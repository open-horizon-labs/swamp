//! target: crates/core/src/build_adapters/gradle.rs
//! by: audit:ids_only_in_their_module
//! why: a build adapter naming another ecosystem's detector id directly, with no twin relationship, is the central-chain shape the rule forbids
fn sweep_reject_cross_family_id(id: &str) -> bool {
    id == "android"
}
