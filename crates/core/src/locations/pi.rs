//! Pi (badlogic/pi-mono's `coding-agent` package, also published as
//! `earendil-works/pi`) home: `~/.pi/agent`, or the whole of
//! `PI_CODING_AGENT_DIR` when set. **Distinct tool from Oh My Pi**
//! (`crate::locations::oh_my_pi`), which is a fork of this one -- do not
//! conflate the two identities, even though both currently document the
//! very same override variable name (see below).
//!
//! Sourced from primary docs during implementation (never a real
//! `~/.pi` on this machine -- PRIVACY IS A HARD RULE), from
//! <https://github.com/badlogic/pi-mono>, current `main` as of this
//! chunk:
//! - `packages/coding-agent/README.md`: "Sessions are automatically
//!   saved to `~/.pi/agent/sessions/`, organized by working directory,"
//!   overridable independently via `--session-dir` or
//!   `PI_CODING_AGENT_SESSION_DIR`; `PI_CODING_AGENT_DIR` "overrides the
//!   main config directory (defaults to `~/.pi/agent`)"; models via
//!   `~/.pi/agent/models.json`; provider packages relocatable via
//!   `PI_PACKAGE_DIR`.
//! - `packages/coding-agent/docs/settings.md`: global settings at
//!   `~/.pi/agent/settings.json` and trust decisions at
//!   `~/.pi/agent/trust.json`; user-scoped npm package installs at
//!   `~/.pi/agent/npm/`.
//!
//! Named collision risk this chunk found and is honest about: Pi's own
//! README documents the override as `PI_CODING_AGENT_DIR`, the *same*
//! name Oh My Pi's own docs use for its own agent-directory override
//! (`crate::locations::oh_my_pi`'s doc comment). If a human sets this
//! variable while both tools are installed, both detectors resolve to
//! the *same* path, and `crate::agents::pi`/`crate::agents::oh_my_pi`
//! would both attempt to identify the same directory's interior under
//! two different tool ids -- a real, disclosed limitation, not silently
//! deduplicated (this chunk found no reliable way to tell which tool
//! actually owns an overridden directory without reading its content,
//! which `crate::agents::pi`'s own format-marker check partially
//! mitigates: Pi's own layout has no session title-slot header, unlike
//! Oh My Pi's, so the two rarely both match).
//!
//! The issue text (`#96`) names this override `PI_AGENT_DIR`; this
//! chunk's own primary-source read of the current `README.md` found
//! `PI_CODING_AGENT_DIR` instead (matching Oh My Pi's own documented
//! name) and honors that verified name, not the issue's paraphrase.
//! `PI_AGENT_DIR` is honored too, defensively, as a secondary override
//! in case an older/different released version used it, but is flagged
//! as unconfirmed.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const PI_DETECTOR_ID: &str = "pi";

pub struct PiDetector;

impl Detector for PiDetector {
    fn id(&self) -> &'static str {
        PI_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Pi"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "github.com/badlogic/pi-mono packages/coding-agent/README.md and docs/settings.md, \
         current main as of this chunk; PI_CODING_AGENT_DIR is confirmed by primary source and \
         shared with Oh My Pi's own documented override -- see this module's doc comment for the \
         disclosed collision risk. PI_AGENT_DIR (the issue text's name) is honored defensively \
         but is unconfirmed"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env
            .env_var("PI_CODING_AGENT_DIR")
            .or_else(|| env.env_var("PI_AGENT_DIR"))
        {
            Some(v) if !v.is_empty() => {
                let var = if env.env_var("PI_CODING_AGENT_DIR").is_some() {
                    "PI_CODING_AGENT_DIR"
                } else {
                    "PI_AGENT_DIR"
                };
                (
                    std::path::PathBuf::from(v),
                    Provenance::EnvVar(var.to_string()),
                )
            }
            _ => (
                env.home.join(".pi").join("agent"),
                Provenance::BuiltinConvention,
            ),
        };
        vec![ProposedLocation {
            detector_id: PI_DETECTOR_ID.to_string(),
            path: Some(base),
            category: StorageCategory::LocalState,
            provenance,
            status: LocationStatus::Resolved,
            note: Some(
                "Pi agent directory: sessions/ (organized by working directory), settings.json, \
                 trust.json, models.json, npm/ package installs; see crate::agents::pi for \
                 interior identification and its format-marker disambiguation from Oh My Pi"
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
    fn convention_path_no_override() {
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        let got = PiDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.pi/agent")));
    }

    #[test]
    fn primary_env_override_wins() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "PI_CODING_AGENT_DIR".to_string(),
            "/opt/pi-agent".to_string(),
        );
        env_vars.insert("PI_AGENT_DIR".to_string(), "/opt/other".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = PiDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/pi-agent")));
    }

    #[test]
    fn secondary_override_used_when_primary_absent() {
        let mut env_vars = HashMap::new();
        env_vars.insert("PI_AGENT_DIR".to_string(), "/opt/pi-agent-2".to_string());
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = PiDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/pi-agent-2")));
    }
}
