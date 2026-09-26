//! Machine-wide build stores, joined into the live report.
//!
//! The build adapters identify the interior of a Maven repository, a
//! Gradle home, an npm or pnpm store, Go's module and build caches, pip
//! and uv caches, DerivedData, CoreSimulator and the Android SDK. Until
//! this module none of it reached a user's report: those stores are
//! detector-resolved *external* locations, measured and history-tracked
//! by [`crate::external::discover_and_measure`], and the per-root build
//! consumer could not join them without observing the same bytes a
//! second time in the same pass.
//!
//! So the join happens where the measurement already does. The external
//! observation measures each store through the one folded walk; for a
//! store the detector declared (`locations::Detector::build_stores`) it
//! keeps the per-directory rows that walk produces, hands them to the
//! adapter that claims the declared kind
//! (`build_adapters::BuildAdapter::store_kinds`), and records the
//! identified units' history on their own key family
//! ([`crate::growth::KeyFamily::BuildStore`]) under the same
//! observation's ownership. Nothing here matches a detector id, an
//! adapter id or a path shape
//! (`.oh/guardrails/build-stores-join-by-capability.md`).
//!
//! # Reuse
//!
//! An unchanged store is not walked: its folded total is replayed under
//! the pass's event window. Its units are replayed under the same window
//! from [`crate::assoc_store::BuildStoreTable`]; only when they cannot be
//! (no stored units, a different swamp version, a window that does not
//! vouch for them) is the store measured with its directory rows kept,
//! which is the walk a miss would have done anyway.
//!
//! # Daemon stores
//!
//! A BuildKit cache is not a directory swamp walks. [`daemon_containers`]
//! joins the Docker daemon's already-fetched answers to the adapter that
//! claims [`BuildStoreKind::BuildKitCache`], through the same two
//! capabilities.

use crate::artifact::{
    AccountingBasis, ArtifactCoverage, ArtifactEvidence, ArtifactRole, ArtifactVariant, Membership,
    NestedActionCapability, NestedArtifact, TimeSource,
};
use crate::build_adapters::{BuildContainer, FoldedDir};
use crate::entities::Confidence;
use crate::locations::{BuildStoreKind, StorageCategory};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One detector location the external observation measured this pass.
pub(crate) struct Located<'a> {
    pub detector_id: &'a str,
    pub category: StorageCategory,
    pub path: &'a Path,
}

/// The join: which measured locations are stores an adapter identifies,
/// as `(index into located, container)`.
///
/// For each detector, its measured locations are matched against its own
/// `build_stores()` declarations (the anchor vocabulary evaluates against
/// what the detector published), and each declared kind goes to the
/// first adapter, in registry order, whose `store_kinds()` claims it. A
/// location is one store: the first declaration that selects it wins.
pub(crate) fn containers_for(
    adapters: &crate::build_adapters::registry::Registry,
    detectors: &crate::locations::Registry,
    located: &[Located],
) -> Vec<(usize, BuildContainer)> {
    let mut out: Vec<(usize, BuildContainer)> = Vec::new();
    for detector in detectors.detectors() {
        let mine: Vec<usize> = located
            .iter()
            .enumerate()
            .filter(|(_, l)| l.detector_id == detector.id())
            .map(|(i, _)| i)
            .collect();
        if mine.is_empty() {
            continue;
        }
        let published: Vec<(StorageCategory, &Path)> = mine
            .iter()
            .map(|&i| (located[i].category, located[i].path))
            .collect();
        for decl in detector.build_stores() {
            let Some(adapter) = adapters
                .adapters()
                .iter()
                .find(|a| a.store_kinds().contains(&decl.kind))
            else {
                continue;
            };
            for j in decl.anchor.select(&published) {
                let i = mine[j];
                if out.iter().any(|(k, _)| *k == i) {
                    continue;
                }
                out.push((
                    i,
                    BuildContainer::shared_store_of(
                        adapter.id(),
                        located[i].path.to_path_buf(),
                        decl.kind,
                    ),
                ));
            }
        }
    }
    out
}

