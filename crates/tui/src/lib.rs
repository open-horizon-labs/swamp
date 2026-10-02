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
pub mod detail;
pub mod filter;
pub mod model;
pub mod names;
pub mod picker;
pub mod term;
pub mod tool_sheet;
pub mod ui;
pub mod units;
pub mod worker;

use anyhow::Result;
use app::App;
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
        } else if app
            .tool_sheet
            .as_ref()
            .is_some_and(|s| matches!(s.stage, tool_sheet::Stage::Running(_)))
        {
            // A manager stopped halfway can leave a half-removed install:
            // Ctrl-C waits for it like every other key.
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

/// Where a scrolled view lands after `code`, or `None` for a key that does
/// not scroll. `page` is the rows in one screenful, `last` the furthest
/// first-line index.
fn scrolled(cur: usize, code: KeyCode, page: usize, last: usize) -> Option<usize> {
    Some(match code {
        KeyCode::Down => cur.saturating_add(1).min(last),
        KeyCode::Up => cur.saturating_sub(1),
        KeyCode::PageDown => cur.saturating_add(page.max(1)).min(last),
        KeyCode::PageUp => cur.saturating_sub(page.max(1)),
        KeyCode::Home => 0,
        KeyCode::End => last,
        _ => return None,
    })
}

/// `shift` distinguishes Shift-→/Shift-← inside the picker's growth field.
pub fn handle_key_mod(app: &mut App, code: KeyCode, _shift: bool) {
    // A result stays until the next key, and only a key removes it: no
    // timer repaints the screen while nobody is looking.
    if app.operation.is_none() {
        app.last_result = None;
    }
    if app.operation.is_some() {
        if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
            app.cancel_operation();
        } else if code == KeyCode::Char('R') {
            app.say_refresh_waits();
        }
        return;
    }
    if app.tool_sheet.is_some() {
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => app.tool_back(),
            KeyCode::Up => app.tool_move(-1),
            KeyCode::Down => app.tool_move(1),
            KeyCode::Enter => app.tool_enter(),
            KeyCode::Char('Y') => app.tool_remove_key(),
            KeyCode::Char('R') if !app.tool_sheet.as_ref().is_some_and(|s| s.waiting()) => {
                app.tool_sheet = None;
                app.refresh_now();
            }
            _ => {}
        }
        return;
    }
    if let Some(lines) = &app.cargo_inspection {
        if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
            app.cargo_inspection = None;
        } else if let Some(at) = scrolled(
            app.cargo_inspection_scroll as usize,
            code,
            app.page.get(),
            lines.len().saturating_sub(1).min(u16::MAX as usize),
        ) {
            app.cargo_inspection_scroll = at as u16;
        }
        return;
    }
    if let Some(p) = app.picker.as_mut() {
        match code {
            KeyCode::Up => p.up(),
            KeyCode::Down => p.down(),
            KeyCode::Home | KeyCode::PageUp => p.first(),
            KeyCode::End | KeyCode::PageDown => p.last(),
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
        if matches!(code, KeyCode::Char('?' | 'q') | KeyCode::Esc) {
            app.toggle_help();
        } else if let Some(at) =
            scrolled(app.help_scroll.get(), code, app.page.get(), usize::MAX / 2)
        {
            // The drawer clamps this to the real end of the text.
            app.help_scroll.set(at);
        }
        return;
    }
    if app.blocked_open {
        // The blocked list is read-only: nothing under it can be marked.
        match code {
            KeyCode::Esc | KeyCode::Char('b' | 'd' | 'q') => app.blocked_open = false,
            KeyCode::Char('r') => app.recheck_blocked(),
            _ => {
                if let Some(at) = scrolled(
                    app.blocked_scroll,
                    code,
                    app.page.get(),
                    app.blocked.len().saturating_sub(1),
                ) {
                    app.blocked_scroll = at;
                }
            }
        }
        return;
    }
    if app.confirm_open && app.confirm_details_open && code == KeyCode::Esc {
        app.confirm_details_open = false;
        app.confirm_scroll.set(0);
        return;
    }
    if app.confirm_open && code == KeyCode::Char('l') {
        app.confirm_details_open = !app.confirm_details_open;
        app.confirm_scroll.set(0);
        return;
    }
    if app.confirm_open
        && let Some(at) = scrolled(
            app.confirm_scroll.get(),
            code,
            app.page.get(),
            crate::ui::confirm_display_rows(app, app.width.saturating_sub(2).max(1))
                .saturating_sub(app.page.get().saturating_add(1)),
        )
    {
        app.confirm_scroll.set(at);
        return;
    }
    // An open confirm is its own small mode: the plan on screen is what
    // Enter would run, so nothing may change what is under it. Keys that
    // move the cursor, switch views or sections, open the filter, or mark
    // and unmark are swallowed; the ones it uses (Enter, Esc, d, b, k, ?,
    // R, q) fall through to the arms below.
    if app.confirm_open
        && matches!(
            code,
            KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Backspace
                | KeyCode::Char('v' | '0'..='3' | '/' | ':' | ' ' | 'A')
        )
    {
        return;
    }
    match code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('b') => app.open_blocked(),
        KeyCode::Char('d') if app.confirm_open => app.open_blocked(),
        KeyCode::Up => app.move_selection(-1),
        KeyCode::Down => app.move_selection(1),
        KeyCode::PageUp => app.page_selection(-1),
        KeyCode::PageDown => app.page_selection(1),
        KeyCode::Home => app.select_first(),
        KeyCode::End => app.select_last(),
        // Traversal, the way every file tree does it: right goes in,
        // left comes back out. Enter and Esc still do the same, so the
        // muscle memory either way works.
        KeyCode::Right => app.enter_row(),
        KeyCode::Left => app.leave_row(),
        KeyCode::Enter => app.drill_into_selected(),
        KeyCode::Esc => {
            if app.confirm_open {
                if app.confirm_details_open {
                    app.confirm_details_open = false;
                    app.confirm_scroll.set(0);
                } else {
                    app.cancel_confirm();
                }
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
        // Three sections, and the views inside them: Tab and Shift-Tab move
        // between sections, `1` `2` `3` jump to one, `v` cycles the views of
        // the current section. Nothing else opens a view.
        KeyCode::Tab => app.set_section(app.view.section().next()),
        KeyCode::BackTab => app.set_section(app.view.section().prev()),
        KeyCode::Char('v') => app.set_view(app.view.next()),
        KeyCode::Char(k @ '1'..='3') => {
            if let Some(sec) = app::Section::from_key(k) {
                app.set_section(sec);
            }
        }
        KeyCode::Char('g') => app.set_sort(Sort::Growth),
        KeyCode::Char('s') => app.set_sort(Sort::Size),
        KeyCode::Char('n') => app.set_sort(Sort::Name),
        KeyCode::Char('t') => app.set_sort(Sort::Type),
        KeyCode::Char('a') => app.set_sort(Sort::Age),
        KeyCode::Char('r') => app.toggle_reverse(),
        KeyCode::Char('R') => app.refresh_now(),
        KeyCode::Char('k') => app.toggle_keep_executables(),
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Char('i') => app.inspect_selected_cargo_profile(),
        _ => {}
    }
}

/// How far back the store can answer for `root`. Growth windows are
/// bounded by it: a 7d window over 4h of observations would report a
/// week of growth that was never observed. History is stored under the
/// root-scoped directory (`growth::history_span_for_root`), the same one
/// the header's sparkline reads; the bare device directory holds only
/// side tables, which made the picker say "no observations yet" next to
/// a header that showed history.
fn history_span(store: &std::path::Path, root: &std::path::Path) -> Option<u64> {
    swamp_core::growth::history_span_for_root(store, root, swamp_core::entities::now())
}

/// The longest history any of `roots` has.
fn history_span_of_roots(store: &Path, roots: &[PathBuf]) -> Option<u64> {
    roots.iter().filter_map(|r| history_span(store, r)).max()
}

/// Runs the interactive UI against `root`.
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

/// `Some(banner)` when the store's volume is too full to observe: one
/// `statfs`, no walk, nothing written. The TUI then shows the stored
/// report and skips every refresh (startup, background, live).
fn disk_full_banner(store: &Path) -> Option<String> {
    let min_free = swamp_core::growth::load_config(store).min_free_bytes;
    match swamp_core::disk_guard::check(store, min_free) {
        swamp_core::disk_guard::DiskDecision::Proceed => None,
        swamp_core::disk_guard::DiskDecision::Abort { free, .. } => Some(format!(
            "disk nearly full: refresh skipped ({} free)",
            swamp_core::disk_guard::human(free)
        )),
    }
}

/// The multi-root TUI state built from the last stored observation
/// only: no walk, no observation worker, no live watch. `None` when the
/// store holds no observation for this scope.
pub fn app_from_stored_multi_root(
    scope: &swamp_core::scope::EffectiveScope,
    store: &Path,
    banner: String,
) -> Option<App> {
    stored_multi_root_app(scope, store, Some(banner))
}

/// `app_from_stored_multi_root` with an optional disk banner: the one
/// place the stored multi-root paint is built, for both the
/// disk-full path and the normal path (which then refreshes in the
/// background).
fn stored_multi_root_app(
    scope: &swamp_core::scope::EffectiveScope,
    store: &Path,
    banner: Option<String>,
) -> Option<App> {
    let snapshot = swamp_core::report::report_scope_from_store(scope, store).ok()?;
    let mut app = App::new_multi_root(snapshot.report, scope.scan_paths());
    app.set_external_units(snapshot.external_units);
    app.set_store_interiors(snapshot.store_interiors);
    app.set_agent_units(snapshot.agent_units);
    app.set_manager_facts(snapshot.manager_facts);
    app.set_ledger(swamp_core::volume_ledger::read_reading(store));
    app.scope = Some(scope.clone());
    app.observed_label = "from last observation".into();
    app.disk_banner = banner;
    app.previous_scope_roots = snapshot.previous_scope.as_ref().map(|p| p.roots);
    app.set_declared_roots(&swamp_core::roots::declared_roots(
        scope,
        &snapshot.coverage,
    ));
    finish_startup(&mut app, store, Some(&snapshot.coverage));
    Some(app)
}

/// The resolved swamp dir: the gate's one resolver.
fn store_dir() -> PathBuf {
    swamp_core::fs_gate::StoreDir::resolved()
        .path()
        .to_path_buf()
}

/// Takes the screen before the stored index is read (about half a second
/// on a large store) so the terminal is never blank while it loads.
fn enter_with_splash() -> Result<term::TerminalGuard> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("swamp ui needs an interactive terminal; use swamp report for text");
    }
    let mut guard = term::TerminalGuard::enter()?;
    guard.splash("swamp · reading the last observation…");
    Ok(guard)
}

