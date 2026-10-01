//! v0.8.0 G6 verification: the review's open-file snapshot cannot run
//! (no `lsof` available). One test in its own process, so PATH cannot leak.

/// macOS only: that platform's probe is `lsof`. Linux reads `/proc` and
/// never runs `lsof` (`platform::OccupancyProbe::Procfs`), so with `lsof`
/// absent its confirm rightly says nothing; the Linux "could not check"
/// line is covered by `occupancy::tests::procfs_a_foreign_pid_namespace_or_missing_proc_is_unknown`
/// (Unknown, never free) and `reclaim_trash::tests::occupancy_is_tri_state_lines_never_silence_for_unknown`
/// (Unknown becomes a line).
///
/// Tempting wrong patch: an open-file probe that could not start reads as
/// "nothing holds it" (no line), so the confirm is silent exactly when
/// swamp did not look. Missing `lsof` is said on the confirm; it is a line,
/// never a refusal.
#[cfg(target_os = "macos")]
#[test]
fn adv_b_a_review_without_lsof_says_it_could_not_check_and_still_offers_the_move() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let p = root.join("cache");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("f"), b"x").unwrap();
    let empty = root.join("empty-bin");
    std::fs::create_dir_all(&empty).unwrap();
    // SAFETY: this process has exactly this one test. swamp never
    // searches PATH (#199), so "no lsof" is an empty fake-program
    // directory, not an empty PATH.
    unsafe { std::env::set_var("SWAMP_TEST_PROGRAM_DIR", &empty) };
    let t = swamp_core::reclaim_trash::ReclaimTarget::for_path(
        p.clone(),
        "cache",
        Some(1),
        None,
        Vec::new(),
    );
    let store = root.join("store");
    std::fs::create_dir_all(&store).unwrap();
    let r = swamp_core::occupancy::OccupancySnapshot::scoped(|| {
        swamp_core::reclaim_trash::review(&t, Some(&store), Some(&root))
    })
    .expect("missing lsof is a line on the confirm, not a refusal");
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("could not be checked") || w.contains("unresolved")),
        "no lsof, and the confirm reads as if nothing holds it: {:#?}",
        r.warnings
    );
}
