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

    let start = Instant::now();
    if std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty()) {
        swamp_core::work_counters::reset();
    }
    let (tx, rx) = mpsc::channel();
    let work_dir = store_dir.clone();
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

    match wait_for_observation(&rx, start, timeout) {
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
            drop(lock);
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

/// A walk that has sat in one directory this long is stuck in a blocking
/// filesystem call, not slow: the slowest single directory measured on
/// the maintainer's machine lists in well under a second. The pass is
/// abandoned (lock released, path logged) instead of holding the writer
/// lock for the rest of `observe_timeout_sec` (#190).
const DIRECTORY_STALL: Duration = Duration::from_secs(300);

/// How a wait for the observation thread ended without a result.
enum Waited {
    /// Past `observe_timeout_sec`, or one directory past
    /// [`DIRECTORY_STALL`]; carries the directory the walk was inside
    /// longest, if any, and for how long.
    TimedOut(Option<(PathBuf, Duration)>),
    Died,
}

fn wait_for_observation<T>(
    rx: &mpsc::Receiver<T>,
    start: Instant,
    timeout: Duration,
) -> std::result::Result<T, Waited> {
    wait_with(rx, start, timeout, DIRECTORY_STALL, Duration::from_secs(5))
}

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
                let oldest = swamp_core::walk::in_flight::oldest();
                let stalled = oldest.as_ref().is_some_and(|(_, d)| *d >= stall);
                if stalled || start.elapsed() >= timeout {
                    return Err(Waited::TimedOut(oldest));
                }
            }
        }
    }
}

/// The log's outcome for a stopped pass: which directory it was stuck
/// in, when the walk was inside one.
fn timeout_outcome(stuck: Option<&(PathBuf, Duration)>) -> String {
    match stuck {
        Some((path, d)) => format!("timeout(stuck {}s in {})", d.as_secs(), path.display()),
        None => "timeout".to_string(),
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

    /// A pass whose result never comes back is stopped by the overall
    /// timeout; the channel's sender is kept alive so this is a timeout,
    /// not a dead worker. Tempting wrong patch: a single
    /// `recv_timeout(timeout)`, which cannot notice a stalled directory
    /// before the full `observe_timeout_sec` (30 minutes) runs out.
    #[test]
    fn a_silent_pass_times_out_and_a_stalled_directory_stops_it_early() {
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
            PathBuf::from("/x/Caches"),
            Duration::from_secs(301),
        )));
        assert_eq!(named, "timeout(stuck 301s in /x/Caches)");
    }

    /// docs/usage.md states the stall bound and the log line; keep both
    /// in step with the code.
    #[test]
    fn usage_doc_states_the_directory_stall_bound() {
        let doc = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/usage.md"),
        )
        .unwrap();
        assert!(doc.contains(&format!(
            "one directory for {} seconds",
            DIRECTORY_STALL.as_secs()
        )));
        assert!(doc.contains("timeout(stuck <N>s in <path>)"));
    }
}
