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
    BuildStoreDecl, BuildStoreKind, Detector, Environment, LocationStatus, Platform,
    ProposedLocation, Provenance, StorageCategory, StoreAnchor,
};

pub const CLAUDE_CODE_DETECTOR_ID: &str = "claude-code";
pub const CLAUDE_CODE_SCRATCH_DETECTOR_ID: &str = "claude-code-scratch";

pub struct ClaudeCodeDetector;

impl Detector for ClaudeCodeDetector {
    fn id(&self) -> &'static str {
        CLAUDE_CODE_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Claude Code"
    }

    fn headline_group(&self) -> Option<super::HeadlineGroup> {
        Some(super::HeadlineGroup::AgentStorage)
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

/// Claude Code's per-user session scratch directory,
/// `/private/tmp/claude-<uid>` on macOS: one exact, known path per user,
/// measured as its own external unit.
///
/// It is a **separate detector from [`ClaudeCodeDetector`]** on purpose.
/// The agent layer takes a tool's *first* authorized location as that
/// tool's home and identifies sessions inside it; a second location on
/// the `claude-code` detector would become the home whenever `~/.claude`
/// is excluded or absent, and its scratch files would be read as a
/// session store. Nothing else under `/private/tmp` is looked at: there
/// is no discovery of temp directories, only this one named path.
///
/// macOS only. The Linux equivalent was not observed on any machine
/// this was built on, so it is not guessed; the platform table reports
/// the detector as not applicable there.
pub struct ClaudeCodeScratchDetector;

impl Detector for ClaudeCodeScratchDetector {
    fn id(&self) -> &'static str {
        CLAUDE_CODE_SCRATCH_DETECTOR_ID
    }

    fn name(&self) -> &'static str {
        "Claude Code session scratch"
    }

    fn headline_group(&self) -> Option<super::HeadlineGroup> {
        Some(super::HeadlineGroup::AgentStorage)
    }

    fn platforms(&self) -> &'static [Platform] {
        &[Platform::MacOS]
    }

    fn version_note(&self) -> &'static str {
        "observed layout: /private/tmp/claude-<uid> holds per-project session scratch directories \
         (macOS); not documented upstream"
    }

    fn build_stores(&self) -> &'static [BuildStoreDecl] {
        &[BuildStoreDecl {
            kind: BuildStoreKind::AgentScratch,
            anchor: StoreAnchor::Categorized {
                category: StorageCategory::Cache,
                suffix: &[],
            },
        }]
    }

    fn detect(&self, env: &Environment) -> Vec<ProposedLocation> {
        vec![ProposedLocation {
            detector_id: CLAUDE_CODE_SCRATCH_DETECTOR_ID.to_string(),
            path: Some(std::path::PathBuf::from(format!(
                "/private/tmp/claude-{}",
                env.uid
            ))),
            category: StorageCategory::Cache,
            provenance: Provenance::BuiltinConvention,
            status: LocationStatus::Resolved,
            note: Some(
                "Claude Code's per-user session scratch; on the machine this was built on, every entry \
                 under /private/tmp was newer than the last boot (swamp does not assume it is \
                 cleared)"
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

    #[test]
    fn scratch_is_the_one_exact_per_user_directory() {
        let mut env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        env.uid = 777;
        let got = ClaudeCodeScratchDetector.detect(&env);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].path, Some(PathBuf::from("/private/tmp/claude-777")));
        assert_eq!(got[0].category, StorageCategory::Cache);
        assert_eq!(got[0].status, LocationStatus::Resolved);
    }

    #[test]
    fn scratch_ignores_home_and_claude_config_dir() {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "CLAUDE_CONFIG_DIR".to_string(),
            "/opt/claude-home".to_string(),
        );
        let env = Environment::fixture(PathBuf::from("/Users/dev"), env_vars, Platform::MacOS);
        let got = ClaudeCodeScratchDetector.detect(&env);
        assert_eq!(got[0].path, Some(PathBuf::from("/private/tmp/claude-501")));
    }

    #[test]
    fn scratch_is_macos_only_and_never_a_claude_code_home() {
        assert_eq!(ClaudeCodeScratchDetector.platforms(), &[Platform::MacOS]);
        // The home detector's own locations stay exactly one, so the
        // agent layer's first-location contract is untouched.
        let env =
            Environment::fixture(PathBuf::from("/Users/dev"), HashMap::new(), Platform::MacOS);
        assert_eq!(ClaudeCodeDetector.detect(&env).len(), 1);
        assert_ne!(ClaudeCodeScratchDetector.id(), ClaudeCodeDetector.id());
    }
}
