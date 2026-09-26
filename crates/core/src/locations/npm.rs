//! npm cache: the whole `~/.npm` directory (its `_cacache` subdirectory
//! holds the actual content-addressed cache; the parent also holds
//! small log/anonymous-cli-metrics files, so the whole directory is
//! proposed as one cache location rather than hard-coding the
//! `_cacache` name, which npm has changed before).
//! https://docs.npmjs.com/cli/v11/commands/npm-cache/
//!
//! Overridable via the `npm_config_cache` config-derived env var (npm's
//! own documented naming: config key `cache` becomes env var
//! `npm_config_cache`), or the `NPM_CONFIG_CACHE` a user is more likely
//! to have actually exported by hand; both are read, never `npm config
//! get cache` (no npm process is ever spawned by this detector).

use super::{
    BuildStoreDecl, BuildStoreKind, ConventionRole, Detector, Environment, LocationStatus,
    ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost, RecoveryHint,
    StorageCategory, StoreAnchor, StoreEntryLookup,
};

pub const NPM_DETECTOR_ID: &str = "npm";

impl NpmDetector {
    fn resolve(&self, env: &Environment) -> (std::path::PathBuf, Provenance) {
        for var in ["npm_config_cache", "NPM_CONFIG_CACHE"] {
            if let Some(v) = env.env_var(var).filter(|v| !v.is_empty()) {
                return (
                    std::path::PathBuf::from(v),
                    Provenance::EnvVar(var.to_string()),
                );
            }
        }
        (env.home.join(".npm"), Provenance::BuiltinConvention)
    }
}

pub struct NpmDetector;

impl Detector for NpmDetector {
    fn id(&self) -> &'static str {
        NPM_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "npm cache"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "npm-cache CLI reference, current stable"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("npm"),
            role: ConventionRole::DependencyStore {
                anchor: StoreAnchor::SoleLocation,
                lookup: StoreEntryLookup::ContentAddressed {
                    basis: "npm cacache content-addressed store",
                    reason: "npm's cache is content-addressed (sha-keyed); a declared package \
                             name+version cannot be mapped to a specific cache entry",
                },
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "npm install",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[BuildStoreDecl {
            kind: BuildStoreKind::NpmCache,
            anchor: StoreAnchor::SoleLocation,
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (path, provenance) = self.resolve(env);
        vec![ProposedLocation {
            detector_id: NPM_DETECTOR_ID.to_string(),
            path: Some(path),
            category: StorageCategory::Cache,
            provenance,
            status: LocationStatus::Resolved,
            note: Some("npm's content-addressed package download cache".to_string()),
        }]
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
        let got = NpmDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.npm")));
    }

    #[test]
    fn lowercase_npm_config_var_wins() {
        let mut env_vars = HashMap::new();
        env_vars.insert("npm_config_cache".to_string(), "/opt/npm-cache".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = NpmDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/npm-cache")));
    }

    #[test]
    fn uppercase_fallback_used_when_lowercase_absent() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "NPM_CONFIG_CACHE".to_string(),
            "/opt/npm-cache2".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = NpmDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/npm-cache2")));
    }

    #[test]
    fn leftovers_found_without_npm_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = NpmDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
