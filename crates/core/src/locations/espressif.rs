//! ESP-IDF's tool directory: `IDF_TOOLS_PATH` if set, else
//! `~/.espressif` (Espressif's own `idf_tools.py` documents both).
//! Three folders under it are measured, each its own unit:
//!
//! - `dist/` -- the archives `idf_tools.py` downloaded (`cache`);
//! - `tools/` -- the installed toolchains and tools (`installation`);
//! - `python_env/` -- one Python virtual environment per ESP-IDF
//!   version (`environments`).
//!
//! The loose files beside them (`idf-env.json`, the
//! `espidf.constraints.*.txt` files) are a few kilobytes and are not
//! measured, so a `du` of the root exceeds the sum of the three units
//! by exactly those. The `~/esp` checkout (ESP-IDF's source) is an
//! ordinary project root the user declares, never proposed here.
//!
//! What removing each one costs is `crate::build_adapters::tool_stores`'s
//! job, in Espressif's own commands.

use super::{
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LastUseDecl, LastUseSource,
    LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};
use std::path::PathBuf;

pub const ESPRESSIF_DETECTOR_ID: &str = "espressif";

fn tools_root(env: &Environment) -> (PathBuf, Provenance) {
    match env.env_var("IDF_TOOLS_PATH").filter(|v| !v.is_empty()) {
        Some(v) => (
            PathBuf::from(v),
            Provenance::EnvVar("IDF_TOOLS_PATH".to_string()),
        ),
        None => (env.home.join(".espressif"), Provenance::BuiltinConvention),
    }
}

pub struct EspressifDetector;

impl Detector for EspressifDetector {
    fn id(&self) -> &'static str {
        ESPRESSIF_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Espressif ESP-IDF"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "ESP-IDF idf_tools.py layout (dist, tools, python_env under IDF_TOOLS_PATH or ~/.espressif)"
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[
            BuildStoreDecl {
                kind: BuildStoreKind::EspressifDist,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Cache,
                    suffix: &[],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::EspressifTools,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Installation,
                    suffix: &[],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::EspressifPythonEnv,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Environments,
                    suffix: &[],
                },
            },
        ]
    }

    fn last_use_sources(&self) -> &'static [LastUseDecl] {
        // `tools/<tool>/<version>/<tool>/bin`.
        &[LastUseDecl {
            anchor: StoreAnchor::Categorized {
                category: StorageCategory::Installation,
                suffix: &[],
            },
            source: LastUseSource::KeyFileAtime { max_depth: 4 },
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (root, provenance) = tools_root(env);
        [
            (
                "dist",
                StorageCategory::Cache,
                "downloaded tool archives (dist/)",
            ),
            (
                "tools",
                StorageCategory::Installation,
                "installed toolchains and tools (tools/)",
            ),
            (
                "python_env",
                StorageCategory::Environments,
                "per-ESP-IDF-version Python virtual environments (python_env/)",
            ),
        ]
        .into_iter()
        .map(|(folder, category, note)| ProposedLocation {
            detector_id: ESPRESSIF_DETECTOR_ID.to_string(),
            path: Some(root.join(folder)),
            category,
            provenance: provenance.clone(),
            status: LocationStatus::Resolved,
            note: Some(note.to_string()),
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn by_folder(got: &[ProposedLocation], root: &str) -> Vec<(String, StorageCategory)> {
        got.iter()
            .map(|l| {
                let p = l.path.as_ref().unwrap();
                assert_eq!(p.parent().unwrap(), PathBuf::from(root));
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    l.category,
                )
            })
            .collect()
    }

    #[test]
    fn convention_root_holds_the_three_units_with_their_kinds() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = EspressifDetector.detect(&env);
        assert_eq!(
            by_folder(&got, "/Users/dev/.espressif"),
            vec![
                ("dist".to_string(), StorageCategory::Cache),
                ("tools".to_string(), StorageCategory::Installation),
                ("python_env".to_string(), StorageCategory::Environments),
            ]
        );
        assert!(
            got.iter()
                .all(|l| l.provenance == Provenance::BuiltinConvention)
        );
    }

    #[test]
    fn idf_tools_path_replaces_the_convention_root() {
        let mut env_vars = HashMap::new();
        env_vars.insert("IDF_TOOLS_PATH".to_string(), "/data/idf-tools".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = EspressifDetector.detect(&env);
        assert_eq!(by_folder(&got, "/data/idf-tools").len(), 3);
        assert!(got.iter().all(|l| {
            l.provenance == Provenance::EnvVar("IDF_TOOLS_PATH".to_string())
                && !l.path.as_ref().unwrap().starts_with("/Users/dev")
        }));
    }

    #[test]
    fn an_empty_idf_tools_path_is_the_convention() {
        let mut env_vars = HashMap::new();
        env_vars.insert("IDF_TOOLS_PATH".to_string(), String::new());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::Linux);
        let got = EspressifDetector.detect(&env);
        assert_eq!(by_folder(&got, "/Users/dev/.espressif").len(), 3);
    }

    #[test]
    fn a_missing_root_is_still_proposed_so_scope_reports_it_missing() {
        let env = Environment::fixture(
            PathBuf::from("/nonexistent"),
            HashMap::new(),
            Platform::Linux,
        );
        let got = EspressifDetector.detect(&env);
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }

    #[test]
    fn no_source_checkout_is_proposed() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        assert!(
            EspressifDetector.detect(&env).iter().all(|l| !l
                .path
                .as_ref()
                .unwrap()
                .starts_with("/Users/dev/esp"))
        );
    }
}
