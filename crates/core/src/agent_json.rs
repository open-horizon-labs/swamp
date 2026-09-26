//! JSON view-shaping shared by every noninteractive (agent-facing) CLI
//! surface: `report --json`/`--view ... --json`, `propose --json`,
//! `grant list --json`. This used to live one-off inside
//! `crates/mcp/src/main.rs`'s per-tool handlers; it is extracted here so
//! the CLI -- now the sole supported machine interface -- reuses the
//! exact same view logic instead of re-deriving it. See
//! `skills/swamp/references/commands-and-json.md` for the documented
//! contract these functions back.
//!
//! Facts, not verdicts (`agent_interface_facts_not_verdicts` audits this
//! module together with `render.rs`): nothing here renders "safe",
//! "stale" or "can be deleted".

use crate::entities::now;
use crate::filter::{Filter, Predicate, WorktreeFacts};
use crate::github::{GithubFacts, MergeComplete, PrStatus, TriState};
use crate::growth::{DEFAULT_SINCE, history_span_for_root, load_config, parse_duration_secs};
use crate::render::{kind_label, reason_label};
use crate::report::{ArtifactKind, ArtifactRow, Report, UnownedReason};
use serde_json::{Value, json};
use std::path::Path;

/// History the store holds for `root`'s volume, and the growth window a
/// call can honestly honor: the asked window, or the history if shorter.
pub fn history_block(store_dir: &Path, root: &Path, since: Option<&str>) -> Value {
    let n = now();
    let history = history_span_for_root(store_dir, root, n);
    let asked = since.and_then(parse_duration_secs);
    let effective = match (asked, history) {
        (Some(a), Some(h)) => Some(a.min(h)),
        (Some(a), None) => Some(a),
        (None, h) => h,
    };
    json!({
        "history_secs": history,
        "asked_window_secs": asked,
        "effective_window_secs": effective,
        "note": match (asked, history) {
            (Some(a), Some(h)) if a > h => Some(format!(
                "asked for {}s of growth but the store holds {}s of observations; growth is reported over {}s",
                a, h, h
            )),
            (_, None) => Some("no observations yet: growth cannot be reported".to_string()),
            _ => None,
        },
    })
}

/// The `since` window a call actually used: the caller's explicit value
/// if it parses, else the store's configured default, else the
/// hard-coded default -- the same resolution order `report_with` applies
/// internally. Every bounded JSON result echoes this back (never just
/// the raw, possibly-absent argument) so a caller can see what window
/// its numbers actually reflect.
pub fn effective_since(store_dir: &Path, since: Option<&str>) -> String {
    if let Some(s) = since
        && parse_duration_secs(s).is_some()
    {
        return s.to_string();
    }
    let config = load_config(store_dir);
    if parse_duration_secs(&config.since).is_some() {
        return config.since;
    }
    DEFAULT_SINCE.to_string()
}

