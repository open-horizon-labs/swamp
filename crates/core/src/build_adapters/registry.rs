//! Static registration for build-artifact adapters, mirroring
//! [`crate::agents::registry`] and [`crate::locations::Registry`]
//! exactly (`.oh/guardrails/build-adapters-are-pluggable.md`).
//!
//! Before this module the build side had exactly one adapter and one
//! named call to it: `consumers/cargo.rs` invoked
//! `cargo_artifacts::folded_units` directly. That is the shape
//! `agents/mod.rs` started with too, and by the time anyone looked it
//! was a fourteen-arm `match tool_id` with a second copy in
//! `actions.rs`. Registering the first adapter before adding the second
//! is the whole repair.

use super::BuildAdapter;
use super::{android, cargo, docker_buildkit, go, gradle, maven, node, python, xcode_swift};

pub struct Registry {
    adapters: Vec<Box<dyn BuildAdapter>>,
}

impl Registry {
    /// Order matters and is fixed here: when two adapters could both
    /// claim a directory -- a Gradle project that is also a Node
    /// workspace, both holding a `build/` -- the earlier adapter wins,
    /// every pass, rather than whichever one the iteration happened to
    /// reach first.
    pub fn with_builtins() -> Self {
        Self {
            adapters: vec![
                Box::new(cargo::Adapter),
                Box::new(node::Adapter),
                // Before Gradle: an Android module's `build/` is a Gradle
                // build directory with the Android plugin's layout in it,
                // and the module manifest decides which adapter claims it.
                Box::new(android::Adapter),
                Box::new(gradle::Adapter),
                Box::new(maven::Adapter),
                Box::new(python::Adapter),
                Box::new(go::Adapter),
                Box::new(xcode_swift::Adapter),
                Box::new(docker_buildkit::Adapter),
            ],
        }
    }

    pub fn adapters(&self) -> &[Box<dyn BuildAdapter>] {
        &self.adapters
    }

    pub fn get(&self, id: &str) -> Option<&dyn BuildAdapter> {
        self.adapters
            .iter()
            .find(|a| a.id() == id)
            .map(|a| a.as_ref())
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|a| a.id()).collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_adapters::matrix;

    #[test]
    fn every_adapter_is_registered_exactly_once() {
        let mut ids = Registry::with_builtins().ids();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len(), "a duplicate registration: {ids:?}");
    }

    #[test]
    fn registry_ids_are_exactly_the_matrix_implemented_ids() {
        // The matrix is the published support claim; the registry is
        // what runs. A family in one and not the other is either an
        // undocumented adapter or a documented family with no code.
        let mut registered = Registry::with_builtins().ids();
        registered.sort_unstable();
        let mut documented = matrix::implemented_ids();
        documented.sort_unstable();
        assert_eq!(registered, documented);
    }

    #[test]
    fn action_claims_have_explicit_role_contracts() {
        for a in Registry::with_builtins().adapters() {
            assert_eq!(
                a.capabilities().actions_available,
                !a.trash_roles().is_empty(),
                "{}",
                a.id()
            );
        }
    }
}
