//! Reclaimable-space estimates with shared-storage and filesystem limits
//! (#59). Separates four different numbers that a single "size" field
//! cannot honestly carry:
//!
//! - **logical bytes**: the nominal/apparent size of the content (a
//!   file's declared length, a Docker object's reported size).
//! - **allocated bytes**: blocks actually charged on disk right now
//!   (`st_blocks * 512`, what the existing walk already measures into
//!   `ArtifactRow::bytes`/`allocated_bytes` -- see `walk.rs`). A sparse
//!   file's allocated bytes can be far below its logical size; that gap
//!   is not "already reclaimed", it was never allocated.
//! - **estimated reclaimable bytes**: what removing this unit would
//!   likely free, which can be *less* than its allocated bytes when
//!   inodes are hardlinked or extents are shared with something outside
//!   the selection (APFS clones/snapshots) -- always [`EstimatedReclaimable`],
//!   never a bare number pretending to be exact.
//! - **observed freed bytes**: an actual before/after `statvfs`
//!   (`df`/`actions::free_space_bytes`) comparison after a real removal,
//!   the only number this module treats as a *measurement* rather than
//!   an estimate, and even that is qualified (Trash, concurrent writers,
//!   and snapshots can all suppress the expected free-space change).
//!
//! Selection-set accounting ([`estimate_selection`]) is the mechanism
//! that keeps a set of chosen units from summing the same physical
//! storage twice -- the adversarial case this module exists to reject.

use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue};

/// A reclaimability estimate that is honest about its own uncertainty:
/// either a specific number, a bounded range with a stated reason, or
/// unknown.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum EstimatedReclaimable {
    /// A specific figure this module is confident in (no hardlink
    /// sharing detected, no clone/snapshot concern applies).
    Known { bytes: u64 },
    /// Bounded between a floor and ceiling, with the reason exact
    /// reclamation cannot be observed (APFS clone/snapshot extent
    /// sharing, unresolved hardlink membership).
    Bounded { min: u64, max: u64, reason: String },
    /// No sourced basis for even a bound.
    Unknown { reason: String },
}

/// The four-way byte accounting for one unit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ByteAccounting {
    /// `None` when no logical/apparent size distinct from allocated
    /// bytes was recorded for this unit (most filesystem artifact rows
    /// only ever measure allocated blocks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_bytes: Option<u64>,
    pub allocated_bytes: u64,
    pub estimated_reclaimable: EstimatedReclaimable,
}

fn now() -> u64 {
    crate::entities::now()
}

fn reclaimable_evidence(estimate: &EstimatedReclaimable, source: EvidenceSource) -> Evidence {
    match estimate {
        EstimatedReclaimable::Known { bytes } => Evidence::known(
            FactKind::Reclaimability,
            FactSubtype::EstimatedReclaimable,
            FactValue::Bytes(*bytes),
            source,
            now(),
        ),
        EstimatedReclaimable::Bounded { min, max, reason } => Evidence::conflicting(
            FactKind::Reclaimability,
            FactSubtype::EstimatedReclaimable,
            vec![FactValue::Bytes(*min), FactValue::Bytes(*max)],
            source,
            now(),
            crate::evidence::Reason::carried(reason.clone()),
        ),
        EstimatedReclaimable::Unknown { reason } => Evidence::unknown(
            FactKind::Reclaimability,
            FactSubtype::EstimatedReclaimable,
            source,
            now(),
            crate::evidence::Reason::carried(reason.clone()),
        ),
    }
}

/// A unit with no known hardlink sharing and no clone/snapshot concern:
/// its allocated bytes are its estimated reclaimable bytes exactly.
pub fn exclusive_allocation(allocated_bytes: u64) -> ByteAccounting {
    ByteAccounting {
        logical_bytes: None,
        allocated_bytes,
        estimated_reclaimable: EstimatedReclaimable::Known {
            bytes: allocated_bytes,
        },
    }
}

