//! R16/R18a-2 (tables 1-3 of this slice's part of the JSON-in-the-store
//! decomposition): `external_units.parquet`/`agent_units.parquet` (+
//! `unit_consumers.parquet`) and `nested_artifacts.parquet` (+
//! `nested_artifact_lists.parquet`/`nested_artifact_evidence.parquet`)
//! are what `swamp report` builds `ReportSnapshot.external_units`/
//! `.agent_units` and `Report.nested_artifacts`/`ReportSnapshot.
//! store_interiors` from. There is no JSON cell for any of these left
//! to fall back to at all -- R18a-3b deleted the last one
//! (`report_json`/`StoredReportSnapshotRow`/`report_rows.parquet`)
//! entirely.
//! (`evidence.parquet`'s exhaustive per-variant round trip is
//! `growth::tests::evidence_table_round_trips_every_status_and_source_
//! variant`; this file's job is proving the *wiring* -- `observe_scope`
//! writes these tables from a real discovery pass, and
//! `report_scope_from_store` rebuilds from them -- not re-proving every
//! `FactValue`/`EvidenceSource` variant a second time.)
//!
//! Adversarial claims, not a happy path:
//!
//! 1. After `observe_scope`, all four files exist under the store.
//! 2. `report_scope_from_store`'s `external_units`/`agent_units`/
//!    `nested_artifacts` equal the observed ones field for field, even
//!    after the snapshot's own remaining `report_json` copy is
//!    tampered with. The tempting shortcut this fails is "keep a JSON
//!    fallback around and merely also write the tables".
//!
//! Disposable `tempfile` fixtures only; the fixture Claude Code home is
//! synthetic (PRIVACY IS A HARD RULE) -- adapted from
//! `crates/cli/tests/agent_storage_cli.rs`'s `fixture()`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::{self, ObservationParts};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};

fn run_git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
    assert!(out.status.success(), "git {args:?} failed");
}

fn write(path: &Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// A repo-linked Claude Code session (for `agent_units.parquet`, project
/// linkage) plus a fixture Cargo home with real cache bytes (for
/// `external_units.parquet`). Returns the repo's path.
fn make_fixture(root: &Path, claude_home: &Path, cargo_home: &Path) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("Cargo.toml"), b"[package]\nname=\"a\"\n").unwrap();
    run_git(&repo, &["add", "Cargo.toml"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);

    let session_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    let session_line = format!(
        "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\"}}\n",
        repo.display()
    );
    write(
        &claude_home
            .join("projects")
            .join("-repo-encoded")
            .join(format!("{session_id}.jsonl")),
        session_line.as_bytes(),
    );
    write(&claude_home.join("settings.json"), b"{}");

    write(
        &cargo_home.join("registry/cache/index.crates.io-x/a-1.0.0.crate"),
        &vec![b'c'; 8192],
    );

    repo
}

struct Fx {
    _tmp: tempfile::TempDir,
    store: PathBuf,
    scope: EffectiveScope,
    repo: PathBuf,
}

fn observe(fx: &Fx) -> report::ScopeObservation {
    report::observe_scope(
        &fx.scope,
        ObservationParts::ALL,
        None,
        None,
        false,
        Some(&fx.store),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .expect("observe_scope")
}

fn build() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = swamp_core::fs_gate::canonicalize(tmp.path()).unwrap_or(tmp.path().to_path_buf());
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let claude_home = root.join("claude-home");
    let cargo_home = root.join("fixture-cargo");
    let repo = make_fixture(&src, &claude_home, &cargo_home);

    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();

    let mut env_vars = HashMap::new();
    env_vars.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        claude_home.display().to_string(),
    );
    env_vars.insert("CARGO_HOME".to_string(), cargo_home.display().to_string());
    let env = Environment::fixture(root.clone(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: vec![src.display().to_string()],
        enabled_detectors: vec!["cargo-home".to_string(), "claude-code".to_string()],
        ..Default::default()
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);
    Fx {
        _tmp: tmp,
        store,
        scope,
        repo,
    }
}

