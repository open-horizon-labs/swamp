//! asdf: `ASDF_DATA_DIR` (default `~/.asdf`), holding `installs/`,
//! `downloads/`, `plugins/`, and `shims/`.
//! https://asdf-vm.com/guide/getting-started.html

use super::{
    ConventionRole, Detector, Environment, InstalledVersionLayout, InstalledVersionNaming,
    LocationStatus, ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost,
    RecoveryHint, StorageCategory,
};

pub const ASDF_DETECTOR_ID: &str = "asdf";

pub struct AsdfDetector;

impl Detector for AsdfDetector {
    fn id(&self) -> &'static str {
        ASDF_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "asdf"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "asdf getting-started guide, current stable data dir layout"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            // `.tool-versions` names the tool on each line, so this
            // convention answers for whichever tool is declared there.
            tool: None,
            role: ConventionRole::DeclaredVersions {
                declaration_files: &[".tool-versions"],
                layout: InstalledVersionLayout::ToolThenVersion,
                naming: InstalledVersionNaming::AsDeclared,
                global_default: None,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "asdf install <tool> <version>",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("ASDF_DATA_DIR") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("ASDF_DATA_DIR".to_string()),
            ),
            _ => (env.home.join(".asdf"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: ASDF_DETECTOR_ID.to_string(),
                path: Some(base.join("installs")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed runtime versions".to_string()),
            },
            ProposedLocation {
                detector_id: ASDF_DETECTOR_ID.to_string(),
                path: Some(base.join("downloads")),
                category: StorageCategory::Downloads,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("downloaded install archives".to_string()),
            },
            ProposedLocation {
                detector_id: ASDF_DETECTOR_ID.to_string(),
                path: Some(base.join("plugins")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("plugin checkouts".to_string()),
            },
            ProposedLocation {
                detector_id: ASDF_DETECTOR_ID.to_string(),
                path: Some(base.join("shims")),
                category: StorageCategory::LocalState,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("generated shim executables".to_string()),
            },
            ProposedLocation {
                detector_id: ASDF_DETECTOR_ID.to_string(),
                path: Some(base),
                category: StorageCategory::LocalState,
                provenance,
                status: LocationStatus::Resolved,
                note: Some("asdf data dir root (asdfrc, misc. state)".to_string()),
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
        let got = AsdfDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.asdf/installs")))
        );
        assert!(
            got.iter()
                .all(|l| matches!(l.provenance, Provenance::BuiltinConvention))
        );
    }

    #[test]
    fn env_var_override_redirects_every_subdirectory() {
        let mut env_vars = HashMap::new();
        env_vars.insert("ASDF_DATA_DIR".to_string(), "/opt/asdf".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = AsdfDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/opt/asdf/installs")))
        );
        assert!(
            !got.iter().any(|l| l
                .path
                .as_ref()
                .is_some_and(|p| p.starts_with("/Users/dev/.asdf"))),
            "an override must redirect every subdirectory, not just add a second location"
        );
    }

    #[test]
    fn leftovers_found_without_asdf_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = AsdfDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