/// A unit on a copy-on-write filesystem (APFS clones, snapshots) where
/// extent sharing outside this selection is not queried: reclaimable is
/// bounded between 0 (every extent retained elsewhere) and this unit's
/// full allocated bytes (nothing shared), never asserted exactly.
pub fn apfs_clone_or_snapshot_bound(allocated_bytes: u64) -> ByteAccounting {
    ByteAccounting {
        logical_bytes: None,
        allocated_bytes,
        estimated_reclaimable: EstimatedReclaimable::Bounded {
            min: 0,
            max: allocated_bytes,
            reason: "APFS clone/snapshot extent sharing outside this selection is not queried; \
                     freeing this copy could reclaim up to its full allocated bytes, or as \
                     little as zero if every extent is retained by a clone or snapshot elsewhere"
                .into(),
        },
    }
}

/// The bound for a unit on a filesystem that can share or compress
/// extents below the file, naming the filesystem. APFS keeps its
/// original wording ([`apfs_clone_or_snapshot_bound`]) byte for byte;
/// the Linux filesystems get the same shape with their own mechanism
/// named, because "clone or snapshot" is not why a Btrfs total or an
/// overlayfs total overstates.
pub fn shared_extent_bound(allocated_bytes: u64, filesystem: &str) -> ByteAccounting {
    if filesystem == "APFS" {
        return apfs_clone_or_snapshot_bound(allocated_bytes);
    }
    let mechanism = match filesystem {
        "overlayfs" => {
            "this is a merged view: a file the upper layer did not modify is allocated in a \
             lower layer this unit does not own"
        }
        "ZFS" => "blocks can be compressed, deduplicated or held by a snapshot",
        "XFS" => "extents can be shared by a reflinked copy elsewhere",
        _ => "extents can be shared by a reflink or a snapshot, or stored compressed",
    };
    ByteAccounting {
        logical_bytes: None,
        allocated_bytes,
        estimated_reclaimable: EstimatedReclaimable::Bounded {
            min: 0,
            max: allocated_bytes,
            reason: format!(
                "{filesystem}: {mechanism}, and this pass does not query that; allocated bytes \
                 are an upper bound on what removing this copy frees, not the amount"
            ),
        },
    }
}

/// `statfs(2)` `f_type` -> the filesystem name [`shared_extent_bound`]
/// uses, for the filesystems whose allocation overstates what a removal
/// frees. Pure so both platforms' tests can hold the table.
pub fn shared_extent_filesystem_for_magic(f_type: u64) -> Option<&'static str> {
    match f_type {
        0x9123_683E => Some("Btrfs"),
        0x2FC1_2FC1 => Some("ZFS"),
        0x5846_5342 => Some("XFS"),
        0xCA45_1A4E => Some("bcachefs"),
        0x794C_7630 => Some("overlayfs"),
        _ => None,
    }
}

/// A unit flagged `hardlinked` (`ArtifactRow::hardlinked`/
/// `ExternalUnit::hardlinked`'s conservative default) whose exact shared
/// inode set has not been reconciled this pass (`dedup_stale`): bounded
/// the same way as an APFS clone/snapshot, but naming the real reason
/// (unresolved hardlink membership, not copy-on-write extents).
pub fn hardlink_unresolved_bound(allocated_bytes: u64) -> ByteAccounting {
    ByteAccounting {
        logical_bytes: None,
        allocated_bytes,
        estimated_reclaimable: EstimatedReclaimable::Bounded {
            min: 0,
            max: allocated_bytes,
            reason: "this unit may contain hardlinked files whose other links are not yet \
                     reconciled; freeing it could reclaim up to its full allocated bytes, or \
                     less if another retained path shares the same inodes"
                .into(),
        },
    }
}

/// A sparse file: `logical_bytes` (apparent/declared length) can be far
/// larger than `allocated_bytes` (blocks actually on disk). Removing it
/// only ever frees the allocated bytes -- the gap was never occupying
/// space, so it is never counted as "reclaimable".
pub fn sparse_file_accounting(logical_bytes: u64, allocated_bytes: u64) -> ByteAccounting {
    ByteAccounting {
        logical_bytes: Some(logical_bytes),
        allocated_bytes,
        estimated_reclaimable: EstimatedReclaimable::Known {
            bytes: allocated_bytes,
        },
    }
}

