//! `swamp ui`: the diffstat-ledger terminal UI. See DESIGN.md and
//! `.impeccable/surfaces/tui.md` (binding). Renders the same
//! `swamp_core::report_with` `Report` the CLI uses; no second
//! data path.

#![cfg_attr(
    not(test),
    deny(clippy::disallowed_methods, clippy::disallowed_types, unsafe_code)
)]

pub mod actions;
pub mod app;
pub mod filter;
pub mod model;
pub mod picker;
pub mod ui;
pub mod units;
pub mod worker;

use anyhow::Result;
use app::{App, ViewKind};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use model::Sort;
use ratatui::Terminal;
use ratatui::backend::Backend;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Dispatches one key event against the app state. Kept separate from
/// the terminal event loop so it is directly unit-testable.
pub fn handle_key(app: &mut App, code: KeyCode) {
    handle_key_mod(app, code, false)
}

pub fn handle_terminal_key(app: &mut App, key: crossterm::event::KeyEvent) {
    if key.code == KeyCode::Char('c')
        && key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        if app.operation.is_some() {
            app.cancel_operation();
        } else {
            app.quit = true;
        }
        return;
    }
    handle_key_mod(
        app,
        key.code,
        key.modifiers
            .contains(crossterm::event::KeyModifiers::SHIFT),
    );
}

/// `shift` distinguishes Shift-→/Shift-← inside the picker's growth field.
pub fn handle_key_mod(app: &mut App, code: KeyCode, _shift: bool) {
    if app.operation.is_some() {
        if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
            app.cancel_operation();
        }
        return;
    }
    if let Some(lines) = &app.cargo_inspection {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => app.cargo_inspection = None,
            KeyCode::Down => {
                app.cargo_inspection_scroll = app
                    .cargo_inspection_scroll
                    .saturating_add(1)
                    .min(lines.len().saturating_sub(1).min(u16::MAX as usize) as u16)
            }
            KeyCode::Up => {
                app.cargo_inspection_scroll = app.cargo_inspection_scroll.saturating_sub(1)
            }
            _ => {}
        }
        return;
    }
    if let Some(p) = app.picker.as_mut() {
        match code {
            KeyCode::Up => p.up(),
            KeyCode::Down => p.down(),
            KeyCode::Char(' ') => p.flip_op(),
            KeyCode::Right => p.cycle(1),
            KeyCode::Left => p.cycle(-1),
            KeyCode::Backspace => p.backspace(),
            KeyCode::Enter => app.apply_picker(),
            KeyCode::Esc => app.picker = None,
            KeyCode::Char('e') if p.field != 3 => app.picker_to_raw_edit(),
            KeyCode::Char(c) if p.field == 3 => p.type_char(c),
            KeyCode::Char('0') => {
                app.picker = None;
                app.clear_filter();
            }
            _ => {}
        }
        return;
    }
    if app.editing_filter {
        match code {
            KeyCode::Enter => app.commit_filter(),
            KeyCode::Esc => app.cancel_filter_edit(),
            KeyCode::Backspace => app.filter_backspace(),
            KeyCode::Tab => app.filter_tab_complete(),
            KeyCode::Char(c) => app.filter_input(c),
            _ => {}
        }
        return;
    }
    if app.help_open {
        if matches!(code, KeyCode::Char('?') | KeyCode::Esc) {
            app.toggle_help();
        }
        return;
    }
    match code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Up => app.move_selection(-1),
        KeyCode::Down => app.move_selection(1),
        // Traversal, the way every file tree does it: right goes in,
        // left comes back out. Enter and Esc still do the same, so the
        // muscle memory either way works.
        KeyCode::Right => app.enter_row(),
        KeyCode::Left => app.leave_row(),
        KeyCode::Enter => app.drill_into_selected(),
        KeyCode::Esc => {
            if app.confirm_open {
                app.cancel_confirm();
            } else if app.view != app::ViewKind::Projects {
                app.set_view(app::ViewKind::Projects);
            }
        }
        KeyCode::Char(' ') => app.review_in_background(false, false),
        // Shift-A, not `a`: `a` sorts by age, and a key that means two
        // things depending on state is a key nobody trusts.
        KeyCode::Char('A') => app.review_in_background(true, true),
        KeyCode::Backspace => app.review_in_background(false, true),
        KeyCode::Char('/') => app.open_picker(),
        KeyCode::Char(':') => app.start_filter_edit(),
        KeyCode::Char('0') => app.clear_filter(),
        KeyCode::Char('v') => app.set_view(app.view.next()),
        KeyCode::Char(d @ '1'..='9') => {
            if let Some(v) = ViewKind::from_digit(d) {
                app.set_view(v);
            }
        }
        KeyCode::Char('g') => app.set_sort(Sort::Growth),
        KeyCode::Char('s') => app.set_sort(Sort::Size),
        KeyCode::Char('n') => app.set_sort(Sort::Name),
        KeyCode::Char('t') => app.set_sort(Sort::Type),
        KeyCode::Char('a') => app.set_sort(Sort::Age),
        KeyCode::Char('r') => app.toggle_reverse(),
        KeyCode::Char('k') => app.toggle_keep_executables(),
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Char('i') => app.inspect_selected_cargo_profile(),
        _ => {}
    }
}

