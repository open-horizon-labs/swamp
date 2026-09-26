//! Gemini CLI identification (#96): per-project temp state (shell
//! history, checkpoints, saved chats), shadow-Git checkpoint history and
//! protected configuration under the home
//! `crate::locations::gemini_cli::GeminiCliDetector` resolves.
//!
//! Layout is sourced from primary docs/source during implementation
//! (never a real `~/.gemini` on this machine -- PRIVACY IS A HARD RULE);
//! see `crate::locations::gemini_cli`'s doc comment for the full
//! citations.
//!
//! ## Project-id linkage (#96's explicit bar)
//!
//! `tmp/<project-id>` and `history/<project-id>` are keyed by an id this
//! adapter treats as **opaque**, because upstream has used two different
//! shapes for it:
//!
//! * historically, `getProjectHash(projectRoot) = sha256(projectRoot).hex()`
//!   (`packages/core/src/utils/paths.ts`) -- a 64-hex-character name and
//!   a one-way function; and
//! * in current versions, a **short slug id**: projects are registered in
//!   `<runtimeDir>/projects.json` and the older hash directories are
//!   migrated across to the slug (`packages/core/src/config/storage.ts`,
//!   google-gemini/gemini-cli main @
//!   `d5b3e3accb26000d273abf16e0f1dd83aa5428a9`).
//!
//! So a directory name here is never assumed to be 64 hex characters,
//! never decoded, and never matched against a basename. Reading
//! `projects.json` -- the one upstream mapping from id back to project
//! root -- is deliberately **out of scope for this adapter**, so every
//! unit under a project-id directory carries
//! `ProjectLinkState::Unresolved` with a reason that says exactly that.
//! A future caller with that mapping (or with a project catalog to hash
//! against, for the legacy shape) could resolve it -- not implemented
//! here, and not guessed here.
//!
//! ## Version-aware boundary
//!
//! Checked content markers: `settings.json`, `GEMINI.md` (or another
//! configured context filename -- only the default is checked),
//! `extensions/`, `tmp/`, `history/`, `trustedFolders.json`, `bin/`,
//! `oauth_creds.json`. None present but the directory non-empty -> one
//! `Unclassified`, non-actionable "unsupported layout version" residual,
//! same discipline `crate::agents::opencode`/`oh_my_pi` use.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, ProjectLinkState,
    mtime_secs,
};
use std::collections::HashSet;
use std::path::Path;

pub const GEMINI_CLI_TOOL_ID: &str = "gemini-cli";

const MAX_FOLD_ENTRIES: usize = 200_000;

/// The one credential filename confirmed by primary source:
/// `packages/core/src/config/storage.ts`'s `OAUTH_FILE`
/// (google-gemini/gemini-cli main @
/// `d5b3e3accb26000d273abf16e0f1dd83aa5428a9`). The defensive filename
/// pattern below stays in place for any *other* credential file this
/// adapter has not confirmed; this constant is what lets the confirmed
/// one carry a confirmed reason instead of a defensive one.
const OAUTH_CREDS_FILE: &str = "oauth_creds.json";

const PROJECT_ID_UNRESOLVED_REASON: &str = "Gemini CLI keys this directory by an opaque project id: historically \
     sha256(project root path) per packages/core/src/utils/paths.ts's getProjectHash, and in \
     current versions a short slug id registered in <runtimeDir>/projects.json (packages/core/\
     src/config/storage.ts), with the older hash directories migrated across. Both shapes are \
     one-way from the directory name alone, projects.json is the upstream mapping back to a \
     project root, and this adapter does not read it -- so the project behind this id is \
     reported unresolved rather than guessed";

const FORMAT_MARKERS: &[&str] = &[
    "settings.json",
    "GEMINI.md",
    "extensions",
    "tmp",
    "history",
    "trustedFolders.json",
    OAUTH_CREDS_FILE,
];

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        GEMINI_CLI_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Gemini CLI"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }
    fn identify(&self, home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
        identify(home, ctx)
    }
}

pub fn identify(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    if !ctx.is_dir(home) {
        return Vec::new();
    }
    if !FORMAT_MARKERS.iter().any(|rel| ctx.exists(&home.join(rel))) {
        return unknown_version_residual(home, ctx);
    }
    let mut units = Vec::new();
    identify_protected(home, ctx, &mut units);
    identify_tmp(home, ctx, &mut units);
    identify_history(home, ctx, &mut units);
    identify_residual(home, ctx, &mut units);
    units
}

