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
//!
//! # The system-wide siblings, and who owns the simulator bytes
//!
//! Beside `Volumes/` the system-wide `/Library/Developer/CoreSimulator`
//! also holds `Caches/` (`cache`) and `Images/`, `Cryptex/`, `Profiles/`
//! (`local-state`: how the runtimes are mounted and which device types
//! exist). Each is measured as its own unit.
//!
//! The mounted runtime volumes under `Volumes/` are backed by disk
//! images under `/System/Library/AssetsV2`, and `/System` is firmlinked
//! onto the data volume, so the same runtime is reachable by two paths.
//! **`Volumes/` owns it and `AssetsV2` is never proposed**: `Volumes/`
//! is the path Xcode and `simctl` present, the one this catalog has
//! always reported and keyed history on, and the one the runtime
//! adapter identifies (`iOS_23F77`) by name; `AssetsV2` is an
//! OS-managed system location holding every kind of MobileAsset, not
//! only simulators. The two numbers differ (measured on the reporter's
//! machine: about 39.9 GB through `Volumes/`, about 24.9 GB of `.dmg`
//! files in `AssetsV2`, because a mounted image is measured by the
//! files inside it) and are never summed.
//! `crate::locations::tests::the_simulator_runtimes_are_counted_through_one_path`
//! fails if both paths are ever proposed.

use std::path::PathBuf;

use super::{
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor, ToolManagedLocation, ToolManager,
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

    /// Runtime images are removed by `simctl runtime delete` (#177):
    /// the system-wide mounted volumes and the per-user runtime bundles.
    fn tool_managed(&self) -> &'static [ToolManagedLocation] {
        &[
            ToolManagedLocation {
                suffix: "CoreSimulator/Volumes",
                manager: ToolManager::Simulator,
            },
            ToolManagedLocation {
                suffix: "CoreSimulator/Profiles/Runtimes",
                manager: ToolManager::Simulator,
            },
        ]
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
            BuildStoreDecl {
                kind: BuildStoreKind::SimulatorSystemSupport,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::LocalState,
                    suffix: &[],
                },
            },
        ]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let base = env.home.join("Library/Developer/CoreSimulator");
        let system = PathBuf::from("/Library/Developer/CoreSimulator");
        let mut out = vec![
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
                path: Some(system.join("Volumes")),
                category: StorageCategory::Installation,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "installed simulator OS runtimes as system-wide APFS volumes; may be \
                     permission-gated (RootStatus::Unreadable), not tied to any home directory"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: CORE_SIMULATOR_DETECTOR_ID.to_string(),
                path: Some(system.join("Caches")),
                category: StorageCategory::Cache,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("CoreSimulator's own cache, system-wide".to_string()),
            },
        ];
        for (folder, note) in [
            (
                "Images",
                "simulator runtime disk-image bookkeeping (images.plist, mount points), system-wide",
            ),
            (
                "Cryptex",
                "simulator runtime cryptex images and caches, system-wide",
            ),
            ("Profiles", "simulator device-type profiles, system-wide"),
        ] {
            out.push(ProposedLocation {
                detector_id: CORE_SIMULATOR_DETECTOR_ID.to_string(),
                path: Some(system.join(folder)),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(note.to_string()),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

    #[test]
    fn system_wide_siblings_are_proposed_with_their_own_kinds() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CoreSimulatorDetector.detect(&env);
        let kind_of = |p: &str| {
            got.iter()
                .find(|l| l.path == Some(PathBuf::from(p)))
                .unwrap_or_else(|| panic!("{p} is not proposed"))
                .category
        };
        assert_eq!(
            kind_of("/Library/Developer/CoreSimulator/Caches"),
            StorageCategory::Cache
        );
        for folder in ["Images", "Cryptex", "Profiles"] {
            assert_eq!(
                kind_of(&format!("/Library/Developer/CoreSimulator/{folder}")),
                StorageCategory::LocalState
            );
        }
        assert_eq!(
            kind_of("/Library/Developer/CoreSimulator/Volumes"),
            StorageCategory::Installation
        );
    }

    #[test]
    fn asset_images_are_never_proposed_because_volumes_owns_the_runtimes() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        assert!(CoreSimulatorDetector.detect(&env).iter().all(|l| {
            !l.path
                .as_ref()
                .unwrap()
                .starts_with("/System/Library/AssetsV2")
        }));
    }
}
