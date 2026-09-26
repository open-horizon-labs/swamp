//! swamp reports; the human decides. A row is marked (Space), the
//! confirm banner shows its current facts (Backspace), and Enter moves
//! it to the Trash. There is no plan store, no grant, no confirmation
//! token: this module drives `swamp_core::fs_gate::destroy` directly on
//! exactly what was marked. The only way a move refuses is an ordinary
//! OS-level error (permission denied, the path is gone, cross-device).

use anyhow::Result;
use std::path::{Path, PathBuf};
use swamp_core::entities::{id_for, now};
use swamp_core::execution::Outcome;
use swamp_core::ledger::{ActionRecord, Ledger, LedgerFact, NO_GRANT, Verb};

use crate::model::human_bytes;

/// One unit the human marked for deletion: enough facts to move exactly
/// what was shown, without a second data path (the facts all came from
/// the one `Report`).
#[derive(Debug, Clone)]
pub struct MarkedUnit {
    /// Set when the row is a Cargo purpose group: its member list is
    /// exactly what moves together into one Trash envelope.
    pub cargo_unit: Option<swamp_core::actions::PlanUnit>,
    /// Set when the row is an agent-storage unit (#101): a single-path
    /// cache/log move, or a session's member list.
    pub agent_unit: Option<swamp_core::actions::PlanUnit>,
    pub path: PathBuf,
    /// Set for a Docker object: what removing it actually runs, and the
    /// fact that it never reaches Trash.
    pub docker: Option<swamp_core::docker::Removal>,
    /// The worktree containing the unit (itself, for a worktree row).
    pub worktree_path: PathBuf,
    pub bytes: u64,
    pub observed_at: u64,
    /// Set when the unit is a linked worktree rather than an artifact dir.
    pub worktree: Option<WorktreeTerms>,
    /// The row's label, for the confirm line.
    pub label: String,
    /// The plain-language consequence of deleting this unit (dirty,
    /// unpushed, untracked content, no remote, git store, what has files
    /// open…). Shown on the confirm banner; never a veto.
    pub warnings: Vec<String>,
}

/// The terms a worktree removal was authorized on; recorded in the ledger.
#[derive(Debug, Clone)]
pub struct WorktreeTerms {
    pub merge_complete: bool,
    pub pr: Option<String>,
    /// True when the unit is a whole checkout (the `archive` verb), not a
    /// linked worktree: a higher bar, since it takes the working copy.
    pub whole_checkout: bool,
    pub remote: Option<String>,
}

/// Result of executing one marked unit, for the inline per-unit outcome.
#[derive(Debug, Clone)]
pub struct UnitResult {
    pub path: PathBuf,
    pub outcome: Result<Outcome, String>,
}

/// The facts the human saw on the confirm line, as typed ledger rows.
fn ledger_evidence(unit: &MarkedUnit, extra: Vec<LedgerFact>) -> Vec<LedgerFact> {
    let mut evidence = vec![
        LedgerFact::new("label", &unit.label),
        LedgerFact::new("bytes", unit.bytes),
        LedgerFact::new("observed_at", unit.observed_at),
        LedgerFact::new("warnings_shown", unit.warnings.join("; ")),
    ];
    evidence.extend(extra);
    evidence
}

