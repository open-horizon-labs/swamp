//! target: crates/core/src/occupancy.rs
//! expect: accept
//! source: accept
//! why: an Unknown fact whose reason is a checked `reason!` template with an argument
/// Accept: an unanswerable probe says why.
fn sweep_accept_unknown(observed_at: u64, e: &std::io::Error) -> Evidence {
    Evidence::unknown(
        FactKind::CurrentUse,
        FactSubtype::Lock,
        EvidenceSource::DockerApi {
            detail: "not consulted".into(),
        },
        observed_at,
        crate::reason!("the lock probe could not run: {e}"),
    )
}