/// Runs the interactive UI against `root`.
/// How far back the store can answer for `root`'s volume. Growth windows
/// are bounded by it: a 7d window over 4h of observations would report a
/// week of growth that was never observed.
fn history_span(store: &std::path::Path, root: &std::path::Path) -> Option<u64> {
    let now = swamp_core::entities::now();
    let dev = swamp_core::fs_gate::device_of(root)?;
    let dir = store.join(dev.to_string());
    swamp_core::growth::history_span_secs(&dir, now)
}

/// The authorized scope for this invocation, resolved once from the
/// stored config plus whatever explicit roots the command named.
///
/// `explicit_roots` is passed through rather than dropped: re-resolving
/// with an empty explicit-root list is how `swamp ui <one-project>`
/// quietly widened itself back out to the whole configured catalog for
/// external/agent discovery (the review's scope finding). `None` when
/// there is no readable config, which every caller treats as "cannot
/// observe" rather than "observe everything".
fn resolved_scope(
    store: &Path,
    explicit_roots: &[PathBuf],
) -> Option<swamp_core::scope::EffectiveScope> {
    let cfg = swamp_core::growth::load_config_checked(store).ok()?;
    let env = swamp_core::locations::Environment::from_process();
    let registry = swamp_core::locations::Registry::with_builtins();
    Some(swamp_core::scope::resolve_effective_scope(
        &env,
        &cfg.scan,
        explicit_roots,
        &registry,
        swamp_core::entities::now(),
    ))
}

/// The resolved swamp dir: the gate's one resolver.
fn store_dir() -> PathBuf {
    swamp_core::fs_gate::StoreDir::resolved()
        .path()
        .to_path_buf()
}

