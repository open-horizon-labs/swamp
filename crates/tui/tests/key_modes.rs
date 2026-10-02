//! Cross-mode key invariants. These fixtures are synthetic and the tests
//! never start an observer or execute a Trash operation.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swamp_core::report::Report;
use swamp_tui::{
    actions::MarkedUnit,
    app::{App, ViewKind},
    handle_key, handle_terminal_key,
};

fn app() -> App {
    let root = PathBuf::from("/synthetic/swamp-key-modes");
    let mut app = App::new(Report::empty(root.clone()), root);
    app.clear_filter();
    app
}

fn app_with_project() -> App {
    let root = PathBuf::from("/synthetic/swamp-key-modes");
    let mut report = Report::empty(root.clone());
    report.projects.push(swamp_core::report::ProjectRow {
        project_id: "synthetic-project".into(),
        name: "synthetic".into(),
        remote: None,
        ecosystems: Vec::new(),
        worktrees: vec![swamp_core::report::WorktreeRow {
            worktree_id: "synthetic-worktree".into(),
            path: root.clone(),
            kind: swamp_core::report::WorktreeKind::Main,
            artifacts: Vec::new(),
            signals: Vec::new(),
            branch: None,
            github: None,
            merge_complete: None,
            idle_secs: None,
        }],
    });
    let mut app = App::new(report, root);
    app.clear_filter();
    app
}

