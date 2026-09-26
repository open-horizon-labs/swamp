//! pyenv: `PYENV_ROOT` (default `~/.pyenv`), holding `versions/`,
//! `cache/` (downloaded Python source tarballs used by the build), and
//! `plugins/`. https://github.com/pyenv/pyenv

use super::{
    ConventionRole, Detector, Environment, InstalledVersionLayout, InstalledVersionNaming,
    LocationStatus, ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost,
    RecoveryHint, StorageCategory,
};

pub const PYENV_DETECTOR_ID: &str = "pyenv";

pub struct PyenvDetector;

impl Detector for PyenvDetector {
    fn id(&self) -> &'static str {
        PYENV_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "pyenv"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "pyenv README, current stable PYENV_ROOT layout"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("python"),
            role: ConventionRole::DeclaredVersions {
                declaration_files: &[".python-version"],
                layout: InstalledVersionLayout::VersionPerEntry,
                naming: InstalledVersionNaming::AsDeclared,
                global_default: None,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "pyenv install <version>",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("PYENV_ROOT") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("PYENV_ROOT".to_string()),
            ),
            _ => (env.home.join(".pyenv"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: PYENV_DETECTOR_ID.to_string(),
                path: Some(base.join("versions")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed Python versions".to_string()),
            },
            ProposedLocation {
                detector_id: PYENV_DETECTOR_ID.to_string(),
                path: Some(base.join("cache")),
                category: StorageCategory::Downloads,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("downloaded source tarballs used to build versions".to_string()),
            },
            ProposedLocation {
                detector_id: PYENV_DETECTOR_ID.to_string(),
                path: Some(base.join("plugins")),
                category: StorageCategory::Installation,
                provenance,
                status: LocationStatus::Resolved,
                note: Some("plugin checkouts".to_string()),
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
        let got = PyenvDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/.pyenv/versions"))
        );
        assert_eq!(got[0].category, StorageCategory::Installation);
        assert_eq!(got[1].category, StorageCategory::Downloads);
    }

    #[test]
    fn env_var_override_wins() {
        let mut env_vars = HashMap::new();
        env_vars.insert("PYENV_ROOT".to_string(), "/opt/pyenv".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = PyenvDetector.detect(&env);
        assert!(
            got.iter()
                .all(|l| l.path.as_ref().unwrap().starts_with("/opt/pyenv"))
        );
    }

    #[test]
    fn leftovers_found_without_pyenv_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = PyenvDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