pub fn run(root: &Path) -> Result<()> {
    let guard = enter_with_splash()?;
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
    let banner = disk_full_banner(&store);
    let has_index = cached.is_some();
    let mut app = match cached {
        Some(snapshot) => {
            let mut a = App::new(snapshot.report, root.clone());
            a.observed_label = "from last observation".into();
            a.set_external_units(snapshot.external_units);
            a.set_store_interiors(snapshot.store_interiors);
            a.set_agent_units(snapshot.agent_units);
            a.set_manager_facts(snapshot.manager_facts);
            a.set_ledger(swamp_core::volume_ledger::read_reading(&store));
            a.disk_banner = banner.clone();
            a
        }
        None => {
            if let Some(b) = &banner {
                anyhow::bail!("{b}, and there is no stored observation to show yet");
            }
            if scope.is_none() {
                anyhow::bail!("could not resolve a scope for {}", root.display());
            }
            // First ever run: open empty; the header's live "observing…"
            // counters show the first walk instead of a blank terminal.
            let mut a = App::new(
                swamp_core::report::Report::empty(root.clone()),
                root.clone(),
            );
            a.observed_label = "no observation yet".into();
            a.store_rebuild = swamp_core::report::store_is_older_generation(&store);
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
    start_background_services(&mut app, has_index);
    run_terminal_loop(guard, &mut app)
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
/// Like `run`, this paints the last stored report immediately, at any
/// age, and scans only when there is no index at all (see
/// `start_background_services`): on a background worker, opening empty
/// with the first walk's progress in the header. `R` refreshes on
/// demand; the schedule keeps the index current. It never blocks on the observation lock: if another
/// process holds it, the header says who and for how long.
pub fn run_scope(scope: &swamp_core::scope::EffectiveScope) -> Result<()> {
    let guard = enter_with_splash()?;
    let store = store_dir();
    let present_roots = scope.scan_paths();
    anyhow::ensure!(
        !present_roots.is_empty(),
        "swamp_tui::run_scope requires at least one present root in scope"
    );
    let banner = disk_full_banner(&store);
    let stored = stored_multi_root_app(scope, &store, banner.clone());
    // "Is there an index" means the store holds an observation of any
    // scope: the first scan is only for a store with none. A scope no
    // stored observation overlaps opens empty and waits for `R`.
    let has_index = stored.is_some() || swamp_core::report::store_has_observation(&store);
    let mut app = match stored {
        Some(app) => app,
        None => {
            if let Some(b) = &banner
                && !has_index
            {
                // Never block startup on a walk the disk cannot take.
                anyhow::bail!("{b}, and there is no stored observation to show yet");
            }
            // First ever run: nothing stored to paint. Open on an empty
            // report and let the header's live "observing…" counters
            // show the first walk's progress instead of a blank terminal.
            let mut app = App::new_multi_root(
                swamp_core::report::Report::empty(
                    present_roots.first().cloned().unwrap_or_default(),
                ),
                present_roots.clone(),
            );
            app.scope = Some(scope.clone());
            app.observed_label = "no observation yet".into();
            app.has_index = false;
            app.store_rebuild = swamp_core::report::store_is_older_generation(&store);
            app.set_declared_roots(&swamp_core::roots::declared_roots(scope, &[]));
            finish_startup(&mut app, &store, None);
            app
        }
    };
    start_background_services(&mut app, has_index);
    run_terminal_loop(guard, &mut app)
}

/// After the first paint is decided: watch the observation lock (so a
/// scheduled observation shows in the header and its result reloads),
/// and scan only when there is no index yet. An existing index, however
/// old, is shown as it is; nothing here waits on a walk or the lock.
fn start_background_services(app: &mut App, has_index: bool) {
    if app.disk_banner.is_some() {
        return;
    }
    app.start_lock_poll(Duration::from_secs(2), std::process::id());
    app.scan_if_no_index(has_index);
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
                reached_by_registry: Vec::new(),
            })
        })
        .collect()
}

