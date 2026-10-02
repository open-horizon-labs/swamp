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
    /// Exact session member facts for the expandable review inventory.
    /// Execution still uses the revalidated plan metadata above.
    pub session_members: Option<Vec<swamp_core::agents::AgentMember>>,
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

/// Compact decision summary. The separate detail inventory retains every
/// target/member path; this view groups actions and identical warning facts.
pub fn confirm_summary(units: &[MarkedUnit]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let trash_n = units.iter().filter(|u| u.docker.is_none()).count();
    let trash_bytes: u64 = units
        .iter()
        .filter(|u| u.docker.is_none())
        .map(|u| u.bytes)
        .sum();
    let docker_n = units.iter().filter(|u| u.docker.is_some()).count();
    let docker_bytes: u64 = units
        .iter()
        .filter(|u| u.docker.is_some())
        .map(|u| u.bytes)
        .sum();
    let count = |n: usize| swamp_core::render::human_count(n as u64);
    let mut totals = Vec::new();
    if trash_n > 0 {
        totals.push(format!(
            "Trash: {} · {}",
            count(trash_n),
            human_bytes(trash_bytes)
        ));
    }
    if docker_n > 0 {
        totals.push(format!(
            "Permanent: {} · {}",
            count(docker_n),
            human_bytes(docker_bytes)
        ));
    }
    let unit_count = count(units.len());
    lines.push(format!(
        "Review {unit_count} {} · {}",
        if units.len() == 1 {
            "action"
        } else {
            "actions"
        },
        totals.join(" · ")
    ));

    let trash_paths: Vec<PathBuf> = units
        .iter()
        .filter(|u| u.docker.is_none())
        .map(|u| u.path.clone())
        .collect();
    let trash_scope = common_parent(&trash_paths);
    let mut groups =
        std::collections::BTreeMap::<(String, String), (usize, u64, Vec<String>)>::new();
    for u in units {
        if let Some(removal) = &u.docker {
            let (kind, target) = match removal {
                swamp_core::docker::Removal::Image { id } => ("Docker image", id.as_str()),
                swamp_core::docker::Removal::Volume { name } => ("Docker volume", name.as_str()),
                swamp_core::docker::Removal::Refused(_) => ("Docker object", u.label.as_str()),
            };
            lines.push(format!(
                "PERMANENT · {kind} · {} · {}",
                human_bytes(u.bytes),
                swamp_core::reclaim_trash::plain(target)
            ));
            continue;
        }
        let destination = "Trash";
        let scope = if let Some(plan) = u.cargo_unit.as_ref().or(u.agent_unit.as_ref()) {
            let project = plan.project();
            if project.is_empty() {
                trash_scope.clone()
            } else {
                project.to_string()
            }
        } else {
            trash_scope.clone()
        };
        let entry = groups.entry((destination.to_string(), scope)).or_default();
        entry.0 += 1;
        entry.1 = entry.1.saturating_add(u.bytes);
        entry.2.push(display_target(u));
    }
    for ((destination, scope), (n, bytes, targets)) in groups {
        if n == 1 {
            let unit = units
                .iter()
                .find(|u| u.docker.is_none() && display_target(u) == targets[0]);
            let dest =
                if unit.is_some_and(|u| u.worktree.as_ref().is_some_and(|t| t.whole_checkout)) {
                    "CHECKOUT → Trash"
                } else {
                    "TRASH"
                };
            lines.push(format!("{dest} · {} · {}", human_bytes(bytes), targets[0]));
        } else {
            lines.push(format!(
                "{destination} · {} actions · {} · {scope}",
                count(n),
                human_bytes(bytes)
            ));
        }
    }

    let mut warnings = std::collections::BTreeMap::<String, Vec<String>>::new();
    for u in units {
        let mut unit_warnings = std::collections::BTreeSet::new();
        for warning in &u.warnings {
            // The member inventory is available in the detail view; repeating
            // one warning per member obscures the decision in the summary.
            if crate::names::is_member_warning(warning)
                && (u.session_members.is_some()
                    || u.cargo_unit
                        .as_ref()
                        .is_some_and(|plan| plan.cargo_group().is_some()))
            {
                continue;
            }
            if let Some(fact) = crate::names::decision_warning(warning) {
                unit_warnings.insert(fact);
            }
        }
        if u.worktree.as_ref().is_some_and(|t| t.whole_checkout) {
            unit_warnings.insert("includes the working copy, .git and source".to_string());
        }
        for warning in unit_warnings {
            warnings.entry(warning).or_default().push(display_target(u));
        }
    }
    for (warning, targets) in warnings {
        if targets.len() == 1 {
            if units.len() == 1 {
                lines.push(if crate::names::decision_fact_is_note(&warning) {
                    warning
                } else {
                    format!("! {warning}")
                });
            } else {
                lines.push(format!("! {warning} · {}", targets[0]));
            }
        } else {
            lines.push(format!(
                "! {warning} · affects {} actions",
                count(targets.len())
            ));
        }
    }

    if trash_n > 0 {
        lines.push("Recoverable from Trash until emptied.".into());
    }
    if docker_n > 0 {
        lines.push("Docker has no Trash recovery.".into());
    }
    lines.join("\n")
}

