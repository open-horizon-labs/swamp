//! End-to-end `swamp observe` checks against the built binary:
//! it writes the growth store and prints one machine-readable line per
//! root.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
}

fn write_git_project(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
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
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), b"real project fixture\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-q", "-m", "initial"]);
}

#[test]
fn observe_on_a_fixture_root_writes_the_store_and_prints_the_line() {
    let root = tempfile::tempdir().expect("root");
    std::fs::write(root.path().join("hello.txt"), b"hi").unwrap();
    let store = tempfile::tempdir().expect("store");

    let output = Command::new(bin())
        .arg("observe")
        .arg(root.path())
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run observe");

    assert!(
        output.status.success(),
        "observe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("observed_at=") && stdout.contains("mode=full"),
        "stdout did not contain a machine-readable observe line: {stdout}"
    );

    // The volume-keyed growth store must now exist under SWAMP_DIR.
    let has_store = std::fs::read_dir(store.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.path().is_dir());
    assert!(
        has_store,
        "observe must persist a volume dir under the store"
    );

    let last_run = swamp_core::schedule::read_last_run(store.path())
        .expect("observe must persist scheduled_runs.parquet");
    assert_eq!(last_run.outcome, "ok");
}

#[test]
fn successful_narrow_observe_cleans_retired_store_state_once_and_keeps_other_roots() {
    let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    for (i, root) in roots.iter().enumerate() {
        std::fs::write(root.path().join("hello.txt"), format!("root {i}")).unwrap();
    }
    let store = tempfile::tempdir().unwrap();
    let root_paths: Vec<_> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r.path()).unwrap())
        .collect();
    // A volume-keyed store generation with compatible state for another
    // root remains owned by the store when this invocation names only
    // root 0. Use a numeric volume directory to match the on-disk layout.
    let other_volume = store.path().join(u64::MAX.to_string());
    assert!(
        root_paths
            .iter()
            .all(|root| { swamp_core::growth::root_scoped_volume_id(root) != u64::MAX })
    );

    let initial = Command::new(bin())
        .arg("observe")
        .args(&root_paths)
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("initial two-root observe");
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );

    // Keep a valid protection entry and config alongside current Parquet
    // facts, then simulate an older store format marker.
    std::fs::write(root_paths[0].join("keep.txt"), b"keep").unwrap();
    let protect = Command::new(bin())
        .args(["protect", "add"])
        .arg(root_paths[0].join("keep.txt"))
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("add retained protection");
    assert!(
        protect.status.success(),
        "{}",
        String::from_utf8_lossy(&protect.stderr)
    );
    std::fs::write(
        store.path().join("config.toml"),
        "# retained user configuration\n",
    )
    .unwrap();
    let retained = ["config.toml", "protect.parquet"]
        .map(|name| (name, std::fs::read(store.path().join(name)).unwrap()));
    std::fs::create_dir(&other_volume).unwrap();
    std::fs::write(
        other_volume.join("report_rows.parquet"),
        b"obsolete report cache",
    )
    .unwrap();
    std::fs::write(
        other_volume.join("summary.parquet"),
        b"obsolete derived view",
    )
    .unwrap();

    let previous_scope_report = Command::new(bin())
        .arg("report")
        .args(&root_paths)
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("read compatible prior scope history");
    assert!(
        previous_scope_report.status.success(),
        "compatible previous scope was lost: {}",
        String::from_utf8_lossy(&previous_scope_report.stderr)
    );

    std::fs::write(
        other_volume.join("enrich.parquet"),
        b"active enrichment marker",
    )
    .unwrap();
    std::fs::write(
        store.path().join("ledger.parquet"),
        b"preserve ledger state",
    )
    .unwrap();

    for name in [
        "unowned.json",
        "fsevents.json",
        "topology.json",
        "docker_facts.json",
        "scope.json",
        "external_consumers.json",
        "toolchain_declarations_cache.json",
        "dependency_identities_cache.json",
        "ledger.jsonl",
    ] {
        std::fs::write(other_volume.join(name), b"retired").unwrap();
    }
    for name in ["last_run.json", "grants.json", "ledger.jsonl"] {
        std::fs::write(store.path().join(name), b"retired").unwrap();
    }
    for name in [
        "last_report-0123456789abcdef.json",
        "last_report-0123456789abcdef.json.zst",
    ] {
        std::fs::write(store.path().join(name), b"retired").unwrap();
    }
    let plans = store.path().join("plans");
    std::fs::create_dir(&plans).unwrap();
    std::fs::write(
        plans.join("01234567-89ab-cdef-0123-456789abcdef.json"),
        b"retired plan",
    )
    .unwrap();
    std::fs::write(
        store.path().join("agent_protect.json"),
        b"unknown human intent",
    )
    .unwrap();
    std::fs::write(store.path().join("housekeeping.version"), b"0\n").unwrap();

    // A narrow-root observation must not treat the other root as orphaned.
    let upgraded = Command::new(bin())
        .arg("observe")
        .arg(&root_paths[0])
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("narrow upgrade observe");
    assert!(
        upgraded.status.success(),
        "{}",
        String::from_utf8_lossy(&upgraded.stderr)
    );
    let upgrade_output = String::from_utf8_lossy(&upgraded.stdout);
    assert!(
        upgrade_output.contains("mode=full"),
        "incompatible generation must force a fresh scan: {upgrade_output}"
    );
    for name in [
        "unowned.json",
        "fsevents.json",
        "topology.json",
        "docker_facts.json",
        "scope.json",
        "external_consumers.json",
        "toolchain_declarations_cache.json",
        "dependency_identities_cache.json",
        "ledger.jsonl",
    ] {
        assert!(!other_volume.join(name).exists());
    }
    for name in ["last_run.json", "grants.json", "ledger.jsonl"] {
        assert!(!store.path().join(name).exists());
    }
    assert!(
        !store
            .path()
            .join("last_report-0123456789abcdef.json")
            .exists()
    );
    assert!(
        !store
            .path()
            .join("last_report-0123456789abcdef.json.zst")
            .exists()
    );
    assert!(
        !plans
            .join("01234567-89ab-cdef-0123-456789abcdef.json")
            .exists()
    );
    for name in ["report_rows.parquet", "summary.parquet"] {
        assert!(
            !other_volume.join(name).exists(),
            "obsolete table survived: {name}"
        );
    }
    assert_eq!(
        std::fs::read(store.path().join("ledger.parquet")).unwrap(),
        b"preserve ledger state"
    );
    assert_eq!(
        std::fs::read(store.path().join("agent_protect.json")).unwrap(),
        b"unknown human intent"
    );
    assert_eq!(
        std::fs::read(other_volume.join("enrich.parquet")).unwrap(),
        b"active enrichment marker"
    );
    for (name, bytes) in retained {
        assert_eq!(std::fs::read(store.path().join(name)).unwrap(), bytes);
    }
    assert!(store.path().join("notes.parquet").is_file());
    assert_eq!(
        std::fs::read_to_string(store.path().join("housekeeping.version")).unwrap(),
        "3\n"
    );

    let report = Command::new(bin())
        .arg("report")
        .arg("--json")
        .arg(&root_paths[0])
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("report after automatic generation reset");
    assert!(
        report.status.success(),
        "report failed after automatic reset: {}",
        String::from_utf8_lossy(&report.stderr)
    );

    let repeated = Command::new(bin())
        .arg("observe")
        .arg(&root_paths[0])
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .env("SWAMP_FSEVENTS_MIN_INTERVAL_SECS", "0")
        .output()
        .expect("repeat observe");
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let output = String::from_utf8_lossy(&repeated.stdout);
    #[cfg(target_os = "macos")]
    assert!(
        output.contains("mode=incremental"),
        "repeat observe was not incremental: {output}"
    );
    #[cfg(target_os = "linux")]
    assert!(
        output.contains("mode=full") && output.contains("no_persisted_change_history"),
        "Linux without a collector should use its documented full-scan fallback: {output}"
    );
}

