//! Oh My Pi identification (#94): sessions, a content-addressed shared
//! blob store, terminal breadcrumbs and protected configuration under
//! the home `crate::locations::oh_my_pi::OhMyPiDetector` resolves.
//!
//! Layout researched from primary source during implementation (never
//! from a real `~/.omp` on this machine -- PRIVACY IS A HARD RULE), both
//! from <https://github.com/can1357/oh-my-pi>, current `main` as of this
//! chunk:
//! - `docs/session.md`: sessions at
//!   `~/.omp/agent/sessions/<encoded-cwd>/<timestamp>_<sessionId>.jsonl`;
//!   files "physically begin with a fixed-width, 256-byte `type:
//!   "title"` slot, followed by the session header" (header fields
//!   include `cwd` and `additionalDirectories`, "normalized, deduplicated
//!   workspace roots beyond cwd"); a content-addressed blob store at
//!   `~/.omp/agent/blobs/<sha256>` ("Image data URLs in `image_url`
//!   fields are always content-addressed in the blob store and replaced
//!   with `blob:sha256:<hash>`"); terminal breadcrumbs at
//!   `~/.omp/agent/terminal-sessions/`.
//! - `docs/settings.md`: main config `~/.omp/agent/config.yml`
//!   (`config.yaml` also accepted); custom models `models.yml`;
//!   authentication in `agent.db`, under the agent directory.
//!
//! ## Unknown-format disambiguation (#94's explicit acceptance)
//!
//! `crate::locations::oh_my_pi` already narrows the default path to the
//! `agent/` subdirectory specifically (not the bare `~/.omp` wrapper),
//! which most unrelated `~/.omp` users (the issue names oh-my-posh as one
//! to check) would have no reason to create. This adapter adds a second,
//! independent check: before identifying anything, it looks for at least
//! one of this format's own content markers (`config.yml`, `config.yaml`,
//! `agent.db`, `sessions/`, `blobs/`). If none are present but the
//! directory exists and is non-empty, this adapter reports one
//! `Unclassified`, non-actionable "unknown format" unit for the whole
//! directory and identifies nothing further -- never a guess.
//!
//! The same discipline holds per session: this adapter parses **only**
//! the documented title-slot shape, through the neutral mechanics in
//! `crate::agents::pi_family`. A header it cannot parse is an explicit
//! unknown-format outcome (`ProjectLinkState::Unresolved` with a stated
//! reason plus a note on the unit), never a retry against the shape some
//! other tool documents -- an adapter names no other adapter, so a change
//! to the upstream project's format can never silently change this
//! tool's identification
//! (`.oh/guardrails/agent-adapters-are-pluggable.md`).
//!
//! ## Shared blobs (#94's named concern)
//!
//! A blob is content-addressed and may be referenced by more than one
//! session's `image_url` fields. Establishing *complete* reference
//! coverage would mean reading every session body in full, which this
//! adapter deliberately does not do (see `MAX_BODY_SCAN_BYTES`): each
//! session is read exactly once per pass, up to that bound, and the same
//! bounded text yields both the session header and any
//! `blob:sha256:<hash>` reference tokens -- never anything else in the
//! body, and never persisted or logged as text, only folded into hash
//! counts in memory. That one read goes through
//! `IdentifyCtx::derived`, so an unchanged session costs **zero** header
//! bytes on a second pass. A session whose body exceeds the bound is
//! marked with incomplete coverage, and if *any* session in this pass had
//! incomplete coverage, every blob's reference count is reported as
//! unknown rather than a possibly-wrong number. No blob is ever offered a
//! selective action in this chunk (`AgentActionCapability::None`,
//! unconditionally) -- reference-based GC is out of scope here, not
//! merely gated, so "block GC until complete" is trivially true rather
//! than a runtime check this adapter could get wrong.

