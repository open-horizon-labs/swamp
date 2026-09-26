//! Codex CLI home: `CODEX_HOME` override, else `~/.codex`.
//! <https://github.com/openai/codex/blob/main/codex-rs/utils/home-dir/src/lib.rs>
//! (`find_codex_home`): "the path to the Codex configuration directory,
//! which can be specified by the `CODEX_HOME` environment variable. If
//! not set, defaults to `~/.codex`." Unlike this detector, upstream's own
//! resolver requires an existing directory when `CODEX_HOME` is set and
//! canonicalizes it; this detector, like every other one in this
//! registry, only proposes a candidate path -- existence is `crate::scope`'s
//! job, and canonicalization is deliberately not done here (see
//! `crate::agents::claude_code`'s own doc comment on why an agent unit's
//! paths are built as non-canonical joins).
//!
//! This detector resolves the home directory itself as one external unit,
//! exactly like `crate::locations::claude_code`; its interior (sessions,
//! archived sessions, SQLite state stores, protected config) is
//! identified by `crate::agents::codex` (#93).
//!
//! Two documented gaps, kept honest rather than modeled speculatively:
//! - `CODEX_SQLITE_HOME` (a *second*, independent env var --
//!   <https://github.com/openai/codex/blob/main/codex-rs/state/src/lib.rs>,
//!   `SQLITE_HOME_ENV`) can relocate Codex's SQLite state databases
//!   *outside* `CODEX_HOME` entirely. This detector does not resolve it;
//!   `crate::agents::codex` documents the same gap next to its SQLite
//!   identification.
//! - The Codex **desktop app** is a materially different client with its
//!   own storage; it is modeled by the separate
//!   `crate::locations::codex_desktop` detector, never by extrapolating
//!   this CLI schema (#93's explicit acceptance).

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const CODEX_DETECTOR_ID: &str = "codex";

pub struct CodexDetector;

impl Detector for CodexDetector {
    fn id(&self) -> &'static str {
        CODEX_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Codex"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "github.com/openai/codex codex-rs source, current main as of this chunk; see \
         crate::agents::codex for the interior identification and its own sourced notes"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("CODEX_HOME") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("CODEX_HOME".to_string()),
            ),
            _ => (env.home.join(".codex"), Provenance::BuiltinConvention),
        };
        vec![ProposedLocation {
            detector_id: CODEX_DETECTOR_ID.to_string(),
            path: Some(base),
            category: StorageCategory::LocalState,
            provenance,
            status: LocationStatus::Resolved,
            note: Some(
                "Codex CLI home: rollout sessions, archived sessions, SQLite state stores and \
                 protected config; see crate::agents::codex for the interior identification"
                    .to_string(),
            ),
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
        let got = CodexDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.codex")));
        assert!(matches!(got[0].provenance, Provenance::BuiltinConvention));
        assert_eq!(got[0].status, LocationStatus::Resolved);
    }

    #[test]
    fn env_var_override_wins_and_is_labelled() {
        let mut env_vars = HashMap::new();
        env_vars.insert("CODEX_HOME".to_string(), "/opt/codex-home".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = CodexDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/codex-home")));
        assert_eq!(
            got[0].provenance,
            Provenance::EnvVar("CODEX_HOME".to_string())
        );
    }

    #[test]
    fn linux_also_proposes_the_convention_path() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = CodexDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/home/dev/.codex")));
    }
}
