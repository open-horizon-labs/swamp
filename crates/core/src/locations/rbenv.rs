//! rbenv: `RBENV_ROOT` (default `~/.rbenv`), holding `versions/`,
//! `cache/`, `plugins/`, and `shims/`. https://github.com/rbenv/rbenv
//!
//! Never loads rbenv's shell init function (`rbenv()` in bash/zsh) or
//! runs `rbenv init` -- the root is read purely from the documented env
//! var/convention.

use super::{
    ConventionRole, Detector, Environment, InstalledVersionLayout, InstalledVersionNaming,
    LocationStatus, ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost,
    RecoveryHint, StorageCategory,
};

pub const RBENV_DETECTOR_ID: &str = "rbenv";

pub struct RbenvDetector;

impl Detector for RbenvDetector {
    fn id(&self) -> &'static str {
        RBENV_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "rbenv"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "rbenv README, current stable RBENV_ROOT layout"
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
            command: "rbenv install <version>",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("RBENV_ROOT") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("RBENV_ROOT".to_string()),
            ),
            _ => (env.home.join(".rbenv"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: RBENV_DETECTOR_ID.to_string(),
                path: Some(base.join("versions")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed Ruby versions".to_string()),
            },
            ProposedLocation {
                detector_id: RBENV_DETECTOR_ID.to_string(),
                path: Some(base.join("cache")),
                category: StorageCategory::Downloads,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("downloaded build sources".to_string()),
            },
            ProposedLocation {
                detector_id: RBENV_DETECTOR_ID.to_string(),
                path: Some(base.join("plugins")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("plugin checkouts (e.g. ruby-build)".to_string()),
            },
            ProposedLocation {
                detector_id: RBENV_DETECTOR_ID.to_string(),
                path: Some(base.join("shims")),
                category: StorageCategory::LocalState,
                provenance,
                status: LocationStatus::Resolved,
                note: Some("generated shim executables".to_string()),
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
        let got = RbenvDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.rbenv/versions")))
        );
    }

    #[test]
    fn env_var_override_redirects_every_subdirectory() {
        let mut env_vars = HashMap::new();
        env_vars.insert("RBENV_ROOT".to_string(), "/opt/rbenv".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = RbenvDetector.detect(&env);
        assert!(
            got.iter()
                .all(|l| l.path.as_ref().unwrap().starts_with("/opt/rbenv"))
        );
    }

    #[test]
    fn leftovers_found_without_rbenv_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = RbenvDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
