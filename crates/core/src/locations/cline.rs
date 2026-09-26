//! Cline (`saoudrizwan.claude-dev`) VS Code extension storage: one
//! `globalStorage/saoudrizwan.claude-dev/` location per known editor
//! host (`crate::locations::vscode_hosts`), never a single merged path
//! -- the same extension ID installed in VS Code, VS Code Insiders,
//! Cursor and Windsurf each has its own, genuinely separate, on-disk
//! storage.
//!
//! Since 2026-09-22 there is a **second, shared root**: Cline 4.x keeps
//! `globalState.json`, `secrets.json` and `workspaces/<hash>/` in a
//! file-backed store outside any editor's globalStorage, shared by the
//! VS Code, CLI and JetBrains clients. Upstream resolves it as
//! `CLINE_DATA_DIR`, else `CLINE_DIR + "/data"`, else `~/.cline/data`
//! (`apps/vscode/src/shared/storage/storage-context.ts:93-100` and
//! `sdk/packages/shared/src/storage/paths.ts:151-186` @
//! `254f40c4b592d1e662b84f2ba06fe45dca77cab3`, vendored under
//! `crates/core/tests/fixtures/upstream/cline/254f40c4b5/`). It is
//! modelled here because the bytes are real; what it does **not** hold
//! is the VS Code host's task history --
//! `vscode-to-file-migration.ts:25-28` states that for VS Code
//! `globalStorageFsPath` "is still the VSCode-managed path (not
//! ~/.cline/data/)", which is why linkage is read from the globalStorage
//! location instead (`crate::agents::cline`).
//!
//! Layout provenance for the per-task shape is a mix of upstream source
//! and community threads (checked during implementation, never learned
//! from a real Cline installation on this machine -- PRIVACY IS A HARD
//! RULE):
//! - `apps/vscode/src/sdk/legacy-state-reader.ts:53-74` @ the commit
//!   above: the per-task files `api_conversation_history.json`,
//!   `ui_messages.json`, `context_history.json`, `task_metadata.json`
//!   under `tasks/<task-id>/`.
//! - <https://github.com/cline/cline/issues/7101>: confirms data lives
//!   under the extension's `globalStorage` directory and that corrupted
//!   JSON task-history files have historically been silently deleted --
//!   named directly in #99's own guardrail text as a reason this
//!   adapter never touches a task file's *contents*, only its metadata
//!   shape.
//! - <https://github.com/cline/cline/issues/14135>: further corroborates
//!   the `tasks/<task-id>/` per-task directory shape.

use super::vscode_hosts::globalstorage_candidates;
use super::{Detector, Environment, LocationStatus, Platform, ProposedLocation, StorageCategory};

pub const CLINE_DETECTOR_ID: &str = "cline";
pub const CLINE_EXTENSION_ID: &str = "saoudrizwan.claude-dev";

pub struct ClineDetector;

impl Detector for ClineDetector {
    fn id(&self) -> &'static str {
        CLINE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Cline"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "community-documented (github.com/cline/cline issues #7101, #14135), no official \
         layout-reference doc found this chunk; macOS hosts only, Linux deferred to the Linux \
         track (#77-#89); see crate::agents::vscode_family"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let mut out: Vec<ProposedLocation> =
            globalstorage_candidates(&env.home, CLINE_EXTENSION_ID)
                .into_iter()
                .map(|(host, path)| ProposedLocation {
                    detector_id: CLINE_DETECTOR_ID.to_string(),
                    path: Some(path),
                    category: StorageCategory::LocalState,
                    provenance: super::Provenance::BuiltinConvention,
                    status: LocationStatus::Resolved,
                    note: Some(format!(
                        "Cline globalStorage under the {host} host: tasks/<task-id>/ (conversation \
                     history, UI messages, checkpoints) and state/taskHistory.json; see \
                     crate::agents::vscode_family"
                    )),
                })
                .collect();
        // The shared file-backed store, resolved exactly as upstream
        // resolves it: CLINE_DATA_DIR > CLINE_DIR/data > ~/.cline/data.
        let (data_dir, provenance) = match env.env_var("CLINE_DATA_DIR") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                super::Provenance::EnvVar("CLINE_DATA_DIR".to_string()),
            ),
            _ => match env.env_var("CLINE_DIR") {
                Some(v) if !v.is_empty() => (
                    std::path::PathBuf::from(v).join("data"),
                    super::Provenance::EnvVar("CLINE_DIR".to_string()),
                ),
                _ => (
                    env.home.join(".cline").join("data"),
                    super::Provenance::BuiltinConvention,
                ),
            },
        };
        out.push(ProposedLocation {
            detector_id: CLINE_DETECTOR_ID.to_string(),
            path: Some(data_dir),
            category: StorageCategory::LocalState,
            provenance,
            status: LocationStatus::Resolved,
            note: Some(
                "Cline shared file-backed store (CLINE_DATA_DIR, else CLINE_DIR/data, else \
                 ~/.cline/data): globalState.json, secrets.json, workspaces/<hash>/, shared by \
                 the VS Code, CLI and JetBrains clients. Upstream states this root does NOT \
                 hold the VS Code host's task history"
                    .to_string(),
            ),
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn one_location_per_known_host() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = ClineDetector.detect(&env);
        assert_eq!(got.len(), 6, "five editor hosts plus the shared data root");
        assert_eq!(
            got.last().unwrap().path,
            Some(PathBuf::from("/Users/dev/.cline/data")),
            "the shared file-backed store is a root of its own"
        );
        assert!(got.iter().any(|l| {
            l.path
                == Some(PathBuf::from(
                    "/Users/dev/Library/Application Support/Code/User/globalStorage/\
                     saoudrizwan.claude-dev",
                ))
        }));
        assert!(got.iter().any(|l| {
            l.path
                == Some(PathBuf::from(
                    "/Users/dev/Library/Application Support/Cursor/User/globalStorage/\
                     saoudrizwan.claude-dev",
                ))
        }));
    }

    /// The shared root's own resolution order, exactly as upstream's
    /// `resolveDataDirFromEnv` spells it.
    #[test]
    fn the_shared_data_root_follows_upstreams_override_order() {
        let cases = [
            (
                HashMap::from([("CLINE_DATA_DIR".to_string(), "/elsewhere/data".to_string())]),
                "/elsewhere/data",
            ),
            (
                HashMap::from([("CLINE_DIR".to_string(), "/opt/cline".to_string())]),
                "/opt/cline/data",
            ),
            (
                // CLINE_DATA_DIR wins over CLINE_DIR.
                HashMap::from([
                    ("CLINE_DATA_DIR".to_string(), "/wins".to_string()),
                    ("CLINE_DIR".to_string(), "/loses".to_string()),
                ]),
                "/wins",
            ),
            (HashMap::new(), "/Users/dev/.cline/data"),
        ];
        for (env_vars, expected) in cases {
            let env = Environment::fixture(
                PathBuf::from("/Users/dev"),
                env_vars.clone(),
                Platform::MacOS,
            );
            let got = ClineDetector.detect(&env);
            assert_eq!(
                got.last().unwrap().path,
                Some(PathBuf::from(expected)),
                "{env_vars:?}"
            );
        }
    }
}
