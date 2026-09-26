//! Xcode: `~/Library/Developer/Xcode/{DerivedData,Archives,iOS
//! DeviceSupport,watchOS DeviceSupport,UserData}`. Not every descendant
//! of `~/Library/Developer` is a cache -- `DerivedData` is disposable
//! build output, `Archives` are distribution builds a developer may
//! keep deliberately, `*DeviceSupport` is shared symbol-file data
//! cached per connected device/OS version (not tied to any one
//! project), and `UserData` is Xcode's own UI/workspace state.
//!
//! `DerivedData`'s location can be customized via Xcode's
//! `IDECustomDerivedDataLocation` preference (Xcode's own "Custom"
//! DerivedData location setting). This is read through the bounded,
//! allow-listed `defaults read com.apple.dt.Xcode
//! IDECustomDerivedDataLocation` query -- never Xcode's own plist parsed
//! with a bespoke binary-plist reader, and never a project's own
//! `.xcodeproj`/`.xcworkspace` settings (a *global* default only).
//! A failed/absent query still leaves the conventional path proposed.

use super::{
    BuildStoreDecl, BuildStoreKind, CommandOutcome, ConventionRole, Detector, Environment,
    LocationStatus, ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost,
    RecoveryHint, StorageCategory, StoreAnchor,
};

pub const XCODE_DETECTOR_ID: &str = "xcode";

pub struct XcodeDetector;

impl Detector for XcodeDetector {
    fn id(&self) -> &'static str {
        XCODE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Xcode"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "Xcode component documentation, current stable ~/Library/Developer/Xcode layout"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("xcode"),
            role: ConventionRole::BuildOutputWorkspaceIndex {
                // `Archives` and any custom DerivedData location share
                // the BuildOutput category, so the suffix picks the one
                // whose subfolders carry an `info.plist` naming the
                // workspace that produced them.
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::BuildOutput,
                    suffix: &["DerivedData"],
                },
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "xcodebuild build",
            cost: RecoveryCost::LocalRebuild,
        })
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[
            BuildStoreDecl {
                kind: BuildStoreKind::XcodeDerivedData,
                anchor: StoreAnchor::CategorizedExcept {
                    category: StorageCategory::BuildOutput,
                    except: &[&["Archives"]],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::XcodeArchives,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::BuildOutput,
                    suffix: &["Archives"],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::XcodeDeviceSupport,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Cache,
                    suffix: &[],
                },
            },
        ]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let developer = env.home.join("Library/Developer/Xcode");
        let mut out = vec![
            ProposedLocation {
                detector_id: XCODE_DETECTOR_ID.to_string(),
                path: Some(developer.join("DerivedData")),
                category: StorageCategory::BuildOutput,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some("build output and module/index caches, per-project".to_string()),
            },
            ProposedLocation {
                detector_id: XCODE_DETECTOR_ID.to_string(),
                path: Some(developer.join("Archives")),
                category: StorageCategory::BuildOutput,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "archived distribution builds -- a developer may keep these deliberately, \
                     unlike DerivedData"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: XCODE_DETECTOR_ID.to_string(),
                path: Some(developer.join("iOS DeviceSupport")),
                category: StorageCategory::Cache,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "per-device/OS-version symbol files, shared across every project -- \
                     not owned by any one of them"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: XCODE_DETECTOR_ID.to_string(),
                path: Some(developer.join("watchOS DeviceSupport")),
                category: StorageCategory::Cache,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "per-device/OS-version symbol files, shared across projects".to_string(),
                ),
            },
            ProposedLocation {
                detector_id: XCODE_DETECTOR_ID.to_string(),
                path: Some(developer.join("UserData")),
                category: StorageCategory::LocalState,
                provenance: Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(
                    "Xcode's own UI/workspace state (breakpoints, window layout)".to_string(),
                ),
            },
        ];

        match env.run_command(
            "defaults",
            &["read", "com.apple.dt.Xcode", "IDECustomDerivedDataLocation"],
        ) {
            Ok(CommandOutcome {
                stdout,
                success: true,
            }) if !stdout.is_empty() => {
                out.push(ProposedLocation {
                    detector_id: XCODE_DETECTOR_ID.to_string(),
                    path: Some(std::path::PathBuf::from(stdout)),
                    category: StorageCategory::BuildOutput,
                    provenance: Provenance::ToolQuery(
                        "defaults read com.apple.dt.Xcode IDECustomDerivedDataLocation".to_string(),
                    ),
                    status: LocationStatus::Resolved,
                    note: Some(
                        "custom DerivedData location (IDECustomDerivedDataLocation)".to_string(),
                    ),
                });
            }
            Ok(CommandOutcome { success: false, .. }) | Err(_) => {
                // No custom preference set (or `defaults` unavailable);
                // the conventional DerivedData path above already covers
                // the common case, so this is not itself reported as an
                // unresolved entry the way Homebrew's tool query is --
                // "no custom location configured" is the overwhelmingly
                // common, expected outcome here, not a query failure.
            }
            Ok(_) => {}
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locations::FakeCommandRunner;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn convention_paths_cover_every_documented_subdirectory() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = XcodeDetector.detect(&env);
        for rel in [
            "DerivedData",
            "Archives",
            "iOS DeviceSupport",
            "watchOS DeviceSupport",
            "UserData",
        ] {
            assert!(
                got.iter().any(|l| l.path
                    == Some(PathBuf::from("/Users/dev/Library/Developer/Xcode").join(rel))),
                "missing {rel}"
            );
        }
    }

    #[test]
    fn derived_data_and_archives_are_build_output_not_a_blanket_cache() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = XcodeDetector.detect(&env);
        let cat = |rel: &str| {
            got.iter()
                .find(|l| {
                    l.path == Some(PathBuf::from("/Users/dev/Library/Developer/Xcode").join(rel))
                })
                .map(|l| l.category)
        };
        assert_eq!(cat("DerivedData"), Some(StorageCategory::BuildOutput));
        assert_eq!(cat("Archives"), Some(StorageCategory::BuildOutput));
        assert_eq!(cat("iOS DeviceSupport"), Some(StorageCategory::Cache));
        assert_eq!(cat("UserData"), Some(StorageCategory::LocalState));
    }

    #[test]
    fn custom_derived_data_location_from_defaults_read() {
        let fake = Arc::new(FakeCommandRunner::new().with_answer(
            "defaults",
            &["read", "com.apple.dt.Xcode", "IDECustomDerivedDataLocation"],
            "/Volumes/fast/DerivedData",
        ));
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS)
                .with_runner(fake.clone());
        let got = XcodeDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Volumes/fast/DerivedData")))
        );
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "defaults");
    }

    #[test]
    fn no_custom_location_still_proposes_the_convention_path() {
        // Default fixture Environment has no command runner wired in
        // (NullCommandRunner): the query fails, but DerivedData is still
        // proposed at its conventional path.
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = XcodeDetector.detect(&env);
        assert!(got.iter().any(|l| l.path
            == Some(PathBuf::from(
                "/Users/dev/Library/Developer/Xcode/DerivedData"
            ))));
    }

    #[test]
    fn leftovers_found_without_xcode_installed() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = XcodeDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
