//! #42/#50 end-to-end: `swamp report` with no explicit root observes the
//! *whole* configured scope coherently, not just its first present root,
//! and exposes per-root coverage in the JSON contract.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

/// `swamp observe` is the only scanner (R12): every fixture below must
/// run it before `report` can read anything back.
fn observe(store: &std::path::Path, home: Option<&std::path::Path>, extra_env: &[(&str, &str)]) {
    let mut cmd = Command::new(bin());
    cmd.arg("observe")
        .env("SWAMP_DIR", store)
        .env("SWAMP_TEST_MODE", "1");
    if let Some(home) = home {
        cmd.env("HOME", home);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let status = cmd.status().expect("run observe");
    assert!(status.success(), "observe failed");
}

fn write_project(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
            .env("GIT_CONFIG_VALUE_0", "false")
            .status()
            .unwrap();
        assert!(status.success());
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), b"hi\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-q", "-m", "initial"]);
}

/// Two configured roots (via `include`, not builtin defaults) must both
/// be observed: the merged JSON report contains projects from both, and
/// `scope_coverage` is empty (both `Complete`) -- the general "keep the
/// report clean when nothing is wrong" behavior.
#[test]
fn report_with_no_explicit_root_observes_every_configured_root() {
    let home = tempfile::tempdir().unwrap();
    let root_a = home.path().join("work-a");
    let root_b = home.path().join("work-b");
    write_project(&root_a.join("proj-a"));
    write_project(&root_b.join("proj-b"));
    let store = tempfile::tempdir().unwrap();
    std::fs::write(
        store.path().join("config.toml"),
        format!(
            "[scan]\ndefaults = false\ninclude = [{:?}, {:?}]\n",
            root_a.display().to_string(),
            root_b.display().to_string()
        ),
    )
    .unwrap();

    observe(store.path(), Some(home.path()), &[]);
    let output = Command::new(bin())
        .arg("report")
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run report with no explicit root");

    assert!(
        output.status.success(),
        "report failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("report --json output parses");
    let names: Vec<String> = json["projects"]
        .as_array()
        .expect("projects array")
        .iter()
        .map(|p| p["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(names.contains(&"proj-a".to_string()), "{names:?}");
    assert!(names.contains(&"proj-b".to_string()), "{names:?}");
}

/// A configured root that does not exist on disk must show up as a
/// distinct, explicit coverage row (`missing`) in the JSON contract --
/// never silently dropped, and never counted as zero bytes of storage.
#[test]
fn report_surfaces_a_missing_configured_root_as_coverage_not_silence() {
    let home = tempfile::tempdir().unwrap();
    let present = home.path().join("present");
    write_project(&present.join("proj"));
    let missing = home.path().join("not-cloned-yet");
    let store = tempfile::tempdir().unwrap();
    std::fs::write(
        store.path().join("config.toml"),
        format!(
            "[scan]\ndefaults = false\ninclude = [{:?}, {:?}]\n",
            present.display().to_string(),
            missing.display().to_string()
        ),
    )
    .unwrap();

    observe(store.path(), Some(home.path()), &[]);
    let output = Command::new(bin())
        .arg("report")
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run report");
    assert!(
        output.status.success(),
        "report failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let coverage = json["scope_coverage"]
        .as_array()
        .expect("scope_coverage present when a region is not Complete");
    let missing_row = coverage
        .iter()
        .find(|c| c["path"].as_str() == Some(missing.display().to_string().as_str()))
        .expect("missing root has its own coverage row");
    assert_eq!(missing_row["status"], "missing");

    // The stderr note names it too, in the text-facing surface.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("scope coverage"), "{stderr}");
}

/// `--view external` (#43): a detector-resolved, project-independent
/// location (here, `CARGO_HOME` pointed at a fixture directory) shows up
/// as its own unit, end to end through the built binary and real
/// `[scan]` config, distinct from `projects`.
#[test]
fn report_view_external_lists_detector_resolved_units() {
    let home = tempfile::tempdir().unwrap();
    let cargo_home = home.path().join("fixture-cargo");
    std::fs::create_dir_all(cargo_home.join("bin")).unwrap();
    std::fs::write(cargo_home.join("bin/cargo"), vec![9u8; 5_000]).unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::write(
        store.path().join("config.toml"),
        "[scan]\ndefaults = false\ndisabled_detectors = [\"rustup\", \"homebrew\"]\n",
    )
    .unwrap();

    observe(
        store.path(),
        Some(home.path()),
        &[("CARGO_HOME", cargo_home.to_str().unwrap())],
    );
    let output = Command::new(bin())
        .arg("report")
        .arg("--view")
        .arg("external")
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("CARGO_HOME", &cargo_home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run report --view external");
    assert!(
        output.status.success(),
        "report --view external failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["view"], "external");
    let units = json["result"]["units"].as_array().expect("units array");
    assert!(
        units
            .iter()
            .any(|u| u["detector_id"] == "cargo-home" && u["category"] == "installation"),
        "{units:?}"
    );
    assert!(json["result"]["total_bytes"].as_u64().unwrap() > 0);
}

/// An explicit root on the command line still takes the single-root
/// path unchanged (#42's "a single explicit root is just a scope of
/// one"): no `scope_coverage` key at all when nothing is ambiguous.
#[test]
fn report_with_an_explicit_root_stays_single_root_and_has_no_scope_coverage_key() {
    let root = tempfile::tempdir().unwrap();
    write_project(&root.path().join("proj"));
    let store = tempfile::tempdir().unwrap();

    let observe_status = Command::new(bin())
        .arg("observe")
        .arg(root.path())
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .status()
        .expect("run observe with an explicit root");
    assert!(observe_status.success());
    let output = Command::new(bin())
        .arg("report")
        .arg(root.path())
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run report with an explicit root");
    assert!(
        output.status.success(),
        "report failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        json.get("scope_coverage").is_none(),
        "an explicit single root must not carry scope-level coverage noise: {json}"
    );
}
