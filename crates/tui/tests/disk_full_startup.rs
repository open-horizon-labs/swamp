//! With the store's volume nearly full, `swamp ui` must not block on a
//! walk: it paints the last stored report plus a banner, starts no
//! observation worker and no live watch, and writes nothing.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};
use swamp_tui::ui;

fn scope_for(home: &Path, include: &Path) -> EffectiveScope {
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: vec![include.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(home.to_path_buf(), HashMap::new(), Platform::MacOS);
    resolve_effective_scope(&env, &cfg, &[], &registry, 1_000)
}

fn listing(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let m = e.metadata().unwrap();
            out.push(format!(
                "{} {} {:?}",
                e.path().strip_prefix(base).unwrap().display(),
                m.len(),
                m.modified().unwrap()
            ));
            if m.is_dir() {
                walk(base, &e.path(), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn startup_on_a_full_disk_shows_the_stored_report_and_banner_without_observing() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let proj = root.path().join("proj");
    fs::create_dir_all(proj.join("target/debug")).unwrap();
    fs::write(proj.join("target/debug/blob.bin"), vec![b'x'; 4096]).unwrap();
    fs::write(proj.join("Cargo.toml"), b"[package]\nname = \"x\"\n").unwrap();
    let scope = scope_for(home.path(), root.path());
    swamp_core::report::observe_scope(
        &scope,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        Some(store.path()),
        None,
        true,
        true,
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )
    .expect("seed observation");

    let before = listing(store.path());
    // A file that appears under the root after the stored observation:
    // a walk would find it, the stored read must not.
    fs::write(proj.join("target/debug/late.bin"), vec![b'y'; 8192]).unwrap();

    let app = swamp_tui::app_from_stored_multi_root(
        &scope,
        store.path(),
        "disk nearly full: refresh skipped (1.9 GiB free)".into(),
    )
    .expect("stored report");

    assert!(
        app.pending.is_none(),
        "no observation worker may be started"
    );
    assert!(app.observing.is_none());
    assert!(app.watches.is_empty(), "no live watch on a full disk");
    assert_eq!(listing(store.path()), before, "startup wrote to the store");

    let backend = TestBackend::new(200, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::draw(f, &app)).unwrap();
    let frame = terminal.backend().to_string();
    assert!(
        frame.contains("disk nearly full: refresh skipped (1.9 GiB free)"),
        "{frame}"
    );
    assert!(
        frame.contains("proj"),
        "stored report rows missing: {frame}"
    );
    assert!(!frame.contains("observing"), "{frame}");
}

#[test]
fn no_stored_report_yields_none_rather_than_walking() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let scope = scope_for(home.path(), root.path());
    assert!(swamp_tui::app_from_stored_multi_root(&scope, store.path(), "b".into()).is_none());
    assert!(listing(store.path()).is_empty());
}
