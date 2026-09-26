//! The runtime half of `.oh/guardrails/coverage-changes-are-not-storage-changes.md`,
//! which until 2026-09-22 had no executable validation at all (the
//! guardrail is `severity: hard` and carried no `audit:` field either).
//!
//! The re-review's CE4 is one instance: adding one `exclude` line to the
//! config reported 64 KB of growth and, on removal, a regrowth event,
//! with zero bytes changed on disk. This file is the family: for each way
//! coverage can change -- an exclusion added, a detector disabled, a
//! location made unreadable, the invocation switched to an explicit root
//! -- three passes over an *unchanged* tree must produce zero growth,
//! zero regrowth and zero tombstones.
//!
//! "Zero tombstones" is checked at the store, not inferred: pass 3
//! restores the original coverage and every unit must come back with the
//! byte total it had in pass 1 and `regrowth_count == 0`. A tombstone
//! written in pass 2 shows up here as a regrowth in pass 3, which is
//! exactly how the bug presented.
//!
//! Disposable `tempfile` fixtures only.

use std::{collections::HashMap, fs};
use swamp_core::{
    agents, external,
    fs_events::EventCoverage,
    locations::{Environment, Platform, Registry},
    scope::{ScanConfig, resolve_effective_scope},
};

fn only(ids: &[&str]) -> ScanConfig {
    ScanConfig {
        defaults: false,
        disabled_detectors: Registry::with_builtins()
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| !ids.contains(&id.as_str()))
            .collect(),
        ..Default::default()
    }
}

/// A Cargo home with a nested registry cache: the parent and the child
/// are separate external units, and the child is inside the parent's
/// path-prefix ownership window. That overlap is what every case below
/// attacks.
struct Fixture {
    _tmp: tempfile::TempDir,
    outer: std::path::PathBuf,
    inner: std::path::PathBuf,
    env: Environment,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let outer = tmp.path().join("cargo");
    let inner = outer.join("registry/cache");
    fs::create_dir_all(&inner).unwrap();
    fs::write(outer.join("top.bin"), vec![b'x'; 4096]).unwrap();
    fs::write(inner.join("crate.crate"), vec![b'y'; 65536]).unwrap();
    let env = Environment::fixture(
        tmp.path().to_owned(),
        HashMap::from([("CARGO_HOME".into(), outer.display().to_string())]),
        Platform::MacOS,
    );
    Fixture {
        _tmp: tmp,
        outer,
        inner,
        env,
    }
}

type Snapshot = Vec<(std::path::PathBuf, u64, Option<i64>, u32)>;

