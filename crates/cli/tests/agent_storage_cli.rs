//! #100/#101 end-to-end: `swamp report --view agents` and `swamp
//! protect`, read-only, through the real built binary. Every fixture
//! Claude Code home here is synthetic (PRIVACY IS A HARD RULE): no real
//! `~/.claude` is read.

use std::path::PathBuf;
use std::process::Command;

/// Fixture scopes are **allow-lists**, never deny-lists.
///
/// `core-simulator`, `homebrew` and `ruby-install` resolve absolute
/// system paths (`/Library/Developer/CoreSimulator/Volumes`,
/// `/opt/homebrew`, `/opt/rubies`) that no injected `HOME` can confine,
/// so a deny-list of two or three ids leaves the developer's real disk
/// in a test fixture's scope: this file's `report` runs took ~1m34s each
/// and read tens of gigabytes of real storage. The core tests already
/// learned this
/// (`crates/core/tests/external_units.rs::a_detector_that_escapes_the_fixture_home_is_named_here_not_discovered_by_a_byte_total`);
/// `a_fixture_scope_reaches_nothing_outside_its_fixture_home` below is
/// this file's copy of the guard.
const CLAUDE_ONLY_SCOPE: &str = "[scan]\ndefaults = false\nenabled_detectors = [\"claude-code\"]\n";
const CARGO_ONLY_SCOPE: &str = "[scan]\ndefaults = false\nenabled_detectors = [\"cargo-home\"]\n";

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn write(path: &std::path::Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// `swamp observe` is the only scanner (R12): every fixture below runs
/// it before a `report --json` call can read anything back.
fn observe(store: &std::path::Path, home: &std::path::Path, claude_home: &std::path::Path) {
    let out = Command::new(bin())
        .arg("observe")
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", claude_home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run observe");
    assert!(
        out.status.success(),
        "observe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn session_line(cwd: &std::path::Path) -> String {
    format!(
        "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\"}}\n",
        cwd.display()
    )
}

/// Builds a fixture Claude Code home with one repo-linked session plus a
/// `shell-snapshots` cache directory. Returns `(home_root, claude_home, repo)`.
fn fixture(home_root: &std::path::Path) -> (PathBuf, PathBuf) {
    let claude_home = home_root.join("claude-home");
    let repo = home_root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let session_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    write(
        &claude_home
            .join("projects")
            .join("-repo-encoded")
            .join(format!("{session_id}.jsonl")),
        session_line(&repo).as_bytes(),
    );
    write(
        &claude_home.join("shell-snapshots").join("snap.sh"),
        b"alias x=y",
    );
    write(&claude_home.join("settings.json"), b"{}");
    (claude_home, repo)
}

#[test]
fn report_view_agents_lists_a_claude_code_session_with_project_linkage() {
    let home = tempfile::tempdir().unwrap();
    let (claude_home, repo) = fixture(home.path());
    let store = tempfile::tempdir().unwrap();
    std::fs::write(store.path().join("config.toml"), CLAUDE_ONLY_SCOPE).unwrap();

    observe(store.path(), home.path(), &claude_home);
    let output = Command::new(bin())
        .arg("report")
        .arg("--view")
        .arg("agents")
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", &claude_home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run report --view agents");
    assert!(
        output.status.success(),
        "report --view agents failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["view"], "agents");
    let units = json["result"]["units"].as_array().expect("units array");
    let session = units
        .iter()
        .find(|u| u["category"] == "sessions")
        .expect("session unit present");
    assert_eq!(session["project_link"]["state"], "linked");
    assert_eq!(
        session["project_link"]["project_path"],
        repo.display().to_string()
    );
    // Redaction: no prompt/message content field exists anywhere in the output.
    let text = serde_json::to_string(&json).unwrap();
    assert!(!text.contains("\"message\""));
}

#[test]
fn report_view_agents_project_filter_narrows_to_linked_units() {
    let home = tempfile::tempdir().unwrap();
    let (claude_home, _repo) = fixture(home.path());
    let store = tempfile::tempdir().unwrap();
    std::fs::write(store.path().join("config.toml"), CLAUDE_ONLY_SCOPE).unwrap();

    observe(store.path(), home.path(), &claude_home);
    let output = Command::new(bin())
        .arg("report")
        .arg("--view")
        .arg("agents")
        .arg("--project")
        .arg("no-such-project")
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", &claude_home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run report --view agents --project");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let units = json["result"]["units"].as_array().expect("units array");
    assert!(
        units.is_empty(),
        "a project filter matching nothing must narrow to zero units: {units:?}"
    );
}