use super::{
    AdapterCapabilities, AgentActionCapability, AgentAdapter, AgentCategory, AgentMember,
    AgentMemberKind, AgentUnitBuilder, CandidateAgentUnit, IdentifyCtx, mtime_secs, pi_family,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const OH_MY_PI_TOOL_ID: &str = "oh-my-pi";

/// Entry budget owned outright by **one** container, never shared with a
/// sibling: a day directory's contents must not depend on how many files
/// the directories before it produced, or rows stored under that rule
/// could not be replayed into a pass that reached the container
/// differently.
const MAX_CONTAINER_ENTRIES: usize = 20_000;
/// How many containers one pass identifies at all. Also decided by the
/// tree alone.
const MAX_CONTAINERS: usize = 20_000;
/// The fold bound for the whole-directory measurements below (terminal
/// breadcrumbs, the home residual): unchanged, and unrelated to the
/// per-container session budget above.
const MAX_FOLD_ENTRIES: usize = 200_000;
const MAX_WALK_DEPTH: usize = 4;
/// Bound on how many bytes of a session's *body* (from the very start of
/// the file, the fixed-width title slot included) this adapter reads.
/// Deliberately larger than a header line needs -- blob references can
/// appear anywhere in the conversation, not just the header -- but still
/// bounded, and still one single read per session: see the module doc
/// comment on why a session exceeding this is marked incomplete rather
/// than fully read.
const MAX_BODY_SCAN_BYTES: usize = 65_536;

/// The only layout this tool documents: the header line follows the
/// fixed-width, 256-byte `type: "title"` slot (`docs/session.md`). A
/// single-element list on purpose -- the list is what this adapter is
/// *willing* to accept, and it accepts nothing it has no primary source
/// for.
const ACCEPTED_LAYOUTS: &[pi_family::HeaderLayout] = &[pi_family::HeaderLayout::AfterTitleSlot];

/// Separates the header part from the blob-reference part of one cached
/// derived value; blob hashes are separated from each other by
/// [`REF_SEP`]. Both are C0 controls, which neither a declared path nor
/// a hex hash can contain.
const PART_SEP: char = '\u{2}';
const REF_SEP: char = '\u{1}';

pub struct Adapter;

impl AgentAdapter for Adapter {
    fn id(&self) -> &'static str {
        OH_MY_PI_TOOL_ID
    }
    fn name(&self) -> &'static str {
        "Oh My Pi"
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
    if !has_format_markers(home, ctx) {
        return unknown_format_residual(home, ctx);
    }
    let mut units = Vec::new();
    // One pass over the session files: the same single bounded read per
    // session yields its header and its blob references, so the blob
    // accounting below costs no extra bytes.
    let refs = identify_sessions(home, ctx, &mut units);
    identify_blobs(home, ctx, &refs, &mut units);
    identify_static_categories(home, ctx, &mut units);
    units
}

/// The home-wide shared-blob reference picture, summed from one partial
/// per session container plus the loose sessions.
///
/// `complete` is what makes a count printable: it is false as soon as
/// any session's body exceeded the scan bound or could not be read
/// **and** as soon as any replayed container's stored partial is
/// missing. Either way every blob's count is reported as unknown; a
/// count that is quietly short is the number a future reference-based GC
/// would act on, so it must never be presented as complete.
#[derive(Default)]
struct BlobReferences {
    counts: HashMap<String, usize>,
    complete: bool,
}

/// One container's partial, as the strings
/// [`IdentifyCtx::container_with_facts`] stores and replays verbatim:
/// `"<64-hex hash> <count>"` per referenced blob, plus the single token
/// [`TRUNCATED_FACT`] when that container's own coverage was incomplete.
const TRUNCATED_FACT: &str = "truncated";

fn encode_refs(counts: &HashMap<String, usize>, truncated: bool) -> Vec<String> {
    let mut out: Vec<String> = counts
        .iter()
        .map(|(hash, n)| format!("{hash} {n}"))
        .collect();
    // Sorted so a container's stored partial does not depend on hash-map
    // iteration order: an unstable encoding would make two identical
    // passes store different rows.
    out.sort();
    if truncated {
        out.push(TRUNCATED_FACT.to_string());
    }
    out
}

/// Folds one container's partial into the running total. `None` is an
/// unrecorded partial, which makes the whole aggregate incomplete.
fn fold_refs(into: &mut BlobReferences, facts: Option<&[String]>) {
    let Some(facts) = facts else {
        into.complete = false;
        return;
    };
    for fact in facts {
        if fact == TRUNCATED_FACT {
            into.complete = false;
            continue;
        }
        let Some((hash, n)) = fact.split_once(' ') else {
            // A partial this binary cannot read back is a partial it
            // does not have.
            into.complete = false;
            continue;
        };
        match n.parse::<usize>() {
            Ok(n) => *into.counts.entry(hash.to_string()).or_insert(0) += n,
            Err(_) => into.complete = false,
        }
    }
}

