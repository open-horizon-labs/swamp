//! First-class external/shared storage units (#43): Cargo registry,
//! rustup toolchains, Homebrew, and every future detector-resolved
//! location that is not a project/worktree walk boundary. An external
//! unit's identity is independent of any project or worktree -- it
//! exists whether or not any project happens to reference it -- and it
//! keeps size/growth/presence/regrowth history under the same
//! current+reverse-delta retention semantics as everything else in the
//! growth store (see `growth::observe_and_annotate_external`, a new key
//! family in the existing store, never a second store).
//!
//! Consumer associations (project/tool <-> external unit) are evidence
//! a *separate* system supplies (#57's job for real ecosystem-sourced
//! evidence: manifests, lockfiles, Docker joins). This module's own
//! association sidecar is read here (`load_consumers`); its writers
//! (`associate_consumer`/`dissociate_consumer`) are test-only, because
//! nothing in the product declares a consumer by hand. They exist to
//! prove the required shape (zero/one/many consumers, counted once,
//! association changes never duplicate the unit or reset its history)
//! without inventing a competing discovery pipeline. Until 2026-09-22
//! they were `pub` with no production caller, which is the dead-API shape
//! `no_dead_public_evidence_api` rejects once "called" means reachable.
//!
//! Action boundary (#43's acceptance criterion 5): `actions::unit_from_external`
//! and `actions::propose_external` let a plan *name* an external unit;
//! `actions::execute*` refuses every one of them unconditionally on
//! `PlanUnit::external_category`. Nothing in this module or in the
//! detector registry ever authorizes removing external storage.

use crate::locations::{Provenance, StorageCategory};
use crate::scope::EffectiveScope;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A declared consumer of an external unit: which project/tool refers to
/// it, and why. Evidence-free in this chunk (see module docs); `label`
/// is free text (a project id, a tool name, or a human note) rather than
/// a typed reference, since #57 owns the real evidence-sourced version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalConsumer {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One measured external storage unit: identity independent of any
/// project/worktree, size/growth/regrowth from the growth store, zero or
/// more declared consumers, never duplicated or reset by an association
/// change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalUnit {
    pub detector_id: String,
    pub detector_name: String,
    pub category: StorageCategory,
    pub provenance: Provenance,
    /// Canonical, absolute path this unit measures as one whole-directory
    /// unit (never walked for project/worktree structure -- an external
    /// unit is opaque from this chunk's point of view).
    pub path: PathBuf,
    pub bytes: u64,
    /// Newest recorded modification among this unit's measured children,
    /// from the folded walk that measured it. The Activity fact
    /// (`docs/usage.md`'s "modification age of the measured directory
    /// only") is built from this; before 2026-09-22 the doc claimed the
    /// fact and the struct had no field to carry it.
    #[serde(default)]
    pub mtime_max: u64,
    /// Whether the unit contains hardlinked files (conservative default
    /// `true` mirrors artifact rows -- see `ArtifactRow::hardlinked`).
    pub hardlinked: bool,
    pub growth_bytes: Option<i64>,
    pub regrowth_count: u32,
    pub observed_at: u64,
    pub consumers: Vec<ExternalConsumer>,
    /// Set when this unit's coverage this pass was incomplete (access
    /// lost, not measured) rather than absent -- distinct from simply
    /// not appearing in the list at all (a genuinely gone unit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Decision evidence (#53): activity/consumer/current-use facts for
    /// this unit -- see `crate::evidence`. Populated from already-
    /// collected facts (this unit's own `consumers`, its `growth_bytes`
    /// history); never a new per-unit scan on every report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<crate::evidence::Evidence>,
    /// Bytes of project worktrees that live inside this unit and are
    /// counted under their projects instead: excluded from `bytes`, and
    /// kept here as a number so a view can add the parts back up without
    /// parsing text (#185). Zero when no worktree is inside.
    #[serde(default)]
    pub bytes_counted_elsewhere: u64,
    /// How many worktrees `bytes_counted_elsewhere` stands for.
    #[serde(default)]
    pub overlap_count: u32,
    /// When this unit was last run or opened, and where that fact came
    /// from (#176). `no record` when nothing reliable says.
    #[serde(default)]
    pub last_used: crate::last_used::LastUsed,
    /// Depth-2 drilldown (#178): the top child folders and a remainder
    /// row that make the rows sum to the walk's total. Empty when the
    /// unit is not drilled into.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<crate::drilldown::UnitChild>,
}

impl ExternalUnit {
    /// The human sentence for the overlap fields, rendered from them:
    /// nothing is stored as text.
    pub fn overlap_note(&self) -> Option<String> {
        (self.overlap_count > 0).then(|| {
            format!(
                "{} inside is counted under projects ({} worktree{}), not in this total",
                crate::render::human_bytes_pub(self.bytes_counted_elsewhere),
                self.overlap_count,
                if self.overlap_count == 1 { "" } else { "s" }
            )
        })
    }

    /// The one bracketed note a row shows: the coverage note (`note`)
    /// and the overlap sentence, each only when it applies.
    pub fn display_note(&self) -> Option<String> {
        match (self.note.as_deref(), self.overlap_note()) {
            (Some(n), Some(o)) => Some(format!("{n}; {o}")),
            (Some(n), None) => Some(n.to_string()),
            (None, Some(o)) => Some(o),
            (None, None) => None,
        }
    }
}

/// Turns this unit's existing declared-consumer sidecar
/// (`associate_consumer`/`dissociate_consumer`) into the shared
/// decision-evidence contract (#53/#57), so a manually-declared
/// consumer and a lockfile/manifest-sourced one (#57's real ecosystem
/// evidence, once wired) render identically. Reuses data already
/// loaded for `ExternalUnit::consumers`; never a new scan.
fn consumers_evidence(consumers: &[ExternalConsumer]) -> Vec<crate::evidence::Evidence> {
    consumers
        .iter()
        .map(|c| {
            let ev = crate::evidence::Evidence::known(
                crate::evidence::FactKind::Consumer,
                crate::evidence::FactSubtype::DeclaredConsumer,
                crate::evidence::FactValue::Text(c.label.clone()),
                crate::evidence::EvidenceSource::ConfigDeclaration {
                    path: "swamp consumer association".to_string(),
                },
                crate::entities::now(),
            );
            match &c.note {
                Some(n) => ev.with_note(n.clone()),
                None => ev,
            }
        })
        .collect()
}

/// The stable identity string used both for the growth-store row key and
/// the consumer-association sidecar key: `(detector_id, category,
/// device, canonical path)`, exactly the growth store's own key family.
pub fn unit_key(detector_id: &str, category: StorageCategory, device: u64, path: &Path) -> String {
    crate::growth::external_row_key(
        detector_id,
        category_str(category),
        device,
        &path.display().to_string(),
    )
}

/// The stable label for a [`StorageCategory`], used both for the
/// external row key and (item 3, `render.rs`) for rendering: no user
/// surface prints `{:?}` on this enum.
/// The category's label for text output (`swamp scope`).
pub fn category_label(c: StorageCategory) -> &'static str {
    category_str(c)
}

pub(crate) fn category_str(c: StorageCategory) -> &'static str {
    match c {
        StorageCategory::Installation => "installation",
        StorageCategory::Downloads => "downloads",
        StorageCategory::Cache => "cache",
        StorageCategory::LocalState => "local-state",
        StorageCategory::Environments => "environments",
        StorageCategory::BuildOutput => "build-output",
        StorageCategory::Models => "models",
        StorageCategory::Unclassified => "unclassified",
    }
}

fn device_of(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        use crate::fs_gate::MetadataExt;
        crate::fs_gate::metadata_following(path)
            .map(|m| m.dev())
            .unwrap_or(0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0
    }
}

/// Discovers and measures every external unit in `scope`: one whole-
/// directory measurement per `Resolved`, currently-present, non-scan-root
/// detector location. The `builtin-defaults` detector is skipped -- it
/// proposes ordinary scan roots (`~/src`, ...), not external storage --
/// so nothing here duplicates `report_scope`'s walked/unowned totals.
///
/// When `swamp_dir` is `Some`, this observes (persists to the growth
/// store) when `observe` is true, or reads existing history read-only
/// otherwise. `swamp_dir: None` returns units with `growth_bytes: None`
/// and `regrowth_count: 0` (no store to consult), matching the
/// artifact-row read-only contract elsewhere.
/// One candidate detector location, before it is known whether it can be
/// measured this pass.
struct Candidate {
    detector_id: String,
    detector_name: String,
    category: StorageCategory,
    provenance: Provenance,
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct DeviceFsUnitCoverage {
    unit_key: String,
    mounts: Vec<PathBuf>,
}

/// Stable namespace in the existing overlap marker table. The length
/// prefix makes unit keys unambiguous even though both unit keys and
/// absolute paths contain punctuation.
pub(crate) fn mount_marker_prefix(unit_key: &str) -> String {
    format!("devicefs-mount:v1:{}:{unit_key}:", unit_key.len())
}

/// The latest mount-coverage transition for this external unit, if any.
/// Growth history before this timestamp has a different filesystem
/// boundary and is not comparable to the current measurement.
pub(crate) fn mount_coverage_floor(marks: &HashMap<String, (u32, u64)>, unit_key: &str) -> u64 {
    let prefix = mount_marker_prefix(unit_key);
    marks
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|(_, (_, changed_at))| *changed_at)
        .max()
        .unwrap_or(0)
}

fn devicefs_unit_coverage(
    automatic_scope: bool,
    candidates: &[(Candidate, PathBuf)],
    mounts: &[crate::fs_gate::fs_space::MountPoint],
) -> Vec<DeviceFsUnitCoverage> {
    candidates
        .iter()
        .map(|(candidate, root)| {
            let unit_key = unit_key(
                &candidate.detector_id,
                candidate.category,
                device_of(root),
                root,
            );
            let mut nested: Vec<PathBuf> = mounts
                .iter()
                .filter(|mount| automatic_scope && mount.fs_type.eq_ignore_ascii_case("devicefs"))
                .map(|mount| mount.path.clone())
                .filter(|path| path != root && path.starts_with(root))
                .collect();
            nested.sort();
            nested.dedup();
            DeviceFsUnitCoverage {
                unit_key,
                mounts: nested,
            }
        })
        .collect()
}