#[test]
fn missing_root_does_not_run_store_housekeeping() {
    let missing_parent = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::write(store.path().join("housekeeping.version"), b"0\n").unwrap();
    std::fs::write(store.path().join("last_run.json"), b"retired candidate").unwrap();

    let _ = Command::new(bin())
        .arg("observe")
        .arg(missing_parent.path().join("not-created"))
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("observe missing root");

    assert_eq!(
        std::fs::read(store.path().join("last_run.json")).unwrap(),
        b"retired candidate"
    );
    assert_eq!(
        std::fs::read_to_string(store.path().join("housekeeping.version")).unwrap(),
        "0\n"
    );
}

#[test]
fn current_generation_narrow_observe_keeps_other_explicit_root_history() {
    let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    for root in &roots {
        write_git_project(root.path());
    }
    let paths: Vec<_> = roots
        .iter()
        .map(|root| std::fs::canonicalize(root.path()).unwrap())
        .collect();
    let store = tempfile::tempdir().unwrap();
    let first = Command::new(bin())
        .arg("observe")
        .args(&paths)
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(store.path().join("housekeeping.version")).unwrap(),
        "3\n"
    );
    let other_volume = swamp_core::growth::volume_store_dir(store.path(), &paths[1]);
    let history = std::fs::read(other_volume.join("current.parquet")).unwrap();

    let narrow = Command::new(bin())
        .arg("observe")
        .arg(&paths[0])
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .unwrap();
    assert!(
        narrow.status.success(),
        "{}",
        String::from_utf8_lossy(&narrow.stderr)
    );
    assert_eq!(
        std::fs::read(other_volume.join("current.parquet")).unwrap(),
        history
    );

    let report = Command::new(bin())
        .arg("report")
        .arg("--json")
        .args(&paths)
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .unwrap();
    assert!(
        report.status.success(),
        "compatible two-root history was lost: {}",
        String::from_utf8_lossy(&report.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    let names: Vec<_> = json["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "both stored projects should remain reportable: {names:?}"
    );
}

/// A closed real pipe makes the first stdout write return EPIPE. Observe
/// has already saved its snapshot at that point, so the CLI must exit
/// cleanly and still finish recording the successful run.
#[cfg(unix)]
#[test]
fn observe_handles_a_closed_stdout_pipe_after_persisting() {
    use std::fs::File;
    use std::os::fd::FromRawFd;

    let root = tempfile::tempdir().expect("root");
    std::fs::write(root.path().join("hello.txt"), b"hi").unwrap();
    let store = tempfile::tempdir().expect("store");

    let mut pipe_fds = [0; 2];
    // SAFETY: `pipe_fds` points to two valid descriptors for libc to fill.
    assert_eq!(unsafe { libc::pipe(pipe_fds.as_mut_ptr()) }, 0);
    // Close the only reader before launching the child so its stdout
    // writes reliably receive EPIPE instead of depending on scheduling.
    // SAFETY: `pipe_fds[0]` is the live read descriptor created above.
    assert_eq!(unsafe { libc::close(pipe_fds[0]) }, 0);
    // SAFETY: ownership of the still-open write descriptor transfers to File.
    let stdout = unsafe { File::from_raw_fd(pipe_fds[1]) };

    let output = Command::new(bin())
        .arg("observe")
        .arg(root.path())
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::piped())
        .output()
        .expect("run observe with a closed stdout pipe");

    assert!(
        output.status.success(),
        "closed stdout should not fail observe or panic: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let last_run = swamp_core::schedule::read_last_run(store.path())
        .expect("successful observation should persist scheduled_runs.parquet");
    assert_eq!(last_run.outcome, "ok");
    assert!(
        std::fs::read_dir(store.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.path().is_dir()),
        "observe should persist its volume snapshot before handling EPIPE"
    );
}

/// R16 CI-red fix: `merge_root_report_into` prefixes every per-root note
/// with `"[{root}] "` before folding it into `Report.notes` (multi-root
/// disambiguation, long predating R12). `cmd_observe`'s one-line summary
/// (`crates/cli/src/schedule.rs`) reads that *merged* `notes` list for
/// its `mode=`/`reason=`/`changed_dirs=` fields with
/// `strip_prefix("fsevents: ")`, which only ever matched an unprefixed,
/// single-root `Report.notes` entry -- never the merged one, on any
/// platform, for any root. The shortcut this fails: reverting to
/// `strip_prefix` (matching only a note that starts with the marker)
/// silently falls back to the hardcoded "mode=full reason=no_store
/// changed_dirs=0" text on every call whose summary line this reads,
/// which happens to equal the correct text for a first-ever observation
/// -- so only a *second* observe on an existing store, which must show a
/// real reason (`too_soon`/`no_stored_event_id`/...), exposes it.
#[test]
fn observe_summary_line_reports_the_real_reason_not_the_no_store_fallback() {
    let root = tempfile::tempdir().expect("root");
    std::fs::write(root.path().join("hello.txt"), b"hi").unwrap();
    let store = tempfile::tempdir().expect("store");

    let run = || {
        let output = Command::new(bin())
            .arg("observe")
            .arg(root.path())
            .env("SWAMP_DIR", store.path())
            .env("SWAMP_TEST_MODE", "1")
            .output()
            .expect("run observe");
        assert!(
            output.status.success(),
            "observe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    // The specific "no anchor yet" reason code is per-platform
    // (`RefreshRefusal::reason_str`: FSEvents says `no_stored_event_id`;
    // the Linux persisted-change-log source says
    // `no_persisted_change_history`) -- either is a real reason, and
    // either is distinct from the generic fallback this test guards
    // against.
    let real_first_reason = |s: &str| {
        s.contains("reason=no_stored_event_id") || s.contains("reason=no_persisted_change_history")
    };

    let first = run();
    assert!(
        real_first_reason(&first),
        "first observation of a fresh store has no stored anchor: {first}"
    );

    let second = run();
    assert!(
        !second.contains("reason=no_store "),
        "a second observe against an existing store must not report the \
         generic no-store fallback (the merge's \"[{{root}}] \" prefix \
         must not defeat the summary line's reason lookup): {second}"
    );
    assert!(
        second.contains("reason=too_soon") || real_first_reason(&second),
        "second observe must report a real, specific reason: {second}"
    );
}

/// #41: `observe` with no explicit root resolves the configured scope
/// (shared with `report`/`ui`/`schedule` through
/// `swamp_core::scope::resolve_effective_scope`) instead of requiring an
/// explicit root.
///
/// Every non-builtin detector is disabled -- enumerated from the
/// registry, not listed by hand -- so the outcome depends only on the
/// fixture `HOME`'s layout. The hand-written list this used to carry
/// (`cargo-home`, `rustup`, `homebrew`) named the three detectors that
/// happened to fire on a Mac. On a GitHub Linux runner the ones that
/// fired instead were nvm (`NVM_DIR=/home/runner/.nvm`, an absolute
/// path no fixture `HOME` redirects) and Android
/// (`ANDROID_SDK_ROOT=/usr/local/lib/android/sdk`): the test walked
/// 2.6 GB of the runner's real SDK for 143 seconds and then failed an
/// assertion that had nothing to do with either. A detector list kept
/// in step with the registry by hand is a list that is wrong on the
/// next machine.
#[test]
fn observe_with_no_roots_uses_the_configured_default_scope() {
    let home = tempfile::tempdir().expect("home");
    std::fs::create_dir_all(home.path().join("src")).unwrap();
    std::fs::write(home.path().join("src/hello.txt"), b"hi").unwrap();
    let store = tempfile::tempdir().expect("store");
    let disabled = swamp_core::locations::Registry::with_builtins()
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| id != swamp_core::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID)
        .map(|id| format!("{id:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        store.path().join("config.toml"),
        format!("[scan]\ndisabled_detectors = [{disabled}]\n"),
    )
    .unwrap();

    let output = Command::new(bin())
        .arg("observe")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        // The Linux built-in defaults include `$XDG_CACHE_HOME`, which
        // an inherited environment would point at the *real* user's
        // cache -- a second real root, walked, in a test about the
        // fixture home. Redirecting it into the fixture (where it does
        // not exist, so it resolves Missing) keeps the test about what
        // it says it is about, on both platforms.
        .env("XDG_CACHE_HOME", home.path().join(".cache"))
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run observe with no roots");

    assert!(
        output.status.success(),
        "observe with no roots should resolve the configured default scope: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("/usr/") && !stderr.contains("/opt/"),
        "a scope resolved from a fixture HOME must not reach system paths: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&home.path().join("src").display().to_string()),
        "observe should have walked the configured ~/src default root: {stdout}"
    );

    // #41: a resolved scope is persisted so the next run can report
    // coverage changes; this is coverage bookkeeping, never byte
    // history.
    let scope_file = store.path().join("scope_roots.parquet");
    assert!(
        scope_file.exists(),
        "observe must persist the effective scope for next time"
    );
}

/// #41's "empty effective scope is explicit... never a silent fallback
/// to cwd or home": with defaults off and every detector disabled and
/// nothing configured under `include`, `observe` must fail with a
/// visible, nonzero-exit error -- never quietly scan the process's
/// current directory or the fixture `HOME` itself.
#[test]
fn observe_with_empty_scope_fails_visibly_never_falls_back_to_cwd() {
    let home = tempfile::tempdir().expect("home");
    let store = tempfile::tempdir().expect("store");
    // Every detector in the catalog (#45-#49 added many more since this
    // test was written) except `builtin-defaults`, which `defaults =
    // false` above already disables -- derived here from the same
    // registry the binary itself uses (never a hand-maintained literal
    // list), so this test does not go stale every time a detector is
    // added.
    let disabled: Vec<String> = swamp_core::locations::Registry::with_builtins()
        .detectors()
        .iter()
        .map(|d| d.id().to_string())
        .filter(|id| id != swamp_core::locations::builtin::BUILTIN_DEFAULTS_DETECTOR_ID)
        .collect();
    let disabled_toml = disabled
        .iter()
        .map(|id| format!("{id:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        store.path().join("config.toml"),
        format!("[scan]\ndefaults = false\ndisabled_detectors = [{disabled_toml}]\n"),
    )
    .unwrap();

    let output = Command::new(bin())
        .arg("observe")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run observe with empty scope");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains("empty"),
        "must fail with a visible, explicit message naming the empty scope, not a silent fallback: {stderr}"
    );
}

/// Invalid `[scan]` config must refuse to run rather than silently
/// falling back to the (broader) all-defaults scope -- #41's core
/// safety requirement, exercised end-to-end through the real binary.
#[test]
fn observe_with_invalid_scan_config_fails_visibly() {
    let home = tempfile::tempdir().expect("home");
    let store = tempfile::tempdir().expect("store");
    std::fs::write(
        store.path().join("config.toml"),
        "[scan]\ndefaults = \"not-a-bool\"\n",
    )
    .unwrap();

    let output = Command::new(bin())
        .arg("observe")
        .env("SWAMP_DIR", store.path())
        .env("HOME", home.path())
        .output()
        .expect("run observe with invalid config");

    assert!(!output.status.success());
}

/// A `gh` on PATH that only records that it was run and then fails, so
/// the test sees whether observe *tried* to enrich without a network.
fn gh_shim(dir: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin_dir = dir.join("shim-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let log = dir.join("gh-calls.log");
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> '{}'\necho 'shim: not authenticated' >&2\nexit 1\n",
        log.display()
    );
    let gh = bin_dir.join("gh");
    std::fs::write(&gh, script).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

fn observe_with_gh_shim(extra: &[&str]) -> String {
    let root = tempfile::tempdir().expect("root");
    write_git_project(&root.path().join("proj"));
    let git_remote = Command::new("git")
        .arg("-C")
        .arg(root.path().join("proj"))
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/example/proj.git",
        ])
        .status()
        .unwrap();
    assert!(git_remote.success());
    let store = tempfile::tempdir().expect("store");
    let scratch = tempfile::tempdir().expect("scratch");
    let shim = gh_shim(scratch.path());
    let path = format!("{}:{}", shim.display(), std::env::var("PATH").unwrap());
    let output = Command::new(bin())
        .arg("observe")
        .args(extra)
        .arg(root.path())
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .env("PATH", path)
        .output()
        .expect("run observe");
    assert!(
        output.status.success(),
        "observe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(scratch.path().join("gh-calls.log")).unwrap_or_default()
}

#[test]
fn observe_enriches_from_github_by_default() {
    let calls = observe_with_gh_shim(&[]);
    assert!(
        calls.contains("auth status"),
        "a plain observe must try GitHub enrichment (the scheduled run passes no flags); gh calls: {calls:?}"
    );
}

#[test]
fn observe_no_enrich_makes_no_gh_calls() {
    let calls = observe_with_gh_shim(&["--no-enrich"]);
    assert_eq!(calls, "", "--no-enrich must not run gh");
}

fn tree_listing(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let rel = e.path().strip_prefix(base).unwrap().display().to_string();
            let len = e.metadata().unwrap().len();
            out.push(format!("{rel} {len}"));
            if e.file_type().unwrap().is_dir() {
                walk(base, &e.path(), out);
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// A `min_free_bytes` no volume can satisfy stands in for a full disk:
/// the abort must come before any store file is created or changed and
/// exit with the documented code 3.
#[test]
fn observe_aborts_before_touching_the_store_when_free_space_is_below_the_minimum() {
    let root = tempfile::tempdir().expect("root");
    std::fs::write(root.path().join("hello.txt"), b"hi").unwrap();
    let store = tempfile::tempdir().expect("store");
    std::fs::write(
        store.path().join("config.toml"),
        format!("min_free_bytes = {}\n", i64::MAX),
    )
    .unwrap();
    let before = tree_listing(store.path());

    let output = Command::new(bin())
        .arg("observe")
        .arg(root.path())
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run observe");

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    for needle in [
        "disk nearly full",
        "free",
        "minimum",
        "did not write to the store",
    ] {
        assert!(stderr.contains(needle), "missing {needle:?}: {stderr}");
    }
    assert!(
        stderr.contains(&store.path().display().to_string()),
        "{stderr}"
    );
    assert!(output.stdout.is_empty(), "no observe line on abort");
    assert_eq!(tree_listing(store.path()), before, "store changed on abort");
}

#[test]
fn min_free_bytes_zero_disables_the_check() {
    let root = tempfile::tempdir().expect("root");
    std::fs::write(root.path().join("hello.txt"), b"hi").unwrap();
    let store = tempfile::tempdir().expect("store");
    std::fs::write(store.path().join("config.toml"), "min_free_bytes = 0\n").unwrap();
    let output = Command::new(bin())
        .arg("observe")
        .arg(root.path())
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run observe");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn observe_enrich_flag_forces_a_github_refresh() {
    let calls = observe_with_gh_shim(&["--enrich"]);
    assert!(
        calls.contains("auth status"),
        "--enrich must run gh; gh calls: {calls:?}"
    );
}

#[test]
fn observe_enrich_conflicts_with_no_enrich() {
    let store = tempfile::tempdir().expect("store");
    let output = Command::new(bin())
        .args(["observe", "--enrich", "--no-enrich"])
        .env("SWAMP_DIR", store.path())
        .env("SWAMP_TEST_MODE", "1")
        .output()
        .expect("run observe");
    assert!(!output.status.success());
}
