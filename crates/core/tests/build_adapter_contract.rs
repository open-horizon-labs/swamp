//! The shared contract every build adapter obeys, asserted once over the
//! whole registry rather than once per adapter.
//!
//! The per-adapter tests (`build_adapter_test_contract`) prove each
//! adapter's own five properties. These prove the properties that are
//! only visible across adapters: that the registry, the matrix and the
//! docs agree; that no adapter can quietly ship a unit claiming an
//! action; and that the aggregation over a mixed-ecosystem report does
//! not double-count.

use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::artifact::{
    AccountingBasis, ArtifactRole, NestedActionCapability, RoleFamily, TimeSource,
};
use swamp_core::build_adapters::{
    BuildCtx, ContainerCache, FoldedDir, FoldedIndex, identify_all, matrix, registry::Registry,
    summarize_families,
};
use swamp_core::fs_events::EventCoverage;

fn index(entries: &[(PathBuf, u64, u64)]) -> FoldedIndex {
    FoldedIndex::from_dirs(entries.iter().map(|(p, b, m)| FoldedDir {
        path: p.clone(),
        allocated_total: *b,
        mtime_max: *m,
        complete: true,
    }))
}

/// A checkout that is a Rust crate, a Node package, a Gradle build and a
/// Maven module all at once -- which is not exotic (a Tauri app is Rust
/// and Node; an Android app is Gradle and Node) and is exactly where a
/// central `match ecosystem` used to have to pick one.
fn mixed_project(root: &Path) -> Vec<(PathBuf, u64, u64)> {
    fs::create_dir_all(root).unwrap();
    for (name, body) in [
        ("Cargo.toml", "[package]\nname='x'\n"),
        ("package.json", "{\"name\":\"x\"}"),
        ("build.gradle", ""),
        ("pom.xml", "<project/>"),
    ] {
        fs::write(root.join(name), body).unwrap();
    }
    let mut dirs = Vec::new();
    for (rel, bytes) in [
        ("target", 4_000u64),
        ("target/debug", 4_000),
        ("target/debug/deps", 3_000),
        ("node_modules", 8_000),
        ("node_modules/left-pad", 200),
        ("dist", 900),
        ("coverage", 300),
        (".turbo", 150),
        ("build", 2_000),
        ("build/classes", 1_500),
        (".gradle", 400),
    ] {
        let p = root.join(rel);
        fs::create_dir_all(&p).unwrap();
        dirs.push((p, bytes, 1_000_000u64));
    }
    fs::write(
        root.join("node_modules/left-pad/package.json"),
        br#"{"name":"left-pad","version":"1.3.0"}"#,
    )
    .unwrap();
    dirs
}

fn identify(
    root: &Path,
    dirs: &[(PathBuf, u64, u64)],
) -> Vec<swamp_core::artifact::NestedArtifact> {
    let idx = index(dirs);
    let none = EventCoverage::untrusted();
    let cache = ContainerCache::disabled();
    let ctx = BuildCtx::new(2_000_000, &idx, &none, &cache);
    let candidates: Vec<PathBuf> = dirs.iter().map(|(p, _, _)| p.clone()).collect();
    identify_all(
        &Registry::with_builtins(),
        &[(root.to_path_buf(), candidates)],
        &[],
        &ctx,
    )
}

fn blank_report(root: &Path) -> swamp_core::Report {
    swamp_core::Report {
        store_dir: None,
        observed_at: 2_000_000,
        root: root.to_path_buf(),
        projects: Vec::new(),
        unowned: Vec::new(),
        reconciliation: swamp_core::report::Reconciliation {
            unique_estimate: None,
            attributed: 0,
            unowned: 0,
            walked_total: 0,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        },
        series_by_key: Default::default(),
        total_series: Vec::new(),
        series_window_secs: 0,
        notes: Vec::new(),
        dirs_by_worktree: None,
        files_by_worktree: None,
        schedule_line: None,
        summary: Default::default(),
        github_enrichment: None,
        nested_artifacts: Vec::new(),
    }
}

#[test]
fn action_claims_are_backed_by_role_contracts_and_project_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    let units = identify(&root, &dirs);
    assert!(!units.is_empty(), "the mixed project identified nothing");
    for u in &units {
        if u.action == NestedActionCapability::TrashPath {
            let registry = Registry::with_builtins();
            let adapter = registry.get(u.adapter.as_deref().unwrap()).unwrap();
            assert!(adapter.trash_roles().contains(&u.role));
            assert!(u.path.starts_with(&root) && u.path != root);
            assert!(u.consequence.is_some() && u.coverage.supported);
        }
        if u.coverage.supported {
            assert!(
                !u.producer_evidence.is_empty(),
                "{} claims support with no evidence behind it",
                u.path.display()
            );
        } else {
            assert!(
                !u.coverage.limits.is_empty(),
                "{} is unsupported with no stated limit, which is a silent omission",
                u.path.display()
            );
        }
    }
}