/// Mark DeviceFS exclusion on/off for each external unit in this pass.
/// Entries remain after unmount or an explicit-scope switch, so a 1→0
/// transition invalidates comparisons across that measurement boundary.
fn update_devicefs_mount_marks(
    marks: &mut HashMap<String, (u32, u64)>,
    units: &[DeviceFsUnitCoverage],
    observed_at: u64,
) {
    for unit in units {
        let prefix = mount_marker_prefix(&unit.unit_key);
        let current: HashSet<String> = unit
            .mounts
            .iter()
            .map(|path| format!("{prefix}{}", path.display()))
            .collect();

        let previous: Vec<String> = marks
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        for key in previous {
            if current.contains(&key) {
                continue;
            }
            if let Some((present, changed_at)) = marks.get_mut(&key)
                && *present != 0
            {
                *present = 0;
                *changed_at = observed_at;
            }
        }

        for key in current {
            match marks.get_mut(&key) {
                Some((present, _)) if *present == 1 => {}
                Some((present, changed_at)) => {
                    *present = 1;
                    *changed_at = observed_at;
                }
                None => {
                    marks.insert(key, (1, observed_at));
                }
            }
        }
    }
}

fn append_coverage_note(existing: &mut Option<String>, addition: impl AsRef<str>) {
    let addition = addition.as_ref();
    *existing = Some(match existing.take() {
        Some(note) if !note.is_empty() => format!("{note}; {addition}"),
        _ => addition.to_string(),
    });
}

fn devicefs_unit_notes(
    units: &[DeviceFsUnitCoverage],
    marks: &HashMap<String, (u32, u64)>,
    observed_at: u64,
    since_secs: u64,
) -> (HashMap<String, Vec<String>>, Vec<String>) {
    let mut by_unit: HashMap<String, Vec<String>> = HashMap::new();
    let mut report_notes = Vec::new();
    for unit in units {
        for path in &unit.mounts {
            let note = format!(
                "not measured: virtual mounted filesystem at {}; outside this host-storage measurement",
                path.display()
            );
            by_unit
                .entry(unit.unit_key.clone())
                .or_default()
                .push(note.clone());
            report_notes.push(note);
        }
        let prefix = mount_marker_prefix(&unit.unit_key);
        let transition_note = marks
            .iter()
            .filter(|(key, (present, changed_at))| {
                key.starts_with(&prefix)
                    && *changed_at > 0
                    && changed_at.saturating_add(since_secs) > observed_at
                    && (*present == 0 || *present == 1)
            })
            .map(|(key, _)| {
                let path = key.strip_prefix(&prefix).unwrap_or(key);
                let state = marks.get(key).map(|(present, _)| *present).unwrap_or(0);
                format!(
                    "mounted filesystem coverage changed at {path} ({}); growth is not compared across that change",
                    if state == 1 { "excluded from host measurement" } else { "no longer excluded from this measurement" }
                )
            });
        for note in transition_note {
            by_unit
                .entry(unit.unit_key.clone())
                .or_default()
                .push(note.clone());
            report_notes.push(note);
        }
    }
    (by_unit, report_notes)
}

/// A candidate that was actually measured this pass, keyed by its
/// growth-store row key, holding what `discover_and_measure` needs to
/// build the final `ExternalUnit` after growth annotation.
struct MeasuredUnit {
    detector_id: String,
    detector_name: String,
    category: StorageCategory,
    provenance: Provenance,
    path: PathBuf,
    bytes: u64,
    hardlinked: bool,
    mtime_max: u64,
    /// Project worktrees that live inside this unit and were subtracted
    /// from it: their count and the bytes the project rows report for
    /// them.
    overlap: Option<(usize, u64)>,
    /// The last-use sources this unit's detector declared for it.
    last_use_sources: Vec<crate::locations::LastUseSource>,
    /// The depth-2 drilldown taken (or replayed) this pass.
    children: Vec<crate::drilldown::UnitChild>,
    /// When the sealed volumes replayed inside it were walked.
    sealed_walked_at: Option<u64>,
}

/// Inputs fixed on the report thread before an independent external-unit
/// measurement is handed to a worker. Candidate reduction remains ordered.
struct ParallelExternalMeasure {
    idx: usize,
    path: PathBuf,
    exclusions: Vec<PathBuf>,
    want_children: bool,
    reuse_ok: bool,
    size_hint: u64,
}

struct ParallelExternalResult {
    path: PathBuf,
    observation: crate::folded_measurement::UnitObservation,
    child_dirs: Option<Vec<crate::report::DirRollup>>,
    elapsed: std::time::Duration,
}

fn external_measure_worker_limit() -> usize {
    // The two-worker experiment did not show a reliable end-to-end gain;
    // retain the opt-in for controlled diagnostics, but default to the
    // original serial behavior to avoid competing with the walk pool.
    match std::env::var("SWAMP_EXTERNAL_MEASURE_WORKERS") {
        Ok(value) => value.parse::<usize>().unwrap_or(1).clamp(1, 2),
        Err(std::env::VarError::NotPresent) => 1,
        Err(std::env::VarError::NotUnicode(_)) => 1,
    }
}

fn external_measure_trace_enabled() -> bool {
    std::env::var("SWAMP_EXTERNAL_MEASURE_TRACE")
        .is_ok_and(|value| value != "0" && !value.is_empty())
}

fn select_independent_external_jobs(
    mut eligible: Vec<ParallelExternalMeasure>,
) -> Vec<ParallelExternalMeasure> {
    eligible.sort_by_key(|job| std::cmp::Reverse(job.size_hint));
    let mut selected: Vec<ParallelExternalMeasure> = Vec::new();
    for job in eligible {
        if selected
            .iter()
            .any(|chosen| job.path.starts_with(&chosen.path) || chosen.path.starts_with(&job.path))
        {
            continue;
        }
        selected.push(job);
    }
    selected
}

