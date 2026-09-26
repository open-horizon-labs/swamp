//! Codex desktop app identification (#93): logs only. See
//! `crate::locations::codex_desktop` for why this adapter deliberately
//! covers less than `crate::agents::codex` does -- the desktop app's
//! settings/session storage is not confirmed by primary source this
//! chunk, and this adapter says so rather than guessing.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentUnitBuilder,
    CandidateAgentUnit, IdentifyCtx,
};
use std::path::Path;

pub const CODEX_DESKTOP_TOOL_ID: &str = "codex-desktop";

const MAX_FOLD_ENTRIES: usize = 200_000;

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        CODEX_DESKTOP_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Codex desktop app"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        identify(home, ctx)
    }
}

/// `home` here is the *log directory itself*
/// (`~/Library/Logs/com.openai.codex`), not a tool home with an interior
/// to decompose further -- there is nothing else confirmed under it.
/// The whole directory is one folded, actionable Logs-category unit,
/// same recovery contract as any other tool's log directory.
pub fn identify(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    if !ctx.exists(home) {
        return Vec::new();
    }
    // A freshly created, still-empty log directory is not yet worth a
    // row (see the identical note in `oh_my_pi::unknown_format_residual`
    // on why an entry check, not the folded mtime, decides this).
    if !ctx.has_entries(home) {
        return Vec::new();
    }
    let (bytes, mtime, truncated) = ctx.folded_bytes(home, MAX_FOLD_ENTRIES);
    let unit = AgentUnitBuilder::new(
        CODEX_DESKTOP_TOOL_ID,
        AgentCategory::Logs,
        home.to_path_buf(),
    )
    .relative_path("(log directory)")
    .bytes(bytes)
    .mtime_max(mtime)
    .action(AgentActionCapability::CacheOrLogTrash)
    .note(
        "desktop app log directory (date-tree session logs); settings/session storage beyond \
         logs is an unconfirmed layout, not modeled here",
    );
    let unit = if truncated {
        unit.incomplete("directory entry count bound reached, total may be an undercount")
    } else {
        unit
    };
    vec![unit.build()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{IdentificationCache, contract};
    use std::fs;

    fn run(home: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify(home, &IdentifyCtx::new(1, &cache))
    }

    #[test]
    fn missing_directory_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(&dir.path().join("does-not-exist")).is_empty());
    }

    #[test]
    fn log_files_are_folded_into_one_actionable_unit() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/09/21");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("session.log"), b"line 1\nline 2\n").unwrap();
        let units = run(dir.path());
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].category(), AgentCategory::Logs);
        assert_eq!(units[0].action(), AgentActionCapability::CacheOrLogTrash);
        assert!(!units[0].protected());
        assert!(units[0].bytes() > 0);
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        // This adapter is handed a confirmed log directory and models
        // nothing else, so "unknown format" here means: content that is
        // not a date tree is still reported, with a note saying what is
        // and is not modeled -- never silently dropped, and never
        // re-interpreted as some other Codex layout.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("something-else.bin"), b"\x00\x01").unwrap();
        let units = run(dir.path());
        assert_eq!(units.len(), 1, "unrecognized content must still surface");
        let note = units[0].note.as_deref().unwrap_or_default();
        assert!(
            note.contains("unconfirmed layout"),
            "the unit must say what is not modeled: {note}"
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-CODEX-DESKTOP-DO-NOT-LEAK-7f31";
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/09/21");
        fs::create_dir_all(&day).unwrap();
        fs::write(
            day.join("session.log"),
            format!("user said: {canary}\n").as_bytes(),
        )
        .unwrap();
        contract::no_content_leak(&run(dir.path()), canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/09/21");
        fs::create_dir_all(&day).unwrap();
        for i in 0..50 {
            fs::write(day.join(format!("s{i}.log")), vec![b'x'; 20_000]).unwrap();
        }
        let (units, counters) = contract::measured(|| run(dir.path()));
        assert_eq!(units.len(), 1);
        // This adapter reads no file contents at all: its identification
        // is `stat` plus one bounded listing per directory.
        assert_eq!(
            counters.header_bytes_read, 0,
            "a log directory is measured, never read"
        );
        contract::within_header_cap(counters, 0);
    }

    #[test]
    fn protected_categories_default_protected() {
        // This adapter models exactly one confirmed directory and it is
        // a log directory, so there is no default-protected category in
        // its output; the shared assertion is exercised against a unit
        // built through this module's own construction path, so the
        // builder's guarantee is still proven here.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.log"), b"x").unwrap();
        let units = run(dir.path());
        assert!(
            units.iter().all(|u| !u.category().default_protected()),
            "this adapter is documented as modeling logs only"
        );
        let config = AgentUnitBuilder::new(
            CODEX_DESKTOP_TOOL_ID,
            AgentCategory::ProtectedConfig,
            dir.path().join("settings.json"),
        )
        .build();
        contract::protection_defaults_hold(&[config]);
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        // A log directory is tool-wide: `NotApplicable` is the honest
        // answer, and a subdirectory named after a repo must not turn
        // into a link.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("my-repo-name");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("a.log"), b"x").unwrap();
        let units = run(dir.path());
        assert!(matches!(
            units[0].project_link(),
            crate::agents::ProjectLinkState::NotApplicable
        ));
        contract::linkage_is_declared_or_explicit(&units, "my-repo-name");
    }
}