#[test]
fn every_ecosystem_in_one_checkout_is_identified_by_its_own_adapter() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    let units = identify(&root, &dirs);
    let adapter_of = |rel: &str| {
        units
            .iter()
            .find(|u| u.path == root.join(rel))
            .unwrap_or_else(|| panic!("no unit for {rel}"))
            .adapter
            .clone()
            .unwrap_or_default()
    };
    assert_eq!(adapter_of("target"), "cargo");
    assert_eq!(adapter_of("node_modules"), "node");
    assert_eq!(adapter_of("dist"), "node");
    assert_eq!(adapter_of("coverage"), "node");
    // `build/` is ambiguous between Node and Gradle. The JVM markers are
    // present, so it is Gradle's -- every pass, by registry order plus
    // the adapters' own marker checks, never by iteration luck.
    assert_eq!(adapter_of("build"), "gradle");
    assert_eq!(adapter_of(".gradle"), "gradle");
}

#[test]
fn aggregate_nodes_carry_no_physical_charge() {
    // #65: a container's total already includes its descendants, so a
    // directory unit charging physical bytes would count them twice.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    for u in identify(&root, &dirs).iter().filter(|u| u.is_dir) {
        assert_eq!(
            u.physical_total,
            0,
            "{} is an aggregate and charges {} physical bytes",
            u.path.display(),
            u.physical_total
        );
    }
}

#[test]
fn family_summaries_never_exceed_their_container() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    let units = identify(&root, &dirs);
    let node_modules = root.join("node_modules");
    let inside: Vec<_> = units
        .iter()
        .filter(|u| u.path.starts_with(&node_modules))
        .cloned()
        .collect();
    let container_bytes = inside
        .iter()
        .find(|u| u.path == node_modules)
        .expect("the container itself")
        .bytes;
    let total: u64 = summarize_families(&node_modules, &inside)
        .iter()
        .map(|f| f.bytes)
        .sum();
    assert!(
        total <= container_bytes,
        "the family rows sum to {total}, more than the container's {container_bytes}: a \
         descendant is being counted inside its ancestor and again on its own"
    );
}

#[test]
fn unknown_ages_rank_last_and_are_never_treated_as_ancient() {
    // #64's cleanup-guidance contract: "unknown/future times must not
    // rank as ancient". A unit with no known time reports `None`, and
    // the count of such units is a separate number.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    let units = identify(&root, &dirs);
    for f in summarize_families(&root, &units) {
        if let Some(oldest) = f.oldest_modified {
            assert!(
                oldest > 0,
                "family {} reports an epoch-zero oldest modification, which is how an unknown \
                 time becomes \"ancient\"",
                f.family.label()
            );
        }
        assert!(f.unknown_age <= f.count);
    }
    for u in &units {
        if u.time_source == TimeSource::Unknown {
            assert_eq!(
                u.mtime_max,
                0,
                "{} has an unknown time source and a nonzero time",
                u.path.display()
            );
        }
    }
}

#[test]
fn identification_spawns_no_processes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    // Executables named exactly what an adapter would be tempted to run.
    for name in ["gradlew", "mvnw"] {
        fs::write(root.join(name), b"#!/bin/sh\nexit 11\n").unwrap();
    }
    let (_units, counted) = swamp_core::work_counters::measured(|| identify(&root, &dirs));
    assert_eq!(
        counted.subprocess_spawns, 0,
        "identification spawned a process; no build tool is ever invoked to explain its own output"
    );
}

#[test]
fn reclassification_never_fabricates_growth() {
    // #65: "Metadata-only reclassification [...] must not manufacture
    // physical growth". The same bytes classified two different ways
    // must produce the same byte totals, so a role change can never
    // reach the history as a delta.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    let before = identify(&root, &dirs);
    // Turn `dist/` into something the adapter classifies differently by
    // adding the JVM marker set's ambiguity resolver -- the *bytes* do
    // not move.
    fs::write(root.join("settings.gradle"), b"").unwrap();
    let after = identify(&root, &dirs);
    let sum = |v: &[swamp_core::artifact::NestedArtifact]| -> u64 {
        v.iter().filter(|u| u.is_dir).map(|u| u.bytes).sum()
    };
    let (b, a) = (sum(&before), sum(&after));
    assert!(
        a <= b,
        "reclassification increased the measured total from {b} to {a}; a role is evidence, not \
         bytes"
    );
    for u in &after {
        assert_ne!(
            u.role.family(),
            RoleFamily::Unknown,
            "{} fell into the unknown family after reclassification",
            u.path.display()
        );
    }
}

