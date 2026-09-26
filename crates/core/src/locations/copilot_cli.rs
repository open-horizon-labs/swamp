//! GitHub Copilot CLI home: `~/.copilot`, or the whole of `COPILOT_HOME`
//! when set. Sourced directly from GitHub's own reference page (fetched
//! during implementation, never from a real `~/.copilot` on this machine
//! -- PRIVACY IS A HARD RULE):
//! <https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference>,
//! current as of this chunk. That page documents, verbatim:
//! - `config.json`: "Automatically managed application state
//!   (authentication, installed plugins, and other internal data)".
//! - `settings.json`: primary user configuration (JSONC).
//! - `mcp-config.json` / `lsp-config.json` / `permissions-config.json` /
//!   `providers.json`: user-level MCP servers, LSP definitions, saved
//!   per-project tool/directory permissions, and a bring-your-own-key
//!   provider/model registry, respectively.
//! - `mcp-oauth-config/` and `mcp-secrets/`: MCP OAuth tokens/
//!   registration fallback storage and MCP secret placeholders/index.
//! - `agents/`, `extensions/`, `hooks/`, `instructions/`, `skills/`,
//!   `installed-plugins/`, `plugin-data/`: user-level definitions/data.
//! - `copilot-instructions.md`: personal cross-session instructions.
//! - `session-state/`: **"Session history and workspace artifacts"** --
//!   this is #97's own named uncertainty ("verify names") resolved: the
//!   real directory is `session-state/`, not `history-session-state/`
//!   as the issue text guessed.
//! - `command-history-state/`: reverse-search command history (distinct
//!   from `session-state/`, and from shell history -- this is Copilot
//!   CLI's own REPL command recall, not conversation content).
//! - `session-store.db`: "SQLite database for cross-session data".
//! - `logs/`: session logs and debugging data.
//! - `ide/`: IDE integration state/lock files.
//!
//! The **cache** directory is explicitly separate and platform-
//! conventional, *not* affected by `COPILOT_HOME`: macOS
//! `~/Library/Caches/copilot`, Linux `${XDG_CACHE_HOME:-~/.cache}/
//! copilot`, overridable independently via `COPILOT_CACHE_HOME`. This
//! detector proposes it as a second, non-decomposed location (same
//! "opaque, wholly re-downloadable" treatment `crate::locations::opencode`
//! gives its own cache root) -- `crate::agents::copilot_cli` only
//! decomposes the home root.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const COPILOT_CLI_DETECTOR_ID: &str = "github-copilot-cli";

pub struct CopilotCliDetector;

impl Detector for CopilotCliDetector {
    fn id(&self) -> &'static str {
        COPILOT_CLI_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "GitHub Copilot CLI"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "docs.github.com copilot-cli-reference/cli-config-dir-reference, current as of this \
         chunk; session-state/ and command-history-state/ are the real directory names, \
         correcting #97's own issue-text guess of history-session-state/"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (home, home_provenance) = match env.env_var("COPILOT_HOME") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("COPILOT_HOME".to_string()),
            ),
            _ => (env.home.join(".copilot"), Provenance::BuiltinConvention),
        };
        let (cache, cache_provenance) = match env.env_var("COPILOT_CACHE_HOME") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("COPILOT_CACHE_HOME".to_string()),
            ),
            _ => match env.platform {
                Platform::MacOS => (
                    env.home.join("Library").join("Caches").join("copilot"),
                    Provenance::BuiltinConvention,
                ),
                Platform::Linux => {
                    let (base, _) = match env.env_var("XDG_CACHE_HOME") {
                        Some(v) if !v.is_empty() => (
                            std::path::PathBuf::from(v),
                            Provenance::EnvVar("XDG_CACHE_HOME".to_string()),
                        ),
                        _ => (env.home.join(".cache"), Provenance::BuiltinConvention),
                    };
                    (base.join("copilot"), Provenance::BuiltinConvention)
                }
            },
        };
        vec![
            ProposedLocation {
                detector_id: COPILOT_CLI_DETECTOR_ID.to_string(),
                path: Some(home),
                category: StorageCategory::LocalState,
                provenance: home_provenance,
                status: LocationStatus::Resolved,
                note: Some(
                    "GitHub Copilot CLI home: config/settings/mcp/permissions (protected), \
                     session-state/, command-history-state/, session-store.db, logs/; see \
                     crate::agents::copilot_cli"
                        .to_string(),
                ),
            },
            ProposedLocation {
                detector_id: COPILOT_CLI_DETECTOR_ID.to_string(),
                path: Some(cache),
                category: StorageCategory::Cache,
                provenance: cache_provenance,
                status: LocationStatus::Resolved,
                note: Some(
                    "GitHub Copilot CLI cache: platform-conventional, independent of COPILOT_HOME; \
                     reported as an opaque external unit, not decomposed"
                        .to_string(),
                ),
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
    fn convention_paths_macos() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = CopilotCliDetector.detect(&env);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.copilot")));
        assert_eq!(
            got[1].path,
            Some(PathBuf::from("/Users/dev/Library/Caches/copilot"))
        );
    }

    #[test]
    fn convention_paths_linux() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = CopilotCliDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/home/dev/.copilot")));
        assert_eq!(got[1].path, Some(PathBuf::from("/home/dev/.cache/copilot")));
    }

    #[test]
    fn home_and_cache_overrides_are_independent() {
        let mut env_vars = HashMap::new();
        env_vars.insert("COPILOT_HOME".to_string(), "/opt/copilot-home".to_string());
        env_vars.insert(
            "COPILOT_CACHE_HOME".to_string(),
            "/opt/copilot-cache".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = CopilotCliDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/copilot-home")));
        assert_eq!(got[1].path, Some(PathBuf::from("/opt/copilot-cache")));
    }
}