fn observe(
    env: &Environment,
    cfg: &ScanConfig,
    explicit: &[std::path::PathBuf],
    store: &std::path::Path,
    at: u64,
) -> Snapshot {
    let registry = Registry::with_builtins();
    let scope = resolve_effective_scope(env, cfg, explicit, &registry, at);
    let units = external::discover_and_measure(
        &scope,
        Some(store),
        true,
        at,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    let mut out: Snapshot = units
        .into_iter()
        .map(|u| (u.path, u.bytes, u.growth_bytes, u.regrowth_count))
        .collect();
    out.sort();
    out
}

fn assert_no_invented_history(label: &str, snap: &Snapshot) {
    let grew: Vec<_> = snap
        .iter()
        .filter(|(_, _, g, _)| g.unwrap_or(0) != 0)
        .collect();
    assert!(
        grew.is_empty(),
        "{label}: nothing changed on disk, yet growth was reported: {grew:?}"
    );
    let regrew: Vec<_> = snap.iter().filter(|(_, _, _, r)| *r > 0).collect();
    assert!(
        regrew.is_empty(),
        "{label}: nothing changed on disk, yet a regrowth was recorded: {regrew:?}"
    );
}

/// Pass 1 baseline, pass 2 with coverage changed, pass 3 with the
/// original coverage restored. No byte on disk moves.
fn three_passes(label: &str, narrowed: impl Fn(&Fixture) -> (ScanConfig, Vec<std::path::PathBuf>)) {
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let base = only(&["cargo-home"]);

    let first = observe(&f.env, &base, &[], store.path(), 1_000);
    assert!(
        first.len() >= 2,
        "{label} precondition: the parent and the nested location are both measured: {first:?}"
    );

    let (narrow_cfg, narrow_roots) = narrowed(&f);
    let second = observe(&f.env, &narrow_cfg, &narrow_roots, store.path(), 2_000);
    assert_no_invented_history(&format!("{label} pass 2 (coverage narrowed)"), &second);

    let third = observe(&f.env, &base, &[], store.path(), 3_000);
    assert_no_invented_history(&format!("{label} pass 3 (coverage restored)"), &third);
    assert_eq!(
        first.iter().map(|(p, b, _, _)| (p, b)).collect::<Vec<_>>(),
        third.iter().map(|(p, b, _, _)| (p, b)).collect::<Vec<_>>(),
        "{label}: restoring coverage did not restore the same measured units"
    );
}

#[test]
fn adding_an_exclusion_is_not_growth_or_regrowth() {
    three_passes("exclusion", |f| {
        (
            ScanConfig {
                exclude: vec![f.inner.display().to_string()],
                ..only(&["cargo-home"])
            },
            vec![],
        )
    });
}

#[test]
fn disabling_a_detector_is_not_growth_or_regrowth() {
    three_passes("disabled detector", |_| {
        let registry = Registry::with_builtins();
        (
            ScanConfig {
                defaults: false,
                disabled_detectors: registry
                    .detectors()
                    .iter()
                    .map(|d| d.id().to_string())
                    .collect(),
                ..Default::default()
            },
            vec![],
        )
    });
}

#[test]
fn switching_to_an_explicit_root_that_excludes_a_location_is_not_growth_or_regrowth() {
    three_passes("explicit root + exclusion", |f| {
        (
            ScanConfig {
                exclude: vec![f.inner.display().to_string()],
                ..only(&["cargo-home"])
            },
            vec![f.outer.clone()],
        )
    });
}

/// Lost access is the case the guardrail names first: "unobserved is not
/// deleted". A location that cannot be read this pass keeps its last
/// known value and must never be tombstoned.
#[test]
fn losing_access_to_a_location_is_not_a_deletion() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let base = only(&["cargo-home"]);
    let first = observe(&f.env, &base, &[], store.path(), 1_000);
    assert!(first.len() >= 2, "precondition: {first:?}");

    fs::set_permissions(&f.inner, fs::Permissions::from_mode(0o000)).unwrap();
    let second = observe(&f.env, &base, &[], store.path(), 2_000);
    fs::set_permissions(&f.inner, fs::Permissions::from_mode(0o755)).unwrap();
    // Running as root defeats the permission bit; only assert where the
    // platform enforces it.
    let canonical_inner = fs::canonicalize(&f.inner).unwrap();
    let unreadable = second
        .iter()
        .find(|(p, _, _, _)| *p == canonical_inner)
        .is_some_and(|(_, b, _, _)| *b == 65536);
    if !unreadable {
        assert_no_invented_history("lost access pass 2", &second);
        let third = observe(&f.env, &base, &[], store.path(), 3_000);
        assert_no_invented_history("lost access pass 3 (access restored)", &third);
    }
}

/// The queued bug this file's family missed: `losing_access_to_a_location`
/// above makes a whole *unit's root* unreadable. Here the root
/// (`f.outer`, the CARGO_HOME unit) stays readable and only a plain
/// subdirectory a few levels inside it goes `chmod 000`. The folded walk
/// still visits and sums everything it *can* read, so it used to report
/// a smaller, "complete" total -- indistinguishable from bytes actually
/// having been removed, which regrowth on pass 3 would then report as
/// growth returning. `FoldedUnit::complete` (`folded_measurement.rs`) is
/// what closes this: an incomplete fold is never stored, so it never
/// overwrites pass 1's baseline and never anchors a reuse.
#[test]
fn an_unreadable_subdirectory_inside_a_unit_is_not_growth_or_regrowth() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let store = tempfile::tempdir().unwrap();
    let base = only(&["cargo-home"]);

    // A subdirectory of the CARGO_HOME unit itself, well clear of the
    // separately-measured `registry/cache` nested unit.
    let locked = f.outer.join("bin").join("locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("cargo-fmt"), vec![b'z'; 8192]).unwrap();

    let first = observe(&f.env, &base, &[], store.path(), 1_000);
    let canonical_outer = fs::canonicalize(&f.outer).unwrap();
    let baseline = first
        .iter()
        .find(|(p, _, _, _)| *p == canonical_outer)
        .expect("the cargo home unit is measured")
        .1;
    assert!(baseline > 0, "precondition: {first:?}");

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let second = observe(&f.env, &base, &[], store.path(), 2_000);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    // Running as root defeats the permission bit; only assert where the
    // platform actually enforces it -- i.e. the pass really did see a
    // smaller, incomplete total for the unit.
    let saw_incomplete_total = second
        .iter()
        .find(|(p, _, _, _)| *p == canonical_outer)
        .is_some_and(|(_, b, _, _)| *b < baseline);
    if saw_incomplete_total {
        assert_no_invented_history("unreadable subdirectory pass 2", &second);
        let third = observe(&f.env, &base, &[], store.path(), 3_000);
        assert_no_invented_history("unreadable subdirectory pass 3 (access restored)", &third);
        let restored = third
            .iter()
            .find(|(p, _, _, _)| *p == canonical_outer)
            .map(|(_, b, _, _)| *b);
        assert_eq!(
            restored,
            Some(baseline),
            "pass 3 must see the same total pass 1 did, not a partial one carried forward"
        );
    }
}

