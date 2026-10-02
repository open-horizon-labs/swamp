//! `swamp observe` and `swamp schedule` subcommand handlers.
//!
//! `observe` is the program the LaunchAgent installed by `schedule` runs:
//! walk + growth-store write only, never a rendered report. `schedule`
//! installs/reports/removes the per-user LaunchAgent itself. All LaunchAgent
//! logic lives in `swamp_core::schedule`; this module is CLI glue only.

use crate::{safe_print, safe_println};
use anyhow::{Result, bail};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use swamp_core::growth::load_config;
use swamp_core::report::ObservationParts;
use swamp_core::schedule::{
    self, LockOutcome, RunOutcome, acquire_lock, append_log, write_last_run,
};

fn trace_enabled() -> bool {
    std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty())
}

fn safe_console_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn count_noun(count: usize, singular: &str, plural: &str) -> String {
    format!(
        "{} {}",
        swamp_core::render::human_count(count as u64),
        if count == 1 { singular } else { plural }
    )
}

fn coverage_summary(coverage: &[swamp_core::coverage::RootCoverage]) -> String {
    use swamp_core::coverage::RegionStatus as Status;

    let mut counts = [0usize; 7];
    for row in coverage {
        let index = match &row.status {
            Status::Complete => 0,
            Status::Partial { .. } => 1,
            Status::DetectorOnly => 2,
            Status::NotMeasured { .. } => 3,
            Status::Inaccessible { .. } => 4,
            Status::Excluded => 5,
            Status::Missing => 6,
        };
        counts[index] += 1;
    }
    let descriptions = [
        ("complete root", "complete roots"),
        ("partial root", "partial roots"),
        (
            "tool location not scanned for projects",
            "tool locations not scanned for projects",
        ),
        ("root not measured", "roots not measured"),
        ("inaccessible root", "inaccessible roots"),
        ("excluded root", "excluded roots"),
        ("missing root", "missing roots"),
    ];
    let parts = counts
        .iter()
        .zip(descriptions)
        .filter(|(count, _)| **count > 0)
        .map(|(count, (one, many))| {
            format!(
                "{} {}",
                swamp_core::render::human_count(*count as u64),
                if *count == 1 { one } else { many }
            )
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "Coverage: no root coverage was returned.".to_string()
    } else {
        format!("Coverage: {}.", parts.join(", "))
    }
}

/// Keep ordinary completion output concise when optional detector locations
/// are absent. Configured/explicit roots and every other measurement failure
/// remain path-specific; diagnostic output includes all paths.
fn coverage_details(
    coverage: &[swamp_core::coverage::RootCoverage],
    roots: &[swamp_core::scope::ScopeRoot],
    diagnostics: bool,
) -> (Vec<String>, Vec<PathBuf>) {
    use swamp_core::coverage::RegionStatus as Status;

    let mut details = Vec::new();
    let mut detailed_paths = Vec::new();
    let mut absent_tool_locations = 0usize;
    for row in coverage {
        if matches!(&row.status, Status::Complete | Status::DetectorOnly) {
            continue;
        }
        let optional_missing_tool = matches!(&row.status, Status::Missing)
            && roots
                .iter()
                .find(|root| root.path == row.path)
                .is_some_and(|root| !root.is_project_root());
        if optional_missing_tool && !diagnostics {
            absent_tool_locations += 1;
            continue;
        }
        details.push(format!(
            "  {}: {}",
            safe_console_text(&row.path.display().to_string()),
            safe_console_text(&row.status.label()),
        ));
        detailed_paths.push(row.path.clone());
    }
    if absent_tool_locations > 0 {
        details.push(format!(
            "  {} absent optional tool location{}",
            swamp_core::render::human_count(absent_tool_locations as u64),
            if absent_tool_locations == 1 { "" } else { "s" },
        ));
    }
    (details, detailed_paths)
}

fn quarantine_output_notes(
    notes: &[(PathBuf, String)],
    detailed_coverage_paths: &[PathBuf],
) -> Vec<String> {
    notes
        .iter()
        .filter(|(path, _)| !detailed_coverage_paths.contains(path))
        .map(|(_, note)| format!("  {}", safe_console_text(note)))
        .collect()
}

/// `swamp observe [root...]`. Exits 0 on success, on a graceful
/// "another observation is running" skip, and even on a timeout/error --
/// only in-process misuse is a hard error, since a launchd-triggered run
/// should never wedge into a retry storm.
///
/// R12: the *only* command that scans. One coherent
/// `report::observe_scope` call over the whole resolved `scope` --
/// walk, project grouping, signals, enrichment, external + agent
/// discovery, evidence, store interiors -- persists everything
/// `swamp report`/the TUI need as stored Parquet current rows
/// (typed Parquet tables only, R18a-3b: no JSON render-cache row),
/// written by `observe_scope` itself. `swamp report` never runs any
/// of this; it only reads what this call leaves behind.
#[allow(clippy::too_many_arguments)]
pub fn cmd_observe(
    store_dir: PathBuf,
    scope: swamp_core::scope::EffectiveScope,
    force_full: bool,
    docker_facts: Option<PathBuf>,
    verify_du: bool,
    since: Option<String>,
    enrich: bool,
    volume: bool,
    verbose: bool,
    finish_progress: impl FnOnce(),
) -> Result<()> {
    let command_started = Instant::now();
    // Resolved before any work: a misconfigured test (the hermeticity
    // guard) fails here, not after a whole observation.
    let log = schedule::log_file_for_append();
    let config = load_config(&store_dir);
    let timeout = Duration::from_secs(config.observe_timeout_sec.max(1));
    let retention_days = config.retention_days;
    let since_secs = since
        .as_deref()
        .and_then(swamp_core::growth::parse_duration_secs)
        .unwrap_or(24 * 3600);

    let lock = match acquire_lock(&store_dir)? {
        LockOutcome::Acquired(guard) => guard,
        LockOutcome::HeldBy { pid, since } => {
            finish_progress();
            let now = swamp_core::entities::now();
            safe_println!(
                "another observation is running (pid {pid}) since {}",
                schedule::lock_since_label(since, now)
            );
            return Ok(());
        }
    };

    // A path an earlier pass was stopped on is skipped as not measured
    // for a day, so one blocking path cannot fail every scheduled pass
    // (#190).
    let mut quarantine_notes = Vec::new();
    let mut quarantine_seen = std::collections::HashSet::new();
    let mut skipped = Vec::new();
    for (at, path) in schedule::quarantined(&store_dir, swamp_core::entities::now()) {
        let reason = format!("stalled on {}", schedule::utc_date(at));
        if quarantine_seen.insert(path.clone()) {
            quarantine_notes.push((
                path.clone(),
                format!("{}: not measured ({reason})", path.display()),
            ));
        }
        skipped.push((path, reason));
    }
    swamp_core::walk::set_not_measured(skipped);
    let stall = Duration::from_secs(
        config
            .observe_stall_secs
            .max(swamp_core::growth::MIN_OBSERVE_STALL_SECS),
    );

    let start = Instant::now();
    if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
        swamp_core::work_counters::reset();
    }
    let (tx, rx) = mpsc::channel();
    let work_dir = store_dir.clone();
    let pass_scope = scope.clone();
    thread::spawn(move || {
        let res = swamp_core::report::observe_scope(
            &scope,
            ObservationParts::ALL,
            None,
            docker_facts.as_deref(),
            verify_du,
            Some(&work_dir),
            since.as_deref(),
            true,
            true, // include_dirs: `report`/the TUI need the dir rows stored too
            enrich,
            force_full,
            swamp_core::fs_events::platform_source().as_ref(),
            retention_days,
            since_secs,
        );
        // The receiver may already be gone if we timed out; that's fine,
        // the thread just finishes its work and exits.
        let _ = tx.send(res);
    });

    match wait_with(&rx, start, timeout, stall, Duration::from_secs(1)) {
        Ok(Ok(observation)) => {
            let wall_ms = start.elapsed().as_millis() as u64;
            let now = swamp_core::entities::now();
            let merged = &observation.merged;
            // `merged.notes` entries are prefixed `"[{root}] "` by
            // `merge_root_report_into` (multi-root disambiguation), so a
            // plain `strip_prefix("fsevents: ")` never matches here (it
            // did on a per-root `Report.notes`, which is unprefixed, but
            // this is the *merged* one) -- find the marker wherever it
            // falls in the line instead of requiring it at the start.
            let fsevents_line = merged
                .notes
                .iter()
                .find_map(|n| n.split_once("fsevents: ").map(|(_, rest)| rest))
                .map(str::to_string)
                .unwrap_or_else(|| "mode=full reason=no_store changed_dirs=0".to_string());
            let mode = fsevents_line
                .strip_prefix("mode=")
                .and_then(|s| s.split(' ').next())
                .unwrap_or("full")
                .to_string();
            let github = merged.github_enrichment.clone().unwrap_or_default();
            let walked_total = merged.reconciliation.walked_total;
            let projects = merged.projects.len();
            let diagnostics = verbose || trace_enabled();

            let coverage_line = coverage_summary(&observation.coverage);
            let (coverage_details, detailed_coverage_paths) =
                coverage_details(&observation.coverage, &pass_scope.roots, diagnostics);
            let mut diagnostic_lines = Vec::new();
            if diagnostics {
                diagnostic_lines.push(format!(
                    "observed_at={} wall_ms={wall_ms} walked_total={walked_total} projects={projects} external_units={} agent_units={} {fsevents_line}",
                    merged.observed_at,
                    observation.external_units.len(),
                    observation.agent_units.len(),
                ));
                for c in &observation.coverage {
                    diagnostic_lines.push(format!(
                        "  root={} status={} mode={} walked_total={} projects={}",
                        safe_console_text(&c.path.display().to_string()),
                        safe_console_text(&c.status.label()),
                        if c.mode.is_empty() { "-" } else { &c.mode },
                        c.walked_total,
                        c.projects
                    ));
                }
                diagnostic_lines.push(format!(
                    "  github: calls={} worktrees_enriched={} elapsed={:.1}s",
                    github.calls_made, github.worktrees_enriched, github.elapsed_secs
                ));
            }
            let mut exceptional_notes =
                quarantine_output_notes(&quarantine_notes, &detailed_coverage_paths);
            for (path, why) in swamp_core::signals::declined_repositories() {
                exceptional_notes.push(format!(
                    "  {}: Git repository not measured ({})",
                    safe_console_text(&path.display().to_string()),
                    safe_console_text(why)
                ));
            }
            let trace_counters = trace_enabled().then(|| {
                format!(
                    "[debug] work counters: {:?}",
                    swamp_core::work_counters::snapshot()
                )
            });
            let manager_notes =
                record_manager_reports(&store_dir, &observation.external_units, now, diagnostics)?;

            let outcome = RunOutcome {
                observed_at: now,
                wall_ms,
                walked_total,
                projects,
                mode,
                outcome: "ok".to_string(),
            };
            let mut deferred_warnings = Vec::new();
            if let Err(e) = append_log(&log, &outcome) {
                // The observation is recorded in the store either way; a
                // log that cannot be written is said once, not a failure.
                deferred_warnings.push(format!("observe log not written: {e:#}"));
            }
            write_last_run(&store_dir, &outcome)?;
            // The observation is over: its lock is released before the volume
            // pass starts, and the pass takes its own (`volume-pass.lock`), so
            // a slow or stuck pass never makes a scheduled observe say
            // "another observation is running".
            drop(lock);
            let volume_lines = volume_step(&store_dir, &config, &pass_scope, &observation, volume)?;
            finish_progress();
            for warning in deferred_warnings {
                eprintln!("{warning}");
            }
            for line in volume_lines {
                safe_println!("{line}");
            }
            for note in manager_notes {
                safe_println!("{note}");
            }
            for line in diagnostic_lines {
                safe_println!("{line}");
            }
            if let Some(counters) = trace_counters {
                eprintln!("{counters}");
            }
            for line in exceptional_notes {
                safe_println!("{line}");
            }
            for line in coverage_details {
                safe_println!("{line}");
            }
            safe_println!("{coverage_line}");
            safe_println!(
                "Observed {}, {} and {} in {}.",
                count_noun(projects, "project", "projects"),
                count_noun(
                    observation.external_units.len(),
                    "external storage unit",
                    "external storage units"
                ),
                count_noun(
                    observation.agent_units.len(),
                    "agent storage unit",
                    "agent storage units"
                ),
                schedule::format_duration_ms(command_started.elapsed().as_millis() as u64),
            );
            Ok(())
        }
        Ok(Err(e)) => {
            finish_progress();
            let wall_ms = start.elapsed().as_millis() as u64;
            let now = swamp_core::entities::now();
            let outcome = RunOutcome {
                observed_at: now,
                wall_ms,
                walked_total: 0,
                projects: 0,
                mode: "full".to_string(),
                outcome: format!("error({e})"),
            };
            if let Err(e) = append_log(&log, &outcome) {
                // The observation is recorded in the store either way; a
                // log that cannot be written is said once, not a failure.
                eprintln!("observe log not written: {e:#}");
            }
            let _ = write_last_run(&store_dir, &outcome);
            drop(lock);
            eprintln!("observe failed: {e}");
            std::process::exit(1);
        }
        Err(Waited::TimedOut(stuck)) => {
            finish_progress();
            let wall_ms = start.elapsed().as_millis() as u64;
            let now = swamp_core::entities::now();
            let outcome = RunOutcome {
                observed_at: now,
                wall_ms,
                walked_total: 0,
                projects: 0,
                mode: "full".to_string(),
                outcome: timeout_outcome(stuck.as_ref()),
            };
            if let Some((_, path, _)) = &stuck {
                let _ = schedule::record_stalled(&store_dir, path, now);
            }
            if let Err(e) = append_log(&log, &outcome) {
                // The observation is recorded in the store either way; a
                // log that cannot be written is said once, not a failure.
                eprintln!("observe log not written: {e:#}");
            }
            let _ = write_last_run(&store_dir, &outcome);
            // Released before exiting so the next observation (and the
            // TUI's first-run scan) is not left waiting on a pass that is
            // parked in one blocking filesystem call (#190).
            drop(lock);
            eprintln!(
                "observe stopped after {}: {}",
                schedule::format_duration_ms(wall_ms),
                outcome.outcome
            );
            std::process::exit(1);
        }
        Err(Waited::Died) => {
            drop(lock);
            bail!("observe worker thread died without reporting a result");
        }
    }
}