/// The daemon-answered stores: one container per BuildKit builder the
/// daemon described, for the adapter that claims a daemon-answered kind
/// some registered detector declares.
///
/// Empty when the daemon was not asked (Docker out of scope) or could
/// not answer: an unavailable daemon is a coverage note on the report,
/// never an empty cache.
pub(crate) fn daemon_containers(
    adapters: &crate::build_adapters::registry::Registry,
    detectors: &crate::locations::Registry,
    facts: &crate::docker::DockerFacts,
) -> Vec<BuildContainer> {
    if facts.unavailable.is_some() {
        return Vec::new();
    }
    let mut kinds: Vec<BuildStoreKind> = detectors
        .detectors()
        .iter()
        .flat_map(|d| d.build_stores().iter().map(|decl| decl.kind))
        .filter(|k| k.answered_by_daemon())
        .collect();
    kinds.sort();
    kinds.dedup();
    let mut out = Vec::new();
    for kind in kinds {
        let Some(adapter) = adapters
            .adapters()
            .iter()
            .find(|a| a.store_kinds().contains(&kind))
        else {
            continue;
        };
        for builder in facts.builder_names() {
            out.push(BuildContainer::daemon_store(adapter.id(), &builder, kind));
        }
    }
    out
}

/// A store's folded directory rows, as the adapters see them: absolute
/// paths, allocated totals rolled up bottom-up, completeness rolled up
/// with them, and each directory's newest modification rolled up over
/// its descendants (a module version's last change is its newest file's,
/// wherever inside it that file is).
pub(crate) fn folded_dirs(store: &Path, mut dirs: Vec<crate::report::DirRollup>) -> Vec<FoldedDir> {
    crate::report::aggregate_dir_totals(&mut dirs, &std::collections::HashSet::new());
    let mut newest: HashMap<String, i32> = dirs
        .iter()
        .map(|d| (d.rel_path.clone(), d.mod_time_min))
        .collect();
    let mut order: Vec<usize> = (0..dirs.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(dirs[i].rel_path.matches('/').count()));
    for &i in &order {
        let own = *newest.get(&dirs[i].rel_path).unwrap_or(&0);
        if let Some(parent) = &dirs[i].parent_rel_path {
            let p = newest.entry(parent.clone()).or_insert(0);
            *p = (*p).max(own);
        }
    }
    dirs.into_iter()
        .map(|d| FoldedDir {
            path: if d.rel_path.is_empty() {
                store.to_path_buf()
            } else {
                store.join(&d.rel_path)
            },
            allocated_total: d.allocated_total,
            mtime_max: newest
                .get(&d.rel_path)
                .copied()
                .unwrap_or(d.mod_time_min)
                .max(0) as u64
                * 60,
            complete: d.complete,
        })
        .collect()
}

// ---------------------------------------------------------------------
// Persisted units, for replay
// ---------------------------------------------------------------------

/// The fingerprint every stored row carries: units written by another
/// swamp version are never replayed.
fn fingerprint() -> String {
    format!("swamp-{}", env!("CARGO_PKG_VERSION"))
}

/// Every stored store container's units, with the observation that last
/// verified them, keyed by container scope.
///
/// Includes a container this pass identified zero nested artifacts in
/// (`units` empty): that is a real, verified answer -- "this store has
/// nothing an adapter recognizes" -- not a cache miss, and dropping it
/// here used to mean any such container (a raw cache directory with no
/// build-tool structure at all: CoreSimulator device data, a language's
/// download cache, ...) could never be replayed, so an otherwise
/// unchanged pass re-walked it in full every single time. `save_units`
/// already writes these rows (`this_pass` is seeded with every
/// container's scope before interiors are folded in, so an empty result
/// still gets an entry); this was the read side silently discarding
/// them.
pub(crate) fn load_units(store_dir: &Path) -> HashMap<String, (u64, Vec<NestedArtifact>)> {
    let fp = fingerprint();
    let mut out = HashMap::new();
    for (key, cached) in crate::assoc_store::BuildStoreTable::open(store_dir).load() {
        if cached.fingerprint != fp {
            continue;
        }
        let units = decode(&cached.rows);
        out.insert(key, (cached.observed_at, units));
    }
    out
}

/// Writes this pass's store units: every container identified or
/// replayed this pass, verified at `observed_at`, plus every stored
/// container this pass did not reach, carried forward with its own
/// (older) verification time -- so carrying a row forward can never make
/// it look re-verified.
pub(crate) fn save_units(
    store_dir: &Path,
    this_pass: &[(String, Vec<NestedArtifact>)],
    previous: HashMap<String, (u64, Vec<NestedArtifact>)>,
    observed_at: u64,
) {
    let fp = fingerprint();
    let mut cache: HashMap<String, crate::assoc_store::CachedRows> = HashMap::new();
    for (key, (at, units)) in previous {
        cache.insert(
            key,
            crate::assoc_store::CachedRows {
                fingerprint: fp.clone(),
                observed_at: at,
                rows: encode(&units),
            },
        );
    }
    for (key, units) in this_pass {
        cache.insert(
            key.clone(),
            crate::assoc_store::CachedRows {
                fingerprint: fp.clone(),
                observed_at,
                rows: encode(units),
            },
        );
    }
    // A failed write is a cache that misses next pass, which re-identifies:
    // the slow answer, never a wrong one.
    let _ = crate::assoc_store::BuildStoreTable::open(store_dir).save(&cache, observed_at);
}

