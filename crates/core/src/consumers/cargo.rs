//! Build-artifact identification over the folded directory
//! measurements, through the static adapter registry.
//!
//! The consumer used to be Cargo's and only Cargo's: it called
//! `cargo_artifacts::folded_units` by name. Adding Node would have meant
//! a second named call, and the fourth would have meant a `match`. It
//! now builds one [`crate::build_adapters::BuildCtx`] and hands it to
//! [`crate::build_adapters::identify_all`], so a new ecosystem is one
//! line in `build_adapters::Registry::with_builtins()` and nothing here
//! changes (`.oh/guardrails/build-adapters-are-pluggable.md`).
//!
//! The consumer keeps the two jobs an adapter must not do: deciding
//! *which* containers exist in the authorized scope, and deciding
//! whether the previous pass's units may be replayed. The replay gate is
//! [`crate::fs_events::EventCoverage`], built from this root's trusted
//! window -- the same gate stack/13 put on the agent containers, and
//! never a directory stamp.
use crate::build_adapters::{BuildContainer, BuildCtx, ContainerCache, FoldedDir, FoldedIndex};
use crate::bus::{Consumer, Ctx, Draft, Event, EventKind};
use anyhow::Result;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub struct CargoConsumer {
    changed: Mutex<Option<Arc<Vec<PathBuf>>>>,
    /// The static adapter set, built once when the consumer is
    /// constructed rather than per event. Nothing registers an adapter
    /// at runtime; `Registry::with_builtins()` is a fixed list, and the
    /// `build_adapters_are_pluggable` audit checks it against the module
    /// set so an unregistered adapter fails the build.
    adapters: crate::build_adapters::registry::Registry,
}

