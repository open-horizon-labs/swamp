//! Folder-name inference for Claude Code sessions whose declared `cwd`
//! is absent or cannot resolve (`crate::agents::KnownWorktrees`).
//!
//! The link is inferred only when the session's `projects/<slug>`
//! folder name re-encodes exactly one of this pass's known worktree
//! paths, is labelled `LinkSource::Inferred`, and is re-derived against
//! the current known set on every pass -- never replayed from the store.
//! Every case below is adversarial against the tempting shortcut it
//! names:
//!
//! - unique slug match -> Inferred (the feature)
//! - a declared `cwd` beats the folder name (no "folder wins" shortcut)
//! - a declared `cwd` that is missing can fall back to one exact full-path
//!   slug match, preserving the failure reason and inferred provenance
//! - a declared `cwd` that is missing with no slug match stays Missing
//! - `/a/b-c` vs `/a-b/c` collide under the lossy encoding -> Unresolved
//!   (no "pick the first" shortcut)
//! - the same worktree listed twice is one candidate, not a collision
//! - a known path that is no longer a checkout -> Unresolved naming it
//!   (no basename link, no `Moved` without prior identity)
//! - a Codex session has no slug -> unchanged Unresolved
//! - a replayed container re-derives the inference from the *current*
//!   known set: removed worktree -> Unresolved; restored -> Inferred
//!
//! Disposable `tempfile` fixtures only; synthetic transcripts with a
//! canary that must never leave the fixture.

use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::agents::{
    CandidateAgentUnit, ContainerCache, IdentificationCache, IdentifyCtx, KnownWorktrees,
    LinkSource, ProjectLinkState, claude_folder_slug,
};
use swamp_core::fs_events::EventCoverage;

const CANARY: &str = "CANARY-folder-inference-8d1f";

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn checkout(path: &Path) {
    fs::create_dir_all(path.join(".git")).unwrap();
}

/// A session whose first records carry no `cwd` at all, under the
/// folder Claude Code would have created for `workspace`.
fn session_without_cwd(home: &Path, workspace: &Path, id: &str) -> PathBuf {
    let jsonl = home
        .join("projects")
        .join(claude_folder_slug(workspace))
        .join(format!("{id}.jsonl"));
    write(
        &jsonl,
        &format!(
            "{{\"type\":\"queue-operation\",\"operation\":\"enqueue\"}}\n\
             {{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{CANARY}\"}}}}\n"
        ),
    );
    jsonl
}

/// A session declaring `cwd`, under an arbitrary folder name.
fn session_with_cwd(home: &Path, folder: &str, cwd: &Path, id: &str) -> PathBuf {
    let jsonl = home
        .join("projects")
        .join(folder)
        .join(format!("{id}.jsonl"));
    write(
        &jsonl,
        &format!(
            "{{\"type\":\"user\",\"cwd\":\"{}\",\"message\":{{\"role\":\"user\",\"content\":\"{CANARY}\"}}}}\n",
            cwd.display()
        ),
    );
    jsonl
}

/// One pass of the Claude adapter with the shared layer's link
/// finishing, exactly as `discover_and_measure_in` runs it: a container
/// cache loaded from `store` under `coverage`, this pass's known
/// worktrees, and `finish_link` on every unit.
fn pass(
    home: &Path,
    store: &Path,
    at: u64,
    coverage: EventCoverage,
    known: &[PathBuf],
) -> Vec<CandidateAgentUnit> {
    let cache = IdentificationCache::load(store);
    let containers = ContainerCache::load(store, coverage).with_known_worktrees(known);
    let ctx = IdentifyCtx::with_containers(at, &cache, &containers);
    let mut units = swamp_core::agents::claude_code::identify(home, &ctx);
    for u in units.iter_mut() {
        containers.finish_link(u);
    }
    cache.save(store, at).unwrap();
    containers.save(store, at).unwrap();
    units
}

