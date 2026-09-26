//! BuildKit build-cache records, as the Docker daemon reports them (#71).
//!
//! A BuildKit cache is not a directory swamp walks: its records live
//! inside Docker's data root (on macOS, inside a VM's disk image), and the
//! only honest source for what they are is the daemon. So this adapter
//! identifies nothing from disk. It receives the daemon's answers --
//! already fetched, bounded and cached by `crate::docker` through its
//! allow-listed queries (`docker system df -v`, `docker buildx ls`,
//! `docker buildx du --verbose`, `docker version`) -- and turns each record
//! into a unit **in the daemon's own terms**:
//!
//! * the size is the daemon's logical size for *that record alone*; a
//!   record's parents are separate records with their own sizes, so a
//!   parent/child view never adds one into the other;
//! * `created`, `last used`, `in use`, `shared` and `reclaimable` are the
//!   daemon's facts, carried as such (`reported_by`, a tool-recorded time,
//!   the last-use string verbatim), never converted into a filesystem age
//!   or a build generation;
//! * the host's backing-file allocation (Docker Desktop's disk image,
//!   measured by its detector) is a different number and is never added
//!   to these.
//!
//! # No action, and why
//!
//! Every unit is inspection only. The native operations are coarser
//! than a row: `docker buildx prune --filter id=<id>` removes a record
//! *and* records that depend on it, and `docker builder prune` removes
//! every record not in use that matches its filters. Neither is offered here, and
//! nothing here touches Docker's files.

use super::layout::rfc3339_secs;
use super::{BuildAdapter, BuildCapabilities, BuildContainer, BuildCtx, NestedUnitBuilder};
use crate::artifact::{ArtifactRole, ArtifactVariant, Membership, NestedArtifact};
use crate::entities::Confidence;
use crate::locations::BuildStoreKind;
use std::path::{Path, PathBuf};

pub struct Adapter;

/// How old cached daemon facts may be before a unit says so. The docker
/// module refetches after five minutes; a facts file has no capture time
/// at all.
const STALE_FACTS_SECS: u64 = 5 * 60;

impl BuildAdapter for Adapter {
    fn id(&self) -> &'static str {
        "docker-buildkit"
    }

    fn name(&self) -> &'static str {
        "Docker / BuildKit"
    }

    fn capabilities(&self) -> BuildCapabilities {
        BuildCapabilities {
            identifies_shared_stores: true,
            attributes_package_identity: false,
            actions_available: false,
        }
    }

    fn store_kinds(&self) -> &'static [BuildStoreKind] {
        &[BuildStoreKind::BuildKitCache]
    }

    /// Nothing inside a checkout is a BuildKit cache.
    fn containers(&self, _project_root: &Path, _candidates: &[PathBuf]) -> Vec<BuildContainer> {
        Vec::new()
    }

    fn identify(&self, container: &BuildContainer, ctx: &BuildCtx) -> Vec<NestedArtifact> {
        let builder = container
            .path
            .to_string_lossy()
            .strip_prefix(super::DAEMON_STORE_SCHEME)
            .unwrap_or_default()
            .to_string();
        let Some(facts) = ctx.daemon() else {
            return vec![
                NestedUnitBuilder::new(
                    container,
                    ArtifactRole::Intermediate,
                    container.path.clone(),
                )
                .unsupported_layout(
                    "the Docker daemon was not asked this pass, so this builder's records are \
                         not known",
                )
                .build(),
            ];
        };
        let records = facts.records_of(&builder);
        let total: u64 = records.iter().map(|r| r.bytes).sum();
        let mut root = NestedUnitBuilder::new(
            container,
            ArtifactRole::Intermediate,
            container.path.clone(),
        )
        .is_dir(true)
        .reported_by_manager("docker", total, None)
        .supported_with_reason(format!("BuildKit's build cache on the builder `{builder}`"))
        .evidence(
            "buildkit-daemon",
            format!("{} records reported by the daemon", records.len()),
            Confidence::High,
        )
        .membership(Membership::Unknown)
        .limit(
            "sizes are the daemon's logical sizes, each record's own; the host disk image \
                 that holds them is measured separately and never added to these",
        )
        .consequence(
            "the next builds on this builder rebuild the steps these records cached; cache \
                 mounts are refilled by the builds that use them",
        )
        .no_action_because(
            "native pruning (`docker buildx prune`) removes a record together with the \
                 records that depend on it, which is broader than one row",
        );
        let caps = &facts.capabilities;
        if let Some(v) = &caps.api_version {
            root = root.evidence(
                "buildkit-daemon",
                format!("daemon API {v}"),
                Confidence::High,
            );
        } else {
            root = root.limit("the daemon's API version was not reported");
        }
        let unsupported_api = caps.build_cache_api_unsupported();
        if unsupported_api {
            root = root.limit(format!(
                "the daemon's API {} predates build-cache record detail (API {}.{}); record \
                 types, parents and descriptions may be missing",
                caps.api_version.as_deref().unwrap_or("?"),
                crate::docker::MIN_BUILD_CACHE_API.0,
                crate::docker::MIN_BUILD_CACHE_API.1
            ));
        }
        for l in &caps.buildx_limits {
            if l.contains(&format!("`{builder}`")) || builder == crate::docker::DEFAULT_BUILDER {
                root = root.limit(l.clone());
            }
        }
        match facts.captured_at {
            Some(at) if ctx.observed_at >= at && ctx.observed_at - at > STALE_FACTS_SECS => {
                root = root.limit(format!(
                    "the daemon's answers are {} seconds old (cached); records created or \
                     removed since are not shown",
                    ctx.observed_at - at
                ));
            }
            Some(_) => {}
            None => {
                root = root.limit(
                    "the daemon's answers came from a facts file with no capture time: how \
                     current they are is unknown",
                );
            }
        }
        if let Some(b) = facts.builders.iter().find(|b| b.name == builder) {
            if let Some(status) = &b.status {
                root = root.evidence(
                    "buildkit-daemon",
                    format!("builder status {status}"),
                    Confidence::High,
                );
            }
            if let Some(driver) = &b.driver {
                root = root.evidence(
                    "buildkit-daemon",
                    format!("builder driver {driver}"),
                    Confidence::High,
                );
            }
        }
        let mut children: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for r in &records {
            for p in &r.parents {
                *children.entry(p.as_str()).or_default() += 1;
            }
        }
        let mut units = vec![root.build()];
        for r in &records {
            if r.id.is_empty() {
                continue;
            }
            let n = children.get(r.id.as_str()).copied().unwrap_or(0);
            units.push(record_unit(container, &builder, r, n, unsupported_api));
        }
        units
    }
}

