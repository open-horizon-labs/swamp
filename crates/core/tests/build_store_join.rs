//! Machine-wide build stores, joined into the live report
//! (`.oh/guardrails/build-stores-join-by-capability.md`).
//!
//! Part 1 of the build-adapter work identified the interior of a Maven
//! repository, a Gradle home, an npm store -- and none of it reached a
//! user's report, because the stores are external locations the external
//! observation owns. These tests drive the join through the real
//! pipeline (`report::observe_scope`, which walks the scope and runs
//! `external::observe_external` in the same pass), and prove the
//! properties a join like this can get wrong: that every declared store
//! kind reaches its adapter, that a custom store root with no
//! conventional suffix still does, that interiors get history on their
//! own key family and only the observation that measured a store may
//! sweep it, and that an unchanged store costs nothing.
//!
//! Disposable `tempfile` fixtures throughout. Every "store" is a
//! synthetic tree this file wrote; no real tool home is read.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::artifact::{ArtifactRole, NestedArtifact};
use swamp_core::fs_events::EventCoverage;
use swamp_core::locations::{Environment, Platform, Registry, StorageCategory};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};

/// The work counters are process-global; the cost test would measure the
/// others.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

const STORE_DETECTORS: &[&str] = &[
    "maven", "gradle", "npm", "go", "pip", "uv", "xcode", "android",
];

fn config(enabled: &[&str], exclude: &[PathBuf]) -> ScanConfig {
    ScanConfig {
        defaults: false,
        enabled_detectors: enabled.iter().map(|s| s.to_string()).collect(),
        exclude: exclude.iter().map(|p| p.display().to_string()).collect(),
        ..Default::default()
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    env: HashMap<String, String>,
    /// The custom module cache: no `pkg/mod` suffix anywhere.
    gomodcache: PathBuf,
}

impl Fixture {
    fn scope(&self, enabled: &[&str], exclude: &[PathBuf]) -> EffectiveScope {
        let env = Environment::fixture(self.home.clone(), self.env.clone(), Platform::MacOS);
        resolve_effective_scope(
            &env,
            &config(enabled, exclude),
            &[],
            &Registry::with_builtins(),
            1_000,
        )
    }

    /// The path the scope resolved for `detector`'s location in `category`
    /// whose path ends with `suffix` (or its only one).
    fn located(&self, detector: &str, category: StorageCategory, suffix: &str) -> PathBuf {
        let scope = self.scope(STORE_DETECTORS, &[]);
        scope
            .detectors
            .iter()
            .find(|d| d.detector_id == detector)
            .and_then(|d| {
                d.locations_for_display()
                    .iter()
                    .filter(|l| l.category == category)
                    .filter_map(|l| l.path.clone())
                    .find(|p: &PathBuf| p.ends_with(suffix))
            })
            .unwrap_or_else(|| {
                panic!("{detector} proposes no {category:?} location ending {suffix}")
            })
    }
}

fn put(p: &Path, bytes: usize) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![b'x'; bytes]).unwrap();
}

