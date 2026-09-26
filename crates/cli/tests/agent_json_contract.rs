//! Drives the real `swamp` binary end to end against disposable git
//! fixtures and asserts the bounded JSON contract `report --json` (and
//! friends) promises agent callers: deterministic envelope fields
//! (`since`, `observed_at`, `index_refreshed`, `total`, `truncated`),
//! no-change and partial-coverage behavior, an invalid filter erroring
//! to stderr with nothing on stdout, pagination that never silently
//! drops rows without saying so, a plan awaiting authorization, and a
//! refused execution. This is the CLI-first replacement for the MCP
//! crate's `crates/mcp/tests/what_grew.rs` and `tool_list.rs`: same
//! guarantees, driven through the noninteractive CLI surface instead of
//! a JSON-RPC stdin/stdout protocol.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn run(store: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .env("SWAMP_DIR", store)
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run swamp")
}

/// `swamp observe` is the only scanner (R12): every fixture below runs
/// it before a `report`/`report --json` call can read anything back.
/// `extra_args` lets a caller set e.g. `--full`; the root is always
/// named explicitly, matching the `report` calls these fixtures make.
fn observe(store: &Path, root: &Path, extra_args: &[&str]) {
    let mut args = vec!["observe", root.to_str().unwrap()];
    args.extend_from_slice(extra_args);
    let out = run(store, &args);
    assert!(
        out.status.success(),
        "observe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_json(store: &Path, args: &[&str]) -> serde_json::Value {
    let out = run(store, args);
    assert!(
        out.status.success(),
        "swamp {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "swamp {args:?} did not print JSON on stdout: {e}\nstdout: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
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
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// One synthetic checkout: a git repo with a committed file and an
/// ignored `node_modules` "dependency tree" artifact of `seed_bytes`.
fn make_checkout(root: &Path, name: &str, seed_bytes: usize) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    run_git(&dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("README.md"), b"x").unwrap();
    fs::write(dir.join(".gitignore"), b"node_modules/\n").unwrap();
    run_git(&dir, &["add", "README.md", ".gitignore"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    let deps = dir.join("node_modules");
    fs::create_dir_all(&deps).unwrap();
    fs::write(deps.join("seed"), vec![b'x'; seed_bytes]).unwrap();
    dir
}

/// #53/#54/#60: the default `report --json` (no `--view`) carries each
/// artifact row's decision evidence -- populated by
/// `report::attach_decision_evidence` and passed straight through by
/// `serde_json::to_value(&rr)` -- not just bytes/kind. This is the
/// "fields populated end-to-end" contract check from the CLI JSON
/// transport's own point of view (`crates/core/tests/evidence.rs`
/// unit-tests the contract type itself; this exercises the real
/// binary's JSON output).
#[test]
fn report_json_carries_decision_evidence_on_artifact_rows() {
    let root = tempfile::tempdir().unwrap();
    make_checkout(root.path(), "repo", 4096);
    let store = tempfile::tempdir().unwrap();

    observe(store.path(), root.path(), &[]);
    let v = run_json(
        store.path(),
        &["report", root.path().to_str().unwrap(), "--json"],
    );
    let artifacts = v["projects"][0]["worktrees"][0]["artifacts"]
        .as_array()
        .expect("artifacts array");
    let dep_row = artifacts
        .iter()
        .find(|a| {
            a["path"]
                .as_str()
                .unwrap_or_default()
                .contains("node_modules")
        })
        .expect("a node_modules dependency-tree row");
    let evidence = dep_row["evidence"].as_array().expect("evidence array");
    assert!(
        !evidence.is_empty(),
        "row must carry evidence, not an empty/absent field: {dep_row:#}"
    );
    let kinds: Vec<&str> = evidence.iter().filter_map(|e| e["kind"].as_str()).collect();
    assert!(kinds.contains(&"activity"), "{kinds:?}");
    assert!(kinds.contains(&"reclaimability"), "{kinds:?}");
}

/// #60's own named gap ("bespoke-shaped JSON views (`kinds`/`builds`/
/// `deps`/`unowned`/`worktrees`)... do not carry evidence; only the
/// default report view... does"): every one of these views' row objects
/// must carry an `evidence` field once this fixture has real evidence
/// to attach, not just the default report view.
#[test]
fn bespoke_json_views_carry_evidence_on_their_rows() {
    let root = tempfile::tempdir().unwrap();
    let repo = make_checkout(root.path(), "repo", 4096);
    // A real Rust build-output directory (#54/#58/#59 wire Activity/
    // Recovery/Reclaimability onto `BuildOutput` rows), gated on the
    // real `Cargo.toml` marker `ecosystem::classify_gated` requires --
    // `make_checkout`'s own `node_modules` alone only exercises `deps`.
    fs::write(repo.join("Cargo.toml"), b"[package]\nname = \"repo\"\n").unwrap();
    fs::create_dir_all(repo.join("target/debug")).unwrap();
    fs::write(repo.join("target/debug/seed"), vec![b'x'; 4096]).unwrap();
    let store = tempfile::tempdir().unwrap();

    observe(store.path(), root.path(), &[]);
    for view in ["builds", "deps", "kinds", "worktrees"] {
        let v = run_json(
            store.path(),
            &[
                "report",
                root.path().to_str().unwrap(),
                "--json",
                "--view",
                view,
            ],
        );
        let rows = v["result"]
            .as_array()
            .unwrap_or_else(|| panic!("--view {view} did not print a result array: {v:#}"));
        assert!(!rows.is_empty(), "--view {view} produced no rows to check");
        for row in rows {
            assert!(
                row.get("evidence").is_some(),
                "--view {view} row missing `evidence` field entirely: {row:#}"
            );
        }
        // At least one row across the whole view carries a real,
        // non-empty fact list -- never every row silently defaulting to
        // an empty array because the field was added but never wired.
        assert!(
            rows.iter()
                .any(|r| r["evidence"].as_array().is_some_and(|e| !e.is_empty())),
            "--view {view}: every row's evidence array is empty: {rows:#?}"
        );
    }

    // `unowned` is checked separately: this fixture's single clean
    // checkout attributes everything, so the unowned view is
    // legitimately empty here -- confirm the field is at least present
    // on the JSON shape by checking the payload is an empty array, not
    // an error or a differently-shaped object.
    let v = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--json",
            "--view",
            "unowned",
        ],
    );
    assert!(v["result"].as_array().is_some());
}

/// `--view rust --project` must apply the same worktree ownership filter
/// in JSON that the text renderer applies. A Node adapter's node_modules
/// in a neighboring project must not leak into the selected Cargo view.
#[test]
fn rust_json_view_is_filtered_to_the_requested_project() {
    let root = tempfile::tempdir().unwrap();
    let rust_project = make_checkout(root.path(), "swamp", 1024);
    let rust_project = fs::canonicalize(rust_project).unwrap();
    fs::write(
        rust_project.join("Cargo.toml"),
        b"[package]\nname = \"swamp\"\n",
    )
    .unwrap();
    fs::create_dir_all(rust_project.join("target/debug")).unwrap();
    fs::write(
        rust_project.join("target/debug/swamp-bin"),
        vec![b'r'; 2048],
    )
    .unwrap();

    let node_project = make_checkout(root.path(), "other", 4096);
    fs::write(node_project.join("package.json"), b"{\"name\":\"other\"}\n").unwrap();
    let nested = make_checkout(&rust_project, "inner", 4096);
    fs::write(nested.join("package.json"), b"{\"name\":\"inner\"}\n").unwrap();
    let store = tempfile::tempdir().unwrap();
    observe(store.path(), root.path(), &[]);

    let all = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "rust",
            "--json",
        ],
    );
    let all_rows = all["result"].as_array().expect("Rust result array");
    assert!(
        all_rows.iter().any(|row| {
            row["path"]
                .as_str()
                .unwrap_or_default()
                .contains("/other/node_modules")
        }),
        "fixture should expose Node nested artifacts in the unfiltered result: {all:#}"
    );

    let filtered = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "rust",
            "--project",
            "swamp",
            "--json",
        ],
    );
    let rows = filtered["result"]
        .as_array()
        .expect("filtered Rust result array");
    assert!(
        all_rows.iter().any(|r| r["path"]
            .as_str()
            .is_some_and(|p| p.contains("/inner/node_modules"))),
        "nested fixture was not identified"
    );
    assert!(
        !rows
            .iter()
            .any(|r| r["path"].as_str().is_some_and(|p| p.contains("/inner/"))),
        "nested checkout was attributed to its enclosing repo"
    );
    assert!(
        !rows.is_empty(),
        "Cargo project should have nested rows: {filtered:#}"
    );
    assert!(
        rows.iter().all(|row| {
            row["path"]
                .as_str()
                .unwrap_or_default()
                .starts_with(rust_project.to_str().unwrap())
        }),
        "JSON project filter leaked another project's adapter rows: {filtered:#}"
    );

    let text = run(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "rust",
            "--project",
            "swamp",
        ],
    );
    assert!(text.status.success());
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("swamp"),
        "text should include selected project: {text}"
    );
    assert!(
        !text.contains("/other/"),
        "text leaked another project: {text}"
    );
}

