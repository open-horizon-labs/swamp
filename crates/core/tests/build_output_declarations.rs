//! Declaration transport is populated from source directories even when the
//! only output is outside the checkout and report directory detail is off.
use std::fs;
use std::process::Command;
use swamp_core::report;

fn checkout(root: &std::path::Path) {
    fs::create_dir_all(root).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn nested_project_without_local_output_keeps_its_external_declaration() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("repo");
    checkout(&root);
    let nested = root.join("packages/app");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("package.json"), "{}").unwrap();
    let output = home.join("ts-output");
    fs::write(
        nested.join("tsconfig.json"),
        serde_json::json!({
            "compilerOptions": {"outDir": output}
        })
        .to_string(),
    )
    .unwrap();
    let observed = report::report_with(&root, None, false, None, None).unwrap();
    assert!(observed.dirs_by_worktree.is_none());
    assert!(
        observed
            .configured_outputs
            .iter()
            .any(|o| o.path == output && o.project_root == nested),
        "expected nested declaration, got {:?}",
        observed.configured_outputs
    );
}

#[test]
fn dependency_manifests_do_not_become_output_consumers() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("repo");
    checkout(&root);
    fs::write(root.join("package.json"), "{}").unwrap();
    let dependency = root.join("node_modules/dependency");
    fs::create_dir_all(&dependency).unwrap();
    fs::write(dependency.join("package.json"), "{}").unwrap();
    fs::write(
        dependency.join("tsconfig.json"),
        r#"{"compilerOptions":{"outDir":"/tmp/not-a-project-output"}}"#,
    )
    .unwrap();
    let observed = report::report_with(&root, None, false, None, None).unwrap();
    assert!(observed.configured_outputs.is_empty());
}

#[test]
fn nested_tsconfig_without_package_manifest_is_a_project_declaration() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("repo");
    checkout(&root);
    fs::write(root.join("package.json"), "{}").unwrap();
    let nested = root.join("packages/app");
    fs::create_dir_all(&nested).unwrap();
    let output = home.join("tsconfig-only-output");
    fs::write(
        nested.join("tsconfig.json"),
        serde_json::json!({
            "compilerOptions": {"outDir": output}
        })
        .to_string(),
    )
    .unwrap();
    let observed = report::report_with(&root, None, false, None, None).unwrap();
    assert!(
        observed
            .configured_outputs
            .iter()
            .any(|o| o.path == output && o.project_root == nested),
        "expected nested declaration, got {:?}",
        observed.configured_outputs
    );
}

fn observe_only_project(
    home: &std::path::Path,
    root: &std::path::Path,
) -> swamp_core::report::ScopeObservation {
    use swamp_core::{
        locations::{Environment, Platform, Registry},
        scope::{ScanConfig, resolve_effective_scope},
    };
    let registry = Registry::with_builtins();
    let config = ScanConfig {
        defaults: false,
        include: vec![root.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_owned())
            .collect(),
        ..Default::default()
    };
    let environment = Environment::fixture(home.to_path_buf(), Default::default(), Platform::MacOS);
    let scope = resolve_effective_scope(&environment, &config, &[], &registry, 1000);
    report::observe_scope(
        &scope,
        report::ObservationParts::ALL,
        None,
        None,
        false,
        None,
        None,
        true,
        false,
        false,
        true,
        &swamp_core::fs_events::UnsupportedPlatformSource,
        30,
        86400,
    )
    .unwrap()
}

#[test]
fn broad_output_declarations_never_receive_an_event_root_or_measurement() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("repo");
    checkout(&root);
    fs::write(root.join("package.json"), "{}").unwrap();
    for path in [&root, &home, std::path::Path::new("/")] {
        fs::write(
            root.join("tsconfig.json"),
            serde_json::json!({"compilerOptions":{"outDir": path}}).to_string(),
        )
        .unwrap();
        let observation = observe_only_project(&home, &root);
        assert!(
            observation.external_units.is_empty(),
            "broad declaration {path:?} must not authorize storage measurement"
        );
        assert!(
            observation.unit_root_coverage.is_empty(),
            "broad declaration {path:?} must be rejected before event replay"
        );
        assert!(
            observation
                .merged
                .notes
                .iter()
                .any(|note| note.contains("would contain its declaring project"))
        );
    }
}

#[test]
fn missing_output_is_a_visible_reference_and_coverage_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("repo");
    checkout(&root);
    let output = home.join("missing-output");
    fs::write(
        root.join("tsconfig.json"),
        serde_json::json!({"compilerOptions":{"outDir": output}}).to_string(),
    )
    .unwrap();
    let observation = observe_only_project(&home, &root);
    assert!(observation.external_units.is_empty());
    assert!(
        observation
            .merged
            .configured_outputs
            .iter()
            .any(|reference| reference.path == output)
    );
    assert!(
        observation
            .merged
            .notes
            .iter()
            .any(|note| note.contains("was referenced but is not present")
                && note.contains("missing-output"))
    );
}

#[test]
fn linked_worktree_outside_root_supplies_its_own_configured_output() {
    let tmp = tempfile::tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("repo");
    checkout(&root);
    let run = |args: &[&str]| {
        let result = Command::new("git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.test",
                "-C",
            ])
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "git fixture: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run(&["commit", "--allow-empty", "-qm", "fixture"]);
    let linked = home.join("linked");
    run(&["worktree", "add", "-qb", "linked", linked.to_str().unwrap()]);
    fs::write(
        linked.join("Cargo.toml"),
        "[package]\nname='linked'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::create_dir_all(linked.join(".cargo")).unwrap();
    fs::write(
        linked.join(".cargo/config.toml"),
        "[build]\ntarget-dir='../linked-output'\n",
    )
    .unwrap();
    let report = report::report_with(&root, None, false, None, None).unwrap();
    assert!(
        report
            .configured_outputs
            .iter()
            .any(|reference| reference.project_root == linked
                && reference.path == home.join("linked-output")),
        "linked worktree metadata must use the linked checkout's own paths: {:?}",
        report.configured_outputs
    );
}
