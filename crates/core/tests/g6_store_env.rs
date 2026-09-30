//! v0.8.0 G6 review M3: the store SWAMP_DIR names (and so a preview
//! store) is refused with its ancestors. One test in its own process, so
//! the environment variable cannot leak into another.

use std::path::Path;

use swamp_core::drilldown::ChildKind;
use swamp_core::reclaim_trash::{ReclaimTarget, review};

/// Tempting wrong patch: only the `store` argument is protected, so a TUI
/// started with a different SWAMP_DIR (the preview store) lets `~/.local`
/// move while its real store is inside. The resolved store counts.
#[test]
fn swamp_dir_override_store_and_its_ancestors_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let store = root.join("home/.local/share/swamp-preview");
    std::fs::create_dir_all(&store).unwrap();
    // SAFETY: this process has exactly this one test.
    unsafe { std::env::set_var("SWAMP_DIR", &store) };
    let _ = ChildKind::Entry;
    for p in [store.clone(), root.join("home/.local")] {
        let t = ReclaimTarget::for_path(p.clone(), "cache", Some(1), None, Vec::new());
        let err = review(
            &t,
            Some(Path::new("/nonexistent-store")),
            Some(&root.join("home")),
        )
        .unwrap_err();
        assert!(err.contains("swamp's own ledger"), "{}: {err}", p.display());
    }
}