fn has_format_markers(home: &Path, ctx: &IdentifyCtx) -> bool {
    ["config.yml", "config.yaml", "agent.db", "sessions", "blobs"]
        .iter()
        .any(|rel| ctx.exists(&home.join(rel)))
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
    let (bytes, mtime, _truncated) = ctx.folded_bytes(home, MAX_FOLD_ENTRIES);
    vec![
        AgentUnitBuilder::new(
            OH_MY_PI_TOOL_ID,
            AgentCategory::Unclassified,
            home.to_path_buf(),
        )
        .relative_path("(unknown format)")
        .bytes(bytes)
        .mtime_max(mtime)
        .action(AgentActionCapability::None)
        .note(
            "no Oh My Pi content markers found (config.yml/config.yaml/agent.db/sessions/blobs) \
             at this resolved path; this directory may belong to a different tool, be empty, or \
             use an unsupported Oh My Pi version -- treated as unknown format, not scanned \
             further",
        )
        .build(),
    ]
}

// ---------------------------------------------------------------------
// Sessions (and, from the same bounded read, blob references)
// ---------------------------------------------------------------------

/// Identifies every session unit and returns the home-wide blob
/// reference picture.
///
/// Each immediate subdirectory of `sessions/` is a container, and each
/// container carries its **own** blob-reference partial in its stored
/// rows. That is what lets a pass replay some containers and re-identify
/// others and still print one right total: the replayed containers
/// contribute the partials they were stored with, the re-identified ones
/// contribute fresh partials, and a container whose stored partial is
/// missing makes the total unknown rather than short.
///
/// Before 2026-09-22 this adapter opted out of the container seam for
/// exactly this reason -- a partially replayed pass would have counted
/// only the sessions it identified and printed a number that was wrong
/// rather than unknown. Storing the partial with the container is the
/// conversion route that keeps the count honest.
fn identify_sessions(
    home: &Path,
    ctx: &IdentifyCtx,
    out: &mut Vec<CandidateAgentUnit>,
) -> BlobReferences {
    let base = home.join("sessions");
    let mut refs = BlobReferences {
        counts: HashMap::new(),
        complete: true,
    };
    let mut loose = Vec::new();
    let mut containers = 0usize;
    let (listed, truncation) = ctx.list_checked(&base);
    // Containers past the listing's cap are never folded -- with a
    // partial or without one -- so a count over the rest is short and
    // must not be printed as complete (re-review 3, F1).
    if truncation.is_truncated() {
        refs.complete = false;
    }
    for entry in listed {
        let path = base.join(&entry.name);
        if entry.is_dir {
            if containers >= MAX_CONTAINERS {
                // A pass-level cap on how many containers are identified
                // at all. Decided by the tree alone, never by what a
                // sibling produced, so a container's contents can be
                // replayed into a pass that reached it differently. The
                // containers it skips are unfolded, so the aggregate is
                // no longer complete either.
                refs.complete = false;
                break;
            }
            containers += 1;
            let (units, facts) = ctx.container_with_facts(OH_MY_PI_TOOL_ID, &path, &|| {
                let mut files = Vec::new();
                let listing_truncated = collect_files(&path, 1, ctx, &mut files);
                let mut units = Vec::new();
                let (counts, truncated) = session_units(home, files, ctx, &mut units);
                (units, encode_refs(&counts, truncated || listing_truncated))
            });
            match &facts {
                super::ContainerFacts::Recorded(f) => fold_refs(&mut refs, Some(f)),
                super::ContainerFacts::Unrecorded => fold_refs(&mut refs, None),
            }
            out.extend(units);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            loose.push(path);
        }
    }
    // Session files sitting directly in `sessions/` belong to no
    // container and are identified every pass; their partial is always
    // fresh.
    let (counts, truncated) = session_units(home, loose, ctx, out);
    fold_refs(&mut refs, Some(&encode_refs(&counts, truncated)));
    refs
}

