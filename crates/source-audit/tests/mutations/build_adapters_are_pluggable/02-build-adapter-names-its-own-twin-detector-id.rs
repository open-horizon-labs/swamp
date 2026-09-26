//! target: crates/core/src/build_adapters/android.rs
//! expect: accept
//! source: accept
//! why: a build adapter's own module may name its same-named detector's id constant, the same twin relationship a tool adapter already has with its detector, one family over
fn sweep_accept_same_family_id() -> &'static str {
    "android"
}
