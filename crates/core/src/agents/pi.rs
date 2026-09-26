//! Pi identification (#96): sessions, protected configuration, and a
//! locally-installed npm package cache under the home
//! `crate::locations::pi::PiDetector` resolves. **Distinct tool from Oh
//! My Pi** -- see that detector's sibling doc comment for the disclosed
//! override-variable collision risk.
//!
//! Layout sourced from primary docs during implementation (never a real
//! `~/.pi` on this machine -- PRIVACY IS A HARD RULE); see
//! `crate::locations::pi`'s doc comment for citations.
//!
//! ## Explicit format detection, and why there is no cross-tool fallback
//!
//! Pi's own README documents its session files as "JSONL files with a
//! tree structure. Each entry has an `id` and `parentId`" -- the header
//! object is the file's first line, at byte offset 0. This adapter parses
//! **only** that documented shape, through the neutral mechanics in
//! `crate::agents::pi_family` (`HeaderLayout::OffsetZero`).
//!
//! A session header this adapter cannot parse is reported as an explicit
//! unknown-format outcome -- `ProjectLinkState::Unresolved` with a reason
//! naming the shape that was expected, plus a note on the unit -- and a
//! whole home with none of this tool's content markers is reported as one
//! explicit unknown-format residual unit. It is **never** retried against
//! some other tool's header shape.
//!
//! That is not caution, it is the guardrail
//! (`.oh/guardrails/agent-adapters-are-pluggable.md`): an adapter names
//! no other adapter, because if this adapter understood a sibling tool's
//! layout, a change to *that* tool's format would silently change *this*
//! tool's identification -- and a wrong project link is worse than an
//! honest "unknown format". Genuinely shared mechanics (finding a header
//! line at a byte offset, pulling a declared `cwd` out of it) live in the
//! neutral `pi_family` module, which names no tool at all.
//!
//! `sessions/` is documented as "organized by working directory" -- this
//! adapter deliberately does not decode a directory name into a project
//! path (no encoding scheme is confirmed by primary source), relying
//! only on each session file's own declared `cwd`.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, mtime_secs, pi_family,
};
use std::path::{Path, PathBuf};

pub const PI_TOOL_ID: &str = "pi";

const MAX_FOLD_ENTRIES: usize = 200_000;
/// Session files one `sessions/<dir>/` container will identify. Per
/// container, never shared: see [`collect_files`].
const MAX_CONTAINER_ENTRIES: usize = 20_000;
/// Session containers one pass will identify.
const MAX_CONTAINERS: usize = 20_000;
const MAX_WALK_DEPTH: usize = 4;
/// Per-session header read cap. Pi's header is one JSON line at byte
/// offset 0, so the read never needs a title-slot allowance.
const HEADER_READ_BYTES: usize = pi_family::HEADER_READ_BYTES;

/// The only layout this tool documents. A single-element list on
/// purpose: the list is what an adapter is *willing* to accept, and this
/// one accepts nothing it has no primary source for.
const ACCEPTED_LAYOUTS: &[pi_family::HeaderLayout] = &[pi_family::HeaderLayout::OffsetZero];

const FORMAT_MARKERS: &[&str] = &[
    "settings.json",
    "trust.json",
    "models.json",
    "sessions",
    "npm",
];

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        PI_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Pi"
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
        return unknown_format_residual(home, ctx);
    }
    let mut units = Vec::new();
    identify_sessions(home, ctx, &mut units);
    identify_static_categories(home, ctx, &mut units);
    units
}