#[test]
fn docs_table_equals_the_capability_matrix() {
    // The executable half of `build_adapter_matrix_matches_docs`: the
    // published table and the code's own table name the same families
    // with the same statuses, in both directions.
    let doc = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("docs/build-artifacts.md"),
    )
    .expect("docs/build-artifacts.md");

    // Support-matrix rows: `| id | status | families | layouts | limits |
    // granularity | actions |`.
    let rows: Vec<(String, Vec<String>)> = doc
        .lines()
        .filter(|l| l.starts_with('|'))
        .filter_map(|l| {
            let cells: Vec<String> = l.split('|').map(|c| c.trim().to_string()).collect();
            if cells.len() != 9 {
                return None;
            }
            let id = cells[1].strip_prefix('`')?.split('`').next()?.to_string();
            Some((id, cells))
        })
        .filter(|(id, _)| matrix::MATRIX.iter().any(|e| e.id == *id))
        .collect();

    for e in matrix::MATRIX {
        let (_, cells) = rows
            .iter()
            .find(|(id, _)| id == e.id)
            .unwrap_or_else(|| panic!("docs/build-artifacts.md has no row for `{}`", e.id));
        assert_eq!(
            cells[2],
            e.status.label(),
            "`{}` is {} in code and {} in the docs",
            e.id,
            e.status.label(),
            cells[2]
        );
        let families = if e.families.is_empty() {
            "--".to_string()
        } else {
            e.families
                .iter()
                .map(|f| f.label())
                .collect::<Vec<_>>()
                .join(", ")
        };
        assert_eq!(cells[3], families, "`{}`: families column", e.id);
        assert_eq!(
            cells[6], e.operation_granularity,
            "`{}`: operation granularity column",
            e.id
        );
        assert_eq!(cells[7], e.actions, "`{}`: actions column", e.id);
    }
    let mut ids: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut coded: Vec<&str> = matrix::MATRIX.iter().map(|e| e.id).collect();
    coded.sort_unstable();
    assert_eq!(
        ids, coded,
        "the docs table and the capability matrix name different families"
    );

    let registered = Registry::with_builtins().ids();
    for e in matrix::MATRIX {
        let implemented = e.status == matrix::Status::Implemented;
        assert_eq!(
            implemented,
            registered.contains(&e.id),
            "`{}` says {} but the registry {}",
            e.id,
            e.status.label(),
            if registered.contains(&e.id) {
                "runs it"
            } else {
                "does not"
            }
        );
    }
}

#[test]
fn no_unit_renders_a_verdict() {
    // The facts-not-verdicts contract, over every string a build unit
    // can put in front of a person: role labels, limits, consequences
    // and evidence details.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    let units = identify(&root, &dirs);
    let banned = [
        "safe to delete",
        "can be deleted",
        "unused",
        "obsolete",
        "no longer needed",
        "stale",
    ];
    for u in &units {
        let mut text = vec![
            u.role.label().to_string(),
            u.consequence.clone().unwrap_or_default(),
        ];
        text.extend(u.coverage.limits.clone());
        text.extend(u.producer_evidence.iter().map(|e| e.detail.clone()));
        if let NestedActionCapability::Unsupported { reason } = &u.action {
            text.push(reason.clone());
        }
        let joined = text.join(" ").to_ascii_lowercase();
        for word in banned {
            assert!(
                !joined.contains(word),
                "{} renders the verdict {word:?}: {joined}",
                u.path.display()
            );
        }
    }
}

#[test]
fn a_role_round_trips_through_its_label() {
    // A stored report is read back by label. A role whose label does not
    // resolve would silently become something else.
    for role in ArtifactRole::ALL {
        assert_eq!(
            ArtifactRole::from_label(role.label()).as_ref(),
            Some(role),
            "{} does not round-trip",
            role.label()
        );
    }
    for basis in [
        AccountingBasis::Allocated,
        AccountingBasis::Logical,
        AccountingBasis::UniqueAllocated,
        AccountingBasis::Unknown,
    ] {
        assert!(!basis.label().is_empty());
    }
}

