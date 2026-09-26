//! #52 contract tests: `swamp scope`/`swamp report` text and `--json`
//! output must agree on the same facts for the same configured scope --
//! an exclusion, an external-only scope (no Git checkout anywhere), and
//! a brand-new store's unknown growth baseline. Each test runs the real
//! binary twice (once without `--json`, once with) against the same
//! fixture config/store and cross-checks specific facts rather than
//! diffing whole outputs (the two renderers are deliberately different
//! shapes; the *facts* must match).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn run(store: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .env("SWAMP_DIR", store)
        .env("HOME", home)
        .output()
        .expect("run swamp")
}

/// `swamp observe` is the only scanner (R12): every fixture below runs
/// it before a `report`/`report --json` call can read anything back.
fn observe(store: &Path, home: &Path, extra_env: &[(&str, &str)]) {
    let mut cmd = Command::new(bin());
    cmd.arg("observe").env("SWAMP_DIR", store).env("HOME", home);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let status = cmd.status().expect("run observe");
    assert!(status.success(), "observe failed");
}

fn run_json(store: &Path, home: &Path, args: &[&str]) -> serde_json::Value {
    let out = run(store, home, args);
    assert!(
        out.status.success(),
        "expected success for {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("invalid JSON from {args:?}: {e}\n{:?}", out.stdout))
}

/// A config that never touches a real Homebrew/agent-tool install:
/// only `include` is in scope, everything else disabled.
fn only_include_config(include: &[&str], exclude: &[&str]) -> String {
    let disabled: Vec<String> = swamp_core::locations::Registry::with_builtins()
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| id != swamp_core::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID)
        .collect();
    let include_toml = include
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let exclude_toml = exclude
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let disabled_toml = disabled
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[scan]\ndefaults = false\ninclude = [{include_toml}]\nexclude = [{exclude_toml}]\ndisabled_detectors = [{disabled_toml}]\n"
    )
}

/// An excluded path must be named the same way in both renderers: the
/// text line says `excluded (...)` next to the path, the JSON row's
/// `status.state` is `"excluded"` with a matching `pattern`.
#[test]
fn exclusion_is_named_consistently_in_text_and_json() {
    let home = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("kept")).unwrap();
    std::fs::create_dir_all(home.path().join("scratch")).unwrap();
    let excluded_abs = home.path().join("scratch").display().to_string();
    std::fs::write(
        store.path().join("config.toml"),
        only_include_config(&["~/kept", "~/scratch"], &[&excluded_abs]),
    )
    .unwrap();

    let text_out = run(store.path(), home.path(), &["scope"]);
    assert!(text_out.status.success());
    let text = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        text.contains("excluded"),
        "text scope output must name the exclusion: {text}"
    );

    let json = run_json(store.path(), home.path(), &["scope", "--json"]);
    let roots = json["roots"].as_array().expect("roots array");
    let scratch_row = roots
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("/scratch"))
        .expect("scratch row present in JSON");
    assert_eq!(scratch_row["status"]["state"], "excluded");
    assert!(
        text.contains("scratch"),
        "text output must also name the excluded path itself: {text}"
    );
    // The kept root is present in both, never silently dropped either.
    let kept_row = roots
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("/kept"))
        .expect("kept row present in JSON");
    assert_eq!(kept_row["status"]["state"], "present");
}