fn unknown_format_residual(home: &Path, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    // A genuinely empty, existing directory (nothing here yet) is not
    // "unknown format" -- there is simply nothing to classify. Checked by
    // directory entries, not by the folded byte/mtime pair: an empty
    // directory's own mtime is non-zero, which would otherwise read as
    // "something present" and produce a bogus residual unit.
    if !ctx.has_entries(home) {
        return Vec::new();
    }
    let (bytes, mtime, _t) = ctx.folded_bytes(home, MAX_FOLD_ENTRIES);
    vec![
        AgentUnitBuilder::new(PI_TOOL_ID, AgentCategory::Unclassified, home.to_path_buf())
            .relative_path("(unknown format)")
            .bytes(bytes)
            .mtime_max(mtime)
            .action(AgentActionCapability::None)
            .note(
                "no Pi content markers found (settings.json/trust.json/models.json/sessions/npm) \
                 at this resolved path; this directory may belong to a different tool, be empty, \
                 or use an unsupported version -- treated as unknown format, not scanned further",
            )
            .build(),
    ]
}

/// Each immediate subdirectory of `sessions/` is a container: Pi's
/// layout is a directory tree, and nothing in a session's unit is
/// derived from outside its own subtree, so a subtree nothing touched is
/// replayed rather than re-listed. Session files sitting directly in
/// `sessions/` belong to no container and are identified every pass.
fn identify_sessions(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    let base = home.join("sessions");
    let mut loose = Vec::new();
    let mut containers = 0usize;
    for entry in ctx.list(&base) {
        let path = base.join(&entry.name);
        if entry.is_dir {
            if containers >= MAX_CONTAINERS {
                break;
            }
            containers += 1;
            let units = ctx.container(PI_TOOL_ID, &path, &|| {
                let mut files = Vec::new();
                collect_files(&path, 1, ctx, &mut files);
                session_units(home, files, ctx)
            });
            out.extend(units);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            loose.push(path);
        }
    }
    out.extend(session_units(home, loose, ctx));
}

fn session_units(home: &Path, files: Vec<PathBuf>, ctx: &IdentifyCtx) -> Vec<CandidateAgentUnit> {
    let mut out = Vec::new();
    for jsonl in files {
        let Ok(meta) = ctx.stat(&jsonl) else {
            continue;
        };
        let bytes = meta.len();
        let mtime = mtime_secs(&meta);
        let header = pi_family::derived_header(
            ctx,
            PI_TOOL_ID,
            "session-cwd",
            &jsonl,
            HEADER_READ_BYTES,
            ACCEPTED_LAYOUTS,
        );
        let unknown_format = header.is_empty();
        // `project_link_declared`, not `project_link`: a replayed
        // container re-resolves the declared path live rather than
        // replaying a resolution that may have gone stale
        // (`crate::agents::LinkBasis`).
        let mut unit = AgentUnitBuilder::new(PI_TOOL_ID, AgentCategory::Sessions, jsonl.clone())
            .relative_to(home)
            .members(vec![AgentMember {
                path: jsonl,
                bytes,
                kind: AgentMemberKind::Transcript,
            }])
            .mtime_max(mtime)
            .project_link_declared(
                header.cwd,
                &pi_family::no_layout_matched_reason(ACCEPTED_LAYOUTS),
            )
            .action(AgentActionCapability::SessionRemoval);
        if unknown_format {
            // Explicit, not silent: this session's header is not in the
            // shape Pi documents, and this adapter says so rather than
            // retrying another tool's shape.
            unit = unit.note(
                "unknown-format session header: not parseable as a JSON header at byte offset 0 \
                 (Pi's own documented shape); another tool's header shape is deliberately never \
                 tried here, so this session's project linkage stays unresolved",
            );
        }
        out.push(unit.build());
    }
    out
}