fn put_text(p: &Path, text: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// One of every declared store, each with a little identifiable content.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let gomodcache = base.join("elsewhere/modules");
    let gocache = base.join("elsewhere/compiled");
    let mut env = HashMap::new();
    env.insert("GOMODCACHE".to_string(), gomodcache.display().to_string());
    env.insert("GOCACHE".to_string(), gocache.display().to_string());
    let f = Fixture {
        _tmp: tmp,
        home,
        env,
        gomodcache: gomodcache.clone(),
    };

    let m2 = f.located("maven", StorageCategory::Unclassified, "repository");
    put(&m2.join("org/example/lib/1.0/lib-1.0.jar"), 8_000);
    put_text(
        &m2.join("org/example/lib/1.0/_remote.repositories"),
        "lib-1.0.jar>central=\n",
    );

    let gradle_caches = f.located("gradle", StorageCategory::Cache, "caches");
    put(
        &gradle_caches.join("modules-2/files-2.1/org.example/core/2.0/abc123/core-2.0.jar"),
        6_000,
    );

    let npm = f.located("npm", StorageCategory::Cache, "");
    put(&npm.join("content-v2/sha512/ab/cd/blob"), 5_000);

    put(&gomodcache.join("github.com/org/lib@v1.2.0/lib.go"), 4_000);
    put(
        &gomodcache.join("cache/download/github.com/org/lib/@v/v1.2.0.zip"),
        3_000,
    );
    put_text(
        &gomodcache.join("cache/download/github.com/org/lib/@v/v1.2.0.mod"),
        "module github.com/org/lib\n",
    );
    put(&gocache.join("0a/0a1b-d"), 2_000);
    put_text(&gocache.join("trim.txt"), "1700000000\n");

    let pip = f.located("pip", StorageCategory::Cache, "");
    put(&pip.join("http-v2/a/b/c/blob"), 1_500);

    let uv = f.located("uv", StorageCategory::Cache, "");
    put(
        &uv.join("wheels-v5/pypi/requests/requests-2.31.0-py3-none-any.whl"),
        1_200,
    );

    let dd = f.located("xcode", StorageCategory::BuildOutput, "DerivedData");
    let folder = dd.join("App-abcdefghijklmnopqrstuvwxyzab");
    put_text(
        &folder.join("info.plist"),
        "<plist><dict><key>WorkspacePath</key><string>/fixture/App.xcworkspace</string></dict></plist>",
    );
    put(
        &folder.join("Build/Products/Debug-iphonesimulator/App.app/App"),
        7_000,
    );

    let platforms = f.located("android", StorageCategory::Installation, "platforms");
    put_text(
        &platforms.join("android-34/source.properties"),
        "Pkg.Revision=3\n",
    );
    put(&platforms.join("android-34/android.jar"), 2_500);
    f
}

fn observe(scope: &EffectiveScope, store: &Path) -> swamp_core::report::ScopeObservation {
    swamp_core::report::observe_scope(
        scope,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        Some(store),
        Some("1h"),
        true,
        false,
        false,
        false,
        // Every pass a full walk: these tests are about ownership and
        // identity, and a replay window that claimed "nothing changed"
        // would hide the changes they make. Reuse is proved separately,
        // with explicit windows.
        &swamp_core::fs_events::UnsupportedPlatformSource,
        30,
        3600,
    )
    .expect("scope observation")
}

fn unit<'a>(
    units: &'a [NestedArtifact],
    pred: impl Fn(&NestedArtifact) -> bool,
    what: &str,
) -> &'a NestedArtifact {
    units.iter().find(|u| pred(u)).unwrap_or_else(|| {
        panic!(
            "no store unit {what}; got {:#?}",
            units
                .iter()
                .map(|u| (
                    u.adapter.clone(),
                    u.relative_path.clone(),
                    u.variant.package.clone()
                ))
                .collect::<Vec<_>>()
        )
    })
}