#[test]
fn protect_add_list_remove_round_trip_through_the_binary() {
    let home = tempfile::tempdir().unwrap();
    let (claude_home, _repo) = fixture(home.path());
    let store = tempfile::tempdir().unwrap();
    let target = claude_home.join("shell-snapshots");

    let add = Command::new(bin())
        .arg("protect")
        .arg("add")
        .arg(&target)
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .output()
        .expect("run protect add");
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );

    let list = Command::new(bin())
        .arg("protect")
        .arg("list")
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .output()
        .expect("run protect list");
    assert!(list.status.success());
    let json: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    let paths: Vec<String> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(paths.contains(&target.display().to_string()), "{paths:?}");

    let remove = Command::new(bin())
        .arg("protect")
        .arg("remove")
        .arg(&target)
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .output()
        .expect("run protect remove");
    assert!(remove.status.success());

    let list2 = Command::new(bin())
        .arg("protect")
        .arg("list")
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .output()
        .expect("run protect list again");
    let json2: serde_json::Value = serde_json::from_slice(&list2.stdout).unwrap();
    assert!(json2.as_array().unwrap().is_empty());
}

/// #100: `report --project NAME --json` (no `--view`) includes this
/// project's own linked agent-storage units, not only when `--view
/// agents` is also passed.
#[test]
fn report_project_json_without_view_includes_linked_agent_storage() {
    let home = tempfile::tempdir().unwrap();
    let (claude_home, repo) = fixture(home.path());
    let store = tempfile::tempdir().unwrap();
    std::fs::write(store.path().join("config.toml"), CLAUDE_ONLY_SCOPE).unwrap();
    let project_name = repo.file_name().unwrap().to_str().unwrap().to_string();

    let observe_status = Command::new(bin())
        .arg("observe")
        .arg(home.path())
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", &claude_home)
        .env("SWAMP_TEST_MODE", "1")
        .status()
        .expect("run observe against a real root");
    assert!(observe_status.success());
    let output = Command::new(bin())
        .arg("report")
        .arg(home.path())
        .arg("--project")
        .arg(&project_name)
        .arg("--json")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("CLAUDE_CONFIG_DIR", &claude_home)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run report --project --json against a real root");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        json.get("agent_storage").is_some(),
        "project-scoped JSON must include agent_storage: {json}"
    );
    assert!(
        !json["agent_storage"]["units"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the fixture session is linked to this project, so at least one unit must be present: {json}"
    );
}

/// This file's copy of the core suite's escaping-detector guard: a
/// fixture scope must reach nothing outside its fixture home.
///
/// The core tests catch a *detector* that escapes; this catches a
/// *fixture* that lets one in, which is the failure this file actually
/// had. It runs the real binary with the same scope constant the report
/// tests use and fails on any resolved root outside the tempdir --
/// including the absolute system paths (`/opt/homebrew`,
/// `/Library/Developer/CoreSimulator/Volumes`, `/opt/rubies`) that a
/// deny-list of two or three detector ids leaves in scope.
#[test]
fn a_fixture_scope_reaches_nothing_outside_its_fixture_home() {
    for scope in [CLAUDE_ONLY_SCOPE, CARGO_ONLY_SCOPE] {
        let home = tempfile::tempdir().unwrap();
        let (claude_home, _repo) = fixture(home.path());
        let store = tempfile::tempdir().unwrap();
        std::fs::write(store.path().join("config.toml"), scope).unwrap();
        let output = Command::new(bin())
            .arg("scope")
            .arg("--json")
            .env("SWAMP_DIR", store.path())
            .env("HOME", home.path())
            .env("CLAUDE_CONFIG_DIR", &claude_home)
            .env("CARGO_HOME", home.path().join("fixture-cargo"))
            .env("SWAMP_TEST_MODE", "1")
            .output()
            .expect("run scope --json");
        assert!(
            output.status.success(),
            "scope --json failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let home_prefix = std::fs::canonicalize(home.path())
            .unwrap()
            .display()
            .to_string();
        let escaping: Vec<String> = json["roots"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|r| r["status"]["state"] == "present")
            .filter_map(|r| r["path"].as_str().map(str::to_string))
            .filter(|p| {
                !p.starts_with(&home_prefix) && !p.starts_with(home.path().to_str().unwrap())
            })
            .collect();
        assert!(
            escaping.is_empty(),
            "a fixture scope resolved roots outside its fixture home, so this test reads real \
             user data: {escaping:?} (scope was {scope:?})"
        );
    }
}
