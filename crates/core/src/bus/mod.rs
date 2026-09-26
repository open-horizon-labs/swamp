//! The report pipeline as consumers on an in-memory event bus.
//!
//! Every stage of building a [`Report`] is a [`Consumer`] that declares
//! which [`EventKind`]s wake it and returns follow-on events. The bus holds
//! the registry and routes; **it is the only coupling between stages**.
//! No consumer knows another consumer exists. See
//! `docs/ADRs/001-event-bus-report-pipeline.md`.
//!
//! **Static registration, dynamic routing.** [`EventBus::with_builtins`]
//! registers every consumer before the first event fires; `run` seals the
//! registry. There is no runtime registration and no conditional wiring:
//! a consumer that has nothing to do for a run returns no events.
//!
//! **Runtime.** Consumers are `async fn on_event`, dispatched on a tokio
//! current-thread runtime ([`EventBus::run_blocking`]). All subscribers of
//! one event run concurrently; the follow-on events they return are
//! dispatched depth-first, in the order the consumers were registered.
//!
//! **Facts flow as events.** A consumer never sees `&mut Report`. It gets
//! the facts it subscribed to and emits new ones; gates wait for a set of
//! facts and emit the assembled rows; the assembler folds the final facts
//! into the report. Payloads that several consumers read are `Arc`s.
//!
//! Adding a stage: implement [`Consumer`] in a new file under
//! `consumers/`, register it in [`EventBus::with_builtins`]. The source
//! audits (`cargo run -p swamp-source-audit`) fail the build if a
//! consumer names another, registers at runtime, or if `report.rs` calls a
//! stage directly.

use crate::attribution::AttributionResult;
use crate::git::DiscoveredWorktree;
use crate::github::GithubFacts;
use crate::report::Signal;
use crate::report::{
    ArtifactRow, DirRollup, FileRow, GithubEnrichmentSummary, ProjectRow, Reconciliation, Report,
    UnownedRow,
};
use crate::signals::RawSignals;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod registry;

/// Everything a run was asked for. Consumers read it; nobody writes it.
pub struct Ctx<'a> {
    pub root: PathBuf,
    pub docker_facts: Option<PathBuf>,
    pub verify_du: bool,
    pub store_dir: Option<PathBuf>,
    pub since_override: Option<String>,
    /// Persist a new observation into the growth store.
    pub observe: bool,
    /// Group per-directory rollups under each worktree into the report.
    pub include_dirs: bool,
    /// Refresh GitHub enrichment live (shells out to `gh`).
    pub enrich: bool,
    /// Skip the FSEvents attempt and walk everything.
    pub force_full: bool,
    pub fs_events: &'a dyn crate::fs_events::FsEventsSource,
    pub observed_at: u64,
    pub large_file_min_bytes: u64,
    /// Subtrees to prune from this walk (#42 --
    /// `scope::EffectiveScope::pruned_subtrees`, filtered to this root).
    /// Empty for every pre-#42 caller (a single-root `report`/`observe`
    /// call with no scope-level exclusion context); populated only by
    /// `report::report_scope`.
    pub pruned_subtrees: Vec<PathBuf>,
    /// Whether this invocation's authorized scope includes Docker at
    /// all.
    ///
    /// The 2026-09-22 re-review's CE6: `consumers::docker` called
    /// `docker::load_cached` on every `ProjectsGrouped` event and
    /// consulted the effective scope nowhere, so `docker-desktop` being
    /// disabled -- or excluded -- did not stop swamp asking the Docker
    /// daemon to enumerate the user's images, volumes and containers.
    /// Measured: five `docker` spawns per observation, on every
    /// `report`, every `propose` and every TUI startup, permanently, for
    /// a tool the user did not authorize.
    ///
    /// `true` for the pre-scope single-root entry points, which have no
    /// scope to consult and whose behavior must not change; the
    /// scope-aware path (`report::report_scope_with_parts`) derives it
    /// from `EffectiveScope::docker_in_scope()`.
    pub docker_in_scope: bool,
    /// Where `consumers::walk` leaves this root's trusted event window,
    /// so the caller can hand it to the unit families as
    /// `crate::fs_events::EventCoverage`.
    ///
    /// `None` means this root produced no trusted window (a full walk,
    /// any refusal, no store), and the unit families then reuse nothing.
    /// `Some((changed, since))` is the replay's own change list -- the
    /// unfiltered one, because a subtree this walk pruned is still a
    /// subtree the window must be able to speak about -- and the
    /// observation time it replays from.
    pub event_window: crate::fs_events::EventWindowSlot,
}

