//! Aider identification (#96): a home-level, wholly re-downloadable
//! cache root under `~/.aider` (`crate::locations::aider::AiderDetector`
//! resolves the home), and -- materially different from every other
//! adapter in this catalog -- per-repo history/tags-cache files that
//! live **inside each project checkout** rather than under any tool
//! home. See `crate::locations::aider`'s doc comment for the full
//! primary-source citations
//! (`aider/models.py`/`versioncheck.py`/`args.py`/`repomap.py`, current
//! `main` of <https://github.com/Aider-AI/aider> as of this chunk).
//!
//! [`identify`] covers the home: `caches/` (regenerable) and, if
//! present, a home-level `.aider.conf.yml`. [`identify_repo_units`]
//! covers the per-repo files and is reached through
//! [`AgentAdapter::project_local_units`] -- this adapter is the one that
//! declares [`AdapterCapabilities::project_local_units`], so the shared
//! layer calls it once per **known project worktree root** instead of
//! keeping a bespoke call path for one tool. That is #96's explicit
//! acceptance ("attach to the existing worktree artifact model as an
//! agent category, not a tool-home unit"). A worktree root is itself the
//! "declared path" here (the caller already knows it is a real
//! checkout), so project linkage reuses `resolve_declared_path` against
//! that same root rather than inventing a second mechanism.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, mtime_secs,
    resolve_declared_path,
};
use std::path::Path;

pub const AIDER_TOOL_ID: &str = "aider";

const MAX_FOLD_ENTRIES: usize = 200_000;

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        AIDER_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Aider"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            project_local_units: true,
            ..Default::default()
        }
    }
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        identify(home, ctx)
    }
    fn project_local_units(
        &self,
        worktree_root: &Path,
        ctx: &IdentifyCtx,
    ) -> Vec<CandidateAgentUnit> {
        identify_repo_units(worktree_root, ctx)
    }
}

// ---------------------------------------------------------------------
// Home-level: ~/.aider/caches (+ an optional home-level .aider.conf.yml).
// ---------------------------------------------------------------------