/// Everything after an `App` has its initial `report`/`roots` set:
/// external/agent-tool storage discovery, the header's coverage note,
/// and restoring persisted UI state. (No filesystem watch: the TUI never
/// scans on file events, so there is nothing to watch for.)
/// Shared by `run` and `run_scope` so this bookkeeping is never
/// duplicated (or allowed to drift) between the single- and multi-root
/// startup paths.
fn finish_startup(
    app: &mut App,
    store: &Path,
    coverage: Option<&[swamp_core::coverage::RootCoverage]>,
) {
    app.store_dir = Some(store.to_path_buf());
    app.live_age = true;
    // External/agent-tool storage is no longer discovered here. It
    // arrives from the same `report::observe_scope` pass that produced
    // the report, and is refreshed by every later observation -- the
    // review found this function was the *only* production caller that
    // ever set those vectors, so the agents view could be arbitrarily
    // stale while the header said the report was live.
    if let Some(c) = coverage {
        app.set_scope_note(c);
    }
    // The picker may offer a window as long as the longest history of any
    // root this report covers.
    app.history_secs = history_span_of_roots(store, &app.roots);
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
    // A store that has never shown the new views shows the pointer to them
    // once; the flag is written when either is opened.
    app.views_seen = saved.views_seen;
}

fn run_terminal_loop(mut guard: term::TerminalGuard, app: &mut App) -> Result<()> {
    let result = event_loop(&mut guard.terminal, app);

    // Terminal failure must not abandon an in-flight filesystem move.
    if app.operation.is_some() {
        app.cancel_operation();
        while app.operation.is_some() {
            app.poll_operation();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // The last sort, filter or `k` choice reaches the disk before exit.
    app.flush_ui_state();
    // The guard puts the terminal back as it drops, here or on a panic.
    drop(guard);
    result
}

/// Decides whether the screen needs painting. Idle, nothing changes and
/// nothing is painted: no timer repaints an unchanged screen (it cost a
/// terminal escape burst five times a second over ssh and tmux). It paints
/// when something asked (a key, a resize, a worker's result), while
/// anything is busy (the spinner and the elapsed time move), and when a
/// clock-driven part of the screen reads differently (the age of the
/// index, a refusal that ran out).
#[derive(Default)]
struct RedrawGate {
    forced: bool,
    was_busy: bool,
    last_clock: Option<String>,
}

impl RedrawGate {
    /// Something other than the clock changed the screen's inputs.
    fn touch(&mut self) {
        self.forced = true;
    }

    /// Whether to paint now; records what the clock parts showed.
    fn due(&mut self, app: &App) -> bool {
        let clock = ui::clock_signature(app);
        let busy = app.is_busy();
        // The frame after the last busy one shows the finished state.
        let due =
            self.forced || busy || self.was_busy || self.last_clock.as_deref() != Some(&clock);
        self.was_busy = busy;
        self.forced = false;
        self.last_clock = Some(clock);
        due
    }

    /// How long to wait for input: quick while something moves, slow when
    /// only the clock could change.
    fn wait(app: &App) -> Duration {
        if app.is_busy() {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(1)
        }
    }
}

/// Everything that arrives without a key: worker results, the lock poll,
/// the terminal size. Each one that changed the screen's inputs touches the
/// gate, so the very next frame shows it.
fn advance(app: &mut App, gate: &mut RedrawGate, size: Option<(u16, u16)>) {
    if app.poll_operation() {
        gate.touch();
    }
    if app.poll_tool() {
        gate.touch();
    }
    if app.apply_held_reload() {
        gate.touch();
    }
    if app.operation.is_none()
        && let Some(rx) = &app.pending
        && let Ok(res) = rx.try_recv()
    {
        app.pending = None;
        app.observing = None;
        gate.touch();
        match res {
            Ok(fresh) => {
                // Each re-observed root replaced exactly its own entry
                // (#51) on the worker, which also merged: the event
                // thread only installs what the worker prepared.
                app.land_observation(fresh);
            }
            Err(e) => app.status = Some(format!("observation failed: {e}")),
        }
    }
    if app.drain_lock_poll() {
        gate.touch();
    }
    if let Some((w, h)) = size
        && (app.width != w || app.height != h)
    {
        app.width = w;
        app.height = h;
        gate.touch();
    }
}

fn event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut gate = RedrawGate::default();
    loop {
        let size = terminal.size().ok().map(|sz| (sz.width, sz.height));
        advance(app, &mut gate, size);
        if gate.due(app) {
            app.frame = app.frame.wrapping_add(1);
            terminal.draw(|f| ui::draw(f, app))?;
        }
        if app.quit {
            return Ok(());
        }
        // A confirm just appeared: whatever was already queued (key
        // repeat, typeahead, a paste) was typed before it was seen.
        if app.take_confirm_drain() {
            while event::poll(Duration::ZERO)? {
                let _ = event::read()?;
            }
        }
        if event::poll(RedrawGate::wait(app))? {
            match event::read()? {
                // A paste is never keys: nothing on screen acts on it.
                Event::Paste(_) => {}
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    gate.touch();
                    handle_terminal_key(app, key);
                }
                // A resize, a focus change or anything else may have
                // disturbed the screen: paint it again.
                _ => gate.touch(),
            }
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
            bytes_done: 0,
            bytes_total: 0,
            started: std::time::Instant::now(),
            cancel: cancel.clone(),
            checking_open_files: None,
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

    fn buffer_text(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| ui::draw(f, app)).unwrap();
        t.backend().to_string()
    }

    #[test]
    fn delete_result_and_key_legend_are_both_visible_at_80_columns() {
        let mut app = App::new(empty_report(), "/root".into());
        app.set_result(
            "Moved 3 items (1.3GB) to Trash. Space is freed when Trash is emptied.".into(),
        );
        let s = buffer_text(&app, 80, 24);
        assert!(s.contains("Moved 3 items (1.3GB) to Trash."), "{s}");
        assert!(s.contains("Space is freed when Trash is emptied"), "{s}");
        assert!(s.contains("? help  q quit"), "{s}");
    }

    #[test]
    fn a_result_stays_until_the_next_key_and_time_alone_never_erases_it() {
        let mut app = App::new(empty_report(), "/root".into());
        app.set_result("Moved 2 items (1MB) to Trash.".into());
        assert!(buffer_text(&app, 80, 24).contains("Moved 2 items"));
        std::thread::sleep(Duration::from_millis(1100));
        let s = buffer_text(&app, 80, 24);
        assert!(s.contains("Moved 2 items"), "{s}");
        assert!(s.contains("? help  q quit"), "{s}");
        handle_key(&mut app, KeyCode::Down);
        let s = buffer_text(&app, 80, 24);
        assert!(!s.contains("Moved 2 items"), "{s}");
        assert!(s.contains("? help  q quit"), "{s}");
    }

    #[test]
    fn review_status_names_the_open_file_check_then_the_item_count() {
        let mut app = App::new(empty_report(), "/root".into());
        app.operation = Some(crate::app::Operation {
            label: "Reviewing",
            completed: 0,
            total: 284,
            succeeded: 0,
            failed: 0,
            current: "swamp · incremental build (target/debug)".into(),
            bytes_done: 0,
            bytes_total: 0,
            started: std::time::Instant::now(),
            cancel: Default::default(),
            checking_open_files: Some(std::time::Instant::now()),
        });
        let s = buffer_text(&app, 100, 24);
        assert!(s.contains("Checking what is in use"), "{s}");
        assert!(s.contains("Nothing has been changed"), "{s}");
        app.operation.as_mut().unwrap().checking_open_files = None;
        let s = buffer_text(&app, 100, 24);
        assert!(s.contains("Checked 0 of 284"), "{s}");
        assert!(
            s.contains("swamp · incremental build (target/debug)"),
            "{s}"
        );
        assert!(!s.contains("Checking what is in use"), "{s}");
    }

    /// A stored report plus a resolved scope, in a scratch store.
    fn stored_app(observed_at: u64) -> (tempfile::TempDir, tempfile::TempDir, App) {
        let store = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let scope = resolved_scope(store.path(), &[root.path().to_path_buf()]).expect("scope");
        let mut report = empty_report();
        report.observed_at = observed_at;
        let mut app = App::new(report, root.path().to_path_buf());
        app.scope = Some(scope);
        app.store_dir = Some(store.path().to_path_buf());
        app.live_age = true;
        (store, root, app)
    }

    #[test]
    fn an_existing_index_is_never_scanned_on_open_however_old() {
        let (_s, _r, mut app) = stored_app(1);
        assert!(!app.scan_if_no_index(true));
        assert!(app.pending.is_none() && app.observing.is_none());
    }

    #[test]
    fn no_index_scans_in_the_background_with_progress() {
        let (_s, _r, mut app) = stored_app(0);
        assert!(app.scan_if_no_index(false));
        assert!(app.pending.is_some() && app.observing.is_some());
        let s = buffer_text(&app, 120, 24);
        assert!(s.contains("observing"), "{s}");
        assert!(!s.contains("press R"), "{s}");
    }

    fn header_of(app: &App) -> String {
        buffer_text(app, 200, 24)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn header_shows_a_fresh_age_plainly() {
        let now = swamp_core::entities::now();
        let (_s, _r, app) = stored_app(now - 4 * 60);
        let h = header_of(&app);
        assert!(h.contains("observed 4m ago"), "{h}");
        assert!(!h.contains("older than"), "{h}");
    }

    #[test]
    fn header_warns_when_the_index_is_over_the_limit() {
        let now = swamp_core::entities::now();
        let (_s, _r, app) = stored_app(now - (app::STALE_AFTER_SECS + 32 * 60));
        let h = header_of(&app);
        assert!(
            h.contains("observed 47m ago · older than 15 min, press R to refresh"),
            "{h}"
        );
        // Bold, not a color: yellow is unreadable on many light themes.
        let mut t = Terminal::new(TestBackend::new(200, 24)).unwrap();
        t.draw(|f| ui::draw(f, &app)).unwrap();
        let head: Vec<_> = t.backend().buffer().content().iter().take(200).collect();
        assert!(
            head.iter()
                .any(|c| c.modifier.contains(ratatui::style::Modifier::BOLD)),
            "the warning is bold"
        );
        assert!(
            head.iter().all(|c| !matches!(
                c.fg,
                ratatui::style::Color::Yellow | ratatui::style::Color::Red
            )),
            "no color carries the warning"
        );
    }

    #[test]
    fn a_running_scan_replaces_the_refresh_hint() {
        let now = swamp_core::entities::now();
        let (_s, _r, mut app) = stored_app(now - 3600);
        assert!(header_of(&app).contains("press R to refresh"));
        app.observing = Some((0, 0));
        let h = header_of(&app);
        assert!(h.contains("observing") && !h.contains("press R"), "{h}");
        app.observing = None;
        app.external_observer = Some(swamp_core::schedule::LockHolder {
            pid: 4242,
            since: now - 72,
        });
        let h = header_of(&app);
        assert!(
            // The clock may tick between building the holder and drawing it.
            h.contains("another observation running (pid 4242, 1m 1") && !h.contains("press R"),
            "{h}"
        );
    }

    /// Issue #210. Tempting wrong patch: count `chars()` (or stop at one
    /// hint) and let the terminal clip the line, which ends `q ` with no
    /// word. Every shown hint must carry its full label at every width,
    /// for every view.
    #[test]
    fn legend_never_shows_a_hint_without_its_label_at_any_width() {
        let known = [
            "Tab section",
            "v view",
            "/ filter",
            "R refresh",
            "⌫ delete",
            "⌫ trash",
            "Space mark",
            "A mark all",
            "↑↓ move",
            "→/← in/out",
            "g/s/n/t/a sort",
            "r reverse",
            "? help",
            "q quit",
            "b blocked",
        ];
        for v in app::ViewKind::ALL {
            for w in 1u16..=200 {
                let mut app = App::new(empty_report(), "/root".into());
                app.set_view(v);
                let mut t = Terminal::new(TestBackend::new(w, 24)).unwrap();
                t.draw(|f| ui::draw(f, &app)).unwrap();
                let buf = t.backend().buffer().clone();
                let last: String = (0..w)
                    .map(|x| buf[(x, 23)].symbol().to_string())
                    .collect::<String>();
                let line = last.trim_end();
                if line.is_empty() {
                    continue;
                }
                for part in line.split("  ").map(str::trim).filter(|p| !p.is_empty()) {
                    assert!(
                        known.contains(&part),
                        "view {v:?} width {w}: cut hint {part:?} in {line:?}"
                    );
                }
                if w >= 40 {
                    assert!(line.ends_with("q quit"), "view {v:?} width {w}: {line:?}");
                }
            }
        }
    }

    /// Adversarial (#210): the "never cut mid-hint" rule must hold for the
    /// keys row in every mode, not only the main legend. Tempting wrong
    /// patch: fit only `footer_legend` and leave the fixed mode strings
    /// (filter editor, blocked list) to be clipped by the terminal.
    #[test]
    fn adv_mode_key_rows_never_cut_a_hint_at_narrow_widths() {
        type Setup = fn(&mut App);
        let modes: [(&str, Setup, &[&str]); 2] = [
            (
                "filter editor",
                |a| a.editing_filter = true,
                &["Tab complete", "Enter apply", "Esc cancel"],
            ),
            (
                "blocked list",
                |a| a.blocked_open = true,
                &["↑↓ scroll", "r check again", "Esc close"],
            ),
        ];
        for (name, setup, hints) in modes {
            for w in 12u16..=80 {
                let mut app = App::new(empty_report(), "/root".into());
                setup(&mut app);
                let mut t = Terminal::new(TestBackend::new(w, 24)).unwrap();
                t.draw(|f| ui::draw(f, &app)).unwrap();
                let buf = t.backend().buffer().clone();
                let line: String = (0..w).map(|x| buf[(x, 23)].symbol().to_string()).collect();
                let line = line.trim_end();
                for part in line.split(" · ").map(str::trim).filter(|p| !p.is_empty()) {
                    assert!(
                        hints.contains(&part),
                        "{name} width {w}: cut hint {part:?} in {line:?}"
                    );
                }
                assert!(
                    line.contains("Esc"),
                    "{name} width {w}: no way out shown: {line:?}"
                );
            }
        }
    }

    /// Tempting wrong patch: only the main legend and two modes are fitted;
    /// help's title, the plan confirm and the tool sheet are cut by the
    /// terminal. In each mode, at every width, the last row/title shows only
    /// whole hints and the way out (Esc) while any hint fits.
    #[test]
    fn every_mode_keeps_the_way_out_and_cuts_no_hint() {
        use crate::tool_sheet::{Stage, ToolSheet};
        let manager = swamp_core::tool_removal::Manager::Mise;
        type Setup = fn(&mut App);
        let modes: [(&str, Setup, usize, &[&str]); 5] = [
            (
                "help",
                |a| a.help_open = true,
                0,
                &["help", "↑↓ PgUp PgDn Home End scroll", "Esc closes"],
            ),
            (
                "confirm",
                |a| a.confirm_open = true,
                23,
                &[
                    "Esc cancel",
                    "No actions marked",
                    "Read every line before action",
                    "Terminal too small to review",
                    "l inspect paths",
                ],
            ),
            (
                "tool sheet listing",
                |a| a.tool_sheet = Some(ToolSheet::new(swamp_core::tool_removal::Manager::Mise)),
                22,
                &["Esc cancel (nothing is removed)", "Esc cancel"],
            ),
            (
                "tool sheet choose",
                |a| {
                    let mut t = ToolSheet::new(swamp_core::tool_removal::Manager::Mise);
                    t.stage = Stage::Choose;
                    a.tool_sheet = Some(t);
                },
                22,
                &["↑↓ choose", "Enter review", "Esc close"],
            ),
            (
                "filter",
                |a| a.editing_filter = true,
                23,
                &["Tab complete", "Enter apply", "Esc cancel"],
            ),
        ];
        let _ = manager;
        for (name, setup, row, hints) in modes {
            for w in 14u16..=100 {
                let mut app = App::new(empty_report(), "/root".into());
                setup(&mut app);
                let mut t = Terminal::new(TestBackend::new(w, 24)).unwrap();
                t.draw(|f| ui::draw(f, &app)).unwrap();
                let buf = t.backend().buffer().clone();
                let line: String = (0..w)
                    .map(|x| buf[(x, row as u16)].symbol().to_string())
                    .collect();
                let line = line.rsplit('┌').next().unwrap_or(&line);
                let line = line.trim_matches(|c: char| " │─┐└┘".contains(c));
                for part in line.split(" · ").map(str::trim).filter(|p| !p.is_empty()) {
                    // help's title also carries a "N-M of T" range.
                    let range = part.contains(" of ") && part.contains('-');
                    assert!(
                        hints.contains(&part) || range,
                        "{name} width {w}: cut hint {part:?} in {line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn footer_legend_names_the_refresh_key() {
        let app = App::new(empty_report(), "/root".into());
        assert!(buffer_text(&app, 200, 24).contains("R refresh"));
    }

    #[test]
    fn nearly_full_disk_never_observes() {
        let (_s, _r, mut app) = stored_app(0);
        app.disk_banner = Some("disk nearly full".into());
        assert!(!app.scan_if_no_index(false));
        app.refresh_now();
        assert!(app.pending.is_none());
    }

    #[test]
    fn held_lock_is_reported_not_waited_on() {
        let (s, _r, mut app) = stored_app(0);
        let _held = match swamp_core::schedule::acquire_lock(s.path()).unwrap() {
            swamp_core::schedule::LockOutcome::Acquired(g) => g,
            _ => panic!("scratch store must be free"),
        };
        let started = std::time::Instant::now();
        app.refresh_now();
        // The key handler returned at once; the worker answers.
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        let res = app
            .pending
            .as_ref()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("worker answered");
        let err = res
            .err()
            .expect("held lock is not an observation")
            .to_string();
        assert!(err.contains("another observation is running"), "{err}");
    }

    #[test]
    fn an_idle_screen_is_never_repainted_until_something_changes() {
        let now = swamp_core::entities::now();
        let (_s, _r, mut app) = stored_app(now - 30);
        let mut gate = RedrawGate::default();
        assert!(gate.due(&app), "the first frame is always painted");
        for _ in 0..50 {
            assert!(!gate.due(&app), "an unchanged idle screen is not repainted");
        }
        assert_eq!(RedrawGate::wait(&app), Duration::from_secs(1));
        // A key, a resize or a worker result asks for exactly one paint.
        gate.touch();
        assert!(gate.due(&app));
        assert!(!gate.due(&app));
        // Anything busy paints on the 200 ms tick so the glyph and the
        // elapsed time move.
        app.observing = Some((0, 0));
        assert_eq!(RedrawGate::wait(&app), Duration::from_millis(200));
        assert!(gate.due(&app) && gate.due(&app));
        app.observing = None;
        assert!(gate.due(&app), "the busy to idle frame is painted once");
        assert!(!gate.due(&app));
        // The index's age is a clock: crossing into the next minute paints.
        app.report.observed_at = now - 200;
        assert!(gate.due(&app), "the age label changed");
        assert!(!gate.due(&app));
        // A refusal that runs out repaints without a key.
        app.refusal = Some(("refused: x".into(), std::time::Instant::now()));
        assert!(gate.due(&app), "the refusal appears");
        assert!(!gate.due(&app));
        app.refusal = Some((
            "refused: x".into(),
            std::time::Instant::now() - app::REFUSAL_DISPLAY - Duration::from_millis(1),
        ));
        assert!(gate.due(&app), "the refusal ran out: one paint clears it");
        assert!(!gate.due(&app));
    }

    #[test]
    fn ui_state_is_written_off_the_event_thread_newest_wins_and_flushes_on_exit() {
        let (store, _r, mut app) = stored_app(0);
        app.set_sort(Sort::Size);
        app.set_sort(Sort::Size); // off again
        app.set_sort(Sort::Name);
        app.toggle_reverse();
        app.flush_ui_state();
        let saved = app::load_ui_state(store.path());
        assert_eq!(saved.sort, "name");
        assert!(saved.reverse);
    }

    #[test]
    fn r_while_another_observation_runs_says_so_and_starts_nothing() {
        let now = swamp_core::entities::now();
        let (_s, _r, mut app) = stored_app(now - 3600);
        app.external_observer = Some(swamp_core::schedule::LockHolder {
            pid: 4242,
            since: now - 32,
        });
        handle_key(&mut app, KeyCode::Char('R'));
        assert!(app.pending.is_none(), "no second walk");
        let s = buffer_text(&app, 80, 24);
        assert!(
            s.contains("An observation is already running (pid 4242, 3"),
            "{s}"
        );
        assert!(s.contains("Its result loads here"), "{s}");
        assert!(
            !s.contains("scheduled"),
            "a manual `swamp observe` is not 'scheduled': {s}"
        );
    }

    #[test]
    fn lock_poll_shows_and_clears_an_external_observer() {
        let (s, _r, mut app) = stored_app(0);
        let guard = match swamp_core::schedule::acquire_lock(s.path()).unwrap() {
            swamp_core::schedule::LockOutcome::Acquired(g) => g,
            _ => panic!("scratch store must be free"),
        };
        // Pretend to be a different process so this pid reads as foreign.
        app.start_lock_poll(std::time::Duration::from_millis(20), std::process::id() + 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.external_observer.is_none() {
            assert!(std::time::Instant::now() < deadline, "holder never shown");
            app.drain_lock_poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(app.external_observer.unwrap().pid, std::process::id());
        let s = buffer_text(&app, 200, 24);
        assert!(s.contains("another observation running"), "{s}");
        drop(guard);
        while app.external_observer.is_some() {
            assert!(std::time::Instant::now() < deadline, "holder never cleared");
            app.drain_lock_poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
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

    // ---- adversarial review (audit/v0.7.5-adversarial) ----

    fn test_op() -> crate::app::Operation {
        crate::app::Operation {
            label: "Reviewing",
            completed: 3,
            total: 3,
            succeeded: 3,
            failed: 0,
            current: "/tmp/x".into(),
            bytes_done: 0,
            bytes_total: 0,
            started: std::time::Instant::now(),
            cancel: Default::default(),
            checking_open_files: None,
        }
    }

    /// The event loop calls `poll_operation` (which may end the operation
    /// and open the confirm) and then asks the gate. The iteration where
    /// the operation ends is not busy and nothing touched the gate, so
    /// the finished state (confirm, result line) must still be painted.
    #[test]
    fn adv_the_frame_after_an_operation_finishes_is_painted() {
        let now = swamp_core::entities::now();
        let (_s, _r, mut app) = stored_app(now - 30);
        let mut gate = RedrawGate::default();
        app.operation = Some(test_op());
        assert!(gate.due(&app), "busy: painted");
        // What poll_operation does when `Reviewed` arrives.
        app.operation = None;
        app.set_result("Marked 3 more. 3 marked in all (1MB).".into());
        assert!(
            gate.due(&app),
            "operation ended (result/confirm now on screen) but the gate did not paint: stale 'Reviewing' frame stays up"
        );
    }

    fn unit(path: &str, bytes: u64, docker: bool) -> crate::actions::MarkedUnit {
        crate::actions::MarkedUnit {
            cargo_unit: None,
            agent_unit: None,
            session_members: None,
            reclaim: None,
            path: path.into(),
            docker: docker.then(|| swamp_core::docker::Removal::Volume {
                name: "pgdata".into(),
            }),
            worktree_path: "/root/p".into(),
            bytes,
            observed_at: 0,
            worktree: None,
            label: if docker {
                "pgdata".into()
            } else {
                String::new()
            },
            warnings: vec![],
        }
    }

    #[test]
    fn confirmation_inventory_is_optional_and_enter_returns_to_summary_first() {
        let mut app = App::new(empty_report(), "/root".into());
        let mut marked = unit("/root/session", 12, false);
        marked.session_members = Some(vec![swamp_core::agents::AgentMember {
            path: "/root/session/transcript.jsonl".into(),
            bytes: 12,
            kind: swamp_core::agents::AgentMemberKind::Transcript,
        }]);
        app.marked.insert("/root/session".into(), marked);
        app.confirm_open = true;
        app.width = 80;
        app.height = 24;
        let _ = buffer_text(&app, 80, 24);
        assert!(app.confirm_review_is_complete(80, 24));

        handle_key(&mut app, KeyCode::Char('l'));
        assert!(app.confirm_details_open);
        assert!(!app.confirm_review_is_complete(80, 24));
        assert!(buffer_text(&app, 80, 24).contains("transcript.jsonl"));
        handle_key(&mut app, KeyCode::Enter);
        assert!(app.operation.is_none() && app.confirm_open);

        handle_key(&mut app, KeyCode::Esc);
        assert!(!app.confirm_details_open && app.confirm_open);
        assert!(app.confirm_review_is_complete(80, 24));
    }

    /// A plan with a Trash part and a permanent docker part: whatever the
    /// terminal size, if the confirm keys are on screen the permanent
    /// removal must be too (Enter removes it for good).
    #[test]
    fn adv_permanent_docker_removal_is_visible_whenever_enter_confirm_is() {
        let mut app = App::new(empty_report(), "/root".into());
        app.marked.insert(
            "/root/p/node_modules".into(),
            unit("/root/p/node_modules", 1 << 30, false),
        );
        app.marked
            .insert("docker:pgdata".into(), unit("docker:pgdata", 5 << 30, true));
        app.confirm_open = true;
        let mut bad = Vec::new();
        for (w, h) in [
            (200u16, 60u16),
            (80, 24),
            (80, 12),
            (60, 10),
            (40, 10),
            (40, 8),
            (20, 5),
        ] {
            let s = buffer_text(&app, w, h);
            let keys = s.contains("Enter");
            let permanent = s.to_ascii_lowercase().contains("permanently")
                || s.to_ascii_lowercase().contains("docker");
            if keys && !permanent {
                bad.push(format!("{w}x{h}:\n{s}"));
            }
        }
        assert!(
            bad.is_empty(),
            "Enter offered without the permanent part:\n{}",
            bad.join("\n")
        );
    }

    #[test]
    fn adv_tiny_and_degenerate_terminals_never_panic() {
        let now = swamp_core::entities::now();
        let (_s, _r, mut app) = stored_app(now - 30);
        for (w, h) in [
            (1u16, 1u16),
            (0, 0),
            (1, 0),
            (0, 5),
            (20, 5),
            (40, 10),
            (300, 60),
            (2, 2),
        ] {
            for state in 0..5 {
                app.help_open = state == 1;
                app.operation = (state == 2).then(test_op);
                app.confirm_open = state == 3;
                app.set_result(if state == 4 {
                    "x".repeat(500)
                } else {
                    String::new()
                });
                let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
                t.draw(|f| ui::draw(f, &app)).unwrap();
            }
        }
        app.help_open = false;
        app.operation = None;
        app.confirm_open = false;
        for k in [
            KeyCode::PageDown,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::Home,
            KeyCode::Down,
            KeyCode::Up,
        ] {
            handle_key(&mut app, k);
            let _ = buffer_text(&app, 20, 5);
        }
    }

    // ---- v0.7.5 adversarial fixes ----

    #[test]
    fn plan_sheet_heads_ready_and_blocked_and_names_each_kind_once() {
        let mut app = App::new(empty_report(), "/root".into());
        for i in 0..40 {
            let path = format!("/root/p{i}/node_modules");
            app.marked.insert(path.clone(), unit(&path, 1 << 20, false));
        }
        for i in 0..3 {
            let path = format!("/root/q{i}/.build");
            app.marked.insert(path.clone(), unit(&path, 1 << 20, false));
        }
        app.blocked = vec![app::BlockedItem {
            name: "x".into(),
            reason: "nothing reclaimable in this project".into(),
            next: "n".into(),
        }];
        app.confirm_open = true;
        let s = buffer_text(&app, 100, 30);
        assert!(s.contains("Review 43 actions"), "{s}");
        assert!(
            s.contains("/root"),
            "grouped summary names the shared root: {s}"
        );
        let units: Vec<_> = app.marked.values().cloned().collect();
        let details = crate::actions::confirm_details(&units);
        assert!(details.contains("/root/p0/node_modules"), "{details}");
        assert!(details.contains("/root/q2/.build"), "{details}");
        assert!(s.contains("d blocked"), "{s}");
        assert!(s.contains("Enter move to Trash"), "{s}");
    }

    #[test]
    fn a_small_plan_sheet_fills_its_rows_with_count_first() {
        let mut app = App::new(empty_report(), "/root".into());
        app.marked
            .insert("/root/p/a".into(), unit("/root/p/a", 1 << 20, false));
        app.confirm_open = true;
        let s = buffer_text(&app, 40, 10);
        let rows: Vec<&str> = s.lines().collect();
        let top = rows
            .iter()
            .position(|r| r.starts_with("\"┌ Review actions"))
            .unwrap();
        let bottom = rows
            .iter()
            .enumerate()
            .skip(top + 1)
            .find(|(_, r)| r.starts_with("\"└"))
            .map(|(i, _)| i)
            .unwrap();
        let empty_inside = rows[top + 1..bottom]
            .iter()
            .filter(|r| r.trim_matches('"').trim_matches('│').trim().is_empty())
            .count();
        assert!(s.contains("Review 1 action"), "{s}");
        assert!(s.contains("/root/p/a"), "{s}");
        assert!(s.contains("Enter move to Trash"), "{s}");
        assert_eq!(empty_inside, 0, "no blank row inside the box:\n{s}");
    }

    #[test]
    fn a_fresh_index_reads_just_now_so_the_idle_screen_stays_still() {
        let now = swamp_core::entities::now();
        let (_s, _r, app) = stored_app(now - 5);
        let mut gate = RedrawGate::default();
        assert!(gate.due(&app));
        assert!(app.live_age);
        for _ in 0..3 {
            assert!(!gate.due(&app));
        }
        let s = buffer_text(&app, 100, 24);
        assert!(s.contains("observed just now"), "{s}");
    }

    #[test]
    fn without_a_terminal_the_ui_says_what_to_use_instead() {
        // Test output is captured, so stdout is never a terminal here.
        let err = enter_with_splash().err().expect("no terminal, no UI");
        assert_eq!(
            err.to_string(),
            "swamp ui needs an interactive terminal; use swamp report for text"
        );
    }

    #[test]
    fn a_plan_with_a_long_blocked_line_leaves_no_blank_row_at_60x10() {
        let mut app = App::new(empty_report(), "/root".into());
        for i in 0..5 {
            let path = format!("/root/p{i}/node_modules");
            app.marked.insert(path.clone(), unit(&path, 1 << 20, false));
        }
        app.blocked = vec![
            app::BlockedItem {
                name: "x".into(),
                reason: "nothing reclaimable in this project".into(),
                next: "n".into(),
            },
            app::BlockedItem {
                name: "y".into(),
                reason: "in use".into(),
                next: "n".into(),
            },
        ];
        app.confirm_open = true;
        let s = buffer_text(&app, 60, 11);
        let blank_inside = s
            .lines()
            .filter(|r| {
                r.starts_with("\"│") && r.trim_matches('"').trim_matches('│').trim().is_empty()
            })
            .count();
        assert!(s.contains("Review 5 actions"), "{s}");
        let details =
            crate::actions::confirm_details(&app.marked.values().cloned().collect::<Vec<_>>());
        assert!(details.contains("/root/p0/node_modules"), "{details}");
        assert!(s.contains("Enter move to Trash"), "{s}");
        assert_eq!(blank_inside, 0, "{s}");
    }
}