/// Per-worktree git activity, as one consumer computes it and others read it.
#[derive(Debug, Clone)]
pub struct WorktreeSignals {
    pub path: PathBuf,
    pub rows: Vec<Signal>,
    pub raw: RawSignals,
    pub branch: Option<String>,
}

/// The report under construction: what the assembly gate hands to the
/// growth store, which hands it on to tracking, which hands it to the
/// assembler. Each stage returns a new `Draft`; nothing is mutated in place
/// across consumers.
#[derive(Debug, Clone)]
pub struct Draft {
    pub projects: Vec<ProjectRow>,
    pub unowned: Vec<UnownedRow>,
    pub dirs: Vec<DirRollup>,
    pub files: Vec<FileRow>,
    pub dirs_by_worktree: Option<HashMap<String, Vec<DirRollup>>>,
    pub files_by_worktree: Option<HashMap<String, Vec<FileRow>>>,
    pub notes: Vec<String>,
    pub reconciliation: Reconciliation,
    pub github_enrichment: Option<GithubEnrichmentSummary>,
    pub schedule_line: Option<String>,
    pub nested_artifacts: Arc<Vec<crate::artifact::NestedArtifact>>,
    /// The Docker daemon's answers this pass, when it was asked and
    /// answered: what the build consumer hands the BuildKit adapter
    /// (`crate::build_stores::daemon_containers`). `None` when Docker
    /// was out of scope.
    pub docker_facts: Option<Arc<crate::docker::DockerFacts>>,
    /// Worktree ids the walk could not confirm gone-vs-inaccessible this
    /// pass (#42) -- see `growth::compute_unconfirmed_worktrees`. The
    /// growth store must never tombstone rows for these ids from this
    /// observation.
    pub protected_worktree_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    RootRequested,
    RootObserved,
    ProjectsGrouped,
    SignalsComputed,
    EcosystemsDetected,
    GithubEnriched,
    DockerJoined,
    RowsAssembled,
    CargoAnnotated,
    GrowthAnnotated,
    TrackingAnnotated,
    HistoryLoaded,
    ReportAssembled,
    ReportCached,
    /// Test-only traffic for the bus's own tests; no builtin subscribes.
    Probe,
}