pub fn run(root: &Path) -> Result<()> {
    let store = store_dir();
    let root = swamp_core::fs_gate::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // The authorized scope for this invocation, resolved once with the
    // explicit root the user named. Every observation below -- the
    // startup walk, the background refresh, later live refreshes --
    // goes through it, so an exclusion applies to all of them and not
    // just to whichever one happened to be written first
    // (`.oh/guardrails/tui-refresh-preserves-scope.md`).
    let scope = resolved_scope(&store, std::slice::from_ref(&root));
    // Paint the last stored observation immediately (milliseconds, a
    // read of stored Parquet tables -- no walk); observe in the
    // background and swap the result in. With no stored observation yet,
    // the first observation has to happen before there is anything to
    // show (R12: the TUI never walks to produce its own instant paint,
    // same discipline as `swamp report`).
    let cached = scope
        .as_ref()
        .and_then(|s| swamp_core::report::report_scope_from_store(s, &store).ok());
    let mut app = match cached {
        Some(snapshot) => {
            let mut a = App::new(snapshot.report, root.clone());
            a.observed_label = "from last observation".into();
            a.observing = Some((0, 0));
            a.scope = scope.clone();
            a.set_external_units(snapshot.external_units);
            a.set_store_interiors(snapshot.store_interiors);
            a.set_agent_units(snapshot.agent_units);
            let (tx, rx) = std::sync::mpsc::channel();
            let (scope2, store2) = (scope.clone(), store.clone());
            let (roots, cache) = (a.roots.clone(), a.reports_by_root.clone());
            crate::worker::spawn(move || {
                let Some(scope2) = scope2 else {
                    let _ = tx.send(Ok(app::RefreshedObservation::merged_on_worker(
                        &roots,
                        cache,
                        Vec::new(),
                        None,
                        None,
                        None,
                    )));
                    return;
                };
                // include_dirs: the Source row expands into its own
                // directories, so the startup observe must produce them
                // too or the first report shows `source` with no children.
                let res = swamp_core::report::observe_scope(
                    &scope2,
                    swamp_core::report::ObservationParts::ALL,
                    None,
                    None,
                    false,
                    Some(&store2),
                    None,
                    true,
                    true,
                    false,
                    false,
                    swamp_core::fs_events::platform_source().as_ref(),
                    30,
                    24 * 3600,
                )
                .map(|o| {
                    app::RefreshedObservation::merged_on_worker(
                        &roots,
                        cache,
                        o.per_root.into_iter().collect(),
                        Some(o.external_units),
                        Some(o.agent_units),
                        Some(o.store_interiors),
                    )
                });
                let _ = tx.send(res);
            });
            a.pending = Some(rx);
            a
        }
        None => {
            let scope_now = scope.clone().ok_or_else(|| {
                anyhow::anyhow!("could not resolve a scope for {}", root.display())
            })?;
            let observation = swamp_core::report::observe_scope(
                &scope_now,
                swamp_core::report::ObservationParts::ALL,
                None,
                None,
                false,
                Some(&store),
                None,
                true,
                true, // include_dirs: Source rows expand into their own directories
                false,
                false,
                swamp_core::fs_events::platform_source().as_ref(),
                30,
                24 * 3600,
            )?;
            let mut a = App::new(observation.merged, root.clone());
            a.reports_by_root = observation.per_root;
            a.set_external_units(observation.external_units);
            a.set_store_interiors(observation.store_interiors);
            a.set_agent_units(observation.agent_units);
            a
        }
    };
    app.scope = scope.clone();
    // The header's coverage clause is about *this report's own* root,
    // not the wider configured-scope catalog `finish_startup` resolves
    // for external/agent-unit discovery (same split the CLI's `report
    // --view external/agents` already has: "works the same whether
    // report is scoped to the configured catalog or an explicit root").
    // `run` always has exactly one explicit root ("explicit roots
    // replace defaults/detectors entirely", #41), so this note is about
    // that one root's own current status, resolved fresh (it can have
    // changed between the walk above and now) rather than reused from
    // `finish_startup`'s wider catalog scope.
    let coverage = swamp_core::growth::load_config_checked(&store)
        .ok()
        .map(|cfg| {
            let env = swamp_core::locations::Environment::from_process();
            let registry = swamp_core::locations::Registry::with_builtins();
            let scope = swamp_core::scope::resolve_effective_scope(
                &env,
                &cfg.scan,
                std::slice::from_ref(&root),
                &registry,
                swamp_core::entities::now(),
            );
            coverage_from_scope(&scope)
        });
    finish_startup(&mut app, &store, coverage.as_deref());
    run_terminal_loop(&mut app)
}