fn link_of<'a>(units: &'a [CandidateAgentUnit], path: &Path) -> &'a ProjectLinkState {
    units
        .iter()
        .find(|u| u.path() == path)
        .unwrap_or_else(|| panic!("no unit for {}", path.display()))
        .project_link()
}

#[test]
fn the_slug_encoding_is_the_documented_one_and_is_lossy() {
    assert_eq!(
        claude_folder_slug(Path::new("/Users/dev/src/open-horizon-labs/swamp")),
        "-Users-dev-src-open-horizon-labs-swamp"
    );
    assert_eq!(
        claude_folder_slug(Path::new("/Users/dev/.codex/.chatgpt")),
        "-Users-dev--codex--chatgpt"
    );
    // The collision this whole file guards against.
    assert_eq!(
        claude_folder_slug(Path::new("/a/b-c")),
        claude_folder_slug(Path::new("/a-b/c"))
    );
    // Documented normalization for the rest: `_`, `.` and every
    // non-ASCII character become one `-` each. If Claude Code encodes
    // some class differently the slugs simply never match -- no link,
    // never a wrong one.
    assert_eq!(claude_folder_slug(Path::new("/x/a_b.c")), "-x-a-b-c");
    assert_eq!(claude_folder_slug(Path::new("/x/café")), "-x-caf-");
}

/// The provenance a consumer sees: JSON carries `"source":"inferred"`
/// through the existing serde shape, including the reason for fallback, and the text view
/// says in words that the link was inferred from the folder name --
/// so an inferred link can never be mistaken for a declared one in any
/// output.
#[test]
fn an_inferred_link_is_labelled_inferred_in_json_and_text() {
    let link = ProjectLinkState::Linked {
        project_id: "p".into(),
        project_name: "repo".into(),
        project_path: PathBuf::from("/x/repo"),
        source: LinkSource::Inferred,
        fallback_reason: Some("declared cwd missing: /old/repo".into()),
        worktree_kind: "main".into(),
    };
    let json = serde_json::to_value(&link).unwrap();
    assert_eq!(json["state"], "linked");
    assert_eq!(json["source"], "inferred");

    let unit = swamp_core::agents::AgentUnit {
        tool_id: "claude-code".into(),
        tool_name: "Claude Code".into(),
        tool_home: PathBuf::from("/x/.claude"),
        category: swamp_core::agents::AgentCategory::Sessions,
        id: "s1".into(),
        relative_path: "projects/-x-repo/s1.jsonl".into(),
        path: PathBuf::from("/x/.claude/projects/-x-repo/s1.jsonl"),
        members: Vec::new(),
        bytes: 1024,
        hardlinked: true,
        complete: true,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 1_000,
        mtime_max: 900,
        protected: false,
        protect_reason: None,
        project_link: link,
        action: swamp_core::agents::AgentActionCapability::SessionRemoval,
        note: None,
        evidence: Vec::new(),
    };
    let text =
        swamp_core::render::render_view_agents(std::slice::from_ref(&unit), None, true, 1_000);
    assert!(
        text.contains("inferred from the tool's project folder name, not declared"),
        "{text}"
    );
    // And the `--project` filter still finds it by name: an inferred
    // link is a link, labelled.
    let filtered = swamp_core::render::render_view_agents(
        std::slice::from_ref(&unit),
        Some("repo"),
        true,
        1_000,
    );
    assert!(filtered.contains("s1.jsonl"), "{filtered}");
}

