//! End-to-end `swamp observe` checks against the built binary:
//! it writes the growth store and prints one machine-readable line per
//! root.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_swamp"))
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