/// The session units for `files`, and this group's blob-reference
/// partial: the counts it observed, and whether any of its sessions'
/// coverage was incomplete (body larger than the scan bound, or
/// unreadable).
fn session_units(
    home: &Path,
    files: Vec<PathBuf>,
    ctx: &IdentifyCtx,
    out: &mut Vec<CandidateAgentUnit>,
) -> (HashMap<String, usize>, bool) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut any_truncated = false;
    for jsonl in files {
        let Ok(meta) = ctx.stat(&jsonl) else {
            // Coverage for this session is unknown, which makes every
            // blob count unknown -- never silently complete.
            any_truncated = true;
            continue;
        };
        if meta.len() as usize > MAX_BODY_SCAN_BYTES {
            any_truncated = true;
        }
        let bytes = meta.len();
        let mtime = mtime_secs(&meta);
        let (header, hashes) = session_facts(ctx, &jsonl);
        for hash in hashes {
            *counts.entry(hash).or_insert(0) += 1;
        }
        let unknown_format = header.is_empty();
        // Declared, not resolved: a container may only be replayed
        // around linkage the shared layer can re-resolve live, or a
        // replay would report a worktree that was deleted or moved
        // between two passes. `additionalDirectories` rides along, so
        // the `Shared` widening is redone on replay rather than frozen.
        let mut unit =
            AgentUnitBuilder::new(OH_MY_PI_TOOL_ID, AgentCategory::Sessions, jsonl.clone())
                .relative_to(home)
                .members(vec![AgentMember {
                    path: jsonl,
                    bytes,
                    kind: AgentMemberKind::Transcript,
                }])
                .mtime_max(mtime)
                .project_link_declared_workspace(
                    header.cwd.clone(),
                    header.additional_directories.clone(),
                    &pi_family::no_layout_matched_reason(ACCEPTED_LAYOUTS),
                )
                .action(AgentActionCapability::SessionRemoval);
        if unknown_format {
            unit = unit.note(
                "unknown-format session header: no JSON header line after the documented \
                 256-byte title slot; another tool's header shape is deliberately never tried \
                 here, so this session's project linkage stays unresolved",
            );
        }
        out.push(unit.build());
    }
    (counts, any_truncated)
}

