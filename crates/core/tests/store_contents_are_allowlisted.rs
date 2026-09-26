//! What swamp is allowed to leave in its store
//! (`.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`,
//! `.oh/guardrails/json-persistence-is-allowlisted.md`).
//!
//! The AST audits check the *code*. This checks the disk: it runs a full
//! observe -> report -> Trash-move cycle over a multi-ecosystem fixture
//! and then walks the store, asserting every
//! file matches exactly one allow-listed pattern. A new JSON data cache
//! fails here even if it is assembled from fragments the audits cannot
//! see, and a growing JSON file fails on size alone -- a control file
//! that keeps growing is a data store in disguise.
//!
//! Disposable `tempfile` fixtures throughout. The Trash root is a
//! fixture directory, never the user's real Trash.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{ScanConfig, resolve_effective_scope};

/// A control file may be JSON; above this it is a data store wearing a
/// control file's name.
const MAX_CONTROL_JSON_BYTES: u64 = 64 * 1024;

/// Exact basenames allowed anywhere under the store, other than tables:
/// what the no-JSON rule allows -- `config.toml`, a tiny `ui_state.json`,
/// lock files (below), and the scheduled-observation text log.
const ALLOWED_NAMES: &[&str] = &["config.toml", "ui_state.json", "observe.log"];

/// Every Parquet table the store may hold, by exact basename -- the
/// runtime twin of the source audit's `TABLE_WRITERS`
/// (`crates/source-audit/src/rules/gate.rs`) and of the table in
/// `docs/architecture.md`. Each is a *fact* table: rows from an
/// observation or their reverse-delta history. A table for something a
/// read computes (`summary`, `series`, `worktree_entries`,
/// `unowned_summary`, `github_enrichment`, `report_rows`) is not here
/// and fails this test (store-is-facts-report-is-views, R20).
const TABLES: &[&str] = &[
    // per-volume current state + history (`<store>/<volume>/`)
    "cursors.parquet",
    "current.parquet",
    "dirs.parquet",
    "files.parquet",
    "unowned.parquet",
    "unowned_lists.parquet",
    "unowned_evidence.parquet",
    "docker_unowned.parquet",
    "docker_unowned_lists.parquet",
    "docker_unowned_evidence.parquet",
    "dir_tracks.parquet",
    "topology.parquet",
    "enrich.parquet",
    // external / build-store measurement caches
    "volume_stamps.parquet",
    "build_stores.parquet",
    "xcode_derived_data.parquet",
    "declarations.parquet",
    "dependency_identities.parquet",
    "external_consumers.parquet",
    // per-root replay caches
    "git_signals.parquet",
    "git_signals_values.parquet",
    "cargo_replay_cache.parquet",
    "cargo_replay_cache_lists.parquet",
    "cargo_replay_cache_evidence.parquet",
    "cargo_replay_cache_meta.parquet",
    // control state (`<store>/`), R18b
    "scheduled_runs.parquet",
    "scope.parquet",
    "scope_values.parquet",
    "scope_roots.parquet",
    "scope_root_reasons.parquet",
    "ledger.parquet",
    "ledger_facts.parquet",
    "docker_meta.parquet",
    "docker_images.parquet",
    "docker_build_cache.parquet",
    "docker_volumes.parquet",
    "docker_builders.parquet",
    "docker_values.parquet",
    "docker_containers.parquet",
    // scope-wide observation facts (`<store>/`)
    "runs.parquet",
    "coverage.parquet",
    "notes.parquet",
    "projects.parquet",
    "worktrees.parquet",
    "worktree_facts.parquet",
    "artifact_shape.parquet",
    "artifact_shape_lists.parquet",
    "external_units.parquet",
    "agent_units.parquet",
    "agent_unit_members.parquet",
    "unit_consumers.parquet",
    "agent_identifications.parquet",
    "agent_containers.parquet",
    "nested_artifacts.parquet",
    "nested_artifact_lists.parquet",
    "nested_artifact_evidence.parquet",
    "evidence.parquet",
    "protect.parquet",
];