fn record_unit(
    container: &BuildContainer,
    builder: &str,
    r: &crate::docker::DockerCacheFact,
    children: usize,
    unsupported_api: bool,
) -> NestedArtifact {
    let kind = r.cache_type.clone();
    let (role, what, consequence) = match kind.as_deref() {
        Some("regular") => (
            ArtifactRole::Intermediate,
            "a cached build step's filesystem snapshot",
            "the next build that reaches this step runs it again",
        ),
        Some("source.local") => (
            ArtifactRole::Intermediate,
            "an uploaded build context",
            "the next build uploads its context again",
        ),
        Some("source.git.checkout") => (
            ArtifactRole::Intermediate,
            "a git checkout used as a build source",
            "the next build clones the repository again",
        ),
        Some("exec.cachemount") => (
            ArtifactRole::Intermediate,
            "a `RUN --mount=type=cache` cache mount",
            "the builds that use this cache mount refill it (package downloads, compiler caches)",
        ),
        Some("frontend") | Some("internal") => (
            ArtifactRole::Metadata,
            "BuildKit's own bookkeeping",
            "BuildKit recreates it on the next build",
        ),
        _ => (ArtifactRole::Residual, "", ""),
    };
    let path = container.path.join(&r.id);
    let mut b = NestedUnitBuilder::new(container, role.clone(), path).reported_by_manager(
        "docker",
        r.bytes,
        r.created_at.as_deref().and_then(rfc3339_secs),
    );
    b = if role == ArtifactRole::Residual {
        b.unsupported_layout(match &kind {
            Some(k) => format!("record type `{k}` is not one this adapter identifies"),
            None if unsupported_api => {
                "the daemon did not report a record type (its API predates it)".to_string()
            }
            None => "the daemon did not report a record type".to_string(),
        })
    } else {
        b.supported_with_reason(format!(
            "the daemon reports this record as `{}`: {what}",
            kind.as_deref().unwrap_or("")
        ))
        .consequence(consequence)
    };
    b = b.variant(ArtifactVariant {
        configuration: kind.clone(),
        target: Some(builder.to_string()),
        unknowns: vec!["build-generation".into()],
        ..Default::default()
    });
    if let Some(d) = &r.description {
        b = b.evidence("buildkit-description", d.clone(), Confidence::High);
    }
    if let Some(lu) = &r.last_used {
        b = b.evidence(super::LAST_USED_EVIDENCE, lu.clone(), Confidence::High);
    }
    if let Some(n) = r.usage_count {
        b = b.evidence(
            "buildkit-daemon",
            format!("used {n} times (daemon count)"),
            Confidence::High,
        );
    }
    for p in &r.parents {
        b = b.evidence("buildkit-parent", p.clone(), Confidence::High);
    }
    if !r.parents.is_empty() || children > 0 {
        b = b.limit(format!(
            "{} parent record(s), {children} child record(s): this size is this record's own, \
             never including a parent's or a child's",
            r.parents.len()
        ));
    }
    if r.shared {
        b = b.membership(Membership::Unknown).limit(
            "the daemon reports this record shared: other records or images use its bytes, so \
             they are not freed on their own",
        );
    }
    match r.reclaimable {
        Some(true) => {
            b = b.evidence(
                "buildkit-daemon",
                "the daemon reports it reclaimable",
                Confidence::High,
            )
        }
        Some(false) => b = b.limit("the daemon reports it not reclaimable"),
        None => {}
    }
    if r.in_use {
        b = b
            .limit("the daemon reports this record in use by a build running now")
            .no_action_because("the daemon reports this record in use");
    } else {
        b = b.no_action_because(
            "native pruning removes a record with the records that depend on it; no per-row \
             action is offered",
        );
    }
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{AccountingBasis, NestedActionCapability, TimeSource};
    use crate::build_adapters::{ContainerCache, FoldedIndex};
    use crate::docker::{DockerBuilderFact, DockerCacheFact, DockerFacts};
    use crate::fs_events::EventCoverage;

    fn rec(id: &str, kind: &str, bytes: u64) -> DockerCacheFact {
        DockerCacheFact {
            id: id.into(),
            bytes,
            cache_type: Some(kind.into()),
            created_at: Some("2024-01-15T10:32:00Z".into()),
            last_used: Some("2024-02-01T08:00:00Z".into()),
            ..Default::default()
        }
    }

    fn run(facts: &DockerFacts, builder: &str, now: u64) -> Vec<NestedArtifact> {
        let c =
            BuildContainer::daemon_store("docker-buildkit", builder, BuildStoreKind::BuildKitCache);
        let idx = FoldedIndex::default();
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        Adapter.identify(
            &c,
            &BuildCtx::new(now, &idx, &none, &cache).with_daemon(facts),
        )
    }

    fn facts(records: Vec<DockerCacheFact>) -> DockerFacts {
        DockerFacts {
            build_cache: records,
            captured_at: Some(1_000),
            ..Default::default()
        }
    }

    #[test]
    fn unknown_layout_is_explicit_not_empty() {
        let f = facts(vec![rec("a1", "some.future.type", 10)]);
        let units = run(&f, "default", 1_000);
        let u = units.iter().find(|u| u.relative_path == "a1").unwrap();
        assert!(!u.coverage.supported);
        assert!(
            u.coverage
                .limits
                .iter()
                .any(|l| l.contains("some.future.type"))
        );
    }

    #[test]
    fn identification_reads_no_more_than_manifest_cap() {
        let f = facts(vec![rec("a1", "regular", 10)]);
        let (_u, counted) = crate::work_counters::measured(|| run(&f, "default", 1_000));
        assert_eq!(
            counted.header_bytes_read, 0,
            "daemon facts are already in memory; nothing is read"
        );
        assert_eq!(counted.dirs_listed, 0);
        assert_eq!(counted.files_statted, 0, "no Docker file is ever statted");
    }

    #[test]
    fn no_project_or_build_code_is_executed() {
        let f = facts(vec![rec("a1", "regular", 10)]);
        let (_u, counted) = crate::work_counters::measured(|| run(&f, "default", 1_000));
        assert_eq!(
            counted.subprocess_spawns, 0,
            "the adapter never asks the daemon itself"
        );
    }

    #[test]
    fn variants_never_collapse_by_basename() {
        // The same record id on two builders is two records.
        let mut a = rec("same", "regular", 10);
        a.builder = Some("ci".into());
        let b = rec("same", "regular", 20);
        let mut f = facts(vec![a, b]);
        f.builders = vec![DockerBuilderFact {
            name: "ci".into(),
            driver: Some("docker-container".into()),
            status: Some("running".into()),
        }];
        let on_default = run(&f, "default", 1_000);
        let on_ci = run(&f, "ci", 1_000);
        let d = on_default
            .iter()
            .find(|u| u.relative_path == "same")
            .unwrap();
        let c = on_ci.iter().find(|u| u.relative_path == "same").unwrap();
        assert_ne!(d.id, c.id);
        assert_eq!((d.bytes, c.bytes), (20, 10));
        assert_eq!(c.variant.target.as_deref(), Some("ci"));
    }

    #[test]
    fn age_is_not_obsolescence() {
        let f = facts(vec![rec("old", "regular", 10)]);
        let units = run(&f, "default", 2_000_000_000);
        let u = units.iter().find(|u| u.relative_path == "old").unwrap();
        assert_eq!(
            u.time_source,
            TimeSource::ToolRecorded,
            "the daemon's record, not a file age"
        );
        assert_eq!(u.reported_by.as_deref(), Some("docker"));
        assert_eq!(u.basis, AccountingBasis::Logical);
        assert!(matches!(
            u.action,
            NestedActionCapability::Unsupported { .. }
        ));
        assert!(
            u.producer_evidence
                .iter()
                .any(|e| e.source == crate::build_adapters::LAST_USED_EVIDENCE
                    && e.detail == "2024-02-01T08:00:00Z"),
            "last use is carried verbatim as the daemon's fact"
        );
    }

    #[test]
    fn shared_in_use_and_parent_records_keep_their_own_sizes() {
        let mut parent = rec("p", "regular", 1000);
        parent.shared = true;
        let mut child = rec("c", "exec.cachemount", 300);
        child.parents = vec!["p".into()];
        child.in_use = true;
        let f = facts(vec![parent, child]);
        let units = run(&f, "default", 1_000);
        let root = units.iter().find(|u| u.relative_path.is_empty()).unwrap();
        assert_eq!(
            root.bytes, 1300,
            "the container is the sum of records' own sizes, each counted once"
        );
        let p = units.iter().find(|u| u.relative_path == "p").unwrap();
        assert_eq!(p.bytes, 1000, "a parent's size never includes its child's");
        assert!(p.coverage.limits.iter().any(|l| l.contains("shared")));
        assert!(
            p.coverage
                .limits
                .iter()
                .any(|l| l.contains("1 child record"))
        );
        let c = units.iter().find(|u| u.relative_path == "c").unwrap();
        assert_eq!(c.bytes, 300, "a child's size never includes its parent's");
        assert!(c.coverage.limits.iter().any(|l| l.contains("in use")));
        let summary = crate::build_adapters::summarize_container(&root.path, &units);
        let total: u64 = summary.families.iter().map(|f| f.bytes).sum();
        assert_eq!(
            total, 1300,
            "the family view adds each record once: {summary:?}"
        );
        assert_eq!(summary.unaccounted_bytes, Some(0));
    }

    #[test]
    fn an_unsupported_api_and_stale_facts_are_stated() {
        let mut r = rec("x", "regular", 5);
        r.cache_type = None;
        let mut f = facts(vec![r]);
        f.capabilities.api_version = Some("1.38".into());
        let units = run(&f, "default", 1_000 + 3_600);
        let root = &units[0];
        assert!(
            root.coverage
                .limits
                .iter()
                .any(|l| l.contains("predates build-cache record detail"))
        );
        assert!(
            root.coverage
                .limits
                .iter()
                .any(|l| l.contains("3600 seconds old"))
        );
        let x = units.iter().find(|u| u.relative_path == "x").unwrap();
        assert!(x.coverage.limits.iter().any(|l| l.contains("API predates")));
    }

    #[test]
    fn an_unasked_daemon_is_an_explicit_unknown() {
        let c = BuildContainer::daemon_store(
            "docker-buildkit",
            "default",
            BuildStoreKind::BuildKitCache,
        );
        let idx = FoldedIndex::default();
        let none = EventCoverage::untrusted();
        let cache = ContainerCache::disabled();
        let units = Adapter.identify(&c, &BuildCtx::new(1, &idx, &none, &cache));
        assert_eq!(units.len(), 1);
        assert!(!units[0].coverage.supported);
        assert!(
            units[0]
                .coverage
                .limits
                .iter()
                .any(|l| l.contains("not asked"))
        );
    }
}
