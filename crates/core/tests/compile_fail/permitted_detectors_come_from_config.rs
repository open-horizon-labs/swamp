//! discovery-consumes-effective-scope, explicit-only-scope-when-defaults-false:
//! resolving detector locations takes `PermittedDetectors`, built only from
//! the scan config (which applies `defaults = false`). Resolving without it
//! does not compile.
use swamp_core::locations::{Environment, Registry};

fn run_every_detector(registry: &Registry, env: &Environment) {
    let _ = registry.resolve(env);
}

fn main() {}
