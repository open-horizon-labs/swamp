//! target: crates/core/src/consumers/cache.rs
//! mode: append
//! by: audit:bus_static_registration
//! why: re-review 3 sweep -- one consumer reaching into another consumer's module for its parsing (blind spot: coupling is detected only through the *struct* names that `impl Consumer`; a free function in another consumer module is invisible)
/// Sweep: reaches into the docker consumer's own helpers.
pub fn sweep_reuse_docker_parsing(s: &str) -> u64 {
    crate::consumers::docker::sweep_parse_helper(s)
}
//! file: crates/core/src/consumers/docker.rs
//! mode: append
/// Sweep: the helper the other consumer now depends on.
pub fn sweep_parse_helper(s: &str) -> u64 {
    s.len() as u64
}
