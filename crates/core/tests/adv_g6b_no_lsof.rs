//! v0.8.0 G6 verification: the review's open-file snapshot cannot run
//! (no `lsof` on PATH). One test in its own process, so PATH cannot leak.

/// Tempting wrong patch: an open-file probe that could not start reads as
/// "nothing holds it" (no line), so the confirm is silent exactly when
/// swamp did not look. Missing `lsof` is said on the confirm; it is a line,
/// never a refusal.
#[test]
fn adv_b_a_review_without_lsof_says_it_could_not_check_and_still_offers_the_move() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let p = root.join("cache");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("f"), b"x").unwrap();
    let empty = root.join("empty-bin");
    std::fs::create_dir_all(&empty).unwrap();
    // SAFETY: this process has exactly this one test.
    unsafe { std::env::set_var("PATH", &empty) };
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