/// Every fact the pipeline emits. Each variant carries what its readers
/// need; no consumer polls context for another's output.
#[derive(Debug, Clone)]
pub enum Event {
    RootRequested,
    RootObserved {
        changed_paths: Option<Arc<Vec<PathBuf>>>,
        discovered: Arc<Vec<DiscoveredWorktree>>,
        attribution: Arc<AttributionResult>,
        notes: Vec<String>,
        /// Worktree ids the walk actually visited; `None` = all of them.
        rewalked: Option<Arc<Vec<String>>>,
        /// Worktree ids this pass could not confirm gone-vs-inaccessible
        /// (#42): absent from `discovered`, but their path still exists
        /// and could not be read. Carried to `GrowthAnnotated` so the
        /// growth store never tombstones their rows from this pass.
        unconfirmed_worktree_ids: Arc<Vec<String>>,
    },
    ProjectsGrouped {
        projects: Arc<Vec<ProjectRow>>,
        rewalked: Option<Arc<Vec<String>>>,
        worktree_paths: Arc<Vec<(PathBuf, String)>>,
        /// project_id -> normalized remote URL.
        project_remotes: Arc<HashMap<String, String>>,
        /// worktree_id -> that worktree's raw remote URL.
        worktree_remotes: Arc<HashMap<String, String>>,
        unowned: Arc<Vec<UnownedRow>>,
        dirs: Arc<Vec<DirRollup>>,
        files: Arc<Vec<FileRow>>,
        reconciliation: Reconciliation,
        notes: Vec<String>,
        unconfirmed_worktree_ids: Arc<Vec<String>>,
    },
    SignalsComputed {
        by_worktree: Arc<HashMap<String, WorktreeSignals>>,
    },
    EcosystemsDetected {
        /// project_id -> ecosystem tags.
        by_project: Arc<HashMap<String, Vec<String>>>,
    },
    GithubEnriched {
        facts_by_worktree: Arc<HashMap<String, GithubFacts>>,
        summary: Option<GithubEnrichmentSummary>,
        notes: Vec<String>,
    },
    DockerJoined {
        rows_by_worktree: Arc<HashMap<String, Vec<ArtifactRow>>>,
        unowned: Arc<Vec<UnownedRow>>,
        attributed_bytes: u64,
        unowned_bytes: u64,
        notes: Vec<String>,
        /// The facts the rows came from, for the BuildKit record
        /// identification; `None` when the daemon was not asked.
        facts: Option<Arc<crate::docker::DockerFacts>>,
    },
    RowsAssembled(Arc<Draft>),
    CargoAnnotated(Arc<Draft>),
    GrowthAnnotated(Arc<Draft>),
    TrackingAnnotated(Arc<Draft>),
    HistoryLoaded {
        series_by_key: Arc<HashMap<String, Vec<Option<u64>>>>,
        total_series: Vec<Option<u64>>,
        window_secs: u64,
    },
    ReportAssembled(Arc<Report>),
    /// All observation/history consumers and the rebuildable report cache have
    /// completed successfully. Only now may the walk advance its replay anchor.
    ReportCached,
    Probe {
        tag: String,
        depth: u8,
    },
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Event::RootRequested => EventKind::RootRequested,
            Event::RootObserved { .. } => EventKind::RootObserved,
            Event::ProjectsGrouped { .. } => EventKind::ProjectsGrouped,
            Event::SignalsComputed { .. } => EventKind::SignalsComputed,
            Event::EcosystemsDetected { .. } => EventKind::EcosystemsDetected,
            Event::GithubEnriched { .. } => EventKind::GithubEnriched,
            Event::DockerJoined { .. } => EventKind::DockerJoined,
            Event::RowsAssembled(_) => EventKind::RowsAssembled,
            Event::CargoAnnotated(_) => EventKind::CargoAnnotated,
            Event::GrowthAnnotated(_) => EventKind::GrowthAnnotated,
            Event::TrackingAnnotated(_) => EventKind::TrackingAnnotated,
            Event::HistoryLoaded { .. } => EventKind::HistoryLoaded,
            Event::ReportAssembled(_) => EventKind::ReportAssembled,
            Event::ReportCached => EventKind::ReportCached,
            Event::Probe { .. } => EventKind::Probe,
        }
    }
}

/// One stage. **Must not** register consumers, name other consumers, or
/// reach for another consumer's output except through the events it
/// subscribes to.
#[async_trait::async_trait(?Send)]
pub trait Consumer {
    /// Identifier for diagnostics.
    fn name(&self) -> &str;
    /// Which event kinds wake this consumer. `on_event` is called only for
    /// events whose kind appears here.
    fn subscribes_to(&self) -> &[EventKind];
    /// React to an event; return follow-on events to emit.
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>>;
}

pub use registry::{EventBus, Stage};

