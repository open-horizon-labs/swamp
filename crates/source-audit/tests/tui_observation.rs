//! Mutation checks for the UI observation boundary, including worker wrappers.
use swamp_source_audit::{audits, model::Workspace};

fn audit(source: &str) -> Result<(), String> {
    let tmp = tempfile::tempdir().unwrap();
    for (path, text) in [
        (
            "crates/core/src/lib.rs",
            "pub mod report { pub fn observe_scope() {} }",
        ),
        ("crates/cli/src/main.rs", "fn main() {}"),
        ("crates/tui/src/lib.rs", source),
    ] {
        let path = tmp.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let ws = Workspace::load(tmp.path());
    assert!(
        ws.parse_errors.is_empty(),
        "fixtures must parse: {:?}",
        ws.parse_errors
    );
    audits::run("tui_observation_is_explicit", tmp.path())
}

#[test]
fn rejects_delete_render_helpers_workers_aliases_and_function_values() {
    for source in [
        "mod app { struct App; impl App { fn finish_delete(&mut self) { self.observe_in_background(); } } }",
        "mod app { struct App; impl App { fn finish_delete(&mut self) { self.refresh_now(); } } }",
        "mod app { struct App; impl App { fn finish_delete(&mut self) { Self::refresh_now(self); } } }",
        "fn draw() { swamp_core::report::observe_scope(); }",
        "fn helper() { swamp_core::report::report_full(); }",
        "fn helper() { use swamp_core::report::report_scope_with_parts as scan; scan(); }",
        "fn helper() { worker::spawn(|| swamp_core::walk::walk()); }",
        "fn helper() { worker::spawn(|| swamp_core::report::observe_scope()); }",
        "fn helper() { use swamp_core::report::observe_scope as rescan; rescan(); }",
        "fn helper() { let rescan = swamp_core::report::observe_scope; rescan(); }",
        "fn helper() { let rescan = app::App::refresh_now; rescan(); }",
        "fn helper() { invoke!(swamp_core::report::observe_scope()); }",
    ] {
        let error = audit(source).expect_err(source);
        assert!(
            error.contains("explicit observation boundary"),
            "{source}: {error}"
        );
    }
}

#[test]
fn accepts_explicit_refresh_first_startup_and_test_fixtures() {
    audit(r#"
        mod app {
            pub struct App;
            impl App {
                fn observe_in_background(&mut self) { worker::spawn(|| swamp_core::report::observe_scope()); }
                fn refresh_now(&mut self) { self.observe_in_background(); }
                fn scan_if_no_index(&mut self) { self.observe_in_background(); }
                fn finish_delete(&mut self) { self.prune_removed(); }
            }
        }
        fn handle_key_mod(app: &mut app::App) { app.refresh_now(); }
        fn start_background_services(app: &mut app::App) { app.scan_if_no_index(); }
        fn stored_report() { swamp_core::report::report_scope_from_store(); }
        #[cfg(test)] mod tests { fn fixture() { swamp_core::report::observe_scope(); } }
    "#).unwrap();
}

#[test]
fn real_ui_obeys_registered_observation_guard() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    audits::run("tui_observation_is_explicit", root).unwrap();
}