#[test]
fn every_declared_store_reaches_the_report_through_its_adapter() {
    let _g = serial();
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let o = observe(&f.scope(STORE_DETECTORS, &[]), store.path());
    let units = &o.store_interiors;

    let maven = unit(
        units,
        |u| u.variant.package.as_deref() == Some("org.example:lib"),
        "for the Maven artifact",
    );
    assert_eq!(maven.adapter.as_deref(), Some("maven"));
    assert_eq!(maven.variant.version.as_deref(), Some("1.0"));

    let gradle = unit(
        units,
        |u| u.variant.package.as_deref() == Some("org.example:core"),
        "for the Gradle module",
    );
    assert_eq!(gradle.adapter.as_deref(), Some("gradle"));

    unit(
        units,
        |u| u.adapter.as_deref() == Some("node") && u.relative_path == "content-v2",
        "for npm's content",
    );

    let go_mod = unit(
        units,
        |u| {
            u.variant.package.as_deref() == Some("github.com/org/lib")
                && u.role == ArtifactRole::SharedStoreEntry
                && u.is_dir
        },
        "for the extracted Go module",
    );
    assert!(
        go_mod.path.starts_with(&f.gomodcache),
        "a custom GOMODCACHE with no conventional suffix is still the module cache"
    );
    unit(
        units,
        |u| u.path.ends_with("v1.2.0.zip"),
        "for the Go download",
    );
    unit(
        units,
        |u| u.adapter.as_deref() == Some("go") && u.relative_path == "0a",
        "for the Go build-cache bucket",
    );
    unit(
        units,
        |u| u.adapter.as_deref() == Some("python") && u.relative_path == "http-v2",
        "for pip's cache",
    );
    unit(
        units,
        |u| u.variant.package.as_deref() == Some("requests"),
        "for uv's cached package",
    );
    let xcode = unit(
        units,
        |u| u.variant.target.as_deref() == Some("App.app"),
        "for the DerivedData product",
    );
    assert_eq!(xcode.adapter.as_deref(), Some("xcode-swift"));
    let sdk = unit(
        units,
        |u| u.variant.toolchain.as_deref() == Some("android-34"),
        "for the SDK platform",
    );
    assert_eq!(sdk.role, ArtifactRole::Installation);

    assert_no_verdict(units);

    // The CLI's text and JSON surfaces carry the interiors.
    let text = swamp_core::render::render_view_external_with(
        &o.external_units,
        &o.store_interiors,
        o.merged.observed_at,
    );
    assert!(
        text.contains("inside (identification only -- no cleanup is offered here)"),
        "{text}"
    );
    assert!(text.contains("Shared store entries"), "{text}");
    assert!(text.contains("Installed SDKs & runtimes"), "{text}");
    let m2 = f.located("maven", StorageCategory::Unclassified, "repository");
    let json = swamp_core::agent_json::interior_json(&m2, &o.store_interiors).expect("json");
    assert!(json["units"].as_array().unwrap().len() >= 2);
    assert!(
        json["families"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["action"] == "inspection-only")
    );

    // Every interior unit belongs to an external unit the same pass
    // measured, and names its container.
    for u in units {
        assert!(
            o.external_units.iter().any(|e| u.path.starts_with(&e.path)),
            "{} lies under no measured external unit",
            u.path.display()
        );
        assert!(u.container_id.is_some());
        assert!(
            u.decision_evidence
                .iter()
                .all(|e| !format!("{:?}", e.subtype).is_empty()),
            "decision evidence attached"
        );
    }
    // And no store unit is claimed by two adapters.
    let mut paths: Vec<&Path> = units.iter().map(|u| u.path.as_path()).collect();
    let before = paths.len();
    paths.sort();
    paths.dedup();
    assert_eq!(before, paths.len(), "a store unit identified twice");
}

/// `(path, bytes, growth, regrowth)` for one adapter's units *inside*
/// stores, on the store family's keys.
/// The facts-not-verdicts contract over every string a unit can put in
/// front of a person (`build_adapter_contract::no_unit_renders_a_verdict`,
/// for the store and daemon units).
fn assert_no_verdict(units: &[NestedArtifact]) {
    let banned = [
        "safe to delete",
        "can be deleted",
        "unused",
        "obsolete",
        "no longer needed",
        "stale",
    ];
    for u in units {
        let mut text = vec![
            u.role.label().to_string(),
            u.consequence.clone().unwrap_or_default(),
        ];
        text.extend(u.coverage.limits.clone());
        text.extend(u.producer_evidence.iter().map(|e| e.detail.clone()));
        if let swamp_core::artifact::NestedActionCapability::Unsupported { reason } = &u.action {
            text.push(reason.clone());
        }
        let joined = text.join(" ").to_ascii_lowercase();
        for word in banned {
            assert!(
                !joined.contains(word),
                "{} renders {word:?}: {joined}",
                u.path.display()
            );
        }
    }
}