/// Runs the interactive UI over every root a resolved `EffectiveScope`
/// currently walks (#51): `swamp ui` with no explicit root, so
/// project/shared/external/agent-tool storage from any included root
/// is all in one report at once -- including a root with no Git
/// checkout in it at all, which the single-root `run` above (and the
/// pre-#51 `swamp ui` that always picked exactly one present root
/// before even starting the TUI) could never show alongside another
/// root's projects.
///
/// Unlike `run`, this always observes every present root synchronously
/// before opening the TUI (via `report::report_scope_with_parts`, the
/// same coherent multi-root entry point `report`/`observe` use) rather
/// than painting a cached report and refreshing in the background --
/// a deliberate simplification: `report_scope_with_parts` already picks
/// the cheap incremental path per root when nothing changed, so an
/// unchanged multi-root scope opens about as fast as a cached paint
/// would have, without needing a second, parallel "merge fresh results
/// into a cached multi-root report" bootstrap path.
pub fn run_scope(scope: &swamp_core::scope::EffectiveScope) -> Result<()> {
    let store = store_dir();
    let present_roots = scope.scan_paths();
    anyhow::ensure!(
        !present_roots.is_empty(),
        "swamp_tui::run_scope requires at least one present root in scope"
    );
    let observation = swamp_core::report::observe_scope(
        scope,
        swamp_core::report::ObservationParts::ALL,
        None,
        None,
        false,
        Some(&store),
        None,
        true,
        true, // include_dirs: Source rows expand into their own directories
        false,
        false,
        swamp_core::fs_events::platform_source().as_ref(),
        30,
        24 * 3600,
    )?;
    let coverage = observation.coverage;
    let mut app = App::new_multi_root(observation.merged, present_roots);
    app.reports_by_root = observation.per_root;
    app.set_external_units(observation.external_units);
    app.set_store_interiors(observation.store_interiors);
    app.set_agent_units(observation.agent_units);
    app.scope = Some(scope.clone());
    app.observed_label = "just now".into();
    finish_startup(&mut app, &store, Some(&coverage));
    run_terminal_loop(&mut app)
}

/// Maps a resolved `EffectiveScope` to the same `coverage::RootCoverage`
/// shape `report_scope` returns, for the one caller (`run`'s single
/// explicit root) that has a scope but never actually calls
/// `report_scope` itself. `RootStatus::Present` becomes `Complete`
/// (this function has no walk outcome to draw `Partial` from -- an
/// explicit-root `run` never distinguishes that today); every other
/// status maps onto its `RegionStatus` equivalent one-for-one. Nested
/// roots produce no row, matching `report_scope`'s own contract.
fn coverage_from_scope(
    scope: &swamp_core::scope::EffectiveScope,
) -> Vec<swamp_core::coverage::RootCoverage> {
    use swamp_core::coverage::{RegionStatus, RootCoverage};
    use swamp_core::scope::RootStatus;
    scope
        .roots
        .iter()
        .filter_map(|r| {
            let status = match &r.status {
                RootStatus::Present => RegionStatus::Complete,
                RootStatus::Missing => return Some(RootCoverage::missing(r.path.clone())),
                RootStatus::Unreadable { reason } => {
                    return Some(RootCoverage::inaccessible(r.path.clone(), reason.clone()));
                }
                RootStatus::Excluded { .. } => return Some(RootCoverage::excluded(r.path.clone())),
                RootStatus::SkippedAsNested { .. } => return None,
            };
            Some(RootCoverage {
                path: r.path.clone(),
                status,
                walked_total: 0,
                projects: 0,
                mode: String::new(),
            })
        })
        .collect()
}

/// Everything after an `App` has its initial `report`/`roots` set:
/// external/agent-tool storage discovery, the header's coverage note,
/// starting the live watch(es), and restoring persisted UI state.
/// Shared by `run` and `run_scope` so this bookkeeping is never
/// duplicated (or allowed to drift) between the single- and multi-root
/// startup paths.
fn finish_startup(
    app: &mut App,
    store: &Path,
    coverage: Option<&[swamp_core::coverage::RootCoverage]>,
) {
    app.store_dir = Some(store.to_path_buf());
    // External/agent-tool storage is no longer discovered here. It
    // arrives from the same `report::observe_scope` pass that produced
    // the report, and is refreshed by every later observation -- the
    // review found this function was the *only* production caller that
    // ever set those vectors, so the agents view could be arbitrarily
    // stale while the header said the report was live.
    if let Some(c) = coverage {
        app.set_scope_note(c);
    }
    app.start_watch();
    // Multi-root history windows are bounded by the *primary* root's own
    // history for now (`App::root`, `roots[0]`) -- a per-root history
    // bound is a real, named simplification (see this chunk's session
    // note), not a silent one: a multi-root picker can currently offer
    // a window longer than a non-primary root's own store actually has.
    app.history_secs = history_span(store, &app.root);
    let saved = app::load_ui_state(store);
    if !saved.filter.is_empty() {
        app.filter_text = saved.filter;
        app.commit_filter();
    }
    if !saved.sort.is_empty() {
        app.sort = app::sort_from_str(&saved.sort);
    }
    app.reverse = saved.reverse;
    app.keep_executables = saved.keep_executables;
}

