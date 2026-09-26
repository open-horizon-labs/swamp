//! #102: independent validation of agent-storage modeling, privacy and
//! selective cleanup, on top of (never duplicating) the extensive
//! per-adapter test files this catalog already has
//! (`crates/core/src/agents/*.rs`'s own module tests,
//! `crates/core/tests/agent_units_actions*.rs`,
//! `crates/cli/tests/agent_storage_cli.rs`,
//! `crates/core/tests/agent_refusal_matrix.rs`). This file focuses on
//! the specific cross-cutting properties #102's acceptance names that
//! those files do not already cover as a single, independent check:
//!
//! - custom roots actually redirect discovery (not merely "also work"),
//! - malformed/partial metadata never panics and is always explicit,
//! - an unrecognized/unknown schema never falls back to a destructive
//!   action capability,
//! - a shared resource's reference coverage state is explicit, never
//!   guessed, and never actionable,
//! - a canary sweep across render text, the CLI JSON contract, the
//!   plan, the execute result and the ledger together, for two
//!   differently-shaped adapters (Claude Code and Codex),
//! - nested accounting (project-tree totals equal the flat Agents-view
//!   total for the same project, no double counting),
//! - unchanged/incremental growth history and stable history after a
//!   session's project attribution changes,
//! - benchmarked scan cost for unchanged refresh, one-session append
//!   and store growth.
//!
//! PRIVACY IS A HARD RULE: every fixture in this file is synthetic and
//! sanitized; no real tool home is ever read.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use swamp_core::actions;
use swamp_core::agents::{AgentActionCapability, discover_and_measure};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::render::render_view_agents;
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

const CANARY: &str = "CANARY-VALIDATION-MUST-NEVER-LEAK-e91a4c";

fn write(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

const ALL_AGENT_DETECTOR_IDS: &[&str] = &[
    "claude-code",
    "codex",
    "codex-desktop",
    "oh-my-pi",
    "opencode",
    "gemini-cli",
    "pi",
    "aider",
    "github-copilot-cli",
    "cursor",
    "windsurf",
    "cline",
    "roo-code",
    "continue",
];

/// Disables every named agent detector except `keep`, plus the noisy
/// build-tool detectors every fixture in this file wants out of the
/// way. Centralized here so a future 15th tool only needs updating in
/// one place (this list caused a real, caught duplicate-identification
/// bug once already -- see `agent_units_actions_new_adapters.rs`'s own
/// fixed `only_detector`).
fn only(keep: &[&str]) -> ScanConfig {
    // An allow-list makes a typo silent -- an unknown id simply
    // authorizes nothing, and the fixture then "passes" by finding no
    // units. So the ids are checked against the catalog here.
    for id in keep {
        assert!(
            ALL_AGENT_DETECTOR_IDS.contains(id),
            "{id:?} is not an agent detector id; an allow-list typo would leave this fixture \
             with an empty scope and a vacuously passing assertion"
        );
    }
    ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        // An **allow-list**. A deny-list naming only the noisy build-tool
        // detectors left the rest of the catalog in scope, including
        // three detectors whose paths are machine-wide conventions no
        // injected `HOME` can relocate -- `homebrew`'s prefixes,
        // `ruby-install`'s /opt/rubies, and `core_simulator`'s
        // /Library/Developer/CoreSimulator/Volumes. A fixture that
        // reaches those reads the developer's real storage, which is
        // both wrong and slow (42 GB and ninety seconds on the machine
        // where it was found). See
        // `external_units.rs::a_detector_that_escapes_the_fixture_home_is_named_here_not_discovered_by_a_byte_total`.
        disabled_detectors: Vec::new(),
        enabled_detectors: keep.iter().copied().map(str::to_string).collect(),
    }
}