pub fn identify(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    if !ctx.is_dir(home) {
        return Vec::new();
    }
    let mut units = Vec::new();

    let caches = home.join("caches");
    let caches_existed = ctx.exists(&caches);
    if ctx.is_dir(&caches) {
        let (bytes, mtime, truncated) = ctx.folded_bytes(&caches, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new(AIDER_TOOL_ID, AgentCategory::Caches, caches)
            .relative_path("caches")
            .bytes(bytes)
            .mtime_max(mtime)
            .action(AgentActionCapability::CacheOrLogTrash)
            .note("model-price/context-window and version-check caches, wholly re-downloadable");
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        units.push(unit.build());
    }

    let conf = home.join(".aider.conf.yml");
    if let Ok(meta) = ctx.stat(&conf)
        && meta.is_file()
    {
        units.push(
            AgentUnitBuilder::new(AIDER_TOOL_ID, AgentCategory::ProtectedConfig, conf)
                .relative_path(".aider.conf.yml")
                .bytes(meta.len())
                .mtime_max(mtime_secs(&meta))
                .protect("home-level Aider configuration")
                .action(AgentActionCapability::None)
                .build(),
        );
    }

    if !caches_existed && !ctx.exists(&home.join(".aider.conf.yml")) {
        // A genuinely empty/unrelated ~/.aider is not "unsupported
        // version" the way an ambiguous ~/.omp or ~/.opencode-shaped
        // directory would be -- Aider has no other documented top-level
        // marker this chunk found, so there is nothing to classify, not
        // an unknown format to flag.
        if ctx.has_entries(home) {
            let (bytes, mtime, _t) = ctx.folded_bytes(home, MAX_FOLD_ENTRIES);
            units.push(
                AgentUnitBuilder::new(
                    AIDER_TOOL_ID,
                    AgentCategory::Unclassified,
                    home.to_path_buf(),
                )
                .relative_path("(unclassified residual)")
                .bytes(bytes)
                .mtime_max(mtime)
                .action(AgentActionCapability::None)
                .note("no recognized Aider home markers (caches/, .aider.conf.yml) found here")
                .build(),
            );
        }
    }
    units
}

// ---------------------------------------------------------------------
// Per-repo: identified against a known worktree root, not a tool home.
// ---------------------------------------------------------------------

/// The three per-repo Aider files/directories this chunk's primary-source
/// research confirmed, checked directly at `worktree_root` (never a
/// recursive search -- these files live exactly at the git root per
/// `aider/args.py`/`repomap.py`).
pub fn identify_repo_units(worktree_root: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    if !ctx.is_dir(worktree_root) {
        return Vec::new();
    }
    let project_link = resolve_declared_path(
        Some(worktree_root.display().to_string()),
        "worktree root supplied by the caller was not itself resolvable",
    );
    let mut units = Vec::new();

    for (rel, note) in [
        (
            ".aider.chat.history.md",
            "unique chat transcript for this checkout; not regenerated by re-running Aider",
        ),
        (
            ".aider.input.history",
            "unique input-line history for this checkout; not regenerated by re-running Aider",
        ),
    ] {
        let path = worktree_root.join(rel);
        if let Ok(meta) = ctx.stat(&path)
            && meta.is_file()
        {
            units.push(
                AgentUnitBuilder::new(AIDER_TOOL_ID, AgentCategory::Sessions, path.clone())
                    .relative_path(rel)
                    .members(vec![AgentMember {
                        path,
                        bytes: meta.len(),
                        kind: AgentMemberKind::Transcript,
                    }])
                    .mtime_max(mtime_secs(&meta))
                    .project_link(project_link.clone())
                    .action(AgentActionCapability::SessionRemoval)
                    .note(note)
                    .build(),
            );
        }
    }

    // `.aider.tags.cache.v{3,4}` -- the version number is a repomap
    // implementation detail (whether the optional TSL pack is in use),
    // checked directly rather than guessed at one fixed number, so the
    // prefix match is deliberately not pinned to a version suffix.
    for name in ctx.dir_names(worktree_root) {
        if !name.starts_with(".aider.tags.cache.v") {
            continue;
        }
        let path = worktree_root.join(&name);
        let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new(AIDER_TOOL_ID, AgentCategory::Caches, path)
            .relative_path(name)
            .bytes(bytes)
            .mtime_max(mtime)
            .project_link(project_link.clone())
            .action(AgentActionCapability::CacheOrLogTrash)
            .note("repo-map tags cache, regenerated on next Aider run");
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        units.push(unit.build());
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{IdentificationCache, LinkSource, ProjectLinkState, contract};
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn run(home: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify(home, &IdentifyCtx::new(1, &cache))
    }

    fn run_repo(worktree_root: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify_repo_units(worktree_root, &IdentifyCtx::new(1, &cache))
    }

    fn touch(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn empty_home_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path()).is_empty());
    }

    #[test]
    fn caches_dir_is_actionable() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(
            &home.join("caches/model_prices_and_context_window.json"),
            b"{}",
        );
        touch(&home.join("caches/versioncheck"), b"");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "caches")
            .unwrap();
        assert!(!u.protected());
        assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn home_conf_is_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join(".aider.conf.yml"), b"dark-mode: true");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == ".aider.conf.yml")
            .unwrap();
        assert!(u.protected());
    }

    #[test]
    fn repo_units_are_not_identified_without_a_known_worktree_root() {
        // identify() (home-level) must never reach into a project
        // checkout for these files -- that is identify_repo_units's job.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        fs::create_dir_all(home.join(".git")).unwrap();
        touch(
            &home.join(".aider.chat.history.md"),
            b"# aider chat history",
        );
        let units = run(home);
        assert!(
            units
                .iter()
                .all(|u| u.relative_path() != ".aider.chat.history.md"),
            "home-level identify must not pick up per-repo files"
        );
    }

    #[test]
    fn repo_history_files_are_linked_and_actionable() {
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        fs::create_dir_all(repo.join(".git")).unwrap();
        let canary = "CANARY-AIDER-DO-NOT-LEAK-55dd";
        touch(
            &repo.join(".aider.chat.history.md"),
            format!("# aider chat started\n\nUser: {canary}\n").as_bytes(),
        );
        touch(&repo.join(".aider.input.history"), b"+++ /help\n");
        let units = run_repo(repo);
        assert_eq!(units.len(), 2);
        for u in &units {
            assert_eq!(u.category(), AgentCategory::Sessions);
            assert_eq!(u.action(), AgentActionCapability::SessionRemoval);
            assert!(matches!(u.project_link(), ProjectLinkState::Linked { .. }));
        }
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "chat content leaked");
    }

    #[test]
    fn tags_cache_is_versioned_and_regenerable() {
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(&repo.join(".aider.tags.cache.v3/tags.db"), b"cache-bytes");
        let units = run_repo(repo);
        let u = units
            .iter()
            .find(|u| u.relative_path() == ".aider.tags.cache.v3")
            .expect("tags cache identified");
        assert_eq!(u.category(), AgentCategory::Caches);
        assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn a_tags_cache_of_any_version_suffix_is_identified() {
        // The version suffix is a repomap implementation detail, so the
        // prefix match must not be pinned to the versions that happened
        // to exist when this adapter was written.
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(&repo.join(".aider.tags.cache.v9/tags.db"), b"cache-bytes");
        let units = run_repo(repo);
        assert!(
            units
                .iter()
                .any(|u| u.relative_path() == ".aider.tags.cache.v9"),
            "a future tags-cache version must still be identified: {units:?}"
        );
    }

    #[test]
    fn a_worktree_with_no_aider_files_yields_nothing() {
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        fs::create_dir_all(repo.join(".git")).unwrap();
        assert!(run_repo(repo).is_empty());
    }

    #[test]
    fn identification_cost_is_bounded_for_a_large_tags_cache() {
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        fs::create_dir_all(repo.join(".git")).unwrap();
        for i in 0..2000 {
            touch(
                &repo.join(format!(".aider.tags.cache.v3/f{i}")),
                &b"x".repeat(2_000),
            );
        }
        let start = SystemTime::now();
        let units = run_repo(repo);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!(
            "[measured] aider identify_repo_units() over a 2000-file tags cache took {elapsed:?}"
        );
        assert_eq!(units.len(), 1);
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        // A ~/.aider with none of the documented markers is still
        // reported, as one explicit `(unclassified residual)` row that
        // names which markers were looked for -- never an empty vec,
        // and never re-read as some other tool's layout.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("something-unrecognized.bin"), b"\x00\x01");
        let units = run(home);
        assert_eq!(units.len(), 1, "unrecognized content must still surface");
        assert_eq!(units[0].relative_path(), "(unclassified residual)");
        assert_eq!(units[0].category(), AgentCategory::Unclassified);
        assert_eq!(units[0].action(), AgentActionCapability::None);
        let note = units[0].note.as_deref().unwrap_or_default();
        assert!(
            note.contains("no recognized Aider home markers"),
            "the unit must say what was looked for: {note}"
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-AIDER-CONTRACT-DO-NOT-LEAK-91ab";
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        fs::create_dir_all(repo.join(".git")).unwrap();
        // First line *and* body, since a future header read would see
        // the first line.
        touch(
            &repo.join(".aider.chat.history.md"),
            format!("# aider chat started {canary}\n\nUser: {canary}\n").as_bytes(),
        );
        touch(
            &repo.join(".aider.input.history"),
            format!("{canary}\n+++ /help\n").as_bytes(),
        );
        contract::no_content_leak(&run_repo(repo), canary);

        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(
            &home.join(".aider.conf.yml"),
            format!("openai-api-key: {canary}\n").as_bytes(),
        );
        contract::no_content_leak(&run(home), canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let repo = tempfile::tempdir().unwrap();
        let repo = repo.path();
        fs::create_dir_all(repo.join(".git")).unwrap();
        let mut fixture_bytes = 0u64;
        for i in 0..20 {
            let body = b"x".repeat(50_000);
            touch(&repo.join(format!(".aider.tags.cache.v3/f{i}")), &body);
            fixture_bytes += body.len() as u64;
        }
        let transcript = b"y".repeat(200_000);
        touch(&repo.join(".aider.chat.history.md"), &transcript);
        fixture_bytes += transcript.len() as u64;

        let (units, counters) = contract::measured(|| run_repo(repo));
        assert_eq!(units.len(), 2, "transcript + tags cache");
        // Aider's per-repo files are identified by name and measured by
        // `stat`; this adapter reads no file contents at all, so the
        // bound is not "small", it is zero.
        assert_eq!(
            counters.header_bytes_read, 0,
            "aider identification reads no content"
        );
        assert!(
            counters.header_bytes_read < fixture_bytes,
            "{} vs {fixture_bytes} fixture bytes",
            counters.header_bytes_read
        );
        contract::within_header_cap(counters, 0);
    }

    #[test]
    fn protected_categories_default_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join(".aider.conf.yml"), b"model: gpt-4o\n");
        touch(&home.join("caches/versioncheck"), b"");
        let units = run(home);
        let conf = units
            .iter()
            .find(|u| u.relative_path() == ".aider.conf.yml")
            .expect("home config identified");
        assert_eq!(conf.category(), AgentCategory::ProtectedConfig);
        assert!(conf.protected());
        assert_eq!(
            conf.protect_reason(),
            Some("home-level Aider configuration")
        );
        contract::protection_defaults_hold(&units);
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        // (a) A worktree root the caller already resolved is the
        // declared path: linkage comes back `Declared`, from that root's
        // own git identity.
        let declared = tempfile::tempdir().unwrap();
        let declared = declared.path().join("declared-checkout");
        fs::create_dir_all(declared.join(".git")).unwrap();
        touch(&declared.join(".aider.input.history"), b"+++ /help\n");
        let linked = run_repo(&declared);
        assert_eq!(linked.len(), 1);
        match &linked[0].project_link() {
            ProjectLinkState::Linked { source, .. } => assert_eq!(*source, LinkSource::Declared),
            other => panic!("a resolved worktree root must link: {other:?}"),
        }

        // (b) A home-level directory merely *named* like a project is
        // never turned into a link: the home-level units are tool-wide,
        // so `NotApplicable` is the honest answer and the basename never
        // reaches the linkage field.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let decoy = home.join("looks-like-my-project");
        fs::create_dir_all(decoy.join(".git")).unwrap();
        touch(&decoy.join(".aider.chat.history.md"), b"# not ours\n");
        let home_units = run(home);
        assert!(!home_units.is_empty(), "the residual must still surface");
        assert!(
            home_units
                .iter()
                .all(|u| matches!(u.project_link(), ProjectLinkState::NotApplicable)),
            "home-level units are tool-wide: {home_units:?}"
        );
        contract::linkage_is_declared_or_explicit(&home_units, "looks-like-my-project");
        contract::linkage_is_declared_or_explicit(&linked, "looks-like-my-project");
    }
}
