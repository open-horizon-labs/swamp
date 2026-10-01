//! `ProjectsGrouped` → `DockerJoined`: images, build cache and volumes
//! joined to worktrees on explicit evidence only (compose label, compose
//! file `name:`, `image.source` matching a remote); the rest unowned with
//! the reason.

use crate::bus::{Consumer, Ctx, Event, EventKind};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

fn force_docker_refresh(enrich: bool, explicit_refresh: bool) -> bool {
    enrich && explicit_refresh
}

fn capture_age_note(captured_at: Option<u64>, observed_at: u64, from_facts_file: bool) -> String {
    let source = if from_facts_file {
        "facts file; automatic refresh does not apply"
    } else {
        "daemon capture; refresh window 5 minutes"
    };
    match captured_at {
        Some(captured_at) if captured_at <= observed_at => format!(
            "Docker facts: {} seconds before this observation ({source})",
            observed_at - captured_at
        ),
        Some(captured_at) => format!(
            "Docker facts: capture is {} seconds after this observation timestamp ({source})",
            captured_at - observed_at
        ),
        None => format!("Docker facts: capture time not recorded (age unknown; {source})"),
    }
}

fn append_note(note: &mut Option<String>, addition: &str) {
    *note = Some(match note.take() {
        Some(existing) if !existing.is_empty() => format!("{existing}; {addition}"),
        _ => addition.to_string(),
    });
}

fn annotate_capture_age(join: &mut crate::report::DockerJoinResult, note: &str) {
    for row in join.rows_by_worktree.values_mut().flatten() {
        if matches!(
            row.kind,
            crate::report::ArtifactKind::DockerImage
                | crate::report::ArtifactKind::DockerBuildCache
                | crate::report::ArtifactKind::DockerVolume
        ) {
            append_note(&mut row.note, note);
        }
    }
    for row in &mut join.unowned {
        if row.reason == crate::report::UnownedReason::DockerNoJoin {
            append_note(&mut row.note, note);
        }
    }
}

#[cfg(test)]
type TestLoader = Arc<
    dyn Fn(
            Option<std::path::PathBuf>,
            Option<std::path::PathBuf>,
            bool,
        ) -> crate::docker::DockerFacts
        + Send
        + Sync,
>;

pub struct DockerConsumer {
    pending: std::sync::Mutex<Option<std::thread::JoinHandle<crate::docker::DockerFacts>>>,
    #[cfg(test)]
    test_loader: Option<TestLoader>,
}

impl Default for DockerConsumer {
    fn default() -> Self {
        Self {
            pending: std::sync::Mutex::new(None),
            #[cfg(test)]
            test_loader: None,
        }
    }
}

impl DockerConsumer {
    #[cfg(test)]
    fn with_loader(loader: TestLoader) -> Self {
        Self {
            pending: std::sync::Mutex::new(None),
            test_loader: Some(loader),
        }
    }

    fn start_prefetch(&self, ctx: &Ctx<'_>) {
        // Match the existing ProjectsGrouped gate exactly: an excluded or
        // disabled Docker detector does no work; an explicit facts file is
        // still an authorized input even when the detector is disabled.
        if !(ctx.docker_in_scope || ctx.docker_facts.is_some()) {
            return;
        }
        if self.pending.lock().unwrap().is_some() {
            return;
        }
        let facts_path = ctx.docker_facts.clone();
        let store_dir = ctx.store_dir.clone();
        let fresh = force_docker_refresh(ctx.enrich, crate::docker::force_refresh());
        // Attribute cache/file work performed by this observation to its
        // scoped measurement, just as the walk and GitHub workers do.
        let counters = crate::work_counters::current();
        #[cfg(test)]
        let test_loader = self.test_loader.clone();

        let spawned = std::thread::Builder::new()
            .name("swamp-docker-facts".into())
            .spawn(move || {
                crate::work_counters::install(counters);
                #[cfg(test)]
                if let Some(loader) = test_loader {
                    return loader(facts_path, store_dir, fresh);
                }
                crate::docker::load_cached(facts_path.as_deref(), store_dir.as_deref(), fresh)
            });
        let Ok(handle) = spawned else {
            // Thread creation failure is a performance miss, not a report
            // failure. ProjectsGrouped falls back to the original inline load.
            return;
        };
        *self.pending.lock().unwrap() = Some(handle);
    }