/// Docker's own logical object accounting (`docker system df`'s
/// unique/shared sizes) is a *different number* from the host VM's
/// backing-file allocated bytes (`locations::docker_desktop`'s
/// measurement) -- summing them would double-count the same underlying
/// blocks (the backing file materializes the very objects `docker
/// system df` already reports). This type keeps them structurally
/// separate; there is deliberately no method that adds them together.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DockerByteAccounting {
    /// Sum of this selection's Docker objects' own reported unique
    /// bytes (`docker system df --format`), logical to the daemon.
    pub logical_object_bytes: u64,
    /// The host's VM backing-file allocation
    /// (`locations::docker_desktop`), when measured this pass. `None`
    /// when the backing file was out of scope/unreadable this pass --
    /// never assumed zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_backing_allocated_bytes: Option<u64>,
}

impl DockerByteAccounting {
    /// One Docker object's own daemon-reported size, with no host
    /// backing-file measurement attached. What a joined Docker row
    /// (image, volume, build cache) uses: its bytes came from `docker
    /// system df -v`, never from a folded directory walk.
    pub fn for_object(logical_object_bytes: u64) -> Self {
        Self {
            logical_object_bytes,
            host_backing_allocated_bytes: None,
        }
    }

    /// Evidence facts for both numbers, explicit that they are not
    /// summed into a combined total.
    pub fn evidence(&self) -> Vec<Evidence> {
        let mut out = vec![
            Evidence::known(
                FactKind::Reclaimability,
                FactSubtype::LogicalBytes,
                FactValue::Bytes(self.logical_object_bytes),
                EvidenceSource::DockerApi {
                    detail: "docker system df object accounting".into(),
                },
                now(),
            )
            .with_note("Docker's own logical object size; not the host VM backing-file allocation"),
        ];
        match self.host_backing_allocated_bytes {
            Some(b) => out.push(
                Evidence::known(
                    FactKind::Reclaimability,
                    FactSubtype::AllocatedBytes,
                    FactValue::Bytes(b),
                    EvidenceSource::FilesystemMetadata {
                        detail: "Docker Desktop/OrbStack VM backing-file allocated blocks".into(),
                    },
                    now(),
                )
                .with_note("host disk-image allocation; not the same accounting as Docker's own logical object sizes"),
            ),
            // No host backing-file measurement this pass. Naming
            // `FilesystemMetadata` as the source of a number no
            // filesystem produced is exactly the provenance error the
            // PR #123 review found on joined Docker rows; the honest
            // fact is that reclaimable space is unknown, sourced from
            // the daemon that reported the object.
            None => out.push(
                Evidence::unknown(
                    FactKind::Reclaimability,
                    FactSubtype::EstimatedReclaimable,
                    EvidenceSource::DockerApi {
                        detail: "docker system df object accounting".into(),
                    },
                    now(),
                    crate::reason!("removing this object frees an unknown amount of host backing store: layers \
                     may be shared with other images, and the VM disk image does not shrink on \
                     its own"),
                )
                .with_note(
                    "the host disk-image allocation was not measured this pass, so no filesystem \
                     number is claimed for it",
                ),
            ),
        }
        out
    }
}

/// One unit's contribution to a selection set: its allocated bytes, and
/// -- when known -- the physical inode identities that back it, so
/// [`estimate_selection`] can avoid charging the same inode twice when
/// two selected units happen to share a hardlink.
#[derive(Debug, Clone)]
pub struct SelectionMember {
    pub label: String,
    pub allocated_bytes: u64,
    /// `ArtifactRow::hardlinked`-style conservative flag: `true` means
    /// this unit *may* contain hardlinked files even though the exact
    /// inode set is not tracked at this granularity.
    pub hardlinked: bool,
    /// Known `(device, inode)` identities this unit's allocation charges
    /// to, when the caller has them (a full per-file reconciliation was
    /// run). `None` means "unknown membership" -- conservatively summed
    /// as if exclusive, but flagged via [`SelectionEstimate::unknown_sharing`].
    pub inodes: Option<Vec<(u64, u64)>>,
}