#[test]
fn a_unique_folder_match_is_an_inferred_link_never_claimed_as_declared() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, store) = (root.join("home"), root.join("store"));
    let repo = root.join("src").join("real-repo");
    checkout(&repo);
    let jsonl = session_without_cwd(&home, &repo, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");

    let units = pass(
        &home,
        &store,
        1_000,
        EventCoverage::untrusted(),
        &[repo.clone()],
    );
    match link_of(&units, &jsonl) {
        ProjectLinkState::Linked {
            source,
            project_path,
            ..
        } => {
            assert_eq!(*source, LinkSource::Inferred, "never claimed as declared");
            assert_eq!(project_path, &repo);
        }
        other => panic!("expected an inferred link, got {other:?}"),
    }
    // Without a known set (a bare adapter run, any tool that has no
    // known worktrees this pass) the same session is unresolved: the
    // folder name alone is never an answer.
    let units = pass(&home, &store, 2_000, EventCoverage::untrusted(), &[]);
    assert!(
        matches!(link_of(&units, &jsonl), ProjectLinkState::Unresolved { .. }),
        "{:?}",
        link_of(&units, &jsonl)
    );
}

#[test]
fn a_declared_cwd_beats_the_folder_name_and_a_failed_cwd_can_use_unique_folder_slug() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, store) = (root.join("home"), root.join("store"));
    let folder_repo = root.join("src").join("folder-repo");
    let declared_repo = root.join("src").join("declared-repo");
    checkout(&folder_repo);
    checkout(&declared_repo);
    // Sits in folder-repo's folder, declares declared-repo.
    let declared = session_with_cwd(
        &home,
        &claude_folder_slug(&folder_repo),
        &declared_repo,
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
    );
    // Sits in folder-repo's folder, declares a path that is gone.
    let gone = root.join("src").join("gone-repo");
    let missing = session_with_cwd(
        &home,
        &claude_folder_slug(&folder_repo),
        &gone,
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    );

    let known = vec![folder_repo.clone(), declared_repo.clone()];
    let units = pass(&home, &store, 1_000, EventCoverage::untrusted(), &known);
    match link_of(&units, &declared) {
        ProjectLinkState::Linked {
            source,
            project_path,
            ..
        } => {
            assert_eq!(*source, LinkSource::Declared);
            assert_eq!(
                project_path, &declared_repo,
                "the declared cwd wins over the folder the session sits in"
            );
        }
        other => panic!("{other:?}"),
    }
    match link_of(&units, &missing) {
        ProjectLinkState::Linked {
            source,
            project_path,
            fallback_reason,
            ..
        } => {
            assert_eq!(*source, LinkSource::Inferred);
            assert_eq!(project_path, &folder_repo);
            assert!(
                fallback_reason
                    .as_deref()
                    .unwrap()
                    .contains(&gone.display().to_string())
            );
        }
        other => panic!("unique folder slug should be an explicitly inferred fallback: {other:?}"),
    }

    // No unique matching known worktree: keep the original typed failure.
    let only_elsewhere = root.join("elsewhere").join("other-repo");
    checkout(&only_elsewhere);
    let units = pass(
        &home,
        &store,
        2_000,
        EventCoverage::untrusted(),
        &[only_elsewhere],
    );
    assert_eq!(
        link_of(&units, &missing),
        &ProjectLinkState::Missing { path: gone },
        "without a unique slug candidate preserve the declared-path failure"
    );
}

#[test]
fn a_non_project_cwd_can_fall_back_to_the_exact_unique_tool_folder_slug() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, store) = (root.join("home"), root.join("store"));
    let repo = root.join("src").join("repo");
    checkout(&repo);
    let plain = root.join("not-a-checkout");
    fs::create_dir_all(&plain).unwrap();
    let jsonl = session_with_cwd(
        &home,
        &claude_folder_slug(&repo),
        &plain,
        "dddddddd-dddd-4ddd-8ddd-ddddddddddde",
    );
    let units = pass(
        &home,
        &store,
        1_000,
        EventCoverage::untrusted(),
        &[repo.clone()],
    );
    match link_of(&units, &jsonl) {
        ProjectLinkState::Linked {
            source,
            project_path,
            fallback_reason,
            ..
        } => {
            assert_eq!(*source, LinkSource::Inferred);
            assert_eq!(project_path, &repo);
            assert!(
                fallback_reason
                    .as_deref()
                    .unwrap()
                    .contains("not a known checkout")
            );
        }
        other => panic!("expected an inferred fallback, got {other:?}"),
    }
}