fn opt(v: &Option<String>) -> Option<String> {
    v.clone()
}

/// One unit as `(unit, field, value)` rows. Only fields a replay needs;
/// history (`growth_bytes`, `regrowth_count`) and decision evidence are
/// recomputed every pass and never stored here.
fn encode(units: &[NestedArtifact]) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for u in units {
        let mut put = |field: &str, value: String| {
            rows.push(vec![u.id.clone(), field.to_string(), value]);
        };
        put("path", u.path.display().to_string());
        put("relative_path", u.relative_path.clone());
        if let Some(p) = &u.parent_id {
            put("parent_id", p.clone());
        }
        if let Some(c) = &u.container_id {
            put("container_id", c.clone());
        }
        put("role", u.role.label().to_string());
        put(
            "membership",
            match u.membership {
                Membership::Exclusive => "exclusive",
                Membership::SharedHardlink => "shared-hardlink",
                Membership::Residual => "residual",
                Membership::Unknown => "unknown",
            }
            .to_string(),
        );
        put("is_dir", u.is_dir.to_string());
        put("device", u.device.to_string());
        put("inode", u.inode.to_string());
        put("logical_bytes", u.logical_bytes.to_string());
        put("bytes", u.bytes.to_string());
        put("physical_bytes", u.physical_bytes.to_string());
        put("physical_total", u.physical_total.to_string());
        put("mtime_max", u.mtime_max.to_string());
        for (name, value) in [
            ("variant.package", opt(&u.variant.package)),
            ("variant.version", opt(&u.variant.version)),
            ("variant.target", opt(&u.variant.target)),
            ("variant.profile", opt(&u.variant.profile)),
            ("variant.architecture", opt(&u.variant.architecture)),
            ("variant.toolchain", opt(&u.variant.toolchain)),
            ("variant.features", opt(&u.variant.features)),
            ("variant.configuration", opt(&u.variant.configuration)),
            ("variant.generation", opt(&u.variant.generation)),
        ] {
            if let Some(v) = value {
                put(name, v);
            }
        }
        for x in &u.variant.unknowns {
            put("variant.unknown", x.clone());
        }
        for e in &u.producer_evidence {
            put(
                "producer",
                format!(
                    "{}\u{1}{}\u{1}{}",
                    e.source,
                    confidence_label(&e.confidence),
                    e.detail
                ),
            );
        }
        for e in &u.consumer_evidence {
            put(
                "consumer",
                format!(
                    "{}\u{1}{}\u{1}{}",
                    e.source,
                    confidence_label(&e.confidence),
                    e.detail
                ),
            );
        }
        put("supported", u.coverage.supported.to_string());
        put("complete", u.coverage.complete.to_string());
        for l in &u.coverage.limits {
            put("limit", l.clone());
        }
        if let Some(g) = &u.action_group {
            put("action_group", g.clone());
        }
        put("present", u.present.to_string());
        if let Some(a) = &u.adapter {
            put("adapter", a.clone());
        }
        put("basis", u.basis.label().to_string());
        put("time_source", u.time_source.label().to_string());
        match &u.action {
            NestedActionCapability::TrashPath => put("action", "trash-path".into()),
            NestedActionCapability::InspectionOnly => put("action", "inspection-only".into()),
            NestedActionCapability::Unsupported { reason } => {
                put("action", "unsupported".into());
                put("action_reason", reason.clone());
            }
        }
        if let Some(c) = &u.consequence {
            put("consequence", c.clone());
        }
        if let Some(m) = &u.reported_by {
            put("reported_by", m.clone());
        }
        if let Some(l) = &u.writer_lock {
            put("writer_lock", l.display().to_string());
        }
    }
    rows
}

fn confidence_label(c: &Confidence) -> &'static str {
    match c {
        Confidence::High => "high",
        Confidence::Medium => "medium",
        Confidence::Low => "low",
    }
}

fn confidence_from(s: &str) -> Confidence {
    match s {
        "high" => Confidence::High,
        "low" => Confidence::Low,
        _ => Confidence::Medium,
    }
}