    fn take_prefetched(&self) -> Option<crate::docker::DockerFacts> {
        let handle = self.pending.lock().unwrap().take()?;
        Some(
            handle
                .join()
                .unwrap_or_else(|_| crate::docker::DockerFacts {
                    unavailable: Some("docker: unavailable (prefetch worker failed)".to_string()),
                    ..Default::default()
                }),
        )
    }
}

impl Drop for DockerConsumer {
    fn drop(&mut self) {
        // A walk may fail before ProjectsGrouped is dispatched. Keep any
        // cache read/write owned by this report: dropping the bus joins the
        // prefetch instead of detaching work that can outlive the report.
        let pending = self
            .pending
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(handle) = pending.take() {
            let _ = handle.join();
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Consumer for DockerConsumer {
    fn name(&self) -> &str {
        "docker"
    }
    fn subscribes_to(&self) -> &[EventKind] {
        &[EventKind::RootRequested, EventKind::ProjectsGrouped]
    }
    async fn on_event(
        &self,
        event: &Event,
        ctx: &Ctx<'_>,
        _stage: &crate::bus::Stage,
    ) -> Result<Vec<Event>> {
        if matches!(event, Event::RootRequested) {
            self.start_prefetch(ctx);
            return Ok(vec![]);
        }
        let Event::ProjectsGrouped {
            projects,
            worktree_paths,
            project_remotes,
            ..
        } = event
        else {
            return Ok(vec![]);
        };
        let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
        let mut notes = Vec::new();
        // Docker is probed only when the authorized scope includes it.
        // A disabled or excluded `docker-desktop` detector means the
        // user did not authorize asking the daemon about their images,
        // volumes and containers -- and asking cost ~0.9 s per
        // observation, forever, when the daemon is installed and
        // stopped (the 2026-09-22 re-review's CE6). A mocked facts file
        // is a test/CLI input, not a probe, so it is still honoured.
        let asked = ctx.docker_in_scope || ctx.docker_facts.is_some();
        let started = std::time::Instant::now();
        let facts = if asked {
            self.take_prefetched().unwrap_or_else(|| {
                crate::docker::load_cached(
                    ctx.docker_facts.as_deref(),
                    ctx.store_dir.as_deref(),
                    force_docker_refresh(ctx.enrich, crate::docker::force_refresh()),
                )
            })
        } else {
            crate::docker::DockerFacts {
                unavailable: Some(
                    "docker: not in scope this invocation (the docker-desktop detector is \
                     disabled or excluded), so the daemon was not asked"
                        .to_string(),
                ),
                ..Default::default()
            }
        };
        if trace {
            eprintln!(
                "[xtrace] docker consumer: load facts asked={asked} projects={} worktrees={} elapsed={:?}",
                projects.len(),
                worktree_paths.len(),
                started.elapsed()
            );
        }
        if let Some(reason) = &facts.unavailable {
            notes.push(reason.clone());
        }
        // compose project name -> worktrees that declare it, so a
        // `com.docker.compose.project` label joins by explicit evidence.
        let started = std::time::Instant::now();
        let mut compose_index: HashMap<String, Vec<String>> = HashMap::new();
        let mut compose_names = 0usize;
        for (path, worktree_id) in worktree_paths.iter() {
            for name in crate::compose::discover_candidate_names(path) {
                compose_names += 1;
                compose_index
                    .entry(name)
                    .or_default()
                    .push(worktree_id.clone());
            }
        }
        if trace {
            eprintln!(
                "[xtrace] docker consumer: compose candidates worktrees={} names={} elapsed={:?}",
                worktree_paths.len(),
                compose_names,
                started.elapsed()
            );
        }
        let started = std::time::Instant::now();
        let mut join = crate::report::join_docker_facts(
            &facts,
            projects,
            worktree_paths,
            project_remotes,
            &compose_index,
        );
        let capture_age = capture_age_note(
            facts.captured_at,
            ctx.observed_at,
            ctx.docker_facts.is_some(),
        );
        annotate_capture_age(&mut join, &capture_age);
        if trace {
            eprintln!(
                "[xtrace] docker consumer: join facts={} projects={} worktrees={} elapsed={:?}",
                facts.images.len() + facts.build_cache.len() + facts.volumes.len(),
                projects.len(),
                worktree_paths.len(),
                started.elapsed()
            );
        }
        Ok(vec![Event::DockerJoined {
            rows_by_worktree: Arc::new(join.rows_by_worktree),
            unowned: Arc::new(join.unowned),
            attributed_bytes: join.attributed_bytes,
            unowned_bytes: join.unowned_bytes,
            notes,
            facts: asked.then(|| Arc::new(facts)),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{Consumer, Ctx, Event, EventBus, EventKind};
    use crate::report::Reconciliation;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender};
    use std::time::Duration;

    #[test]
    fn force_refresh_requires_an_explicit_docker_refresh_request() {
        assert!(!force_docker_refresh(false, false));
        assert!(!force_docker_refresh(false, true));
        assert!(!force_docker_refresh(true, false));
        assert!(force_docker_refresh(true, true));
    }

    #[test]
    fn capture_age_notes_keep_unknown_and_future_times_explicit() {
        assert_eq!(
            capture_age_note(Some(997), 1_000, false),
            "Docker facts: 3 seconds before this observation (daemon capture; refresh window 5 minutes)"
        );
        assert_eq!(
            capture_age_note(Some(1_002), 1_000, false),
            "Docker facts: capture is 2 seconds after this observation timestamp (daemon capture; refresh window 5 minutes)"
        );
        assert_eq!(
            capture_age_note(None, 1_000, false),
            "Docker facts: capture time not recorded (age unknown; daemon capture; refresh window 5 minutes)"
        );
        assert_eq!(
            capture_age_note(Some(997), 1_000, true),
            "Docker facts: 3 seconds before this observation (facts file; automatic refresh does not apply)"
        );
        let mut note = Some("compose_ambiguous=2".to_string());
        append_note(&mut note, "Docker facts: age");
        assert_eq!(
            note.as_deref(),
            Some("compose_ambiguous=2; Docker facts: age")
        );
    }

    #[test]
    fn facts_file_age_is_attached_only_to_docker_rows() {
        let docker_row: crate::report::ArtifactRow = serde_json::from_value(serde_json::json!({
            "kind": "DockerImage",
            "path": "/image",
            "bytes": 12,
            "growth_bytes": null,
            "regrowth_count": 0,
            "observed_at": 1_000,
            "confidence": "High",
            "source": {"tool": "docker.compose_label"}
        }))
        .unwrap();
        let other_row: crate::report::ArtifactRow = serde_json::from_value(serde_json::json!({
            "kind": "Git",
            "path": "/repo/.git",
            "bytes": 4,
            "growth_bytes": null,
            "regrowth_count": 0,
            "observed_at": 1_000,
            "confidence": "High",
            "source": {"tool": "git"}
        }))
        .unwrap();
        let unowned: crate::report::UnownedRow = serde_json::from_value(serde_json::json!({
            "path_or_object": "sha256:unjoined",
            "bytes": 7,
            "reason": "DockerNoJoin",
            "note": "source remote unmatched"
        }))
        .unwrap();
        let mut join = crate::report::DockerJoinResult {
            rows_by_worktree: HashMap::from([("wt".to_string(), vec![docker_row, other_row])]),
            unowned: vec![unowned],
            attributed_bytes: 0,
            unowned_bytes: 0,
        };

        let note = capture_age_note(Some(997), 1_000, true);
        annotate_capture_age(&mut join, &note);

        let rows = &join.rows_by_worktree["wt"];
        assert!(
            rows[0]
                .note
                .as_deref()
                .unwrap()
                .contains("facts file; automatic refresh does not apply")
        );
        assert!(
            rows[1].note.is_none(),
            "non-Docker rows must stay untouched"
        );
        assert_eq!(
            join.unowned[0].note.as_deref(),
            Some(
                "source remote unmatched; Docker facts: 3 seconds before this observation (facts file; automatic refresh does not apply)"
            )
        );
    }

    fn context<'a>(source: &'a crate::fs_events::UnsupportedPlatformSource) -> Ctx<'a> {
        crate::bus::ctx_for(
            Path::new("/tmp/docker-prefetch-test"),
            None,
            false,
            None,
            None,
            true,
            false,
            false,
            false,
            source,
        )
    }

    fn projects_grouped() -> Event {
        Event::ProjectsGrouped {
            projects: Arc::new(Vec::new()),
            rewalked: None,
            worktree_paths: Arc::new(Vec::new()),
            project_remotes: Arc::new(HashMap::new()),
            worktree_remotes: Arc::new(HashMap::new()),
            unowned: Arc::new(Vec::new()),
            dirs: Arc::new(Vec::new()),
            files: Arc::new(Vec::new()),
            reconciliation: Reconciliation {
                unique_estimate: None,
                attributed: 0,
                unowned: 0,
                walked_total: 0,
                du_total: None,
                docker_attributed: 0,
                docker_unowned: 0,
            },
            notes: Vec::new(),
            unconfirmed_worktree_ids: Arc::new(Vec::new()),
        }
    }

    struct GatedWalk {
        worker_started: Receiver<()>,
        release_worker: Sender<()>,
        worker_finished: Arc<AtomicBool>,
    }

    #[async_trait::async_trait(?Send)]
    impl Consumer for GatedWalk {
        fn name(&self) -> &str {
            "walk-gate"
        }

        fn subscribes_to(&self) -> &[EventKind] {
            &[EventKind::RootRequested]
        }

        async fn on_event(
            &self,
            _event: &Event,
            _ctx: &Ctx<'_>,
            _stage: &crate::bus::Stage,
        ) -> Result<Vec<Event>> {
            if let Err(error) = self.worker_started.recv_timeout(Duration::from_secs(3)) {
                // Let a late-starting worker exit even when the ordering
                // assertion fails, so the regression test cannot hang in
                // DockerConsumer::drop while reporting the failure.
                let _ = self.release_worker.send(());
                anyhow::bail!("Docker prefetch did not start before walk: {error}");
            }
            assert!(!self.worker_finished.load(Ordering::SeqCst));
            // Model synchronous filesystem work while Docker remains in
            // flight. A serial dispatcher cannot pass the start handshake.
            std::thread::sleep(Duration::from_millis(40));
            assert!(!self.worker_finished.load(Ordering::SeqCst));
            self.release_worker
                .send(())
                .map_err(|e| anyhow::anyhow!("could not release Docker worker: {e}"))?;
            Ok(vec![projects_grouped()])
        }
    }

    #[test]
    fn registered_docker_prefetch_overlaps_the_blocking_walk() {
        let builtins = EventBus::with_builtins();
        let names = builtins.consumer_names();
        let docker = names.iter().position(|name| *name == "docker").unwrap();
        let walk = names.iter().position(|name| *name == "walk").unwrap();
        assert!(
            docker < walk,
            "Docker must start before the blocking walk: {names:?}"
        );

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let worker_finished = Arc::new(AtomicBool::new(false));
        let finished_by_worker = worker_finished.clone();
        let loader: TestLoader = Arc::new(move |facts_path, store_dir, fresh| {
            crate::work_counters::record_spawn();
            assert!(facts_path.is_none());
            assert!(store_dir.is_none());
            assert!(!fresh, "the no-enrich setting must pass through unchanged");
            started_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            finished_by_worker.store(true, Ordering::SeqCst);
            crate::docker::DockerFacts::default()
        });

        let source = crate::fs_events::UnsupportedPlatformSource;
        let ctx = context(&source);
        let mut bus = EventBus::new_for_test();
        bus.register_for_test(Box::new(DockerConsumer::with_loader(loader)))
            .unwrap();
        bus.register_for_test(Box::new(GatedWalk {
            worker_started: started_rx,
            release_worker: release_tx,
            worker_finished: worker_finished.clone(),
        }))
        .unwrap();
        let (events, counted) = crate::work_counters::measured(|| {
            bus.run_blocking(Event::RootRequested, &ctx).unwrap()
        });
        assert!(worker_finished.load(Ordering::SeqCst));
        assert_eq!(counted.subprocess_spawns, 1);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::DockerJoined { .. }))
        );
    }

    struct FailingWalk {
        worker_started: Receiver<()>,
    }

    #[async_trait::async_trait(?Send)]
    impl Consumer for FailingWalk {
        fn name(&self) -> &str {
            "failing-walk"
        }

        fn subscribes_to(&self) -> &[EventKind] {
            &[EventKind::RootRequested]
        }

        async fn on_event(
            &self,
            _event: &Event,
            _ctx: &Ctx<'_>,
            _stage: &crate::bus::Stage,
        ) -> Result<Vec<Event>> {
            self.worker_started
                .recv_timeout(Duration::from_secs(3))
                .map_err(|e| {
                    anyhow::anyhow!("Docker prefetch did not start before failure: {e}")
                })?;
            anyhow::bail!("synthetic walk failure")
        }
    }

    #[test]
    fn dropping_a_failed_pipeline_joins_its_docker_worker() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let worker_finished = Arc::new(AtomicBool::new(false));
        let finished_by_worker = worker_finished.clone();
        let loader: TestLoader = Arc::new(move |_, _, _| {
            started_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            finished_by_worker.store(true, Ordering::SeqCst);
            crate::docker::DockerFacts::default()
        });
        let source = crate::fs_events::UnsupportedPlatformSource;
        let ctx = context(&source);
        let mut bus = EventBus::new_for_test();
        bus.register_for_test(Box::new(DockerConsumer::with_loader(loader)))
            .unwrap();
        bus.register_for_test(Box::new(FailingWalk {
            worker_started: started_rx,
        }))
        .unwrap();
        assert!(bus.run_blocking(Event::RootRequested, &ctx).is_err());

        let (release_after_drop_started_tx, release_after_drop_started_rx) =
            std::sync::mpsc::channel();
        let releaser = std::thread::spawn(move || {
            release_after_drop_started_rx.recv().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            release_tx.send(()).unwrap();
        });
        release_after_drop_started_tx.send(()).unwrap();
        drop(bus);
        assert!(
            worker_finished.load(Ordering::SeqCst),
            "the failed pipeline must not detach its cache-writing worker"
        );
        releaser.join().unwrap();
    }