/// Moves one unit to the Trash and appends one ledger line. Nothing here
/// is re-derived and compared against what was true at mark time: the
/// human already saw the current facts on the confirm banner, and the
/// only failures below are ordinary OS-level errors.
fn execute_one(
    unit: &MarkedUnit,
    ledger: &Ledger,
    trash_root: &Path,
    keep_executables: bool,
) -> UnitResult {
    let actor = "human:tui";
    if let Some(u) = unit.cargo_unit.as_ref() {
        let result = (|| -> Result<Outcome> {
            if keep_executables {
                anyhow::bail!("keep-executables conflicts with selective build removal");
            }
            let group = u
                .cargo_group()
                .ok_or_else(|| anyhow::anyhow!("marked Cargo unit carries no group"))?;
            let dest = swamp_core::actions::trash_cargo_group(group, trash_root)?;
            ledger.append(&ActionRecord {
                id: swamp_core::entities::new_id(),
                verb: Verb::Delete,
                entity_id: id_for(&unit.path.display().to_string()),
                evidence: ledger_evidence(
                    unit,
                    vec![LedgerFact::new("cargo_group", format!("{group:?}"))],
                ),
                grant_id: NO_GRANT.to_string(),
                actor: actor.into(),
                outcome: "completed".into(),
                recovery_location: Some(dest.clone()),
                measured_free_space_delta: None,
                observed_path_state: Some("trashed".into()),
                recorded_at: now(),
            })?;
            Ok(Outcome {
                unit_id: id_for(&unit.path.display().to_string()),
                status: "completed".into(),
                reason: None,
                intended_bytes: unit.bytes,
                observed_free_space_delta: None,
            })
        })();
        return UnitResult {
            path: unit.path.clone(),
            outcome: result.map_err(|e| e.to_string()),
        };
    }
    if let Some(u) = unit.agent_unit.as_ref() {
        let result = (|| -> Result<Outcome> {
            let meta = u
                .agent_meta()
                .ok_or_else(|| anyhow::anyhow!("marked agent unit carries no agent_meta"))?;
            let at = now();
            let (dest, _bytes) = match &meta.session_members {
                Some(members) => swamp_core::actions::trash_agent_session(
                    meta, &unit.path, members, trash_root, at,
                )
                .map_err(|e| {
                    if let Some(partial) =
                        e.downcast_ref::<swamp_core::actions::PartialAgentRemoval>()
                    {
                        anyhow::anyhow!("{partial}")
                    } else {
                        e
                    }
                })?,
                None => swamp_core::actions::trash_agent_cache(&unit.path, trash_root, at)?,
            };
            ledger.append(&ActionRecord {
                id: swamp_core::entities::new_id(),
                verb: Verb::Delete,
                entity_id: id_for(&unit.path.display().to_string()),
                evidence: ledger_evidence(
                    unit,
                    vec![
                        LedgerFact::new("tool_id", &meta.tool_id),
                        LedgerFact::new("category", format!("{:?}", meta.category)),
                        LedgerFact::new("session_removal", meta.session_members.is_some()),
                    ],
                ),
                grant_id: NO_GRANT.to_string(),
                actor: actor.into(),
                outcome: "completed".into(),
                recovery_location: Some(dest),
                measured_free_space_delta: None,
                observed_path_state: Some("trashed".into()),
                recorded_at: now(),
            })?;
            Ok(Outcome {
                unit_id: id_for(&unit.path.display().to_string()),
                status: "completed".into(),
                reason: None,
                intended_bytes: unit.bytes,
                observed_free_space_delta: None,
            })
        })();
        return UnitResult {
            path: unit.path.clone(),
            outcome: result.map_err(|e| e.to_string()),
        };
    }
    let outcome = if let Some(terms) = &unit.worktree {
        remove_worktree(unit, terms, ledger, trash_root, actor)
    } else if let Some(target) = &unit.docker {
        remove_docker(unit, target, ledger, actor)
    } else {
        trash_path(
            unit,
            Verb::Delete,
            ledger,
            trash_root,
            actor,
            keep_executables,
            Vec::new(),
        )
        .map(|(outcome, _)| outcome)
    };
    UnitResult {
        path: unit.path.clone(),
        outcome: outcome.map_err(|e| e.to_string()),
    }
}

