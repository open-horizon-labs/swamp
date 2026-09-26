//! CE3 of the stack/10 re-review: the scope-preserving refresh repair
//! keeps the *scope* through a live refresh but throws away the agent and
//! external views while doing it.
//!
//! `App::observe_live` narrows the scope to the one root a watcher fired
//! under (`EffectiveScope::restricted_to`, app.rs:1872) and then asks
//! `observe_scope` for `ObservationParts::ALL`, which returns
//! `agent_units`/`external_units` derived from that *one-root* scope. The
//! event loop applies them with `set_agent_units` / `set_external_units`
//! unconditionally (tui/src/lib.rs:497-502). Every tool home outside the
//! root that happened to change is therefore absent from the narrowed
//! scope, so the agent view empties on the first watch event.
//!
//! `scope_preserving_refresh.rs::external_and_agent_units_are_replaced_by_every_refresh`
//! only exercises `observe_in_background` (full scope) and only asserts
//! the vectors are `Some(_)`, so it passes while this fails.
//!
//! Disposable `tempfile` fixtures only; no real home is read.
//!
//! Run:
//! ```sh
//! cargo test -p swamp-tui --test reviewer_counterexamples_stack2_tui \
//!   --target-dir <scratch>/target-audit -- --nocapture
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use swamp_core::locations::{Environment, Platform, Registry};
use swamp_core::scope::{EffectiveScope, ScanConfig, resolve_effective_scope};
use swamp_tui::app::App;

fn scope_for(home: &Path, claude: &Path, src: &Path) -> EffectiveScope {
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: vec![src.display().to_string()],
        disabled_detectors: registry
            .detectors()
            .iter()
            .map(|d| d.id().to_string())
            .filter(|id| id != "claude-code")
            .collect(),
        ..Default::default()
    };
    let env = Environment::fixture(
        home.to_path_buf(),
        HashMap::from([("CLAUDE_CONFIG_DIR".into(), claude.display().to_string())]),
        Platform::MacOS,
    );
    resolve_effective_scope(&env, &cfg, &[], &registry, 1_000)
}

fn observe(scope: &EffectiveScope, store: &Path) -> swamp_core::report::ScopeObservation {
    swamp_core::report::observe_scope(
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
    .expect("scope observation")
}

#[test]
fn a_live_refresh_of_one_root_must_not_empty_the_agent_view() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = fs::canonicalize(tmp.path()).unwrap();
    let claude = tmp_path.join("claude");
    fs::create_dir_all(claude.join("debug")).unwrap();
    fs::write(claude.join("debug/log.txt"), vec![b'x'; 2048]).unwrap();
    let src = tmp_path.join("src");
    fs::create_dir_all(src.join("p/target/debug")).unwrap();
    fs::write(src.join("p/target/debug/blob.bin"), vec![b'x'; 4096]).unwrap();
    fs::write(src.join("p/Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
    let store = tempfile::tempdir().unwrap();

    let scope = scope_for(&tmp_path, &claude, &src);
    let first = observe(&scope, store.path());
    assert!(
        !first.agent_units.is_empty(),
        "precondition: the agent home must be discovered at startup"
    );
    let at_startup = first.agent_units.len();

    let mut app = App::new_multi_root(first.merged.clone(), scope.scan_paths());
    app.reports_by_root = first.per_root.clone();
    app.store_dir = Some(store.path().to_path_buf());
    app.scope = Some(scope.clone());
    app.set_agent_units(first.agent_units.clone());
    app.set_external_units(first.external_units.clone());

    // A watcher fires for one file under the *project* root only.
    app.live_changes.insert(src.join("p/target/debug/blob.bin"));
    app.live_last_event_id = 1;
    app.observe_live();
    let rx = app.pending.take().expect("a live refresh was started");
    let fresh = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the live worker answered")
        .expect("the live refresh succeeded");
    // Exactly what `tui::lib`'s event loop does with the result.
    if let Some(units) = fresh.agent_units.clone() {
        app.set_agent_units(units);
    }
    if let Some(units) = fresh.external_units.clone() {
        app.set_external_units(units);
    }
    assert_eq!(
        app.agent_units.len(),
        at_startup,
        "a live refresh of an unrelated root emptied the agent view ({at_startup} -> {})",
        app.agent_units.len()
    );
}