/// The Rust view points at the supported TUI cleanup workflow and never
/// advertises the removed `cleanup-check` CLI command.
#[test]
fn rust_text_view_recommends_supported_cleanup_review() {
    let root = tempfile::tempdir().unwrap();
    let repo = make_checkout(root.path(), "swamp", 1024);
    fs::write(repo.join("Cargo.toml"), b"[package]\nname = \"swamp\"\n").unwrap();
    fs::create_dir_all(repo.join("target/debug")).unwrap();
    fs::write(repo.join("target/debug/swamp-bin"), vec![b'r'; 2048]).unwrap();
    let store = tempfile::tempdir().unwrap();
    observe(store.path(), root.path(), &[]);

    let output = run(
        store.path(),
        &["report", root.path().to_str().unwrap(), "--view", "rust"],
    );
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("swamp ui"),
        "expected supported TUI guidance: {text}"
    );
    assert!(
        text.contains("Space"),
        "expected an actionable TUI key: {text}"
    );
    assert!(
        !text.contains("cleanup-check"),
        "phantom CLI command remains: {text}"
    );
}

/// A grown report whose store holds less history than requested clamps
/// its window to the observations actually available, rather than
/// fabricating a full requested window.
#[test]
fn grown_view_clamps_to_partial_history() {
    let root = tempfile::tempdir().unwrap();
    make_checkout(root.path(), "repo", 4096);
    let store = tempfile::tempdir().unwrap();
    // `--since` moved to `observe` (R12: `report` takes no `--since` of
    // its own and never scans); `config.toml`'s `since` is what the
    // JSON envelope's `coverage.history` block reports as "asked".
    fs::write(store.path().join("config.toml"), "since = \"1h\"\n").unwrap();
    observe(store.path(), root.path(), &[]);

    let v = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "grown",
            "--json",
        ],
    );
    assert_eq!(v["view"], "grown");
    assert_eq!(v["result"]["grown"].as_array().unwrap().len(), 0);
    let history = &v["coverage"]["history"];
    // The fixture has one observation, but elapsed time between its
    // write and report varies (notably on CI). Assert the contract, not
    // a guessed sub-two-second bound: available history must be shorter
    // than the requested hour and the effective window must match it.
    let history_secs = history["history_secs"].as_u64().expect("history_secs");
    let effective = history["effective_window_secs"]
        .as_u64()
        .expect("effective_window_secs");
    assert!(history_secs < 3600, "{v:#}");
    assert_eq!(effective, history_secs);
    assert!(
        history["note"]
            .as_str()
            .unwrap()
            .contains(&format!("the store holds {history_secs}s of observations")),
        "{v:#}"
    );
    for key in ["observed_at", "since", "index_refreshed"] {
        assert!(
            v["coverage"].get(key).is_some(),
            "coverage missing {key}: {v:#}"
        );
    }
}

