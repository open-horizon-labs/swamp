//! #R13 item B: only `~/src`-style built-in roots, `[scan] include`
//! entries, and explicit command roots get ordinary Git/ecosystem
//! discovery and an unowned-remainder walk. A detector-resolved location
//! (Cargo home, rustup, Homebrew, `~/Library/Caches`,
//! `~/Library/Developer`, ...) is measured only as an external unit
//! (folded row + its adapter's own interior identification) -- it is
//! never walked for projects, and a Git checkout planted inside one does
//! not become a project.
//!
//! The tempting shortcut this file exists to catch: treating "detector
//! location" as just another kind of scan root that happens to get a
//! label, so a Git repository dropped inside one still gets discovered,
//! grouped, and reported as an ordinary project with its own unowned
//! remainder -- exactly the double-purpose treatment the user rejected.

use std::fs;
use std::path::PathBuf;

use swamp_core::coverage::RegionStatus;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::report::report_scope;
use swamp_core::scope::{RootReason, ScanConfig, resolve_effective_scope};

fn init_git_repo(path: &std::path::Path) {
    fs::create_dir_all(path).unwrap();
    let out = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(path)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .expect("git init");
    assert!(out.status.success(), "{:?}", out);
    fs::write(path.join("README.md"), b"hello\n").unwrap();
    let add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = std::process::Command::new("git")
        .args([
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=T",
            "commit",
            "-q",
            "-m",
            "init",
        ])
        .current_dir(path)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .output()
        .unwrap();
    assert!(commit.status.success(), "{:?}", commit);
}

/// Every detector except `keep` disabled, so a fixture home never picks
/// up incidental noise from other catalog entries.
fn only_config(keep: &[&str], registry: &Registry) -> ScanConfig {
    let disabled: Vec<String> = registry
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| id != "builtin-defaults" && !keep.contains(&id.as_str()))
        .collect();
    ScanConfig {
        defaults: true,
        disabled_detectors: disabled,
        enabled_detectors: keep.iter().map(|s| s.to_string()).collect(),
        ..ScanConfig::default()
    }
}

/// A Git repository planted inside a fake detector home (here: a
/// Cargo-home-shaped `CARGO_HOME`) must not become a project, and the
/// scope must never walk that home for an unowned remainder either.
#[test]
fn a_git_repo_inside_a_fake_detector_home_is_not_a_project_and_yields_no_unowned_rows() {
    let home = tempfile::tempdir().unwrap();
    let cargo_home = home.path().join("fixture-cargo-home");
    fs::create_dir_all(&cargo_home).unwrap();
    // A real project, deep enough inside the detector home to survive
    // any naive "only check the immediate entry" nesting logic.
    let planted_repo = cargo_home.join("registry/src/sneaky-repo");
    init_git_repo(&planted_repo);
    fs::write(planted_repo.join("extra.txt"), vec![b'x'; 5_000]).unwrap();
    // Ordinary detector-home noise alongside it, so the fixture also
    // proves a non-project detector location produces no unowned rows.
    fs::create_dir_all(cargo_home.join("registry/cache")).unwrap();
    fs::write(
        cargo_home.join("registry/cache/some-crate.crate"),
        vec![b'c'; 8_000],
    )
    .unwrap();

    let env = Environment::fixture(
        home.path().to_path_buf(),
        std::collections::HashMap::from([(
            "CARGO_HOME".to_string(),
            cargo_home.display().to_string(),
        )]),
        Platform::MacOS,
    );
    let registry = Registry::with_builtins();
    let cfg = only_config(&["cargo-home"], &registry);
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);

    let cargo_home_root = scope
        .roots
        .iter()
        .find(|r| r.path == cargo_home)
        .expect("the fixture CARGO_HOME must be a candidate root");
    assert!(
        !cargo_home_root.is_project_root(),
        "a detector-resolved location must never be a project root"
    );
    assert!(
        cargo_home_root.reasons.iter().all(|r| !matches!(
            r,
            RootReason::BuiltinDefault | RootReason::Included | RootReason::ExplicitCommand
        )),
        "only Detector/NestedFrom reasons are allowed on a detector location: {:?}",
        cargo_home_root.reasons
    );

    let (report, coverage) =
        report_scope(&scope, None, false, None, None, false, false, false, true)
            .expect("report_scope succeeds over the fixture scope");

    assert!(
        report
            .projects
            .iter()
            .flat_map(|p| &p.worktrees)
            .all(|w| !w.path.starts_with(&cargo_home)),
        "the Git repo planted inside the detector home must not surface as a project: {:?}",
        report.projects
    );
    assert!(
        report
            .unowned
            .iter()
            .all(|u| !PathBuf::from(&u.path_or_object).starts_with(&cargo_home)),
        "a detector location must produce no unowned rows at all: {:?}",
        report.unowned
    );
    assert_eq!(
        report.reconciliation.walked_total, 0,
        "nothing under the detector home should be attributed to the ordinary walk"
    );
    let cargo_home_coverage = coverage
        .iter()
        .find(|c| c.path == cargo_home)
        .expect("the detector home gets its own coverage row");
    assert_eq!(cargo_home_coverage.status, RegionStatus::DetectorOnly);

    // It is still measured -- as an external unit, not a project.
    let units = swamp_core::external::discover_and_measure(
        &scope,
        None,
        false,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .expect("discover_and_measure succeeds");
    assert!(
        units
            .iter()
            .any(|u| u.detector_id == "cargo-home" && u.bytes > 0),
        "the Cargo home must still be measured as an external unit: {units:?}"
    );
}

/// `~/Library/Caches` and `~/Library/Developer` are the built-in
/// defaults detector's own candidates, but only `~/src` is a project
/// root: the other two must resolve as detector locations.
#[test]
fn macos_builtin_caches_and_developer_are_detector_locations_not_project_roots() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join("src")).unwrap();
    fs::create_dir_all(home.path().join("Library/Caches")).unwrap();
    fs::create_dir_all(home.path().join("Library/Developer")).unwrap();

    let env = Environment::fixture(
        home.path().to_path_buf(),
        std::collections::HashMap::new(),
        Platform::MacOS,
    );
    let registry = Registry::with_builtins();
    let cfg = only_config(&[], &registry);
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);

    let src_root = scope
        .roots
        .iter()
        .find(|r| r.path == home.path().join("src"))
        .expect("~/src is a candidate");
    assert!(src_root.is_project_root(), "~/src must be a project root");

    for sub in ["Library/Caches", "Library/Developer"] {
        let root = scope
            .roots
            .iter()
            .find(|r| r.path == home.path().join(sub))
            .unwrap_or_else(|| panic!("{sub} must be a candidate root"));
        assert!(
            !root.is_project_root(),
            "{sub} must be a detector location, not a project root"
        );
    }

    // Being excluded from the project walk must never mean being
    // excluded from measurement altogether: both still show up as
    // external units (this is the whole point of the split -- they are
    // *only* measured this way now).
    std::fs::write(home.path().join("Library/Caches/blob"), vec![b'c'; 4_096]).unwrap();
    let units = swamp_core::external::discover_and_measure(
        &scope,
        None,
        false,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .expect("discover_and_measure succeeds");
    assert!(
        units
            .iter()
            .any(|u| u.detector_id == "builtin-defaults" && u.bytes > 0),
        "~/Library/Caches must be measured as an external unit, not silently unmeasured: {units:?}"
    );
}
