//! Re-review 3, brief item: adjudicate the `RefreshRefusal::TooSoon`
//! residual left by `reviewer_cost_measurement_stack2::
//! two_unchanged_full_observations_cost_report`.
//!
//! The stack/10 test called `observe(scope, store, 1_000)` and
//! `observe(scope, store, 2_000)` -- but `report::observe_scope` takes
//! **no** `observed_at` argument (`crates/core/src/report.rs:2301-2316`),
//! so that third parameter was dead and the two passes ran about one
//! wall-clock second apart. `growth::replay_unit_roots` refuses a window
//! when `observed_at - stored < min_interval_secs()` (default 3,
//! `growth.rs:2050-2061`), so every root got `too_soon` and every unit
//! was honestly re-measured.
//!
//! This is the same fixture, spaced past the floor, with the counters
//! printed for four consecutive passes. Disposable fixtures only;
//! nothing reads a real agent or editor home.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};
use swamp_core::work_counters::{self, WorkCounters};

const SESSIONS: usize = 5_000;
const EXTERNAL_FILES: usize = 20_000;

fn shim_dir(dir: &Path, counter: &Path, names: &[&str]) -> String {
    fs::create_dir_all(dir).unwrap();
    for n in names {
        let p = dir.join(n);
        fs::write(
            &p,
            format!("#!/bin/sh\necho \"{n}\" >> {}\nexit 1\n", counter.display()),
        )
        .unwrap();
        let mut perms = fs::metadata(&p).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&p, perms).unwrap();
    }
    let prev = std::env::var("PATH").unwrap_or_default();
    unsafe { std::env::set_var("PATH", format!("{}:{prev}", dir.display())) };
    prev
}

/// The PATH-shim oracle: every `Program` the gate can run, plus the
/// original list's `simctl` and `mdls`.
fn spawn_oracle() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = swamp_core::fs_gate::spawn::Program::ALL
        .iter()
        .map(|p| p.binary())
        .collect();
    names.extend(["simctl", "mdls"]);
    names
}

/// The oracle cannot fall behind the gate: a new `Program` variant is
/// shimmed the moment it exists, and the binaries the 2026-09-22
/// re-review found unshimmed are in it.
#[test]
fn spawn_oracle_covers_every_program_the_gate_can_run() {
    let oracle = spawn_oracle();
    for p in swamp_core::fs_gate::spawn::Program::ALL {
        assert!(oracle.contains(&p.binary()), "{p:?} is not shimmed");
    }
    for name in [
        "lsof",
        "plutil",
        "xcrun",
        "du",
        "docker",
        "git",
        "gh",
        "df",
        "id",
        "launchctl",
    ] {
        assert!(oracle.contains(&name), "`{name}` is not shimmed");
        assert!(
            swamp_core::fs_gate::spawn::Program::named(name).is_some(),
            "`{name}` is spawned by production but is not a gate Program"
        );
    }
}

fn spawns(counter: &Path) -> Vec<String> {
    fs::read_to_string(counter)
        .unwrap_or_default()
        .lines()
        .map(|s| s.to_string())
        .collect()
}

