//! Exact non-Rust paths go through the normal human TUI flow, not a CLI
//! delete command or an invented native prune operation. Disposable fixtures.
use std::{fs, path::Path};
use swamp_core::{
    artifact::NestedActionCapability,
    fs_events::{FsEventsPlan, FsEventsRequest, FsEventsSource},
};
use swamp_tui::{app::App, model};

struct Full;
impl FsEventsSource for Full {
    fn replay(&self, _: &FsEventsRequest) -> FsEventsPlan {
        FsEventsPlan::from_live(vec![], 1000, None)
    }
}
fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn node_framework_cache_can_be_marked_without_selecting_served_output_or_dependencies() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fs::canonicalize(tmp.path()).unwrap().join("web");
    fs::create_dir_all(&repo).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    write(&repo.join("package.json"), br#"{"name":"web"}"#);
    write(&repo.join(".gitignore"), b".next/\nnode_modules/\n");
    write(&repo.join(".next/cache/compiler/blob"), b"rebuildable");
    write(&repo.join(".next/server/page.js"), b"served-output");
    write(&repo.join("node_modules/dependency/index.js"), b"retained");
    let store = tempfile::tempdir().unwrap();
    let mut report = swamp_core::report::report_full_mode_scoped(
        &repo,
        None,
        false,
        Some(store.path()),
        Some("1h"),
        true,
        false,
        false,
        true,
        &Full,
        &[],
        false,
    )
    .unwrap();
    let cache = repo.join(".next/cache");
    let identified = report
        .nested_artifacts
        .iter()
        .find(|u| u.path == cache)
        .unwrap();
    assert_eq!(identified.action, NestedActionCapability::TrashPath);
    let plan = swamp_core::actions::propose_checking_protection(
        &report,
        None,
        std::slice::from_ref(&cache),
        "test",
        &[],
    )
    .unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].path(), cache);
    assert!(plan[0].cargo_group().is_none());
    assert!(plan[0].recovery().contains("next build"));

    // A nested cache already included in an output parent's bytes must not
    // sneak into this category's selection when it isn't displayed there.
    let mut output_parent = identified.clone();
    output_parent.path = repo.join(".next/server");
    output_parent.id = "fixture-output-parent".into();
    output_parent.role = swamp_core::artifact::ArtifactRole::Output;
    let mut hidden_cache = identified.clone();
    hidden_cache.path = repo.join(".next/server/internal-cache");
    hidden_cache.id = "fixture-hidden-cache".into();
    report
        .nested_artifacts
        .extend([output_parent, hidden_cache]);

    let rows = model::tree_rows(
        &report,
        &report.projects[0].name,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    );
    let key = format!("family-open:{}:intermediates", repo.join(".next").display());
    let row = rows
        .iter()
        .find(|r| r.expansion_key.as_deref() == Some(&key))
        .expect("actual TUI cache group");
    let mut app = App::new(report, repo.clone());
    app.mark_row(row);
    assert_eq!(app.marked.len(), 1);
    let marked = app.marked.values().next().unwrap();
    assert_eq!(marked.path, cache);
    assert!(marked.cargo_unit.is_none());
    assert!(marked.warnings.iter().any(|w| w.contains("next build")));
    // Exercise the same exact-path Trash primitive the TUI uses. A sibling
    // and installed dependencies must survive; no parent fallback is allowed.
    let trash = tmp.path().join("trash");
    fs::create_dir_all(&trash).unwrap();
    let ledger = swamp_core::ledger::Ledger::open(store.path().join("ledger.parquet")).unwrap();
    let results = swamp_tui::actions::execute_plan(
        &app.marked.values().cloned().collect::<Vec<_>>(),
        &ledger,
        &trash,
        false,
    );
    assert_eq!(results.len(), 1);
    assert!(results[0].outcome.is_ok(), "{:?}", results[0]);
    assert_eq!(ledger.all().unwrap().len(), 1);
    assert!(!cache.exists());
    assert_eq!(
        fs::read(repo.join(".next/server/page.js")).unwrap(),
        b"served-output"
    );
    assert!(repo.join("node_modules/dependency/index.js").exists());
    write(&cache.join("compiler/blob"), b"rebuilt-fixture");
    assert!(cache.join("compiler/blob").exists());
}
