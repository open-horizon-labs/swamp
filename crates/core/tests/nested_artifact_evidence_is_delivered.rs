//! `.oh/guardrails/computed-but-not-delivered.md`'s own defect, closed.
//!
//! `CHANGELOG.md` claimed decision evidence was "attached to ... nested
//! build-artifact units". `NestedArtifact::decision_evidence` was
//! written only as `Vec::new()`, `report::attach_decision_evidence`
//! iterated `projects[].worktrees[].artifacts` and never touched
//! `report.nested_artifacts`, `skip_serializing_if` hid the empty vector
//! from `--view rust` JSON, and no test existed. The 2026-09-22
//! re-review found it, and found it because this was the one guardrail
//! in the directory whose frontmatter said `audit: none`.
//!
//! This test asserts the whole chain the guardrail requires: the fact is
//! extracted, carried on the schema, and visible in real output.

use std::{fs, path::PathBuf};
use swamp_core::report::report_full_mode;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("repo");
    fs::create_dir_all(&root).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let target = root.join("target");
    fs::create_dir_all(target.join("debug/deps")).unwrap();
    fs::create_dir_all(target.join("debug/.fingerprint/fixture-aaa")).unwrap();
    fs::write(target.join("debug/.cargo-lock"), b"").unwrap();
    fs::write(
        target.join("debug/.fingerprint/fixture-aaa/test-lib-fixture.json"),
        r#"{"rustc":42,"features":"[]"}"#,
    )
    .unwrap();
    for name in ["fixture-aaa", "fixture-aaa.d", "libdependency.rlib"] {
        fs::write(target.join("debug/deps").join(name), vec![1u8; 8192]).unwrap();
    }
    (tmp, root, target)
}

fn report(root: &std::path::Path, store: &std::path::Path) -> swamp_core::Report {
    report_full_mode(
        root,
        None,
        false,
        Some(store),
        Some("1h"),
        true,
        false,
        false,
        true,
    )
    .unwrap()
}

#[test]
fn every_nested_build_artifact_unit_carries_decision_evidence() {
    let (_tmp, root, _target) = fixture();
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    assert!(
        !r.nested_artifacts.is_empty(),
        "precondition: the Cargo target tree produced nested units"
    );
    let bare: Vec<String> = r
        .nested_artifacts
        .iter()
        .filter(|u| u.decision_evidence.is_empty())
        .map(|u| u.relative_path.clone())
        .collect();
    assert!(
        bare.is_empty(),
        "these nested build-artifact units carry no decision evidence: {bare:?}"
    );
}

/// Activity and reclaimability are the two domains the CHANGELOG claim
/// names for nested units. A unit whose evidence is only, say, a
/// consumer fact would satisfy "non-empty" while delivering neither.
#[test]
fn nested_unit_evidence_covers_activity_and_reclaimability() {
    use swamp_core::evidence::FactKind;
    let (_tmp, root, _target) = fixture();
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    for u in &r.nested_artifacts {
        let kinds: Vec<FactKind> = u.decision_evidence.iter().map(|e| e.kind).collect();
        assert!(
            kinds.contains(&FactKind::Activity),
            "{} has no activity fact: {kinds:?}",
            u.relative_path
        );
        assert!(
            kinds.contains(&FactKind::Reclaimability),
            "{} has no reclaimability fact: {kinds:?}",
            u.relative_path
        );
    }
}

/// `--view rust` serializes `report.nested_artifacts` wholesale, and
/// `decision_evidence` is `skip_serializing_if = "Vec::is_empty"`, so an
/// empty vector is invisible there. That was the other half of the
/// overclaim: even if the field had been populated, nothing proved the
/// JSON showed it.
#[test]
fn the_rust_view_json_shows_nested_unit_evidence() {
    let (_tmp, root, _target) = fixture();
    let store = tempfile::tempdir().unwrap();
    let r = report(&root, store.path());
    let json = serde_json::to_string(&r.nested_artifacts).unwrap();
    assert!(
        json.contains("decision_evidence"),
        "`--view rust` JSON does not carry decision_evidence: {}",
        &json[..json.len().min(400)]
    );
    // Facts, not verdicts: the evidence contract's own vocabulary ban
    // applies to what a nested unit publishes too.
    for banned in ["\"safe\"", "\"unused\"", "\"stale\""] {
        assert!(
            !json.contains(banned),
            "nested unit evidence rendered the verdict {banned}"
        );
    }
}

/// Evidence is current-state, never byte history: attaching it must not
/// create a growth delta or a tombstone
/// (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
#[test]
fn attaching_nested_evidence_does_not_touch_byte_history() {
    let (_tmp, root, _target) = fixture();
    let store = tempfile::tempdir().unwrap();
    let first = report(&root, store.path());
    let second = report(&root, store.path());
    assert!(
        !first.nested_artifacts.is_empty() && !second.nested_artifacts.is_empty(),
        "precondition: both passes produced nested units"
    );
    let invented: Vec<_> = second
        .nested_artifacts
        .iter()
        .filter(|u| u.regrowth_count > 0)
        .map(|u| (u.relative_path.clone(), u.regrowth_count))
        .collect();
    assert!(
        invented.is_empty(),
        "a second pass with nothing changed recorded regrowth: {invented:?}"
    );
}