fn fixture(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let claude = root.join("claude");
    for p in 0..5 {
        let proj = claude.join(format!("projects/-Users-dev-proj{p}"));
        fs::create_dir_all(&proj).unwrap();
        for s in 0..(SESSIONS / 5) {
            fs::write(
                proj.join(format!("sess-{p}-{s}.jsonl")),
                format!("{{\"cwd\":\"/Users/dev/proj{p}\",\"type\":\"user\"}}\n"),
            )
            .unwrap();
        }
    }
    fs::create_dir_all(claude.join("debug")).unwrap();
    fs::write(claude.join("debug/log.txt"), vec![b'x'; 4096]).unwrap();

    let cargo_home = root.join("cargo-home");
    let registry = cargo_home.join("registry/cache/index.crates.io-abc");
    fs::create_dir_all(&registry).unwrap();
    for i in 0..EXTERNAL_FILES {
        fs::write(registry.join(format!("crate-{i}.crate")), b"x").unwrap();
    }

    let npm = root.join("npm-cache/_cacache/content-v2");
    fs::create_dir_all(&npm).unwrap();
    for i in 0..500 {
        fs::write(npm.join(format!("blob-{i}")), vec![b'y'; 64]).unwrap();
    }
    let hf = root.join("hf/hub/models--x--y/blobs");
    fs::create_dir_all(&hf).unwrap();
    for i in 0..200 {
        fs::write(hf.join(format!("b-{i}")), vec![b'z'; 1024]).unwrap();
    }

    let src = root.join("src");
    fs::create_dir_all(src.join("p/target/debug")).unwrap();
    fs::write(src.join("p/target/debug/blob.bin"), vec![b'x'; 65536]).unwrap();
    fs::write(src.join("p/Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
    (claude, cargo_home, src)
}

fn scope_for(root: &Path, claude: &Path, cargo_home: &Path, src: &Path) -> EffectiveScope {
    let registry = Registry::with_builtins();
    let keep = ["claude-code", "cargo-home", "npm", "huggingface"];
    let cfg = ScanConfig {
        defaults: false,
        include: vec![src.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| !keep.contains(&id.as_str()))
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(
        root.to_path_buf(),
        HashMap::from([
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                claude.display().to_string(),
            ),
            ("CARGO_HOME".to_string(), cargo_home.display().to_string()),
            (
                "npm_config_cache".to_string(),
                root.join("npm-cache").display().to_string(),
            ),
            ("HF_HOME".to_string(), root.join("hf").display().to_string()),
        ]),
        Platform::MacOS,
    );
    resolve_effective_scope(&env, &cfg, &[], &registry, 1_000)
}

fn observe(scope: &EffectiveScope, store: &Path) -> (usize, usize) {
    let (counts, _) = observe_with_reasons(scope, store);
    counts
}

/// The same observation, with the per-root refusal reasons the Linux
/// contract is stated in (`docs/platform.md`): every unit root and every
/// walked root names why it was re-measured.
fn observe_with_reasons(scope: &EffectiveScope, store: &Path) -> ((usize, usize), Vec<String>) {
    let o = swamp_core::report::observe_scope(
        scope,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        Some(store),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .expect("scope observation");
    let mut reasons: Vec<String> = o
        .unit_root_coverage
        .iter()
        .map(|u| format!("unit:{}:{}", u.event_covered, u.reason))
        .collect();
    reasons.extend(o.coverage.iter().map(|c| format!("walk:{}", c.mode)));
    ((o.agent_units.len(), o.external_units.len()), reasons)
}

fn measured<T>(f: impl FnOnce() -> T) -> (T, WorkCounters, std::time::Duration) {
    work_counters::reset();
    let started = std::time::Instant::now();
    let out = f();
    (out, work_counters::snapshot(), started.elapsed())
}

/// Four consecutive observations of an unchanged tree, each more than
/// the `TooSoon` floor after the last.
///
/// The required behaviour: once a pass has had a replay window, an
/// unchanged tree costs no session-header bytes, no re-stat of measured
/// members, and no subprocess. The assertions below are the stack/10
/// ones, applied to the passes where the floor cannot be the excuse.
#[test]
fn unchanged_observations_spaced_past_the_toosoon_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let (claude, cargo_home, src) = fixture(&root);
    let store = tempfile::tempdir().unwrap();
    let shims = root.join("shims");
    let counter = root.join("spawns.log");
    fs::write(&counter, b"").unwrap();
    // Every binary production may spawn (the gate's `Program::ALL`:
    // git, gh, df, id, launchctl, kill, brew, defaults as well as the
    // original seven's lsof, plutil, xcrun, du, docker), plus `simctl`
    // and `mdls` from the original list: "spawns: []" means all of them.
    let prev_path = shim_dir(&shims, &counter, &spawn_oracle());

    // The fixture's own 25,000 file creations are real filesystem
    // events. fseventsd persists them with a lag, so a first pass taken
    // immediately after building the tree opens a window that then
    // reports the construction itself. Settle before pass 1, so what is
    // measured below is an *unchanged* tree rather than a tree that has
    // just been written.
    std::thread::sleep(std::time::Duration::from_millis(6_000));

    let scope = scope_for(&root, &claude, &cargo_home, &src);
    let authorized = scope.authorized_roots().0;
    let mut rows: Vec<(usize, WorkCounters, std::time::Duration, usize)> = Vec::new();
    let mut spawns_before = 0usize;
    for pass in 1..=4 {
        if pass > 1 {
            // Past the 3 s `min_interval_secs()` floor, in wall-clock
            // time, because `observe_scope` derives `observed_at`
            // itself.
            std::thread::sleep(std::time::Duration::from_millis(3_500));
        }
        let ((_a, _e), counters, elapsed) = measured(|| observe(&scope, store.path()));
        let now = spawns(&counter).len();
        rows.push((pass, counters, elapsed, now - spawns_before));
        spawns_before = now;
    }
    let all_spawns = spawns(&counter);
    unsafe { std::env::set_var("PATH", prev_path) };

    println!("--- RE-REVIEW 3 COST REPORT (spaced past the TooSoon floor) ---");
    println!(
        "fixture: {SESSIONS} agent sessions, {EXTERNAL_FILES}-file cargo cache, 500-file npm \
         cache, 200-file model store, 1 walked project root"
    );
    println!("authorized roots: {}", authorized.len());
    for (pass, c, t, s) in &rows {
        println!(
            "pass {pass}: {t:?} dirs_listed={} files_statted={} header_bytes={} \
             cache_hits={} cache_misses={} spawns={s}",
            c.dirs_listed,
            c.files_statted,
            c.header_bytes_read,
            c.identification_cache_hits,
            c.identification_cache_misses
        );
    }
    println!("all spawns: {all_spawns:?}");
    println!("--- END ---");

    // Required behaviour, in the mandate's own words: an unchanged
    // observation costs zero header bytes, zero subprocesses, and no
    // traversal "beyond the events/coverage check". The floor this
    // stack actually reaches (passes 3 and 4 below) is one listing and
    // a little over one stat per authorized root, which *is* that
    // check. So the bound is stated per root rather than as a flat
    // zero -- and it has to hold from the **second** observation, not
    // the third: a user who runs `swamp report` twice has observed an
    // unchanged tree twice.
    // Adjudicated 2026-09-22 (CHUNK_R7): the reviewer's assertions stand
    // as written on macOS, where FSEvents gives the unit roots and the
    // walk a replay window. Linux has no replay source yet (#81/#82), so
    // its contract (`docs/platform.md`) is a named-reason full re-measure
    // with zero header bytes and no reuse claimed; that branch is below.
    #[cfg(target_os = "macos")]
    {
        let roots = authorized.len().max(1) as u64;
        for (pass, c, _, s) in &rows {
            if *pass < 2 {
                continue;
            }
            assert_eq!(
                c.header_bytes_read, 0,
                "pass {pass}: an unchanged pass must read zero session-header bytes"
            );
            assert_eq!(
                *s, 0,
                "pass {pass}: an unchanged observation must spawn no subprocesses"
            );
            assert!(
                c.dirs_listed <= roots * 2,
                "pass {pass}: an unchanged pass must not list more than the events/coverage \
                 check needs ({} listings for {roots} authorized roots)",
                c.dirs_listed
            );
            assert!(
                c.files_statted <= roots * 2,
                "pass {pass}: an unchanged pass must not re-stat measured members ({} stats for \
                 {roots} authorized roots)",
                c.files_statted
            );
        }
    }
    #[cfg(target_os = "linux")]
    linux_contract(&scope, store.path(), &rows);
}

/// Linux: no FSEvents, so no replay window. Every pass re-measures, and
/// says why: every unit root is `no_persisted_change_history` and not
/// event-covered, and no walk claims an incremental mode. What must still
/// hold from the second pass: zero session-header bytes (identification
/// is cached independently of events) and zero subprocesses.
///
/// Compiled everywhere, called on Linux, so a macOS build type-checks it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_contract(
    scope: &EffectiveScope,
    store: &Path,
    rows: &[(usize, WorkCounters, std::time::Duration, usize)],
) {
    for (pass, c, _, s) in rows {
        if *pass < 2 {
            continue;
        }
        assert_eq!(
            c.header_bytes_read, 0,
            "pass {pass}: an unchanged pass must read zero session-header bytes"
        );
        assert_eq!(
            *s, 0,
            "pass {pass}: an unchanged observation must spawn no subprocesses"
        );
    }
    let (_, reasons) = observe_with_reasons(scope, store);
    assert!(
        reasons.iter().any(|r| r.starts_with("unit:")),
        "the observation reported no unit roots: {reasons:?}"
    );
    for r in &reasons {
        if let Some(rest) = r.strip_prefix("unit:") {
            assert_eq!(
                rest, "false:no_persisted_change_history",
                "a Linux unit root is re-measured and names the platform as the reason: {reasons:?}"
            );
        }
        if let Some(mode) = r.strip_prefix("walk:") {
            assert_ne!(
                mode, "incremental",
                "no Linux walk claims an incremental replay: {reasons:?}"
            );
        }
    }
}
