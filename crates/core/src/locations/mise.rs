//! mise: `MISE_DATA_DIR` (default `~/.local/share/mise` -- *not*
//! `~/.mise`, an older/other-tool convention this detector must not
//! guess its way into), `MISE_CACHE_DIR` (default `~/.cache/mise`), and
//! `MISE_CONFIG_DIR` (default `~/.config/mise`).
//! https://mise.jdx.dev/directories.html
//!
//! The data dir's own installs/downloads/plugins/shims subdirectories
//! are proposed as their own categorized locations; `crate::scope`'s
//! nested-folding plus the external-unit prune (see `crate::external`
//! and `report::report_scope_with_source`) mean proposing both the base
//! dir and its named children never double-counts their bytes.

use super::{
    ConventionRole, Detector, Environment, InstalledVersionLayout, InstalledVersionNaming,
    LocationStatus, ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost,
    RecoveryHint, StorageCategory,
};

pub const MISE_DETECTOR_ID: &str = "mise";

pub struct MiseDetector;

fn base_dir(env: &Environment, var: &str, default_rel: &str) -> (std::path::PathBuf, Provenance) {
    match env.env_var(var) {
        Some(v) if !v.is_empty() => (
            std::path::PathBuf::from(v),
            Provenance::EnvVar(var.to_string()),
        ),
        _ => (env.home.join(default_rel), Provenance::BuiltinConvention),
    }
}

impl Detector for MiseDetector {
    fn id(&self) -> &'static str {
        MISE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "mise"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "mise directories reference, current stable (data dir is ~/.local/share/mise, not ~/.mise)"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            // mise reads asdf's `.tool-versions` as well as its own
            // config, and both name the tool themselves.
            tool: None,
            role: ConventionRole::DeclaredVersions {
                declaration_files: &[".tool-versions", ".mise.toml", "mise.toml"],
                layout: InstalledVersionLayout::ToolThenVersion,
                naming: InstalledVersionNaming::AsDeclared,
                global_default: None,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "mise install",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (data, data_prov) = base_dir(env, "MISE_DATA_DIR", ".local/share/mise");
        let (cache, cache_prov) = base_dir(env, "MISE_CACHE_DIR", ".cache/mise");
        let (config, config_prov) = base_dir(env, "MISE_CONFIG_DIR", ".config/mise");

        vec![
            ProposedLocation {
                detector_id: MISE_DETECTOR_ID.to_string(),
                path: Some(data.clone()),
                category: StorageCategory::LocalState,
                provenance: data_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some("mise data dir (state.db and misc. state)".to_string()),
            },
            ProposedLocation {
                detector_id: MISE_DETECTOR_ID.to_string(),
                path: Some(data.join("installs")),
                category: StorageCategory::Installation,
                provenance: data_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some("installed tool/runtime versions".to_string()),
            },
            ProposedLocation {
                detector_id: MISE_DETECTOR_ID.to_string(),
                path: Some(data.join("downloads")),
                category: StorageCategory::Downloads,
                provenance: data_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some("downloaded install archives".to_string()),
            },
            ProposedLocation {
                detector_id: MISE_DETECTOR_ID.to_string(),
                path: Some(data.join("plugins")),
                category: StorageCategory::Installation,
                provenance: data_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some("asdf-compatible plugin checkouts".to_string()),
            },
            ProposedLocation {
                detector_id: MISE_DETECTOR_ID.to_string(),
                path: Some(data.join("shims")),
                category: StorageCategory::LocalState,
                provenance: data_prov,
                status: LocationStatus::Resolved,
                note: Some("generated shim executables".to_string()),
            },
            ProposedLocation {
                detector_id: MISE_DETECTOR_ID.to_string(),
                path: Some(cache),
                category: StorageCategory::Cache,
                provenance: cache_prov,
                status: LocationStatus::Resolved,
                note: Some("download/build cache".to_string()),
            },
            ProposedLocation {
                detector_id: MISE_DETECTOR_ID.to_string(),
                path: Some(config),
                category: StorageCategory::LocalState,
                provenance: config_prov,
                status: LocationStatus::Resolved,
                note: Some("mise configuration".to_string()),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    #[test]
    fn convention_data_dir_is_xdg_share_not_dot_mise() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = MiseDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/.local/share/mise")),
            "mise's normal Unix data root is ~/.local/share/mise, never ~/.mise"
        );
        assert!(
            !got.iter().any(|l| l
                .path
                .as_ref()
                .is_some_and(|p| p == Path::new("/Users/dev/.mise"))),
            "must never propose the wrong ~/.mise convention"
        );
    }

    #[test]
    fn installs_downloads_plugins_shims_are_categorized_distinctly() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = MiseDetector.detect(&env);
        let cat = |rel: &str| {
            got.iter()
                .find(|l| {
                    l.path.as_ref().is_some_and(|p| {
                        p == &PathBuf::from("/Users/dev/.local/share/mise").join(rel)
                    })
                })
                .map(|l| l.category)
        };
        assert_eq!(cat("installs"), Some(StorageCategory::Installation));
        assert_eq!(cat("downloads"), Some(StorageCategory::Downloads));
        assert_eq!(cat("plugins"), Some(StorageCategory::Installation));
        assert_eq!(cat("shims"), Some(StorageCategory::LocalState));
    }

    #[test]
    fn env_overrides_each_independently() {
        let mut env_vars = HashMap::new();
        env_vars.insert("MISE_DATA_DIR".to_string(), "/opt/mise-data".to_string());
        env_vars.insert("MISE_CACHE_DIR".to_string(), "/opt/mise-cache".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = MiseDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/mise-data")));
        assert_eq!(
            got.iter()
                .find(|l| l.category == StorageCategory::Cache)
                .unwrap()
                .path,
            Some(PathBuf::from("/opt/mise-cache"))
        );
        // MISE_CONFIG_DIR was not overridden: still the convention path.
        assert_eq!(
            got.iter()
                .find(|l| l.category == StorageCategory::LocalState
                    && l.note.as_deref() == Some("mise configuration"))
                .unwrap()
                .path,
            Some(PathBuf::from("/Users/dev/.config/mise"))
        );
    }

    #[test]
    fn leftovers_found_without_mise_executable() {
        // No PATH/executable check exists in this detector at all --
        // every location is proposed purely from convention/env, so a
        // leftover data dir is still found after `mise` is uninstalled.
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = MiseDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
