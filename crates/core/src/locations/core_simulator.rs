//! CoreSimulator: `~/Library/Developer/CoreSimulator/{Devices,Caches}`
//! (per-user), plus simulator runtime images, which can live in either
//! of two documented locations depending on how they were installed:
//! `~/Library/Developer/CoreSimulator/Profiles/Runtimes` (per-user,
//! Xcode-managed) or `/Library/Developer/CoreSimulator/Volumes`
//! (system-wide, APFS volumes mounted by `runtimed` -- typically
//! requiring elevated access this detector never attempts to gain).
//!
//! `Devices/` holds mutable, per-simulator instance state (installed
//! apps, user data) -- an "environment" a developer boots into, not a
//! build output and not a pure cache. `Caches/` is CoreSimulator's own
//! cache. A permission gap on the system-wide runtime volumes is
//! reported explicitly through ordinary scope resolution (`RootStatus::
//! Unreadable`), never silently treated as "not present".

use super::{
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};

pub const CORE_SIMULATOR_DETECTOR_ID: &str = "core-simulator";

pub struct CoreSimulatorDetector;

impl Detector for CoreSimulatorDetector {
    fn id(&self) -> &'static str {
        CORE_SIMULATOR_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "CoreSimulator"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "CoreSimulator layout, current stable (system-wide runtime volumes may be permission-gated)"
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[
            BuildStoreDecl {
                kind: BuildStoreKind::SimulatorDevices,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Environments,
                    suffix: &[],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::SimulatorCaches,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Cache,
                    suffix: &[],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::SimulatorRuntimes,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Installation,
                    suffix: &[],
                },
            },
        ]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let base = env.home.join("Library/Developer/CoreSimulator");
        vec![
            ProposedLocation {
                detector_id: CORE_SIMULATOR_DETECTOR_ID.to_string(),
                path: Some(base.join("Devices")),
                category: StorageCategory::Environments,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "mutable per-simulator instance data (installed apps, user data)".to_string(),
                ),
            },
            ProposedLocation {
                detector_id: CORE_SIMULATOR_DETECTOR_ID.to_string(),
                path: Some(base.join("Caches")),
                category: StorageCategory::Cache,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("CoreSimulator's own cache".to_string()),
            },
            ProposedLocation {
                detector_id: CORE_SIMULATOR_DETECTOR_ID.to_string(),
                path: Some(base.join("Profiles/Runtimes")),
                category: StorageCategory::Installation,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "installed simulator OS runtimes (per-user); shared across every \
                     project targeting that OS version, not owned by any one of them"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: CORE_SIMULATOR_DETECTOR_ID.to_string(),
                path: Some(std::path::PathBuf::from(
                    "/Library/Developer/CoreSimulator/Volumes",
                )),
                category: StorageCategory::Installation,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "installed simulator OS runtimes as system-wide APFS volumes; may be \
                     permission-gated (RootStatus::Unreadable), not tied to any home directory"
                        .to_string(),
                ),
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
        let got = CoreSimulatorDetector.detect(&env);
        assert!(got.iter().any(|l| l.path
            == Some(PathBuf::from(
                "/Users/dev/Library/Developer/CoreSimulator/Devices"
            ))));
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Library/Developer/CoreSimulator/Volumes")))
        );
    }

    #[test]
    fn devices_is_environments_not_build_output_or_cache() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CoreSimulatorDetector.detect(&env);
        let devices = got
            .iter()
            .find(|l| {
                l.path
                    == Some(PathBuf::from(
                        "/Users/dev/Library/Developer/CoreSimulator/Devices",
                    ))
            })
            .unwrap();
        assert_eq!(devices.category, StorageCategory::Environments);
    }

    #[test]
    fn leftovers_found_without_simctl_installed() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CoreSimulatorDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
