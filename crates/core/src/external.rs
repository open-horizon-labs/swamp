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
        swamp_dir,
        observe,
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
    swamp_dir: Option<&Path>,
    observe: bool,
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
    coverage: &crate::fs_events::EventCoverage,
) -> Result<ExternalObservation> {
    // Authorized scope only -- never raw detector candidates. The
    // review's `excluded_agent_home_must_not_be_scanned` counterexample
    // was exactly this loop reading `scope.detectors` and so never
    // seeing the user's exclusion
    // (`.oh/guardrails/discovery-consumes-effective-scope.md`).
    let (candidates, out_of_scope) = authorized_candidates(scope);
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

    // Which of these locations are machine-wide build stores, and which
    // adapter identifies each: the detector's declaration against the
    // adapter's, nothing else (`crate::build_stores::containers_for`).
    let adapters = crate::build_adapters::registry::Registry::with_builtins();
    let detectors = crate::locations::Registry::with_builtins();
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

    for (idx, (candidate, canonical)) in canon_candidates.iter().enumerate() {
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
            .filter(|(j, (_, other_canonical))| {
                *j != idx
                    && other_canonical != &canonical
                    && other_canonical.starts_with(&canonical)
            })
            .map(|(_, (_, other_canonical))| other_canonical.clone())
            .collect();
        nested_exclusions.extend(
            out_of_scope
                .iter()
                .filter(|p| *p != &canonical && p.starts_with(&canonical))
                .cloned(),
        );
        nested_exclusions.sort();
        nested_exclusions.dedup();
        let device = device_of(&canonical);
        let key = unit_key(&detector_id, category, device, &canonical);

        // Access and measurement both go through the one folded
        // measurement seam; nothing in this module lists a directory or
        // re-sizes a tree itself
        // (`.oh/guardrails/no-second-traversal-on-report-path.md`).
        // `observe_unit` tries the stored folded rows first, so a unit
        // this pass's event window vouches for costs no listing, no
        // `stat` and not even the readability probe.
        let debug_trace = std::env::var("SWAMP_TRACE").is_ok_and(|v| v != "0" && !v.is_empty());
        let before = debug_trace.then(crate::work_counters::snapshot);
        let started = std::time::Instant::now();
        let observation = match store_containers.get(&idx) {
            Some(container) => {
                let reuse = probe.can_reuse(container);
                let (obs, dirs) = crate::folded_measurement::observe_unit_with_dirs(
                    swamp_dir,
                    &canonical,
                    &nested_exclusions,
                    observed_at,
                    coverage,
                    reuse,
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
                if let Some(dirs) = dirs {
                    store_dirs.extend(crate::build_stores::folded_dirs(&canonical, dirs));
                }
                obs
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
                obs
            }
        };
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
                protected_keys.insert(key);
                continue;
            }
            crate::folded_measurement::UnitObservation::Unit(row) => row,
        };
        if row.reused {
            reused_unit_paths.push(canonical.display().to_string());
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
            },
        );
    }

    // Only the regions this pass actually measured completely may be
    // swept for disappearances, and only in this family's rows. A root
    // that was excluded, whose detector was disabled, or that could not
    // be read contributes nothing here, so nothing under it is
    // tombstoned (`.oh/guardrails/history-sweeps-are-owned.md`).
    let ownership = crate::growth::ObservationOwnership::new(
        crate::growth::KeyFamily::External,
        meta_by_key.values().map(|m| m.path.clone()).collect(),
    )
    // Inside a covered root, outside this pass: an excluded nested
    // location keeps its stored row exactly as it is.
    .excluding(out_of_scope.clone());
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
    let build_ctx =
        crate::build_adapters::BuildCtx::new(observed_at, &folded_rows, coverage, &replay_cache);
    let mut interiors =
        crate::build_adapters::identify_all(&adapters, &[], &containers, &build_ctx);
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
        },
    ) in meta_by_key
    {
        let (growth_bytes, regrowth_count) = annotations.get(&key).copied().unwrap_or((None, 0));
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
            note: None,
            evidence,
        });
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
            });
        }
    }

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
    Ok(ExternalObservation { units, interiors })
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