#[test]
fn two_known_paths_that_collide_under_the_lossy_encoding_stay_unresolved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, store) = (root.join("home"), root.join("store"));
    let a = root.join("a").join("b-c");
    let b = root.join("a-b").join("c");
    checkout(&a);
    checkout(&b);
    assert_eq!(claude_folder_slug(&a), claude_folder_slug(&b));
    let jsonl = session_without_cwd(&home, &a, "dddddddd-dddd-4ddd-8ddd-dddddddddddd");

    let units = pass(
        &home,
        &store,
        1_000,
        EventCoverage::untrusted(),
        &[a.clone(), b.clone()],
    );
    match link_of(&units, &jsonl) {
        ProjectLinkState::Unresolved { reason } => {
            assert!(reason.contains("2 known worktree paths"), "{reason}");
            assert!(reason.contains("cannot tell apart"), "{reason}");
        }
        other => panic!("a colliding slug must not pick a candidate: {other:?}"),
    }

    // The same worktree listed twice is one candidate, not a collision.
    let units = pass(
        &home,
        &store,
        2_000,
        EventCoverage::untrusted(),
        &[a.clone(), a.clone()],
    );
    assert!(
        matches!(
            link_of(&units, &jsonl),
            ProjectLinkState::Linked {
                source: LinkSource::Inferred,
                ..
            }
        ),
        "{:?}",
        link_of(&units, &jsonl)
    );
}

#[test]
fn a_known_path_that_is_no_longer_a_checkout_is_named_not_linked_by_basename() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, store) = (root.join("home"), root.join("store"));
    let old = root.join("src").join("proj");
    checkout(&old);
    let jsonl = session_without_cwd(&home, &old, "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee");
    // The project moved: same basename at a new path, the old path gone.
    let moved = root.join("elsewhere").join("proj");
    checkout(&moved);
    fs::remove_dir_all(&old).unwrap();

    // Stale known set naming the old path: the folder matches it, but
    // it is not a checkout any more -> unresolved, saying so.
    let units = pass(
        &home,
        &store,
        1_000,
        EventCoverage::untrusted(),
        &[old.clone()],
    );
    match link_of(&units, &jsonl) {
        ProjectLinkState::Unresolved { reason } => {
            assert!(reason.contains("no longer a git checkout"), "{reason}");
            assert!(reason.contains(&old.display().to_string()), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    // Current known set naming only the new path: the slug does not
    // match it (a different path), and the shared basename is never
    // evidence -> plain unresolved. No `Moved` is invented either:
    // nothing ties this session to the new path.
    let units = pass(
        &home,
        &store,
        2_000,
        EventCoverage::untrusted(),
        &[moved.clone()],
    );
    match link_of(&units, &jsonl) {
        ProjectLinkState::Unresolved { reason } => {
            assert!(!reason.contains("elsewhere"), "{reason}");
        }
        other => panic!("a basename match must never link or claim a move: {other:?}"),
    }
}

#[test]
fn a_replayed_container_re_derives_the_inference_from_the_current_known_set() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, store) = (root.join("home"), root.join("store"));
    let repo = root.join("src").join("repo");
    checkout(&repo);
    let jsonl = session_without_cwd(&home, &repo, "ffffffff-ffff-4fff-8fff-ffffffffffff");
    let quiet = || EventCoverage::trusted(home.clone(), Vec::new(), 1_000);

    // Identified, inferred, stored.
    let units = pass(
        &home,
        &store,
        1_000,
        EventCoverage::untrusted(),
        &[repo.clone()],
    );
    assert!(matches!(
        link_of(&units, &jsonl),
        ProjectLinkState::Linked {
            source: LinkSource::Inferred,
            ..
        }
    ));

    // Replayed (quiet window) with the worktree gone from the known
    // set: the stored rows must not carry the old link through.
    let (units, work) =
        swamp_core::work_counters::measured(|| pass(&home, &store, 2_000, quiet(), &[]));
    assert!(
        work.containers_reused >= 1,
        "precondition: the container must be replayed, not re-identified: {work:?}"
    );
    assert!(
        matches!(link_of(&units, &jsonl), ProjectLinkState::Unresolved { .. }),
        "a replayed inferred link must be re-derived, not replayed: {:?}",
        link_of(&units, &jsonl)
    );

    // Replayed again with the worktree back: inferred again.
    let (units, work) = swamp_core::work_counters::measured(|| {
        pass(&home, &store, 3_000, quiet(), &[repo.clone()])
    });
    assert!(work.containers_reused >= 1, "{work:?}");
    assert!(matches!(
        link_of(&units, &jsonl),
        ProjectLinkState::Linked {
            source: LinkSource::Inferred,
            ..
        }
    ));

    // And the replayed unit's serialized form never leaks the canary.
    let json = serde_json::to_string(
        units
            .iter()
            .map(|u| u.project_link())
            .collect::<Vec<_>>()
            .as_slice(),
    )
    .unwrap();
    assert!(!json.contains(CANARY));
}

