//! uv: `UV_PYTHON_INSTALL_DIR` (default `~/.local/share/uv/python`),
//! `UV_TOOL_DIR` (default `~/.local/share/uv/tools`), and
//! `UV_CACHE_DIR` (default `~/.cache/uv` -- uv documents this same
//! `~/.cache/uv` path on *both* macOS and Linux, deliberately not
//! macOS's `~/Library/Caches`, unlike this catalog's other cache
//! locations; verified against uv's storage reference rather than
//! assumed from the general macOS convention).
//! https://docs.astral.sh/uv/reference/storage/

use super::{
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};

pub const UV_DETECTOR_ID: &str = "uv";

fn resolve(env: &Environment, var: &str, default_rel: &str) -> (std::path::PathBuf, Provenance) {
    match env.env_var(var) {
        Some(v) if !v.is_empty() => (
            std::path::PathBuf::from(v),
            Provenance::EnvVar(var.to_string()),
        ),
        _ => (env.home.join(default_rel), Provenance::BuiltinConvention),
    }
}

pub struct UvDetector;

impl Detector for UvDetector {
    fn id(&self) -> &'static str {
        UV_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "uv"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "uv storage reference, current stable (cache dir is ~/.cache/uv on macOS too)"
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[
            BuildStoreDecl {
                kind: BuildStoreKind::UvCache,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Cache,
                    suffix: &[],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::UvPythonInstallations,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Installation,
                    suffix: &[],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::UvToolEnvironments,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Environments,
                    suffix: &[],
                },
            },
        ]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (python_dir, python_prov) =
            resolve(env, "UV_PYTHON_INSTALL_DIR", ".local/share/uv/python");
        let (tool_dir, tool_prov) = resolve(env, "UV_TOOL_DIR", ".local/share/uv/tools");
        let (cache_dir, cache_prov) = resolve(env, "UV_CACHE_DIR", ".cache/uv");

        vec![
            ProposedLocation {
                detector_id: UV_DETECTOR_ID.to_string(),
                path: Some(python_dir),
                category: StorageCategory::Installation,
                provenance: python_prov,
                status: LocationStatus::Resolved,
                note: Some("managed Python installations".to_string()),
            },
            ProposedLocation {
                detector_id: UV_DETECTOR_ID.to_string(),
                path: Some(tool_dir),
                category: StorageCategory::Environments,
                provenance: tool_prov,
                status: LocationStatus::Resolved,
                note: Some("installed tool (CLI package) environments".to_string()),
            },
            ProposedLocation {
                detector_id: UV_DETECTOR_ID.to_string(),
                path: Some(cache_dir),
                category: StorageCategory::Cache,
                provenance: cache_prov,
                status: LocationStatus::Resolved,
                note: Some("dependency/wheel cache and script venvs".to_string()),
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
    fn convention_paths_never_use_library_caches_on_macos() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = UvDetector.detect(&env);
        let cache = got
            .iter()
            .find(|l| l.category == StorageCategory::Cache)
            .unwrap();
        assert_eq!(cache.path, Some(PathBuf::from("/Users/dev/.cache/uv")));
    }

    #[test]
    fn env_overrides_each_independently() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "UV_PYTHON_INSTALL_DIR".to_string(),
            "/opt/uv-python".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = UvDetector.detect(&env);
        assert_eq!(
            got.iter()
                .find(|l| l.category == StorageCategory::Installation)
                .unwrap()
                .path,
            Some(PathBuf::from("/opt/uv-python"))
        );
        // Untouched vars keep their convention path.
        assert_eq!(
            got.iter()
                .find(|l| l.category == StorageCategory::Environments)
                .unwrap()
                .path,
            Some(PathBuf::from("/Users/dev/.local/share/uv/tools"))
        );
    }

    #[test]
    fn leftovers_found_without_uv_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = UvDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
