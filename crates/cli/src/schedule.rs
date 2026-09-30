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
    self, LockOutcome, RunOutcome, acquire_lock, append_log, log_file, write_last_run,
};

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
) -> Result<()> {
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
            safe_println!("another observation is running (pid {pid}) since {since}");
            return Ok(());
        }
    };

    // A path an earlier pass was stopped on is skipped as not measured
    // for a day, so one blocking path cannot fail every scheduled pass
    // (#190).
    let mut quarantine_notes = Vec::new();
    let mut skipped = Vec::new();
    for (at, path) in schedule::quarantined(&store_dir, swamp_core::entities::now()) {
        let reason = format!("stalled on {}", schedule::utc_date(at));
        quarantine_notes.push(format!("{}: not measured ({reason})", path.display()));
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

            safe_println!(
                "observed_at={} wall_ms={wall_ms} walked_total={walked_total} projects={projects} external_units={} agent_units={} {fsevents_line}",
                merged.observed_at,
                observation.external_units.len(),
                observation.agent_units.len(),
            );
            for c in &observation.coverage {
                safe_println!(
                    "  root={} mode={} walked_total={} projects={}",
                    c.path.display(),
                    if c.mode.is_empty() { "-" } else { &c.mode },
                    c.walked_total,
                    c.projects
                );
            }
            for note in &quarantine_notes {
                safe_println!("  {note}");
            }
            for (path, why) in swamp_core::signals::declined_repositories() {
                safe_println!("  {}: git repository not measured ({why})", path.display());
            }
            safe_println!(
                "  github: calls={} worktrees_enriched={} elapsed={:.1}s",
                github.calls_made,
                github.worktrees_enriched,
                github.elapsed_secs
            );
            if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
                eprintln!(
                    "[debug] work counters: {:?}",
                    swamp_core::work_counters::snapshot()
                );
            }

            let outcome = RunOutcome {
                observed_at: now,
                wall_ms,
                walked_total,
                projects,
                mode,
                outcome: "ok".to_string(),
            };
            append_log(&log_file(), &outcome)?;
            write_last_run(&store_dir, &outcome)?;
            // The observation is over: its lock is released before the volume
            // pass starts, and the pass takes its own (`volume-pass.lock`), so
            // a slow or stuck pass never makes a scheduled observe say
            // "another observation is running".
            drop(lock);
            volume_step(&store_dir, &config, &pass_scope, &observation, volume)?;
            Ok(())
        }
        Ok(Err(e)) => {
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
            append_log(&log_file(), &outcome)?;
            let _ = write_last_run(&store_dir, &outcome);
            drop(lock);
            eprintln!("observe failed: {e}");
            std::process::exit(1);
        }
        Err(Waited::TimedOut(stuck)) => {
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
            append_log(&log_file(), &outcome)?;
            let _ = write_last_run(&store_dir, &outcome);
            // Released before exiting so the next observation (and the
            // TUI's first-run scan) is not left waiting on a pass that is
            // parked in one blocking filesystem call (#190).
            drop(lock);
            eprintln!(
                "observe stopped after {}s: {}",
                wall_ms / 1000,
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

/// The volume pass, after a successful observation and still under the
/// single-flight observe lock. It never fails the observation: its result
/// is one line, and what it could not do is in the ledger's own rows.
fn volume_step(
    store_dir: &std::path::Path,
    config: &swamp_core::growth::GrowthConfig,
    scope: &swamp_core::scope::EffectiveScope,
    observation: &swamp_core::report::ScopeObservation,
    force: bool,
) -> Result<()> {
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
        if force {
            safe_println!("volume pass skipped: {why}");
        }
        return Ok(());
    }
    let mut config = config.clone();
    if config.volume_pass_budget_secs < 5 {
        safe_println!(
            "volume_pass_budget_secs = {} is below the 5 s minimum; using 5",
            config.volume_pass_budget_secs
        );
        config.volume_pass_budget_secs = 5;
    }
    match run_after_observation(store_dir, &config, scope, observation, &home, force) {
        Ok(PassOutcome::Ran(summary)) => safe_println!("{}", summary.line()),
        Ok(PassOutcome::Skipped(line)) => safe_println!("{line}"),
        Ok(PassOutcome::NotDue(_)) => {}
        Err(e) => safe_println!("volume pass failed: {e}"),
    }
    Ok(())
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
