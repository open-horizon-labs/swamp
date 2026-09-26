//! Cargo home: `CARGO_HOME` override, else `~/.cargo` (Cargo Book,
//! "cargo-home"). https://doc.rust-lang.org/cargo/guide/cargo-home.html
//!
//! Reports the home directory itself (installation-adjacent: `bin/`,
//! `config.toml`, credentials) separately from four categorized
//! subtrees (#47's refinement over the original two-way registry/git
//! split): `registry/cache` (raw downloaded `.crate` files) and
//! `git/db` (bare git object databases) are raw downloads; `registry/src`
//! (extracted crate sources) and `git/checkouts` (checked-out worktrees
//! materialized from `git/db`) are derived/extracted content -- still
//! disposable, but re-materializing them costs a local extraction, not
//! a network fetch, which is a materially different "how expensive to
//! get back" story worth keeping visible. This registry only proposes
//! locations; it does not judge disposal.

use std::path::PathBuf;

use super::{
    ConventionRole, Detector, Environment, LocationStatus, ManagerConvention, Platform,
    ProposedLocation, Provenance, RecoveryCost, RecoveryHint, StorageCategory, StoreAnchor,
    StoreEntryLookup,
};

pub const CARGO_HOME_DETECTOR_ID: &str = "cargo-home";

pub struct CargoHomeDetector;

impl Detector for CargoHomeDetector {
    fn id(&self) -> &'static str {
        CARGO_HOME_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Cargo home"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "Cargo Book cargo-home layout, current stable"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("cargo"),
            role: ConventionRole::DependencyStore {
                // `registry/src` holds the *extracted* sources, the one
                // subtree where a name+version is a real directory;
                // `git/checkouts` is the detector's other Cache
                // location, which the suffix keeps distinct.
                anchor: StoreAnchor::Categorized {
                    category: StorageCategory::Cache,
                    suffix: &["registry", "src"],
                },
                lookup: StoreEntryLookup::RegistrySourceTree,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "cargo fetch",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("CARGO_HOME") {
            Some(v) if !v.is_empty() => (
                PathBuf::from(v),
                Provenance::EnvVar("CARGO_HOME".to_string()),
            ),
            _ => (env.home.join(".cargo"), Provenance::BuiltinConvention),
        };
        vec![
            ProposedLocation {
                detector_id: CARGO_HOME_DETECTOR_ID.to_string(),
                path: Some(base.clone()),
                category: StorageCategory::Installation,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("cargo home: bin/, config.toml, credentials".to_string()),
            },
            ProposedLocation {
                detector_id: CARGO_HOME_DETECTOR_ID.to_string(),
                path: Some(base.join("registry/cache")),
                category: StorageCategory::Downloads,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("downloaded .crate archives (raw registry download cache)".to_string()),
            },
            ProposedLocation {
                detector_id: CARGO_HOME_DETECTOR_ID.to_string(),
                path: Some(base.join("registry/src")),
                category: StorageCategory::Cache,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("extracted crate sources, materialized from registry/cache".to_string()),
            },
            ProposedLocation {
                detector_id: CARGO_HOME_DETECTOR_ID.to_string(),
                path: Some(base.join("registry/index")),
                category: StorageCategory::Downloads,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("registry index cache".to_string()),
            },
            ProposedLocation {
                detector_id: CARGO_HOME_DETECTOR_ID.to_string(),
                path: Some(base.join("git/db")),
                category: StorageCategory::Downloads,
                provenance: provenance.clone(),
                status: LocationStatus::Resolved,
                note: Some("bare git object databases for git dependencies".to_string()),
            },
            ProposedLocation {
                detector_id: CARGO_HOME_DETECTOR_ID.to_string(),
                path: Some(base.join("git/checkouts")),
                category: StorageCategory::Cache,
                provenance,
                status: LocationStatus::Resolved,
                note: Some("checked-out worktrees materialized from git/db".to_string()),
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
        let got = CargoHomeDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.cargo")));
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.cargo/registry/cache")))
        );
        assert!(
            got.iter()
                .any(|l| l.path == Some(PathBuf::from("/Users/dev/.cargo/git/checkouts")))
        );
        assert!(matches!(got[0].provenance, Provenance::BuiltinConvention));
    }

    /// #47's refinement: raw downloads (registry/cache, registry/index,
    /// git/db) and derived/extracted content (registry/src,
    /// git/checkouts) must be categorized distinctly, not collapsed
    /// back into one "cache" bucket -- the tempting shortcut this
    /// catches is re-merging registry/git into a single category entry
    /// the way the pre-#47 detector did.
    #[test]
    fn downloads_and_extracted_content_are_categorized_distinctly() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CargoHomeDetector.detect(&env);
        let cat = |rel: &str| {
            got.iter()
                .find(|l| l.path == Some(PathBuf::from("/Users/dev/.cargo").join(rel)))
                .map(|l| l.category)
        };
        assert_eq!(cat("registry/cache"), Some(StorageCategory::Downloads));
        assert_eq!(cat("registry/src"), Some(StorageCategory::Cache));
        assert_eq!(cat("registry/index"), Some(StorageCategory::Downloads));
        assert_eq!(cat("git/db"), Some(StorageCategory::Downloads));
        assert_eq!(cat("git/checkouts"), Some(StorageCategory::Cache));
    }

    #[test]
    fn env_var_override_wins_and_is_labelled() {
        let mut env_vars = HashMap::new();
        env_vars.insert("CARGO_HOME".to_string(), "/opt/cargo-home".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = CargoHomeDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/cargo-home")));
        assert_eq!(
            got[0].provenance,
            Provenance::EnvVar("CARGO_HOME".to_string())
        );
    }

    #[test]
    fn leftovers_found_without_executable_present() {
        // The detector never checks whether `cargo` is on PATH; a
        // convention probe still proposes the path. Existence is scope
        // resolution's job (`crate::scope`), not the detector's.
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CargoHomeDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
