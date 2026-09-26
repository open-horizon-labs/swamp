//! `CargoAnnotated` → `GrowthAnnotated`: the reverse-delta store. Persists
//! this observation (or reads the store read-only) and fills growth and
//! regrowth on every artifact, directory and file row; groups directory
//! and file rows per worktree when the run asked for them.

use crate::bus::{Consumer, Ctx, Draft, Event, EventKind};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

pub struct GrowthConsumer;

#[async_trait::async_trait(?Send)]
impl Consumer for GrowthConsumer {
    fn name(&self) -> &str {
        "growth"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::CargoAnnotated]
    }
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        let Event::CargoAnnotated(draft) = event else {
            return Ok(vec![]);
        };
        let trace_all = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
        let t0 = std::time::Instant::now();
        let mut d: Draft = (**draft).clone();
        if trace_all {
            eprintln!("[trace] growth: clone draft: {:?}", t0.elapsed());
        }
        let t0 = std::time::Instant::now();
        // CargoAnnotated carries directory totals already aggregated once.
        let roots = crate::report::artifact_roots(&d.projects);
        let nested_shadow_paths =
            add_nested_history_rows(&mut d.projects, &d.nested_artifacts, ctx.observed_at);
        // R15 item 3: the artifact history stores each row's ecosystem,
        // so it has to be known before the store is written, not one
        // stage later in tracking.
        crate::report::annotate_artifact_ecosystems(&mut d.projects);
        if trace_all {
            eprintln!(
                "[trace] growth: nested rows + ecosystems: {:?}",
                t0.elapsed()
            );
        }
        if let Some(dir) = &ctx.store_dir {
            // The store is scoped to the canonical requested root, not just
            // the device. Multiple roots on one volume must never share
            // current state or reverse-delta history.
            let volume_id = crate::growth::root_scoped_volume_id(&ctx.root);
            let config = crate::growth::load_config(dir);
            let since_secs = ctx
                .since_override
                .as_deref()
                .and_then(crate::growth::parse_duration_secs)
                .or_else(|| crate::growth::parse_duration_secs(&config.since))
                .unwrap_or(24 * 3600);
            let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
            let t = std::time::Instant::now();
            if ctx.observe {
                let protected_worktree_ids: std::collections::HashSet<String> =
                    d.protected_worktree_ids.iter().cloned().collect();
                crate::growth::observe_and_annotate(
                    stage,
                    dir,
                    volume_id,
                    &mut d.projects,
                    ctx.observed_at,
                    config.retention_days,
                    since_secs,
                    &protected_worktree_ids,
                )?;
                if trace {
                    eprintln!("[trace] growth: artifacts store: {:?}", t.elapsed());
                }
                let t = std::time::Instant::now();
                crate::growth::observe_and_annotate_dirs(
                    stage,
                    dir,
                    volume_id,
                    &mut d.dirs,
                    ctx.observed_at,
                    config.retention_days,
                    since_secs,
                )?;
                if trace {
                    eprintln!(
                        "[trace] growth: dirs store ({} rows): {:?}",
                        d.dirs.len(),
                        t.elapsed()
                    );
                }
                let t = std::time::Instant::now();
                crate::growth::observe_and_annotate_files(
                    stage,
                    dir,
                    volume_id,
                    &mut d.files,
                    ctx.observed_at,
                    config.retention_days,
                    since_secs,
                )?;
                if trace {
                    eprintln!(
                        "[trace] growth: files store ({} rows): {:?}",
                        d.files.len(),
                        t.elapsed()
                    );
                }
            } else {
                crate::growth::annotate_readonly(
                    dir,
                    volume_id,
                    &mut d.projects,
                    ctx.observed_at,
                    config.retention_days,
                    since_secs,
                )?;
                crate::growth::annotate_readonly_dirs(
                    dir,
                    volume_id,
                    &mut d.dirs,
                    ctx.observed_at,
                    config.retention_days,
                    since_secs,
                )?;
                crate::growth::annotate_readonly_files(
                    dir,
                    volume_id,
                    &mut d.files,
                    ctx.observed_at,
                    config.retention_days,
                    since_secs,
                )?;
            }
            let t = std::time::Instant::now();
            d.schedule_line = Some(crate::schedule::header_line(
                dir,
                &ctx.root,
                ctx.observed_at,
            ));
            if trace_all {
                eprintln!("[trace] growth: schedule line: {:?}", t.elapsed());
            }
        }
        let t0 = std::time::Instant::now();
        crate::report::attach_allocated_from_dirs(&mut d.projects, &d.dirs, &roots);
        if trace_all {
            eprintln!("[trace] growth: allocated from dirs: {:?}", t0.elapsed());
        }
        let t0 = std::time::Instant::now();
        // Interior rows of folded artifacts live in the store only: the
        // report shows an artifact as one unit.
        d.dirs
            .retain(|row| !crate::report::dir_inside_artifact(row, &roots));
        copy_nested_history(
            &mut d.projects,
            Arc::make_mut(&mut d.nested_artifacts).as_mut_slice(),
            &nested_shadow_paths,
        );
        if ctx.include_dirs {
            let mut by_dir: HashMap<String, Vec<_>> = HashMap::new();
            for row in std::mem::take(&mut d.dirs) {
                by_dir.entry(row.worktree_id.clone()).or_default().push(row);
            }
            let mut by_file: HashMap<String, Vec<_>> = HashMap::new();
            for row in std::mem::take(&mut d.files) {
                by_file
                    .entry(row.worktree_id.clone())
                    .or_default()
                    .push(row);
            }
            crate::report::sort_drill_down(&mut by_dir, &mut by_file);
            d.dirs_by_worktree = Some(by_dir);
            d.files_by_worktree = Some(by_file);
        }
        if trace_all {
            eprintln!(
                "[trace] growth: retain/nested/group/sort: {:?}",
                t0.elapsed()
            );
        }
        Ok(vec![Event::GrowthAnnotated(Arc::new(d))])
    }
}

