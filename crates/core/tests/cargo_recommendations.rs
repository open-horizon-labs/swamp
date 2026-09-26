use swamp_core::{
    cargo_artifacts::inspect_target,
    cargo_cleanup::{cleanup_order, guidance, guidance_at, recommendation},
};

#[test]
fn recommendations_explain_tradeoffs_without_inventing_disuse() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    std::fs::create_dir_all(target.join("debug/incremental/crate-a")).unwrap();
    std::fs::write(target.join("debug/incremental/crate-a/state"), b"data").unwrap();
    let report = inspect_target(&target, Some(&target));
    let mut unit = report
        .units
        .iter()
        .find(|u| u.path.ends_with("crate-a"))
        .unwrap()
        .clone();
    assert_eq!(guidance(&unit).check_status, "unchecked");
    assert!(recommendation(&unit).0.contains("Start here"));
    assert!(recommendation(&unit).1.contains("slower"));
    let now = 30 * 86400;
    unit.mtime_max = now - 21 * 86400;
    let advice = guidance_at(&unit, now);
    assert!(advice.recommendation.contains("Cleanup candidate"));
    assert!(advice.recommendation.contains("21d ago"));
    assert_eq!(advice.modified_age_secs, Some(21 * 86400));
    let mut recent = unit.clone();
    recent.mtime_max = now - 3600;
    recent.bytes = unit.bytes + 1_000_000;
    assert!(
        cleanup_order(&unit, &recent, now).is_lt(),
        "older beats larger"
    );
    assert!(guidance_at(&recent, now).recommendation.contains("1h ago"));
    assert_eq!(
        guidance_at(&recent, now).check_status,
        "unchecked",
        "recent remains reviewable"
    );
    for timestamp in [0, now + 1] {
        let mut unknown = unit.clone();
        unknown.mtime_max = timestamp;
        assert_eq!(guidance_at(&unknown, now).modified_age_secs, None);
        assert!(cleanup_order(&recent, &unknown, now).is_lt());
    }
    let before = recommendation(&unit);
    unit.mtime_max = 1;
    assert_eq!(
        recommendation(&unit),
        before,
        "age is not evidence of disuse"
    );
    unit.coverage.complete = false;
    assert_eq!(recommendation(&unit).0, "Inspect coverage");
    assert_eq!(guidance(&unit).check_status, "blocked");
}

/// R18a: `Report.nested_artifacts` used to compute its `"cleanup"`
/// guidance (including `modified_age_secs`) at *serialization* time via
/// a `#[serde(serialize_with = ...)]` hook that called
/// `crate::entities::now()` directly -- so serializing the very same
/// `Report` twice, a wall-clock second apart, produced two different
/// JSON bodies (`swamp report --json` was not a pure read of stored
/// data). `NestedArtifact::guidance` is now computed exactly once, at
/// observe time, from the report's own fixed `observed_at`
/// (`report::attach_cargo_guidance`) and serialized as a plain field:
/// this test builds one `Report`, computes guidance once with an
/// artificially early `observed_at`, and asserts that serializing it
/// twice -- with a real wall-clock gap in between -- is byte-identical.
#[test]
fn report_json_with_nested_artifacts_serializes_byte_identically_across_a_real_clock_gap() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    std::fs::create_dir_all(target.join("debug/incremental/crate-a")).unwrap();
    std::fs::write(target.join("debug/incremental/crate-a/state"), b"data").unwrap();
    let inspected = inspect_target(&target, Some(&target));
    let mut unit = inspected
        .units
        .iter()
        .find(|u| u.path.ends_with("crate-a"))
        .unwrap()
        .clone();
    // Fixed in the past, far enough that `modified_age_secs` is a
    // provably stable, non-zero number -- if anything downstream ever
    // recomputed it from a live clock again, the two serializations
    // below would disagree by however many seconds this test's own
    // wall-clock sleep below takes.
    let observed_at = 1_700_000_000;
    unit.mtime_max = observed_at - 3600;

    let mut report = swamp_core::report::Report {
        observed_at,
        root: tmp.path().to_path_buf(),
        projects: Vec::new(),
        unowned: Vec::new(),
        reconciliation: swamp_core::report::Reconciliation {
            unique_estimate: None,
            attributed: 0,
            unowned: 0,
            walked_total: 0,
            du_total: None,
            docker_attributed: 0,
            docker_unowned: 0,
        },
        series_by_key: std::collections::HashMap::new(),
        total_series: Vec::new(),
        series_window_secs: 0,
        notes: Vec::new(),
        dirs_by_worktree: None,
        files_by_worktree: None,
        schedule_line: None,
        summary: swamp_core::report::Summary::default(),
        github_enrichment: None,
        nested_artifacts: vec![unit],
        store_dir: None,
    };
    // The same finalizer `bus::run_report` calls once per observe pass,
    // called here directly since this fixture builds its own `Report`
    // rather than going through a full observe.
    swamp_core::report::attach_decision_evidence(&mut report);

    let first = serde_json::to_string(&report).unwrap();
    // A real wall-clock gap -- long enough to cross a whole-second
    // boundary, which is exactly what made the old `serialize_with`
    // hook's `entities::now()` call flaky.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = serde_json::to_string(&report).unwrap();
    assert_eq!(
        first, second,
        "serializing the same Report twice, a wall-clock second apart, must be byte-identical"
    );

    let value: serde_json::Value = serde_json::from_str(&first).unwrap();
    let cleanup = &value["nested_artifacts"][0]["cleanup"];
    assert_eq!(
        cleanup["modified_age_secs"], 3600,
        "guidance must reflect the report's own observed_at, not a live clock"
    );
}
