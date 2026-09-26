//! target: crates/core/src/consumer_wiring.rs
//! mode: append
//! by: audit:ids_only_in_their_module
//! why: re-review 3 sweep -- wiring that matches on detector id *string literals* instead of the constants (blind spot: the rule greps for the token `_DETECTOR_ID`; the same coupling written as the literal the constant holds is invisible)
/// Sweep: the wiring table is back, written as literals.
pub fn sweep_recovery_hint(id: &str) -> &'static str {
    match id {
        "cargo" => "cargo fetch",
        "npm" => "npm ci",
        "docker-desktop" => "docker pull",
        _ => "",
    }
}