    #[test]
    fn excluded_docker_does_not_start_a_prefetch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_by_loader = calls.clone();
        let loader: TestLoader = Arc::new(move |_, _, _| {
            calls_by_loader.fetch_add(1, Ordering::SeqCst);
            crate::docker::DockerFacts::default()
        });
        let source = crate::fs_events::UnsupportedPlatformSource;
        let mut ctx = context(&source);
        ctx.docker_in_scope = false;
        let mut bus = EventBus::new_for_test();
        bus.register_for_test(Box::new(DockerConsumer::with_loader(loader)))
            .unwrap();
        bus.run_blocking(Event::RootRequested, &ctx).unwrap();
        let events = bus.run_blocking(projects_grouped(), &ctx).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(events.iter().any(|event| matches!(
            event,
            Event::DockerJoined { facts: None, notes, .. }
                if notes.iter().any(|note| note.contains("not in scope"))
        )));
    }

    #[test]
    fn explicit_facts_fixture_is_still_loaded_without_enrichment() {
        let tmp = tempfile::tempdir().unwrap();
        let facts_path = tmp.path().join("docker.json");
        std::fs::write(
            &facts_path,
            r#"{"Images":[{"ID":"sha256:fixture","Repository":"fixture","Tag":"latest","Size":"12"}]}"#,
        )
        .unwrap();
        let source = crate::fs_events::UnsupportedPlatformSource;
        let mut ctx = context(&source);
        ctx.docker_in_scope = false;
        ctx.docker_facts = Some(facts_path);
        ctx.enrich = false;
        let mut bus = EventBus::new_for_test();
        bus.register_for_test(Box::new(DockerConsumer::default()))
            .unwrap();
        bus.run_blocking(Event::RootRequested, &ctx).unwrap();
        let events = bus.run_blocking(projects_grouped(), &ctx).unwrap();
        let facts = events.iter().find_map(|event| match event {
            Event::DockerJoined { facts, .. } => facts.clone(),
            _ => None,
        });
        let facts = facts.expect("explicit fixture facts remain authorized");
        assert_eq!(facts.images.len(), 1);
        assert_eq!(facts.images[0].id, "sha256:fixture");
        assert_eq!(facts.images[0].unique_bytes, 12);
    }
}