fn display_target(u: &MarkedUnit) -> String {
    if let Some(removal) = &u.docker {
        let target = match removal {
            swamp_core::docker::Removal::Image { id } => id.as_str(),
            swamp_core::docker::Removal::Volume { name } => name.as_str(),
            swamp_core::docker::Removal::Refused(_) => u.label.as_str(),
        };
        swamp_core::reclaim_trash::plain(target)
    } else {
        swamp_core::reclaim_trash::plain(&u.path.display().to_string())
    }
}

fn common_parent(paths: &[PathBuf]) -> String {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    let mut iter = paths.iter();
    if let Some(first) = iter.next() {
        parts = first
            .parent()
            .unwrap_or(first)
            .components()
            .map(|c| c.as_os_str().to_owned())
            .collect();
        for path in iter {
            let next: Vec<_> = path
                .parent()
                .unwrap_or(path)
                .components()
                .map(|c| c.as_os_str().to_owned())
                .collect();
            let shared = parts
                .iter()
                .zip(next.iter())
                .take_while(|(a, b)| a == b)
                .count();
            parts.truncate(shared);
        }
    }
    let mut path = PathBuf::new();
    for part in parts {
        path.push(part);
    }
    if path.as_os_str().is_empty() {
        "selected paths".into()
    } else {
        swamp_core::reclaim_trash::plain(&path.display().to_string())
    }
}

