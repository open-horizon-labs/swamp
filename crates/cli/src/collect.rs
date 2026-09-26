//! `swamp collect`: the opt-in, user-owned collector (#82).
//!
//! Linux keeps no change history, so the only way a later one-shot
//! `swamp report`/`observe` can avoid a full walk is for something to
//! have been watching the whole time. This is that something -- a
//! foreground process the user starts (directly, under their own
//! service manager, or through `swamp schedule --collector`), that
//! watches the scope's roots with inotify and keeps a bounded checkpoint
//! in the store. It never deletes, never renders, and stops on SIGINT or
//! SIGTERM.
//!
//! macOS does not need one and refuses: FSEvents keeps the history
//! itself, and a resident process there would be cost with no benefit.

use crate::safe_println;
use anyhow::Result;
use std::path::PathBuf;

/// One root to collect for, with the exclusions under it.
pub struct Root {
    pub path: PathBuf,
    /// Read only by the Linux collector.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub excluded: Vec<PathBuf>,
}

pub fn cmd_collect(store_dir: PathBuf, roots: Vec<Root>) -> Result<()> {
    match swamp_core::platform::Os::current() {
        swamp_core::platform::Os::MacOs => anyhow::bail!(
            "swamp collect is for platforms without persisted change history. macOS has one \
             (FSEvents): every observation replays it, with no process left running. Use \
             `swamp schedule --every <interval>` for periodic observation."
        ),
        swamp_core::platform::Os::Linux => run(store_dir, roots),
    }
}

#[cfg(target_os = "linux")]
fn run(store_dir: PathBuf, roots: Vec<Root>) -> Result<()> {
    let stop = swamp_core::continuity::stop_on_signals();
    let roots: Vec<swamp_core::continuity::CollectRoot> = roots
        .into_iter()
        .map(|r| swamp_core::continuity::CollectRoot {
            root: r.path,
            excluded: r.excluded,
        })
        .collect();
    eprintln!(
        "swamp collect: watching {} root(s); stop with Ctrl-C or SIGTERM",
        roots.len()
    );
    swamp_core::continuity::run_collector(&store_dir, &roots, stop, |line| {
        eprintln!("swamp collect: {line}");
    })
}

#[cfg(not(target_os = "linux"))]
fn run(_store_dir: PathBuf, _roots: Vec<Root>) -> Result<()> {
    anyhow::bail!("swamp collect has no backend on this platform")
}

/// `swamp collect --status [--json]`: what each root's checkpoint says,
/// and whether its collector is alive. Reads only.
pub fn cmd_collect_status(store_dir: PathBuf, roots: Vec<PathBuf>, json: bool) -> Result<()> {
    let mut rows = Vec::new();
    for root in roots {
        let p = swamp_core::continuity::paths(&store_dir, &root);
        let alive = swamp_core::continuity::collector_alive(&p);
        let ck = swamp_core::continuity::read_checkpoint(&p);
        rows.push((root, alive, ck));
    }
    if json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|(root, alive, ck)| {
                serde_json::json!({
                    "root": root,
                    "collector_running": alive,
                    "checkpoint": ck.as_ref().map(|c| serde_json::json!({
                        "epoch_id": c.epoch_id,
                        "opened_at": c.opened_at,
                        "coverage": match &c.lost {
                            None => serde_json::json!({"status": "complete"}),
                            Some(l) => serde_json::json!({"status": "lost", "reason": l.reason, "detail": l.detail, "at": l.at}),
                        },
                        "previous_loss": c.previous_loss,
                        "dirty_directories": c.dirty.len(),
                        "watches": c.watches,
                        "max_user_watches": c.max_user_watches,
                        "kernel_bytes_estimate": c.kernel_bytes_estimate,
                        "flushed_at": c.flushed_at,
                        "stopped_at": c.stopped_at,
                        "pid": c.pid,
                    })),
                })
            })
            .collect();
        let text = serde_json::to_string_pretty(&serde_json::json!({ "roots": out }))?;
        safe_println!("{text}");
        return Ok(());
    }
    for (root, alive, ck) in rows {
        safe_println!("{}", root.display());
        match (alive, ck) {
            (_, None) => safe_println!("  no collector checkpoint: observations walk fully"),
            (false, Some(c)) => safe_println!(
                "  collector not running (last flush {}, pid {}): its list cannot vouch for \
                 anything since, so observations walk fully",
                c.flushed_at,
                c.pid
            ),
            (true, Some(c)) => {
                safe_println!(
                    "  collector running (pid {}), epoch opened at {}",
                    c.pid,
                    c.opened_at
                );
                match &c.lost {
                    None => safe_println!(
                        "  coverage complete; {} directories changed since the last observation",
                        c.dirty.len()
                    ),
                    Some(l) => safe_println!("  coverage lost: {} ({})", l.reason, l.detail),
                }
                safe_println!(
                    "  {} inotify watches (limit {}), ~{} of kernel memory",
                    c.watches,
                    c.max_user_watches
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "unknown".into()),
                    swamp_core::render::human_bytes_pub(c.kernel_bytes_estimate)
                );
            }
        }
    }
    Ok(())
}