/// Collects a container's session files within its depth and entry
/// bounds. Returns whether any bound or listing cap cut the collection
/// short: a container whose files were not all seen has an incomplete
/// reference partial, exactly as a truncated session body does.
fn collect_files(dir: &Path, depth: usize, ctx: &IdentifyCtx, out: &mut Vec<PathBuf>) -> bool {
    if depth > MAX_WALK_DEPTH || out.len() >= MAX_CONTAINER_ENTRIES {
        return true;
    }
    let (listed, truncation) = ctx.list_checked(dir);
    let mut cut = truncation.is_truncated();
    for entry in listed {
        if out.len() >= MAX_CONTAINER_ENTRIES {
            return true;
        }
        let path = dir.join(&entry.name);
        if entry.is_dir {
            cut |= collect_files(&path, depth + 1, ctx, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    cut
}

/// The two facts this adapter takes from one session file, from **one**
/// cached, bounded read: its declared workspace roots, and the blob
/// hashes its (bounded) body references.
fn session_facts(ctx: &IdentifyCtx, path: &Path) -> (pi_family::SessionHeader, Vec<String>) {
    let encoded = ctx.derived(
        OH_MY_PI_TOOL_ID,
        "session-header-and-blob-refs",
        path,
        MAX_BODY_SCAN_BYTES,
        &|text| {
            let header = pi_family::encode_header(&pi_family::parse_header(text, ACCEPTED_LAYOUTS))
                .unwrap_or_default();
            let hashes = extract_blob_hashes(text);
            if header.is_empty() && hashes.is_empty() {
                return None;
            }
            Some(format!(
                "{header}{PART_SEP}{}",
                hashes.join(&REF_SEP.to_string())
            ))
        },
    );
    let Some(encoded) = encoded else {
        return (pi_family::SessionHeader::default(), Vec::new());
    };
    let mut parts = encoded.splitn(2, PART_SEP);
    let header = pi_family::decode_header(parts.next().unwrap_or_default());
    let hashes = parts
        .next()
        .unwrap_or_default()
        .split(REF_SEP)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (header, hashes)
}

// ---------------------------------------------------------------------
// Shared blobs
// ---------------------------------------------------------------------

const BLOB_PREFIX: &str = "blob:sha256:";

fn extract_blob_hashes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find(BLOB_PREFIX) {
        let after = &rest[idx + BLOB_PREFIX.len()..];
        let hash: String = after
            .chars()
            .take(64)
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        let advance = hash.len().min(after.len());
        if hash.len() == 64 {
            out.push(hash);
        }
        rest = &after[advance..];
    }
    out
}

fn identify_blobs(
    home: &Path,
    ctx: &IdentifyCtx,
    refs: &BlobReferences,
    out: &mut Vec<CandidateAgentUnit>,
) {
    let referenced = &refs.counts;
    let any_truncated = !refs.complete;
    let base = home.join("blobs");
    for entry in ctx.list(&base) {
        if entry.is_dir {
            continue;
        }
        let path = base.join(&entry.name);
        let Ok(meta) = ctx.stat(&path) else {
            continue;
        };
        let hash = entry.name.as_str();
        let note = if any_truncated {
            "shared content-addressed blob; reference coverage unknown this pass (a session \
             listing was cut at its cap or could not be read, the container cap was reached, or \
             a session's body exceeded the scan bound) -- not offered for removal"
                .to_string()
        } else {
            match referenced.get(hash) {
                Some(n) if *n > 0 => format!(
                    "shared content-addressed blob; referenced by {n} known session(s) in this \
                     pass's full coverage -- not offered for removal without a supported GC \
                     action"
                ),
                _ => "shared content-addressed blob; no referencing session found in this \
                      pass's full coverage -- not offered for removal without a supported GC \
                      action"
                    .to_string(),
            }
        };
        out.push(
            AgentUnitBuilder::new(OH_MY_PI_TOOL_ID, AgentCategory::Attachments, path)
                .relative_to(home)
                .bytes(meta.len())
                .mtime_max(mtime_secs(&meta))
                // Never actionable: reference-based GC is out of scope,
                // so no blob is ever offered for removal.
                .action(AgentActionCapability::None)
                .note(note)
                .build(),
        );
    }
}

// ---------------------------------------------------------------------
// Static top-level categories
// ---------------------------------------------------------------------

fn identify_static_categories(home: &Path, ctx: &IdentifyCtx, out: &mut Vec<CandidateAgentUnit>) {
    for (rel, note) in [
        ("config.yml", "main configuration"),
        (
            "config.yaml",
            "main configuration (alternate accepted filename)",
        ),
        ("models.yml", "custom model definitions"),
    ] {
        let path = home.join(rel);
        if !ctx.exists(&path) {
            continue;
        }
        let (bytes, mtime, _truncated) = ctx.folded_bytes(&path, MAX_FOLD_ENTRIES);
        out.push(
            AgentUnitBuilder::new(OH_MY_PI_TOOL_ID, AgentCategory::ProtectedConfig, path)
                .relative_path(rel)
                .bytes(bytes)
                .mtime_max(mtime)
                .protect(note)
                .action(AgentActionCapability::None)
                .note(note)
                .build(),
        );
    }

    let db = home.join("agent.db");
    if let Ok(meta) = ctx.stat(&db)
        && meta.is_file()
    {
        // The SQLite family is folded into one unit with its sidecars as
        // members and never split: `crate::actions` refuses any selective
        // action on an `AgentMemberKind::Database` path unconditionally.
        let mut members = vec![AgentMember {
            path: db.clone(),
            bytes: meta.len(),
            kind: AgentMemberKind::Database,
        }];
        let mut bytes = meta.len();
        let mut mtime_max = mtime_secs(&meta);
        for ext in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{ext}", db.display()));
            if let Ok(sm) = ctx.stat(&sidecar)
                && sm.is_file()
            {
                bytes += sm.len();
                mtime_max = mtime_max.max(mtime_secs(&sm));
                members.push(AgentMember {
                    path: sidecar,
                    bytes: sm.len(),
                    kind: AgentMemberKind::Database,
                });
            }
        }
        out.push(
            AgentUnitBuilder::new(OH_MY_PI_TOOL_ID, AgentCategory::ProtectedConfig, db)
                .relative_path("agent.db")
                .bytes(bytes)
                .members_keep_bytes(members)
                .mtime_max(mtime_max)
                .protect(
                    "authentication database (SQLite); contents are never read by this adapter",
                )
                .action(AgentActionCapability::None)
                .build(),
        );
    }

    let terminal = home.join("terminal-sessions");
    if ctx.is_dir(&terminal) {
        let (bytes, mtime, truncated) = ctx.folded_bytes(&terminal, MAX_FOLD_ENTRIES);
        let unit = AgentUnitBuilder::new(OH_MY_PI_TOOL_ID, AgentCategory::Logs, terminal)
            .relative_path("terminal-sessions")
            .bytes(bytes)
            .mtime_max(mtime)
            .action(AgentActionCapability::CacheOrLogTrash)
            .note("terminal breadcrumb files; regenerated automatically");
        let unit = if truncated {
            unit.incomplete("directory entry count bound reached")
        } else {
            unit
        };
        out.push(unit.build());
    }

    let seen: HashSet<&str> = [
        "config.yml",
        "config.yaml",
        "models.yml",
        "agent.db",
        "agent.db-wal",
        "agent.db-shm",
        "terminal-sessions",
        "sessions",
        "blobs",
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
        let (bytes, mtime, _truncated) =
            ctx.folded_bytes(&home.join(&entry.name), MAX_FOLD_ENTRIES);
        residual_bytes += bytes;
        residual_mtime = residual_mtime.max(mtime);
        residual_names.push(entry.name);
    }
    if !residual_names.is_empty() {
        residual_names.sort();
        out.push(
            AgentUnitBuilder::new(
                OH_MY_PI_TOOL_ID,
                AgentCategory::Unclassified,
                home.to_path_buf(),
            )
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
    use crate::agents::{IdentificationCache, ProjectLinkState, contract};
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

    /// A session file in Oh My Pi's own documented shape: a fixed-width
    /// 256-byte title slot, then the header line.
    fn session_bytes(cwd: &str, additional: &[&str], canary: &str) -> Vec<u8> {
        let mut title = vec![b' '; 256];
        let title_json = b"{\"type\":\"title\"}";
        title[..title_json.len()].copy_from_slice(title_json);
        title[255] = b'\n';
        let extra = additional
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(",");
        let header = format!(
            "{{\"type\":\"session\",\"cwd\":\"{cwd}\",\"additionalDirectories\":[{extra}]}}\n"
        );
        let entry = format!(
            "{{\"id\":\"1\",\"parentId\":null,\"timestamp\":0,\"content\":\"{canary}\"}}\n"
        );
        let mut out = title;
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(entry.as_bytes());
        out
    }

    #[test]
    fn empty_home_yields_no_units() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path()).is_empty());
    }

    #[test]
    fn no_format_markers_yields_unknown_format_not_a_guess() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("unrelated-file.txt"), b"hello");
        let units = run(dir.path());
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].relative_path(), "(unknown format)");
        assert_eq!(units[0].action(), AgentActionCapability::None);
    }

    #[test]
    fn a_session_is_identified_and_linked_via_cwd() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        touch(&home.join("config.yml"), b"providers: {}");
        let canary = "CANARY-OMP-DO-NOT-LEAK-77bb";
        let jsonl = home
            .join("sessions/-fixture-repo/1700000000_11111111-1111-4111-8111-111111111111.jsonl");
        touch(
            &jsonl,
            &session_bytes(&repo.display().to_string(), &[], canary),
        );
        let units = run(home);
        let session = units
            .iter()
            .find(|u| u.category() == AgentCategory::Sessions && u.path == jsonl)
            .expect("session identified");
        assert!(matches!(
            session.project_link(),
            ProjectLinkState::Linked { .. }
        ));
        assert_eq!(session.action(), AgentActionCapability::SessionRemoval);
        let serialized = format!("{units:?}");
        assert!(!serialized.contains(canary), "prompt content leaked");
    }

    #[test]
    fn additional_directories_naming_a_different_project_yields_shared() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let repo_a = home.join("repo-a");
        let repo_b = home.join("repo-b");
        fs::create_dir_all(repo_a.join(".git")).unwrap();
        fs::create_dir_all(repo_b.join(".git")).unwrap();
        touch(&home.join("config.yml"), b"providers: {}");
        let jsonl = home.join("sessions/x/1_22222222-2222-4222-8222-222222222222.jsonl");
        touch(
            &jsonl,
            &session_bytes(
                &repo_a.display().to_string(),
                &[&repo_b.display().to_string()],
                "x",
            ),
        );
        let units = run(home);
        let session = units.iter().find(|u| u.path == jsonl).unwrap();
        assert!(matches!(
            session.project_link(),
            ProjectLinkState::Shared { .. }
        ));
    }

    #[test]
    fn blobs_report_reference_counts_from_full_coverage() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        let hash = "a".repeat(64);
        touch(&home.join("blobs").join(&hash), b"binary-image-bytes");
        let jsonl = home.join("sessions/x/1_33333333-3333-4333-8333-333333333333.jsonl");
        let mut body = session_bytes("/no/such/dir", &[], "unread");
        body.extend_from_slice(format!("{{\"image_url\":\"blob:sha256:{hash}\"}}\n").as_bytes());
        touch(&jsonl, &body);
        let units = run(home);
        let blob = units
            .iter()
            .find(|u| u.category() == AgentCategory::Attachments)
            .expect("blob unit present");
        assert_eq!(blob.action(), AgentActionCapability::None);
        assert!(blob.note.as_deref().unwrap().contains("referenced by 1"));
    }

    #[test]
    fn unreferenced_blob_says_so_without_offering_gc() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        let hash = "b".repeat(64);
        touch(&home.join("blobs").join(&hash), b"binary-image-bytes");
        let units = run(home);
        let blob = units
            .iter()
            .find(|u| u.category() == AgentCategory::Attachments)
            .unwrap();
        assert_eq!(blob.action(), AgentActionCapability::None);
        assert!(
            blob.note
                .as_deref()
                .unwrap()
                .contains("no referencing session found")
        );
    }

    #[test]
    fn a_session_larger_than_the_scan_bound_makes_every_blob_count_unknown() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        let hash = "c".repeat(64);
        touch(&home.join("blobs").join(&hash), b"binary-image-bytes");
        let jsonl = home.join("sessions/x/1_44444444-4444-4444-8444-444444444444.jsonl");
        let mut body = session_bytes("/no/such/dir", &[], "unread");
        body.extend_from_slice(&b"x".repeat(MAX_BODY_SCAN_BYTES));
        touch(&jsonl, &body);
        let units = run(home);
        let blob = units
            .iter()
            .find(|u| u.category() == AgentCategory::Attachments)
            .unwrap();
        assert_eq!(blob.action(), AgentActionCapability::None);
        assert!(
            blob.note
                .as_deref()
                .unwrap()
                .contains("reference coverage unknown"),
            "{:?}",
            blob.note
        );
    }

    #[test]
    fn agent_db_is_protected_and_folds_sidecars() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("agent.db"), b"sqlite");
        touch(&home.join("agent.db-wal"), b"wal");
        let units = run(home);
        let db = units
            .iter()
            .find(|u| u.relative_path() == "agent.db")
            .unwrap();
        assert!(db.protected());
        assert_eq!(db.action(), AgentActionCapability::None);
        assert_eq!(db.members().len(), 2);
        assert!(
            db.members()
                .iter()
                .all(|m| m.kind == AgentMemberKind::Database)
        );
    }

    #[test]
    fn terminal_sessions_are_actionable_cache_or_log_trash() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        touch(&home.join("terminal-sessions").join("t1"), b"breadcrumb");
        let units = run(home);
        let u = units
            .iter()
            .find(|u| u.relative_path() == "terminal-sessions")
            .unwrap();
        assert!(!u.protected());
        assert_eq!(u.action(), AgentActionCapability::CacheOrLogTrash);
    }

    #[test]
    fn identification_cost_is_bounded_for_many_sessions() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        let repo = home.join("big-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        for i in 0..500 {
            let jsonl = home.join(format!(
                "sessions/-big-repo/{i}_77777777-7777-4777-8{i:03}-777777777777.jsonl"
            ));
            let mut body = session_bytes(&repo.display().to_string(), &[], "unread-canary");
            body.extend_from_slice(&b"x".repeat(200_000));
            touch(&jsonl, &body);
        }
        let start = SystemTime::now();
        let units = run(home);
        let elapsed = SystemTime::now().duration_since(start).unwrap_or_default();
        eprintln!("[measured] oh_my_pi identify() over 500 synthetic sessions took {elapsed:?}");
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
        touch(&dir.path().join("unrelated-file.txt"), b"hello");
        let units = run(dir.path());
        assert_eq!(units.len(), 1, "an unrecognized home must still surface");
        assert_eq!(units[0].relative_path(), "(unknown format)");
        assert!(
            units[0]
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("unknown format")
        );

        // (b) a session whose header is not in this tool's documented
        // shape -- here the upstream project's offset-zero header -- is
        // an explicit unresolved outcome naming what was expected, never
        // parsed by borrowing another adapter's shape.
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        let repo = home.join("fixture-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let jsonl = home.join("sessions/x/1_55555555-5555-4555-8555-555555555555.jsonl");
        touch(
            &jsonl,
            format!("{{\"id\":\"1\",\"cwd\":\"{}\"}}\n", repo.display()).as_bytes(),
        );
        let units = run(home);
        let session = units.iter().find(|u| u.path == jsonl).unwrap();
        match &session.project_link() {
            ProjectLinkState::Unresolved { reason } => assert!(
                reason.contains("title slot"),
                "the reason must name the shape that was expected: {reason}"
            ),
            other => panic!("expected an explicit unresolved outcome, got {other:?}"),
        }
        assert!(
            session
                .note
                .as_deref()
                .unwrap_or_default()
                .contains("unknown-format session header"),
            "{:?}",
            session.note
        );
    }

    #[test]
    fn canary_content_never_appears_in_output() {
        let canary = "CANARY-OMP-BODY-DO-NOT-LEAK-3f92";
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        let jsonl = home.join("sessions/x/1_66666666-6666-4666-8666-666666666666.jsonl");
        // The canary sits in the title slot (which the read covers), in
        // the header line, and in the body past it.
        let mut body = vec![b' '; 256];
        let title = format!("{{\"type\":\"title\",\"title\":\"{canary}\"}}");
        body[..title.len()].copy_from_slice(title.as_bytes());
        body[255] = b'\n';
        body.extend_from_slice(
            format!(
                "{{\"type\":\"session\",\"cwd\":\"/no/such/dir\",\"summary\":\"{canary}\"}}\n\
                 {{\"id\":\"1\",\"content\":\"{canary}\"}}\n"
            )
            .as_bytes(),
        );
        touch(&jsonl, &body);
        contract::no_content_leak(&run(home), canary);
    }

    #[test]
    fn identification_reads_no_more_than_header_cap() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        let mut fixture_bytes = 0u64;
        for i in 0..20 {
            let jsonl = home.join(format!("sessions/x/{i}_session.jsonl"));
            let mut body = session_bytes("/no/such/dir", &[], "unread");
            body.extend_from_slice(&b"x".repeat(200_000));
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
        // Exactly one bounded read per session -- the header and the
        // blob references come out of the same read, so blob accounting
        // costs nothing extra.
        contract::within_header_cap(counters, 20);
        assert!(
            counters.header_bytes_read <= 20 * MAX_BODY_SCAN_BYTES as u64,
            "read {} bytes, above this adapter's own per-session bound",
            counters.header_bytes_read
        );
        assert!(
            counters.header_bytes_read < fixture_bytes,
            "identification read {} of {fixture_bytes} fixture bytes",
            counters.header_bytes_read
        );
    }

    #[test]
    fn protected_categories_default_protected() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        touch(&home.join("models.yml"), b"models: {}");
        touch(&home.join("agent.db"), b"sqlite");
        touch(&home.join("terminal-sessions").join("t1"), b"breadcrumb");
        let units = run(home);
        contract::protection_defaults_hold(&units);
        let config = units
            .iter()
            .find(|u| u.relative_path() == "config.yml")
            .unwrap();
        assert_eq!(config.category(), AgentCategory::ProtectedConfig);
        assert_eq!(
            config.protect_reason(),
            Some("main configuration"),
            "the adapter's own, more specific reason survives the builder default"
        );
    }

    #[test]
    fn project_link_is_declared_or_unresolved_never_basename_guess() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        touch(&home.join("config.yml"), b"providers: {}");
        // (a) declared metadata naming a real worktree resolves Linked.
        let repo = home.join("declared-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let declared = home.join("sessions/-declared-repo/1_declared.jsonl");
        touch(
            &declared,
            &session_bytes(&repo.display().to_string(), &[], "body"),
        );
        // (b) a session inside a directory *named* after a real worktree
        // but declaring nothing: unresolved, never linked.
        let guessable = home.join("guessable-repo");
        fs::create_dir_all(guessable.join(".git")).unwrap();
        let undeclared = home.join("sessions/guessable-repo/2_undeclared.jsonl");
        let mut body = vec![b' '; 256];
        body[255] = b'\n';
        body.extend_from_slice(b"{\"type\":\"session\"}\n");
        touch(&undeclared, &body);

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