/// Removes one Docker object, permanently, through the daemon. The
/// daemon's own refusal text is the outcome when it declines (an image a
/// container still references, a volume still mounted). Nothing here is
/// reversible, so `recovery_location` is `None` and the ledger says so.
fn remove_docker(
    unit: &MarkedUnit,
    target: &swamp_core::docker::Removal,
    ledger: &Ledger,
    actor: &str,
) -> Result<Outcome> {
    swamp_core::docker::remove(target).map_err(|e| anyhow::anyhow!(e))?;
    ledger.append(&ActionRecord {
        id: swamp_core::entities::new_id(),
        verb: Verb::Delete,
        entity_id: id_for(&unit.path.display().to_string()),
        evidence: ledger_evidence(
            unit,
            vec![
                LedgerFact::new("docker", format!("{target:?}")),
                LedgerFact::new("permanent", true),
            ],
        ),
        grant_id: NO_GRANT.to_string(),
        actor: actor.to_string(),
        outcome: "completed".to_string(),
        // The daemon has no Trash: there is nowhere to point at.
        recovery_location: None,
        measured_free_space_delta: None,
        observed_path_state: Some("removed via docker".to_string()),
        recorded_at: now(),
    })?;
    Ok(Outcome {
        unit_id: unit.path.display().to_string(),
        status: "completed".to_string(),
        reason: None,
        intended_bytes: unit.bytes,
        observed_free_space_delta: None,
    })
}

