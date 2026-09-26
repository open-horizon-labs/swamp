//! nvm: `NVM_DIR` (default `~/.nvm`), holding `versions/node/` and
//! `.cache/`. https://github.com/nvm-sh/nvm
//!
//! Never sources `nvm.sh` (the shell function that makes `nvm` a shell
//! builtin) -- only the documented `NVM_DIR` env var / convention path.

use super::{
    ConventionRole, Detector, Environment, InstalledVersionLayout, InstalledVersionNaming,
    LocationStatus, ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost,
    RecoveryHint, StorageCategory,
};

pub const NVM_DETECTOR_ID: &str = "nvm";

pub struct NvmDetector;

impl Detector for NvmDetector {
    fn id(&self) -> &'static str {
        NVM_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "nvm"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "nvm README, current stable NVM_DIR layout"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("nodejs"),
            role: ConventionRole::DeclaredVersions {
                declaration_files: &[".nvmrc", ".node-version"],
                layout: InstalledVersionLayout::VersionPerEntry,
                naming: InstalledVersionNaming::AsDeclared,
                global_default: None,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "nvm install <version>",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("NVM_DIR") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("NVM_DIR".to_string()),
            ),
            _ => (env.home.join(".nvm"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: NVM_DETECTOR_ID.to_string(),
                path: Some(base.join("versions/node")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed Node.js versions".to_string()),
            },
            ProposedLocation {
                detector_id: NVM_DETECTOR_ID.to_string(),
                path: Some(base.join(".cache")),
                category: StorageCategory::Cache,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("download cache".to_string()),
            },
            ProposedLocation {
                detector_id: NVM_DETECTOR_ID.to_string(),
                path: Some(base),
                category: StorageCategory::LocalState,
                provenance,
                status: LocationStatus::Resolved,
                note: Some("nvm.sh and misc. state".to_string()),
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
        let got = NvmDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.nvm/versions/node")))
        );
    }

    #[test]
    fn env_var_override_wins() {
        let mut env_vars = HashMap::new();
        env_vars.insert("NVM_DIR".to_string(), "/opt/nvm".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = NvmDetector.detect(&env);
        assert!(
            got.iter()
                .all(|l| l.path.as_ref().unwrap().starts_with("/opt/nvm"))
        );
    }

    #[test]
    fn leftovers_found_without_nvm_shell_function_sourced() {
        // No shell init is ever run by this detector; a leftover ~/.nvm
        // is still found purely from the convention path/env var.
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = NvmDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