/// Builds a [`Report`] for `ctx` by running the builtin consumers from
/// `RootRequested`. The one entry point the report module calls.
pub fn run_report(ctx: &Ctx<'_>) -> Result<Report> {
    let mut bus = EventBus::with_builtins();
    let events = bus.run_blocking(Event::RootRequested, ctx)?;
    for e in events.into_iter().rev() {
        if let Event::ReportAssembled(report) = e {
            let mut report = Arc::try_unwrap(report).unwrap_or_else(|arc| (*arc).clone());
            // Decision evidence (#53-#54, #58-#59): a pure post-pass over
            // facts this report already collected, never a new walk or
            // byte-history write -- see `report::attach_decision_evidence`.
            crate::report::attach_decision_evidence(&mut report);
            return Ok(report);
        }
    }
    bail!("the bus finished without assembling a report")
}

/// The context a caller builds from the report-function arguments.
#[allow(clippy::too_many_arguments)]
pub fn ctx_for<'a>(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events: &'a dyn crate::fs_events::FsEventsSource,
) -> Ctx<'a> {
    ctx_for_excluding(
        root,
        docker_facts,
        verify_du,
        store_dir,
        since_override,
        observe,
        include_dirs,
        enrich,
        force_full,
        fs_events,
        &[],
    )
}