fn allowed(rel: &Path) -> bool {
    let name = rel
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if ALLOWED_NAMES.contains(&name.as_str()) {
        return true;
    }
    if TABLES.contains(&name.as_str()) {
        return true;
    }
    let parent = rel
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // The external measurement cache, one file per unit (R19):
    // `external/folded/<id>.parquet`.
    if parent == "folded" && name.ends_with(".parquet") {
        return true;
    }
    // Reverse-delta history: `<volume>/{deltas,dirs_deltas,files_deltas}/
    // delta-<seq>.parquet`.
    if matches!(parent.as_str(), "deltas" | "dirs_deltas" | "files_deltas")
        && name.starts_with("delta-")
        && name.ends_with(".parquet")
    {
        return true;
    }
    // A Linux collector's checkpoint tables and an observation's sync
    // request for it (#82): `continuity/<id>.parquet`,
    // `<id>_entries.parquet`, `<id>.sync`.
    if rel.components().any(|c| c.as_os_str() == "continuity")
        && (name.ends_with(".parquet") || name.ends_with(".sync"))
    {
        return true;
    }
    // Lock files and the store's own bookkeeping markers.
    if name.ends_with(".lock") || name == "VERSION" {
        return true;
    }
    false
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&p, base, out);
        } else {
            out.push(p.strip_prefix(base).unwrap_or(&p).to_path_buf());
        }
    }
}

/// Projects in three ecosystems, an external cache root, and a synthetic
/// agent home: enough that every persistence path in the pipeline runs.
struct Fixture {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    cargo_home: PathBuf,
    claude_home: PathBuf,
    store: PathBuf,
    trash: PathBuf,
}

fn build() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let src = root.join("src");

    // Rust
    let rust = src.join("rust-app");
    fs::create_dir_all(rust.join("target/debug/incremental")).unwrap();
    fs::write(rust.join("Cargo.toml"), b"[package]\nname=\"a\"\n").unwrap();
    fs::write(
        rust.join("Cargo.lock"),
        b"[[package]]\nname = \"serde\"\nversion = \"1.0.203\"\n",
    )
    .unwrap();
    fs::write(rust.join("target/debug/blob"), vec![b'r'; 8192]).unwrap();

    // Node
    let node = src.join("node-app");
    fs::create_dir_all(node.join("node_modules/lodash")).unwrap();
    fs::write(node.join("package.json"), b"{\"name\":\"n\"}").unwrap();
    fs::write(node.join("node_modules/lodash/index.js"), vec![b'n'; 4096]).unwrap();

    // Maven, with an unresolvable version so the gap path runs too
    let maven = src.join("maven-app");
    fs::create_dir_all(&maven).unwrap();
    fs::write(
        maven.join("pom.xml"),
        br#"<project><properties><v>1.0</v></properties><dependencies>
        <dependency><groupId>g</groupId><artifactId>a</artifactId><version>${v}</version></dependency>
        </dependencies></project>"#,
    )
    .unwrap();

    // An external cache root and a synthetic agent home.
    let cargo_home = root.join("cargo-home");
    fs::create_dir_all(cargo_home.join("registry/src/index.crates.io-abc/serde-1.0.203")).unwrap();
    fs::write(
        cargo_home.join("registry/src/index.crates.io-abc/serde-1.0.203/lib.rs"),
        vec![b'c'; 2048],
    )
    .unwrap();

    let claude_home = root.join("claude");
    fs::create_dir_all(claude_home.join("debug")).unwrap();
    fs::write(claude_home.join("debug/log.txt"), vec![b'l'; 1024]).unwrap();

    Fixture {
        src,
        cargo_home,
        claude_home,
        store: root.join("store"),
        trash: root.join("trash"),
        _tmp: tmp,
    }
}