fn unknown_version_residual(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    if !ctx.has_entries(home) {
        return Vec::new();
    }
    let (bytes, mtime, _t) = ctx.folded_bytes(home, MAX_FOLD_ENTRIES);
    vec![
        AgentUnitBuilder::new(
            GEMINI_CLI_TOOL_ID,
            AgentCategory::Unclassified,
            home.to_path_buf(),
        )
        .relative_path("(unsupported layout version)")
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::None)
        .note(
            "no recognized Gemini CLI markers found (settings.json/GEMINI.md/extensions/tmp/\
             history/trustedFolders.json/bin/oauth_creds.json) at this resolved path -- \
             unsupported or future layout version, treated as unknown, not scanned further",
        )
        .build(),
    ]
}

/// A top-level filename that *looks* credential-shaped. Guardrail
/// precedent: `crate::actions::is_sqlite_like`. Kept alongside the
/// confirmed `oauth_creds.json` because upstream may write other
/// credential/account files this adapter has not confirmed, and an
/// unconfirmed credential file must be protected rather than left
/// actionable pending a citation.
fn is_credential_shaped(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.contains("oauth") || lower.contains("cred")) && !lower.starts_with('.')
}

fn identify_protected(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    for (rel, note) in [
        ("settings.json", "user settings"),
        ("GEMINI.md", "context/memory file"),
        ("trustedFolders.json", "trusted-folder decisions"),
    ] {
        let path = home.join(rel);
        if let Ok(meta) = ctx.stat(&path)
            && meta.is_file()
        {
            out.push(
                AgentUnitBuilder::new(GEMINI_CLI_TOOL_ID, AgentCategory::ProtectedConfig, path)
                    .relative_path(rel)
                    .bytes(meta.len())
                    .mtime_max(mtime_secs(&meta))
                    .protect(note)
                    .project_link(ProjectLinkState::NotApplicable)
                    .action(AgentActionCapability::None)
                    .build(),
            );
        }
    }
    for entry in ctx.list(home) {
        if entry.is_dir {
            continue;
        }
        let confirmed = entry.name == OAUTH_CREDS_FILE;
        if !confirmed && !is_credential_shaped(&entry.name) {
            continue;
        }
        let path = home.join(&entry.name);
        let Ok(meta) = ctx.stat(&path) else {
            continue;
        };
        let reason = if confirmed {
            "OAuth credentials -- confirmed upstream as packages/core/src/config/storage.ts's \
             OAUTH_FILE = \"oauth_creds.json\" (google-gemini/gemini-cli main @ \
             d5b3e3accb26000d273abf16e0f1dd83aa5428a9)"
        } else {
            "credential-shaped filename (defensive pattern match; oauth_creds.json is the one \
             confirmed credential file upstream, and the pattern stays for any other this \
             adapter has not confirmed)"
        };
        out.push(
            AgentUnitBuilder::new(GEMINI_CLI_TOOL_ID, AgentCategory::ProtectedConfig, path)
                .relative_path(entry.name.clone())
                .bytes(meta.len())
                .mtime_max(mtime_secs(&meta))
                .protect(reason)
                .project_link(ProjectLinkState::NotApplicable)
                .action(AgentActionCapability::None)
                .build(),
        );
    }
    let extensions = home.join("extensions");
    if ctx.is_dir(&extensions) {
        let (bytes, mtime, _t) = ctx.folded_bytes(&extensions, MAX_FOLD_ENTRIES);
        out.push(
            AgentUnitBuilder::new(GEMINI_CLI_TOOL_ID, AgentCategory::Plugins, extensions)
                .relative_path("extensions")
                .bytes(bytes)
                .mtime_max(mtime)
                .protect("installed extensions; removing breaks the CLI's configured integrations")
                .project_link(ProjectLinkState::NotApplicable)
                .action(AgentActionCapability::None)
                .build(),
        );
    }
    // `tmp/bin`, not `bin`. Upstream builds it as
    // `getGlobalBinDir() = join(getGlobalTempDir(), BIN_DIR_NAME)` and
    // `getGlobalTempDir() = join(getGlobalRuntimeDir(), TMP_DIR_NAME)`
    // (`packages/core/src/config/storage.ts:24-25,195-201` @
    // `d5b3e3accb26000d273abf16e0f1dd83aa5428a9`). This adapter looked
    // at `~/.gemini/bin`, a path that does not exist in any version --
    // dead code that reported nothing, found by the 2026-09-22
    // re-review fetching the file the row already cited.
    let bin = home.join("tmp").join("bin");
    if ctx.is_dir(&bin) {
        let (bytes, mtime, truncated) = ctx.folded_bytes(&bin, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new(GEMINI_CLI_TOOL_ID, AgentCategory::Caches, bin)
            .relative_path("tmp/bin")
            .bytes(bytes)
            .mtime_max(mtime)
            .project_link(ProjectLinkState::NotApplicable)
            .action(AgentActionCapability::CacheOrLogTrash)
            .note("downloaded runtime tools (e.g. LiteRT-LM), re-downloadable");
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        out.push(unit.build());
    }
}

fn identify_tmp(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let base = home.join("tmp");
    // Every subdirectory name is opaque: a legacy 64-hex project hash and
    // a current short slug id are handled identically, because neither is
    // invertible here (see the module docs).
    for project_id in ctx.dir_names(&base) {
        // `tmp/bin` is the downloaded-runtime-tools cache
        // (`Storage.getGlobalBinDir()`), not a project id. It has its own
        // unit above; treating it as a project directory here would both
        // double-count it and invent a project that does not exist.
        if project_id == "bin" {
            continue;
        }
        let project_dir = base.join(&project_id);
        let mut seen: HashSet<&str> = HashSet::new();

        let shell_history = project_dir.join("shell_history");
        if let Ok(meta) = ctx.stat(&shell_history)
            && meta.is_file()
        {
            seen.insert("shell_history");
            out.push(
                AgentUnitBuilder::new(GEMINI_CLI_TOOL_ID, AgentCategory::Logs, shell_history)
                    .relative_to(home)
                    .bytes(meta.len())
                    .mtime_max(mtime_secs(&meta))
                    .project_link(ProjectLinkState::Unresolved {
                        reason: PROJECT_ID_UNRESOLVED_REASON.to_string(),
                    })
                    .action(AgentActionCapability::CacheOrLogTrash)
                    .note("per-project shell command history for this CLI session")
                    .build(),
            );
        }

        let checkpoints = project_dir.join("checkpoints");
        if ctx.is_dir(&checkpoints) {
            seen.insert("checkpoints");
            let (bytes, mtime, truncated) = ctx.folded_bytes(&checkpoints, MAX_FOLD_ENTRIES);
            let unit =
                AgentUnitBuilder::new(GEMINI_CLI_TOOL_ID, AgentCategory::Checkpoints, checkpoints)
                    .relative_to(home)
                    .bytes(bytes)
                    .mtime_max(mtime)
                    .project_link(ProjectLinkState::Unresolved {
                        reason: PROJECT_ID_UNRESOLVED_REASON.to_string(),
                    })
                    .action(AgentActionCapability::None)
                    .note(
                        "tool-call checkpoint state for /restore; not a supported selective \
                         action this chunk",
                    );
            let unit = if truncated {
                unit.incomplete("directory entry count bound reached")
            } else {
                unit
            };
            out.push(unit.build());
        }

        let chats = project_dir.join("chats");
        if ctx.is_dir(&chats) {
            seen.insert("chats");
            for name in ctx.file_names(&chats) {
                let path = chats.join(&name);
                let Ok(meta) = ctx.stat(&path) else {
                    continue;
                };
                out.push(
                    AgentUnitBuilder::new(
                        GEMINI_CLI_TOOL_ID,
                        AgentCategory::Sessions,
                        path.clone(),
                    )
                    .relative_to(home)
                    .mtime_max(mtime_secs(&meta))
                    .members(vec![AgentMember {
                        path,
                        bytes: meta.len(),
                        kind: AgentMemberKind::Transcript,
                    }])
                    .project_link(ProjectLinkState::Unresolved {
                        reason: PROJECT_ID_UNRESOLVED_REASON.to_string(),
                    })
                    .action(AgentActionCapability::SessionRemoval)
                    .note("saved chat (/chat save, /resume)")
                    .build(),
                );
            }
        }

        let (residual_bytes, residual_mtime, residual_names) =
            fold_residual_children(&project_dir, &seen, ctx);
        if !residual_names.is_empty() {
            out.push(
                AgentUnitBuilder::new(
                    GEMINI_CLI_TOOL_ID,
                    AgentCategory::Unclassified,
                    project_dir.clone(),
                )
                .relative_path(format!(
                    "{} (unclassified residual)",
                    super::relative_to(home, &project_dir)
                ))
                .bytes(residual_bytes)
                .mtime_max(residual_mtime)
                .project_link(ProjectLinkState::Unresolved {
                    reason: PROJECT_ID_UNRESOLVED_REASON.to_string(),
                })
                .action(AgentActionCapability::None)
                .note(format!(
                    "entries with no specific rule in this adapter: {}",
                    residual_names.join(", ")
                ))
                .build(),
            );
        }
    }
}

fn identify_history(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let base = home.join("history");
    for project_id in ctx.dir_names(&base) {
        let path = base.join(&project_id);
        let (bytes, mtime, truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new(GEMINI_CLI_TOOL_ID, AgentCategory::Checkpoints, path)
            .relative_to(home)
            .bytes(bytes)
            .mtime_max(mtime)
            .project_link(ProjectLinkState::Unresolved {
                reason: PROJECT_ID_UNRESOLVED_REASON.to_string(),
            })
            .action(AgentActionCapability::None)
            .note(
                "shadow Git repository backing this project's /restore checkpoints, \
                 independent of the project's own .git; not a supported selective action \
                 this chunk",
            );
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        out.push(unit.build());
    }
}

fn fold_residual_children(
    dir: &Path,
    seen: &HashSet<&str>,
    ctx: &IdentifyCtx,
) -> (u64, u64, Vec<String>) {
    let mut bytes = 0u64;
    let mut mtime = 0u64;
    let mut names = Vec::new();
    for entry in ctx.list(dir) {
        if seen.contains(entry.name.as_str()) {
            continue;
        }
        let (b, m, _t) = ctx.folded_bytes(&dir.join(&entry.name), MAX_FOLD_ENTRIES);
        bytes += b;
        mtime = mtime.max(m);
        names.push(entry.name);
    }
    names.sort();
    (bytes, mtime, names)
}

fn identify_residual(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let seen: HashSet<&str> = [
        "settings.json",
        "GEMINI.md",
        "trustedFolders.json",
        "extensions",
        "tmp",
        "history",
    ]
    .into_iter()
    .collect();
    let (bytes, mtime, names) = fold_residual_children(home, &seen, ctx);
    // Credential files (the confirmed `oauth_creds.json` and anything the
    // defensive pattern claimed) already have their own protected units;
    // exclude them from the generic residual so they are not
    // double-counted.
    let names: Vec<String> = names
        .into_iter()
        .filter(|n| n != OAUTH_CREDS_FILE && !is_credential_shaped(n))
        .collect();
    if names.is_empty() {
        return;
    }
    out.push(
        AgentUnitBuilder::new(
            GEMINI_CLI_TOOL_ID,
            AgentCategory::Unclassified,
            home.to_path_buf(),
        )
        .relative_path("(unclassified residual)")
        .bytes(bytes)
        .mtime_max(mtime)
        .project_link(ProjectLinkState::NotApplicable)
        .action(AgentActionCapability::None)
        .note(format!(
            "entries with no specific rule in this adapter: {}",
            names.join(", ")
        ))
        .build(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{IdentificationCache, contract};
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn run(home: &Path) -> Vec<CandidateAgentUnit> {
        let cache = IdentificationCache::disabled();
        identify(home, &IdentifyCtx::new(1, &cache))
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
    fn settings_and_gemini_md_are_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join("GEMINI.md"), b"# context");
        let units = run(home);
        assert!(
            units
                .iter()
                .find(|u| u.relative_path() == "settings.json")
                .unwrap()
                .protected()
        );
        assert!(
            units
                .iter()
                .find(|u| u.relative_path() == "GEMINI.md")
                .unwrap()
                .protected()
        );
    }

    #[test]
    fn confirmed_oauth_creds_file_is_protected_with_its_citation() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join(OAUTH_CREDS_FILE), b"[redacted]");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == OAUTH_CREDS_FILE)
            .expect("the confirmed credential file is identified on its own");
        assert!(u.protected());
        assert_eq!(u.action(), AgentActionCapability::None);
        // Not `.unwrap_or_default()`: an absent reason is a failure of
        // this assertion, not an empty string to search. `check.sh`'s
        // grep layer also rejects that pattern on any line mentioning
        // protection, deliberately without exception
        // (`.oh/guardrails/protection-fails-closed.md`).
        let reason = u
            .protect_reason()
            .expect("a protected unit must carry a stated reason");
        assert!(
            reason.contains("storage.ts") && reason.contains("OAUTH_FILE"),
            "the confirmed file must carry its primary-source citation: {reason}"
        );
    }

    #[test]
    fn credential_shaped_filename_is_protected_defensively() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(
            &home.join("google_accounts_credentials.json"),
            b"[redacted]",
        );
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "google_accounts_credentials.json")
            .expect("credential-shaped file identified");
        assert!(u.protected());
        assert_eq!(u.action(), AgentActionCapability::None);
        assert!(
            u.protect_reason()
                .unwrap_or_default()
                .contains("defensive pattern match"),
            "an unconfirmed credential file says so"
        );
        assert!(
            !units.iter().any(|u| u
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("google_accounts_credentials.json")),
            "a claimed credential file must not also land in the residual"
        );
    }

    #[test]
    fn extensions_are_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("extensions/foo/package.json"), b"{}");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "extensions")
            .unwrap();
        assert!(u.protected());
    }

    #[test]
    fn tmp_project_dir_children_are_categorized_and_project_id_is_unresolved() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let hash = "e".repeat(64);
        touch(&home.join(format!("tmp/{hash}/shell_history")), b"ls\ncd\n");
        touch(&home.join(format!("tmp/{hash}/checkpoints/1.json")), b"{}");
        let canary = "CANARY-GEMINI-DO-NOT-LEAK-33ff";
        touch(
            &home.join(format!("tmp/{hash}/chats/decision-point.json")),
            format!("{{\"note\":\"{canary}\"}}").as_bytes(),
        );
        let units = run(home);
        let shell = units
            .iter()
            .find(|u| u.category() == AgentCategory::Logs)
            .expect("shell history");
        assert!(matches!(
            shell.project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
        let checkpoint = units
            .iter()
            .find(|u| u.category() == AgentCategory::Checkpoints)
            .expect("checkpoint dir");
        assert_eq!(checkpoint.action(), AgentActionCapability::None);
        let chat = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .expect("saved chat");
        assert_eq!(chat.action(), AgentActionCapability::SessionRemoval);
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "chat content leaked");
    }

    /// The corrected path, asserted both ways: `tmp/bin` is the cache,
    /// and the old `~/.gemini/bin` guess is not.
    ///
    /// Upstream: `getGlobalBinDir() = join(getGlobalTempDir(), 'bin')`,
    /// `getGlobalTempDir() = join(getGlobalRuntimeDir(), 'tmp')`
    /// (`packages/core/src/config/storage.ts:24-25,195-201` @
    /// `d5b3e3accb26000d273abf16e0f1dd83aa5428a9`, vendored at
    /// `crates/core/tests/fixtures/upstream/gemini-cli/d5b3e3accb/storage.ts`).
    #[test]
    fn the_downloaded_tools_cache_is_tmp_bin_and_not_a_project_id() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        touch(&home.join("tmp/bin/litert-lm"), &vec![b'x'; 4096]);
        touch(&home.join("bin/should-not-be-found"), &vec![b'y'; 8192]);
        let units = run(home);
        let cache = units
            .iter()
            .find(|u| u.relative_path() == "tmp/bin")
            .expect("tmp/bin must be the downloaded-tools cache");
        assert_eq!(cache.category(), AgentCategory::Caches);
        assert!(cache.bytes() >= 4096, "{}", cache.bytes());
        assert!(
            !units.iter().any(|u| u.relative_path() == "bin"),
            "~/.gemini/bin is not a path this tool writes; identifying it would be a guess"
        );
        // And `tmp/bin` must not also be reported as a per-project
        // directory, which is what iterating `tmp/`'s children blind
        // would do.
        assert!(
            !units
                .iter()
                .any(|u| u.relative_path().starts_with("tmp/bin/")),
            "tmp/bin is the tools cache, not a project id: {:?}",
            units
                .iter()
                .map(|u| u.relative_path().to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_short_slug_project_id_is_identified_like_a_legacy_hash() {
        // Current upstream registers projects in projects.json and keys
        // these directories by a short slug id; a 64-hex name is the
        // legacy shape, not a requirement (see the module docs).
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("tmp/a1b2c3/chats/s.json"), b"{}");
        touch(&home.join("history/a1b2c3/HEAD"), b"ref: refs/heads/main");
        let units = run(home);
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            1,
            "a slug-named project directory is identified, not skipped"
        );
        assert!(
            units
                .iter()
                .any(|u| u.category() == AgentCategory::Checkpoints),
            "a slug-named history directory is identified too"
        );
    }

    #[test]
    fn history_shadow_repo_is_checkpoint_category_and_not_actionable() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let hash = "f".repeat(64);
        touch(
            &home.join(format!("history/{hash}/HEAD")),
            b"ref: refs/heads/main",
        );
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.category() == AgentCategory::Checkpoints)
            .expect("history shadow repo identified");
        assert_eq!(u.action(), AgentActionCapability::None);
        assert!(matches!(
            u.project_link(),
            ProjectLinkState::Unresolved { .. }
        ));
    }

    #[test]
    fn identification_cost_is_bounded_for_many_project_ids() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        for i in 0..300 {
            let hash = format!("{i:064}");
            touch(
                &home.join(format!("tmp/{hash}/chats/session.json")),
                &b"x".repeat(2_000),
            );
        }
        let start = SystemTime::now();
        let units = run(home);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!(
            "[measured] gemini_cli identify() over 300 synthetic project ids took {elapsed:?}"
        );
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            300
        );
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated.txt"), b"hello");
        let units = run(dir.path());
        assert_eq!(units.len(), 1, "an unrecognized home still surfaces a row");
        assert_eq!(units[0].relative_path(), "(unsupported layout version)");
        assert_eq!(units[0].category(), AgentCategory::Unclassified);
        assert_eq!(units[0].action(), AgentActionCapability::None);
        assert!(
            units[0]
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("unsupported or future layout version"),
            "the row must say why it is unclassified, not guess another tool's shape"
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-GEMINI-DO-NOT-LEAK-91ab";
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let id = "a".repeat(64);
        // First line and body alike: this adapter reads neither.
        touch(
            &home.join(format!("tmp/{id}/chats/s.json")),
            format!("{{\"title\":\"{canary}\"}}\nbody: {canary}\n").as_bytes(),
        );
        touch(
            &home.join(format!("tmp/{id}/shell_history")),
            format!("echo {canary}\n").as_bytes(),
        );
        touch(&home.join("settings.json"), canary.as_bytes());
        contract::no_content_leak(&run(home), canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let mut total = 0u64;
        for i in 0..40 {
            let id = format!("{i:064}");
            let body = b"x".repeat(30_000);
            total += body.len() as u64;
            touch(&home.join(format!("tmp/{id}/chats/s.json")), &body);
        }
        let (units, counters) = contract::measured(|| run(home));
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            40
        );
        // This adapter resolves nothing from a chat body: its whole
        // identification is `stat` plus bounded listings.
        assert_eq!(
            counters.header_bytes_read, 0,
            "a Gemini CLI home is measured and listed, never read"
        );
        assert!(total > 0);
        contract::within_header_cap(counters, 0);
    }

    #[test]
    fn protected_categories_default_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join("trustedFolders.json"), b"{}");
        touch(&home.join(OAUTH_CREDS_FILE), b"[redacted]");
        let id = "b".repeat(64);
        touch(&home.join(format!("tmp/{id}/chats/s.json")), b"{}");
        let units = run(home);
        contract::protection_defaults_hold(&units);
        assert!(
            units
                .iter()
                .any(|u| u.relative_path() == OAUTH_CREDS_FILE && u.protected()),
            "the confirmed credential file is protected"
        );
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        // Gemini CLI declares no project path anywhere this adapter
        // reads: the project id is one-way, so `Linked` is never
        // produced and a directory *named* like a repo must not become
        // one.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("tmp/my-repo-name/chats/s.json"), b"{}");
        touch(&home.join("history/my-repo-name/HEAD"), b"ref: x");
        let units = run(home);
        assert!(
            !units.is_empty(),
            "a project-id directory is identified even when its id is unresolvable"
        );
        for u in &units {
            assert!(
                !matches!(u.project_link(), ProjectLinkState::Linked { .. }),
                "{} claimed a link with no declared metadata",
                u.relative_path()
            );
        }
        let reason = units
            .iter()
            .find_map(|u| match &u.project_link() {
                ProjectLinkState::Unresolved { reason } => Some(reason.clone()),
                _ => None,
            })
            .expect("a project-id unit is explicitly unresolved");
        assert!(
            reason.contains("projects.json") && reason.contains("one-way"),
            "the reason must name the upstream mapping this adapter does not read: {reason}"
        );
        contract::linkage_is_declared_or_explicit(&units, "my-repo-name");
    }
}
