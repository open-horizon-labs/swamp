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

    match rx.recv_timeout(timeout) {
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
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let wall_ms = start.elapsed().as_millis() as u64;
            let now = swamp_core::entities::now();
            let outcome = RunOutcome {
                observed_at: now,
                wall_ms,
                walked_total: 0,
                projects: 0,
                mode: "full".to_string(),
                outcome: "timeout".to_string(),
            };
            append_log(&log_file(), &outcome)?;
            let _ = write_last_run(&store_dir, &outcome);
            drop(lock);
            eprintln!("observe timed out after {}s", config.observe_timeout_sec);
            std::process::exit(1);
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            drop(lock);
            bail!("observe worker thread died without reporting a result");
        }
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