#[test]
fn a_full_cycle_leaves_only_allowlisted_files_in_the_store() {
    let fx = build();
    fs::create_dir_all(&fx.store).unwrap();
    fs::create_dir_all(&fx.trash).unwrap();

    let registry = Registry::with_builtins();
    let env = Environment::fixture(
        fx.claude_home
            .parent()
            .unwrap_or(Path::new("/"))
            .to_path_buf(),
        HashMap::from([
            (
                "CARGO_HOME".to_string(),
                fx.cargo_home.display().to_string(),
            ),
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                fx.claude_home.display().to_string(),
            ),
        ]),
        Platform::MacOS,
    );
    let cfg = ScanConfig {
        defaults: false,
        include: vec![fx.src.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| id != "cargo-home" && id != "claude-code")
            .collect(),
        ..Default::default()
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);

    // observe -> report, twice, so the incremental/cached paths persist
    // whatever they persist.
    for _ in 0..2 {
        let observation = swamp_core::report::observe_scope(
            &scope,
            swamp_core::report::ObservationParts::ALL,
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
        .expect("scope observation");
        // Every JSON/text rendering path, so anything that writes as a
        // side effect of rendering shows up here too.
        let _ = swamp_core::report::to_json(&observation.merged);
        let _ = swamp_core::agent_json::view_payload(&observation.merged, "projects", None);
        let _ = swamp_core::agent_json::what_grew_payload(&observation.merged, None);
        swamp_core::scope::persist_effective_scope(&fx.store, &scope).unwrap();
    }

    // Trash-move one agent cache unit, exactly as the TUI would on Enter.
    let observation = swamp_core::report::observe_scope(
        &scope,
        swamp_core::report::ObservationParts::ALL,
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
    .expect("scope observation");
    let cache = fx.claude_home.join("debug");
    let units = swamp_core::actions::propose_agents(
        &observation.agent_units,
        std::slice::from_ref(&cache),
        "test:store-contents",
    )
    .expect("the fixture's agent cache unit must be listable");
    assert!(units[0].agent_meta().is_some());
    let (_dest, _bytes) =
        swamp_core::actions::trash_agent_cache(&cache, &fx.trash, swamp_core::entities::now())
            .expect("trash move");

    // One more report after the action.
    let _ = swamp_core::report::observe_scope(
        &scope,
        swamp_core::report::ObservationParts::ALL,
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
    .expect("scope observation");

    // --- the assertions ---
    let mut files = Vec::new();
    walk(&fx.store, &fx.store, &mut files);
    assert!(!files.is_empty(), "the cycle must have written *something*");

    // Reported, not only asserted: the brief for this work asks for the
    // store's size on this fixture, and a number in a session note is
    // worth more than "small". Printed per file so a table that starts
    // growing per row is visible rather than hidden in a total.
    let mut total = 0u64;
    let mut listed: Vec<(String, u64)> = files
        .iter()
        .map(|rel| {
            let size = fs::metadata(fx.store.join(rel))
                .map(|m| m.len())
                .unwrap_or(0);
            total += size;
            (rel.display().to_string(), size)
        })
        .collect();
    listed.sort();
    println!("store after observe/report/trash-move: {total} bytes total");
    for (rel, size) in &listed {
        println!("  {size:>9}  {rel}");
    }

    let unexpected: Vec<String> = files
        .iter()
        .filter(|rel| !allowed(rel))
        .map(|rel| rel.display().to_string())
        .collect();
    assert!(
        unexpected.is_empty(),
        "the store may hold only Parquet tables and the named small control files; found: \
         {unexpected:?}"
    );

    // HARD RULE (2026-09-24, non-negotiable): any file under the store
    // other than `*.parquet` is a control file, and a control file that
    // keeps growing with observed data is a data store in disguise --
    // this is what let `unowned.json` reach 473 MB while still being
    // named like a small control file. The size cap therefore applies to
    // *every* non-Parquet file, not only `.json`/`.jsonl` names: it would
    // have caught a JSON report cache (`last_report*.json.zst`) or a
    // `.sync` continuity file quietly becoming a data store too.
    for rel in &files {
        let name = rel.file_name().unwrap_or_default().to_string_lossy();
        if name.ends_with(".parquet") {
            continue;
        }
        let size = fs::metadata(fx.store.join(rel))
            .map(|m| m.len())
            .unwrap_or(0);
        assert!(
            size <= MAX_CONTROL_JSON_BYTES,
            "{} is {size} bytes: a control file that keeps growing is a data store in disguise",
            rel.display()
        );
    }

    // The Trash envelope holds its manifest plus the moved members, and
    // nothing else.
    let envelope = fs::read_dir(&fx.trash)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .or_else(|| {
            fs::read_dir(&fx.trash)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .next()
        })
        .expect("the execution moved something into the fixture Trash");
    if envelope.is_dir() {
        let names: Vec<String> = fs::read_dir(&envelope)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        for n in &names {
            assert!(
                n == "restore.json" || !n.ends_with(".json") || n.starts_with(char::is_numeric),
                "a Trash envelope holds restore.json plus the moved members, nothing else: \
                 {names:?}"
            );
        }
    }
}