fn evidence_from(v: &str) -> Option<ArtifactEvidence> {
    let mut parts = v.splitn(3, '\u{1}');
    let source = parts.next()?.to_string();
    let confidence = confidence_from(parts.next()?);
    let detail = parts.next()?.to_string();
    Some(ArtifactEvidence {
        source,
        detail,
        confidence,
    })
}

/// The inverse of [`encode`]. A unit whose rows are incomplete or
/// unreadable is dropped, and a store with a dropped unit is simply
/// re-identified: a decode never guesses a field.
fn decode(rows: &[Vec<String>]) -> Vec<NestedArtifact> {
    let mut order: Vec<String> = Vec::new();
    let mut by_unit: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for r in rows {
        let [unit, field, value] = r.as_slice() else {
            continue;
        };
        if !by_unit.contains_key(unit) {
            order.push(unit.clone());
        }
        by_unit
            .entry(unit.clone())
            .or_default()
            .push((field.clone(), value.clone()));
    }
    let mut out = Vec::new();
    for id in order {
        let fields = &by_unit[&id];
        if let Some(u) = decode_one(&id, fields) {
            out.push(u);
        }
    }
    out
}

fn decode_one(id: &str, fields: &[(String, String)]) -> Option<NestedArtifact> {
    let one = |name: &str| -> Option<String> {
        fields
            .iter()
            .find(|(f, _)| f == name)
            .map(|(_, v)| v.clone())
    };
    let all = |name: &str| -> Vec<String> {
        fields
            .iter()
            .filter(|(f, _)| f == name)
            .map(|(_, v)| v.clone())
            .collect()
    };
    let num = |name: &str| -> Option<u64> { one(name)?.parse().ok() };
    let flag = |name: &str| -> Option<bool> { one(name)?.parse().ok() };
    let role = ArtifactRole::from_label(&one("role")?)?;
    let membership = match one("membership")?.as_str() {
        "exclusive" => Membership::Exclusive,
        "shared-hardlink" => Membership::SharedHardlink,
        "residual" => Membership::Residual,
        _ => Membership::Unknown,
    };
    let basis = match one("basis")?.as_str() {
        "allocated" => AccountingBasis::Allocated,
        "logical" => AccountingBasis::Logical,
        "unique-allocated" => AccountingBasis::UniqueAllocated,
        _ => AccountingBasis::Unknown,
    };
    let time_source = match one("time_source")?.as_str() {
        "file-modification" => TimeSource::FileModification,
        "folded-directory-modification" => TimeSource::FoldedDirectoryModification,
        "tool-recorded" => TimeSource::ToolRecorded,
        _ => TimeSource::Unknown,
    };
    let action = match one("action")?.as_str() {
        "trash-path" => NestedActionCapability::TrashPath,
        "unsupported" => NestedActionCapability::Unsupported {
            reason: one("action_reason").unwrap_or_default(),
        },
        _ => NestedActionCapability::InspectionOnly,
    };
    Some(NestedArtifact {
        id: id.to_string(),
        path: PathBuf::from(one("path")?),
        relative_path: one("relative_path")?,
        parent_id: one("parent_id"),
        container_id: one("container_id"),
        role,
        membership,
        is_dir: flag("is_dir")?,
        device: num("device")?,
        inode: num("inode")?,
        logical_bytes: num("logical_bytes")?,
        bytes: num("bytes")?,
        physical_bytes: num("physical_bytes")?,
        physical_total: num("physical_total")?,
        mtime_max: num("mtime_max")?,
        variant: ArtifactVariant {
            package: one("variant.package"),
            version: one("variant.version"),
            target: one("variant.target"),
            profile: one("variant.profile"),
            architecture: one("variant.architecture"),
            toolchain: one("variant.toolchain"),
            features: one("variant.features"),
            configuration: one("variant.configuration"),
            generation: one("variant.generation"),
            unknowns: all("variant.unknown"),
        },
        producer_evidence: all("producer")
            .iter()
            .filter_map(|v| evidence_from(v))
            .collect(),
        consumer_evidence: all("consumer")
            .iter()
            .filter_map(|v| evidence_from(v))
            .collect(),
        coverage: ArtifactCoverage {
            supported: flag("supported")?,
            complete: flag("complete")?,
            limits: all("limit"),
        },
        action_group: one("action_group"),
        present: flag("present")?,
        growth_bytes: None,
        regrowth_count: 0,
        decision_evidence: Vec::new(),
        adapter: one("adapter"),
        basis,
        time_source,
        action,
        consequence: one("consequence"),
        reported_by: one("reported_by"),
        writer_lock: one("writer_lock").map(PathBuf::from),
        guidance: crate::cargo_cleanup::Guidance::default(),
    })
}