fn scope_for(
    home_root: &Path,
    env_vars: HashMap<String, String>,
    keep: &[&str],
) -> swamp_core::scope::EffectiveScope {
    let env = Environment::fixture(home_root.to_path_buf(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    resolve_effective_scope(&env, &only(keep), &[], &registry, 1_000)
}

// ---------------------------------------------------------------------
// Custom roots
// ---------------------------------------------------------------------

/// A custom root override does not merely "also work" -- it *replaces*
/// the convention path. Content placed only at the convention path
/// (which does not exist here at all) must not appear, and content at
/// the override path must.
#[test]
fn custom_root_overrides_redirect_discovery_away_from_the_convention_path() {
    for (env_var, detector_id, home_subdir, marker_rel) in [
        (
            "CLAUDE_CONFIG_DIR",
            "claude-code",
            "claude-custom",
            "settings.json",
        ),
        ("CODEX_HOME", "codex", "codex-custom", "config.toml"),
        (
            "PI_CODING_AGENT_DIR",
            "oh-my-pi",
            "omp-custom",
            "config.yml",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let custom_home = root.path().join(home_subdir);
        write(&custom_home.join(marker_rel), b"marker-content");
        // No directory at all exists at any conventional fallback name
        // under `root` (e.g. `.claude`, `.codex`, `.omp`) -- so any unit
        // found at all can only have come from the override.
        let mut env_vars = HashMap::new();
        env_vars.insert(env_var.to_string(), custom_home.display().to_string());
        let scope = scope_for(root.path(), env_vars, &[detector_id]);
        let store = tempfile::tempdir().unwrap();
        let units = discover_and_measure(
            &scope,
            &[],
            Some(store.path()),
            false,
            1_000,
            30,
            3600,
            &swamp_core::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        assert!(
            units.iter().any(|u| u.tool_home == custom_home),
            "detector {detector_id}: expected a unit whose tool_home is the override path {}, \
             got: {:?}",
            custom_home.display(),
            units.iter().map(|u| &u.tool_home).collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------
// Malformed / partial metadata never panics, always explicit
// ---------------------------------------------------------------------

/// An adapter context with the identification cache disabled: every
/// derivation reads live, which is what a validation fixture wants (a
/// cached answer would mean a second call proved nothing).
fn ctx_for(at: u64) -> (swamp_core::agents::IdentificationCache, u64) {
    (swamp_core::agents::IdentificationCache::disabled(), at)
}

macro_rules! identify_with {
    ($adapter:path, $home:expr) => {{
        let (cache, at) = ctx_for(1_000);
        let ctx = swamp_core::agents::IdentifyCtx::new(at, &cache);
        $adapter($home, &ctx)
    }};
}

#[test]
fn malformed_or_truncated_metadata_never_panics_and_is_always_explicit() {
    // Claude Code: a completely empty transcript file.
    {
        let home = tempfile::tempdir().unwrap();
        let jsonl = home
            .path()
            .join("projects")
            .join("-x")
            .join("55555555-5555-4555-8555-555555555555.jsonl");
        write(&jsonl, b"");
        let result = std::panic::catch_unwind(|| {
            identify_with!(swamp_core::agents::claude_code::identify, home.path())
        });
        let units = result.expect("an empty transcript must never panic identification");
        let unit = units
            .iter()
            .find(|u| u.path() == jsonl)
            .expect("the empty session is still identified as a unit");
        assert!(
            matches!(
                unit.project_link(),
                swamp_core::agents::ProjectLinkState::Unresolved { .. }
            ),
            "{:?}",
            unit.project_link()
        );
    }
    // Codex: transcript contents do not declare project linkage, even
    // when the file happens to contain a cwd-shaped JSON snippet.
    {
        let home = tempfile::tempdir().unwrap();
        let jsonl = home
            .path()
            .join("sessions/2026/09/21/rollout-not-json.jsonl");
        write(&jsonl, b"{\"cwd\":\"/a/guessed/project\"}\n");
        let result = std::panic::catch_unwind(|| {
            identify_with!(swamp_core::agents::codex::identify, home.path())
        });
        let units = result.expect("transcript content is never parsed for linkage");
        let unit = units.iter().find(|u| u.path() == jsonl);
        if let Some(unit) = unit {
            assert!(
                matches!(
                    unit.project_link(),
                    swamp_core::agents::ProjectLinkState::Unresolved { .. }
                ),
                "{:?}",
                unit.project_link()
            );
        }
    }
    // OpenCode: a `storage/project/*.json` file that is not valid JSON.
    {
        let home = tempfile::tempdir().unwrap();
        write(&home.path().join("storage/project/p1.json"), b"{not json");
        write(&home.path().join("storage/session/p1/s1.json"), b"{}");
        write(&home.path().join("auth.json"), b"[redacted]");
        let result = std::panic::catch_unwind(|| {
            identify_with!(swamp_core::agents::opencode::identify, home.path())
        });
        result.expect("a malformed project.json must never panic identification");
    }
}

// ---------------------------------------------------------------------
// Unknown schema never falls back to a destructive action
// ---------------------------------------------------------------------

#[test]
fn unrecognized_or_unknown_schema_units_are_never_actionable() {
    let cases: Vec<Vec<swamp_core::agents::CandidateAgentUnit>> = vec![
        {
            let home = tempfile::tempdir().unwrap();
            fs::create_dir_all(home.path()).unwrap();
            write(&home.path().join("unrelated.txt"), b"hello");
            identify_with!(swamp_core::agents::windsurf::identify, home.path())
        },
        {
            let home = tempfile::tempdir().unwrap();
            fs::create_dir_all(home.path()).unwrap();
            write(&home.path().join("unrelated.txt"), b"hello");
            identify_with!(swamp_core::agents::opencode::identify, home.path())
        },
        {
            let home = tempfile::tempdir().unwrap();
            fs::create_dir_all(home.path()).unwrap();
            write(&home.path().join("unrelated.txt"), b"hello");
            identify_with!(swamp_core::agents::oh_my_pi::identify, home.path())
        },
    ];
    for units in cases {
        assert!(
            !units.is_empty(),
            "an unknown residual must still be visible, never dropped"
        );
        for u in &units {
            assert_eq!(
                u.action(),
                AgentActionCapability::None,
                "an unrecognized-schema unit must never carry a destructive action capability: {u:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------
// Shared resources: reference coverage is explicit, never actionable
// ---------------------------------------------------------------------

#[test]
fn oh_my_pi_shared_blob_reference_states_are_explicit_and_never_actionable() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    write(&home.join("config.yml"), b"providers: {}");
    let repo_path = "/tmp/does-not-need-to-exist-for-this-check";
    let referenced_hash = "a".repeat(64);
    let unreferenced_hash = "b".repeat(64);
    write(&home.join("blobs").join(&referenced_hash), b"blob-bytes-1");
    write(
        &home.join("blobs").join(&unreferenced_hash),
        b"blob-bytes-2",
    );
    let session = home
        .join("sessions")
        .join("-repo")
        .join("1700000000_66666666-6666-4666-8666-666666666666.jsonl");
    let mut header = serde_json::json!({"cwd": repo_path}).to_string();
    header.push('\n');
    let mut bytes = vec![0u8; 256];
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(format!("blob:sha256:{referenced_hash}\n").as_bytes());
    write(&session, &bytes);

    let units = identify_with!(swamp_core::agents::oh_my_pi::identify, home);
    let referenced = units
        .iter()
        .find(|u| u.path().ends_with(&referenced_hash))
        .expect("referenced blob identified");
    let unreferenced = units
        .iter()
        .find(|u| u.path().ends_with(&unreferenced_hash))
        .expect("unreferenced blob identified");
    assert_eq!(referenced.action(), AgentActionCapability::None);
    assert_eq!(unreferenced.action(), AgentActionCapability::None);
    assert!(
        referenced
            .note()
            .unwrap_or_default()
            .contains("referenced by"),
        "{:?}",
        referenced.note()
    );
    assert!(
        unreferenced
            .note()
            .unwrap_or_default()
            .contains("no referencing session found"),
        "{:?}",
        unreferenced.note()
    );
}

// ---------------------------------------------------------------------
// Canary sweep: render text, JSON, plan, execute result, ledger --
// across two differently-shaped adapters (Claude Code and Codex).
// ---------------------------------------------------------------------

fn claude_code_fixture_with_canary(home_root: &Path) -> (PathBuf, PathBuf) {
    let claude_home = home_root.join("claude-home");
    let repo = home_root.join("repo-a");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let session_id = "77777777-7777-4777-8777-777777777777";
    let jsonl = claude_home
        .join("projects")
        .join("-repo-a-encoded")
        .join(format!("{session_id}.jsonl"));
    write(
        &jsonl,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"{CANARY}\"}}}}\n",
            repo.display()
        )
        .as_bytes(),
    );
    (claude_home, jsonl)
}

fn codex_fixture_with_canary(home_root: &Path) -> (PathBuf, PathBuf) {
    let codex_home = home_root.join("codex-home");
    let repo = home_root.join("repo-b");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let jsonl = codex_home.join(
        "sessions/2026/09/21/rollout-2026-09-21T10-00-00-88888888-8888-4888-8888-888888888888.jsonl",
    );
    write(
        &jsonl,
        format!(
            "{{\"type\":\"session_meta\",\"cwd\":\"{}\",\"payload\":{{\"prompt\":\"{CANARY}\"}}}}\n",
            repo.display()
        )
        .as_bytes(),
    );
    (codex_home, jsonl)
}

#[test]
fn canary_never_leaks_across_render_text_json_plan_execute_or_ledger() {
    let root = tempfile::tempdir().unwrap();
    let (claude_home, claude_jsonl) = claude_code_fixture_with_canary(root.path());
    let (codex_home, codex_jsonl) = codex_fixture_with_canary(root.path());

    let mut env_vars = HashMap::new();
    env_vars.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        claude_home.display().to_string(),
    );
    env_vars.insert("CODEX_HOME".to_string(), codex_home.display().to_string());
    let scope = scope_for(root.path(), env_vars, &["claude-code", "codex"]);
    let store = tempfile::tempdir().unwrap();
    let units = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    assert!(units.iter().any(|u| u.path == claude_jsonl));
    assert!(units.iter().any(|u| u.path == codex_jsonl));

    // 1. Rendered text (`swamp report --view agents`'s own renderer).
    let text = render_view_agents(&units, None, true, 2_000);
    assert!(
        !text.contains(CANARY),
        "rendered agents view leaked content:\n{text}"
    );

    // 2. JSON serialization of the units themselves (the CLI's own
    // `--view agents --json` payload shape).
    let json = serde_json::to_string(&units).unwrap();
    assert!(!json.contains(CANARY));

    // 3. Plan (propose) -- both sessions selected together.
    let plan =
        actions::propose_agents(&units, &[claude_jsonl.clone(), codex_jsonl.clone()], "test")
            .unwrap();
    let plan_json = serde_json::to_string(&plan).unwrap();
    assert!(!plan_json.contains(CANARY));

    // 4. Trash move (what the TUI's Enter does directly, no plan/grant
    // store any more): one move per unit, straight from the built plan.
    let trash = tempfile::tempdir().unwrap();
    let mut completed = 0;
    let ledger = swamp_core::ledger::Ledger::open(store.path().join("ledger.parquet")).unwrap();
    for unit in &plan {
        let meta = unit.agent_meta().unwrap();
        let at = swamp_core::entities::now();
        let (dest, bytes) = match &meta.session_members {
            Some(members) => {
                actions::trash_agent_session(meta, unit.path(), members, trash.path(), at).unwrap()
            }
            None => actions::trash_agent_cache(unit.path(), trash.path(), at).unwrap(),
        };
        ledger
            .append(&swamp_core::ledger::ActionRecord {
                id: swamp_core::entities::new_id(),
                verb: swamp_core::ledger::Verb::Delete,
                entity_id: swamp_core::entities::id_for(&unit.path().display().to_string()),
                evidence: vec![
                    swamp_core::ledger::LedgerFact::new("tool_id", &meta.tool_id),
                    swamp_core::ledger::LedgerFact::new("bytes", bytes),
                ],
                grant_id: swamp_core::ledger::NO_GRANT.to_string(),
                actor: "human:test".into(),
                outcome: "completed".into(),
                recovery_location: Some(dest),
                measured_free_space_delta: None,
                observed_path_state: Some("trashed".into()),
                recorded_at: at,
            })
            .unwrap();
        completed += 1;
    }
    assert_eq!(completed, 2);

    // 5. Ledger.
    let ledger_text = format!("{:?}", ledger.all().unwrap());
    assert!(!ledger_text.contains(CANARY));
}

// ---------------------------------------------------------------------
// Nested accounting: project-tree totals equal the flat Agents-view
// total for the same project -- no double counting.
// ---------------------------------------------------------------------

#[test]
fn project_linked_totals_agree_between_the_flat_view_and_the_project_tree() {
    let root = tempfile::tempdir().unwrap();
    let (claude_home, jsonl) = claude_code_fixture_with_canary(root.path());
    let extra_jsonl = claude_home
        .join("projects")
        .join("-repo-a-encoded")
        .join("99999999-9999-4999-8999-999999999999.jsonl");
    write(
        &extra_jsonl,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"s2\",\"cwd\":\"{}\",\"gitBranch\":\"main\"}}\n",
            root.path().join("repo-a").display()
        )
        .as_bytes(),
    );

    let mut env_vars = HashMap::new();
    env_vars.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        claude_home.display().to_string(),
    );
    let scope = scope_for(root.path(), env_vars, &["claude-code"]);
    let store = tempfile::tempdir().unwrap();
    let units = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        false,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    assert!(units.iter().any(|u| u.path == jsonl));
    assert!(units.iter().any(|u| u.path == extra_jsonl));

    let project = swamp_core::report::ProjectRow {
        project_id: "p-repo-a".into(),
        name: "repo-a".into(),
        remote: None,
        ecosystems: Vec::new(),
        worktrees: Vec::new(),
    };
    let tree_rows = swamp_core::tree::agent_rows_for_project(&project, &units);
    let tree_total: u64 = tree_rows.iter().map(|r| r.bytes).sum();

    let flat_total: u64 = units
        .iter()
        .filter(|u| {
            matches!(
                &u.project_link,
                swamp_core::agents::ProjectLinkState::Linked { project_name, .. }
                    if project_name.eq_ignore_ascii_case("repo-a")
            )
        })
        .map(|u| u.bytes)
        .sum();

    assert!(
        flat_total > 0,
        "the fixture must actually produce linked bytes"
    );
    assert_eq!(
        tree_total, flat_total,
        "the project tree's collapsed agent-storage total must equal the flat view's total for \
         the same project, with no double counting"
    );
}

// ---------------------------------------------------------------------
// Incremental history: unchanged refresh reports no growth; adding one
// session reports exactly its own bytes, without disturbing the
// unchanged session's own history (nested accounting across units).
// ---------------------------------------------------------------------

#[test]
fn unchanged_refresh_is_silent_then_a_new_session_reports_only_its_own_growth() {
    let root = tempfile::tempdir().unwrap();
    let (claude_home, jsonl) = claude_code_fixture_with_canary(root.path());
    let mut env_vars = HashMap::new();
    env_vars.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        claude_home.display().to_string(),
    );
    let scope = scope_for(root.path(), env_vars.clone(), &["claude-code"]);
    let store = tempfile::tempdir().unwrap();

    // First observation: no baseline yet.
    let first = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let first_unit = first.iter().find(|u| u.path == jsonl).unwrap();
    assert_eq!(
        first_unit.growth_bytes, None,
        "{:?}",
        first_unit.growth_bytes
    );
    let original_bytes = first_unit.bytes;

    // Second observation, nothing changed: zero growth, never a
    // fabricated delta from re-observing the exact same bytes.
    let second = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        2_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let second_unit = second.iter().find(|u| u.path == jsonl).unwrap();
    assert_eq!(second_unit.bytes, original_bytes);
    assert!(
        second_unit.growth_bytes.is_none_or(|g| g == 0),
        "unchanged content must report no growth: {:?}",
        second_unit.growth_bytes
    );

    // Third observation: a brand-new session appears. It reports growth
    // (first sighting for its own row is `None`, same as any new row);
    // the *existing*, unchanged session must still show zero growth --
    // one unit's new history must never bleed into another's.
    let new_session = claude_home
        .join("projects")
        .join("-repo-a-encoded")
        .join("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.jsonl");
    write(
        &new_session,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"s3\",\"cwd\":\"{}\",\"gitBranch\":\"main\"}}\n",
            root.path().join("repo-a").display()
        )
        .as_bytes(),
    );
    let third = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        3_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let third_existing = third.iter().find(|u| u.path == jsonl).unwrap();
    assert!(
        third_existing.growth_bytes.is_none_or(|g| g == 0),
        "the pre-existing session must still show no growth after an unrelated new session \
         appeared: {:?}",
        third_existing.growth_bytes
    );
    let third_new = third.iter().find(|u| u.path == new_session).unwrap();
    assert_eq!(
        third_new.growth_bytes, None,
        "a session observed for the first time reports no growth yet (no baseline), same as \
         `first_unit` above -- never a fabricated delta equal to its own full size"
    );
}

// ---------------------------------------------------------------------
// Stable history after attribution changes: relinking a session to a
// different project must not fabricate a byte-history delta.
// ---------------------------------------------------------------------

#[test]
fn relinking_a_session_to_a_different_project_leaves_bytes_and_growth_history_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let claude_home = root.path().join("claude-home");
    let repo_a = root.path().join("proj-aaaa");
    let repo_b = root.path().join("proj-bbbb");
    fs::create_dir_all(repo_a.join(".git")).unwrap();
    fs::create_dir_all(repo_b.join(".git")).unwrap();
    assert_eq!(
        repo_a.display().to_string().len(),
        repo_b.display().to_string().len(),
        "both project paths must be the same length so relinking changes only which project a \
         session is attributed to, never the transcript's own byte count"
    );
    let session_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let jsonl = claude_home
        .join("projects")
        .join("-proj-aaaa-encoded")
        .join(format!("{session_id}.jsonl"));
    write(
        &jsonl,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\"}}\n",
            repo_a.display()
        )
        .as_bytes(),
    );

    let mut env_vars = HashMap::new();
    env_vars.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        claude_home.display().to_string(),
    );
    let scope = scope_for(root.path(), env_vars.clone(), &["claude-code"]);
    let store = tempfile::tempdir().unwrap();

    let before = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let before_unit = before.iter().find(|u| u.path == jsonl).unwrap();
    let bytes_before = before_unit.bytes;
    assert!(matches!(
        &before_unit.project_link,
        swamp_core::agents::ProjectLinkState::Linked { project_name, .. } if project_name == "proj-aaaa"
    ));

    // Attribution change only: rewrite the declared `cwd` to point at a
    // different, same-length project path. The file's own byte count is
    // unchanged (same JSON structure, same-length replacement).
    write(
        &jsonl,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\"}}\n",
            repo_b.display()
        )
        .as_bytes(),
    );

    // What a *fresh* identification sees. This is the path every
    // execution sink takes (`reidentify_for_tool` runs with both caches
    // disabled), and it is the boundary that matters: an approval is
    // never spent against a cached derivation.
    let rechecked = swamp_core::agents::reidentify_for_tool("claude-code", &claude_home, 2_000)
        .expect("the claude-code adapter must be registered");
    let rechecked_unit = rechecked.iter().find(|u| u.path() == jsonl).unwrap();
    assert!(
        matches!(
            &rechecked_unit.project_link(),
            swamp_core::agents::ProjectLinkState::Linked { project_name, .. } if project_name == "proj-bbbb"
        ),
        "a fresh re-identification must see the moved attribution: {:?}",
        rechecked_unit.project_link()
    );

    // The ordinary report path, under the event coverage an ordinary
    // pass has: the replay names the rewritten transcript and its
    // parent, because a rewrite *is* an event.
    //
    // This assertion was weakened in stack/12 to "a replayed container
    // still reports the declared path it was stored with", because
    // reuse was then keyed on the container's directory stamp, which an
    // in-place rewrite does not move. Gating reuse on trusted event
    // coverage (stack/13) removed the reason for the weakening, and the
    // original requirement is re-asserted here: the next observation
    // sees the new project.
    let events = swamp_core::fs_events::EventCoverage::trusted(
        root.path().to_path_buf(),
        vec![
            jsonl.clone(),
            claude_home.join("projects").join("-proj-aaaa-encoded"),
        ],
        1_000,
    );
    let after = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        2_000,
        30,
        3600,
        &events,
    )
    .unwrap();
    let after_unit = after.iter().find(|u| u.path == jsonl).unwrap();
    assert!(
        matches!(
            &after_unit.project_link,
            swamp_core::agents::ProjectLinkState::Linked { project_name, .. } if project_name == "proj-bbbb"
        ),
        "the next ordinary observation under event coverage must see the new project: {:?}",
        after_unit.project_link
    );
    assert_eq!(
        after_unit.bytes, bytes_before,
        "byte count must be identical -- only the declared cwd text changed, at the same length"
    );
    assert!(
        after_unit.growth_bytes.is_none_or(|g| g == 0),
        "a pure attribution change (same bytes) must never fabricate a growth delta: {:?}",
        after_unit.growth_bytes
    );

    // And a pass with *no* window reuses nothing, so it sees the same
    // thing for the same reason a fresh identification does. Two
    // branches, one answer: there is no pass that reports the stale
    // project.
    let store2 = tempfile::tempdir().unwrap();
    let cold = discover_and_measure(
        &scope,
        &[],
        Some(store2.path()),
        true,
        2_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let cold_unit = cold.iter().find(|u| u.path == jsonl).unwrap();
    assert!(
        matches!(
            &cold_unit.project_link,
            swamp_core::agents::ProjectLinkState::Linked { project_name, .. } if project_name == "proj-bbbb"
        ),
        "a pass with no trusted window must re-identify and see the new project: {:?}",
        cold_unit.project_link
    );

    // A later, genuinely quiet pass replays the container -- and what it
    // replays is the *new* attribution, because the pass above stored
    // it. A replay can only ever be as stale as the observation that
    // wrote it, which is the whole point of gating on the window.
    let later = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        3_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::trusted(
            root.path().to_path_buf(),
            Vec::new(),
            2_000,
        ),
    )
    .unwrap();
    let later_unit = later.iter().find(|u| u.path == jsonl).unwrap();
    assert!(
        matches!(
            &later_unit.project_link,
            swamp_core::agents::ProjectLinkState::Linked { project_name, .. } if project_name == "proj-bbbb"
        ),
        "a replayed container must carry the attribution the last identification stored: {:?}",
        later_unit.project_link
    );
    assert_eq!(
        later_unit.bytes, bytes_before,
        "byte count must still be identical -- only the declared cwd text changed"
    );
    assert!(
        later_unit.growth_bytes.is_none_or(|g| g == 0),
        "a pure attribution change must never fabricate a growth delta, not on any pass: {:?}",
        later_unit.growth_bytes
    );
}

