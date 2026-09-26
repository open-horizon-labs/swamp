//! One tree model for a project: checkouts/worktrees -> artifact rows,
//! shared by the CLI's `--project` drill and the TUI's tree view so the
//! two never drift apart (#33).
//!
//! Folding rule (issue #33 follow-up comment): an artifact row more than
//! two path components deep inside its worktree, or smaller than 1 MB,
//! is not shown on its own line. It folds into its parent directory (the
//! row's first two path components, relative to the worktree root) with
//! a count, so e.g. ESP-IDF's `managed_components/*/test/target` rows
//! don't clutter the drill with dozens of near-empty lines.
//!
//! Paths on every row are relative to the worktree/checkout root, never
//! absolute -- an absolute path is a bug fixed by this module (it used
//! to leak straight from the filesystem walk into `render_project`).

use crate::report::{ArtifactKind, ArtifactRow, ProjectRow, Signal, WorktreeKind};
use std::path::Path;

/// Below this size, and beyond `FOLD_DEPTH` path components deep, an
/// artifact row folds into its parent directory instead of getting its
/// own line.
pub const FOLD_MIN_BYTES: u64 = 1024 * 1024;
/// Path components (relative to the worktree root) beyond which a row
/// folds regardless of size.
pub const FOLD_DEPTH: usize = 2;

/// One renderable row under a worktree: either a single artifact, or a
/// folded group of them sharing a parent directory.
#[derive(Debug, Clone)]
pub struct TreeRow {
    pub kind_label: &'static str,
    /// Set only for an unfolded row (`folded_count == 1`): the original
    /// artifact's kind, so a caller (the TUI) can decide markability or
    /// spot a Docker row without re-deriving it from `kind_label`.
    pub kind: Option<ArtifactKind>,
    /// Relative to the worktree root; `"."` for the worktree root itself.
    pub rel_path: String,
    pub bytes: u64,
    pub growth_bytes: Option<i64>,
    /// > 1 when this row folds several artifacts sharing a parent dir.
    pub folded_count: u32,
    /// Newest file mtime inside the unit (0 unknown); max over a fold.
    pub mtime_max: u64,
    /// Ecosystem tag that generates this artifact, when one does.
    pub ecosystem: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TreeWorktree {
    pub worktree_id: String,
    pub kind: WorktreeKind,
    /// Relative to the project root (the report's `root`, or the first
    /// checkout's parent when no report root is known).
    pub rel_path: String,
    pub bytes: u64,
    pub growth_bytes: Option<i64>,
    pub signals: Vec<Signal>,
    pub rows: Vec<TreeRow>,
}

/// One tool's collapsed contribution to a project's linked agent
/// storage (#100's "project tree shows the collapsed 'Agent storage
/// (linked)' row for every tool"): never a session/transcript-level
/// drill-down here (that stays `report --view agents`/the Agents view)
/// -- just enough to answer "does this project have linked agent
/// storage, from which tools, how much" from the project tree itself.
#[derive(Debug, Clone)]
pub struct ProjectAgentToolRow {
    pub tool_id: String,
    pub tool_name: String,
    pub bytes: u64,
    pub growth_bytes: Option<i64>,
    pub unit_count: usize,
}

#[derive(Debug, Clone)]
pub struct ProjectTree {
    pub name: String,
    pub bytes: u64,
    pub growth_bytes: Option<i64>,
    pub worktrees: Vec<TreeWorktree>,
    /// Agent-storage units whose `project_link` names this project
    /// (`Linked` by name, or `Shared` naming this project's id),
    /// aggregated per tool. Empty when no agent-storage unit is linked
    /// here -- absence is not rendered as a zero row.
    pub agent_rows: Vec<ProjectAgentToolRow>,
}

fn kind_label(kind: &ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::BuildOutput => "build",
        ArtifactKind::DependencyTree => "deps",
        ArtifactKind::Git => "git",
        ArtifactKind::Cache => "cache",
        ArtifactKind::Source => "source",
        ArtifactKind::Ignored => "ignored",
        ArtifactKind::Untracked => "untracked",
        ArtifactKind::DockerImage => "docker-image",
        ArtifactKind::DockerBuildCache => "docker-cache",
        ArtifactKind::DockerVolume => "docker-volume",
        ArtifactKind::Loose => "loose",
        ArtifactKind::Unknown => "unknown",
    }
}