// ---------------------------------------------------------------------
// History, on the store family's own keys
// ---------------------------------------------------------------------

/// One store this pass identified the interior of, for history.
pub(crate) struct IdentifiedStore {
    pub detector_id: String,
    pub device: u64,
    pub path: PathBuf,
}

/// The history row key of one unit inside a store: the store's detector,
/// the store family's category, the store's device, the unit's own path.
/// Never the unit's role -- reclassifying a unit must not change its key
/// (#65: "metadata-only reclassification ... must not manufacture
/// physical growth/deletion").
fn unit_key(store: &IdentifiedStore, unit: &NestedArtifact) -> String {
    crate::growth::external_row_key(
        &store.detector_id,
        &format!("{}unit", crate::growth::BUILD_STORE_CATEGORY_PREFIX),
        store.device,
        &unit.path.display().to_string(),
    )
}

/// Persists (or reads) the identified store units' size, presence and
/// regrowth on [`crate::growth::KeyFamily::BuildStore`], and annotates
/// each unit with its growth.
///
/// Ownership is the whole point. The sweep may tombstone a unit only
/// inside a store *this* observation identified, and never inside a unit
/// this pass could not measure completely (an unreadable module
/// directory's children are not observed, which is not the same as
/// gone), nor inside a region the scope put out of reach. A store whose
/// detector is disabled, that is excluded, missing or unreadable, is not
/// in `stores` at all, so nothing under it is touched.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_history(
    store_dir: Option<&Path>,
    observe: bool,
    stores: &[IdentifiedStore],
    units: &mut [NestedArtifact],
    out_of_scope: &[PathBuf],
    observed_at: u64,
    retention_days: u64,
    since_secs: u64,
) -> anyhow::Result<()> {
    let Some(dir) = store_dir else {
        return Ok(());
    };
    let mut observed: Vec<crate::growth::ObservedExternal> = Vec::new();
    let mut key_of: Vec<Option<String>> = Vec::with_capacity(units.len());
    for u in units.iter() {
        let Some(store) = stores
            .iter()
            .filter(|s| u.path.starts_with(&s.path))
            .max_by_key(|s| s.path.components().count())
        else {
            key_of.push(None);
            continue;
        };
        // The store's own row is the external unit's: recording it again
        // here would be the same bytes on two keys.
        if u.path == store.path {
            key_of.push(None);
            continue;
        }
        let key = unit_key(store, u);
        observed.push(crate::growth::ObservedExternal {
            key: key.clone(),
            detector_id: store.detector_id.clone(),
            category: format!("{}unit", crate::growth::BUILD_STORE_CATEGORY_PREFIX),
            device: store.device,
            path: u.path.display().to_string(),
            bytes: u.bytes,
            hardlinked: u.membership == Membership::SharedHardlink,
        });
        key_of.push(Some(key));
    }
    let mut not_observed: Vec<PathBuf> = out_of_scope.to_vec();
    // Inside a unit this pass could not read completely, absence of a
    // child is not evidence the child is gone.
    not_observed.extend(
        units
            .iter()
            .filter(|u| u.is_dir && !u.coverage.complete)
            .map(|u| u.path.clone()),
    );
    let ownership = crate::growth::ObservationOwnership::new(
        crate::growth::KeyFamily::BuildStore,
        stores.iter().map(|s| s.path.clone()).collect(),
    )
    .excluding(not_observed);
    let annotations = if observe {
        crate::growth::observe_and_annotate_external(
            dir,
            &observed,
            &std::collections::HashSet::new(),
            &ownership,
            observed_at,
            retention_days,
            since_secs,
        )?
    } else {
        let keys: Vec<String> = observed.iter().map(|o| o.key.clone()).collect();
        crate::growth::annotate_readonly_external(
            dir,
            &keys,
            observed_at,
            retention_days,
            since_secs,
        )?
    };
    for (u, key) in units.iter_mut().zip(key_of) {
        if let Some(key) = key
            && let Some((growth, regrowth)) = annotations.get(&key)
        {
            let amended = crate::build_adapters::NestedUnitBuilder::amend(u.clone())
                .history(*growth, *regrowth)
                .build();
            *u = amended;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_store_kind_is_claimed_by_exactly_one_adapter() {
        let adapters = crate::build_adapters::registry::Registry::with_builtins();
        let detectors = crate::locations::Registry::with_builtins();
        let declared: std::collections::BTreeSet<BuildStoreKind> = detectors
            .detectors()
            .iter()
            .flat_map(|d| d.build_stores().iter().map(|x| x.kind))
            .collect();
        for kind in &declared {
            let claimed: Vec<&str> = adapters
                .adapters()
                .iter()
                .filter(|a| a.store_kinds().contains(kind))
                .map(|a| a.id())
                .collect();
            assert_eq!(
                claimed.len(),
                1,
                "{kind:?} is declared by a detector and claimed by {claimed:?}: a declared store \
                 with no adapter reaches no report, and two adapters would identify it twice"
            );
        }
        for kind in BuildStoreKind::ALL {
            assert!(
                declared.contains(kind),
                "{kind:?} is a store kind no detector declares: dead vocabulary"
            );
        }
    }

    #[test]
    fn a_unit_round_trips_through_the_stored_rows() {
        let c = BuildContainer::shared_store_of(
            "maven",
            PathBuf::from("/fixture/repo"),
            BuildStoreKind::MavenRepository,
        );
        let u = crate::build_adapters::NestedUnitBuilder::new(
            &c,
            ArtifactRole::SharedStoreEntry,
            PathBuf::from("/fixture/repo/org/x/1.0"),
        )
        .supported_with_reason("fixture")
        .bytes_on_basis(4096, AccountingBasis::Allocated)
        .modified(1_000, TimeSource::FoldedDirectoryModification)
        .limit("a limit")
        .consequence("a consequence")
        .no_action_because("shared")
        .build();
        let back = decode(&encode(std::slice::from_ref(&u)));
        assert_eq!(back, vec![u]);
    }

    /// The bug this session found: `save_units` always writes an entry
    /// for a container this pass identified -- even one with zero
    /// nested artifacts, a real verified answer ("this store has
    /// nothing an adapter recognizes"), not a miss -- but `load_units`
    /// used to filter those rows back out on read (`if
    /// !units.is_empty()`), so a container with no build-tool structure
    /// at all (a raw cache directory, or a mounted read-only volume like
    /// CoreSimulator's simulator runtimes) could never be replayed and
    /// was fully re-walked every single pass, unchanged or not. Asserts
    /// the whole round trip, not just that `load_units` returns
    /// *something*: the key must be present, its `units` must be empty
    /// (not fabricated), and its `observed_at` must be the one it was
    /// saved with.
    #[test]
    fn a_container_with_zero_identified_units_still_replays() {
        let tmp = tempfile::tempdir().unwrap();
        let this_pass: Vec<(String, Vec<crate::artifact::NestedArtifact>)> =
            vec![("empty-store-scope".to_string(), Vec::new())];
        save_units(tmp.path(), &this_pass, HashMap::new(), 1_000);

        let loaded = load_units(tmp.path());
        let (observed_at, units) = loaded
            .get("empty-store-scope")
            .expect("a zero-unit container must still be recorded, not silently dropped");
        assert_eq!(*observed_at, 1_000);
        assert!(
            units.is_empty(),
            "a genuinely empty container must round-trip as empty, not fabricate rows"
        );
    }

    #[test]
    fn folded_dirs_roll_bytes_and_newest_modification_up() {
        let row = |rel: &str, parent: Option<&str>, own: u64, min: i32| crate::report::DirRollup {
            worktree_id: "build-store".into(),
            track: None,
            rel_path: rel.into(),
            parent_rel_path: parent.map(str::to_string),
            allocated_total: own,
            own_allocated: own,
            file_count: 1,
            entry_count: 1,
            symlink_count: 0,
            mod_time_min: min,
            complete: true,
            growth_bytes: None,
        };
        let dirs = folded_dirs(
            Path::new("/s"),
            vec![
                row("", None, 1, 10),
                row("a", Some(""), 2, 5),
                row("a/b", Some("a"), 4, 99),
            ],
        );
        let a = dirs.iter().find(|d| d.path == Path::new("/s/a")).unwrap();
        assert_eq!(a.allocated_total, 6);
        assert_eq!(
            a.mtime_max,
            99 * 60,
            "a directory's last change includes its deepest file's"
        );
        let root = dirs.iter().find(|d| d.path == Path::new("/s")).unwrap();
        assert_eq!(root.allocated_total, 7);
    }
}