/// Narrows a report in place to what a filter admits: a project stays
/// when it passes the project-level predicates (`type:`, `project:`,
/// `size` on its total) and at least one artifact passes the artifact
/// predicates (`kind:`, `growth`, `size`, `age`); artifacts that fail are
/// dropped from the kept projects. Worktree predicates (`idle`,
/// `merge-complete`, `pr:`) keep a project when any worktree passes.
pub fn apply_filter_to_report(r: &mut Report, f: &Filter) {
    let has_artifact_preds = f.predicates.iter().any(|p| {
        matches!(
            p,
            Predicate::Kind(_)
                | Predicate::Growth { .. }
                | Predicate::Size { .. }
                | Predicate::AgeGreaterThan(_)
        )
    });
    let has_wt_preds = f.predicates.iter().any(|p| {
        matches!(
            p,
            Predicate::IdleGreaterThan(_) | Predicate::MergeComplete | Predicate::Pr(_)
        )
    });
    r.projects.retain_mut(|p| {
        let project_level = f.predicates.iter().all(|pred| match pred {
            Predicate::Project(name) => crate::filter::name_matches(name, &p.name),
            Predicate::Type(_) => {
                let probe = p.clone();
                Filter {
                    predicates: vec![pred.clone()],
                }
                .matches_artifact(&probe, &dummy_artifact())
            }
            _ => true,
        });
        if !project_level {
            return false;
        }
        if has_wt_preds {
            let any = p.worktrees.iter().any(|wt| {
                let merge_complete = wt
                    .merge_complete
                    .as_ref()
                    .is_some_and(|m| m.verdict == TriState::Yes);
                let unknown = PrStatus::Unknown;
                let facts = WorktreeFacts {
                    merge_complete,
                    idle_secs: wt.idle_secs,
                    pr: wt
                        .github
                        .as_ref()
                        .map(|g| &g.pull_request)
                        .unwrap_or(&unknown),
                };
                Filter {
                    predicates: f
                        .predicates
                        .iter()
                        .filter(|q| {
                            matches!(
                                q,
                                Predicate::IdleGreaterThan(_)
                                    | Predicate::MergeComplete
                                    | Predicate::Pr(_)
                            )
                        })
                        .cloned()
                        .collect(),
                }
                .matches_worktree(p, wt, &facts)
            });
            if !any {
                return false;
            }
        }
        if has_artifact_preds {
            let snapshot = p.clone();
            let mut any = false;
            for wt in &mut p.worktrees {
                wt.artifacts.retain(|a| {
                    let keep = Filter {
                        predicates: f
                            .predicates
                            .iter()
                            .filter(|q| {
                                matches!(
                                    q,
                                    Predicate::Kind(_)
                                        | Predicate::Growth { .. }
                                        | Predicate::Size { .. }
                                        | Predicate::AgeGreaterThan(_)
                                )
                            })
                            .cloned()
                            .collect(),
                    }
                    .matches_artifact(&snapshot, a);
                    any |= keep;
                    keep
                });
            }
            return any;
        }
        true
    });
}

fn dummy_artifact() -> ArtifactRow {
    ArtifactRow {
        kind: ArtifactKind::Source,
        path: Default::default(),
        bytes: 0,
        mtime_max: 0,
        ecosystem: None,
        hardlinked: false,
        dedup_stale: false,
        allocated_bytes: None,
        allocated_growth_bytes: None,
        local_bytes: 0,
        track: None,
        growth_bytes: None,
        regrowth_count: 0,
        observed_at: 0,
        confidence: crate::entities::Confidence::High,
        source: crate::report::Source::new("filter"),
        note: None,
        created_at: None,
        containers: Vec::new(),
        shared_with: Vec::new(),
        dangling: false,
        evidence: Vec::new(),
    }
}

/// Scopes `r`'s `projects` (and `dirs_by_worktree`) to one project by
/// name (or its `owner/repo` display name).
pub fn scope_to_project(r: &mut Report, name: &str) {
    r.projects
        .retain(|p| p.name == name || crate::render::project_display_name(p) == name);
    if let Some(dirs) = &mut r.dirs_by_worktree {
        let keep: std::collections::HashSet<String> = r
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter().map(|w| w.worktree_id.clone()))
            .collect();
        dirs.retain(|k, _| keep.contains(k));
    }
}