/// Relative path (as a `/`-joined string) of `path` under `root`. Falls
/// back to the path's own display when it is not actually inside `root`
/// (should not happen for a well-formed report, but never worth a panic
/// in a renderer).
fn rel_string(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(p) if p.as_os_str().is_empty() => ".".to_string(),
        Ok(p) => p.to_string_lossy().replace('\\', "/"),
        Err(_) => path.display().to_string(),
    }
}

/// First `n` path components of a `/`-joined relative path string.
fn prefix_components(rel: &str, n: usize) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() <= n {
        rel.to_string()
    } else {
        parts[..n].join("/")
    }
}

fn depth(rel: &str) -> usize {
    if rel == "." {
        0
    } else {
        rel.split('/').count()
    }
}

/// Folds a worktree's artifact rows per the rule above, sorted by growth
/// desc then bytes desc.
fn build_rows(worktree_root: &Path, artifacts: &[ArtifactRow]) -> Vec<TreeRow> {
    let mut kept: Vec<TreeRow> = Vec::new();
    // fold_key -> aggregated row.
    let mut folded: std::collections::BTreeMap<String, TreeRow> = std::collections::BTreeMap::new();

    for a in artifacts {
        let rel = rel_string(&a.path, worktree_root);
        let d = depth(&rel);
        let should_fold = d > FOLD_DEPTH || a.bytes < FOLD_MIN_BYTES;
        // Docker rows are not filesystem paths; never fold them (there is
        // no "parent directory" for an image reference).
        let is_docker = matches!(
            a.kind,
            ArtifactKind::DockerImage | ArtifactKind::DockerBuildCache | ArtifactKind::DockerVolume
        );
        if should_fold && !is_docker && rel != "." {
            let key = prefix_components(&rel, FOLD_DEPTH);
            let entry = folded.entry(key.clone()).or_insert(TreeRow {
                kind_label: "artifacts",
                kind: None,
                rel_path: key,
                bytes: 0,
                growth_bytes: None,
                folded_count: 0,
                mtime_max: 0,
                ecosystem: None,
            });
            entry.bytes += a.bytes;
            entry.mtime_max = entry.mtime_max.max(a.mtime_max);
            entry.folded_count += 1;
            if let Some(g) = a.growth_bytes {
                entry.growth_bytes = Some(entry.growth_bytes.unwrap_or(0) + g);
            }
        } else {
            kept.push(TreeRow {
                kind_label: kind_label(&a.kind),
                kind: Some(a.kind.clone()),
                rel_path: rel,
                bytes: a.bytes,
                growth_bytes: a.growth_bytes,
                folded_count: 1,
                mtime_max: a.mtime_max,
                ecosystem: a.ecosystem.clone(),
            });
        }
    }

    let mut rows: Vec<TreeRow> = kept.into_iter().chain(folded.into_values()).collect();
    rows.sort_by(|a, b| {
        b.growth_bytes
            .unwrap_or(i64::MIN)
            .cmp(&a.growth_bytes.unwrap_or(i64::MIN))
            .then_with(|| b.bytes.cmp(&a.bytes))
    });
    rows
}

/// True when `unit`'s `project_link` names `project` -- either directly
/// (`Linked` by name, matched case-insensitively, the same rule
/// `crate::agents`' own CLI helper uses) or as one of several projects
/// a `Shared` unit's members collectively named (matched by id, since
/// `Shared` only carries ids, never names). Every other link state
/// (`Unresolved`/`Missing`/`NotAProject`/`Moved`/`Remote`/
/// `NotApplicable`) is, by construction, never "this project" -- they
/// are surfaced elsewhere (the Agents view/`--view agents`), never
/// silently folded into a project's row.
fn agent_unit_links_project(unit: &crate::agents::AgentUnit, project: &ProjectRow) -> bool {
    match &unit.project_link {
        crate::agents::ProjectLinkState::Linked { project_name, .. } => {
            project_name.eq_ignore_ascii_case(&project.name)
        }
        crate::agents::ProjectLinkState::Shared { project_ids } => {
            project_ids.iter().any(|id| id == &project.project_id)
        }
        _ => false,
    }
}

