//! #171: a directory `CARGO_TARGET_DIR` built into is its own kind, with
//! its consequence and age on the row, plannable through the reviewed
//! Trash flow with the in-use reading -- and only when it carries Cargo's
//! own signature. Disposable fixtures; nothing outside the temp root is
//! read.
//!
//! Tempting wrong patches these fail: (1) treating any `CACHEDIR.TAG` as
//! Cargo's (pytest, uv and others write the same signature); (2)
//! recognizing `.rustc_info.json` alone; (3) double counting a project's
//! own `target/`; (4) guessing a project from the directory's name.

use std::fs;
use std::path::{Path, PathBuf};
use swamp_core::evidence::{FactKind, FactStatus, FactValue};
use swamp_core::fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource};
use swamp_core::report::UnownedReason;

const SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n";

struct Quiet;
impl FsEventsSource for Quiet {
    fn replay(&self, _: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan::from_live(Vec::new(), 1000, None)
    }
}

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7u8; bytes]).unwrap();
}

fn cargo_target(dir: &Path) {
    write(&dir.join("CACHEDIR.TAG"), 0);
    fs::write(dir.join("CACHEDIR.TAG"), SIGNATURE).unwrap();
    write(&dir.join(".rustc_info.json"), 300);
    write(&dir.join("debug/deps/libfoo.rlib"), 200_000);
    write(&dir.join("debug/deps/foo.d"), 400);
}

fn report(root: &Path, store: &Path) -> swamp_core::Report {
    swamp_core::report::report_full_mode_scoped(
        root,
        None,
        false,
        Some(store),
        Some("1h"),
        true,
        false,
        false,
        true,
        &Quiet,
        &[],
        false,
    )
    .unwrap()
}

fn git_init(root: &Path) {
    fs::create_dir_all(root).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
}

fn row_for<'a>(
    r: &'a swamp_core::Report,
    path: &Path,
) -> Option<&'a swamp_core::report::UnownedRow> {
    r.unowned
        .iter()
        .find(|u| Path::new(&u.path_or_object) == path)
}

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: tempfile::TempDir,
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    Fx {
        _tmp: tmp,
        root,
        store: tempfile::tempdir().unwrap(),
    }
}

#[test]
fn a_directory_with_cargos_signature_is_its_own_kind_with_its_consequence_and_age() {
    let fx = fixture();
    let target = fx.root.join("swamp-scratch-target-145");
    cargo_target(&target);
    let r = report(&fx.root, fx.store.path());
    let row = row_for(&r, &target).expect("the target directory is one unowned unit");
    assert_eq!(row.reason, UnownedReason::StandaloneCargoTarget);
    assert!(row.bytes >= 200_000, "allocated size: {}", row.bytes);
    let note = row.note.as_deref().unwrap();
    assert!(note.contains("cargo build"), "{note}");
    // No project is named: nothing in the directory records one.
    assert!(!note.to_lowercase().contains("swamp-scratch"), "{note}");
    // Age from its modification time, as evidence on the row.
    let age = row
        .evidence
        .iter()
        .find(|e| e.subtype == swamp_core::evidence::FactSubtype::Modified)
        .expect("a modification fact");
    assert!(matches!(age.status, FactStatus::Known(FactValue::Timestamp(t)) if t > 0));
    // Never described as unowned residual under the generic reason.
    assert_ne!(row.reason, UnownedReason::NoContainingRepo);
}

