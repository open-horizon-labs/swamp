//! #102: an independent canary sweep of the TUI's own rendered frames,
//! driven by REAL identification output (`swamp_core::agents::
//! discover_and_measure` over a synthetic, on-disk Claude Code fixture),
//! not a hand-built `AgentUnit` literal the way `frames.rs`'s own
//! `agents_view` test does. `frames.rs` proves the Agents view's layout;
//! this file proves that a real session's own transcript content never
//! reaches the rendered frame, in either the flat Agents view or the
//! project tree's new collapsed "Agent storage (linked)" row (#100).
//! PRIVACY IS A HARD RULE: the fixture below is synthetic, never a real
//! `~/.claude` directory.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::PathBuf;
use swamp_core::report::{Report, report_full_mode};
use swamp_tui::app::{App, ViewKind};
use swamp_tui::ui;

const CANARY: &str = "CANARY-TUI-FRAME-MUST-NEVER-SHOW-9c41";

fn write(path: &std::path::Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn capture(app: &App, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    terminal.backend().to_string()
}

/// Builds a real, on-disk Claude Code fixture whose session transcript
/// carries `CANARY` in its own message content (never in any path/name
/// the adapter's identification logic looks at), runs the real
/// `discover_and_measure`, and returns a real walked `Report` (via
/// `report_full_mode`, the same entry point the CLI/TUI actually use)
/// over the fixture's own project directory, plus the agent units.
fn real_units_with_canary_content() -> (Report, PathBuf, Vec<swamp_core::agents::AgentUnit>) {
    use swamp_core::locations::{Environment, Platform, Registry};
    use swamp_core::scope::{ScanConfig, resolve_effective_scope};

    let home_root = tempfile::tempdir().unwrap().keep();
    let claude_home = home_root.join("claude-home");
    let repo = home_root.join("fixture-repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let session_id = "44444444-4444-4444-8444-444444444444";
    let jsonl = claude_home
        .join("projects")
        .join("-fixture-repo-encoded")
        .join(format!("{session_id}.jsonl"));
    write(
        &jsonl,
        format!(
            "{{\"type\":\"user\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"gitBranch\":\"main\",\"message\":{{\"role\":\"user\",\"content\":\"{CANARY}\"}}}}\n",
            repo.display()
        )
        .as_bytes(),
    );

    let mut env_vars = std::collections::HashMap::new();
    env_vars.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        claude_home.display().to_string(),
    );
    let env = Environment::fixture(home_root.clone(), env_vars, Platform::MacOS);
    let registry = Registry::with_builtins();
    let cfg = ScanConfig {
        defaults: false,
        include: Vec::new(),
        exclude: Vec::new(),
        // An allow-list: see the note in
        // `crates/core/tests/external_units.rs`'s escaping-detector
        // test. A deny-list of the other agent detectors left
        // `core-simulator`, `homebrew` and `ruby-install` reaching real
        // machine-wide paths from a fixture.
        disabled_detectors: Vec::new(),
        enabled_detectors: vec!["claude-code".to_string()],
    };
    let scope = resolve_effective_scope(&env, &cfg, &[], &registry, 1_000);
    let store = tempfile::tempdir().unwrap();
    let units = swamp_core::agents::discover_and_measure(
        &scope,
        std::slice::from_ref(&repo),
        Some(store.path()),
        false,
        1_000,
        30,
        3600,
        &swamp_core::fs_events::EventCoverage::untrusted(),
    )
    .unwrap();
    assert!(
        units.iter().any(|u| u.path == jsonl),
        "fixture session must actually be identified"
    );

    let report = report_full_mode(
        &home_root,
        None,
        false,
        Some(store.path()),
        None,
        false,
        false,
        false,
        false,
    )
    .unwrap();
    assert!(
        report.projects.iter().any(|p| p.name == "fixture-repo"),
        "the walked report must contain the fixture project: {:?}",
        report.projects.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    (report, home_root, units)
}

#[test]
fn agents_view_frame_never_renders_real_session_content() {
    let (report, _root, units) = real_units_with_canary_content();
    let mut app = App::new(report, "/tmp".into());
    app.set_agent_units(units);
    app.set_view(ViewKind::Agents);
    for (w, h) in [(80, 24), (200, 60)] {
        let frame = capture(&app, w, h);
        assert!(
            !frame.contains(CANARY),
            "Agents view frame at {w}x{h} leaked session content:\n{frame}"
        );
    }
}

#[test]
fn project_tree_frame_with_the_linked_agent_storage_row_never_renders_session_content() {
    let (report, _root, units) = real_units_with_canary_content();
    let mut app = App::new(report, "/tmp".into());
    app.set_agent_units(units);
    app.set_view(ViewKind::Tree);
    app.selected_project = Some("fixture-repo".to_string());
    for (w, h) in [(80, 24), (200, 60)] {
        let frame = capture(&app, w, h);
        assert!(
            !frame.contains(CANARY),
            "project tree frame at {w}x{h} leaked session content:\n{frame}"
        );
        assert!(
            frame.to_lowercase().contains("agent storage"),
            "the collapsed agent-storage row must actually render for a project with a linked \
             session, at {w}x{h}:\n{frame}"
        );
    }
}