/// Same as [`ctx_for`], with `pruned_subtrees` (#42) supplied explicitly.
/// Used by `report::report_scope`, which has scope-level exclusions to
/// enforce per root; every other caller goes through [`ctx_for`] with an
/// empty list.
#[allow(clippy::too_many_arguments)]
pub fn ctx_for_excluding<'a>(
    root: &Path,
    docker_facts: Option<&Path>,
    verify_du: bool,
    store_dir: Option<&Path>,
    since_override: Option<&str>,
    observe: bool,
    include_dirs: bool,
    enrich: bool,
    force_full: bool,
    fs_events: &'a dyn crate::fs_events::FsEventsSource,
    pruned_subtrees: &[PathBuf],
) -> Ctx<'a> {
    let large_file_min_bytes = store_dir
        .map(|dir| crate::growth::load_config(dir).large_file_min_bytes)
        .unwrap_or(crate::growth::DEFAULT_LARGE_FILE_MIN_BYTES);
    Ctx {
        root: root.to_path_buf(),
        docker_facts: docker_facts.map(Path::to_path_buf),
        verify_du,
        store_dir: store_dir.map(Path::to_path_buf),
        since_override: since_override.map(str::to_string),
        observe,
        include_dirs,
        enrich,
        force_full,
        fs_events,
        observed_at: crate::entities::now(),
        large_file_min_bytes,
        pruned_subtrees: pruned_subtrees.to_vec(),
        docker_in_scope: true,
        event_window: std::sync::Arc::new(std::sync::Mutex::new(None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn ctx<'a>(src: &'a dyn crate::fs_events::FsEventsSource) -> Ctx<'a> {
        ctx_for(
            Path::new("/nonexistent"),
            None,
            false,
            None,
            None,
            false,
            false,
            false,
            true,
            src,
        )
    }

    /// Records every probe it sees; re-emits one deeper probe per event
    /// until `max_depth`.
    struct Prober {
        name: String,
        seen: Mutex<Vec<String>>,
        max_depth: u8,
    }
    #[async_trait::async_trait(?Send)]
    impl Consumer for Prober {
        fn name(&self) -> &str {
            &self.name
        }
        fn subscribes_to(&self) -> &[EventKind] {
            &[EventKind::Probe]
        }
        async fn on_event(
            &self,
            event: &Event,
            _ctx: &Ctx<'_>,
            _stage: &crate::bus::Stage,
        ) -> Result<Vec<Event>> {
            let Event::Probe { tag, depth } = event else {
                return Ok(vec![]);
            };
            self.seen.lock().unwrap().push(format!("{tag}@{depth}"));
            if *depth < self.max_depth {
                return Ok(vec![Event::Probe {
                    tag: format!("{}>{}", tag, self.name),
                    depth: depth + 1,
                }]);
            }
            Ok(vec![])
        }
    }

    fn prober(name: &str, max_depth: u8) -> Box<Prober> {
        Box::new(Prober {
            name: name.into(),
            seen: Mutex::new(vec![]),
            max_depth,
        })
    }

    #[test]
    fn follow_on_events_are_routed_to_subscribers() {
        let src = crate::fs_events::UnsupportedPlatformSource;
        let c = ctx(&src);
        let mut bus = EventBus::new_for_test();
        bus.register_for_test(prober("a", 1)).unwrap();
        let events = bus
            .run_blocking(
                Event::Probe {
                    tag: "seed".into(),
                    depth: 0,
                },
                &c,
            )
            .unwrap();
        let tags: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                Event::Probe { tag, .. } => Some(tag.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            tags,
            vec!["seed", "seed>a"],
            "the follow-on was dispatched too"
        );
    }

    #[test]
    fn follow_on_events_dispatch_depth_first() {
        // Two seeds queued; a's follow-on from seed1 must run before seed2.
        let src = crate::fs_events::UnsupportedPlatformSource;
        let c = ctx(&src);
        let mut bus = EventBus::new_for_test();
        bus.register_for_test(prober("a", 1)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let events = rt
            .block_on(async {
                // Seed with a probe whose follow-on itself has a follow-on:
                // order must be seed, seed>a, (seed>a)>a — never breadth-first.
                bus.register_for_test(prober("b", 2)).unwrap();
                bus.run(
                    Event::Probe {
                        tag: "s".into(),
                        depth: 0,
                    },
                    &c,
                )
                .await
            })
            .unwrap();
        let tags: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                Event::Probe { tag, .. } => Some(tag.clone()),
                _ => None,
            })
            .collect();
        // a and b both answer "s" (depth 0): follow-ons s>a and s>b are
        // queued in registration order. Depth-first means s>a's own
        // descendants (only b answers at depth 1: s>a>b) come before s>b.
        assert_eq!(tags, vec!["s", "s>a", "s>a>b", "s>b", "s>b>b"]);
    }

    #[test]
    fn consumer_receives_events_regardless_of_registration_order() {
        let src = crate::fs_events::UnsupportedPlatformSource;
        let c = ctx(&src);
        for order in [["a", "b"], ["b", "a"]] {
            let mut bus = EventBus::new_for_test();
            for n in order {
                bus.register_for_test(prober(n, 0)).unwrap();
            }
            let events = bus
                .run_blocking(
                    Event::Probe {
                        tag: "x".into(),
                        depth: 0,
                    },
                    &c,
                )
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(bus.consumer_names().len(), 2);
        }
    }

    #[test]
    fn registration_is_closed_once_run_starts() {
        let src = crate::fs_events::UnsupportedPlatformSource;
        let c = ctx(&src);
        let mut bus = EventBus::new_for_test();
        bus.register_for_test(prober("a", 0)).unwrap();
        bus.run_blocking(
            Event::Probe {
                tag: "x".into(),
                depth: 0,
            },
            &c,
        )
        .unwrap();
        let err = bus.register_for_test(prober("late", 0)).unwrap_err();
        assert!(err.to_string().contains("sealed"), "{err}");
    }

    #[test]
    fn builtins_cover_the_whole_pipeline_once_each() {
        let bus = EventBus::with_builtins();
        let names = bus.consumer_names();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "a consumer registered twice");
        for needed in [
            "walk", "projects", "signals", "github", "docker", "gate", "growth", "tracking",
            "history", "assemble", "cache",
        ] {
            assert!(
                names.iter().any(|n| n.contains(needed)),
                "missing {needed}: {names:?}"
            );
        }
    }
}