/// How a wait for the observation thread ended without a result.
enum Waited {
    /// Past `observe_timeout_sec`, or nothing progressed for
    /// `observe_stall_secs`; carries the step running longest (phase,
    /// path, how long), if any.
    TimedOut(Option<(&'static str, PathBuf, Duration)>),
    Died,
}

/// Waits for the pass, checking the progress beacon every `tick`: a pass
/// that is slow but still moving (directories entered and left, long
/// listings beating) is never stopped early; one where nothing has moved
/// for `stall` is (#190).
fn wait_with<T>(
    rx: &mpsc::Receiver<T>,
    start: Instant,
    timeout: Duration,
    stall: Duration,
    tick: Duration,
) -> std::result::Result<T, Waited> {
    loop {
        let left = timeout.saturating_sub(start.elapsed());
        match rx.recv_timeout(left.min(tick)) {
            Ok(v) => return Ok(v),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Waited::Died),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let idle = swamp_core::beacon::idle_for().min(start.elapsed());
                if idle >= stall || start.elapsed() >= timeout {
                    return Err(Waited::TimedOut(swamp_core::beacon::stuck()));
                }
            }
        }
    }
}

/// The log's outcome for a stopped pass: which step it was stuck in, and
/// on which path, when one was running.
fn timeout_outcome(stuck: Option<&(&'static str, PathBuf, Duration)>) -> String {
    match stuck {
        Some((phase, path, d)) => format!(
            "timeout(stuck {}s in {phase} at {})",
            d.as_secs(),
            path.display()
        ),
        None => "timeout".to_string(),
    }
}

/// The scheduled manager pass: after a successful observation, ask each
/// package manager that owns a measured unit its own read-only questions
/// (`manager_facts::collect`) and store the answers. Only `observe` runs
/// it; `report` and the TUI read what it stored. A manager that is
/// missing, slow or unreadable is a `not observed` row, never a failure
/// of the observation.
fn record_manager_reports(
    store_dir: &std::path::Path,
    units: &[swamp_core::external::ExternalUnit],
    now: u64,
    diagnostics: bool,
) -> Result<Vec<String>> {
    let started = std::time::Instant::now();
    let facts = swamp_core::manager_facts::collect(
        units,
        &swamp_core::manager_facts::SystemProbeRunner,
        now,
        store_dir,
    );
    let mut messages = Vec::new();
    if trace_enabled() {
        messages.push(format!(
            "[trace] manager reports: {}",
            schedule::format_duration_ms(started.elapsed().as_millis() as u64)
        ));
    }
    let not_observed = facts
        .iter()
        .filter(|f| f.kind == swamp_core::manager_facts::FactKind::NotObserved)
        .count();
    match swamp_core::growth::write_manager_fact_table(store_dir, &facts) {
        Ok(()) if diagnostics => messages.push(format!(
            "manager reports: {} rows recorded, {not_observed} not observed",
            facts.len()
        )),
        Ok(()) => {}
        Err(e) => messages.push(format!("manager reports not recorded: {e}")),
    }
    Ok(messages)
}

/// Disk accounting runs after observation, with the observation lock released.
/// It never fails the observation; any limits or failures are returned as
/// user-facing messages, and unavailable measurements remain in ledger rows.
fn volume_step(
    store_dir: &std::path::Path,
    config: &swamp_core::growth::GrowthConfig,
    scope: &swamp_core::scope::EffectiveScope,
    observation: &swamp_core::report::ScopeObservation,
    force: bool,
) -> Result<Vec<String>> {
    use swamp_core::volume_ledger::pass::{PassOutcome, allowed, run_after_observation};
    let home = swamp_core::locations::Environment::from_process().home;
    let account_home = swamp_core::volume_ledger::pass::account_home();
    if let Err(swamp_core::volume_ledger::pass::NotAllowed(why)) = allowed(
        scope.explicit,
        force,
        config.volume_pass_interval_hours,
        &home,
        account_home.as_deref(),
    ) {
        return Ok(if force {
            vec![format!("Disk accounting skipped: {why}")]
        } else {
            Vec::new()
        });
    }
    let mut config = config.clone();
    let mut messages = Vec::new();
    if config.volume_pass_budget_secs < 5 {
        messages.push(
            "Volume scan budget was below the five-second minimum; using five seconds.".to_string(),
        );
        config.volume_pass_budget_secs = 5;
    }
    match run_after_observation(store_dir, &config, scope, observation, &home, force) {
        Ok(PassOutcome::Ran(summary)) => messages.push(summary.line()),
        Ok(PassOutcome::Skipped(line)) => messages.push(line),
        Ok(PassOutcome::NotDue(_)) => {}
        Err(e) => messages.push(format!("Disk accounting failed: {e}")),
    }
    Ok(messages)
}

#[cfg(test)]
mod observe_output_tests {
    use super::*;

    fn root_coverage(
        status: swamp_core::coverage::RegionStatus,
    ) -> swamp_core::coverage::RootCoverage {
        root_coverage_at("/root", status)
    }

    fn root_coverage_at(
        path: &str,
        status: swamp_core::coverage::RegionStatus,
    ) -> swamp_core::coverage::RootCoverage {
        swamp_core::coverage::RootCoverage {
            path: PathBuf::from(path),
            status,
            walked_total: 0,
            projects: 0,
            mode: String::new(),
            reached_by_registry: Vec::new(),
        }
    }

    #[test]
    fn coverage_summary_distinguishes_measurement_outcomes() {
        use swamp_core::coverage::RegionStatus as Status;
        let coverage = vec![
            root_coverage(Status::Complete),
            root_coverage(Status::Partial {
                reason: "permission denied".to_string(),
            }),
            root_coverage(Status::DetectorOnly),
            root_coverage(Status::NotMeasured {
                reason: "stalled".to_string(),
            }),
            root_coverage(Status::Inaccessible {
                reason: "permission denied".to_string(),
            }),
            root_coverage(Status::Excluded),
            root_coverage(Status::Missing),
        ];
        assert_eq!(
            coverage_summary(&coverage),
            "Coverage: 1 complete root, 1 partial root, 1 tool location not scanned for projects, 1 root not measured, 1 inaccessible root, 1 excluded root, 1 missing root."
        );
        assert_eq!(
            coverage_summary(&[]),
            "Coverage: no root coverage was returned."
        );
    }

    #[test]
    fn count_noun_uses_singular_and_grouped_plural_forms() {
        assert_eq!(count_noun(1, "project", "projects"), "1 project");
        assert_eq!(count_noun(2, "project", "projects"), "2 projects");
        assert_eq!(count_noun(1_000, "project", "projects"), "1,000 projects");
    }

    #[test]
    fn console_paths_cannot_inject_terminal_control_characters() {
        assert_eq!(safe_console_text("/tmp/a\n\u{1b}[31m"), "/tmp/a??[31m");
    }

    #[test]
    fn missing_detector_locations_are_summarized_unless_diagnostics_are_requested() {
        use swamp_core::coverage::RegionStatus as Status;
        use swamp_core::locations::{Provenance, StorageCategory};
        use swamp_core::scope::{RootReason, RootStatus, ScopeRoot};

        let optional_paths = [
            PathBuf::from("/home/user/.rvm"),
            PathBuf::from("/home/user/.cache/tool"),
        ];
        let roots = vec![
            ScopeRoot {
                path: optional_paths[0].clone(),
                reasons: vec![RootReason::Detector {
                    detector_id: "test-tool".into(),
                    category: StorageCategory::Environments,
                    provenance: Provenance::BuiltinConvention,
                }],
                status: RootStatus::Missing,
            },
            ScopeRoot {
                path: optional_paths[1].clone(),
                reasons: vec![RootReason::Detector {
                    detector_id: "test-tool".into(),
                    category: StorageCategory::Cache,
                    provenance: Provenance::BuiltinConvention,
                }],
                status: RootStatus::Missing,
            },
            ScopeRoot {
                path: PathBuf::from("/work/project"),
                reasons: vec![RootReason::Included],
                status: RootStatus::Missing,
            },
        ];
        let coverage = vec![
            root_coverage_at("/home/user/.rvm", Status::Missing),
            root_coverage_at("/home/user/.cache/tool", Status::Missing),
            root_coverage_at("/work/project", Status::Missing),
            root_coverage_at(
                "/work/blocked",
                Status::NotMeasured {
                    reason: "stalled".into(),
                },
            ),
        ];

        let (default_lines, default_detail_paths) = coverage_details(&coverage, &roots, false);
        assert!(default_lines.contains(&"  2 absent optional tool locations".into()));
        assert!(!default_lines.iter().any(|line| line.contains(".rvm")));
        assert!(
            default_lines
                .iter()
                .any(|line| line.contains("/work/project: missing"))
        );
        assert!(
            default_lines
                .iter()
                .any(|line| line.contains("/work/blocked: not measured"))
        );
        assert!(!default_detail_paths.contains(&optional_paths[0]));

        let (diagnostic_lines, diagnostic_paths) = coverage_details(&coverage, &roots, true);
        assert!(
            diagnostic_lines
                .iter()
                .any(|line| line.contains("/home/user/.rvm: missing"))
        );
        assert!(
            diagnostic_lines
                .iter()
                .any(|line| line.contains("/home/user/.cache/tool: missing"))
        );
        assert!(diagnostic_paths.contains(&optional_paths[0]));
    }

    #[test]
    fn quarantine_note_is_suppressed_when_coverage_already_names_the_path() {
        let path = PathBuf::from("/work/Music");
        let notes = vec![
            (
                path.clone(),
                "/work/Music: not measured (stalled on today)".to_string(),
            ),
            (
                PathBuf::from("/work/Other"),
                "/work/Other: not measured".to_string(),
            ),
        ];
        assert_eq!(
            quarantine_output_notes(&notes, std::slice::from_ref(&path)),
            vec!["  /work/Other: not measured"]
        );
    }
}

/// `swamp schedule [--every <interval>] [--off] <root>...`.
pub fn cmd_schedule(
    store_dir: PathBuf,
    every: Option<String>,
    off: bool,
    collector: bool,
    roots: Vec<PathBuf>,
) -> Result<()> {
    // Each call is bound before it is printed rather than written inside
    // `safe_print!`: the source audit's call graph does not see through macro
    // tokens, and `platform_capabilities_gate_their_backends` walks from
    // here to prove every write is behind the platform's scheduling check.
    // A call hidden in a macro argument is a path that rule cannot follow.
    if off {
        let message = schedule::uninstall()?;
        safe_print!("{message}");
        return Ok(());
    }
    if let Some(interval) = every {
        let message = schedule::install(&interval, &roots, collector)?;
        safe_print!("{message}");
        return Ok(());
    }
    let message = schedule::status(&store_dir)?;
    safe_print!("{message}");
    Ok(())
}

#[cfg(test)]
mod watchdog_tests {
    use super::*;
    use std::path::Path;

    /// The beacon is process-wide; tests that read it run one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A pass whose result never comes back is stopped by the overall
    /// timeout. Tempting wrong patch: a single `recv_timeout(timeout)`,
    /// which cannot notice a stall before `observe_timeout_sec` runs out.
    #[test]
    fn a_silent_pass_times_out_and_names_the_stuck_step() {
        let (_tx, rx) = mpsc::channel::<()>();
        let start = Instant::now();
        let got = wait_with(
            &rx,
            start,
            Duration::from_millis(200),
            Duration::from_secs(3600),
            Duration::from_millis(20),
        );
        assert!(matches!(got, Err(Waited::TimedOut(_))));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(timeout_outcome(None), "timeout");
        let named = timeout_outcome(Some(&(
            "git signals",
            PathBuf::from("/x/proj"),
            Duration::from_secs(301),
        )));
        assert_eq!(named, "timeout(stuck 301s in git signals at /x/proj)");
    }

    /// Slow but progressing: a step that keeps beating (a 200k-entry
    /// listing under load) is not stopped, however long it takes.
    /// Tempting wrong patch: stopping on the age of the oldest running
    /// step rather than on time since the last progress.
    #[test]
    fn a_slow_pass_that_keeps_progressing_is_not_stopped() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _step = swamp_core::beacon::enter("walk", Path::new("/x/huge"));
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(50));
                swamp_core::beacon::beat();
            }
            drop(_step);
            let _ = tx.send(());
        });
        let got = wait_with(
            &rx,
            Instant::now(),
            Duration::from_secs(60),
            Duration::from_millis(400),
            Duration::from_millis(20),
        );
        worker.join().unwrap();
        assert!(got.is_ok(), "a progressing pass was stopped");
    }

    /// Parked: a step that stops beating is stopped at `stall`, long
    /// before the overall timeout, and named.
    #[test]
    fn a_parked_step_is_stopped_at_the_stall_bound_and_named() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let (_tx, rx) = mpsc::channel::<()>();
        let (park_tx, park_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _step = swamp_core::beacon::enter("ignore lens", Path::new("/x/parked"));
            ready_tx.send(()).unwrap();
            let _ = park_rx.recv();
        });
        ready_rx.recv().unwrap();
        let start = Instant::now();
        let got = wait_with(
            &rx,
            start,
            Duration::from_secs(60),
            Duration::from_millis(300),
            Duration::from_millis(20),
        );
        assert!(start.elapsed() < Duration::from_secs(10));
        match got {
            Err(Waited::TimedOut(Some((phase, path, _)))) => {
                assert_eq!((phase, path), ("ignore lens", PathBuf::from("/x/parked")));
            }
            _ => panic!("not stopped at the stall bound"),
        }
        park_tx.send(()).unwrap();
        worker.join().unwrap();
    }

    /// docs/usage.md states the stall key and the log line.
    #[test]
    fn usage_doc_states_the_stall_key_and_log_line() {
        let doc = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/usage.md"),
        )
        .unwrap();
        assert!(doc.contains(&format!(
            "observe_stall_secs = {}",
            swamp_core::growth::DEFAULT_OBSERVE_STALL_SECS
        )));
        assert!(doc.contains(&format!(
            "minimum {}",
            swamp_core::growth::MIN_OBSERVE_STALL_SECS
        )));
        assert!(doc.contains("timeout(stuck <N>s in <phase> at <path>)"));
    }
}
