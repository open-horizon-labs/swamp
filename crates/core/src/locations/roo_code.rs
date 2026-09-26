//! Roo Code (`rooveterinaryinc.roo-cline`) VS Code extension storage:
//! one `globalStorage/rooveterinaryinc.roo-cline/` location per known
//! editor host, same discipline as `crate::locations::cline`.
//!
//! Community-documented (checked during implementation, never learned
//! from a real Roo Code installation on this machine -- PRIVACY IS A
//! HARD RULE):
//! - <https://github.com/RooCodeInc/Roo-Code/issues/4174>: reports the
//!   remote/server hosting path
//!   `~/.vscode-server/data/User/globalStorage/rooveterinaryinc.roo-cline/tasks`
//!   directly (confirming `crate::locations::vscode_hosts`'s remote
//!   entry is the right shape for this extension too), per-task UUID
//!   directories with real-world sizes up to 176 GB for a single task,
//!   and that each task directory has been reported to embed a full Git
//!   checkpoint repository -- flagged here, as the issue itself asks,
//!   because it means a Roo Code task is not necessarily small the way
//!   a Claude Code session usually is; `crate::agents::vscode_family`
//!   still folds a task directory's bytes rather than assuming a size
//!   bound.

use super::vscode_hosts::globalstorage_candidates;
use super::{Detector, Environment, LocationStatus, Platform, ProposedLocation, StorageCategory};

pub const ROO_CODE_DETECTOR_ID: &str = "roo-code";
pub const ROO_CODE_EXTENSION_ID: &str = "rooveterinaryinc.roo-cline";

pub struct RooCodeDetector;

impl Detector for RooCodeDetector {
    fn id(&self) -> &'static str {
        ROO_CODE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Roo Code"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "community-documented (github.com/RooCodeInc/Roo-Code issue #4174), no official \
         layout-reference doc found this chunk; a task directory can embed a full Git checkpoint \
         repo and is not necessarily small -- see crate::agents::vscode_family"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        globalstorage_candidates(&env.home, ROO_CODE_EXTENSION_ID)
            .into_iter()
            .map(|(host, path)| ProposedLocation {
                detector_id: ROO_CODE_DETECTOR_ID.to_string(),
                path: Some(path),
                category: StorageCategory::LocalState,
                provenance: super::Provenance::BuiltinConvention,
                status: LocationStatus::Resolved,
                note: Some(format!(
                    "Roo Code globalStorage under the {host} host: tasks/<task-id>/ (conversation \
                     history, UI messages, possibly a full Git checkpoint repo); see \
                     crate::agents::vscode_family"
                )),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn remote_host_path_matches_the_issue_report() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = RooCodeDetector.detect(&env);
        assert!(got.iter().any(|l| {
            l.path
                == Some(PathBuf::from(
                    "/Users/dev/.vscode-server/data/User/globalStorage/\
                     rooveterinaryinc.roo-cline",
                ))
        }));
    }
}