/// Builds the JSON payload for one named `view` over a report, scoped to
/// `only_project` when set: worktrees (default), builds, deps, docker,
/// kinds, types, unowned, reconciliation. The same view set `--view`
/// exposes for text rendering, serialized instead of formatted.
pub fn view_payload(r: &Report, view: &str, only_project: Option<&str>) -> Value {
    match view {
        "types" => serde_json::to_value(&r.summary.by_type).unwrap_or(Value::Null),
        "kinds" => {
            // #60: no single row backs a kind bucket, so `evidence` here
            // is every underlying row's own evidence concatenated --
            // still the same per-fact shape a contract test can check
            // for, rather than a silently missing field on the one JSON
            // view with no 1:1 unit.
            let mut agg: std::collections::BTreeMap<
                &'static str,
                (u64, u64, Vec<crate::evidence::Evidence>),
            > = std::collections::BTreeMap::new();
            for p in &r.projects {
                if only_project.is_some_and(|name| name != p.name) {
                    continue;
                }
                for wt in &p.worktrees {
                    for a in &wt.artifacts {
                        let entry = agg.entry(kind_label(&a.kind)).or_insert((0, 0, Vec::new()));
                        entry.0 += a.bytes;
                        entry.1 += 1;
                        entry.2.extend(a.evidence.iter().cloned());
                    }
                }
            }
            json!(
                agg.into_iter()
                    .map(|(kind, (bytes, count, evidence))| json!({
                        "kind": kind,
                        "bytes": bytes,
                        "count": count,
                        "evidence": evidence,
                    }))
                    .collect::<Vec<_>>()
            )
        }
        "builds" | "deps" => {
            let kinds: &[ArtifactKind] = if view == "builds" {
                &[ArtifactKind::BuildOutput, ArtifactKind::Cache]
            } else {
                &[ArtifactKind::DependencyTree]
            };
            let mut rows: Vec<Value> = Vec::new();
            for p in &r.projects {
                if only_project.is_some_and(|name| name != p.name) {
                    continue;
                }
                for wt in &p.worktrees {
                    for a in &wt.artifacts {
                        if !kinds.contains(&a.kind) {
                            continue;
                        }
                        let mut row = json!({
                            "project": p.name,
                            "kind": kind_label(&a.kind),
                            "path": a.path,
                            "bytes": a.bytes,
                            "growth_bytes": a.growth_bytes,
                            "evidence": a.evidence,
                        });
                        if let Some(interior) = build_interior_json(r, &a.path) {
                            row["interior"] = interior;
                        }
                        rows.push(row);
                    }
                }
            }
            rows.sort_by(|a, b| {
                b["bytes"]
                    .as_u64()
                    .unwrap_or(0)
                    .cmp(&a["bytes"].as_u64().unwrap_or(0))
            });
            json!(rows)
        }
        "docker" => docker_objects_payload(r, false, only_project),
        "unowned" => {
            let rows: Vec<Value> = r
                .unowned
                .iter()
                .map(|row| {
                    json!({
                        "path_or_object": row.path_or_object,
                        "bytes": row.bytes,
                        "reason": reason_label(&row.reason),
                        "shared_bytes": row.shared_bytes,
                        "docker_kind": row.docker_kind,
                        "note": row.note,
                        "evidence": row.evidence,
                    })
                })
                .collect();
            json!(rows)
        }
        "reconciliation" => json!(r.reconciliation),
        "projects" => list_projects_payload(r, only_project),
        "worktrees" => list_worktrees_payload(r, &Filter::default(), only_project),
        _ => json!({
            "projects": r.projects.iter().filter(|p| only_project.is_none_or(|name| name == p.name)).collect::<Vec<_>>(),
        }),
    }
}

/// Every Docker object joined to a project (with its project name
/// attached) plus every unowned one, sorted by bytes desc.
/// `unowned_only` restricts to the unowned half; `only_project`
/// restricts joined rows to that project and unowned rows to
/// name-alike candidates, explicitly labelled and never attributed.
pub fn docker_objects_payload(r: &Report, unowned_only: bool, only_project: Option<&str>) -> Value {
    let mut rows: Vec<Value> = Vec::new();
    if !unowned_only {
        for p in &r.projects {
            if only_project.is_some_and(|name| name != p.name) {
                continue;
            }
            for wt in &p.worktrees {
                for a in &wt.artifacts {
                    if !matches!(
                        a.kind,
                        ArtifactKind::DockerImage
                            | ArtifactKind::DockerBuildCache
                            | ArtifactKind::DockerVolume
                    ) {
                        continue;
                    }
                    rows.push(json!({
                        "project": p.name,
                        "object": a.path,
                        "kind": kind_label(&a.kind),
                        "bytes": a.bytes,
                        "shared_bytes": null,
                        "created_at": a.created_at,
                        "shared_with": a.shared_with,
                        "containers": a.containers,
                        "dangling": a.dangling,
                        "note": a.note,
                        "unowned": false,
                        "evidence": a.evidence,
                    }));
                }
            }
        }
    }
    for row in &r.unowned {
        if row.reason != UnownedReason::DockerNoJoin {
            continue;
        }
        let project_field = match only_project {
            None => Value::Null,
            Some(name) => {
                if row
                    .path_or_object
                    .to_lowercase()
                    .contains(&name.to_lowercase())
                {
                    json!(format!("{name} (unowned, name-alike)"))
                } else {
                    continue;
                }
            }
        };
        rows.push(json!({
            "project": project_field,
            "object": row.path_or_object,
            "kind": row.docker_kind,
            "bytes": row.bytes,
            "shared_bytes": row.shared_bytes,
            "created_at": row.created_at,
            "shared_with": row.shared_with,
            "containers": row.containers,
            "dangling": row.dangling,
            "note": row.note,
            "unowned": true,
            "evidence": row.evidence,
        }));
    }
    rows.sort_by(|a, b| {
        b["bytes"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["bytes"].as_u64().unwrap_or(0))
    });
    json!(rows)
}