/// The result of reconciling a selection set's allocated bytes against
/// shared inode membership.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectionEstimate {
    /// Sum of every member's `allocated_bytes`, exactly what a naive
    /// "just add up the rows" implementation would report.
    pub naive_sum_bytes: u64,
    /// After removing bytes charged to an inode already counted by an
    /// earlier member in the same selection.
    pub deduplicated_bytes: u64,
    pub double_counted_bytes: u64,
    /// `true` when at least one member is flagged `hardlinked` but has
    /// no known inode list -- `deduplicated_bytes` in that case is a
    /// lower bound on double-counting removed, not a guarantee that
    /// none remains.
    pub unknown_sharing: bool,
}

/// Reconciles a selection so the same physical inode is never charged to
/// more than one member's contribution to the total -- the adversarial
/// case named in #59: summing two rows that happen to hardlink the same
/// files must not double the reported reclaimable total.
pub fn estimate_selection(members: &[SelectionMember]) -> SelectionEstimate {
    let naive_sum_bytes: u64 = members.iter().map(|m| m.allocated_bytes).sum();
    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    let mut deduplicated_bytes = 0u64;
    let mut unknown_sharing = false;
    for m in members {
        match &m.inodes {
            Some(inodes) if !inodes.is_empty() => {
                // Approximate per-inode share: split this member's
                // allocated bytes evenly across its known inodes (the
                // best available estimate without a full per-file
                // ledger), then only charge an inode's share once.
                let per_inode = m.allocated_bytes / inodes.len() as u64;
                let mut remainder = m.allocated_bytes % inodes.len() as u64;
                for inode in inodes {
                    let share = per_inode
                        + if remainder > 0 {
                            remainder -= 1;
                            1
                        } else {
                            0
                        };
                    if seen.insert(*inode) {
                        deduplicated_bytes += share;
                    }
                }
            }
            _ => {
                if m.hardlinked {
                    unknown_sharing = true;
                }
                deduplicated_bytes += m.allocated_bytes;
            }
        }
    }
    SelectionEstimate {
        naive_sum_bytes,
        deduplicated_bytes,
        double_counted_bytes: naive_sum_bytes.saturating_sub(deduplicated_bytes),
        unknown_sharing,
    }
}

/// Observed post-action free-space change (#59's one *measured*, not
/// estimated, number): a real `statvfs`/`df` reading before and after an
/// execution. Missing either reading is `Unknown`, never a fabricated
/// zero. The note always states the limits: Trash, snapshots, open
/// files, and concurrent writers can all suppress or distort the
/// expected change.
pub fn observed_free_space_change(free_before: Option<u64>, free_after: Option<u64>) -> Evidence {
    let observed_at = now();
    match (free_before, free_after) {
        (Some(b), Some(a)) => {
            let delta = a as i64 - b as i64;
            Evidence::known(
                FactKind::Reclaimability,
                FactSubtype::ObservedFreed,
                FactValue::SignedBytes(delta),
                EvidenceSource::Statvfs,
                observed_at,
            )
            .with_note(
                "moving a unit to Trash, an open file held by another process, a filesystem \
                 snapshot, or a concurrent writer can all prevent this from matching the \
                 unit's planned/allocated bytes exactly",
            )
        }
        _ => Evidence::unknown(
            FactKind::Reclaimability,
            FactSubtype::ObservedFreed,
            EvidenceSource::Statvfs,
            observed_at,
            crate::reason!("free space was not measured on both sides of this action"),
        ),
    }
}