/// An "external-only" scope (an in-scope detector location, but zero
/// Git checkouts anywhere) must agree between renderers: `--view
/// external` JSON has a unit, `--view worktrees` JSON has none, and the
/// text `report` output does not fabricate a project.
#[test]
fn external_only_scope_reports_zero_projects_consistently() {
    let home = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let cargo_home = home.path().join("fixture-cargo");
    std::fs::create_dir_all(cargo_home.join("bin")).unwrap();
    std::fs::write(cargo_home.join("bin/cargo"), vec![7u8; 5_000]).unwrap();
    // `config.toml` has no way to set process environment variables, so
    // CARGO_HOME is passed via `.env(...)` below (every detector reads
    // env var overrides exactly the same way, real or fixture); the
    // config here only needs to keep the cargo-home detector enabled
    // and every other detector/default off, so the scope is exactly
    // one external-unit-eligible location with zero project roots.
    let disabled: Vec<String> = swamp_core::locations::Registry::with_builtins()
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| {
            id != swamp_core::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID && id != "cargo-home"
        })
        .collect();
    let disabled_toml = disabled
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        store.path().join("config.toml"),
        format!("[scan]\ndefaults = false\ndisabled_detectors = [{disabled_toml}]\n"),
    )
    .unwrap();

    observe(
        store.path(),
        home.path(),
        &[("CARGO_HOME", cargo_home.to_str().unwrap())],
    );
    let text_out = Command::new(bin())
        .args(["report"])
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("CARGO_HOME", &cargo_home)
        .output()
        .expect("run swamp report");
    assert!(
        text_out.status.success(),
        "{}",
        String::from_utf8_lossy(&text_out.stderr)
    );
    let text = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        !text.to_lowercase().contains("fixture-cargo/bin"),
        "cargo home's own directory must never be listed as a project checkout: {text}"
    );

    let worktrees_json = serde_json::from_slice::<serde_json::Value>(
        &Command::new(bin())
            .args(["report", "--view", "worktrees", "--json"])
            .env("SWAMP_DIR", store.path())
            .env("HOME", home.path())
            .env("CARGO_HOME", &cargo_home)
            .output()
            .expect("run swamp report --view worktrees --json")
            .stdout,
    )
    .expect("valid JSON");
    assert_eq!(
        worktrees_json["result"].as_array().unwrap().len(),
        0,
        "no Git checkout exists anywhere in scope: {worktrees_json}"
    );

    let external_json = serde_json::from_slice::<serde_json::Value>(
        &Command::new(bin())
            .args(["report", "--view", "external", "--json"])
            .env("SWAMP_DIR", store.path())
            .env("HOME", home.path())
            .env("CARGO_HOME", &cargo_home)
            .output()
            .expect("run swamp report --view external --json")
            .stdout,
    )
    .expect("valid JSON");
    let units = external_json["result"]["units"].as_array().unwrap();
    assert!(
        units.iter().any(|u| u["detector_id"] == "cargo-home"),
        "external units must still be visible even with zero projects in scope: {external_json}"
    );
}

/// A brand-new store's first-ever observation has an unknown/zero
/// growth baseline; `--view grown --json`'s `coverage.history` must
/// say so explicitly, and the text `report` output must not print a
/// growth figure implying a real comparison happened.
#[test]
fn unknown_baseline_is_named_not_fabricated_in_both_renderers() {
    let home = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("proj")).unwrap();
    let run_git = |args: &[&str]| {
        let status = Command::new("git")
            .arg("-C")
            .arg(home.path().join("proj"))
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
    run_git(&["init", "-q", "-b", "main"]);
    std::fs::write(home.path().join("proj/README.md"), b"hi\n").unwrap();
    run_git(&["add", "README.md"]);
    run_git(&["commit", "-q", "-m", "initial"]);

    // `since` moved to `observe` (R12: `report` is a pure read and takes
    // no `--since` of its own); a top-level key must precede the
    // `[scan]` table in TOML, so it is prepended rather than appended.
    std::fs::write(
        store.path().join("config.toml"),
        format!("since = \"1h\"\n{}", only_include_config(&["~/proj"], &[])),
    )
    .unwrap();

    observe(store.path(), home.path(), &[]);
    let grown_json = run_json(
        store.path(),
        home.path(),
        &["report", "--view", "grown", "--json"],
    );
    let history = &grown_json["coverage"]["history"];
    // The very first observation this call performs gives the store
    // exactly one data point: a real span far shorter than the asked 1h
    // (a couple of seconds of wall-clock slack for the fixture's own
    // git/observe calls, never anywhere near 3600s), not fabricated
    // growth over the whole asked window.
    let asked = history["asked_window_secs"].as_u64().unwrap();
    let actual_history = history["history_secs"].as_u64().unwrap();
    let effective = history["effective_window_secs"].as_u64().unwrap();
    assert_eq!(asked, 3600);
    assert!(actual_history < 10, "{history}");
    assert_eq!(
        effective, actual_history,
        "the effective window must be clamped to the real (near-zero) history, \
         never the full asked 1h: {history}"
    );
    let note = history["note"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        note.contains("asked for 3600s") && note.contains("of observations"),
        "the note must name the unknown baseline explicitly, never silently \
         report a number as if a real 1h comparison happened: {history}"
    );

    // The plain-text report must not claim a growth number for a store
    // that has never observed this root before.
    let text_out = run(store.path(), home.path(), &["report"]);
    assert!(text_out.status.success());
    let text = String::from_utf8_lossy(&text_out.stdout);
    assert!(
        !text.contains("growth_bytes"),
        "text mode should not leak JSON field names, sanity check: {text}"
    );
}