impl Default for CargoConsumer {
    fn default() -> Self {
        Self {
            changed: Mutex::new(None),
            adapters: crate::build_adapters::registry::Registry::with_builtins(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Consumer for CargoConsumer {
    fn name(&self) -> &str {
        "build-artifacts"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::RootObserved, EventKind::RowsAssembled]
    }
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        if let Event::RootObserved { changed_paths, .. } = event {
            *self.changed.lock().unwrap() = changed_paths.clone();
            return Ok(vec![]);
        }
        let Event::RowsAssembled(draft) = event else {
            return Ok(vec![]);
        };
        let mut draft: Draft = (**draft).clone();
        // Aggregate once, before either identification or history uses it.
        let artifact_roots = crate::report::artifact_roots(&draft.projects);
        crate::report::aggregate_dir_totals(&mut draft.dirs, &artifact_roots);

        // The previous pass's nested-artifact units, from
        // `cargo_replay_cache.parquet` (R18a-4) -- root-keyed, never
        // scope-keyed, so this replay decision is available on this
        // root's own bus pass whether it is a single-root, scope-less
        // call or one root inside a multi-root scope (unlike
        // `nested_artifacts.parquet`, which is scope-wide and only
        // written once, at the end of `observe_scope`, after every
        // root's pass -- including this one -- has already run).
        let root_key = crate::growth::root_key(&ctx.root);
        let previous = ctx
            .store_dir
            .as_deref()
            .and_then(|d| crate::growth::read_cargo_replay_cache(d, &root_key));

        // The replay gate. A root this pass replayed successfully leaves
        // a trusted window here; a full walk, a refusal or a store-less
        // call leaves none, and then nothing is reused.
        let mut coverage = crate::fs_events::EventCoverage::untrusted();
        if !ctx.force_full
            && let Some((changed, since)) = ctx.event_window.lock().unwrap().clone()
        {
            coverage.trust(ctx.root.clone(), changed, since);
        }
        let cache = match &previous {
            Some((observed_at, units)) if !ctx.force_full => {
                ContainerCache::from_previous(units.clone(), *observed_at)
            }
            _ => ContainerCache::disabled(),
        };

        let folded = folded_index(&draft);
        let facts = draft.docker_facts.clone();
        let mut build_ctx = BuildCtx::new(ctx.observed_at, &folded, &coverage, &cache);
        if let Some(f) = facts.as_deref() {
            build_ctx = build_ctx.with_daemon(f);
        }
        let projects = project_candidates(&draft);
        let shared = shared_containers(&self.adapters, facts.as_deref());
        let nested =
            crate::build_adapters::identify_all(&self.adapters, &projects, &shared, &build_ctx);

        // Persist this pass's units as the next pass's replay cache.
        // Gated on `ctx.observe` like every other current-state table
        // write -- a pure-read call must never advance what a later
        // real observation replays from.
        if ctx.observe
            && let Some(store) = ctx.store_dir.as_deref()
        {
            crate::growth::write_cargo_replay_cache(store, &root_key, &nested, ctx.observed_at)?;
        }

        draft.nested_artifacts = Arc::new(nested);
        Ok(vec![Event::CargoAnnotated(Arc::new(draft))])
    }
}

/// The folded walk's directory rows, as absolute paths.
///
/// This is the whole structural input the adapters get, and building it
/// once per pass is what keeps "no second traversal" true: every adapter
/// reads from this map, and none of them lists a directory the walk
/// already listed.
fn folded_index(draft: &Draft) -> FoldedIndex {
    let worktrees: std::collections::HashMap<&str, &std::path::Path> = draft
        .projects
        .iter()
        .flat_map(|p| &p.worktrees)
        .map(|w| (w.worktree_id.as_str(), w.path.as_path()))
        .collect();
    FoldedIndex::from_dirs(draft.dirs.iter().filter_map(|d| {
        let worktree = worktrees.get(d.worktree_id.as_str())?;
        Some(FoldedDir {
            path: worktree.join(&d.rel_path),
            allocated_total: d.allocated_total,
            // `mod_time_min` is minutes since the epoch (the walk's own
            // resolution). Seconds here, so an adapter never has to know
            // which unit a stored column is in.
            mtime_max: d.mod_time_min.max(0) as u64 * 60,
            complete: d.complete,
        })
    }))
}

/// Each worktree, with the artifact directories the walk classified
/// beneath it.
///
/// Adapters decide which of these are theirs, by name and by the marker
/// files at the checkout root. Nothing here knows what a `target/` or a
/// `node_modules` is -- that knowledge is the adapter's, which is the
/// point.
fn project_candidates(draft: &Draft) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut out = Vec::new();
    for project in &draft.projects {
        for wt in &project.worktrees {
            let candidates: Vec<PathBuf> = wt
                .artifacts
                .iter()
                .filter(|a| !a.kind.is_worktree_remainder() && crate::fs_gate::is_dir(&a.path))
                .map(|a| a.path.clone())
                .collect();
            out.push((wt.path.clone(), candidates));
        }
    }
    out
}

/// The stores this *per-root* pass hands an adapter: the Docker
/// daemon's BuildKit caches, one per builder, when the daemon was asked
/// and answered.
///
/// Until 2026-09-22 this returned an empty list for every machine-wide
/// store, deliberately, because the npm/pnpm stores, Gradle homes, Maven
/// repositories, Go/Python caches, DerivedData, CoreSimulator and the
/// Android SDK are detector-resolved external locations whose
/// measurement and history window `external::observe_external` owns --
/// joining them from here would observe the same bytes twice in one pass.
/// They are now joined *there*, from the folded rows that observation
/// already produces (`crate::build_stores::containers_for`), and reach
/// the report as `ScopeObservation::store_interiors`.
///
/// A BuildKit cache is the one store that belongs here: it is not on
/// disk for anything to walk, the daemon answered for it in this very
/// pass (`consumers::docker`), and its records have no filesystem
/// history to own. Matched by the same two declared capabilities
/// (`.oh/guardrails/build-stores-join-by-capability.md`).
fn shared_containers(
    adapters: &crate::build_adapters::registry::Registry,
    facts: Option<&crate::docker::DockerFacts>,
) -> Vec<BuildContainer> {
    let Some(facts) = facts else {
        return Vec::new();
    };
    crate::build_stores::daemon_containers(
        adapters,
        &crate::locations::Registry::with_builtins(),
        facts,
    )
}