/// The queued follow-up this session note named directly:
/// `folded_measurement::folded_bytes_bounded_stamped` (the agent
/// family's bounded per-session fold, used by `agents::IdentifyCtx::
/// folded_bytes`) had the identical `let Ok(rd) = ... else { continue };`
/// shape as the CARGO_HOME bug above, with *no* completeness signal at
/// all -- only `truncated` (hitting `max_entries`) was tracked. An
/// agent unit's `bytes` is persisted as growth/regrowth history exactly
/// like an external unit's (`agents::discover_and_measure_in` pushes
/// `ObservedExternal` into the same `crate::growth::
/// observe_and_annotate_external` external.rs uses), so this was the
/// same bug, one call away.
///
/// Fixed the same way: an unreadable subdirectory now sets `truncated`
/// (this function's one incompleteness signal) instead of silently
/// `continue`ing past it, and `CandidateAgentUnit::complete` /
/// `AgentUnit::complete` carry that signal out to the caller, which
/// skips the growth-history write and keeps the store's last regrowth
/// count rather than resetting it to zero (mirroring `external.rs`'s
/// `protected_keys`).
#[test]
fn an_unreadable_subdirectory_inside_an_agent_unit_is_not_growth_or_regrowth() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("claude");

    // Claude Code's `shell-snapshots` static category: an ordinary,
    // unprotected `CacheOrLogTrash` unit folded through
    // `IdentifyCtx::folded_bytes`, well clear of the `sessions/`
    // container machinery this file's other tests do not exercise.
    let shell_snapshots = home.join("shell-snapshots");
    let locked = shell_snapshots.join("nested").join("locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("snap.sh"), vec![b'z'; 8192]).unwrap();
    fs::write(shell_snapshots.join("top.sh"), vec![b'x'; 4096]).unwrap();

    let env = Environment::fixture(
        root.clone(),
        HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string())]),
        Platform::MacOS,
    );
    let cfg = only(&["claude-code"]);
    let scope = resolve_effective_scope(&env, &cfg, &[], &Registry::with_builtins(), 1_000);
    let store = tempfile::tempdir().unwrap();

    let observe = |at: u64, coverage: &EventCoverage| {
        agents::discover_and_measure(
            &scope,
            &[],
            Some(store.path()),
            true,
            at,
            30,
            3600,
            coverage,
        )
        .expect("agent discovery")
    };

    let first = observe(1_000, &EventCoverage::untrusted());
    let baseline = first
        .iter()
        .find(|u| u.relative_path == "shell-snapshots")
        .expect("the shell-snapshots unit is measured")
        .bytes;
    assert!(baseline > 0, "precondition: {baseline}");

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let second = observe(2_000, &EventCoverage::untrusted());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    // Running as root defeats the permission bit; only assert where the
    // platform actually enforces it -- i.e. the pass really did see a
    // smaller, incomplete total for the unit.
    let second_unit = second.iter().find(|u| u.relative_path == "shell-snapshots");
    let saw_incomplete_total = second_unit.is_some_and(|u| u.bytes < baseline);
    if saw_incomplete_total {
        let u = second_unit.unwrap();
        assert!(
            !u.complete,
            "a partial total must be marked incomplete, not presented as a confirmed size"
        );
        assert_eq!(
            u.growth_bytes, None,
            "an incomplete pass must never report a growth/regrowth delta"
        );
        assert_eq!(u.regrowth_count, 0, "no regrowth recorded yet");

        let third = observe(3_000, &EventCoverage::untrusted());
        let third_unit = third
            .iter()
            .find(|u| u.relative_path == "shell-snapshots")
            .expect("still measured with access restored");
        assert!(
            third_unit.complete,
            "access restored: the total is complete again"
        );
        assert_eq!(
            third_unit.bytes, baseline,
            "pass 3 must see the same total pass 1 did, not a partial one carried forward"
        );
        assert_eq!(
            third_unit.growth_bytes,
            Some(0),
            "restoring access must never read as growth"
        );
        assert_eq!(
            third_unit.regrowth_count, 0,
            "restoring access must never read as regrowth"
        );
    }
}
