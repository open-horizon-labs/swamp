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
    /// Set when the row is a Reclaim/External unit or folder: the review
    /// it was marked on, rechecked at the move. Plain path Trash move.
    pub reclaim: Option<ReclaimMark>,
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

/// What a Reclaim mark carries to the move: the reviewed identity of the
/// entry, its category and the store whose protect marks are rechecked.
#[derive(Debug, Clone)]
pub struct ReclaimMark {
    pub reviewed: swamp_core::reclaim_trash::Reviewed,
    pub category: String,
    pub store: Option<PathBuf>,
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

/// One ledger-recorded move: a `started` row BEFORE anything moves (a
/// ledger that cannot take it means nothing moved), the move, then the
/// final row in its place. A move that fails leaves a `failed:` row. When
/// the move happened but the final row cannot be written (the ledger stayed
/// locked), the error says so plainly: the item is gone and only the
/// `started` row records what was about to happen.
struct Moved<T> {
    value: T,
    recovery: Option<PathBuf>,
    state: String,
    facts: Vec<LedgerFact>,
}

fn run_recorded<T>(
    unit: &MarkedUnit,
    ledger: &Ledger,
    verb: Verb,
    actor: &str,
    facts: Vec<LedgerFact>,
    happened: &str,
    mv: impl FnOnce() -> Result<Moved<T>>,
) -> Result<T> {
    let id = swamp_core::entities::new_id();
    let entity = id_for(&unit.path.display().to_string());
    let row = |outcome: String, recovery: Option<PathBuf>, state: &str, facts: Vec<LedgerFact>| {
        ActionRecord {
            id: id.clone(),
            verb: verb.clone(),
            entity_id: entity.clone(),
            evidence: ledger_evidence(unit, facts),
            grant_id: NO_GRANT.to_string(),
            actor: actor.to_string(),
            outcome,
            recovery_location: recovery,
            measured_free_space_delta: None,
            observed_path_state: Some(state.to_string()),
            recorded_at: now(),
        }
    };
    if let Err(e) = ledger.append(&row("started".into(), None, "about to move", facts.clone()))
        && !swamp_core::ledger::wrote_into_new_ledger(&e)
    {
        anyhow::bail!("swamp could not write its ledger ({e:#}), so nothing was moved or removed");
    }
    match mv() {
        Err(e) => {
            let _ = ledger.replace(&row(format!("failed:{e:#}"), None, "not moved", facts));
            Err(e)
        }
        Ok(m) => {
            let mut all = facts;
            all.extend(m.facts);
            ledger
                .replace(&row("completed".into(), m.recovery, &m.state, all))
                .map_err(|e| {
                    // The ledger's own text ends "so nothing was written",
                    // true of the row and false of the move: cut it.
                    let why = format!("{e:#}").replace(", so nothing was written", "");
                    anyhow::anyhow!(
                        "{happened}, but the record could not be written ({why}); the `started` row in the ledger is all that records it"
                    )
                })?;
            Ok(m.value)
        }
    }
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
            run_recorded(
                unit,
                ledger,
                Verb::Delete,
                actor,
                vec![LedgerFact::new("cargo_group", format!("{group:?}"))],
                "the build output was moved to Trash",
                || {
                    let dest = swamp_core::actions::trash_cargo_group(group, trash_root)?;
                    Ok(Moved {
                        value: (),
                        recovery: Some(dest),
                        state: "trashed".into(),
                        facts: Vec::new(),
                    })
                },
            )?;
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
            run_recorded(
                unit,
                ledger,
                Verb::Delete,
                actor,
                vec![
                    LedgerFact::new("tool_id", &meta.tool_id),
                    LedgerFact::new("category", format!("{:?}", meta.category)),
                    LedgerFact::new("session_removal", meta.session_members.is_some()),
                ],
                "the agent storage was moved to Trash",
                || {
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
                    Ok(Moved {
                        value: (),
                        recovery: Some(dest),
                        state: "trashed".into(),
                        facts: Vec::new(),
                    })
                },
            )?;
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
    let outcome = if let Some(mark) = &unit.reclaim {
        trash_reclaim(unit, mark, ledger, trash_root)
    } else if let Some(terms) = &unit.worktree {
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

/// Moves one marked Reclaim/External row to the Trash: the core's
/// recheck (same entry, same place, protect marks), a `started` ledger
/// row before the move, the final row after. Nothing moves when the
/// entry changed since review or the ledger cannot take the first row.
fn trash_reclaim(
    unit: &MarkedUnit,
    mark: &ReclaimMark,
    ledger: &Ledger,
    trash_root: &Path,
) -> Result<Outcome> {
    let facts = swamp_core::actions::ReclaimMoveFacts {
        label: unit.label.clone(),
        bytes: unit.bytes,
        observed_at: unit.observed_at,
        warnings: unit.warnings.clone(),
        category: mark.category.clone(),
    };
    swamp_core::actions::trash_reclaim(
        &mark.reviewed,
        &facts,
        mark.store.as_deref(),
        ledger,
        trash_root,
    )
    .map_err(|e| anyhow::anyhow!(e))?;
    Ok(Outcome {
        unit_id: id_for(&unit.path.display().to_string()),
        status: "completed".into(),
        reason: None,
        intended_bytes: unit.bytes,
        observed_free_space_delta: None,
    })
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
    // Permanent: the `started` row is what survives if the final row
    // cannot be written, so it goes first.
    run_recorded(
        unit,
        ledger,
        Verb::Delete,
        actor,
        vec![
            LedgerFact::new("docker", format!("{target:?}")),
            LedgerFact::new("permanent", true),
        ],
        "the Docker object was removed for good",
        || {
            swamp_core::docker::remove(target).map_err(|e| anyhow::anyhow!(e))?;
            Ok(Moved {
                value: (),
                // The daemon has no Trash: there is nowhere to point at.
                recovery: None,
                state: "removed via docker".into(),
                facts: Vec::new(),
            })
        },
    )?;
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
    let moved = run_recorded(
        unit,
        ledger,
        verb,
        actor,
        extra,
        "the folder was moved to Trash",
        || {
            let mut facts = Vec::new();
            if keep_executables {
                let bin = unit.worktree_path.join("bin");
                let kept = swamp_core::actions::preserve_executables(path, &bin)
                    .map_err(|e| anyhow::anyhow!("could not preserve executables: {e}"))?;
                facts.push(LedgerFact::new(
                    "preserved",
                    kept.iter()
                        .map(|k| k.to.display().to_string())
                        .collect::<Vec<_>>()
                        .join("; "),
                ));
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("item");
            let moved = swamp_core::fs_gate::destroy::trash_move(
                path,
                trash_root,
                &format!("{name}-{}", now()),
            )?;
            debug_assert_eq!(
                moved.anchor(),
                path,
                "the receipt names what was actually moved"
            );
            Ok(Moved {
                recovery: Some(moved.path().to_path_buf()),
                value: moved,
                state: "trashed".into(),
                facts,
            })
        },
    )?;
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

/// Enter on the tool-managed removal confirm (#177): the one caller of
/// `tool_removal::execute`, which re-reviews, refuses anything that
/// changed, runs exactly `preview`'s command and records a `tool-remove`
/// ledger line. Runs on a worker, never on the event thread.
pub fn run_tool_removal(
    host: &swamp_core::tool_removal::Host,
    preview: &swamp_core::tool_removal::Preview,
    sizes: &[(PathBuf, u64)],
    store: &swamp_core::fs_gate::StoreDir,
) -> swamp_core::tool_removal::Outcome {
    let ledger = host.ledger_in(store);
    swamp_core::tool_removal::execute(host, preview, sizes, &ledger)
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

/// Human summary for the confirm banner: current facts, shown once,
/// before Enter -- never re-checked afterward.
///
/// One line per fact, in the order a person authorizing a delete needs
/// them: count, size and destination first, then what cannot come back,
/// then names, then warnings. The lines are separate so the screen can
/// wrap or cut at the tail; the numbers are never behind the names.
pub fn confirm_summary(units: &[MarkedUnit]) -> String {
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let reclaim_units: Vec<&MarkedUnit> = units.iter().filter(|u| u.reclaim.is_some()).collect();
    let units: Vec<MarkedUnit> = units
        .iter()
        .filter(|u| u.reclaim.is_none())
        .cloned()
        .collect();
    let units_all_n = units.len() + reclaim_units.len();
    let units = &units[..];
    let permanent_units: Vec<&MarkedUnit> = units.iter().filter(|u| u.docker.is_some()).collect();
    let trash_units = units_all_n - permanent_units.len();
    let permanent: u64 = permanent_units.iter().map(|u| u.bytes).sum();
    let trash_bytes: u64 = units
        .iter()
        .chain(reclaim_units.iter().copied())
        .map(|u| u.bytes)
        .sum::<u64>()
        - permanent;
    let mut lines: Vec<String> = Vec::new();
    let frees_nothing = reclaim_units
        .iter()
        .any(|u| u.warnings.iter().any(|w| w.contains("frees about nothing")));
    let inside_extra = if frees_nothing {
        ", mounted volumes inside hold most of these bytes: moving it frees about nothing"
    } else if reclaim_units.iter().any(|u| {
        u.warnings
            .iter()
            .any(|w| w.contains("counted under projects"))
    }) {
        " plus project worktrees inside, counted under their projects"
    } else {
        ""
    };
    // Two destinations, and the difference is the whole point: a path
    // goes to Trash and comes back, a Docker object does not.
    if trash_units > 0 {
        lines.push(format!(
            "Move {} ({}{inside_extra}) → Trash. Space is freed when Trash is emptied.",
            plural(trash_units, "item", "items"),
            human_bytes(trash_bytes)
        ));
    }
    if !permanent_units.is_empty() {
        lines.push(format!(
            "Remove {} ({}) for good, no Trash.",
            plural(permanent_units.len(), "docker item", "docker items"),
            human_bytes(permanent)
        ));
        // Name every unit that cannot come back, not just its bytes.
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
        lines.push(format!("Gone for good: {}{extra}", names.join(", ")));
    }
    // What kinds of things move, each name once with how many: never the
    // same name repeated. Checkouts are spelled out as such.
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for u in units.iter().filter(|u| u.docker.is_none()) {
        let name = u
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&u.label)
            .to_string();
        let name = match &u.worktree {
            Some(t) if t.whole_checkout => format!("checkout {name}"),
            Some(_) => format!("worktree {name}"),
            None => name,
        };
        match kinds.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += 1,
            None => kinds.push((name, 1)),
        }
    }
    // Most common first; ties keep the order they were found in.
    kinds.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    if !kinds.is_empty() {
        let shown: Vec<String> = kinds
            .iter()
            .take(3)
            .map(|(n, c)| {
                if *c > 1 {
                    format!("{n} ({c})")
                } else {
                    n.clone()
                }
            })
            .collect();
        let rest = kinds.len().saturating_sub(3);
        let rest = if rest > 0 {
            format!(", +{rest} kinds")
        } else {
            String::new()
        };
        lines.push(format!("Includes: {}{rest}", shown.join(", ")));
    }
    if units
        .iter()
        .any(|u| u.worktree.as_ref().is_some_and(|t| t.whole_checkout))
    {
        lines.push("A checkout takes its working copy, .git and source (into Trash).".to_string());
    }
    // Warnings, one line per distinct warning with how many items carry
    // it, never inlined into the names.
    let mut warned: Vec<(String, usize)> = Vec::new();
    for w in units.iter().flat_map(|u| u.warnings.iter()) {
        match warned.iter_mut().find(|(t, _)| t == w) {
            Some((_, n)) => *n += 1,
            None => warned.push((w.clone(), 1)),
        }
    }
    // Every distinct warning is listed: a warning folded into "+N more"
    // while Enter is offered is a fact the person did not read. A plan
    // whose lines do not all fit the sheet offers no Enter (`confirm_fits`).
    for (text, n) in &warned {
        let text = swamp_core::reclaim_trash::plain(text);
        if *n > 1 {
            lines.push(format!("⚠ {text} ({n} items)"));
        } else {
            lines.push(format!("⚠ {text}"));
        }
    }
    lines.extend(reclaim_lines(&reclaim_units));
    lines.join("\n")
}

/// Most marked folders whose own warnings are listed one by one; more
/// than this and the paths are listed and the warnings are merged.
const RECLAIM_BLOCKS: usize = 3;
/// Most paths listed when the warnings are merged.
const RECLAIM_PATHS: usize = 8;

/// The confirm's part for Reclaim/External folders: every exact path and
/// size, then what swamp does and does not know about each (its own
/// review lines), then the way back. Nothing is cut to "N more" until the
/// list is long, and then the count says how many paths are not shown.
fn reclaim_lines(units: &[&MarkedUnit]) -> Vec<String> {
    use swamp_core::reclaim_trash::plain;
    let mut lines: Vec<String> = Vec::new();
    if units.is_empty() {
        return lines;
    }
    let shown = |u: &MarkedUnit| plain(&u.path.display().to_string());
    if units.len() <= RECLAIM_BLOCKS {
        for u in units {
            lines.push(format!("{} ({})", shown(u), human_bytes(u.bytes)));
            for w in &u.warnings {
                lines.push(format!("⚠ {}", plain(w)));
            }
        }
    } else {
        for u in units.iter().take(RECLAIM_PATHS) {
            lines.push(format!("{} ({})", shown(u), human_bytes(u.bytes)));
        }
        if units.len() > RECLAIM_PATHS {
            let rest: u64 = units.iter().skip(RECLAIM_PATHS).map(|u| u.bytes).sum();
            lines.push(format!(
                "+{} more folders ({}); every path is in the ledger row for its move",
                units.len() - RECLAIM_PATHS,
                human_bytes(rest)
            ));
        }
        // Each distinct warning once, and the folders it is about whenever
        // it is not about all of them: a warning is never shown without
        // the path it belongs to.
        let mut merged: Vec<(String, Vec<String>)> = Vec::new();
        for u in units {
            for w in &u.warnings {
                match merged.iter_mut().find(|(t, _)| t == w) {
                    Some((_, who)) => who.push(shown(u)),
                    None => merged.push((w.clone(), vec![shown(u)])),
                }
            }
        }
        for (text, who) in &merged {
            let text = plain(text);
            if who.len() == units.len() {
                lines.push(format!("⚠ {text} (all {} folders)", units.len()));
            } else {
                // Every folder the warning is about, by name: a count
                // would hide which ones. A plan too long for the sheet
                // offers no Enter (`confirm_fits`).
                lines.push(format!("⚠ {text}: {}", who.join(", ")));
            }
        }
    }
    lines.push(
        "Facts above were read when you marked; Enter re-checks that each entry, and what holds it open, is unchanged."
            .to_string(),
    );
    lines.push(
        "Trash is the way back: move the folder out of the Trash to restore it. Space is freed when Trash is emptied."
            .to_string(),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(path: &str, bytes: u64, docker: Option<swamp_core::docker::Removal>) -> MarkedUnit {
        MarkedUnit {
            cargo_unit: None,
            agent_unit: None,
            reclaim: None,
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
        let summary = confirm_summary(std::slice::from_ref(&u));
        assert!(summary.contains("/x"));
    }

    #[test]
    fn a_plan_with_no_docker_says_nothing_about_permanence() {
        let u = unit("/x", 10, None);
        let summary = confirm_summary(std::slice::from_ref(&u));
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

    fn locked_ledger_fixture() -> (tempfile::TempDir, swamp_core::ledger::Ledger, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let store = swamp_core::fs_gate::StoreDir::at(tmp.path()).unwrap();
        let ledger = swamp_core::ledger::Ledger::resolved(&store);
        let path = ledger.path();
        (tmp, ledger, path)
    }

    /// Tempting wrong patch: the row is written AFTER the move, so a ledger
    /// that stays locked leaves the folder gone with no record, and the
    /// message reads as if nothing happened. The started row is written
    /// first: with the ledger locked nothing moves, and the message says
    /// nothing was moved.
    #[test]
    fn a_locked_ledger_stops_the_delete_before_anything_moves() {
        swamp_core::fs_gate::store::set_ledger_lock_wait_ms(300);
        let (tmp, ledger, lpath) = locked_ledger_fixture();
        let target = tmp.path().join("cache");
        std::fs::create_dir_all(&target).unwrap();
        let u = unit(target.to_str().unwrap(), 5, None);
        let held = swamp_core::fs_gate::StoreDir::lock_ledger_writes(&lpath).unwrap();
        let res = execute_plan(
            std::slice::from_ref(&u),
            &ledger,
            &tmp.path().join("trash"),
            false,
        );
        drop(held);
        swamp_core::fs_gate::store::set_ledger_lock_wait_ms(0);
        let err = res[0].outcome.as_ref().unwrap_err();
        assert!(err.contains("nothing was moved or removed"), "{err}");
        assert!(target.exists(), "the folder did not move");
    }

    /// Tempting wrong patch: a move that happened but whose final row
    /// cannot be written says "nothing was written". It says the item was
    /// moved, names the started row, and the ledger keeps that started row.
    #[test]
    fn a_final_row_that_cannot_be_written_says_the_item_was_moved() {
        swamp_core::fs_gate::store::set_ledger_lock_wait_ms(300);
        let (tmp, ledger, lpath) = locked_ledger_fixture();
        let target = tmp.path().join("cache");
        std::fs::create_dir_all(&target).unwrap();
        let u = unit(target.to_str().unwrap(), 5, None);
        let mut other_process_holds = None;
        let res = run_recorded(
            &u,
            &ledger,
            Verb::Delete,
            "human:tui",
            Vec::new(),
            "the folder was moved to Trash",
            || {
                // The move happens, then another swamp takes the ledger.
                std::fs::remove_dir_all(&target).unwrap();
                other_process_holds =
                    Some(swamp_core::fs_gate::StoreDir::lock_ledger_writes(&lpath).unwrap());
                Ok(Moved {
                    value: (),
                    recovery: None,
                    state: "trashed".into(),
                    facts: Vec::new(),
                })
            },
        );
        drop(other_process_holds);
        swamp_core::fs_gate::store::set_ledger_lock_wait_ms(0);
        let err = res.unwrap_err().to_string();
        assert!(
            err.contains("was moved to Trash, but the record could not be written"),
            "{err}"
        );
        assert!(!err.contains("nothing was written"), "{err}");
        let rows = ledger.all().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, "started");
    }

    /// Tempting wrong patch: a Docker removal (permanent) writes its row
    /// only afterwards. The started row exists before the daemon is asked.
    #[test]
    fn the_started_row_exists_before_the_move_runs() {
        let (tmp, ledger, _l) = locked_ledger_fixture();
        let u = unit(tmp.path().join("x").to_str().unwrap(), 1, None);
        let seen = std::cell::Cell::new(false);
        let res: Result<()> = run_recorded(
            &u,
            &ledger,
            Verb::Delete,
            "human:tui",
            Vec::new(),
            "removed",
            || {
                let rows = ledger.all().unwrap();
                seen.set(rows.len() == 1 && rows[0].outcome == "started");
                return Err(anyhow::anyhow!("the daemon refused")) as Result<Moved<()>>;
            },
        );
        assert!(res.is_err() && seen.get(), "the row was not there first");
        let rows = ledger.all().unwrap();
        assert!(
            rows[0].outcome.starts_with("failed:the daemon refused"),
            "{}",
            rows[0].outcome
        );
    }
}