// ---------------------------------------------------------------------
// Benchmarks (recorded in .oh/sessions/2026-09-21-agent-storage-
// validation.md): unchanged refresh, one-session append, and blob/store
// growth all complete without reading full transcript bodies. Each
// session's fixture body is padded well past any bounded-read window,
// so a measured sub-second completion is itself evidence that only
// directory names + bounded metadata were read, not full content.
// ---------------------------------------------------------------------

#[test]
fn benchmark_unchanged_refresh_one_session_append_and_blob_growth() {
    let root = tempfile::tempdir().unwrap();
    let claude_home = root.path().join("claude-home");
    let repo = root.path().join("bench-repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let padding = "x".repeat(200_000);
    for i in 0..300 {
        let id = format!("{i:08x}-0000-4000-8000-000000000000");
        let jsonl = claude_home
            .join("projects")
            .join("-bench-repo-encoded")
            .join(format!("{id}.jsonl"));
        write(
            &jsonl,
            format!(
                "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"padding\":\"{padding}\"}}\n",
                repo.display()
            )
            .as_bytes(),
        );
    }

    let mut env_vars = HashMap::new();
    env_vars.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        claude_home.display().to_string(),
    );
    let scope = scope_for(root.path(), env_vars, &["claude-code"]);
    let store = tempfile::tempdir().unwrap();

    let t0 = std::time::Instant::now();
    let first = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let initial_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(first.len(), 300);

    let t1 = std::time::Instant::now();
    let _unchanged = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        2_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let unchanged_ms = t1.elapsed().as_secs_f64() * 1000.0;

    let extra = claude_home
        .join("projects")
        .join("-bench-repo-encoded")
        .join("ffffffff-0000-4000-8000-000000000000.jsonl");
    write(
        &extra,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"padding\":\"{padding}\"}}\n",
            repo.display()
        )
        .as_bytes(),
    );
    let t2 = std::time::Instant::now();
    let appended = discover_and_measure(
        &scope,
        &[],
        Some(store.path()),
        true,
        3_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let append_ms = t2.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(appended.len(), 301);

    eprintln!(
        "[measured] agent_storage_validation benchmark: initial 300-session scan {initial_ms:.1}ms, \
         unchanged refresh {unchanged_ms:.1}ms, one-session append {append_ms:.1}ms"
    );
    // Loose, CI-headroom bound (the actual observed numbers are recorded
    // in `.oh/sessions/2026-09-21-agent-storage-validation.md`, not this
    // threshold): each pass reads directory names + one bounded header
    // line per session, never the ~200KB padding body, so 300 sessions
    // complete in well under a second on ordinary developer hardware.
    assert!(
        initial_ms < 10_000.0 && unchanged_ms < 10_000.0 && append_ms < 10_000.0,
        "initial={initial_ms}ms unchanged={unchanged_ms}ms append={append_ms}ms"
    );
}