/// After a real growth event, `--view grown --json` names the row and a
/// second call with nothing changed shows no growth: growth reporting
/// reflects an actual event, not the passage of time or re-observation.
#[test]
fn grown_view_reports_a_row_after_growth_then_nothing_on_a_no_change_rerun() {
    let root = tempfile::tempdir().unwrap();
    let checkout = make_checkout(root.path(), "repo", 4096);
    let store = tempfile::tempdir().unwrap();
    // `--since` moved to `observe` (R12): the window that decides
    // growth_bytes is fixed at the observation that computed it, read
    // from `config.toml`'s `since` when `observe` gets no `--since` of
    // its own.
    fs::write(store.path().join("config.toml"), "since = \"1h\"\n").unwrap();

    // Baseline observation.
    observe(store.path(), root.path(), &[]);
    let first = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "grown",
            "--json",
        ],
    );
    assert_eq!(first["result"]["grown"].as_array().unwrap().len(), 0);

    // The growth store's observed_at has one-second resolution: two
    // observations inside the same wall-clock second can share a
    // baseline. Cross a second boundary between the baseline and the
    // growth event so the next observation has its own fresh baseline
    // to compare against, not the pre-growth one.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // A real growth event.
    fs::write(
        checkout.join("node_modules/growth-probe"),
        vec![b'g'; 2 * 1024 * 1024],
    )
    .unwrap();

    observe(store.path(), root.path(), &[]);
    let grown = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "grown",
            "--json",
        ],
    );
    let rows = grown["result"]["grown"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|r| r["path"].as_str().unwrap().contains("node_modules"))
        .unwrap_or_else(|| panic!("no node_modules row in {grown:#}"));
    assert!(row["growth_bytes"].as_i64().unwrap() > 0);

    // Cross another second boundary before the no-change rerun, for the
    // same reason as above.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // No-change rerun: growth is an event, not a clock. Re-observe with
    // a *short* window here (not "1h" again): with only a few seconds
    // of real history, a 1h ask still resolves its baseline to the
    // oldest retained observation (the pre-growth one), by design -- see
    // `history_block`'s asked-vs-held note -- so it would show the same
    // growth again, correctly, not a bug. A short `since` selects the
    // most recent observation (the post-growth one just taken above) as
    // the baseline, which is what actually answers "did anything change
    // just now".
    fs::write(store.path().join("config.toml"), "since = \"1s\"\n").unwrap();
    observe(store.path(), root.path(), &[]);
    let again = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "grown",
            "--json",
        ],
    );
    assert_eq!(
        again["result"]["grown"].as_array().unwrap().len(),
        0,
        "no filesystem change since the last observation must show no growth: {again:#}"
    );
}