/// `--view builds` shows the family breakdown, in the order the row is
/// for: what the family is and what losing it costs, then the numbers.
#[test]
fn the_builds_view_leads_with_the_consequence_and_states_its_basis() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("app");
    let dirs = mixed_project(&root);
    let units = identify(&root, &dirs);

    let mut report = blank_report(&root);
    report.projects = vec![swamp_core::report::ProjectRow {
        project_id: "p".into(),
        name: "app".into(),
        remote: None,
        ecosystems: Vec::new(),
        worktrees: vec![swamp_core::report::WorktreeRow {
            worktree_id: "w".into(),
            path: root.clone(),
            kind: swamp_core::report::WorktreeKind::Main,
            artifacts: Vec::new(),
            signals: Vec::new(),
            branch: None,
            github: None,
            merge_complete: None,
            idle_secs: None,
        }],
    }];
    report.nested_artifacts = units;

    let text = swamp_core::render::render_view_builds(&report, None);
    assert!(
        text.contains("identification only -- no cleanup is offered here"),
        "{text}"
    );
    assert!(
        text.contains("reinstall with `npm ci`"),
        "the consequence is in the view, not only in the JSON: {text}"
    );
    let line = text
        .lines()
        .find(|l| l.contains("reinstall with `npm ci`"))
        .unwrap();
    assert!(
        line.trim_start().starts_with("Installed dependencies")
            && line.find("Review: reinstall from registry") < line.find("reinstall with `npm ci`"),
        "family, then guidance, then the adapter's consequence -- the numbers come after: {line}"
    );
    assert!(
        text.contains("allocated"),
        "every family row states its accounting basis: {text}"
    );
    assert!(
        text.contains("oldest modified"),
        "the age is labelled as a modification, never as a use: {text}"
    );
    for verdict in ["safe to delete", "unused", "obsolete"] {
        assert!(!text.to_ascii_lowercase().contains(verdict), "{text}");
    }

    // A report with nothing identified prints no section at all rather
    // than an empty heading: absence is not a zero row.
    let empty = blank_report(&root);
    let text = swamp_core::render::render_view_builds(&empty, None);
    assert!(!text.contains("Inside these build containers"), "{text}");
}

/// #65: every adapter's units get nested history through the *existing*
/// current + reverse-delta store, on the same key family Cargo's units
/// already used, and without inflating the container they sit inside.
///
/// Written for a Node unit deliberately: the history wiring in
/// `consumers/growth.rs` never looked at a unit's role or adapter, but
/// nothing proved that until there was a second adapter to prove it
/// with.
#[test]
fn a_node_unit_gets_nested_history_on_the_existing_key_family() {
    let store = tempfile::tempdir().unwrap();
    // The synthetic artifact row a nested unit becomes on its way into
    // the growth store: keyed by `nested-id=`, never by its path.
    let row = |bytes: u64| {
        serde_json::json!([{
            "project_id": "p", "name": "p", "worktrees": [{
                "worktree_id": "w", "path": "/fixture", "kind": "Main", "signals": [],
                "artifacts": [{
                    "kind": "Unknown",
                    "path": "/fixture/node_modules/.pnpm",
                    "bytes": bytes, "mtime_max": 0, "observed_at": 1000,
                    "confidence": "Medium", "regrowth_count": 0,
                    "source": {"tool": "cargo.layout"},
                    "note": "nested-id=node-pnpm-unit"
                }]
            }]
        }])
    };
    let mut projects: Vec<swamp_core::report::ProjectRow> =
        serde_json::from_value(row(4096)).unwrap();
    swamp_core::growth::observe_and_annotate(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        1,
        &mut projects,
        1_000,
        30,
        100,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        projects[0].worktrees[0].artifacts[0].growth_bytes, None,
        "a first observation has nothing to compare against"
    );

    projects[0].worktrees[0].artifacts[0].bytes = 12_288;
    swamp_core::growth::observe_and_annotate(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        1,
        &mut projects,
        1_100,
        30,
        100,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        projects[0].worktrees[0].artifacts[0].growth_bytes,
        Some(8_192),
        "a Node unit's growth comes from the same store Cargo's units use"
    );

    // Coverage is not storage: re-observing the same bytes, however the
    // unit is classified, is not a delta.
    swamp_core::growth::observe_and_annotate(
        &swamp_core::bus::Stage::for_tests(),
        store.path(),
        1,
        &mut projects,
        1_200,
        30,
        100,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        projects[0].worktrees[0].artifacts[0].growth_bytes,
        Some(0),
        "re-observing the same bytes is zero growth, not the previous delta repeated: \
         coverage changes are not storage changes"
    );
}