#[test]
fn a_cache_tag_without_cargos_second_half_is_not_cargo() {
    let fx = fixture();
    // pytest's own cache: the standard signature, no `.rustc_info.json`.
    let pytest = fx.root.join("pytest-cache");
    write(&pytest.join("v/cache/lastfailed"), 50_000);
    fs::write(
        pytest.join("CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    // A `.rustc_info.json` beside a tag with the wrong signature.
    let imposter = fx.root.join("imposter");
    write(&imposter.join("debug/x"), 50_000);
    write(&imposter.join(".rustc_info.json"), 100);
    fs::write(
        imposter.join("CACHEDIR.TAG"),
        b"Signature: 0123456789abcdef0123456789abcdef\n",
    )
    .unwrap();
    // `.rustc_info.json` with no tag at all.
    let no_tag = fx.root.join("no-tag");
    write(&no_tag.join("debug/x"), 50_000);
    write(&no_tag.join(".rustc_info.json"), 100);
    // A tag that only mentions the signature further in.
    let mention = fx.root.join("mention");
    write(&mention.join("debug/x"), 50_000);
    write(&mention.join(".rustc_info.json"), 100);
    fs::write(
        mention.join("CACHEDIR.TAG"),
        b"# see also\nSignature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    let r = report(&fx.root, fx.store.path());
    for dir in [&pytest, &imposter, &no_tag, &mention] {
        assert!(
            !r.unowned.iter().any(|u| {
                u.reason == UnownedReason::StandaloneCargoTarget
                    && Path::new(&u.path_or_object).starts_with(dir)
            }),
            "{} was taken for a Cargo target",
            dir.display()
        );
    }
    // The tag-only directory keeps its ordinary unowned row.
    let row = row_for(&r, &pytest).expect("still one unit");
    assert_eq!(row.reason, UnownedReason::NoContainingRepo);
    // And it is not plannable as a Cargo target.
    let err = swamp_core::actions::propose(&r, None, std::slice::from_ref(&pytest), "test")
        .expect_err("a non-Cargo cache is not a plan unit");
    assert!(
        err.to_string().contains("not plannable") || err.to_string().contains("nothing to propose")
    );
}

#[test]
fn a_target_inside_a_project_is_counted_once_under_the_project() {
    let fx = fixture();
    let proj = fx.root.join("proj");
    git_init(&proj);
    fs::write(
        proj.join("Cargo.toml"),
        b"[package]\nname=\"p\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    fs::write(proj.join(".gitignore"), b"target/\n").unwrap();
    cargo_target(&proj.join("target"));
    let standalone = fx.root.join("elsewhere-target");
    cargo_target(&standalone);
    let r = report(&fx.root, fx.store.path());
    let inner = proj.join("target");
    assert!(
        row_for(&r, &inner).is_none(),
        "the project's own target is not also an unowned row"
    );
    let owned: Vec<_> = r
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .flat_map(|w| w.artifacts.iter())
        .filter(|a| a.path == inner)
        .collect();
    assert_eq!(owned.len(), 1, "exactly one artifact row for it");
    assert!(
        row_for(&r, &standalone).is_some_and(|u| u.reason == UnownedReason::StandaloneCargoTarget)
    );
    // Nothing is counted twice: the walk's total is the attributed bytes
    // plus the unowned ones.
    let unowned: u64 = r.unowned.iter().map(|u| u.bytes).sum();
    let attributed: u64 = r
        .projects
        .iter()
        .flat_map(|p| p.worktrees.iter())
        .flat_map(|w| w.artifacts.iter())
        .map(|a| a.bytes)
        .sum();
    assert_eq!(r.reconciliation.walked_total, attributed + unowned);
}

#[test]
fn a_standalone_target_plans_through_the_trash_flow_with_the_in_use_reading() {
    let fx = fixture();
    let target = fx.root.join("swamp-fix-generic-target");
    cargo_target(&target);
    let r = report(&fx.root, fx.store.path());
    let units = swamp_core::actions::propose(&r, None, std::slice::from_ref(&target), "test")
        .expect("a standalone Cargo target is plannable");
    assert_eq!(units.len(), 1);
    let unit = &units[0];
    assert_eq!(unit.path(), target);
    assert_eq!(unit.verb(), "delete");
    let warnings = unit.warnings().join(" | ");
    assert!(warnings.contains("cargo build"), "{warnings}");
    assert!(warnings.contains("Trash"), "{warnings}");
    let lower = warnings.to_lowercase();
    for word in ["unused", "obsolete", "stale", "orphan", "safe to"] {
        assert!(!lower.contains(word), "{word} in {warnings}");
    }
    // The occupancy reading the confirm line shows, taken when planned.
    assert!(
        unit.evidence()
            .iter()
            .any(|e| e.kind == FactKind::CurrentUse),
        "an in-use fact is part of the plan"
    );
}

#[test]
fn an_open_handle_inside_the_target_is_reported_not_hidden() {
    let fx = fixture();
    let target = fx.root.join("held-target");
    cargo_target(&target);
    let r = report(&fx.root, fx.store.path());
    let held = fs::File::open(target.join("debug/deps/libfoo.rlib")).unwrap();
    let units =
        swamp_core::actions::propose(&r, None, std::slice::from_ref(&target), "test").unwrap();
    let current_use: Vec<_> = units[0]
        .evidence()
        .iter()
        .filter(|e| e.kind == FactKind::CurrentUse)
        .collect();
    assert!(!current_use.is_empty());
    // Never a confident "free" while a file inside is open: either the
    // probe saw the handle, or it says it could not look.
    for e in &current_use {
        assert!(
            !matches!(e.status, FactStatus::Known(FactValue::Bool(false))),
            "an open file was reported as not in use: {e:?}"
        );
    }
    drop(held);
}