fn run_terminal_loop(app: &mut App) -> Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = event_loop(&mut terminal, app);

    // Terminal failure must not abandon an in-flight filesystem move.
    if app.operation.is_some() {
        app.cancel_operation();
        while app.operation.is_some() {
            app.poll_operation();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )?;
    result
}

fn event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    loop {
        app.poll_operation();
        if app.operation.is_none()
            && let Some(rx) = &app.pending
            && let Ok(res) = rx.try_recv()
        {
            app.pending = None;
            app.observing = None;
            match res {
                Ok(fresh) => {
                    // Each re-observed root replaced exactly its own
                    // entry (#51) on the worker, which also merged: a
                    // live/background refresh of one or more roots never
                    // touches any other root's rows, and the event thread
                    // only installs what the worker prepared.
                    app.install_refreshed(fresh);
                    app.observed_label = "just now".into();
                }
                Err(e) => app.status = Some(format!("observation failed: {e}")),
            }
        }
        app.drain_watch();
        if app.operation.is_none() && app.live_observe_due() {
            app.observe_live();
        }
        if let Ok(sz) = terminal.size() {
            app.width = sz.width;
        }
        terminal.draw(|f| ui::draw(f, app))?;
        if app.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            handle_terminal_key(app, key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use swamp_core::report::{Reconciliation, Report};

    fn empty_report() -> Report {
        Report {
            store_dir: None,
            observed_at: 0,
            root: "/root".into(),
            projects: vec![],
            unowned: vec![],
            reconciliation: Reconciliation {
                unique_estimate: None,
                attributed: 0,
                unowned: 0,
                walked_total: 0,
                du_total: None,
                docker_attributed: 0,
                docker_unowned: 0,
            },
            notes: vec![],
            series_by_key: Default::default(),
            total_series: Vec::new(),
            series_window_secs: 0,
            summary: Default::default(),
            dirs_by_worktree: None,
            files_by_worktree: None,
            schedule_line: None,
            github_enrichment: None,
            nested_artifacts: Vec::new(),
        }
    }

    #[test]
    fn slash_edits_existing_filter_text_and_esc_restores_it() {
        let mut app = App::new(empty_report(), "/root".into());
        let before = app.filter_text.clone();
        handle_key(&mut app, KeyCode::Char(':'));
        assert!(app.editing_filter);
        assert_eq!(app.filter_text, before, "existing text stays editable");
        for _ in 0..before.len() {
            handle_key(&mut app, KeyCode::Backspace);
        }
        for c in "kind:BuildOutput".chars() {
            handle_key(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.filter_text, "kind:BuildOutput");
        handle_key(&mut app, KeyCode::Esc);
        assert!(!app.editing_filter);
        assert_eq!(app.filter_text, before, "Esc restores the previous filter");
        handle_key(&mut app, KeyCode::Char(':'));
        for c in " idle > 48h".chars() {
            handle_key(&mut app, KeyCode::Char(c));
        }
        handle_key(&mut app, KeyCode::Enter);
        assert!(!app.editing_filter);
        assert!(app.filter_error.is_none(), "{:?}", app.filter_error);
        assert!(app.filter_text.ends_with("idle > 48h"));
    }

    #[test]
    fn ctrl_c_cancels_busy_operation_but_exits_when_idle() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let mut app = App::new(empty_report(), "/root".into());
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        app.operation = Some(crate::app::Operation {
            label: "Deleting",
            completed: 1,
            total: 3,
            succeeded: 1,
            failed: 0,
            current: "/tmp/fixture".into(),
            started: std::time::Instant::now(),
            cancel: cancel.clone(),
        });
        handle_terminal_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.operation.is_some());
        handle_terminal_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!app.quit, "must wait for current group's durable outcome");
        app.operation = None;
        handle_terminal_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.quit);
    }

    #[test]
    fn slash_opens_picker_and_enter_applies_its_filter() {
        let mut app = App::new(empty_report(), "/root".into());
        app.history_secs = Some(30 * 86_400);
        handle_key(&mut app, KeyCode::Char('/'));
        assert!(app.picker.is_some());
        handle_key(&mut app, KeyCode::Down); // window
        handle_key(&mut app, KeyCode::Down); // kind
        handle_key(&mut app, KeyCode::Right); // build
        handle_key(&mut app, KeyCode::Enter);
        assert!(app.picker.is_none());
        assert_eq!(app.filter_text, "growth > 100MB in 7d kind:BuildOutput");
        assert!(app.filter_error.is_none());
        handle_key(&mut app, KeyCode::Char('/'));
        handle_key(&mut app, KeyCode::Esc);
        assert!(app.picker.is_none());
        assert_eq!(
            app.filter_text, "growth > 100MB in 7d kind:BuildOutput",
            "Esc keeps the applied filter"
        );
    }

    #[test]
    fn header_never_claims_a_window_longer_than_the_history() {
        let mut app = App::new(empty_report(), "/root".into());
        app.width = 200;
        app.history_secs = Some(4 * 3_600);
        app.filter_text = "growth > 100MB in 7d".into();
        app.commit_filter();
        let mut t = ratatui::Terminal::new(TestBackend::new(200, 10)).unwrap();
        t.draw(|f| ui::draw(f, &app)).unwrap();
        let s = t.backend().to_string();
        assert!(s.contains("since 4h (asked 1w; history is 4h)"), "{s}");
    }

    #[test]
    fn narrow_signals_spell_out_the_loud_ones() {
        let sig = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            ui::pick_signals(
                &sig(&[
                    "last commit 1d",
                    "clean",
                    "18 unpushed",
                    "unlocked",
                    "idle 12h"
                ]),
                2
            ),
            sig(&["last commit 1d", "18 unpushed"])
        );
        assert_eq!(
            ui::pick_signals(
                &sig(&["last commit 3h", "clean", "0 unpushed", "unlocked"]),
                2
            ),
            sig(&["last commit 3h"])
        );
        assert_eq!(
            ui::pick_signals(&sig(&["clean", "0 unpushed"]), 2),
            sig(&["clean"])
        );
    }

    #[test]
    fn header_drops_trailing_clauses_to_fit_width() {
        let clauses: Vec<String> = [
            "/root",
            "observed just now",
            "56 projects",
            "36GB attributed",
            "193MB unowned",
            "docker 14GB unowned",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let full = ui::fit_clauses(&clauses, 200);
        assert!(full.ends_with("docker 14GB unowned"));
        let narrow = ui::fit_clauses(&clauses, 50);
        assert!(narrow.chars().count() <= 50, "{narrow:?}");
        assert!(narrow.starts_with("/root · observed just now"));
        assert!(!narrow.contains("docker"));
        assert_eq!(
            ui::fit_clauses(&clauses, 3),
            "…ot",
            "first clause is retained within the terminal-cell budget"
        );
    }

    #[test]
    fn q_quits() {
        let mut app = App::new(empty_report(), "/root".into());
        handle_key(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn draws_without_panicking_at_both_first_class_sizes() {
        for (w, h) in [(80u16, 24u16), (200u16, 60u16)] {
            let backend = TestBackend::new(w, h);
            let mut terminal = Terminal::new(backend).unwrap();
            let app = App::new(empty_report(), "/root".into());
            terminal.draw(|f| ui::draw(f, &app)).unwrap();
        }
    }
}
