//! RVM: `rvm_path` (lowercase -- this is RVM's own documented variable
//! name, not a typo) or `~/.rvm`, holding `rubies/` (installed
//! interpreters), `gems/` (gemsets -- per-Ruby package environments),
//! `archives/` (downloaded source tarballs), and `src/` (extracted
//! sources used to build a ruby). https://rvm.io

use super::{
    ConventionRole, Detector, Environment, InstalledVersionLayout, InstalledVersionNaming,
    LocationStatus, ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost,
    RecoveryHint, StorageCategory,
};

pub const RVM_DETECTOR_ID: &str = "rvm";

pub struct RvmDetector;

impl Detector for RvmDetector {
    fn id(&self) -> &'static str {
        RVM_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "RVM"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "rvm.io, current stable rvm_path layout"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("ruby"),
            role: ConventionRole::DeclaredVersions {
                declaration_files: &[".ruby-version"],
                layout: InstalledVersionLayout::VersionPerEntry,
                naming: InstalledVersionNaming::AsDeclared,
                global_default: None,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "rvm install <version>",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("rvm_path") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("rvm_path".to_string()),
            ),
            _ => (env.home.join(".rvm"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: RVM_DETECTOR_ID.to_string(),
                path: Some(base.join("rubies")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed Ruby interpreters".to_string()),
            },
            ProposedLocation {
                detector_id: RVM_DETECTOR_ID.to_string(),
                path: Some(base.join("gems")),
                category: StorageCategory::Environments,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("gemsets: per-Ruby package environments".to_string()),
            },
            ProposedLocation {
                detector_id: RVM_DETECTOR_ID.to_string(),
                path: Some(base.join("archives")),
                category: StorageCategory::Downloads,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("downloaded source archives".to_string()),
            },
            ProposedLocation {
                detector_id: RVM_DETECTOR_ID.to_string(),
                path: Some(base.join("src")),
                category: StorageCategory::Downloads,
                provenance,
                status: LocationStatus::Resolved,
                note: Some("extracted source used to build a ruby".to_string()),
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
        let got = RvmDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.rvm/rubies")))
        );
        assert!(
            got.iter()
                .any(|l| l.category == StorageCategory::Environments
                    && l.path == Some(PathBuf::from("/Users/dev/.rvm/gems")))
        );
    }

    #[test]
    fn lowercase_env_var_override_wins() {
        let mut env_vars = HashMap::new();
        env_vars.insert("rvm_path".to_string(), "/opt/rvm".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = RvmDetector.detect(&env);
        assert!(
            got.iter()
                .all(|l| l.path.as_ref().unwrap().starts_with("/opt/rvm"))
        );
    }

    #[test]
    fn leftovers_found_without_rvm_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = RvmDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