#[test]
fn a_codex_session_has_no_folder_signal_and_stays_unresolved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (home, store) = (root.join("codex-home"), root.join("store"));
    let repo = root.join("src").join("repo");
    checkout(&repo);
    // Codex keys sessions by date, not by workspace, so there is no
    // folder slug to infer from; its linkage is the declared `cwd` in
    // the `session_meta` record (read from the bounded prefix). This
    // transcript has none, so it must stay unresolved -- not guessed.
    let jsonl = home
        .join("sessions")
        .join("2026")
        .join("09")
        .join("25")
        .join("rollout-2026-09-25T10-00-00-00000000-0000-4000-8000-000000000000.jsonl");
    write(
        &jsonl,
        &format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"x\"}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"content\":\"{CANARY}\"}}}}\n"
        ),
    );
    let cache = IdentificationCache::load(&store);
    let containers = ContainerCache::load(&store, EventCoverage::untrusted())
        .with_known_worktrees(&[repo.clone()]);
    let ctx = IdentifyCtx::with_containers(1_000, &cache, &containers);
    let mut units = swamp_core::agents::codex::identify(&home, &ctx);
    for u in units.iter_mut() {
        containers.finish_link(u);
    }
    let unit = units
        .iter()
        .find(|u| u.path() == jsonl)
        .expect("the codex session is a unit");
    assert!(
        matches!(unit.project_link(), ProjectLinkState::Unresolved { .. }),
        "{:?}",
        unit.project_link()
    );
    assert!(
        !matches!(
            unit.link_basis(),
            swamp_core::agents::LinkBasis::Declared {
                folder_slug: Some(_),
                ..
            }
        ),
        "a Codex unit must carry no folder slug: {:?}",
        unit.link_basis()
    );
}

#[test]
fn known_worktrees_index_dedups_paths_and_answers_only_exact_slugs() {
    let a = PathBuf::from("/x/y");
    let known = KnownWorktrees::from_paths(&[a.clone(), a.clone()]);
    // A slug that matches nothing keeps the caller's reason verbatim.
    match known.infer("-x-z", "no cwd") {
        ProjectLinkState::Unresolved { reason } => assert_eq!(reason, "no cwd"),
        other => panic!("{other:?}"),
    }
    // A prefix or basename of a known path is not a match.
    assert!(matches!(
        known.infer("-x", "no cwd"),
        ProjectLinkState::Unresolved { .. }
    ));
    assert!(matches!(
        known.infer("y", "no cwd"),
        ProjectLinkState::Unresolved { .. }
    ));
}
