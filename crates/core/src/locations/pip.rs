//! pip's download/wheel cache: `PIP_CACHE_DIR`, else
//! `~/Library/Caches/pip` on macOS / `~/.cache/pip` on Linux. uv's own
//! cache is covered by `crate::locations::uv`, not here; `__pycache__`
//! directories are project-level bytecode caches (scattered per
//! project, not a single home-relative location) and are deliberately
//! not a detector at all -- see `docs/locations.md`.

use super::{
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};

pub const PIP_DETECTOR_ID: &str = "pip";

pub struct PipDetector;

impl Detector for PipDetector {
    fn id(&self) -> &'static str {
        PIP_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "pip cache"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "pip user guide, current stable cache directory defaults"
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[BuildStoreDecl {
            kind: BuildStoreKind::PipCache,
            anchor: StoreAnchor::SoleLocation,
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (path, provenance) = match env.env_var("PIP_CACHE_DIR").filter(|v| !v.is_empty()) {
            Some(v) => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("PIP_CACHE_DIR".to_string()),
            ),
            None => {
                let default = match env.platform {
                    Platform::MacOS => env.home.join("Library/Caches/pip"),
                    Platform::Linux => env.home.join(".cache/pip"),
                };
                (default, Provenance::BuiltinConvention)
            }
        };
        vec![ProposedLocation {
            detector_id: PIP_DETECTOR_ID.to_string(),
            path: Some(path),
            category: StorageCategory::Cache,
            provenance,
            status: LocationStatus::Resolved,
            note: Some("downloaded wheel/sdist cache".to_string()),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn convention_macos() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = PipDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/Library/Caches/pip"))
        );
    }

    #[test]
    fn convention_linux() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = PipDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/home/dev/.cache/pip")));
    }

    #[test]
    fn env_var_override_wins() {
        let mut env_vars = HashMap::new();
        env_vars.insert("PIP_CACHE_DIR".to_string(), "/opt/pip-cache".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = PipDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/pip-cache")));
    }

    #[test]
    fn leftovers_found_without_pip_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = PipDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