/// Aggregates `agent_units` linked to `project`, one row per tool,
/// sorted by bytes desc so the largest contributor is named first. See
/// [`ProjectAgentToolRow`] for what "collapsed" means here.
pub fn agent_rows_for_project(
    project: &ProjectRow,
    agent_units: &[crate::agents::AgentUnit],
) -> Vec<ProjectAgentToolRow> {
    let mut by_tool: std::collections::BTreeMap<String, ProjectAgentToolRow> =
        std::collections::BTreeMap::new();
    for u in agent_units {
        if !agent_unit_links_project(u, project) {
            continue;
        }
        let entry = by_tool
            .entry(u.tool_id.clone())
            .or_insert_with(|| ProjectAgentToolRow {
                tool_id: u.tool_id.clone(),
                tool_name: u.tool_name.clone(),
                bytes: 0,
                growth_bytes: None,
                unit_count: 0,
            });
        entry.bytes += u.bytes;
        entry.unit_count += 1;
        if let Some(g) = u.growth_bytes {
            entry.growth_bytes = Some(entry.growth_bytes.unwrap_or(0) + g);
        }
    }
    let mut rows: Vec<ProjectAgentToolRow> = by_tool.into_values().collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    rows
}

/// Builds the tree for one project. `project_root` is the path every
/// worktree's own path is made relative to (a checkout's own path
/// relative to itself is `.`; a linked worktree is typically a sibling
/// or a `.worktrees/<name>` child, so it is left as whatever path it
/// actually has relative to `project_root` -- there is no requirement
/// that every worktree live under a single common root, only that the
/// display stays relative rather than absolute). `agent_units` is the
/// full agent-storage catalog (any tool, any project); this function
/// filters it down to this project's own collapsed summary row(s)
/// (#100) -- pass an empty slice when agent-storage is not being
/// computed for this call (e.g. a caller that never touches the Agents
/// view), which yields the same tree as before agent linkage existed.
pub fn build_project_tree(
    project: &ProjectRow,
    project_root: &Path,
    agent_units: &[crate::agents::AgentUnit],
) -> ProjectTree {
    let mut worktrees: Vec<TreeWorktree> = Vec::new();
    let mut total_bytes = 0u64;
    let mut total_growth: Option<i64> = None;
    let mut have_growth = false;

    for wt in &project.worktrees {
        let mut bytes = 0u64;
        let mut growth: Option<i64> = None;
        let mut wt_have_growth = false;
        for a in &wt.artifacts {
            bytes += a.bytes;
            if let Some(g) = a.growth_bytes {
                wt_have_growth = true;
                growth = Some(growth.unwrap_or(0) + g);
            }
        }
        total_bytes += bytes;
        if wt_have_growth {
            have_growth = true;
            total_growth = Some(total_growth.unwrap_or(0) + growth.unwrap_or(0));
        }
        let rel_path = rel_string(&wt.path, project_root);
        worktrees.push(TreeWorktree {
            worktree_id: wt.worktree_id.clone(),
            kind: wt.kind.clone(),
            rel_path,
            bytes,
            growth_bytes: if wt_have_growth { growth } else { None },
            signals: wt.signals.clone(),
            rows: build_rows(&wt.path, &wt.artifacts),
        });
    }

    // Main/Clone first (checkouts), then Linked, each group sorted by
    // growth desc then bytes desc -- same order the issue's example
    // shows (main checkout first, worktrees after).
    worktrees.sort_by(|a, b| {
        let rank = |k: &WorktreeKind| match k {
            WorktreeKind::Main => 0,
            WorktreeKind::Clone => 1,
            WorktreeKind::Linked => 2,
        };
        rank(&a.kind).cmp(&rank(&b.kind)).then_with(|| {
            b.growth_bytes
                .unwrap_or(i64::MIN)
                .cmp(&a.growth_bytes.unwrap_or(i64::MIN))
                .then_with(|| b.bytes.cmp(&a.bytes))
        })
    });

    ProjectTree {
        name: project.name.clone(),
        bytes: total_bytes,
        growth_bytes: if have_growth { total_growth } else { None },
        worktrees,
        agent_rows: agent_rows_for_project(project, agent_units),
    }
}