/// Full path inventory is deliberately separate from the compact summary.
pub fn confirm_details(units: &[MarkedUnit]) -> String {
    let count = |n: usize| swamp_core::render::human_count(n as u64);
    let mut lines = vec!["Paths and supporting facts".into()];
    for u in units {
        let action_kind = u.docker.as_ref().map(|removal| match removal {
            swamp_core::docker::Removal::Image { .. } => "Docker image",
            swamp_core::docker::Removal::Volume { .. } => "Docker volume",
            swamp_core::docker::Removal::Refused(_) => "Docker object",
        });
        lines.push(format!(
            "{}{} · {} · {}",
            if u.docker.is_some() {
                "PERMANENT"
            } else {
                "TRASH"
            },
            action_kind
                .map(|kind| format!(" · {kind}"))
                .unwrap_or_default(),
            human_bytes(u.bytes),
            display_target(u)
        ));
        let mut unit_warnings = std::collections::BTreeSet::new();
        for warning in &u.warnings {
            if crate::names::is_member_warning(warning)
                && (u.session_members.is_some()
                    || u.cargo_unit
                        .as_ref()
                        .is_some_and(|plan| plan.cargo_group().is_some()))
            {
                continue;
            }
            unit_warnings.insert(swamp_core::reclaim_trash::plain(warning));
        }
        for warning in unit_warnings {
            lines.push(format!("  ! {warning}"));
        }
        if let Some(group) = u.cargo_unit.as_ref().and_then(|p| p.cargo_group()) {
            lines.push(format!("  {} Cargo members", count(group.members.len())));
            for member in &group.members {
                lines.push(format!(
                    "  member · {} · {}",
                    human_bytes(member.bytes),
                    swamp_core::reclaim_trash::plain(&member.path.display().to_string())
                ));
            }
        }
        if let Some(members) = &u.session_members {
            for member in members {
                lines.push(format!(
                    "  member · {} · {}",
                    human_bytes(member.bytes),
                    swamp_core::reclaim_trash::plain(&member.path.display().to_string())
                ));
            }
        }
    }
    if lines.len() == 1 {
        lines.push(format!("{} action paths", count(units.len())));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(path: &str, bytes: u64, docker: Option<swamp_core::docker::Removal>) -> MarkedUnit {
        MarkedUnit {
            cargo_unit: None,
            agent_unit: None,
            session_members: None,
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
        assert!(
            summary.contains("PERMANENT · Docker image · 10B · abc"),
            "{summary}"
        );
        assert!(
            summary.contains("Docker has no Trash recovery"),
            "{summary}"
        );
    }

    #[test]
    fn a_plan_with_no_docker_says_nothing_about_permanence() {
        let u = unit("/x", 10, None);
        let summary = confirm_summary(std::slice::from_ref(&u));
        assert!(!summary.to_lowercase().contains("permanent"));
        assert!(summary.contains("Review 1 action"));
        assert!(summary.contains("TRASH · 10B · /x"));
        assert!(summary.contains("Recoverable from Trash until emptied"));
    }

    #[test]
    fn compact_reclaim_facts_keep_source_unknown_activity_and_shared_data() {
        let u = MarkedUnit {
            warnings: vec![
                "regeneration cost not established: no record of its download source (from the model row)".into(),
                "last used: no record (file access time unavailable)".into(),
                "who needs it: not established for this path".into(),
                "open-file check unresolved: process state unknown".into(),
                "shared blobs remain in the cache and are not moved with this folder".into(),
            ],
            ..unit("/cache/model", 1024, None)
        };
        let summary = confirm_summary(std::slice::from_ref(&u));
        let details = confirm_details(&[u]);
        assert!(summary.contains("Cost to restore · unknown"), "{summary}");
        assert!(
            details.contains("no record of its download source (from the model row)"),
            "{details}"
        );
        assert!(summary.contains("Last used · no record"), "{summary}");
        assert!(
            !summary.contains("Consumers · not established"),
            "{summary}"
        );
        assert!(
            details.contains("who needs it: not established"),
            "{details}"
        );
        assert!(summary.contains("shared blobs remain"), "{summary}");
    }

    #[test]
    fn model_review_leads_with_use_and_the_layers_left_behind() {
        let u = MarkedUnit {
            warnings: vec![
                "getting it back: downloaded again with `ollama pull qwen3:0.6b` when needed, if the registry has it (a model made with `ollama create` exists only here); size 522.7MB (from the model's own row)".into(),
                "last used: Aug 30 (file access time of its model layer)".into(),
                "declared consumers: none found among 45 projects in 0 declared roots (incomplete)".into(),
                "moving this manifest frees none of its layers: the layers (522.7MB) stay in blobs/; `ollama rm qwen3:0.6b` removes the model and the layers no other model uses".into(),
            ],
            ..unit("/home/me/.ollama/models/manifests/library/qwen3/0.6b", 522_700_000, None)
        };
        let summary = confirm_summary(std::slice::from_ref(&u));
        assert!(summary.lines().count() <= 6, "{summary}");
        assert!(summary.contains("Restore · ollama pull qwen3:0.6b (if available)"));
        assert!(summary.contains("Last used · Aug 30 (file access time)"));
        assert!(summary.contains("Model layers (522.7MB) stay in blobs/."));
        assert!(!summary.contains("declared roots"));
        let details = confirm_details(&[u]);
        assert!(details.contains("ollama create"));
        assert!(details.contains("45 projects"));
        assert!(details.contains("ollama rm qwen3:0.6b"));
    }

    #[test]
    fn known_bookkeeping_is_disclosed_but_unfamiliar_and_destructive_facts_stay_visible() {
        let u = MarkedUnit {
            warnings: vec![
                "internal file history and subgroup hardlink attribution are not retained".into(),
                "swamp's selection rules for this build folder do not cover it: only this path moves, companions are not included".into(),
                "moves only this selected path to Trash; stop its build before removing it; allocation is not guaranteed freed space".into(),
                "cannot be regenerated: unique local data (from sessions)".into(),
                "in use right now: compiler pid 123".into(),
                "swamp keeps this by default (credentials): the tool may sign out".into(),
                "new adapter warning: destroys the only recovery key".into(),
            ],
            ..unit("/work/project/examples", 1000, None)
        };
        let summary = confirm_summary(std::slice::from_ref(&u));
        for fact in [
            "Cannot be downloaded or rebuilt",
            "compiler pid 123",
            "swamp keeps this by default",
            "destroys the only recovery key",
            "Stop builds",
        ] {
            assert!(summary.contains(fact), "missing {fact}: {summary}");
        }
        for bookkeeping in [
            "internal file history",
            "selection rules",
            "companions are not included",
        ] {
            assert!(!summary.contains(bookkeeping), "{summary}");
            assert!(confirm_details(std::slice::from_ref(&u)).contains(bookkeeping));
        }
    }

    #[test]
    fn large_plan_summary_groups_actions_and_repeated_facts_but_details_keep_every_path() {
        let units: Vec<_> = (0..499)
            .map(|n| MarkedUnit {
                warnings: vec!["open-file check unresolved: process state unknown".into()],
                ..unit(&format!("/work/project/cache/item-{n}"), 1024, None)
            })
            .collect();
        let summary = confirm_summary(&units);
        assert!(summary.contains("499 actions"), "{summary}");
        assert!(summary.contains("affects 499 actions"), "{summary}");
        assert_eq!(
            summary.matches("process state unknown").count(),
            1,
            "{summary}"
        );
        assert!(
            !summary.contains("item-0"),
            "the inventory belongs in details"
        );
        let details = confirm_details(&units);
        assert!(details.contains("/work/project/cache/item-0"));
        assert!(details.contains("/work/project/cache/item-498"));
        assert_eq!(
            details
                .matches("TRASH · 1KB · /work/project/cache/item-")
                .count(),
            499
        );
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
                Err(anyhow::anyhow!("the daemon refused")) as Result<Moved<()>>
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
