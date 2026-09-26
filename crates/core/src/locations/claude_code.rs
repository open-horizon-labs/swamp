//! Claude Code home: `CLAUDE_CONFIG_DIR` override, else `~/.claude`
//! (Anthropic's own "Explore the .claude directory" documentation states
//! the override explicitly: "Personal config: `~/.claude/` (or
//! `$CLAUDE_CONFIG_DIR` if set)"). https://code.claude.com/docs/en/claude-directory
//!
//! This detector resolves the *home directory itself* as one external
//! unit (#43's existing identity/history contract, reused verbatim, no
//! new mechanism). Its *interior* -- sessions, caches, logs, checkpoints,
//! protected config -- is identified into finer-grained `AgentUnit`s by
//! `crate::agents::claude_code` (#91/#92), which reuses the very same
//! `(detector_id, category, device, path)` growth-store key family under
//! per-category/per-session keys distinct from this home-level key.
//!
//! One documented gap: Claude Code also keeps `~/.claude.json` as a
//! *sibling* of the `~/.claude/` directory itself (not inside it), per
//! the same documentation page's "Global Configuration" table. An
//! external/agent unit's identity is a canonical relative path *under*
//! the tool home (see `crate::artifact` and `crate::agents::AgentUnit`
//! doc comments); a file living beside, not under, the home does not fit
//! that identity model. It is not measured by this detector or by
//! `crate::agents::claude_code`, and is recorded as an explicit,
//! documented unknown in `docs/agent-storage.md` rather than forced into
//! a model that does not fit it.

use super::{
    Detector, Environment, LocationStatus, Platform, ProposedLocation, Provenance, StorageCategory,
};

pub const CLAUDE_CODE_DETECTOR_ID: &str = "claude-code";

pub struct ClaudeCodeDetector;

impl Detector for ClaudeCodeDetector {
    fn id(&self) -> &'static str {
        CLAUDE_CODE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Claude Code"
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS, Platform::Linux]
    }

    fn version_note(&self) -> &'static str {
        "code.claude.com/docs/en/claude-directory, current documented layout; internal transcript \
         schema is explicitly undocumented/unstable upstream -- see crate::agents::claude_code"
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        let (base, provenance) = match env.env_var("CLAUDE_CONFIG_DIR") {
            Some(v) if !v.is_empty() => (
                std::path::PathBuf::from(v),
                Provenance::EnvVar("CLAUDE_CONFIG_DIR".to_string()),
            ),
            _ => (env.home.join(".claude"), Provenance::BuiltinConvention),
        };
        vec![ProposedLocation {
            detector_id: CLAUDE_CODE_DETECTOR_ID.to_string(),
            path: Some(base),
            category: StorageCategory::LocalState,
            provenance,
            status: LocationStatus::Resolved,
            note: Some(
                "Claude Code home: sessions, caches, logs, checkpoints and protected config; \
                 see crate::agents::claude_code for the interior identification"
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
        let got = ClaudeCodeDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, Some(PathBuf::from("/Users/dev/.claude")));
        assert!(matches!(got[0].provenance, Provenance::BuiltinConvention));
        assert_eq!(got[0].status, LocationStatus::Resolved);
    }

    #[test]
    fn env_var_override_wins_and_is_labelled() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            "/opt/claude-home".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = ClaudeCodeDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/opt/claude-home")));
        assert_eq!(
            got[0].provenance,
            Provenance::EnvVar("CLAUDE_CONFIG_DIR".to_string())
        );
    }

    #[test]
    fn leftover_found_without_executable_present() {
        // Same convention-probe discipline as CargoHomeDetector: the
        // detector never checks whether `claude` is on PATH.
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::Linux);
        let got = ClaudeCodeDetector.detect(&env);
        assert!(got.iter().all(|l| l.status == LocationStatus::Resolved));
    }

    #[test]
    fn linux_also_proposes_the_convention_path() {
        // Claude Code ships cross-platform; unlike `builtin-defaults`,
        // this detector's convention path is not macOS-specific.
        let env = Environment::fixture(PathBuf::from("/home/dev"), HashMap::new(), Platform::Linux);
        let got = ClaudeCodeDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/home/dev/.claude")));
    }
}
