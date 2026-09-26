//! pnpm content-addressable store: `PNPM_HOME` (store lives at
//! `$PNPM_HOME/store`), else `store-dir` read from `~/.npmrc` (pnpm
//! shares npm's `.npmrc` config file format), else the per-platform
//! convention (`~/Library/pnpm/store` on macOS, `~/.local/share/pnpm/store`
//! on Linux). https://pnpm.io/settings/store
//!
//! pnpm documents a *per-disk* store (a volume without the home
//! directory's disk gets its own `<volume-root>/.pnpm-store`, since
//! hard-linking requires the same filesystem). Enumerating every mounted
//! volume to find one is out of scope for this detector -- see
//! `docs/locations.md`'s documented limit -- so only the home-disk store
//! above is proposed; a project-linked pnpm store on another volume is
//! a real, named gap, not silently claimed as covered.

use super::{
    BuildStoreDecl, BuildStoreKind, ConventionRole, Detector, Environment, LocationStatus,
    ManagerConvention, Platform, ProposedLocation, Provenance, RecoveryCost, RecoveryHint,
    StorageCategory, StoreAnchor, StoreEntryLookup,
};
use std::path::PathBuf;

pub const PNPM_DETECTOR_ID: &str = "pnpm";

pub struct PnpmDetector;

/// `.npmrc` is `key = value` (or `key=value`) lines, one per line, `#`/`;`
/// comments. Only the one key this detector cares about is read.
fn read_npmrc_field(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() == key {
            return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

impl Detector for PnpmDetector {
    fn id(&self) -> &'static str {
        PNPM_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "pnpm store"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "pnpm store settings reference, current stable (per-disk stores not enumerated)"
    }

    fn manager_conventions(&self) -> &'static [ManagerConvention] {
        &[ManagerConvention {
            tool: Some("pnpm"),
            role: ConventionRole::DependencyStore {
                anchor: StoreAnchor::SoleLocation,
                // The store is content-addressed *and* per-file, so a
                // package identity maps to no single entry -- a project
                // that names any pnpm identity consumes the store as a
                // whole, which is all that can honestly be said.
                lookup: StoreEntryLookup::WholeStore,
            },
        }]
    }

    fn recovery_hint(&self) -> Option<RecoveryHint> {
        Some(RecoveryHint {
            command: "pnpm install",
            cost: RecoveryCost::NetworkRefetch,
        })
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[BuildStoreDecl {
            kind: BuildStoreKind::PnpmStore,
            anchor: StoreAnchor::SoleLocation,
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        if let Some(v) = env.env_var("PNPM_HOME").filter(|v| !v.is_empty()) {
            return vec![ProposedLocation {
                detector_id: PNPM_DETECTOR_ID.to_string(),
                path: Some(PathBuf::from(v).join("store")),
                category: StorageCategory::Cache,
                provenance: Provenance::EnvVar("PNPM_HOME".to_string()),
                status: LocationStatus::Resolved,
                note: Some("content-addressed package store".to_string()),
            }];
        }

        let npmrc = env.home.join(".npmrc");
        if let Ok(text) =
            crate::fs_gate::read::bounded_string(&npmrc, crate::fs_gate::read::BoundedCap::MANIFEST)
            && let Some(dir) = read_npmrc_field(&text, "store-dir")
        {
            return vec![ProposedLocation {
                detector_id: PNPM_DETECTOR_ID.to_string(),
                path: Some(PathBuf::from(dir)),
                category: StorageCategory::Cache,
                provenance: Provenance::ConfigField("store-dir".to_string()),
                status: LocationStatus::Resolved,
                note: Some("content-addressed package store (~/.npmrc store-dir)".to_string()),
            }];
        }

        let convention = match env.platform {
            Platform::MacOS => env.home.join("Library/pnpm/store"),
            Platform::Linux => env.home.join(".local/share/pnpm/store"),
        };
        vec![ProposedLocation {
            detector_id: PNPM_DETECTOR_ID.to_string(),
            path: Some(convention),
            category: StorageCategory::Cache,
            provenance: Provenance::BuiltinConvention,
            status: LocationStatus::Resolved,
            note: Some(
                "content-addressed package store (home-disk convention; per-volume \
                 stores on other disks are not enumerated, see docs/locations.md)"
                    .to_string(),
            ),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn macos_convention_path() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = PnpmDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/Users/dev/Library/pnpm/store"))
        );
    }

    #[test]
    fn linux_convention_path() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = PnpmDetector.detect(&env);
        assert_eq!(
            got[0].path,
            Some(PathBuf::from("/home/dev/.local/share/pnpm/store"))
        );
    }

    #[test]
    fn pnpm_home_env_var_wins_over_npmrc_and_convention() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".npmrc"), "store-dir=/from/npmrc\n").unwrap();
        let mut env_vars = HashMap::new();
        env_vars.insert("PNPM_HOME".to_string(), "/opt/pnpm".to_string());
        let env = Environment::fixture(tmp.path().to_path_buf(), env_vars, Platform::MacOS);
        let got = PnpmDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/pnpm/store")));
    }

    #[test]
    fn npmrc_store_dir_used_when_no_pnpm_home() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".npmrc"),
            "# a comment\nregistry=https://example.invalid\nstore-dir = /data/pnpm-store\n",
        )
        .unwrap();
        let env = Environment::fixture(tmp.path().to_path_buf(), HashMap::new(), Platform::MacOS);
        let got = PnpmDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/data/pnpm-store")));
        assert_eq!(
            got[0].provenance,
            Provenance::ConfigField("store-dir".to_string())
        );
    }

    #[test]
    fn leftovers_found_without_pnpm_executable() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = PnpmDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }
}