fn marked(path: &str) -> MarkedUnit {
    MarkedUnit {
        cargo_unit: None,
        agent_unit: None,
        session_members: None,
        reclaim: None,
        path: path.into(),
        docker: None,
        worktree_path: path.into(),
        bytes: 1,
        observed_at: 1,
        worktree: None,
        label: "synthetic unit".into(),
        warnings: Vec::new(),
    }
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

#[test]
fn help_consumes_main_view_and_destructive_keys() {
    let mut app = app();
    app.help_open = true;
    for code in [
        KeyCode::Char('A'),
        KeyCode::Backspace,
        KeyCode::Char('/'),
        KeyCode::Char(':'),
        KeyCode::Char('v'),
        KeyCode::Char('2'),
    ] {
        handle_key(&mut app, code);
        assert!(app.help_open, "{code:?} escaped help");
        assert_eq!(app.view, ViewKind::Projects);
        assert!(app.marked.is_empty());
        assert!(app.operation.is_none());
        assert!(!app.quit);
    }
    handle_key(&mut app, KeyCode::Char('q'));
    assert!(
        !app.help_open && !app.quit,
        "q closes help without quitting"
    );
}

#[test]
fn raw_filter_treats_command_letters_as_draft_text() {
    let mut app = app();
    app.start_filter_edit();
    for c in ['q', 'A', 'R', 'a'] {
        handle_key(&mut app, KeyCode::Char(c));
    }
    assert!(app.editing_filter);
    assert_eq!(app.filter_text, "qARa");
    assert_eq!(app.view, ViewKind::Projects);
    assert!(app.marked.is_empty());
    assert!(app.operation.is_none());
    assert!(!app.quit);
}

#[test]
fn picker_keeps_navigation_and_destructive_keys_inside_picker() {
    let mut app = app();
    app.open_picker();
    let before = app.picker.clone().expect("picker opened");
    for code in [
        KeyCode::Char('A'),
        KeyCode::Char('R'),
        KeyCode::Char('v'),
        KeyCode::Char('2'),
        KeyCode::Right,
    ] {
        handle_key(&mut app, code);
        assert!(app.picker.is_some(), "{code:?} escaped picker");
        assert_eq!(app.view, ViewKind::Projects);
        assert!(app.marked.is_empty());
        assert!(app.operation.is_none());
        assert!(!app.quit);
    }
    // Right advances the current picker choice. The other keys above left
    // the picker open and did not dispatch their main-view meanings.
    assert_ne!(app.picker.as_ref().unwrap().size_ix, before.size_ix);
}

#[test]
fn picker_q_closes_outside_project_field_but_project_field_keeps_reserved_text() {
    let mut close_picker = app();
    close_picker.open_picker();
    handle_key(&mut close_picker, KeyCode::Char('q'));
    assert!(close_picker.picker.is_none());
    assert_eq!(close_picker.view, ViewKind::Projects);
    assert!(!close_picker.quit);
    assert!(close_picker.operation.is_none());

    let mut project_field = app();
    project_field.open_picker();
    project_field.picker.as_mut().unwrap().field = 3;
    for code in [
        KeyCode::Char('q'),
        KeyCode::Char('e'),
        KeyCode::Char('0'),
        KeyCode::Char(' '),
    ] {
        handle_key(&mut project_field, code);
        assert!(
            project_field.picker.is_some(),
            "{code:?} closed project input"
        );
        assert_eq!(project_field.view, ViewKind::Projects);
        assert!(project_field.operation.is_none());
        assert!(!project_field.quit);
    }
    assert_eq!(
        project_field.picker.as_ref().unwrap().project_query,
        "qe0 ",
        "reserved picker keys remain ordinary project-query text"
    );
}

#[test]
fn blocked_and_cargo_inspection_modes_do_not_dispatch_main_commands() {
    let mut blocked = app();
    blocked.blocked_open = true;
    for code in [KeyCode::Char('A'), KeyCode::Char('/'), KeyCode::Char('2')] {
        handle_key(&mut blocked, code);
        assert!(blocked.blocked_open, "{code:?} escaped the blocked list");
        assert_eq!(blocked.view, ViewKind::Projects);
        assert!(blocked.marked.is_empty());
        assert!(blocked.operation.is_none());
        assert!(!blocked.quit);
    }
    handle_key(&mut blocked, KeyCode::Char('q'));
    assert!(!blocked.blocked_open && !blocked.quit);

    let mut cargo = app();
    cargo.cargo_inspection = Some(vec!["synthetic cargo detail".into()]);
    for code in [
        KeyCode::Char('A'),
        KeyCode::Char('/'),
        KeyCode::Char('2'),
        KeyCode::Char('R'),
    ] {
        handle_key(&mut cargo, code);
        assert!(
            cargo.cargo_inspection.is_some(),
            "{code:?} escaped Cargo inspection"
        );
        assert_eq!(cargo.view, ViewKind::Projects);
        assert!(cargo.marked.is_empty());
        assert!(cargo.operation.is_none());
        assert!(!cargo.quit);
    }
    handle_key(&mut cargo, KeyCode::Char('q'));
    assert!(cargo.cargo_inspection.is_none() && !cargo.quit);
}

#[test]
fn trash_summary_and_inventory_cannot_execute_before_review_is_seen() {
    let mut app = app();
    let unit = marked("/synthetic/swamp-key-modes/review-only");
    app.marked.insert(unit.path.display().to_string(), unit);
    app.open_confirm();

    // No draw has recorded any part of the decision summary as seen.
    handle_key(&mut app, KeyCode::Enter);
    assert!(app.confirm_open);
    assert!(!app.confirm_details_open);
    assert!(app.operation.is_none());
    assert_eq!(app.marked.len(), 1);

    // The optional path inventory is also read-only; Enter there returns
    // no authorization and cannot start the operation.
    handle_key(&mut app, KeyCode::Char('l'));
    assert!(app.confirm_details_open);
    handle_key(&mut app, KeyCode::Enter);
    assert!(app.confirm_open && app.confirm_details_open);
    assert!(app.operation.is_none());
    assert_eq!(app.marked.len(), 1);
}

#[test]
fn modified_commands_do_not_fall_through_to_unmodified_actions() {
    let modifiers = [
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SUPER,
    ];
    for modifier in modifiers {
        for code in [
            KeyCode::Backspace,
            KeyCode::Char('A'),
            KeyCode::Char('q'),
            KeyCode::Char('R'),
        ] {
            let mut app = app();
            // If R were dispatched, refresh_now would report the existing
            // observer and alter visible status; it cannot start a scan.
            app.external_observer = Some(swamp_core::schedule::LockHolder {
                pid: u32::MAX,
                since: 1,
            });
            let view = app.view;
            let marks = app.marked.len();
            let status = app.status.clone();
            let last_result = app.last_result.clone();
            handle_terminal_key(&mut app, key(code, modifier));
            assert_eq!(app.view, view, "{modifier:?}+{code:?} changed view");
            assert_eq!(
                app.marked.len(),
                marks,
                "{modifier:?}+{code:?} changed marks"
            );
            assert_eq!(
                app.status, status,
                "{modifier:?}+{code:?} dispatched a command"
            );
            assert_eq!(
                app.last_result, last_result,
                "{modifier:?}+{code:?} changed the visible result"
            );
            assert!(
                app.operation.is_none(),
                "{modifier:?}+{code:?} started work"
            );
            assert!(
                !app.confirm_open,
                "{modifier:?}+{code:?} opened confirmation"
            );
            assert!(!app.quit, "{modifier:?}+{code:?} quit");
        }

        // Enter would drill into this synthetic project if the modifier
        // were discarded; the assertion observes navigation without any
        // filesystem-backed row or worker.
        let mut populated = app_with_project();
        handle_terminal_key(&mut populated, key(KeyCode::Enter, modifier));
        assert_eq!(
            populated.view,
            ViewKind::Projects,
            "{modifier:?}+Enter drilled in"
        );
        assert!(populated.operation.is_none());
        assert!(!populated.confirm_open && !populated.quit);
    }
}

#[test]
fn deliberate_control_c_and_supported_shift_events_keep_their_meaning() {
    let mut quit = app();
    handle_terminal_key(&mut quit, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(quit.quit);

    // Shift+Char('A') is a real terminal event. In a raw draft it inserts
    // the uppercase character instead of invoking bulk mark.
    let mut upper = app();
    upper.start_filter_edit();
    handle_terminal_key(&mut upper, key(KeyCode::Char('A'), KeyModifiers::SHIFT));
    assert!(upper.editing_filter);
    assert_eq!(upper.filter_text, "A");
    assert!(upper.operation.is_none());
    assert!(!upper.quit);

    let mut reverse_section = app();
    handle_terminal_key(
        &mut reverse_section,
        key(KeyCode::BackTab, KeyModifiers::SHIFT),
    );
    assert_eq!(
        reverse_section.view.section(),
        swamp_tui::app::Section::Disk
    );
    assert!(reverse_section.operation.is_none());
}
