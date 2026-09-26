//! Oh My Pi (user-confirmed identity: a fork of badlogic/pi-mono's `pi`
//! coding agent) agent-storage home.
//!
//! Sourced from Oh My Pi's own docs (fetched during implementation, never
//! from a real `~/.omp` on this machine -- PRIVACY IS A HARD RULE):
//! - <https://github.com/can1357/oh-my-pi/blob/main/docs/settings.md>:
//!   "The primary agent directory is `~/.omp/agent/` by default, but can
//!   be relocated using the `PI_CODING_AGENT_DIR` environment variable.
//!   When set, this variable moves the global config file, auth store,
//!   and entire agent directory together." Main config:
//!   `~/.omp/agent/config.yml`; auth: `agent.db`, under the agent
//!   directory.
//! - <https://github.com/can1357/oh-my-pi/blob/main/docs/session.md>:
//!   sessions under `~/.omp/agent/sessions/`, a content-addressed blob
//!   store at `~/.omp/agent/blobs/<sha256>`, terminal breadcrumbs at
//!   `~/.omp/agent/terminal-sessions/`.
//!
//! This detector resolves the **agent directory itself**
//! (`~/.omp/agent`, or the whole of `PI_CODING_AGENT_DIR` when set --
//! upstream's own text says the override "moves ... the entire agent
//! directory", so the override path *is* the agent directory, not a
//! parent of it) as one external unit, same pattern as
//! `crate::locations::claude_code`. Its interior is identified by
//! `crate::agents::oh_my_pi` (#94).
//!
//! Named disambiguation risk (#94's explicit acceptance): `~/.omp` is
//! also a plausible home for unrelated tools (the issue names
//! oh-my-posh as one to check). Resolving the **agent subdirectory**
//! specifically, rather than the bare `~/.omp` wrapper, already avoids
//! most of that collision (oh-my-posh has no reason to create an
//! `agent/` subdirectory under `~/.omp`), but `crate::agents::oh_my_pi`
//! additionally verifies content markers before treating anything under
//! this path as Oh My Pi storage, and reports an explicit unknown-format
//! residual instead of guessing when they are absent -- defense in
//! depth, not reliance on the path shape alone.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const OH_MY_PI_DETECTOR_ID: &str = "oh-my-pi";

pub struct OhMyPiDetector;

impl Detector for OhMyPiDetector {
    fn id(&self) -> &'static str {
        OH_MY_PI_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Oh My Pi"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "github.com/can1357/oh-my-pi docs/settings.md and docs/session.md, current main as of \
         this chunk; see crate::agents::oh_my_pi for the interior identification and its \
         content-marker disambiguation from unrelated ~/.omp users"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("PI_CODING_AGENT_DIR") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("PI_CODING_AGENT_DIR".to_string()),
            ),
            _ => (
                env.home.join(".omp").join("agent"),
                Provenance::BuiltinConvention,
            ),
        };
        vec![ProposedLocation {
            detector_id: OH_MY_PI_DETECTOR_ID.to_string(),
            path: Some(base),
            category: StorageCategory::LocalState,
            provenance,
            status: LocationStatus::Resolved,
            note: Some(
                "Oh My Pi agent directory: sessions, content-addressed blobs, terminal \
                 breadcrumbs and protected config; see crate::agents::oh_my_pi for the interior \
                 identification and its unknown-format fallback"
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
        let got = OhMyPiDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.omp/agent")));
        assert!(matches!(got[0].provenance, Provenance::BuiltinConvention));
    }

    #[test]
    fn env_override_replaces_the_whole_agent_directory() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "PI_CODING_AGENT_DIR".to_string(),
            "/opt/omp-agent".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = OhMyPiDetector.detect(&env);
        // Not `.../agent` appended again: the override already names the
        // agent directory itself, per upstream's own text.
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/omp-agent")));
    }

    #[test]
    fn linux_also_proposes_the_convention_path() {
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = OhMyPiDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/home/dev/.omp/agent")));
    }
}
