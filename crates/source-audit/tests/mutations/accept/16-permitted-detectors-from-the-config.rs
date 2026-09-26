//! target: crates/core/src/scope.rs
//! expect: accept
//! source: accept
//! why: resolving locations with the permitted set the config yields
/// Accept: the config decides which detectors run.
fn sweep_accept_resolve(config: &ScanConfig, env: &crate::locations::Environment) -> usize {
    let registry = crate::locations::Registry::with_builtins();
    let permitted = crate::locations::permitted::PermittedDetectors::from_config(config, &registry);
    registry.resolve(env, &permitted).len()
}