/// `--view docker --json`'s BuildKit half: one entry per builder, its
/// interior in the shared shape, every size the daemon's logical figure.
/// Empty when the daemon was not asked or did not answer.
pub fn buildkit_payload(r: &Report) -> Value {
    let mut containers: Vec<&crate::artifact::NestedArtifact> = r
        .nested_artifacts
        .iter()
        .filter(|u| u.reported_by.is_some() && Some(u.id.as_str()) == u.container_id.as_deref())
        .collect();
    containers.sort_by(|a, b| a.path.cmp(&b.path));
    json!(
        containers
            .iter()
            .filter_map(|root| {
                let interior = interior_json(&root.path, &r.nested_artifacts)?;
                Some(json!({
                    "builder": root.path.to_string_lossy()
                        .strip_prefix(crate::build_adapters::DAEMON_STORE_SCHEME)
                        .unwrap_or_default(),
                    "reported_by": root.reported_by,
                    "logical_bytes": root.bytes,
                    "limits": root.coverage.limits,
                    "interior": interior,
                }))
            })
            .collect::<Vec<_>>()
    )
}

/// Ranked list of every discovered project: name, id, total bytes,
/// growth, checkout+worktree count, and remote, ranked by growth then
/// bytes desc -- the same ordering the overview text render uses.
pub fn list_projects_payload(r: &Report, only_project: Option<&str>) -> Value {
    let mut rows: Vec<Value> = r
        .projects
        .iter()
        .filter(|p| only_project.is_none_or(|name| name == p.name))
        .map(|p| {
            let checkout_count = p
                .worktrees
                .iter()
                .filter(|w| {
                    matches!(
                        w.kind,
                        crate::report::WorktreeKind::Main | crate::report::WorktreeKind::Clone
                    )
                })
                .count();
            let worktree_count = p.worktrees.len();
            let mut bytes = 0u64;
            let mut growth: Option<i64> = None;
            for wt in &p.worktrees {
                for a in &wt.artifacts {
                    bytes += a.bytes;
                    if let Some(g) = a.growth_bytes {
                        growth = Some(growth.unwrap_or(0) + g);
                    }
                }
            }
            json!({
                "name": p.name,
                "display_name": crate::render::project_display_name(p),
                "project_id": p.project_id,
                "bytes": bytes,
                "growth_bytes": growth,
                "checkout_count": checkout_count,
                "worktree_count": worktree_count,
                "remote": p.remote,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        let ga = a["growth_bytes"].as_i64().unwrap_or(i64::MIN);
        let gb = b["growth_bytes"].as_i64().unwrap_or(i64::MIN);
        gb.cmp(&ga).then_with(|| {
            b["bytes"]
                .as_u64()
                .unwrap_or(0)
                .cmp(&a["bytes"].as_u64().unwrap_or(0))
        })
    });
    json!(rows)
}

fn render_pr_json(pr: &PrStatus) -> Value {
    match pr {
        PrStatus::None => json!("none"),
        PrStatus::Unknown => json!("unknown"),
        PrStatus::Some(pr) => json!({
            "number": pr.number,
            "state": format!("{:?}", pr.state).to_lowercase(),
            "draft": pr.draft,
            "url": pr.url,
            "title": pr.title,
            "review_decision": format!("{:?}", pr.review_decision).to_lowercase(),
            "updated_at": pr.updated_at,
        }),
    }
}

/// One entry per worktree (optionally scoped by `filter`, same grammar
/// as `--filter`, and/or `only_project`), with branch, idle, the
/// `merge_complete` composite fact and its terms, and PR status. Never a
/// verdict, never an action -- `remove_command` is text for a human to
/// run, not something a caller can execute through this payload.
pub fn list_worktrees_payload(r: &Report, filter: &Filter, only_project: Option<&str>) -> Value {
    let mut rows: Vec<Value> = Vec::new();
    for project in &r.projects {
        if only_project.is_some_and(|name| name != project.name) {
            continue;
        }
        for wt in &project.worktrees {
            let empty_pr = PrStatus::Unknown;
            let (pr, verdict, merge_complete_terms) = match (&wt.github, &wt.merge_complete) {
                (
                    Some(GithubFacts { pull_request, .. }),
                    Some(MergeComplete { verdict, terms }),
                ) => (pull_request, *verdict, Some(terms.clone())),
                (Some(GithubFacts { pull_request, .. }), None) => {
                    (pull_request, TriState::Unknown, None)
                }
                (None, _) => (&empty_pr, TriState::Unknown, None),
            };
            let facts = WorktreeFacts {
                merge_complete: verdict == TriState::Yes,
                idle_secs: wt.idle_secs,
                pr,
            };
            if !filter.matches_worktree(project, wt, &facts) {
                continue;
            }
            let verdict_str = match verdict {
                TriState::Yes => "yes",
                TriState::No => "no",
                TriState::Unknown => "unknown",
            };
            rows.push(json!({
                "project": project.name,
                "path": wt.path,
                "branch": wt.branch,
                "idle_secs": wt.idle_secs,
                "merge_complete": merge_complete_terms.map(|terms| json!({
                    "verdict": verdict_str,
                    "terms": terms,
                })),
                "pull_request": render_pr_json(pr),
                "remove_command": format!("git worktree remove {}", wt.path.display()),
                // #60: the worktree's own `Source` row is where
                // `consumer_wiring::attach_associations` (#56/#57)
                // attaches this project's toolchain-declaration/
                // dependency-lockfile facts -- see `model::tree_rows`'s
                // identical lookup in the TUI.
                "evidence": wt
                    .artifacts
                    .iter()
                    .find(|a| a.kind == ArtifactKind::Source)
                    .map(|a| a.evidence.clone())
                    .unwrap_or_default(),
            }));
        }
    }
    json!(rows)
}

/// Rows with growth > 0 across every project/worktree/artifact, sorted
/// desc, plus an unowned summary. The caller adds the coverage block
/// (walked vs du vs unowned vs permission-denied, `observed_at`,
/// `since`, `history_block`) since that needs the store dir and root
/// this payload does not carry.
pub fn what_grew_payload(r: &Report, only_project: Option<&str>) -> Value {
    let mut grown: Vec<Value> = Vec::new();
    for project in &r.projects {
        if only_project.is_some_and(|name| name != project.name) {
            continue;
        }
        for wt in &project.worktrees {
            for a in &wt.artifacts {
                if let Some(g) = a.growth_bytes
                    && g > 0
                {
                    grown.push(json!({
                        "project": project.name,
                        "worktree_id": wt.worktree_id,
                        "kind": kind_label(&a.kind),
                        "path": a.path,
                        "bytes": a.bytes,
                        "growth_bytes": g,
                        "regrowth_count": a.regrowth_count,
                    }));
                }
            }
        }
    }
    grown.sort_by(|a, b| {
        b["growth_bytes"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["growth_bytes"].as_i64().unwrap_or(0))
    });

    let mut unowned_by_reason: std::collections::BTreeMap<&'static str, u64> =
        std::collections::BTreeMap::new();
    let mut permission_denied_count = 0u64;
    for row in &r.unowned {
        *unowned_by_reason
            .entry(reason_label(&row.reason))
            .or_insert(0) += row.bytes;
        if row.reason == UnownedReason::PermissionDenied {
            permission_denied_count += 1;
        }
    }

    json!({
        "grown": grown,
        "unowned_by_reason": unowned_by_reason,
        "permission_denied_count": permission_denied_count,
    })
}

/// Bounds a JSON array in place to `limit` items starting at `offset`.
/// Non-array values are left untouched (`None`). `total` is the array's
/// length before bounding; `truncated` is whether this page dropped any
/// rows (either end) -- the caller surfaces both in the response
/// envelope so a bounded page is never mistaken for the whole answer.
pub struct PageMeta {
    pub total: usize,
    pub truncated: bool,
}

pub fn paginate(value: &mut Value, limit: Option<usize>, offset: usize) -> Option<PageMeta> {
    let arr = value.as_array_mut()?;
    let total = arr.len();
    let start = offset.min(total);
    let end = match limit {
        Some(l) => start.saturating_add(l).min(total),
        None => total,
    };
    let truncated = start > 0 || end < total;
    *arr = arr[start..end].to_vec();
    Some(PageMeta { total, truncated })
}

/// The identified interior of one build/dependency row for `--view
/// builds|deps --json`: the same family summary the text view and the
/// TUI show, plus every unit with its contract fields (`role`,
/// `adapter`, `basis`, `time_source`, `action`, `consequence`,
/// `coverage`). `None` when no adapter identified anything under the
/// row -- absence stays absence, never an empty object.
fn build_interior_json(r: &Report, container: &std::path::Path) -> Option<Value> {
    interior_json(container, &r.nested_artifacts)
}

/// [`build_interior_json`] over any set of units -- a machine-wide
/// store's interior (`--view external --json`) or one BuildKit builder's
/// records (`--view docker --json`) -- so every surface serializes an
/// interior the same way.
pub fn interior_json(
    container: &std::path::Path,
    all: &[crate::artifact::NestedArtifact],
) -> Option<Value> {
    let units: Vec<crate::artifact::NestedArtifact> = all
        .iter()
        .filter(|u| u.present && u.path.starts_with(container))
        .cloned()
        .collect();
    if units.is_empty() {
        return None;
    }
    let summary = crate::build_adapters::summarize_container(container, &units);
    let families: Vec<Value> = summary
        .families
        .iter()
        .map(|f| {
            let actionable = crate::build_adapters::family_members(container, &units, f.family)
                .iter()
                .filter(|u| u.action == crate::artifact::NestedActionCapability::TrashPath)
                .count();
            json!({
                "family": f.family.label(),
                "title": f.family.title(),
                "recommendation": f.recommendation,
                "consequence": f.consequence,
                "other_consequences": f.other_consequences,
                "count": f.count,
                "bytes": (f.basis != crate::artifact::AccountingBasis::Unknown).then_some(f.bytes),
                "basis": f.basis.label(),
                "oldest_modified": f.oldest_modified,
                "unknown_age": f.unknown_age,
                "complete": f.complete,
                "action": if actionable == 0 { "inspection-only" } else { "tui-trash-selection" },
                "actionable_count": actionable,
            })
        })
        .collect();
    Some(json!({
        "families": families,
        "unsupported_count": summary.unsupported_count,
        "unsupported_bytes": summary.unsupported_bytes,
        "unaccounted_bytes": summary.unaccounted_bytes,
        "empty": summary.empty,
        "units": units,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::Confidence;
    use crate::report::{ProjectRow, Reconciliation, Signal, Source, WorktreeKind, WorktreeRow};
    use std::path::PathBuf;

    fn docker_image_row(reference: &str, bytes: u64) -> ArtifactRow {
        ArtifactRow {
            kind: ArtifactKind::DockerImage,
            path: PathBuf::from(reference),
            bytes,
            mtime_max: 0,
            ecosystem: None,
            hardlinked: false,
            dedup_stale: false,
            allocated_bytes: None,
            allocated_growth_bytes: None,
            local_bytes: 0,
            track: None,
            growth_bytes: None,
            regrowth_count: 0,
            observed_at: 1,
            confidence: Confidence::High,
            source: Source::new("docker.system_df"),
            note: None,
            created_at: Some("2026-09-10T12:00:00Z".to_string()),
            containers: vec![
                "hiphi-staging-relay-1 (exited, finished 2026-09-10T13:00:00Z)".to_string(),
            ],
            shared_with: vec!["hiphi-authorizer:staging".to_string()],
            dangling: false,
            evidence: Vec::new(),
        }
    }

    fn fixture_report() -> Report {
        Report {
            store_dir: None,
            observed_at: 1_000_000,
            root: PathBuf::from("/src"),
            projects: vec![ProjectRow {
                project_id: "p1".to_string(),
                name: "hiphi-relay".to_string(),
                remote: None,
                ecosystems: Vec::new(),
                worktrees: vec![WorktreeRow {
                    worktree_id: "wt1".to_string(),
                    path: PathBuf::from("/src/hiphi-relay"),
                    kind: WorktreeKind::Main,
                    artifacts: vec![docker_image_row("hiphi-relay:staging", 500_000_000)],
                    signals: vec![Signal {
                        name: "dirty".to_string(),
                        value: "dirty".to_string(),
                    }],
                    branch: None,
                    github: None,
                    merge_complete: None,
                    idle_secs: None,
                }],
            }],
            unowned: vec![],
            reconciliation: Reconciliation {
                unique_estimate: None,
                attributed: 500_000_000,
                unowned: 0,
                walked_total: 500_000_000,
                du_total: None,
                docker_attributed: 500_000_000,
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
    fn docker_objects_payload_carries_container_and_shared_with_detail() {
        let report = fixture_report();
        let payload = docker_objects_payload(&report, false, None);
        let rows = payload.as_array().expect("array");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row["object"], json!("hiphi-relay:staging"));
        assert_eq!(row["created_at"], json!("2026-09-10T12:00:00Z"));
        assert!(
            row["containers"][0]
                .as_str()
                .unwrap()
                .contains("hiphi-staging-relay-1")
        );
        assert_eq!(row["shared_with"][0], json!("hiphi-authorizer:staging"));
    }

    #[test]
    fn view_payload_reconciliation_matches_report_struct() {
        let report = fixture_report();
        let payload = view_payload(&report, "reconciliation", None);
        assert_eq!(payload["attributed"], json!(500_000_000));
        assert_eq!(payload["docker_attributed"], json!(500_000_000));
    }

    #[test]
    fn view_payload_worktrees_scopes_to_project() {
        let mut report = fixture_report();
        report.projects.push(ProjectRow {
            project_id: "p2".to_string(),
            name: "other".to_string(),
            remote: None,
            ecosystems: Vec::new(),
            worktrees: vec![],
        });
        let payload = view_payload(&report, "worktrees", Some("hiphi-relay"));
        let rows = payload.as_array().expect("array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["project"], json!("hiphi-relay"));
    }

    #[test]
    fn paginate_bounds_array_and_reports_truncation() {
        let mut value = json!([1, 2, 3, 4, 5]);
        let meta = paginate(&mut value, Some(2), 1).expect("array");
        assert_eq!(meta.total, 5);
        assert!(meta.truncated);
        assert_eq!(value, json!([2, 3]));

        let mut whole = json!([1, 2]);
        let meta = paginate(&mut whole, None, 0).expect("array");
        assert_eq!(meta.total, 2);
        assert!(!meta.truncated);
        assert_eq!(whole, json!([1, 2]));
    }

    #[test]
    fn paginate_leaves_non_array_untouched() {
        let mut value = json!({"a": 1});
        assert!(paginate(&mut value, Some(1), 0).is_none());
        assert_eq!(value, json!({"a": 1}));
    }
}