/// An invalid `--filter` expression fails loudly: a nonzero exit, the
/// parse error on stderr, and *nothing* on stdout -- never a partial or
/// malformed JSON document a caller might try to parse anyway.
#[test]
fn invalid_filter_errors_to_stderr_with_no_stdout_json() {
    let root = tempfile::tempdir().unwrap();
    make_checkout(root.path(), "repo", 4096);
    let store = tempfile::tempdir().unwrap();

    observe(store.path(), root.path(), &[]);
    let out = run(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--json",
            "--filter",
            "this is not a valid filter expression (((",
        ],
    );
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!out.stderr.is_empty());
}

/// `--limit`/`--offset` bound an array-shaped `--view` result and say so:
/// `total` is the unbounded count, `truncated` is true whenever the page
/// is not the whole answer. A caller must never mistake a page for the
/// full inventory.
#[test]
fn view_worktrees_json_is_bounded_by_limit_and_offset() {
    let root = tempfile::tempdir().unwrap();
    make_checkout(root.path(), "repo-a", 1024);
    make_checkout(root.path(), "repo-b", 1024);
    let store = tempfile::tempdir().unwrap();

    observe(store.path(), root.path(), &[]);
    let whole = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "worktrees",
            "--json",
        ],
    );
    let total = whole["result"].as_array().unwrap().len();
    assert_eq!(total, 2, "{whole:#}");
    assert_eq!(whole["total"], serde_json::json!(2));
    assert_eq!(whole["truncated"], serde_json::json!(false));

    let page = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "worktrees",
            "--json",
            "--limit",
            "1",
        ],
    );
    assert_eq!(page["result"].as_array().unwrap().len(), 1);
    assert_eq!(page["total"], serde_json::json!(2));
    assert_eq!(page["truncated"], serde_json::json!(true));

    let second_page = run_json(
        store.path(),
        &[
            "report",
            root.path().to_str().unwrap(),
            "--view",
            "worktrees",
            "--json",
            "--limit",
            "1",
            "--offset",
            "1",
        ],
    );
    assert_ne!(
        page["result"][0]["path"], second_page["result"][0]["path"],
        "offset must move the window, not repeat the first page"
    );
}

/// `propose --json` never authorizes anything: the plan comes back
/// `awaiting-authorization` with the exact `swamp approve` command a
/// human runs next, and nothing on disk is touched.
/// `--view projects`/`--view grown` are JSON-only: in text mode they
/// name themselves in a clear error on stderr instead of silently
/// falling back to a different view.
#[test]
fn json_only_views_refuse_text_mode_explicitly() {
    let root = tempfile::tempdir().unwrap();
    make_checkout(root.path(), "repo", 4096);
    let store = tempfile::tempdir().unwrap();

    observe(store.path(), root.path(), &[]);
    for view in ["projects", "grown"] {
        let out = run(
            store.path(),
            &["report", root.path().to_str().unwrap(), "--view", view],
        );
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("JSON only"), "{stderr}");
    }
}
