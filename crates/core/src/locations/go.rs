//! Go module cache (`GOMODCACHE`, default `$GOPATH/pkg/mod` =
//! `~/go/pkg/mod`) and build cache (`GOCACHE`, default
//! `~/Library/Caches/go-build` on macOS, `~/.cache/go-build` on Linux).
//! Within the module cache, `cache/download/` holds the raw downloaded
//! `.zip`/`.info`/`.mod` files; everything else is the extracted,
//! read-only module source tree -- the same raw-download-vs-extracted
//! split as Cargo's registry (#47).
//!
//! Overrides are read from the `GOENV` file (default
//! `~/Library/Application Support/go/env` on macOS,
//! `~/.config/go/env` on Linux -- Go's own `os.UserConfigDir()`
//! convention) as plain `KEY=VALUE` lines, or from `GOMODCACHE`/
//! `GOCACHE`/`GOPATH` environment variables directly. `go env` is never
//! executed: this detector only ever reads files/env vars.
//! https://pkg.go.dev/cmd/go

use super::{
    BuildStoreDecl, BuildStoreKind, ConventionRole, Detector, Environment, LocationStatus,
    ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost, RecoveryHint,
    StorageCategory, StoreAnchor, StoreEntryLookup,
};
use std::collections::HashMap;
use std::path::PathBuf;

pub const GO_DETECTOR_ID: &str = "go";

fn read_goenv(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

fn goenv_path(env: &Environment) -> PathBuf {
    match env.platform {
        Platform::MacOS => env.home.join("Library/Application Support/go/env"),
        Platform::Linux => env.home.join(".config/go/env"),
    }
}

pub struct GoDetector;

impl Detector for GoDetector {
    fn id(&self) -> &'static str {
        GO_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Go"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "go command reference, current stable GOMODCACHE/GOCACHE defaults"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("go"),
            role: ConventionRole::DependencyStore {
                // `GOMODCACHE` is a free-form override, so the module
                // cache has no fixed path suffix of its own -- but the
                // `cache/download` location this detector derives from
                // it always does, so the module cache is that sibling's
                // grandparent rather than a guess at a path shape.
                anchor: StoreAnchor::AncestorOfSibling {
                    sibling: StorageCategory::Downloads,
                    up: 2,
                    category: StorageCategory::Cache,
                },
                lookup: StoreEntryLookup::GoModulePath,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "go mod download",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[
            BuildStoreDecl {
                kind: BuildStoreKind::GoModuleCache,
                anchor: StoreAnchor::AncestorOfSibling {
                    sibling: StorageCategory::Downloads,
                    up: 2,
                    category: StorageCategory::Cache,
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::GoModuleDownloads,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Downloads,
                    suffix: &[],
                },
            },
            BuildStoreDecl {
                kind: BuildStoreKind::GoBuildCache,
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::BuildOutput,
                    suffix: &[],
                },
            },
        ]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let goenv: HashMap<String, String> = crate::fs_gate::read::bounded_string(
            goenv_path(env),
            crate::fs_gate::read::BoundedCap::MANIFEST,
        )
        .ok()
        .map(|t| read_goenv(&t))
        .unwrap_or_default();

        let gopath = env
            .env_var("GOPATH")
            .map(|s| s.to_string())
            .or_else(|| goenv.get("GOPATH").cloned())
            .unwrap_or_else(|| env.home.join("go").display().to_string());

        let (modcache, modcache_prov) = match env
            .env_var("GOMODCACHE")
            .map(|s| (s.to_string(), Provenance::EnvVar("GOMODCACHE".to_string())))
            .or_else(|| {
                goenv
                    .get("GOMODCACHE")
                    .map(|v| (v.clone(), Provenance::ConfigField("GOMODCACHE".to_string())))
            }) {
            Some((v, prov)) => (PathBuf::from(v), prov),
            None => (
                PathBuf::from(gopath).join("pkg/mod"),
                Provenance::BuiltinConvention,
            ),
        };

        let (gocache, gocache_prov) = match env
            .env_var("GOCACHE")
            .map(|s| (s.to_string(), Provenance::EnvVar("GOCACHE".to_string())))
            .or_else(|| {
                goenv
                    .get("GOCACHE")
                    .map(|v| (v.clone(), Provenance::ConfigField("GOCACHE".to_string())))
            }) {
            Some((v, prov)) => (PathBuf::from(v), prov),
            None => {
                let default = match env.platform {
                    Platform::MacOS => env.home.join("Library/Caches/go-build"),
                    Platform::Linux => env.home.join(".cache/go-build"),
                };
                (default, Provenance::BuiltinConvention)
            }
        };

        vec![
            ProposedLocation {
                detector_id: GO_DETECTOR_ID.to_string(),
                path: Some(modcache.join("cache/download")),
                category: StorageCategory::Downloads,
                provenance: modcache_prov.clone(),
                status: LocationStatus::Resolved,
                note: Some("raw downloaded module .zip/.info/.mod files".to_string()),
            },
            ProposedLocation {
                detector_id: GO_DETECTOR_ID.to_string(),
                path: Some(modcache),
                category: StorageCategory::Cache,
                provenance: modcache_prov,
                status: LocationStatus::Resolved,
                note: Some("extracted, read-only module source tree".to_string()),
            },
            ProposedLocation {
                detector_id: GO_DETECTOR_ID.to_string(),
                path: Some(gocache),
                // Compiled package and test outputs, keyed by action
                // hash: a build cache, not a download cache. Its own
                // category is also what lets `build_stores` tell it from
                // the module cache, whose path is just as free-form.
                category: StorageCategory::BuildOutput,
                provenance: gocache_prov,
                status: LocationStatus::Resolved,
                note: Some("build cache".to_string()),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn convention_defaults_macos() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = GoDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/go/pkg/mod"))
                    && l.category == StorageCategory::Cache)
        );
        assert!(got.iter().any(|l| l.path
            == Some(PathBuf::from("/Users/dev/go/pkg/mod/cache/download"))
            && l.category == StorageCategory::Downloads));
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/Library/Caches/go-build")))
        );
    }

    #[test]
    fn convention_defaults_linux() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = GoDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/home/dev/.cache/go-build")))
        );
    }

    #[test]
    fn env_vars_override_goenv_file_and_convention() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Library/Application Support/go")).unwrap();
        std::fs::write(
            tmp.path().join("Library/Application Support/go/env"),
            "GOMODCACHE=/from-goenv/mod\nGOCACHE=/from-goenv/build\n",
        )
        .unwrap();
        let mut env_vars = HashMap::new();
        env_vars.insert("GOMODCACHE".to_string(), "/from-env/mod".to_string());
        let env = Environment::fixture(tmp.path().to_path_buf(), env_vars, Platform::MacOS);
        let got = GoDetector.detect(&env);
        // GOMODCACHE env var wins over the goenv file.
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/from-env/mod")))
        );
        // GOCACHE was not set by env, so the goenv file's value applies.
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/from-goenv/build")))
        );
    }

    #[test]
    fn gopath_env_var_relocates_the_default_modcache() {
        let mut env_vars = HashMap::new();
        env_vars.insert("GOPATH".to_string(), "/opt/gopath".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = GoDetector.detect(&env);
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/opt/gopath/pkg/mod")))
        );
    }

    #[test]
    fn leftovers_found_without_go_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = GoDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