/// Moves one path to Trash and records it. The only refusal is an
/// OS-level error: the path is gone, permission denied, or a
/// cross-device Trash with no permanent-delete fallback. With
/// `keep_executables`, compiled outputs are copied out before the move.
#[allow(clippy::too_many_arguments)]
fn trash_path(
    unit: &MarkedUnit,
    verb: Verb,
    ledger: &Ledger,
    trash_root: &Path,
    actor: &str,
    keep_executables: bool,
    extra: Vec<LedgerFact>,
) -> Result<(Outcome, swamp_core::fs_gate::destroy::Trashed)> {
    let path = &unit.path;
    if swamp_core::fs_gate::symlink_metadata(path).is_err() {
        anyhow::bail!("path no longer exists");
    }
    let mut preserved_note = None;
    if keep_executables {
        let bin = unit.worktree_path.join("bin");
        let kept = swamp_core::actions::preserve_executables(path, &bin)
            .map_err(|e| anyhow::anyhow!("could not preserve executables: {e}"))?;
        preserved_note = Some(
            kept.iter()
                .map(|k| k.to.display().to_string())
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("item");
    let moved =
        swamp_core::fs_gate::destroy::trash_move(path, trash_root, &format!("{name}-{}", now()))?;
    debug_assert_eq!(
        moved.anchor(),
        path,
        "the receipt names what was actually moved"
    );
    let mut evidence = ledger_evidence(unit, extra);
    if let Some(preserved) = preserved_note {
        evidence.push(LedgerFact::new("preserved", preserved));
    }
    ledger.append(&ActionRecord {
        id: swamp_core::entities::new_id(),
        verb,
        entity_id: id_for(&path.display().to_string()),
        evidence,
        grant_id: NO_GRANT.to_string(),
        actor: actor.into(),
        outcome: "completed".into(),
        recovery_location: Some(moved.path().to_path_buf()),
        measured_free_space_delta: None,
        observed_path_state: Some("trashed".into()),
        recorded_at: now(),
    })?;
    Ok((
        Outcome {
            unit_id: id_for(&path.display().to_string()),
            status: "completed".into(),
            reason: None,
            intended_bytes: unit.bytes,
            observed_free_space_delta: None,
        },
        moved,
    ))
}

/// Removes a linked worktree (directory to Trash, then `git worktree
/// prune` on the common dir read from the worktree just before the move)
/// or a whole checkout (`archive`). Recoverable: move the directory back
/// (and `git worktree repair` for a linked worktree).
fn remove_worktree(
    unit: &MarkedUnit,
    terms: &WorktreeTerms,
    ledger: &Ledger,
    trash_root: &Path,
    actor: &str,
) -> Result<Outcome> {
    let path = &unit.path;
    let common: Option<PathBuf> = if terms.whole_checkout {
        None
    } else {
        swamp_core::git::linked_common_dir(path)
    };
    let verb = if terms.whole_checkout {
        Verb::Archive
    } else {
        Verb::RemoveWorktree
    };
    let recover = match (&terms.remote, &common) {
        (_, Some(c)) => format!(
            "move the directory back, then git -C {} worktree repair",
            c.display()
        ),
        (Some(r), None) => format!("move the directory back, or git clone {r}"),
        (None, None) => "move the directory back from Trash".to_string(),
    };
    let (outcome, _moved) = trash_path(
        unit,
        verb,
        ledger,
        trash_root,
        actor,
        false,
        vec![
            LedgerFact::new("merge_complete", format!("{:?}", terms.merge_complete)),
            LedgerFact::new("pr", format!("{:?}", terms.pr)),
            LedgerFact::new("remote", terms.remote.clone().unwrap_or_default()),
            LedgerFact::new("recover", &recover),
        ],
    )?;
    if let Some(common) = &common {
        let _ = swamp_core::fs_gate::destroy::git_worktree_prune(common);
    }
    Ok(outcome)
}

/// Executes every unit in order. A failure on one unit does not stop the
/// rest -- the footer reports refusals per unit, not as a single aborted
/// batch.
pub fn execute_plan(
    units: &[MarkedUnit],
    ledger: &Ledger,
    trash_root: &Path,
    keep_executables: bool,
) -> Vec<UnitResult> {
    execute_plan_progress(units, ledger, trash_root, keep_executables, |_, _, _| true)
}

/// Returning false stops before the next unit, never during an in-flight move.
pub fn execute_plan_progress(
    units: &[MarkedUnit],
    ledger: &Ledger,
    trash_root: &Path,
    keep_executables: bool,
    mut progress: impl FnMut(usize, &Path, Option<bool>) -> bool,
) -> Vec<UnitResult> {
    let mut results = Vec::new();
    for (i, u) in units.iter().enumerate() {
        if !progress(i, &u.path, None) {
            break;
        }
        let result = execute_one(u, ledger, trash_root, keep_executables);
        let keep_going = progress(i + 1, &u.path, Some(result.outcome.is_ok()));
        results.push(result);
        if !keep_going {
            break;
        }
    }
    results
}

/// The default Trash root: `~/.Trash` on macOS. Overridable via
/// `SWAMP_TRASH_DIR` for tests and CI, which never wants a real
/// `~/.Trash`.
pub fn trash_root() -> PathBuf {
    swamp_core::actions::trash_root()
}

/// Free space on the volume containing `path`, in bytes, via `df -k`.
/// Returns `None` if `df` cannot be read (kept read-only/advisory: a
/// missing measurement never blocks or fakes the reported result).
pub fn free_space_bytes(path: &Path) -> Option<u64> {
    swamp_core::actions::free_space_bytes(path)
}

/// Human summary line for the confirm banner: current facts, shown
/// once, before Enter -- never re-checked afterward.
pub fn confirm_summary(units: &[MarkedUnit], keep_executables: bool) -> String {
    let total: u64 = units.iter().map(|u| u.bytes).sum();
    let what: Vec<String> = units
        .iter()
        .take(3)
        .map(|u| {
            let name = u
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&u.label)
                .to_string();
            if u.warnings.is_empty() {
                name
            } else {
                format!("{name} ⚠ {}", u.warnings.join(" · "))
            }
        })
        .collect();
    let more = if units.len() > 3 {
        format!(" +{} more", units.len() - 3)
    } else {
        String::new()
    };
    let keep = if keep_executables {
        " · keep executables → bin/ (k)"
    } else {
        " · k keep executables"
    };
    // Two destinations, and the difference is the whole point: a path
    // goes to Trash and comes back, a Docker object does not.
    let permanent_units: Vec<&MarkedUnit> = units.iter().filter(|u| u.docker.is_some()).collect();
    let permanent: u64 = permanent_units.iter().map(|u| u.bytes).sum();
    let destination = match (permanent, total - permanent) {
        (0, _) => "→ Trash".to_string(),
        (p, 0) => format!("→ removed permanently, no Trash ({})", human_bytes(p)),
        (p, t) => format!(
            "→ {} to Trash, {} removed permanently (docker, no Trash)",
            human_bytes(t),
            human_bytes(p)
        ),
    };
    // Name every unit that cannot come back, not just its bytes. One
    // project expands into many units, and the three the line has room
    // for are usually ordinary directories -- which left the one
    // irreversible thing in the plan showing as a number and nothing
    // else. The human authorizing this should read what they are
    // destroying by name.
    let no_way_back = if permanent_units.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = permanent_units
            .iter()
            .take(6)
            .map(|u| {
                let what = match &u.docker {
                    Some(swamp_core::docker::Removal::Image { .. }) => "image",
                    Some(swamp_core::docker::Removal::Volume { .. }) => "volume",
                    _ => "object",
                };
                let name = if u.label.trim().is_empty() {
                    u.path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?")
                        .to_string()
                } else {
                    u.label.trim().to_string()
                };
                format!("{name} (docker {what})")
            })
            .collect();
        let extra = permanent_units.len().saturating_sub(6);
        let extra = if extra > 0 {
            format!(" +{extra} more")
        } else {
            String::new()
        };
        format!(" · gone for good: {}{extra}", names.join(", "))
    };
    format!(
        "delete {}{more} ({}) {destination}{no_way_back}?  Enter yes · Esc no{keep}",
        what.join(", "),
        human_bytes(total)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(path: &str, bytes: u64, docker: Option<swamp_core::docker::Removal>) -> MarkedUnit {
        MarkedUnit {
            cargo_unit: None,
            agent_unit: None,
            path: PathBuf::from(path),
            docker,
            worktree_path: PathBuf::from(path),
            bytes,
            observed_at: now(),
            worktree: None,
            label: path.to_string(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn the_confirm_names_what_cannot_come_back() {
        let u = unit(
            "/x",
            10,
            Some(swamp_core::docker::Removal::Image { id: "abc".into() }),
        );
        let summary = confirm_summary(std::slice::from_ref(&u), false);
        assert!(summary.contains("/x"));
    }

    #[test]
    fn a_plan_with_no_docker_says_nothing_about_permanence() {
        let u = unit("/x", 10, None);
        let summary = confirm_summary(std::slice::from_ref(&u), false);
        assert!(!summary.to_lowercase().contains("permanent"));
    }

    #[test]
    fn end_to_end_delete_moves_to_trash_and_appends_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("f"), b"hello").unwrap();
        let trash = tmp.path().join("trash");
        let store = swamp_core::fs_gate::StoreDir::at(tmp.path()).unwrap();
        let ledger = swamp_core::ledger::Ledger::resolved(&store);
        let mut u = unit(target.to_str().unwrap(), 5, None);
        u.worktree_path = tmp.path().to_path_buf();
        let results = execute_plan(std::slice::from_ref(&u), &ledger, &trash, false);
        assert!(results[0].outcome.is_ok(), "{:?}", results[0].outcome);
        assert!(!target.exists());
        let recs = ledger.all().unwrap();
        assert_eq!(recs.len(), 1);
        assert!(recs[0].recovery_location.is_some());
    }

    #[test]
    fn progress_cancellation_stops_between_units_and_keeps_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let trash = tmp.path().join("trash");
        let store = swamp_core::fs_gate::StoreDir::at(tmp.path()).unwrap();
        let ledger = swamp_core::ledger::Ledger::resolved(&store);
        let mut ua = unit(a.to_str().unwrap(), 1, None);
        ua.worktree_path = tmp.path().to_path_buf();
        let mut ub = unit(b.to_str().unwrap(), 1, None);
        ub.worktree_path = tmp.path().to_path_buf();
        let units = vec![ua, ub];
        let mut seen = 0;
        let results = execute_plan_progress(&units, &ledger, &trash, false, |i, _, _| {
            seen += 1;
            i == 0 // let unit 0 start and finish, then stop before unit 1
        });
        assert_eq!(seen, 2);
        assert_eq!(results.len(), 1, "only the first unit ran: {results:?}");
        assert!(!a.exists(), "the first unit was moved");
        assert!(b.exists(), "the second unit was never reached");
    }
}