/// Bounded collection under **one** container. The entry budget belongs
/// to that container alone: a budget shared across containers would make
/// a replayed subtree mean something different from a live
/// identification of the same subtree, which is what kept this adapter
/// off the container seam until 2026-09-22.
fn collect_files(dir: &Path, depth: usize, ctx: &IdentifyCtx, out: &mut Vec<PathBuf>) {
    if depth > MAX_WALK_DEPTH || out.len() >= MAX_CONTAINER_ENTRIES {
        return;
    }
    for entry in ctx.list(dir) {
        if out.len() >= MAX_CONTAINER_ENTRIES {
            return;
        }
        let path = dir.join(&entry.name);
        if entry.is_dir {
            collect_files(&path, depth + 1, ctx, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn identify_static_categories(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    for (rel, note) in [
        ("settings.json", "main configuration"),
        ("trust.json", "per-project trust decisions"),
        ("models.json", "custom model/provider definitions"),
    ] {
        let path = home.join(rel);
        if let Ok(meta) = ctx.stat(&path)
            && meta.is_file()
        {
            out.push(
                AgentUnitBuilder::new(PI_TOOL_ID, AgentCategory::ProtectedConfig, path)
                    .relative_path(rel)
                    .bytes(meta.len())
                    .mtime_max(mtime_secs(&meta))
                    .protect(note)
                    .action(AgentActionCapability::None)
                    .build(),
            );
        }
    }

    let npm = home.join("npm");
    if ctx.is_dir(&npm) {
        let (bytes, mtime, truncated) = ctx.folded_bytes(&npm, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new(PI_TOOL_ID, AgentCategory::Caches, npm)
            .relative_path("npm")
            .bytes(bytes)
            .mtime_max(mtime)
            .action(AgentActionCapability::CacheOrLogTrash)
            .note("user-scoped npm package installs, reinstallable");
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        out.push(unit.build());
    }

    let seen: std::collections::HashSet<&str> = [
        "settings.json",
        "trust.json",
        "models.json",
        "npm",
        "sessions",
    ]
    .into_iter()
    .collect();
    let mut residual_bytes = 0u64;
    let mut residual_mtime = 0u64;
    let mut residual_names: Vec<String> = Vec::new();
    for entry in ctx.list(home) {
        if seen.contains(entry.name.as_str()) {
            continue;
        }
        let (bytes, mtime, _t) = ctx.folded_bytes(&home.join(&entry.name), MAX_FOLD_ENTRIES);
        residual_bytes += bytes;
        residual_mtime = residual_mtime.max(mtime);
        residual_names.push(entry.name);
    }
    if !residual_names.is_empty() {
        residual_names.sort();
        out.push(
            AgentUnitBuilder::new(PI_TOOL_ID, AgentCategory::Unclassified, home.to_path_buf())
                .relative_path("(unclassified residual)")
                .bytes(residual_bytes)
                .mtime_max(residual_mtime)
                .action(AgentActionCapability::None)
                .note(format!(
                    "entries with no specific rule in this adapter: {}",
                    residual_names.join(", ")
                ))
                .build(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{IdentificationCache, ProjectLinkState, bounded_io, contract};
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

    /// A session file in Pi's own documented shape: the JSON header is
    /// line 1, at byte offset 0.
    fn pi_session_bytes(cwd: &str, canary: &str) -> Vec<u8> {
        format!(
            "{{\"id\":\"1\",\"parentId\":null,\"cwd\":\"{cwd}\"}}\n\
             {{\"id\":\"2\",\"parentId\":\"1\",\"content\":\"{canary}\"}}\n"
        )
        .into_bytes()
    }

    /// A session file in the *sibling* fork's shape: a 256-byte title
    /// slot ahead of the header line. Pi does not document this, so this
    /// adapter must treat it as unknown format -- never parse it.
    fn title_slot_session_bytes(cwd: &str) -> Vec<u8> {
        let mut title = vec![b' '; 256];
        let title_json = b"{\"type\":\"title\"}";
        title[..title_json.len()].copy_from_slice(title_json);
        title[255] = b'\n';
        let mut out = title;
        out.extend_from_slice(format!("{{\"type\":\"session\",\"cwd\":\"{cwd}\"}}\n").as_bytes());
        out
    }

    #[test]
    fn empty_home_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path()).is_empty());
    }

    #[test]
    fn no_markers_yields_unknown_format_not_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated.txt"), b"hello");
        let units = run(dir.path());
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].relative_path(), "(unknown format)");
    }

    #[test]
    fn session_links_via_pis_own_offset_zero_header() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let canary = "CANARY-PI-DO-NOT-LEAK-21cc";
        let jsonl = home.join("sessions/-fixture-repo/1700000000.jsonl");
        touch(
            &jsonl,
            &pi_session_bytes(&repo.display().to_string(), canary),
        );
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions)
            .expect("session identified");
        assert!(matches!(
            session.project_link(),
            ProjectLinkState::Linked { .. }
        ));
        assert_eq!(session.action(), AgentActionCapability::SessionRemoval);
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "content leaked");
    }

    /// Replaces the former `session_falls_back_to_the_omp_title_slot_shape`
    /// test, which asserted exactly the cross-adapter knowledge the
    /// pluggability guardrail forbids: a file in the sibling fork's shape
    /// must come back as an explicit unknown format, not as a link.
    #[test]
    fn a_sibling_forks_header_shape_is_unknown_format_here_not_a_link() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let jsonl = home.join("sessions/x/1.jsonl");
        touch(
            &jsonl,
            &title_slot_session_bytes(&repo.display().to_string()),
        );
        let units = run(home);
        let session = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(
            matches!(session.project_link(), ProjectLinkState::Unresolved { .. }),
            "a shape this tool does not document must not resolve a project: {:?}",
            session.project_link()
        );
        let note = session.note.as_deref().unwrap_or_default();
        assert!(
            note.contains("unknown-format session header"),
            "the unknown format must be stated on the unit: {note}"
        );
    }

    #[test]
    fn npm_is_actionable_cache() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("npm/pkg/index.js"), b"module.exports = {}");
        let units = run(home);
        let u = units.iter().find(|u| u.relative_path() == "npm").unwrap();
        assert!(!u.protected());
        assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn settings_trust_and_models_are_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join("trust.json"), b"{}");
        touch(&home.join("models.json"), b"{}");
        let units = run(home);
        for rel in ["settings.json", "trust.json", "models.json"] {
            let u = units.iter().find(|u| u.relative_path() == rel).unwrap();
            assert!(u.protected());
            assert!(u.protect_reason().is_some());
        }
    }

    #[test]
    fn identification_cost_is_bounded_for_many_sessions() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        let repo = home.join("big-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        for i in 0..500 {
            let jsonl = home.join(format!("sessions/-big-repo/{i}.jsonl"));
            let mut body =
                format!("{{\"id\":\"1\",\"cwd\":\"{}\"}}\n", repo.display()).into_bytes();
            body.extend_from_slice(&b"x".repeat(200_000));
            touch(&jsonl, &body);
        }
        let start = SystemTime::now();
        let units = run(home);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!("[measured] pi identify() over 500 synthetic sessions took {elapsed:?}");
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            500
        );
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    }

    // --- the five contract tests ---------------------------------------

    #[test]
    fn unknown_format_is_explicit_not_empty() {
        // (a) a whole home with none of this tool's markers: one
        // explicit unknown-format unit, never an empty vec.
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated.txt"), b"hello");
        let units = run(dir.path());
        assert_eq!(units.len(), 1, "an unrecognized home must still surface");
        assert_eq!(units[0].relative_path(), "(unknown format)");
        assert_eq!(units[0].action(), AgentActionCapability::None);
        assert!(
            units[0]
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("unknown format")
        );

        // (b) a session header in no shape this tool documents: an
        // explicit unresolved linkage with a stated reason, never a
        // retry against another tool's shape.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        let jsonl = home.join("sessions/x/1.jsonl");
        touch(&jsonl, b"not a session header at all\n");
        let units = run(home);
        let session = units.iter().find(|u| u.path == jsonl).unwrap();
        match &session.project_link() {
            ProjectLinkState::Unresolved { reason } => {
                assert!(
                    reason.contains("byte offset 0"),
                    "the reason must name the shape that was expected: {reason}"
                );
                assert!(
                    !reason.contains("title slot"),
                    "this adapter must not claim to have checked another tool's shape: {reason}"
                );
            }
            other => panic!("expected an explicit unresolved outcome, got {other:?}"),
        }
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-PI-BODY-DO-NOT-LEAK-9d4e";
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        // On the first line (which this adapter *does* read) and in the
        // body (which it never reads).
        let jsonl = home.join("sessions/x/1.jsonl");
        touch(
            &jsonl,
            format!(
                "{{\"id\":\"1\",\"cwd\":\"/no/such/dir\",\"title\":\"{canary}\"}}\n\
                 {{\"id\":\"2\",\"content\":\"{canary}\"}}\n"
            )
            .as_bytes(),
        );
        contract::no_content_leak(&run(home), canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        let mut fixture_bytes = 0u64;
        for i in 0..20 {
            let jsonl = home.join(format!("sessions/x/{i}.jsonl"));
            let mut body = b"{\"id\":\"1\",\"cwd\":\"/no/such/dir\"}\n".to_vec();
            body.extend_from_slice(&b"x".repeat(100_000));
            fixture_bytes += body.len() as u64;
            touch(&jsonl, &body);
        }
        let (units, counters) = contract::measured(|| run(home));
        assert_eq!(
            units
                .iter()
                .filter(|u| u.category() == AgentCategory::Sessions)
                .count(),
            20
        );
        // One capped header read per session, and nothing else.
        contract::within_header_cap(counters, 20);
        assert!(
            counters.header_bytes_read <= 20 * HEADER_READ_BYTES as u64,
            "read {} bytes, above this adapter's own per-session cap",
            counters.header_bytes_read
        );
        assert!(
            counters.header_bytes_read < fixture_bytes,
            "identification read {} of {fixture_bytes} fixture bytes",
            counters.header_bytes_read
        );
        const { assert!(HEADER_READ_BYTES <= bounded_io::MAX_HEADER_BYTES) };
    }

    #[test]
    fn protected_categories_default_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        touch(&home.join("trust.json"), b"{}");
        touch(&home.join("npm/pkg/index.js"), b"{}");
        let units = run(home);
        contract::protection_defaults_hold(&units);
        let settings = units
            .iter()
            .find(|u| u.relative_path() == "settings.json")
            .unwrap();
        assert_eq!(settings.category(), AgentCategory::ProtectedConfig);
        assert_eq!(
            settings.protect_reason(),
            Some("main configuration"),
            "the adapter's own, more specific reason survives the builder default"
        );
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("settings.json"), b"{}");
        // (a) declared metadata naming a real worktree resolves Linked.
        let repo = home.join("declared-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let declared = home.join("sessions/-declared-repo/1.jsonl");
        touch(
            &declared,
            &pi_session_bytes(&repo.display().to_string(), "body"),
        );
        // (b) a session sitting in a directory *named* after a real
        // worktree, declaring nothing: unresolved, never linked.
        let guessable = home.join("guessable-repo");
        fs::create_dir_all(guessable.join(".git")).unwrap();
        let undeclared = home.join("sessions/guessable-repo/2.jsonl");
        touch(&undeclared, b"{\"id\":\"1\",\"parentId\":null}\n");

        let units = run(home);
        let a = units.iter().find(|u| u.path == declared).unwrap();
        match &a.project_link() {
            ProjectLinkState::Linked { source, .. } => {
                assert_eq!(*source, crate::agents::LinkSource::Declared)
            }
            other => panic!("declared cwd must link: {other:?}"),
        }
        let b = units.iter().find(|u| u.path == undeclared).unwrap();
        assert!(
            matches!(b.project_link(), ProjectLinkState::Unresolved { .. }),
            "a directory name is not evidence: {:?}",
            b.project_link()
        );
        contract::linkage_is_declared_or_explicit(&units, "guessable-repo");
    }
}