fn measure_independent_external_jobs(
    jobs: Vec<ParallelExternalMeasure>,
    worker_limit: usize,
    store: Option<&Path>,
    observed_at: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> (HashMap<usize, ParallelExternalResult>, usize) {
    let started = std::time::Instant::now();
    let queue = std::sync::Mutex::new(std::collections::VecDeque::from(jobs));
    let results = std::sync::Mutex::new(HashMap::new());
    // A measured external pass owns its counter sink on this thread. Scoped
    // workers must inherit it so traversal work remains attributable to the
    // caller, just like the directory-walk pool.
    let counters = crate::work_counters::current();
    std::thread::scope(|scope| {
        let worker_count = worker_limit.min(
            queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
        );
        let handles: Vec<_> = (0..worker_count)
            .map(|_| {
                let queue = &queue;
                let results = &results;
                let counters = counters.clone();
                scope.spawn(move || {
                    crate::work_counters::install(counters);
                    loop {
                        let job = queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .pop_front();
                        let Some(job) = job else { break };
                        let measurement_started = std::time::Instant::now();
                        let path = job.path.clone();
                        let (observation, child_dirs) = if job.want_children {
                            crate::folded_measurement::observe_unit_with_dirs(
                                store,
                                &job.path,
                                &job.exclusions,
                                observed_at,
                                coverage,
                                job.reuse_ok,
                                true,
                            )
                        } else {
                            (
                                crate::folded_measurement::observe_unit(
                                    store,
                                    &job.path,
                                    &job.exclusions,
                                    observed_at,
                                    coverage,
                                ),
                                None,
                            )
                        };
                        let result = ParallelExternalResult {
                            path,
                            observation,
                            child_dirs,
                            elapsed: measurement_started.elapsed(),
                        };
                        results
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(job.idx, result);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("external measurement worker panicked");
        }
    });
    let results = results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let worker_count = worker_limit.min(results.len());
    if std::env::var("SWAMP_TRACE").is_ok_and(|value| value != "0" && !value.is_empty())
        || external_measure_trace_enabled()
    {
        eprintln!(
            "[xtrace] independent external measurements workers={} elapsed={:?}; per-unit work-counter deltas include concurrent work; stage wall time is exact",
            worker_count,
            started.elapsed()
        );
        for result in results.values() {
            eprintln!(
                "[xtrace] parallel unit {} elapsed={:?}",
                result.path.display(),
                result.elapsed
            );
        }
    }
    (results, worker_count)
}

#[cfg(test)]
mod parallel_measurement_tests {
    use super::*;
    use crate::folded_measurement::{FoldedUnit, UnitObservation};

    fn job(idx: usize, path: PathBuf, size_hint: u64) -> ParallelExternalMeasure {
        ParallelExternalMeasure {
            idx,
            path,
            exclusions: Vec::new(),
            want_children: true,
            reuse_ok: true,
            size_hint,
        }
    }

    fn folded(observation: &UnitObservation) -> &FoldedUnit {
        let UnitObservation::Unit(unit) = observation else {
            panic!("fixture path was not measurable")
        };
        unit
    }

    fn dir_facts(dirs: Option<&[crate::report::DirRollup]>) -> Vec<(String, u64, u64, u32, bool)> {
        let mut facts: Vec<_> = dirs
            .unwrap_or_default()
            .iter()
            .map(|dir| {
                (
                    dir.rel_path.clone(),
                    dir.allocated_total,
                    dir.own_allocated,
                    dir.entry_count,
                    dir.complete,
                )
            })
            .collect();
        facts.sort();
        facts
    }

    #[test]
    fn selector_queues_all_disjoint_jobs_and_skips_overlapping_roots() {
        let root = PathBuf::from("/fixture/large");
        let jobs = vec![
            job(1, root.clone(), 100),
            job(2, root.join("nested"), 90),
            job(3, PathBuf::from("/fixture/caches"), 80),
            job(4, PathBuf::from("/fixture/homebrew"), 70),
        ];
        let selected = select_independent_external_jobs(jobs);
        assert_eq!(selected.len(), 3, "all non-overlapping jobs stay queued");
        assert_eq!(selected[0].idx, 1);
        assert_eq!(selected[1].idx, 3);
        assert_eq!(selected[2].idx, 4);
        assert!(selected.iter().all(|left| selected.iter().all(|right| {
            left.idx == right.idx
                || (!left.path.starts_with(&right.path) && !right.path.starts_with(&left.path))
        })));
    }

    #[test]
    fn two_workers_match_serial_fold_children_hardlinks_and_persisted_rows() {
        let roots = tempfile::tempdir().unwrap();
        let serial_store = tempfile::tempdir().unwrap();
        let parallel_store = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for i in 0..3 {
            let root = roots.path().join(format!("unit-{i}"));
            std::fs::create_dir_all(root.join("nested")).unwrap();
            std::fs::write(root.join("nested/payload"), vec![b'a' + i as u8; 4096]).unwrap();
            paths.push(root);
        }
        std::fs::hard_link(
            paths[0].join("nested/payload"),
            paths[1].join("nested/shared"),
        )
        .unwrap();
        crate::fs_gate::settle::settle();

        let coverage = crate::fs_events::EventCoverage::untrusted();
        let (oracle, serial_work) = crate::work_counters::measured(|| {
            let mut oracle = HashMap::new();
            for (idx, path) in paths.iter().enumerate() {
                let (observation, dirs) = crate::folded_measurement::observe_unit_with_dirs(
                    Some(serial_store.path()),
                    path,
                    &[],
                    1_000,
                    &coverage,
                    true,
                    true,
                );
                oracle.insert(idx, (observation, dirs));
            }
            oracle
        });

        let jobs = paths
            .iter()
            .enumerate()
            .map(|(idx, path)| job(idx, path.clone(), 100 - idx as u64))
            .collect();
        let ((parallel, workers), parallel_work) = crate::work_counters::measured(|| {
            measure_independent_external_jobs(
                jobs,
                2,
                Some(parallel_store.path()),
                1_000,
                &coverage,
            )
        });
        assert_eq!(workers, 2, "three queued roots are serviced by two workers");
        assert_eq!(parallel.len(), paths.len(), "the queue drains every job");
        assert!(serial_work.dirs_listed > 0 && serial_work.files_statted > 0);
        assert_eq!(parallel_work.dirs_listed, serial_work.dirs_listed);
        assert_eq!(parallel_work.files_statted, serial_work.files_statted);

        for (idx, path) in paths.iter().enumerate() {
            let (expected_observation, expected_dirs) = oracle.get(&idx).unwrap();
            let actual = parallel.get(&idx).unwrap();
            let expected = folded(expected_observation);
            let actual_fold = folded(&actual.observation);
            assert_eq!(actual_fold.bytes, expected.bytes);
            assert_eq!(actual_fold.hardlinked, expected.hardlinked);
            assert_eq!(actual_fold.complete, expected.complete);
            assert_eq!(actual_fold.mtime_max, expected.mtime_max);
            assert_eq!(
                dir_facts(actual.child_dirs.as_deref()),
                dir_facts(expected_dirs.as_deref())
            );
            assert_eq!(
                crate::growth::folded_rows_for(serial_store.path(), &path.display().to_string()),
                crate::growth::folded_rows_for(parallel_store.path(), &path.display().to_string())
            );
        }
        assert!(folded(&parallel[&0].observation).hardlinked);
        assert!(folded(&parallel[&1].observation).hardlinked);
    }
}

/// A project worktree the report already measured, handed to the
/// external pass so bytes inside it are counted under its project and
/// not a second time under an external unit that contains it.
#[derive(Debug, Clone)]
pub struct NestedWorktree {
    pub path: PathBuf,
    /// What the project rows report for it (artifact bytes), for the
    /// overlap note only; never subtracted arithmetically.
    pub reported_bytes: u64,
}

/// Every detector-proposed location the *authorized* scope actually lets
/// this pass measure. The `builtin-defaults` detector's own project-root
/// candidate (`~/src`) never reaches here at all: since #R13 item B it
/// carries `RootReason::BuiltinDefault`, not `RootReason::Detector`, so
/// `AuthorizedRoot::detector_id` is `None` for it and the `?` below
/// filters it out -- nothing here can duplicate `report_scope`'s
/// walked/unowned totals for a project root. The *other* candidates the
/// same detector proposes (`~/Library/Caches`, `~/Library/Developer`,
/// the XDG cache root) still carry `RootReason::Detector { detector_id:
/// "builtin-defaults", .. }`, because they are detector locations, not
/// project roots -- and since they are never walked for projects any
/// more either, this is the *only* place they get measured at all, so
/// they must not be filtered out here too.
///
/// Under an explicit-root invocation, detector locations are in scope
/// only inside the roots the user named
/// (`EffectiveScope::authorized_detector_paths_in_explicit_roots`), so
/// `swamp report <some-project> --view external` cannot quietly widen
/// itself back out to the whole configured catalog.
/// Every detector-proposed location this pass may measure, plus the ones
/// it deliberately may not.
///
/// The second half is not bookkeeping. A nested location the user
/// excluded has to be subtracted from *two* places or a one-line config
/// change becomes a storage change (the 2026-09-22 re-review's CE4):
///
/// * from the parent's `nested_exclusions`, or the parent silently
///   absorbs the excluded child's bytes and the growth annotation
///   reports that absorption as real growth; and
/// * from the ownership window, or the child's stored row -- still
///   inside the measured parent's root -- is tombstoned by the owned
///   sweep, and removing the exclusion again scores a regrowth.
fn authorized_candidates(scope: &EffectiveScope) -> (Vec<Candidate>, Vec<PathBuf>) {
    let (roots, unauthorized) = if scope.explicit {
        (
            scope.authorized_detector_paths_in_explicit_roots(),
            scope.unauthorized_detector_paths_in_explicit_roots(),
        )
    } else {
        scope.authorized_roots()
    };
    let out_of_scope: Vec<PathBuf> = unauthorized
        .into_iter()
        .filter(|u| u.out_of_scope)
        .map(|u| {
            // One spelling, so a subtraction written in either form
            // matches what the measurement pass canonicalized.
            crate::fs_gate::canonicalize(&u.path).unwrap_or(u.path)
        })
        .collect();
    let candidates = roots
        .into_iter()
        .filter_map(|root| {
            let detector_id = root.detector_id?;
            Some(Candidate {
                detector_name: root.detector_name.unwrap_or_else(|| detector_id.clone()),
                detector_id,
                category: root.category,
                provenance: root.provenance,
                path: root.path,
            })
        })
        .collect();
    (candidates, out_of_scope)
}

/// Replaces each candidate its detector proposed as a *container*
/// (`Detector::select_children`: Homebrew's Cellar and Caskroom) with the
/// subdirectories the detector selects, so those are measured as units of
/// their own and the container's unselected remainder is never measured
/// as a unit at all.
///
/// The one place a listing happens for this, and it is the observation
/// pass's: the container is listed once through the capped
/// `locations::shallow_list`. Returns the new candidates, the out-of-scope
/// list extended with selected children the user excluded (so a parent
/// that absorbs them subtracts them, as for any other excluded nested
/// location), and the containers that could not be listed completely
/// (unreadable, or cut at the listing cap), which the ownership sweep must
/// leave alone: an unlistable directory is not an empty one, and units
/// stored under it are not gone.
fn expand_containers(
    candidates: Vec<Candidate>,
    mut out_of_scope: Vec<PathBuf>,
    scope: &EffectiveScope,
    detectors: &crate::locations::Registry,
) -> (Vec<Candidate>, Vec<PathBuf>, Vec<PathBuf>) {
    let mut out = Vec::with_capacity(candidates.len());
    let mut partial: Vec<PathBuf> = Vec::new();
    for c in candidates {
        let detector = detectors
            .detectors()
            .iter()
            .find(|d| d.id() == c.detector_id);
        // Asking with no names first is how a detector says whether this
        // path is a container at all, before anything is listed.
        if !detector.is_some_and(|d| d.select_children(&c.path, &[]).is_some()) {
            out.push(c);
            continue;
        }
        let listing = crate::locations::shallow_list(&c.path);
        if listing.truncation.is_truncated() {
            partial.push(crate::fs_gate::canonicalize(&c.path).unwrap_or_else(|_| c.path.clone()));
        }
        let names: Vec<String> = listing
            .iter()
            .filter(|e| e.is_dir)
            .map(|e| e.name.clone())
            .collect();
        let selected = detector
            .and_then(|d| d.select_children(&c.path, &names))
            .unwrap_or_default();
        for name in selected {
            let child = c.path.join(&name);
            if scope.exclusion_for(&child).is_some() {
                out_of_scope.push(crate::fs_gate::canonicalize(&child).unwrap_or(child));
                continue;
            }
            out.push(Candidate {
                detector_id: c.detector_id.clone(),
                detector_name: c.detector_name.clone(),
                category: c.category,
                provenance: c.provenance.clone(),
                path: child,
            });
        }
    }
    (out, out_of_scope, partial)
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "testing")]
pub fn discover_and_measure_with_worktrees(
    scope: &EffectiveScope,
    worktrees: &[NestedWorktree],
    swamp_dir: Option<&Path>,
    observe: bool,
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> Result<Vec<ExternalUnit>> {
    observe_external(
        &crate::report::DiscoveryPass::for_tests(),
        scope,
        worktrees,
        swamp_dir,
        observe,
        false,
        observed_at,
        retention_days,
        since_secs,
        coverage,
    )
    .map(|o| o.units)
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "testing")]
pub fn discover_and_measure(
    scope: &EffectiveScope,
    swamp_dir: Option<&Path>,
    observe: bool,
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> Result<Vec<ExternalUnit>> {
    observe_external(
        &crate::report::DiscoveryPass::for_tests(),
        scope,
        &[],
        swamp_dir,
        observe,
        false,
        observed_at,
        retention_days,
        since_secs,
        coverage,
    )
    .map(|o| o.units)
}

/// Everything one external observation produces: the external units, and
/// the identified interior of every machine-wide build store among them
/// (`crate::build_stores`), in the same pass and under the same
/// ownership.
#[derive(Debug, Default)]
pub struct ExternalObservation {
    pub units: Vec<ExternalUnit>,
    /// Units inside the stores, each with `container_id` naming its
    /// store; a unit's store is the external unit whose `path` its own
    /// path lies under.
    pub interiors: Vec<crate::artifact::NestedArtifact>,
    /// Coverage facts of this pass, for the report's notes: a container
    /// that could not be listed completely, so what it holds was not
    /// measured (not measured, never zero).
    pub notes: Vec<String>,
}

/// [`discover_and_measure`], plus the store interiors. Takes the
/// [`crate::report::DiscoveryPass`] only `report::observe_scope` mints
/// (`.oh/guardrails/discovery-owned-by-report-pipeline.md`); the
/// `testing`-feature `discover_and_measure` is the fixture spelling, and
/// drops the interiors for callers that only want units.
#[allow(clippy::too_many_arguments)]
pub fn observe_external(
    _pass: &crate::report::DiscoveryPass,
    scope: &EffectiveScope,
    worktrees: &[NestedWorktree],
    swamp_dir: Option<&Path>,
    observe: bool,
    // Whether this pass may ask the network (`crate::hub_api`): a
    // scheduled or CLI `observe` that enriches, never the TUI's refresh.
    fetch: bool,
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> Result<ExternalObservation> {
    let trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
    let trace_started = std::time::Instant::now();
    let mut trace_mark = trace_started;
    // Authorized scope only -- never raw detector candidates. The
    // review's `excluded_agent_home_must_not_be_scanned` counterexample
    // was exactly this loop reading `scope.detectors` and so never
    // seeing the user's exclusion
    // (`.oh/guardrails/discovery-consumes-effective-scope.md`).
    let detectors = crate::locations::Registry::with_builtins();
    let (candidates, out_of_scope) = authorized_candidates(scope);
    let (candidates, mut out_of_scope, partial_containers) =
        expand_containers(candidates, out_of_scope, scope, &detectors);
    // Detector display names, captured from the authorized scope before
    // the candidates are consumed: a coverage note for an unreadable
    // unit still needs a human-readable tool name, and must not reach
    // back into detector output for one.
    let detector_names: HashMap<String, String> = candidates
        .iter()
        .map(|c| (c.detector_id.clone(), c.detector_name.clone()))
        .collect();

    // Canonicalize once up front and de-duplicate exact (category,
    // canonical path) repeats -- e.g. a symlinked alias, or two
    // detectors independently conventionalizing the same real directory
    // -- before deciding what to measure, so a duplicate registration
    // never becomes two units for the same bytes (#45's "duplicate
    // registrations without double counting").
    let mut canon_candidates: Vec<(Candidate, PathBuf)> = Vec::new();
    {
        let mut seen: HashSet<(StorageCategory, PathBuf)> = HashSet::new();
        for c in candidates {
            let canonical =
                crate::fs_gate::canonicalize(&c.path).unwrap_or_else(|_| c.path.clone());
            if !seen.insert((c.category, canonical.clone())) {
                continue;
            }
            canon_candidates.push((c, canonical));
        }
    }
    // DeviceFS is a virtual mount (the developer simulator device image
    // view), not host-owned external storage. In automatic host scope it
    // is explicitly unmeasured; never recurse through its potentially
    // enormous metadata tree or report its synthetic byte count as host
    // storage. Explicit command roots retain their ordinary semantics.
    let devicefs_units = devicefs_unit_coverage(
        !scope.explicit,
        &canon_candidates,
        &crate::fs_gate::fs_space::mount_points(),
    );
    out_of_scope.extend(
        devicefs_units
            .iter()
            .flat_map(|unit| unit.mounts.iter().cloned()),
    );
    out_of_scope.sort();
    out_of_scope.dedup();

    // Persist coverage transitions before any growth annotation. If the
    // marker cannot be written, fail this external observation rather
    // than recording a changed filesystem boundary against old history.
    let mut overlap_marks = swamp_dir
        .map(crate::growth::read_overlap_marks)
        .unwrap_or_default();
    let overlap_marks_before_devicefs = overlap_marks.clone();
    update_devicefs_mount_marks(&mut overlap_marks, &devicefs_units, observed_at);
    if let Some(dir) = swamp_dir
        && observe
        && overlap_marks != overlap_marks_before_devicefs
    {
        crate::growth::write_overlap_marks(dir, &overlap_marks)?;
    }
    let (devicefs_notes_by_unit, devicefs_report_notes) =
        devicefs_unit_notes(&devicefs_units, &overlap_marks, observed_at, since_secs);
    if trace {
        eprintln!(
            "[xtrace] candidate authorization/expansion/canonicalization candidates={} elapsed={:?}",
            canon_candidates.len(),
            trace_mark.elapsed()
        );
        trace_mark = std::time::Instant::now();
    }

    // Which of these locations are machine-wide build stores, and which
    // adapter identifies each: the detector's declaration against the
    // adapter's, nothing else (`crate::build_stores::containers_for`).
    let adapters = crate::build_adapters::registry::Registry::with_builtins();
    let store_containers: HashMap<usize, crate::build_adapters::BuildContainer> = {
        let located: Vec<crate::build_stores::Located> = canon_candidates
            .iter()
            .map(|(c, canonical)| crate::build_stores::Located {
                detector_id: &c.detector_id,
                category: c.category,
                path: canonical,
            })
            .collect();
        crate::build_stores::containers_for(&adapters, &detectors, &located)
            .into_iter()
            .collect()
    };
    // The stores' previously identified units, and the question whether
    // this pass's window vouches for each -- asked before measuring, so
    // a store whose units cannot be replayed is measured with its
    // directory rows kept rather than twice.
    let previous_units = match swamp_dir {
        Some(dir) if !store_containers.is_empty() => crate::build_stores::load_units(dir),
        _ => HashMap::new(),
    };
    let stored_cache = crate::build_adapters::ContainerCache::from_containers(
        previous_units.values().cloned().collect(),
    );
    // A store holding only sealed read-only volumes whose stamps hold is
    // vouched for without a window (`sealed_only_unchanged`).
    let mut vouched = coverage.clone();
    for container in store_containers.values() {
        if crate::folded_measurement::sealed_only_unchanged(swamp_dir, &container.path) {
            vouched.merge(crate::fs_events::EventCoverage::trusted(
                container.path.clone(),
                Vec::new(),
                0,
            ));
        }
    }
    let coverage = &vouched;
    let no_rows = crate::build_adapters::FoldedIndex::default();
    let probe =
        crate::build_adapters::BuildCtx::new(observed_at, &no_rows, coverage, &stored_cache);
    let mut store_dirs: Vec<crate::build_adapters::FoldedDir> = Vec::new();
    // Stores measured this pass (identified or replayed), and which of
    // them were *not* re-walked, so their stored units may be replayed.
    let mut measured_stores: Vec<(usize, bool)> = Vec::new();

    let mut units: Vec<ExternalUnit> = Vec::new();
    let mut observed: Vec<crate::growth::ObservedExternal> = Vec::new();
    // Units whose folded rows this pass replayed rather than re-took.
    // They are re-stamped below so the next pass's window can still
    // vouch for them; without that, reuse would only ever work on every
    // second pass.
    let mut reused_unit_paths: Vec<String> = Vec::new();
    let mut protected_keys: HashSet<String> = HashSet::new();
    // What an incomplete fold *did* read this pass, by key: a lower
    // bound the unit row shows (flagged) when the store has no complete
    // measurement to show instead. Never a growth input.
    let mut lower_bounds: HashMap<String, crate::folded_measurement::FoldedUnit> = HashMap::new();
    let mut meta_by_key: HashMap<String, MeasuredUnit> = HashMap::new();

    // Project worktrees, one spelling. A worktree inside an external
    // unit is measured by its project's walk; the same nested-exclusion
    // that keeps one detector location out of another keeps it out of
    // the unit that contains it, so its bytes are counted once.
    let canonical_worktrees: Vec<(PathBuf, u64)> = worktrees
        .iter()
        .map(|w| {
            (
                crate::fs_gate::canonicalize(&w.path).unwrap_or_else(|_| w.path.clone()),
                w.reported_bytes,
            )
        })
        .collect();

    // A remainder unit (`Detector::remainder_of`) is measured whole and
    // then has its sibling's measured units subtracted, so it goes after
    // every sibling. Units whose measurement did not complete this pass
    // are remembered: a remainder over one of them cannot be derived.
    let remainder_roles: HashMap<usize, crate::locations::Remainder> = canon_candidates
        .iter()
        .enumerate()
        .filter_map(|(i, (c, _))| {
            detectors
                .detectors()
                .iter()
                .find(|d| d.id() == c.detector_id)
                .and_then(|d| d.remainder_of())
                .map(|r| (i, r))
        })
        .collect();
    let mut order: Vec<usize> = (0..canon_candidates.len()).collect();
    order.sort_by_key(|i| remainder_roles.contains_key(i));
    let mut unmeasured: Vec<PathBuf> = Vec::new();

    // Where each measured location's last use is recorded, asked of the
    // detectors that declared it (never a table of tool names here), and
    // the drilldown rows an earlier pass stored for units whose folded
    // rows this pass may replay instead of re-walk.
    let last_use_decls: HashMap<usize, Vec<crate::locations::LastUseSource>> = {
        let located: Vec<crate::build_stores::Located> = canon_candidates
            .iter()
            .map(|(c, canonical)| crate::build_stores::Located {
                detector_id: &c.detector_id,
                category: c.category,
                path: canonical,
            })
            .collect();
        crate::last_used::declared_sources(&detectors, &located)
    };
    let previous_children: HashMap<String, Vec<crate::drilldown::UnitChild>> = swamp_dir
        .map(crate::growth::previous_unit_children)
        .unwrap_or_default();
    if trace {
        eprintln!(
            "[xtrace] build-store/cache/coverage setup containers={} elapsed={:?}",
            store_containers.len(),
            trace_mark.elapsed()
        );
        trace_mark = std::time::Instant::now();
    }
    let mut sources_by_key: HashMap<String, Vec<crate::locations::LastUseSource>> = HashMap::new();
    // What an incomplete fold's rows would show as children this pass.
    let mut lower_children: HashMap<String, Vec<crate::drilldown::UnitChild>> = HashMap::new();

    // Measure at most two large, independent ordinary roots together.
    // All inputs are derived from the same candidate set and event window
    // as the serial path; only the folds overlap. Reduction below remains
    // in `order`, so remainders and ownership retain their existing rules.
    let worker_limit = external_measure_worker_limit();
    let mut parallel_jobs = Vec::new();
    if worker_limit > 1 {
        let mut path_counts: HashMap<PathBuf, usize> = HashMap::new();
        for (_, path) in &canon_candidates {
            *path_counts.entry(path.clone()).or_default() += 1;
        }
        let mut eligible = Vec::new();
        for &idx in &order {
            if remainder_roles.contains_key(&idx) || store_containers.contains_key(&idx) {
                continue;
            }
            let (candidate, canonical) = &canon_candidates[idx];
            if path_counts.get(canonical) != Some(&1) {
                continue;
            }
            let device = device_of(canonical);
            let key = unit_key(
                &candidate.detector_id,
                candidate.category,
                device,
                canonical,
            );
            let size_hint = swamp_dir
                .and_then(|dir| last_known_external(dir, &key).ok().flatten())
                .map(|(bytes, _)| bytes)
                .unwrap_or(0);
            // Avoid dispatch overhead for ordinary small units; the hint
            // is historical metadata, never a freshness or reuse signal.
            if size_hint < 16 * 1024 * 1024 {
                continue;
            }
            let mut exclusions: Vec<PathBuf> = canon_candidates
                .iter()
                .enumerate()
                .filter(|(j, (_, other_path))| {
                    *j != idx && other_path != canonical && other_path.starts_with(canonical)
                })
                .map(|(_, (_, path))| path.clone())
                .collect();
            let inside_worktrees: Vec<&(PathBuf, u64)> = canonical_worktrees
                .iter()
                .filter(|(path, _)| *path != *canonical && path.starts_with(canonical))
                .collect();
            exclusions.extend(inside_worktrees.iter().map(|(path, _)| path.clone()));
            exclusions.extend(
                out_of_scope
                    .iter()
                    .filter(|path| *path != canonical && path.starts_with(canonical))
                    .cloned(),
            );
            exclusions.sort();
            exclusions.dedup();
            let sources = last_use_decls.get(&idx).cloned().unwrap_or_default();
            let unit_id = crate::growth::external_unit_table_id(
                &candidate.detector_id,
                category_str(candidate.category),
                canonical,
            );
            let want_children = crate::drilldown::wants_children(
                candidate.category == StorageCategory::Unclassified,
                !sources.is_empty(),
                swamp_dir
                    .and_then(|dir| last_known_external(dir, &key).ok().flatten())
                    .map(|(bytes, _)| bytes),
            );
            let known_children = previous_children.get(&unit_id).filter(|c| !c.is_empty());
            eligible.push(ParallelExternalMeasure {
                idx,
                path: canonical.clone(),
                exclusions,
                want_children,
                reuse_ok: known_children.is_some() || !want_children,
                size_hint,
            });
        }
        parallel_jobs = select_independent_external_jobs(eligible);
    }
    let mut parallel_results = HashMap::new();
    if parallel_jobs.len() > 1 {
        let (results, _workers_launched) = measure_independent_external_jobs(
            parallel_jobs,
            worker_limit,
            swamp_dir,
            observed_at,
            coverage,
        );
        parallel_results = results;
    }

    for idx in order {
        let (candidate, canonical) = &canon_candidates[idx];
        let role = remainder_roles.get(&idx).copied();
        let Candidate {
            detector_id,
            detector_name,
            category,
            provenance,
            path: _,
        } = candidate;
        let category = *category;
        let detector_id = detector_id.clone();
        let detector_name = detector_name.clone();
        let provenance = provenance.clone();
        let canonical = canonical.clone();
        // Other resolved locations strictly nested inside this one are
        // measured as their own external units; excluding them here is
        // what makes this location's own measurement stop double as
        // "this location plus everything separately counted under it"
        // (the external-location double-measurement fix alongside
        // #45-#49: e.g. Cargo home's registry/git subtrees, mise's
        // installs/downloads/plugins/shims, or a model store's blobs).
        // ... and, since 2026-09-22, nested locations that are *out of
        // scope* as well as nested locations that survived. Computing
        // this from the surviving candidate list alone is what made
        // excluding a child read as the parent growing by the child's
        // size.
        let mut nested_exclusions: Vec<PathBuf> = canon_candidates
            .iter()
            .enumerate()
            .filter(|(j, (other, other_canonical))| {
                *j != idx
                    && other_canonical != &canonical
                    && other_canonical.starts_with(&canonical)
                    // A remainder keeps its sibling's bytes in the whole
                    // measurement and subtracts them afterwards.
                    && role.is_none_or(|r| other.detector_id != r.of)
            })
            .map(|(_, (_, other_canonical))| other_canonical.clone())
            .collect();
        nested_exclusions.extend(
            out_of_scope
                .iter()
                .filter(|p| *p != &canonical && p.starts_with(&canonical))
                .cloned(),
        );
        let inside_worktrees: Vec<&(PathBuf, u64)> = canonical_worktrees
            .iter()
            .filter(|(p, _)| *p != canonical && p.starts_with(&canonical))
            .collect();
        nested_exclusions.extend(inside_worktrees.iter().map(|(p, _)| p.clone()));
        let overlap = (!inside_worktrees.is_empty()).then(|| {
            (
                inside_worktrees.len(),
                inside_worktrees.iter().map(|(_, b)| *b).sum::<u64>(),
            )
        });
        nested_exclusions.sort();
        nested_exclusions.dedup();
        let device = device_of(&canonical);
        let key = unit_key(&detector_id, category, device, &canonical);
        let sources = last_use_decls.get(&idx).cloned().unwrap_or_default();
        sources_by_key.insert(key.clone(), sources.clone());
        let unit_id =
            crate::growth::external_unit_table_id(&detector_id, category_str(category), &canonical);
        // A unit that declares a last-use source is drilled into at any
        // size (its children are the depth-2 rows of a reclaim view); an
        // unclassified root only once it is big enough to be worth
        // reading as more than one number. A root last seen below the
        // threshold is not drilled, so a small unclassified root keeps
        // its replay.
        let want_children = crate::drilldown::wants_children(
            category == StorageCategory::Unclassified,
            !sources.is_empty(),
            swamp_dir
                .and_then(|dir| last_known_external(dir, &key).ok().flatten())
                .map(|(bytes, _)| bytes),
        );
        let known_children = previous_children.get(&unit_id).filter(|c| !c.is_empty());
        // Replay needs the drilldown to replay with it.
        let reuse_ok = known_children.is_some() || !want_children;

        // Access and measurement both go through the one folded
        // measurement seam; nothing in this module lists a directory or
        // re-sizes a tree itself
        // (`.oh/guardrails/no-second-traversal-on-report-path.md`).
        // `observe_unit` tries the stored folded rows first, so a unit
        // this pass's event window vouches for costs no listing, no
        // `stat` and not even the readability probe.
        let debug_trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
        if debug_trace {
            eprintln!(
                "[xtrace] measuring {} want_children={want_children}",
                canonical.display()
            );
        }
        let before = debug_trace.then(crate::work_counters::snapshot);
        let started = std::time::Instant::now();
        let (observation, child_dirs) = if let Some(result) = parallel_results.remove(&idx) {
            if debug_trace {
                eprintln!(
                    "[xtrace] parallel measurement {} elapsed={:?}; per-worker counter deltas overlap",
                    canonical.display(),
                    result.elapsed
                );
            }
            (result.observation, result.child_dirs)
        } else {
            match store_containers.get(&idx) {
                Some(container) => {
                    let reuse = probe.can_reuse(container) && reuse_ok;
                    let (obs, dirs) = crate::folded_measurement::observe_unit_with_dirs(
                        swamp_dir,
                        &canonical,
                        &nested_exclusions,
                        observed_at,
                        coverage,
                        reuse,
                        // A build store's adapter identifies from every
                        // directory row: it is walked whole on a change.
                        false,
                    );
                    if debug_trace {
                        let after = crate::work_counters::snapshot();
                        let before = before.unwrap();
                        eprintln!(
                            "[xtrace] store {} can_reuse={reuse} dirs_delta={} files_delta={} elapsed={:?}",
                            canonical.display(),
                            after.dirs_listed - before.dirs_listed,
                            after.files_statted - before.files_statted,
                            started.elapsed()
                        );
                    }
                    if let crate::folded_measurement::UnitObservation::Unit(_) = &obs {
                        measured_stores.push((idx, dirs.is_none()));
                    }
                    let for_children = if want_children { dirs.clone() } else { None };
                    if let Some(dirs) = dirs {
                        store_dirs.extend(crate::build_stores::folded_dirs(&canonical, dirs));
                    }
                    (obs, for_children)
                }
                None if want_children => {
                    let (obs, dirs) = crate::folded_measurement::observe_unit_with_dirs(
                        swamp_dir,
                        &canonical,
                        &nested_exclusions,
                        observed_at,
                        coverage,
                        reuse_ok,
                        true,
                    );
                    if debug_trace {
                        let after = crate::work_counters::snapshot();
                        let before = before.unwrap();
                        eprintln!(
                            "[xtrace] drill  {} dirs_delta={} files_delta={} elapsed={:?}",
                            canonical.display(),
                            after.dirs_listed - before.dirs_listed,
                            after.files_statted - before.files_statted,
                            started.elapsed()
                        );
                    }
                    (obs, dirs)
                }
                None => {
                    let obs = crate::folded_measurement::observe_unit(
                        swamp_dir,
                        &canonical,
                        &nested_exclusions,
                        observed_at,
                        coverage,
                    );
                    if debug_trace {
                        let after = crate::work_counters::snapshot();
                        let before = before.unwrap();
                        eprintln!(
                            "[xtrace] plain  {} dirs_delta={} files_delta={} elapsed={:?}",
                            canonical.display(),
                            after.dirs_listed - before.dirs_listed,
                            after.files_statted - before.files_statted,
                            started.elapsed()
                        );
                    }
                    (obs, None)
                }
            }
        };
        // The drilldown of this pass's own rows, or (for a replayed unit)
        // the one stored with them. Only for a unit that was measured at
        // all; an unclassified root under the threshold is one row.
        let drill_started = std::time::Instant::now();
        let children: Vec<crate::drilldown::UnitChild> = match &observation {
            crate::folded_measurement::UnitObservation::Unit(folded) if want_children => {
                let big_enough = crate::drilldown::worth_listing(!sources.is_empty(), folded.bytes);
                match child_dirs {
                    Some(dirs) if big_enough => crate::drilldown::children_of(
                        &canonical,
                        dirs,
                        folded.bytes,
                        crate::drilldown::DRILLDOWN_TOP_N,
                    ),
                    None if folded.reused && big_enough => {
                        known_children.cloned().unwrap_or_default()
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        if debug_trace && want_children {
            eprintln!(
                "[xtrace] drilldown {} rows={} elapsed={:?}",
                canonical.display(),
                children.len(),
                drill_started.elapsed()
            );
        }
        let row = match observation {
            // Genuinely absent: no candidate this pass. If it was
            // measured before, this observation's own owned sweep
            // tombstones it correctly (a real removal, e.g. the tool was
            // uninstalled and its whole home deleted).
            crate::folded_measurement::UnitObservation::Absent => continue,
            // Present but not readable this pass: protect it from
            // tombstoning, and skip measuring rather than guessing.
            // Coverage is incomplete, which is not a storage change.
            crate::folded_measurement::UnitObservation::Unreadable(_) => {
                unmeasured.push(canonical.clone());
                protected_keys.insert(key);
                continue;
            }
            // The root read fine, but the fold hit an unreadable
            // directory somewhere underneath it: `row.bytes` is a
            // partial sum, not the unit's size this pass. Treated the
            // same as the root being unreadable -- protected from
            // tombstoning and from a growth/regrowth delta, last known
            // value shown instead -- because a folder going unreadable
            // for one pass is coverage shrinking, not the unit shrinking
            // (`.oh/guardrails/coverage-changes-are-not-storage-changes.md`).
            crate::folded_measurement::UnitObservation::Unit(row) if !row.complete => {
                // A reused-but-incomplete unit (a sealed volume with
                // root-only corners, replayed from its stamp) keeps its
                // rows fresh too; otherwise the next pass's window
                // predates them and the whole tree is walked again
                // (R19: every other pass).
                if row.reused {
                    reused_unit_paths.push(canonical.display().to_string());
                }
                lower_bounds.insert(key.clone(), row);
                unmeasured.push(canonical.clone());
                lower_children.insert(key.clone(), children);
                protected_keys.insert(key);
                continue;
            }
            crate::folded_measurement::UnitObservation::Unit(row) => row,
        };
        if row.reused {
            reused_unit_paths.push(canonical.display().to_string());
        }
        let mut row = row;
        if let Some(role) = role {
            // The whole, minus the sibling's units: the parts add up to
            // the whole, and a file hardlinked between a sibling unit and
            // the rest is counted in the sibling's unit, once. If a
            // sibling unit could not be measured, the remainder cannot be
            // derived this pass.
            if unmeasured.iter().any(|p| p.starts_with(&canonical)) {
                protected_keys.insert(key);
                continue;
            }
            let taken: u64 = meta_by_key
                .values()
                .filter(|m| m.detector_id == role.of && m.path.starts_with(&canonical))
                .map(|m| m.bytes)
                .sum();
            row.bytes = row.bytes.saturating_sub(taken);
        }
        observed.push(crate::growth::ObservedExternal {
            key: key.clone(),
            detector_id: detector_id.clone(),
            category: category_str(category).to_string(),
            device,
            path: canonical.display().to_string(),
            bytes: row.bytes,
            hardlinked: row.hardlinked,
        });
        meta_by_key.insert(
            key,
            MeasuredUnit {
                detector_id,
                detector_name,
                category,
                provenance,
                path: canonical,
                bytes: row.bytes,
                hardlinked: row.hardlinked,
                mtime_max: row.mtime_max,
                overlap,
                last_use_sources: sources,
                children,
                sealed_walked_at: row.sealed_walked_at,
            },
        );
    }
    if trace {
        eprintln!(
            "[xtrace] measurement loop measured={} incomplete={} elapsed={:?}",
            observed.len(),
            unmeasured.len(),
            trace_mark.elapsed()
        );
        trace_mark = std::time::Instant::now();
    }

    // Only the regions this pass actually measured completely may be
    // swept for disappearances, and only in this family's rows. A root
    // that was excluded, whose detector was disabled, or that could not
    // be read contributes nothing here, so nothing under it is
    // tombstoned (`.oh/guardrails/history-sweeps-are-owned.md`).
    let mut notes: Vec<String> = partial_containers
        .iter()
        .map(|p| {
            format!(
                "{} could not be listed completely: the units inside it were not measured this pass",
                p.display()
            )
        })
        .collect();
    notes.extend(devicefs_report_notes.iter().cloned());
    let ownership = crate::growth::ObservationOwnership::new(
        crate::growth::KeyFamily::External,
        meta_by_key.values().map(|m| m.path.clone()).collect(),
    )
    // Inside a covered root, outside this pass: an excluded nested
    // location keeps its stored row exactly as it is.
    .excluding(
        out_of_scope
            .iter()
            .cloned()
            .chain(partial_containers)
            .collect(),
    );
    if let Some(dir) = swamp_dir
        && observe
    {
        // A failed cache write is a cache that will miss next pass,
        // which is the correct outcome and not worth failing a report
        // over -- the same terms `record_folded_measurement` writes on.
        let _ = crate::growth::touch_folded_rows(dir, &reused_unit_paths, observed_at);
    }
    let annotations: HashMap<String, (Option<i64>, u32)> = match swamp_dir {
        Some(dir) if observe => crate::growth::observe_and_annotate_external(
            dir,
            &observed,
            &protected_keys,
            &ownership,
            observed_at,
            retention_days,
            since_secs,
        )?,
        Some(dir) => {
            let keys: Vec<String> = observed.iter().map(|o| o.key.clone()).collect();
            crate::growth::annotate_readonly_external(
                dir,
                &keys,
                observed_at,
                retention_days,
                since_secs,
            )?
        }
        None => HashMap::new(),
    };
    if trace {
        eprintln!(
            "[xtrace] growth annotation rows={} elapsed={:?}",
            observed.len(),
            trace_mark.elapsed()
        );
        trace_mark = std::time::Instant::now();
    }

    let consumers_by_key = swamp_dir
        .map(load_all_consumers)
        .transpose()?
        .unwrap_or_default();

    // The stores' interiors. A store that was not re-walked replays its
    // stored units under the same window its folded total was replayed
    // under; one that was re-walked is identified from the directory rows
    // that walk produced, never from stored units that predate it.
    let containers: Vec<crate::build_adapters::BuildContainer> = measured_stores
        .iter()
        .filter_map(|(i, _)| store_containers.get(i).cloned())
        .collect();
    let replayable: HashSet<String> = measured_stores
        .iter()
        .filter(|(_, reused)| *reused)
        .filter_map(|(i, _)| store_containers.get(i).map(|c| c.scope()))
        .collect();
    let replay_cache = crate::build_adapters::ContainerCache::from_containers(
        previous_units
            .iter()
            .filter(|(k, _)| replayable.contains(*k))
            .map(|(_, v)| v.clone())
            .collect(),
    );
    let folded_rows = crate::build_adapters::FoldedIndex::from_dirs(store_dirs);
    // The pass-through enrichment cache: read once, consulted by every
    // card parse, written back with only what is still wanted.
    let cards = crate::build_adapters::model_cards::CardCache::from_entries(
        swamp_dir
            .map(crate::build_stores::load_cards)
            .unwrap_or_default(),
        crate::build_adapters::model_cards::NEW_PARSES_PER_PASS,
    );
    let build_ctx =
        crate::build_adapters::BuildCtx::new(observed_at, &folded_rows, coverage, &replay_cache)
            .with_cards(&cards);
    let identified_stores: Vec<String> = containers
        .iter()
        .filter(|c| !build_ctx.can_reuse(c))
        .map(|c| format!("|{}|", c.path.display()))
        .collect();
    let mut interiors =
        crate::build_adapters::identify_all(&adapters, &[], &containers, &build_ctx);
    if observe {
        let hf_enrich = swamp_dir.is_some_and(|d| crate::growth::load_config(d).hf_enrich);
        crate::hub_api::enrich(&mut interiors, &cards, observed_at, hf_enrich, fetch);
    }
    if let Some(dir) = swamp_dir
        && observe
    {
        let kept = cards.retained(&|k| !identified_stores.iter().any(|p| k.contains(p.as_str())));
        crate::build_stores::save_cards(dir, kept, observed_at);
    }
    crate::report::nested_decision_evidence(&mut interiors, observed_at, true);
    let identified: Vec<crate::build_stores::IdentifiedStore> = measured_stores
        .iter()
        .filter_map(|(i, _)| {
            let (c, canonical) = canon_candidates.get(*i)?;
            Some(crate::build_stores::IdentifiedStore {
                detector_id: c.detector_id.clone(),
                device: device_of(canonical),
                path: canonical.clone(),
            })
        })
        .collect();
    crate::build_stores::record_history(
        swamp_dir,
        observe,
        &identified,
        &mut interiors,
        &out_of_scope,
        observed_at,
        retention_days,
        since_secs,
    )?;
    if trace {
        eprintln!(
            "[xtrace] build-store interiors/history units={} elapsed={:?} total={:?}",
            interiors.len(),
            trace_mark.elapsed(),
            trace_started.elapsed()
        );
    }
    if let Some(dir) = swamp_dir
        && observe
        && !containers.is_empty()
    {
        let mut this_pass: HashMap<String, Vec<crate::artifact::NestedArtifact>> = HashMap::new();
        for c in &containers {
            this_pass.entry(c.scope()).or_default();
        }
        for u in &interiors {
            if let Some(scope) = &u.container_id
                && let Some(v) = this_pass.get_mut(scope)
            {
                v.push(u.clone());
            }
        }
        let carried: HashMap<String, (u64, Vec<crate::artifact::NestedArtifact>)> = previous_units
            .into_iter()
            .filter(|(k, _)| !this_pass.contains_key(k))
            .collect();
        let this_pass: Vec<(String, Vec<crate::artifact::NestedArtifact>)> =
            this_pass.into_iter().collect();
        crate::build_stores::save_units(dir, &this_pass, carried, observed_at);
    }

    let mut overlap_marks_next: HashMap<String, (u32, u64)> = overlap_marks.clone();
    for (
        key,
        MeasuredUnit {
            detector_id,
            detector_name,
            category,
            provenance,
            path,
            bytes,
            hardlinked,
            mtime_max,
            overlap,
            last_use_sources,
            children,
            sealed_walked_at,
        },
    ) in meta_by_key
    {
        let (mut growth_bytes, regrowth_count) =
            annotations.get(&key).copied().unwrap_or((None, 0));
        let mount_floor = mount_coverage_floor(&overlap_marks, &key);
        let mut coverage_note = None;
        if mount_floor > 0 && mount_floor.saturating_add(since_secs) > observed_at {
            // Read-only reports compute markers in memory too; keep their
            // displayed growth consistent even though they cannot persist
            // the transition for the next pass.
            growth_bytes = None;
            coverage_note = Some(
                "mounted filesystem coverage changed; growth is not compared across that change"
                    .to_string(),
            );
        }
        if let Some(mount_notes) = devicefs_notes_by_unit.get(&key) {
            for note in mount_notes {
                append_coverage_note(&mut coverage_note, note);
            }
        }
        // A worktree registered or unregistered inside this unit moves its
        // bytes with nothing on disk changing: not growth. Growth is not
        // shown while the change is still inside the growth window.
        let overlap_now = overlap.map_or(0u32, |(n, _)| n as u32);
        let changed_at = match overlap_marks.get(&key) {
            Some((count, at)) if *count != overlap_now => observed_at,
            Some((_, at)) => *at,
            None => 0,
        };
        overlap_marks_next.insert(key.clone(), (overlap_now, changed_at));
        if changed_at > 0 && changed_at.saturating_add(since_secs) > observed_at {
            growth_bytes = None;
            let worktree_note = "worktrees inside this unit changed what it counts: a coverage change, so \
                 growth is not shown until it is out of the growth window";
            append_coverage_note(&mut coverage_note, worktree_note);
        }
        let consumers = consumers_by_key.get(&key).cloned().unwrap_or_default();
        let mut evidence = consumers_evidence(&consumers);
        // Activity (#54): the folded walk's own newest-child mtime for
        // this location, labelled modification and never "last used".
        // `docs/usage.md:660` promised this for external locations and
        // nothing produced it.
        evidence.push(crate::activity::modification_evidence_during(
            mtime_max,
            observed_at,
            crate::entities::now(),
        ));
        let found = crate::last_used::probe(&path, &last_use_sources, crate::entities::now());
        units.push(ExternalUnit {
            detector_id,
            detector_name,
            category,
            provenance,
            path,
            bytes,
            mtime_max,
            hardlinked,
            growth_bytes,
            regrowth_count,
            observed_at,
            consumers,
            // The overlap is two numbers now; the sentence is rendered
            // from them (`ExternalUnit::overlap_note`).
            note: with_walked_note(coverage_note, sealed_walked_at),
            evidence,
            bytes_counted_elsewhere: overlap.map_or(0, |(_, b)| b),
            overlap_count: overlap.map_or(0, |(n, _)| n as u32),
            last_used: found.last_used,
            children: crate::drilldown::reconciled(
                crate::last_used::with_child_last_used(children, &found.children),
                bytes,
            ),
        });
    }
    if let Some(dir) = swamp_dir
        && observe
    {
        // A cache that fails to write means no memory of the change: the
        // next pass may show growth once, the slow answer, never a crash.
        let _ = crate::growth::write_overlap_marks(dir, &overlap_marks_next);
    }
    for key in &protected_keys {
        // A protected (inaccessible-this-pass) unit still needs to be
        // visible -- "unknown", never silently dropped -- but there is
        // no fresh measurement to report; fall back to its last known
        // value from the store, when there is one.
        if let Some((detector_id, category_s, _device, path)) = parse_external_key(key) {
            let path_buf = PathBuf::from(path);
            let last = swamp_dir.and_then(|dir| last_known_external(dir, key).ok().flatten());
            let consumers = swamp_dir
                .map(|dir| load_consumers(dir, key))
                .transpose()?
                .unwrap_or_default();
            let evidence = consumers_evidence(&consumers);
            // The last complete measurement when there is one; else the
            // lower bound this pass read, said to be one. A unit whose
            // interior always has a root-only corner (the sealed
            // simulator runtime volumes) used to show 0 B forever.
            let bound = lower_bounds.get(key);
            let (bytes, mtime_max, hardlinked, note) = match (&last, bound) {
                // A lower bound above the last complete figure is the
                // more current fact (the tree grew since), still a bound.
                (Some(last), Some(b)) if b.bytes > last.0 => (
                    b.bytes,
                    b.mtime_max,
                    b.hardlinked,
                    "coverage incomplete: some directories could not be read; bytes are a lower bound",
                ),
                (Some(last), _) => (
                    last.0,
                    0,
                    true,
                    "coverage incomplete this pass: could not be read; last complete measurement shown",
                ),
                (None, Some(b)) => (
                    b.bytes,
                    b.mtime_max,
                    b.hardlinked,
                    "coverage incomplete: some directories could not be read; bytes are a lower bound",
                ),
                (None, None) => (
                    0,
                    0,
                    true,
                    "coverage incomplete this pass: could not be read",
                ),
            };
            let sources = sources_by_key.get(key).cloned().unwrap_or_default();
            let found = crate::last_used::probe(&path_buf, &sources, crate::entities::now());
            let unit_id =
                crate::growth::external_unit_table_id(&detector_id, &category_s, &path_buf);
            // Rows must sum to the total the unit shows. The rows of this
            // pass sum to this pass's lower bound; the stored rows sum to
            // the last complete measurement. Pair each with its own total,
            // and let `reconciled` name any remaining gap.
            let fresh = lower_children.remove(key).filter(|c| !c.is_empty());
            let stored = previous_children.get(&unit_id).cloned();
            let children = match (&last, bound) {
                (Some(last), Some(b)) if b.bytes > last.0 => fresh.or(stored),
                (Some(last), _) => stored
                    .clone()
                    .filter(|c| crate::drilldown::rows_total(c) == last.0 as i64)
                    .or(fresh)
                    .or(stored),
                _ => fresh.or(stored),
            }
            .unwrap_or_default();
            units.push(ExternalUnit {
                detector_id: detector_id.clone(),
                // The authorized scope already told us this detector's
                // name; never read detector output back for a label.
                detector_name: detector_names
                    .get(&detector_id)
                    .cloned()
                    .unwrap_or_else(|| detector_id.clone()),
                category: category_from_str(&category_s).unwrap_or(StorageCategory::Unclassified),
                provenance: Provenance::BuiltinConvention,
                path: path_buf,
                bytes,
                mtime_max,
                hardlinked,
                growth_bytes: None,
                regrowth_count: last.as_ref().map(|r| r.1).unwrap_or(0),
                observed_at,
                consumers,
                note: Some(note.to_string()),
                evidence,
                bytes_counted_elsewhere: 0,
                overlap_count: 0,
                last_used: found.last_used,
                children: crate::drilldown::reconciled(
                    crate::last_used::with_child_last_used(children, &found.children),
                    bytes,
                ),
            });
        }
    }

    // A model store's entries say when each was last read; its drilldown
    // rows take that fact from the adapter (the store declared it so).
    crate::build_adapters::model_stores::attach_last_read(
        &mut units,
        &interiors,
        crate::entities::now(),
    );
    // A store's own row is the external unit, and its history is the
    // external unit's: the same bytes on the same observation, never a
    // second key.
    for u in interiors.iter_mut() {
        if let Some(e) = units.iter().find(|e| e.path == u.path) {
            *u = crate::build_adapters::NestedUnitBuilder::amend(u.clone())
                .history(e.growth_bytes, e.regrowth_count)
                .build();
        }
    }
    Ok(ExternalObservation {
        units,
        interiors,
        notes,
    })
}

pub(crate) fn category_from_str(s: &str) -> Option<StorageCategory> {
    Some(match s {
        "installation" => StorageCategory::Installation,
        "downloads" => StorageCategory::Downloads,
        "cache" => StorageCategory::Cache,
        "local-state" => StorageCategory::LocalState,
        "environments" => StorageCategory::Environments,
        "build-output" => StorageCategory::BuildOutput,
        "models" => StorageCategory::Models,
        "unclassified" => StorageCategory::Unclassified,
        _ => return None,
    })
}

fn parse_external_key(key: &str) -> Option<(String, String, u64, String)> {
    let mut parts = key.split('\u{1}');
    let detector_id = parts.next()?.to_string();
    let category = parts.next()?.to_string();
    let device: u64 = parts.next()?.parse().ok()?;
    let path = parts.next()?.to_string();
    Some((detector_id, category, device, path))
}

fn last_known_external(swamp_dir: &Path, key: &str) -> Result<Option<(u64, u32)>> {
    crate::growth::peek_external_current(swamp_dir, key)
}

// ---------------------------------------------------------------------
// Consumer associations (#43 acceptance: zero/one/many, counted once,
// association changes never duplicate the unit or reset its history).
// A small JSON sidecar, deliberately separate from the byte-history
// Parquet store: touching it can never affect growth/regrowth.
// ---------------------------------------------------------------------

/// The declared-consumer sidecar lives in the store's own columnar
/// table (`crate::assoc_store::ConsumerTable`), not a JSON file: it is
/// per-unit data, and a JSON map rewritten whole on every association
/// change is a parallel database with a JSON syntax
/// (`.oh/guardrails/store-data-is-parquet-not-json-sidecars.md`). It
/// still lives apart from the byte-history store, so touching an
/// association can never affect growth or regrowth.
fn load_all_consumers(swamp_dir: &Path) -> Result<HashMap<String, Vec<ExternalConsumer>>> {
    let mut out: HashMap<String, Vec<ExternalConsumer>> = HashMap::new();
    for (key, cached) in crate::assoc_store::ConsumerTable::open(swamp_dir).load() {
        let entry = out.entry(key).or_default();
        for row in cached.rows {
            let label = row.first().cloned().unwrap_or_default();
            if label.is_empty() {
                continue;
            }
            entry.push(ExternalConsumer {
                label,
                note: row.get(1).filter(|n| !n.is_empty()).cloned(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
fn save_all_consumers(
    swamp_dir: &Path,
    map: &HashMap<String, Vec<ExternalConsumer>>,
) -> Result<()> {
    let cache: HashMap<String, crate::assoc_store::CachedRows> = map
        .iter()
        .map(|(key, consumers)| {
            (
                key.clone(),
                crate::assoc_store::CachedRows {
                    fingerprint: crate::assoc_store::DECLARED_BY_HAND.to_string(),
                    observed_at: 0,
                    rows: consumers
                        .iter()
                        .map(|c| vec![c.label.clone(), c.note.clone().unwrap_or_default()])
                        .collect(),
                },
            )
        })
        .collect();
    crate::assoc_store::ConsumerTable::open(swamp_dir).save(&cache, crate::entities::now())
}

/// This unit's declared consumers, `[]` when none are recorded.
pub fn load_consumers(swamp_dir: &Path, key: &str) -> Result<Vec<ExternalConsumer>> {
    Ok(load_all_consumers(swamp_dir)?
        .remove(key)
        .unwrap_or_default())
}

/// Associates `label` with the external unit identified by `key`.
/// Idempotent (associating the same label twice is a no-op, not a
/// duplicate entry). Never touches the growth store: byte history and
/// regrowth for this unit are completely unaffected by this call.
#[cfg(test)]
pub(crate) fn associate_consumer(
    swamp_dir: &Path,
    key: &str,
    label: &str,
    note: Option<&str>,
) -> Result<()> {
    let mut map = load_all_consumers(swamp_dir)?;
    let entry = map.entry(key.to_string()).or_default();
    if let Some(existing) = entry.iter_mut().find(|c| c.label == label) {
        existing.note = note.map(str::to_string);
    } else {
        entry.push(ExternalConsumer {
            label: label.to_string(),
            note: note.map(str::to_string),
        });
    }
    save_all_consumers(swamp_dir, &map)
}

/// Removes `label` from the external unit identified by `key`, if
/// present. Never touches the growth store.
#[cfg(test)]
pub(crate) fn dissociate_consumer(swamp_dir: &Path, key: &str, label: &str) -> Result<()> {
    let mut map = load_all_consumers(swamp_dir)?;
    if let Some(entry) = map.get_mut(key) {
        entry.retain(|c| c.label != label);
        if entry.is_empty() {
            map.remove(key);
        }
    }
    save_all_consumers(swamp_dir, &map)
}

/// Sum of every external unit's bytes, for `reconciliation` -- counted
/// once per unit regardless of how many consumers reference it (a
/// consumer association never duplicates the unit).
pub fn total_bytes(units: &[ExternalUnit]) -> u64 {
    units.iter().map(|u| u.bytes).sum()
}

#[cfg(test)]
mod devicefs_coverage_tests {
    use super::*;
    use std::fs;

    fn candidate(path: &Path) -> Candidate {
        Candidate {
            detector_id: "developer-home".into(),
            detector_name: "Developer".into(),
            category: StorageCategory::Unclassified,
            provenance: Provenance::BuiltinConvention,
            path: path.to_path_buf(),
        }
    }

    #[test]
    fn automatic_external_walk_excludes_nested_devicefs_but_explicit_scope_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Library/Developer");
        let mount = root.join("CoreDevice/DeviceFS");
        fs::create_dir_all(&mount).unwrap();
        fs::write(root.join("host-file"), vec![b'h'; 4096]).unwrap();
        fs::write(mount.join("virtual-file"), vec![b'v'; 64 * 1024]).unwrap();
        let candidate = candidate(&root);
        let candidates = vec![(candidate, root.clone())];
        let mounts = vec![crate::fs_gate::fs_space::MountPoint {
            path: mount.clone(),
            fs_type: "DeviceFS".into(),
            local: true,
        }];

        let automatic = devicefs_unit_coverage(true, &candidates, &mounts);
        assert_eq!(automatic.len(), 1);
        assert_eq!(automatic[0].mounts, vec![mount.clone()]);
        let explicit = devicefs_unit_coverage(false, &candidates, &mounts);
        assert_eq!(explicit.len(), 1);
        assert!(explicit[0].mounts.is_empty());
        let mut marks = HashMap::new();
        update_devicefs_mount_marks(&mut marks, &automatic, 100);
        update_devicefs_mount_marks(&mut marks, &explicit, 200);
        assert_eq!(mount_coverage_floor(&marks, &explicit[0].unit_key), 200);
        assert!(marks.values().all(|(excluded, _)| *excluded == 0));

        let coverage = crate::fs_events::EventCoverage::untrusted();
        let measured_with_mount =
            crate::folded_measurement::observe_unit(None, &root, &[], 1_000, &coverage);
        let measured_without_mount = crate::folded_measurement::observe_unit(
            None,
            &root,
            &automatic[0].mounts,
            1_000,
            &coverage,
        );
        let bytes = |observation| match observation {
            crate::folded_measurement::UnitObservation::Unit(unit) => unit.bytes,
            _ => panic!("fixture external root should be measurable"),
        };
        assert!(
            bytes(measured_without_mount) < bytes(measured_with_mount),
            "the shared external exclusion must prevent traversal bytes under DeviceFS"
        );
    }

    #[test]
    fn devicefs_mount_markers_record_appearance_stability_and_disappearance() {
        let mount = PathBuf::from("/Users/test/Library/Developer/CoreDevice/DeviceFS");
        let unit_key = "unit-key".to_string();
        let mounted = DeviceFsUnitCoverage {
            unit_key: unit_key.clone(),
            mounts: vec![mount.clone()],
        };
        let absent = DeviceFsUnitCoverage {
            unit_key: unit_key.clone(),
            mounts: Vec::new(),
        };
        let marker = format!("{}{}", mount_marker_prefix(&unit_key), mount.display());
        let mut marks = HashMap::new();

        update_devicefs_mount_marks(&mut marks, &[mounted.clone()], 100);
        assert_eq!(marks[&marker], (1, 100));
        assert_eq!(mount_coverage_floor(&marks, &unit_key), 100);

        update_devicefs_mount_marks(&mut marks, &[mounted], 200);
        assert_eq!(marks[&marker], (1, 100), "stable presence keeps its time");

        update_devicefs_mount_marks(&mut marks, &[absent.clone()], 300);
        assert_eq!(marks[&marker], (0, 300));
        let (notes, _) = devicefs_unit_notes(&[absent.clone()], &marks, 300, 3_600);
        assert!(notes[&unit_key][0].contains("no longer excluded from this measurement"));

        update_devicefs_mount_marks(&mut marks, &[absent], 400);
        assert_eq!(marks[&marker], (0, 300), "stable absence keeps its time");
    }
}

#[cfg(test)]
mod consumer_sidecar_tests {
    //! Moved from `tests/external_units.rs` when the sidecar writers
    //! became test-only: the same two #43 adversarial tests.
    use super::*;
    use crate::locations::{Environment, Platform, Registry, StorageCategory};
    use crate::scope::{ScanConfig, resolve_effective_scope};
    use std::collections::HashMap;
    use std::fs;

    fn fixture_env(home: &std::path::Path, extra: &[(&str, &str)]) -> Environment {
        let mut env: HashMap<String, String> = HashMap::new();
        for (k, v) in extra {
            env.insert((*k).to_string(), (*v).to_string());
        }
        Environment::fixture(home.to_path_buf(), env, Platform::MacOS)
    }

    /// An allow-list (see `tests/external_units.rs`): exactly the
    /// detector under test.
    fn only_cargo_home_config() -> ScanConfig {
        ScanConfig {
            defaults: false,
            include: Vec::new(),
            exclude: Vec::new(),
            disabled_detectors: Vec::new(),
            enabled_detectors: vec!["cargo-home".into()],
        }
    }

    fn write_pattern(path: &std::path::Path, bytes: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; bytes as usize]).unwrap();
    }

    fn cargo_home_unit(units: &[ExternalUnit]) -> &ExternalUnit {
        units
            .iter()
            .find(|u| u.detector_id == "cargo-home" && u.category == StorageCategory::Installation)
            .expect("cargo home's own unit (not registry/git) is present")
    }

    #[test]
    fn shared_consumers_are_counted_once_in_totals() {
        let home = tempfile::tempdir().unwrap();
        let cargo_home = home.path().join("fixture-cargo");
        write_pattern(&cargo_home.join("bin/cargo"), 3_000);
        let env = fixture_env(
            home.path(),
            &[("CARGO_HOME", &cargo_home.display().to_string())],
        );
        let registry = Registry::with_builtins();
        let scope = resolve_effective_scope(&env, &only_cargo_home_config(), &[], &registry, 1);
        let store = tempfile::tempdir().unwrap();

        let units = discover_and_measure(
            &scope,
            Some(store.path()),
            true,
            1_000,
            30,
            3600,
            &crate::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        let key = unit_key(
            "cargo-home",
            StorageCategory::Installation,
            0,
            &fs::canonicalize(&cargo_home).unwrap(),
        );
        // Device is baked into the real key by `discover_and_measure`
        // internally; recover the *actual* key from the unit itself instead
        // of recomputing the device by hand.
        let unit = cargo_home_unit(&units);
        let real_key = unit_key(
            &unit.detector_id,
            unit.category,
            real_device(&unit.path),
            &unit.path,
        );
        let _ = key; // illustrative only; `real_key` is what associate_consumer needs

        associate_consumer(store.path(), &real_key, "project-a", None).unwrap();
        associate_consumer(
            store.path(),
            &real_key,
            "project-b",
            Some("declared in config"),
        )
        .unwrap();

        let units2 = discover_and_measure(
            &scope,
            Some(store.path()),
            true,
            2_000,
            30,
            3600,
            &crate::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        let unit2 = cargo_home_unit(&units2);
        assert_eq!(unit2.consumers.len(), 2, "{:?}", unit2.consumers);
        let labels: std::collections::BTreeSet<_> =
            unit2.consumers.iter().map(|c| c.label.clone()).collect();
        assert_eq!(
            labels,
            ["project-a", "project-b"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        // The unit itself is still exactly one row: total_bytes counts its
        // bytes once, not once per consumer.
        assert_eq!(total_bytes(&units2), unit2.bytes);
    }

    #[cfg(unix)]
    fn real_device(path: &std::path::Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(path).unwrap().dev()
    }

    /// Associating and later dissociating a consumer must never duplicate
    /// the unit or perturb its byte history: growth/regrowth before and
    /// after the association churn must match exactly.
    #[test]
    fn association_changes_never_duplicate_the_unit_or_reset_history() {
        let home = tempfile::tempdir().unwrap();
        let cargo_home = home.path().join("fixture-cargo");
        write_pattern(&cargo_home.join("bin/cargo"), 4_000);
        let env = fixture_env(
            home.path(),
            &[("CARGO_HOME", &cargo_home.display().to_string())],
        );
        let registry = Registry::with_builtins();
        let scope = resolve_effective_scope(&env, &only_cargo_home_config(), &[], &registry, 1);
        let store = tempfile::tempdir().unwrap();

        let before = discover_and_measure(
            &scope,
            Some(store.path()),
            true,
            1_000,
            30,
            3600,
            &crate::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        let before_count = before.len();
        let unit = cargo_home_unit(&before);
        let key = unit_key(
            &unit.detector_id,
            unit.category,
            real_device(&unit.path),
            &unit.path,
        );
        let bytes_before = unit.bytes;
        let regrowth_before = unit.regrowth_count;

        for i in 0..3 {
            associate_consumer(store.path(), &key, &format!("consumer-{i}"), None).unwrap();
        }
        for i in 0..3 {
            dissociate_consumer(store.path(), &key, &format!("consumer-{i}")).unwrap();
        }
        associate_consumer(store.path(), &key, "kept", None).unwrap();

        let after = discover_and_measure(
            &scope,
            Some(store.path()),
            true,
            2_000,
            30,
            3600,
            &crate::fs_events::EventCoverage::untrusted(),
        )
        .unwrap();
        assert_eq!(
            after.len(),
            before_count,
            "association churn must not create or drop unit rows"
        );
        let unit_after = cargo_home_unit(&after);
        assert_eq!(unit_after.bytes, bytes_before, "bytes must be unaffected");
        assert_eq!(
            unit_after.regrowth_count, regrowth_before,
            "regrowth_count must be unaffected by consumer association changes"
        );
        assert_eq!(
            unit_after
                .consumers
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            vec!["kept"]
        );
    }
}

/// The coverage note plus, for a unit whose read-only volumes were
/// replayed from their stamps, when those volumes were actually walked:
/// their bytes are that walk's, not this pass's (#181).
fn with_walked_note(note: Option<String>, walked_at: Option<u64>) -> Option<String> {
    let Some(at) = walked_at else { return note };
    let walked = format!(
        "read-only volumes inside walked {}; unchanged since (their stamps)",
        crate::last_used::format_day(at, crate::entities::now())
    );
    Some(match note {
        Some(n) => format!("{n}; {walked}"),
        None => walked,
    })
}

#[cfg(test)]
mod walked_note_tests {
    /// #181 review round 2. Tempting wrong patch: replay sealed volumes
    /// and say nothing, so the unit reads as measured by this pass. The
    /// note names when the volumes were walked, after any coverage note.
    #[test]
    fn a_replayed_volume_says_when_it_was_walked() {
        let at = crate::entities::now() - 3 * 86_400;
        let note = super::with_walked_note(Some("partial".into()), Some(at)).unwrap();
        assert!(
            note.starts_with("partial; read-only volumes inside walked "),
            "{note}"
        );
        assert_eq!(super::with_walked_note(None, None), None);
    }
}