fn add_nested_history_rows(
    projects: &mut [crate::report::ProjectRow],
    nested: &[crate::artifact::NestedArtifact],
    observed_at: u64,
) -> Vec<(String, std::path::PathBuf)> {
    let mut paths = Vec::new();
    for unit in nested {
        let Some((_, wt)) = projects
            .iter_mut()
            .flat_map(|p| p.worktrees.iter_mut())
            .filter(|wt| {
                unit.path.starts_with(&wt.path)
                    || wt
                        .artifacts
                        .iter()
                        .any(|a| a.source.tool != "cargo.layout" && unit.path.starts_with(&a.path))
            })
            .max_by_key(|wt| wt.path.components().count())
            .map(|wt| ((), wt))
        else {
            continue;
        };
        wt.artifacts.push(crate::report::ArtifactRow {
            kind: crate::report::ArtifactKind::Unknown,
            path: unit.path.clone(),
            bytes: unit.bytes,
            mtime_max: unit.mtime_max,
            ecosystem: None,
            hardlinked: matches!(unit.membership, crate::artifact::Membership::SharedHardlink),
            dedup_stale: false,
            allocated_bytes: None,
            allocated_growth_bytes: None,
            local_bytes: unit.physical_bytes,
            track: None,
            growth_bytes: unit.growth_bytes,
            regrowth_count: unit.regrowth_count,
            observed_at,
            confidence: crate::entities::Confidence::Medium,
            source: crate::report::Source::new("cargo.layout"),
            note: Some(format!("nested-id={}", unit.id)),
            created_at: None,
            containers: Vec::new(),
            shared_with: Vec::new(),
            dangling: false,
            evidence: Vec::new(),
        });
        paths.push((unit.id.clone(), unit.path.clone()));
    }
    paths
}

fn copy_nested_history(
    projects: &mut [crate::report::ProjectRow],
    nested: &mut [crate::artifact::NestedArtifact],
    paths: &[(String, std::path::PathBuf)],
) {
    let _ = paths;
    let facts: HashMap<_, _> = projects
        .iter()
        .flat_map(|p| &p.worktrees)
        .flat_map(|w| &w.artifacts)
        .filter(|a| a.source.tool == "cargo.layout")
        .map(|a| (a.path.clone(), (a.growth_bytes, a.regrowth_count)))
        .collect();
    for unit in nested {
        if let Some((growth, regrowth)) = facts.get(&unit.path) {
            unit.growth_bytes = *growth;
            unit.regrowth_count = *regrowth;
        }
    }
    for project in projects {
        for wt in &mut project.worktrees {
            wt.artifacts.retain(|a| a.source.tool != "cargo.layout");
        }
    }
}