#[test]
fn observe_writes_the_four_tables_and_report_rebuilds_units_and_nested_artifacts_from_them() {
    let fx = build();
    let observation = observe(&fx);

    assert!(
        !observation.external_units.is_empty(),
        "the fixture Cargo home must produce at least one external unit: {:?}",
        observation.external_units
    );
    assert!(
        !observation.agent_units.is_empty(),
        "the fixture Claude Code session must produce at least one agent unit: {:?}",
        observation.agent_units
    );
    let linked_agent = observation
        .agent_units
        .iter()
        .find(|u| {
            matches!(
                u.project_link,
                swamp_core::agents::ProjectLinkState::Linked { .. }
            )
        })
        .expect("the session must link to the fixture repo");
    if let swamp_core::agents::ProjectLinkState::Linked { project_path, .. } =
        &linked_agent.project_link
    {
        assert_eq!(project_path, &fx.repo);
    }

    for table in [
        "external_units.parquet",
        "agent_units.parquet",
        "unit_consumers.parquet",
        "nested_artifacts.parquet",
    ] {
        assert!(
            fx.store.join(table).is_file(),
            "observe_scope must write {table} at the top of the store"
        );
    }

    let snapshot = report::report_scope_from_store(&fx.scope, &fx.store).expect("stored");
    assert_eq!(
        serde_json::to_value(&observation.external_units).unwrap(),
        serde_json::to_value(&snapshot.external_units).unwrap(),
        "external_units must rebuild field for field from the tables"
    );
    assert_eq!(
        serde_json::to_value(&observation.agent_units).unwrap(),
        serde_json::to_value(&snapshot.agent_units).unwrap(),
        "agent_units must rebuild field for field from the tables, project_link included"
    );
    assert_eq!(
        serde_json::to_value(&observation.merged.nested_artifacts).unwrap(),
        serde_json::to_value(&snapshot.report.nested_artifacts).unwrap(),
        "Report.nested_artifacts must rebuild field for field from the table"
    );
    assert_eq!(
        serde_json::to_value(&observation.store_interiors).unwrap(),
        serde_json::to_value(&snapshot.store_interiors).unwrap(),
        "store_interiors must rebuild field for field from the table"
    );
}

/// A direct on-disk tamper of `external_units.parquet`/
/// `agent_units.parquet` (via the public `write_unit_tables` writer --
/// there is no JSON snapshot left to tamper) is what the next
/// `report_scope_from_store` reports: proof this reads the tables live,
/// never a cached copy of the observation that produced them.
/// (`nested_artifacts.parquet`'s own field-exhaustive round-trip/tamper
/// coverage is `growth::tests::nested_artifact_table_round_trips_every_
/// field_and_splits_by_origin`/`..._tables_rebuild_reflects_a_direct_
/// tamper_not_the_original_value` -- this fixture's Cargo home cache
/// file does not itself produce a nested artifact, so it cannot prove
/// that table's wiring the same way it proves external/agent units'.)
#[test]
fn report_reads_units_from_a_direct_tamper_of_the_tables() {
    let fx = build();
    let observation = observe(&fx);
    assert!(!observation.external_units.is_empty());
    assert!(!observation.agent_units.is_empty());

    let key = report::scope_snapshot_key(&fx.scope);

    let mut tampered_external = observation.external_units.clone();
    for u in &mut tampered_external {
        u.bytes += 1;
        u.detector_name.push_str("-TAMPERED");
        u.mtime_max += 1;
        for c in &mut u.consumers {
            c.label.push_str("-TAMPERED");
        }
    }
    let mut tampered_agent = observation.agent_units.clone();
    for u in &mut tampered_agent {
        u.bytes += 1;
        u.tool_name.push_str("-TAMPERED");
        u.protected = !u.protected;
    }
    assert_ne!(
        serde_json::to_value(&tampered_external).unwrap(),
        serde_json::to_value(&observation.external_units).unwrap(),
        "the tamper must actually change external_units"
    );
    assert_ne!(
        serde_json::to_value(&tampered_agent).unwrap(),
        serde_json::to_value(&observation.agent_units).unwrap(),
        "the tamper must actually change agent_units"
    );

    swamp_core::growth::write_unit_tables(&fx.store, &key, &tampered_external, &tampered_agent)
        .unwrap();

    let rebuilt = report::report_scope_from_store(&fx.scope, &fx.store).expect("stored");
    assert_eq!(
        serde_json::to_value(&rebuilt.external_units).unwrap(),
        serde_json::to_value(&tampered_external).unwrap(),
        "external_units must come from the current external_units.parquet/unit_consumers.parquet"
    );
    assert_eq!(
        serde_json::to_value(&rebuilt.agent_units).unwrap(),
        serde_json::to_value(&tampered_agent).unwrap(),
        "agent_units must come from the current agent_units.parquet"
    );
}