fn history(
    o: &swamp_core::report::ScopeObservation,
    adapter: &str,
) -> Vec<(String, u64, Option<i64>, u32)> {
    let mut v: Vec<_> = o
        .store_interiors
        .iter()
        .filter(|u| u.adapter.as_deref() == Some(adapter))
        // A store's own row carries the external unit's history, whose
        // family and sweep are the external observation's, not this one's.
        .filter(|u| !o.external_units.iter().any(|e| e.path == u.path))
        .map(|u| {
            (
                u.path.display().to_string(),
                u.bytes,
                u.growth_bytes,
                u.regrowth_count,
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn store_interiors_keep_history_and_one_changed_unit_is_the_only_growth() {
    let _g = serial();
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let scope = f.scope(STORE_DETECTORS, &[]);
    observe(&scope, store.path());
    let steady = observe(&scope, store.path());
    for (path, _, growth, regrowth) in history(&steady, "maven") {
        assert_eq!(
            growth,
            Some(0),
            "{path}: re-observing the same bytes is zero growth"
        );
        assert_eq!(regrowth, 0, "{path}");
    }
    // One artifact version grows.
    let m2 = f.located("maven", StorageCategory::Unclassified, "repository");
    put(&m2.join("org/example/lib/1.0/lib-1.0-sources.jar"), 40_000);
    let grown = observe(&scope, store.path());
    let grew: Vec<_> = history(&grown, "maven")
        .into_iter()
        .filter(|(_, _, g, _)| g.is_some_and(|g| g > 0))
        .map(|(p, ..)| p)
        .collect();
    assert!(
        grew.iter().any(|p| p.ends_with("org/example/lib/1.0")),
        "the version directory that grew reports growth: {grew:?}"
    );
    for (path, _, growth, _) in history(&grown, "go") {
        assert_eq!(
            growth,
            Some(0),
            "{path}: an untouched store's units did not grow"
        );
    }
}

#[test]
fn a_disabled_excluded_or_unreadable_store_fabricates_no_disappearance_or_regrowth() {
    let _g = serial();
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let all = f.scope(STORE_DETECTORS, &[]);
    let first = observe(&all, store.path());
    let maven_before = history(&first, "maven");
    assert!(!maven_before.is_empty());

    // Pass 2a: Maven's detector disabled. Pass 2b: the Go module cache
    // excluded. Pass 2c: one Maven group unreadable.
    let without_maven: Vec<&str> = STORE_DETECTORS
        .iter()
        .copied()
        .filter(|d| *d != "maven")
        .collect();
    let disabled = observe(&f.scope(&without_maven, &[]), store.path());
    assert!(
        history(&disabled, "maven").is_empty(),
        "a disabled store is not observed"
    );
    let excluded = observe(
        &f.scope(STORE_DETECTORS, &[f.gomodcache.clone()]),
        store.path(),
    );
    assert!(
        !excluded
            .store_interiors
            .iter()
            .any(|u| u.path.starts_with(&f.gomodcache)),
        "an excluded store is not observed"
    );
    let m2 = f.located("maven", StorageCategory::Unclassified, "repository");
    let group = m2.join("org/example");
    let mut perms = fs::metadata(&group).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o000);
    fs::set_permissions(&group, perms.clone()).unwrap();
    let unreadable = observe(&all, store.path());
    perms.set_mode(0o755);
    fs::set_permissions(&group, perms).unwrap();
    assert!(
        !history(&unreadable, "maven")
            .iter()
            .any(|(p, ..)| p.ends_with("org/example/lib/1.0")),
        "precondition: the unreadable version is not observed this pass"
    );

    // Everything back. Nothing moved on disk, so nothing may have
    // regrown and nothing may report growth.
    let after = observe(&all, store.path());
    for adapter in ["maven", "go"] {
        for (path, _, growth, regrowth) in history(&after, adapter) {
            assert_eq!(
                regrowth, 0,
                "{path}: a store unit that was disabled, excluded or unreadable was tombstoned by a \
                 pass that did not observe it, and scored a regrowth when it came back"
            );
            assert_eq!(
                growth,
                Some(0),
                "{path}: coverage changes are not storage changes"
            );
        }
    }
    assert_eq!(
        history(&after, "maven")
            .iter()
            .map(|(p, b, ..)| (p.clone(), *b))
            .collect::<Vec<_>>(),
        maven_before
            .iter()
            .map(|(p, b, ..)| (p.clone(), *b))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn a_real_removal_inside_an_observed_store_is_a_disappearance_and_its_return_a_regrowth() {
    let _g = serial();
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let scope = f.scope(STORE_DETECTORS, &[]);
    observe(&scope, store.path());
    let m2 = f.located("maven", StorageCategory::Unclassified, "repository");
    let version = m2.join("org/example/lib/1.0");
    let backup = m2.join("../lib-1.0.bak");
    fs::rename(&version, &backup).unwrap();
    let gone = observe(&scope, store.path());
    assert!(
        !history(&gone, "maven")
            .iter()
            .any(|(p, ..)| p.ends_with("lib/1.0"))
    );
    fs::rename(&backup, &version).unwrap();
    let back = observe(&scope, store.path());
    let v = history(&back, "maven")
        .into_iter()
        .find(|(p, ..)| p.ends_with("org/example/lib/1.0"))
        .expect("the version is back");
    assert_eq!(
        v.3, 1,
        "a genuine removal and return inside an observed store is one regrowth"
    );
}

#[test]
fn both_orders_with_agent_discovery_leave_store_history_alone() {
    let _g = serial();
    for agent_first in [false, true] {
        let f = fixture();
        let claude = f.home.join(".claude/projects/-repo");
        put_text(
            &claude.join("0000-0000.jsonl"),
            "{\"type\":\"user\",\"sessionId\":\"0000-0000\",\"cwd\":\"/repo\"}\n",
        );
        let mut enabled = STORE_DETECTORS.to_vec();
        enabled.push("claude-code");
        let scope = f.scope(&enabled, &[]);
        let store = tempfile::tempdir().unwrap();
        let mut last = Vec::new();
        for at in [1_000u64, 2_000, 3_000] {
            let run_external = || {
                swamp_core::external::observe_external(
                    &swamp_core::report::DiscoveryPass::for_tests(),
                    &scope,
                    Some(store.path()),
                    true,
                    at,
                    30,
                    3600,
                    &EventCoverage::untrusted(),
                )
                .unwrap()
            };
            let run_agents = || {
                swamp_core::agents::discover_and_measure(
                    &scope,
                    &[],
                    Some(store.path()),
                    true,
                    at,
                    30,
                    3600,
                    &EventCoverage::untrusted(),
                )
                .unwrap()
            };
            let ext = if agent_first {
                let _ = run_agents();
                run_external()
            } else {
                let e = run_external();
                let _ = run_agents();
                e
            };
            last = ext
                .interiors
                .iter()
                .map(|u| (u.path.clone(), u.regrowth_count))
                .collect();
        }
        assert!(!last.is_empty());
        for (path, regrowth) in last {
            assert_eq!(
                regrowth,
                0,
                "{} (agent_first={agent_first}): the other family's sweep touched a store row",
                path.display()
            );
        }
    }
}

#[test]
fn an_unchanged_store_replays_its_units_with_zero_listings_and_zero_reads() {
    let _g = serial();
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let scope = f.scope(&["maven", "go"], &[]);
    let root = f.home.parent().unwrap().to_path_buf();
    let obs = |at: u64, cov: &EventCoverage| {
        swamp_core::external::observe_external(
            &swamp_core::report::DiscoveryPass::for_tests(),
            &scope,
            Some(store.path()),
            true,
            at,
            30,
            3600,
            cov,
        )
        .unwrap()
    };
    // Cold: every store walked and identified.
    let (cold, cold_work) =
        swamp_core::work_counters::measured(|| obs(1_000, &EventCoverage::untrusted()));
    assert!(cold_work.dirs_listed > 0);
    // Warm, under a trusted window that reports nothing.
    let quiet = EventCoverage::trusted(root.clone(), Vec::new(), 1_000);
    let (warm, warm_work) = swamp_core::work_counters::measured(|| obs(2_000, &quiet));
    assert_eq!(
        warm_work.dirs_listed, 0,
        "an unchanged store is not listed: {warm_work:?}"
    );
    assert_eq!(warm_work.files_statted, 0, "or statted");
    assert_eq!(warm_work.header_bytes_read, 0, "or read");
    assert!(warm_work.containers_reused > 0);
    assert_eq!(warm_work.containers_identified, 0, "{warm_work:?}");
    let key = |o: &swamp_core::external::ExternalObservation| {
        let mut v: Vec<(PathBuf, u64, String)> = o
            .interiors
            .iter()
            .map(|u| (u.path.clone(), u.bytes, u.role.label().to_string()))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        key(&warm),
        key(&cold),
        "the replayed interior is the identified one"
    );

    // One store changes: only it is walked and identified again.
    let m2 = f.located("maven", StorageCategory::Unclassified, "repository");
    put(&m2.join("org/example/lib/1.1/lib-1.1.jar"), 9_000);
    let changed = EventCoverage::trusted(root, vec![m2.join("org/example/lib/1.1")], 2_000);
    let (after, one_work) = swamp_core::work_counters::measured(|| obs(3_000, &changed));
    assert_eq!(one_work.containers_identified, 1, "{one_work:?}");
    assert!(
        one_work.containers_reused >= 3,
        "the Go stores replay: {one_work:?}"
    );
    assert!(
        after
            .interiors
            .iter()
            .any(|u| u.path.ends_with("org/example/lib/1.1")),
        "the new version is identified"
    );
    eprintln!(
        "store join cost: cold dirs_listed={} files_statted={} manifest_bytes={} identified={}; \
         unchanged dirs_listed={} files_statted={} manifest_bytes={} reused={}; one store changed \
         dirs_listed={} files_statted={} manifest_bytes={} reused={} identified={}",
        cold_work.dirs_listed,
        cold_work.files_statted,
        cold_work.header_bytes_read,
        cold_work.containers_identified,
        warm_work.dirs_listed,
        warm_work.files_statted,
        warm_work.header_bytes_read,
        warm_work.containers_reused,
        one_work.dirs_listed,
        one_work.files_statted,
        one_work.header_bytes_read,
        one_work.containers_reused,
        one_work.containers_identified,
    );
}

#[test]
fn store_units_persist_as_a_parquet_table_and_replay_across_passes() {
    // The replayed units come from the store's own columnar table, one
    // row per (unit, field) -- not from a JSON sidecar, and not from the
    // previous report, which a CLI invocation may never have written.
    let _g = serial();
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let scope = f.scope(&["maven"], &[]);
    let root = f.home.parent().unwrap().to_path_buf();
    swamp_core::external::observe_external(
        &swamp_core::report::DiscoveryPass::for_tests(),
        &scope,
        Some(store.path()),
        true,
        1_000,
        30,
        3600,
        &EventCoverage::untrusted(),
    )
    .unwrap();
    let table = store.path().join("associations/build_stores.parquet");
    assert!(
        table.exists(),
        "the store units are persisted in a Parquet table"
    );
    let quiet = EventCoverage::trusted(root, Vec::new(), 1_000);
    let (o, work) = swamp_core::work_counters::measured(|| {
        swamp_core::external::observe_external(
            &swamp_core::report::DiscoveryPass::for_tests(),
            &scope,
            Some(store.path()),
            true,
            2_000,
            30,
            3600,
            &quiet,
        )
        .unwrap()
    });
    assert_eq!(
        work.containers_identified, 0,
        "a quiet window replays: {work:?}"
    );
    assert!(
        o.interiors
            .iter()
            .any(|u| u.variant.package.as_deref() == Some("org.example:lib"))
    );
}

/// A daemon's answers in the documented CLI shapes: two builders, a
/// shared parent, an in-use child cache mount.
const DOCKER_FACTS: &str = r#"{
  "Images": [], "Volumes": [],
  "BuildCache": [
    {"ID": "p1", "CacheType": "regular", "Description": "[build 2/5] RUN make", "Size": "120MB",
     "CreatedAt": "2024-01-15T10:32:00Z", "LastUsedAt": "2024-02-01T08:00:00Z", "UsageCount": 4,
     "InUse": false, "Shared": true},
    {"ID": "c1", "CacheType": "exec.cachemount", "Size": "30MB", "Parents": ["p1"],
     "CreatedAt": "2024-01-15T10:33:00Z", "InUse": true, "Shared": false}
  ],
  "Version": {"Server": {"ApiVersion": "1.45"}},
  "Builders": [{"Name": "ci", "Driver": "docker-container", "Nodes": [{"Status": "running"}]}],
  "BuildxDu": [{"Builder": "ci", "Records": [
     {"ID": "x1", "Type": "source.local", "Size": 5000000, "Reclaimable": true, "Shared": false,
      "CreatedAt": "2024-03-01T09:00:00Z"}]}]
}"#;

fn docker_report(facts: &Path) -> swamp_core::Report {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("src");
    fs::create_dir_all(&root).unwrap();
    swamp_core::report::report_full_mode_with_source(
        &root,
        Some(facts),
        false,
        None,
        None,
        false,
        false,
        false,
        true,
        &swamp_core::fs_events::UnsupportedPlatformSource,
    )
    .unwrap()
}

#[test]
fn buildkit_records_reach_the_report_as_daemon_facts_never_filesystem_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let facts = tmp.path().join("docker.json");
    fs::write(&facts, DOCKER_FACTS).unwrap();
    let r = docker_report(&facts);
    let records: Vec<&NestedArtifact> = r
        .nested_artifacts
        .iter()
        .filter(|u| u.reported_by.is_some())
        .collect();
    let rel: Vec<&str> = records.iter().map(|u| u.relative_path.as_str()).collect();
    for want in ["p1", "c1", "x1"] {
        assert!(rel.contains(&want), "{want} missing from {rel:?}");
    }
    let p1 = records.iter().find(|u| u.relative_path == "p1").unwrap();
    let c1 = records.iter().find(|u| u.relative_path == "c1").unwrap();
    assert_eq!(
        (p1.bytes, c1.bytes),
        (120_000_000, 30_000_000),
        "each record's own size"
    );
    assert_ne!(
        p1.container_id,
        records
            .iter()
            .find(|u| u.relative_path == "x1")
            .unwrap()
            .container_id,
        "two builders are two containers"
    );
    assert_no_verdict(&r.nested_artifacts);
    let text = swamp_core::render::render_view_docker(&r, None);
    assert!(
        text.contains("BuildKit build cache, as the daemon reports it"),
        "{text}"
    );
    assert!(text.contains("exec.cachemount"), "{text}");
    assert!(text.contains("in use"), "{text}");
    let payload = swamp_core::agent_json::buildkit_payload(&r);
    let builders: Vec<&str> = payload
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["builder"].as_str().unwrap())
        .collect();
    assert_eq!(builders, vec!["ci", "default"]);
    for u in &records {
        assert!(
            !format!("{:?}", u.decision_evidence).contains("FilesystemMetadata"),
            "{}: a daemon record carries no filesystem provenance: {:?}",
            u.relative_path,
            u.decision_evidence
        );
        assert!(
            matches!(
                u.action,
                swamp_core::artifact::NestedActionCapability::Unsupported { .. }
            ),
            "{}: an inspectable record never receives a delete action",
            u.relative_path
        );
    }
}

#[test]
fn an_unavailable_daemon_is_a_note_not_an_empty_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let r = docker_report(&tmp.path().join("missing.json"));
    assert!(
        !r.nested_artifacts.iter().any(|u| u.reported_by.is_some()),
        "no records are invented for a daemon that did not answer"
    );
    assert!(
        r.notes.iter().any(|n| n.contains("docker: unavailable")),
        "{:?}",
        r.notes
    );
}
