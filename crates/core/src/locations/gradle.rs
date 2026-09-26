//! Gradle: `GRADLE_USER_HOME` (default `~/.gradle`), holding `caches/`
//! (downloaded dependency artifacts and build cache), `wrapper/dists/`
//! (extracted Gradle distributions the wrapper downloaded to run a
//! specific version -- effectively installed runtimes), `daemon/`
//! (daemon registry and logs), and `native/` (native library extraction
//! cache). https://docs.gradle.org/current/userguide/directory_layout.html

use super::{
    BuildStoreDecl, BuildStoreKind, ConventionRole, Detector, Environment, LocationStatus,
    ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost, RecoveryHint,
    StorageCategory, StoreAnchor, StoreEntryLookup,
};

pub const GRADLE_DETECTOR_ID: &str = "gradle";

pub struct GradleDetector;

impl Detector for GradleDetector {
    fn id(&self) -> &'static str {
        GRADLE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Gradle"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "Gradle directory layout reference, current stable"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("gradle"),
            role: ConventionRole::DependencyStore {
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Cache,
                    suffix: &["caches"],
                },
                lookup: StoreEntryLookup::GradleModules,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "gradle build",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[
            BuildStoreDecl {
                kind: BuildStoreKind::GradleCaches,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Cache,
                    suffix: &["caches"],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::GradleWrapperDists,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Installation,
                    suffix: &["wrapper", "dists"],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::GradleDaemon,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::LocalState,
                    suffix: &["daemon"],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::GradleNative,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Cache,
                    suffix: &["native"],
                },
            },
        ]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("GRADLE_USER_HOME") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("GRADLE_USER_HOME".to_string()),
            ),
            _ => (env.home.join(".gradle"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: GRADLE_DETECTOR_ID.to_string(),
                path: Some(base.join("caches")),
                category: StorageCategory::Cache,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("downloaded dependency artifacts and build cache".to_string()),
            },
            ProposedLocation {
                detector_id: GRADLE_DETECTOR_ID.to_string(),
                path: Some(base.join("wrapper/dists")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("extracted Gradle distributions the wrapper downloaded".to_string()),
            },
            ProposedLocation {
                detector_id: GRADLE_DETECTOR_ID.to_string(),
                path: Some(base.join("daemon")),
                category: StorageCategory::LocalState,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("daemon registry and logs".to_string()),
            },
            ProposedLocation {
                detector_id: GRADLE_DETECTOR_ID.to_string(),
                path: Some(base.join("native")),
                category: StorageCategory::Cache,
                provenance,
                status: LocationStatus::Resolved,
                note: Some("native library extraction cache".to_string()),
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
    fn convention_when_no_env_override() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = GradleDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.gradle/caches")))
        );
        assert!(got.iter().any(|l| l.path
            == Some(PathBuf::from("/Users/dev/.gradle/wrapper/dists"))
            && l.category == StorageCategory::Installation));
    }

    #[test]
    fn env_var_override_redirects_every_subdirectory() {
        let mut env_vars = HashMap::new();
        env_vars.insert("GRADLE_USER_HOME".to_string(), "/opt/gradle".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = GradleDetector.detect(&env);
        assert!(
            got.iter()
                .all(|l| l.path.as_ref().unwrap().starts_with("/opt/gradle"))
        );
    }

    #[test]
    fn leftovers_found_without_gradle_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = GradleDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
