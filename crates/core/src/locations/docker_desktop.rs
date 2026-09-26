//! Docker Desktop's host-filesystem VM backing store: the directory
//! containing `Docker.raw` (or `Docker.qcow2` on an older/Intel install)
//! at `~/Library/Containers/com.docker.docker/Data/vms/0/data/`.
//! https://docs.docker.com/desktop/troubleshoot-and-support/faqs/macfaqs/
//!
//! This is a completely different view from `crate::docker`'s existing
//! logical-object accounting (images/containers/volumes reported via the
//! Docker API/CLI): that view sees *what Docker thinks it stores*: this
//! one sees *the host file backing the whole VM*, which is one sparse
//! disk image and can be smaller (or, mid-write, momentarily different)
//! than the sum of logical objects. Nothing in this crate ever adds the
//! two together -- see `docs/architecture.md` and `docs/locations.md`.
//! The backing file is proposed as one *directory* location (its parent,
//! not the file itself): `walk::resize_artifact`'s existing per-file
//! `st_blocks * 512` accounting already reports a sparse file's
//! *allocated* bytes, not its much larger apparent/virtual size, with no
//! special-casing needed here.
//!
//! Docker Desktop lets a user relocate this disk image (Settings ->
//! Resources -> Advanced -> "Disk image location"); this detector does
//! not currently read that preference (no reliably documented,
//! version-stable settings-file field was found) -- a custom location is
//! a named, honest gap here and in `docs/locations.md`, not a guess.
//!
//! OrbStack's data directory (`~/.orbstack/data`) is a broadly similar
//! "VM/container backing data" concept, proposed the same conventional
//! way; OrbStack does not use one single sparse disk image the way
//! Docker Desktop does, so no allocated-vs-apparent distinction applies
//! there beyond the ordinary directory walk.

use super::{
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};

pub const DOCKER_DESKTOP_DETECTOR_ID: &str = "docker-desktop";

pub struct DockerDesktopDetector;

impl Detector for DockerDesktopDetector {
    fn id(&self) -> &'static str {
        DOCKER_DESKTOP_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Docker Desktop / OrbStack host backing storage"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "Docker Desktop Mac FAQ, current stable default disk-image location \
         (a relocated disk image is not read; see this module's doc comment)"
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[BuildStoreDecl {
            kind: BuildStoreKind::BuildKitCache,
            anchor: StoreAnchor::SoleLocation,
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        vec![
            ProposedLocation {
                detector_id: DOCKER_DESKTOP_DETECTOR_ID.to_string(),
                path: Some(
                    env.home
                        .join("Library/Containers/com.docker.docker/Data/vms/0/data"),
                ),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "directory holding the sparse VM disk image (Docker.raw/Docker.qcow2); \
                     measured bytes are allocated (actual disk blocks), not the much \
                     larger apparent/virtual disk size -- never summed with crate::docker's \
                     logical-object view. A relocated disk image (Settings > Resources > \
                     Advanced) is not read; see docs/locations.md."
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: DOCKER_DESKTOP_DETECTOR_ID.to_string(),
                path: Some(env.home.join(".orbstack/data")),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("OrbStack's VM/container/image data directory".to_string()),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn convention_paths() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = DockerDesktopDetector.detect(&env);
        assert!(got.iter().any(|l| l.path
            == Some(PathBuf::from(
                "/Users/dev/Library/Containers/com.docker.docker/Data/vms/0/data"
            ))));
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.orbstack/data")))
        );
    }

    #[test]
    fn leftovers_found_without_docker_desktop_or_orbstack_running() {
        // Neither location's existence is checked by this detector at
        // all (that is scope resolution's job) -- both are still
        // proposed even when neither app is installed/running.
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = DockerDesktopDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
