//! rustup home: `RUSTUP_HOME` override, else `~/.rustup`. Extended
//! (#46) to distinguish `toolchains/` (installed, the bulk of real
//! disk use) from `downloads/` (in-progress/partial download staging)
//! from `tmp/` (rustup's own scratch space for extracting an update
//! before it is moved into place) -- three very different lifetimes
//! under the same root, previously collapsed into one "installation"
//! entry for the whole home.
//! https://rust-lang.github.io/rustup/environment-variables.html

use super::{
    ConventionRole, Detector, Environment, GlobalDefaultFile, GlobalDefaultFormat,
    InstalledVersionLayout, InstalledVersionNaming, LocationStatus, ManagerConvention, Platform,
    ProposedLocation, Provenance, RecoveryCost, RecoveryHint, StorageCategory,
};

pub const RUSTUP_DETECTOR_ID: &str = "rustup";

pub struct RustupDetector;

impl Detector for RustupDetector {
    fn id(&self) -> &'static str {
        RUSTUP_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "rustup"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "rustup environment-variables reference, current stable"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("rust"),
            role: ConventionRole::DeclaredVersions {
                declaration_files: &["rust-toolchain", "rust-toolchain.toml"],
                layout: InstalledVersionLayout::VersionPerEntry,
                // `toolchains/` entries are `<channel>-<host-triple>`,
                // never the bare `stable`/`1.82.0` a project pins.
                naming: InstalledVersionNaming::ChannelWithHostTriple,
                global_default: Some(GlobalDefaultFile {
                    file_name: "settings.toml",
                    field: "default_toolchain",
                    format: GlobalDefaultFormat::TomlTopLevelString,
                }),
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "rustup toolchain install <toolchain>",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("RUSTUP_HOME") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("RUSTUP_HOME".to_string()),
            ),
            _ => (env.home.join(".rustup"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: RUSTUP_DETECTOR_ID.to_string(),
                path: Some(base.clone()),
                category: StorageCategory::LocalState,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("settings.toml, update-hashes, and misc. state".to_string()),
            },
            ProposedLocation {
                detector_id: RUSTUP_DETECTOR_ID.to_string(),
                path: Some(base.join("toolchains")),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed toolchains and components".to_string()),
            },
            ProposedLocation {
                detector_id: RUSTUP_DETECTOR_ID.to_string(),
                path: Some(base.join("downloads")),
                category: StorageCategory::Downloads,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("in-progress/partial toolchain download staging".to_string()),
            },
            ProposedLocation {
                detector_id: RUSTUP_DETECTOR_ID.to_string(),
                path: Some(base.join("tmp")),
                category: StorageCategory::Cache,
                provenance,
                status: LocationStatus::Resolved,
                note: Some("scratch space for extracting an update before install".to_string()),
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
        let got = RustupDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.rustup")));
        assert!(matches!(got[0].provenance, Provenance::BuiltinConvention));
    }

    #[test]
    fn env_var_override_wins() {
        let mut env_vars = HashMap::new();
        env_vars.insert("RUSTUP_HOME".to_string(), "/opt/rustup-home".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = RustupDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/rustup-home")));
        assert_eq!(
            got[0].provenance,
            Provenance::EnvVar("RUSTUP_HOME".to_string())
        );
    }

    #[test]
    fn toolchains_downloads_and_tmp_are_categorized_distinctly() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = RustupDetector.detect(&env);
        let cat = |rel: &str| {
            got.iter()
                .find(|l| l.path == Some(PathBuf::from("/Users/dev/.rustup").join(rel)))
                .map(|l| l.category)
        };
        assert_eq!(cat("toolchains"), Some(StorageCategory::Installation));
        assert_eq!(cat("downloads"), Some(StorageCategory::Downloads));
        assert_eq!(cat("tmp"), Some(StorageCategory::Cache));
        assert_eq!(cat(""), Some(StorageCategory::LocalState));
    }
}