/// Convenience: builds every [`Evidence`] fact for one unit's byte
/// accounting (logical, allocated, estimated reclaimable), for
/// attachment to a report row.
pub fn accounting_evidence(acc: &ByteAccounting, source: EvidenceSource) -> Vec<Evidence> {
    let mut out = vec![Evidence::known(
        FactKind::Reclaimability,
        FactSubtype::AllocatedBytes,
        FactValue::Bytes(acc.allocated_bytes),
        source.clone(),
        now(),
    )];
    if let Some(logical) = acc.logical_bytes {
        out.push(Evidence::known(
            FactKind::Reclaimability,
            FactSubtype::LogicalBytes,
            FactValue::Bytes(logical),
            source.clone(),
            now(),
        ));
    }
    out.push(reclaimable_evidence(&acc.estimated_reclaimable, source));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_allocation_is_known_exactly() {
        let acc = exclusive_allocation(1_000);
        assert_eq!(
            acc.estimated_reclaimable,
            EstimatedReclaimable::Known { bytes: 1_000 }
        );
    }

    #[test]
    fn hardlink_unresolved_bound_is_never_asserted_exact() {
        let acc = hardlink_unresolved_bound(4_000);
        match acc.estimated_reclaimable {
            EstimatedReclaimable::Bounded {
                min,
                max,
                ref reason,
            } => {
                assert_eq!(min, 0);
                assert_eq!(max, 4_000);
                assert!(reason.contains("hardlink"));
            }
            other => panic!("expected Bounded, got {other:?}"),
        }
    }

    #[test]
    fn sparse_file_never_counts_the_apparent_size_as_reclaimable() {
        let acc = sparse_file_accounting(200 * 1024 * 1024, 4096);
        assert_eq!(acc.logical_bytes, Some(200 * 1024 * 1024));
        assert_eq!(
            acc.estimated_reclaimable,
            EstimatedReclaimable::Known { bytes: 4096 }
        );
    }

    #[test]
    fn apfs_clone_bound_is_never_asserted_exact() {
        let acc = apfs_clone_or_snapshot_bound(10_000);
        match acc.estimated_reclaimable {
            EstimatedReclaimable::Bounded { min, max, .. } => {
                assert_eq!(min, 0);
                assert_eq!(max, 10_000);
            }
            other => panic!("expected Bounded, got {other:?}"),
        }
    }

    #[test]
    fn selection_set_never_double_counts_a_shared_inode() {
        // Two members that both charge inode (1, 100): naive sum would
        // double it; deduplicated must not.
        let members = vec![
            SelectionMember {
                label: "a".into(),
                allocated_bytes: 1_000,
                hardlinked: true,
                inodes: Some(vec![(1, 100)]),
            },
            SelectionMember {
                label: "b".into(),
                allocated_bytes: 1_000,
                hardlinked: true,
                inodes: Some(vec![(1, 100)]),
            },
        ];
        let est = estimate_selection(&members);
        assert_eq!(est.naive_sum_bytes, 2_000);
        // The tempting shortcut this rejects: reporting 2_000 as the
        // reclaimable total for a selection sharing one inode.
        assert_eq!(est.deduplicated_bytes, 1_000);
        assert_eq!(est.double_counted_bytes, 1_000);
        assert!(!est.unknown_sharing);
    }

    #[test]
    fn selection_set_with_no_shared_inodes_sums_normally() {
        let members = vec![
            SelectionMember {
                label: "a".into(),
                allocated_bytes: 500,
                hardlinked: false,
                inodes: Some(vec![(1, 1)]),
            },
            SelectionMember {
                label: "b".into(),
                allocated_bytes: 700,
                hardlinked: false,
                inodes: Some(vec![(1, 2)]),
            },
        ];
        let est = estimate_selection(&members);
        assert_eq!(est.deduplicated_bytes, 1_200);
        assert_eq!(est.double_counted_bytes, 0);
    }

    #[test]
    fn unknown_inode_membership_on_a_hardlinked_unit_is_flagged() {
        let members = vec![SelectionMember {
            label: "a".into(),
            allocated_bytes: 500,
            hardlinked: true,
            inodes: None,
        }];
        let est = estimate_selection(&members);
        assert!(est.unknown_sharing);
    }

    #[test]
    fn docker_logical_and_host_backing_are_never_summed() {
        let acc = DockerByteAccounting {
            logical_object_bytes: 5_000,
            host_backing_allocated_bytes: Some(50_000_000),
        };
        let evidence = acc.evidence();
        // Two separate facts, never a combined total in either one.
        assert_eq!(evidence.len(), 2);
        for e in &evidence {
            if let crate::evidence::FactStatus::Known(FactValue::Bytes(b)) = &e.status {
                assert_ne!(*b, 5_000 + 50_000_000);
            }
        }
    }

    #[test]
    fn docker_missing_host_backing_is_unknown_not_zero() {
        let acc = DockerByteAccounting::for_object(5_000);
        let evidence = acc.evidence();
        // Unknown reclaimable, not zero -- and sourced from the daemon
        // that reported the object, never from a filesystem that never
        // measured it (the PR #123 review's provenance finding).
        let gap = evidence
            .iter()
            .find(|e| e.subtype == FactSubtype::EstimatedReclaimable)
            .expect("an unmeasured host backing store is a stated unknown");
        assert!(matches!(
            gap.status,
            crate::evidence::FactStatus::Unknown { .. }
        ));
        assert!(
            evidence
                .iter()
                .all(|e| !matches!(e.source, EvidenceSource::FilesystemMetadata { .. })),
            "no filesystem provenance may be claimed for a Docker object: {evidence:?}"
        );
    }

    #[test]
    fn observed_free_space_change_known_both_sides() {
        let ev = observed_free_space_change(Some(1_000_000), Some(1_050_000));
        match &ev.status {
            crate::evidence::FactStatus::Known(FactValue::SignedBytes(d)) => assert_eq!(*d, 50_000),
            other => panic!("expected known signed bytes, got {other:?}"),
        }
    }

    #[test]
    fn observed_free_space_change_missing_side_is_unknown_not_zero() {
        let ev = observed_free_space_change(Some(1_000_000), None);
        assert!(matches!(
            ev.status,
            crate::evidence::FactStatus::Unknown { .. }
        ));
    }

    #[test]
    fn linux_shared_extent_filesystems_are_bounded_and_named() {
        assert_eq!(
            shared_extent_filesystem_for_magic(0x9123_683E),
            Some("Btrfs")
        );
        assert_eq!(
            shared_extent_filesystem_for_magic(0x794C_7630),
            Some("overlayfs")
        );
        // ext4 and tmpfs share nothing: the exact figure stands.
        assert_eq!(shared_extent_filesystem_for_magic(0xEF53), None);
        assert_eq!(shared_extent_filesystem_for_magic(0x0102_1994), None);
        for fs in ["Btrfs", "ZFS", "XFS", "bcachefs", "overlayfs"] {
            let acc = shared_extent_bound(4096, fs);
            match acc.estimated_reclaimable {
                EstimatedReclaimable::Bounded { min, max, reason } => {
                    assert_eq!((min, max), (0, 4096));
                    assert!(reason.starts_with(fs), "{reason}");
                    assert!(reason.contains("upper bound"), "{reason}");
                }
                other => panic!("{fs} must be a bound, not {other:?}"),
            }
        }
        // APFS keeps its original wording exactly.
        assert_eq!(
            format!("{:?}", shared_extent_bound(10, "APFS")),
            format!("{:?}", apfs_clone_or_snapshot_bound(10))
        );
    }

    #[test]
    fn trash_outcome_note_explains_suppressed_free_space() {
        let ev = observed_free_space_change(Some(1_000_000), Some(1_000_000));
        // Same before/after (moved to Trash on the same volume): still
        // `Known(0)`, with the note explaining why that does not mean
        // "nothing happened".
        assert_eq!(
            ev.status,
            crate::evidence::FactStatus::Known(FactValue::SignedBytes(0))
        );
        assert!(ev.note.as_deref().unwrap().contains("Trash"));
    }
}
