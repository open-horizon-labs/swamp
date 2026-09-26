//! Reconciliation-time hardlink evidence. Identities never leave the walk.
use crate::fs_gate::{Metadata, MetadataExt};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharingGroup {
    /// Disjoint accounting containers, not file names or ownership claims.
    pub containers: Vec<PathBuf>,
    pub bytes: u64,
    /// More links exist than were observed, or metadata changed during the walk.
    pub unresolved_links: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_thousand_shared_entries_collapse_and_high_fanout_is_linear() {
        let mut collector = Collector::new(
            (0..1000)
                .map(|i| PathBuf::from(format!("/container/{i:04}")))
                .collect(),
        );
        for inode in 0..1000 {
            collector.inodes.insert(
                (1, inode),
                Links {
                    bytes: 4096,
                    links: 20,
                    observed: 20,
                    changed: false,
                    containers: (0..20).collect(),
                },
            );
        }
        let result = collector.summary();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].containers.len(), 20);
        assert_eq!(result.groups[0].bytes, 4096000);
        collector.inodes.clear();
        collector.inodes.insert(
            (1, 1),
            Links {
                bytes: 4096,
                links: 1000,
                observed: 1000,
                changed: false,
                containers: (0..1000).collect(),
            },
        );
        let result = collector.summary();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].containers.len(), 1000);
        assert_eq!(result.groups[0].bytes, 4096);
    }

    #[test]
    fn diverse_groups_are_bounded_and_omissions_are_explicit() {
        let mut collector = Collector::new(
            (0..5001)
                .map(|i| PathBuf::from(format!("/c/{i:04}")))
                .collect(),
        );
        for inode in 1..5001 {
            collector.inodes.insert(
                (1, inode),
                Links {
                    bytes: 4096,
                    links: 2,
                    observed: 2,
                    changed: false,
                    containers: [0, inode as usize].into_iter().collect(),
                },
            );
        }
        let result = collector.summary();
        assert_eq!(result.groups.len(), 4096);
        assert_eq!(result.omitted_groups, 904);
        assert_eq!(result.omitted_bytes, 904 * 4096);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharingSummary {
    pub groups: Vec<SharingGroup>,
    pub omitted_groups: u64,
    pub omitted_bytes: u64,
}

struct Links {
    bytes: u64,
    links: u64,
    observed: u64,
    changed: bool,
    containers: BTreeSet<usize>,
}

pub(crate) struct Collector {
    paths: Vec<PathBuf>,
    lookup: HashMap<PathBuf, usize>,
    inodes: HashMap<(u64, u64), Links>,
}

impl Collector {
    pub(crate) fn new(mut paths: Vec<PathBuf>) -> Self {
        paths.sort();
        paths.dedup();
        let lookup = paths
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, p)| (p, i))
            .collect();
        Self {
            paths,
            lookup,
            inodes: HashMap::new(),
        }
    }

    pub(crate) fn record(&mut self, path: &Path, meta: &Metadata) {
        if !meta.is_file() || meta.nlink() <= 1 {
            return;
        }
        let Some(container) = path.ancestors().find_map(|p| self.lookup.get(p)).copied() else {
            return;
        };
        let bytes = meta.blocks().saturating_mul(512);
        let entry = self
            .inodes
            .entry((meta.dev(), meta.ino()))
            .or_insert_with(|| Links {
                bytes,
                links: meta.nlink(),
                observed: 0,
                changed: false,
                containers: BTreeSet::new(),
            });
        entry.changed |= entry.bytes != bytes || entry.links != meta.nlink();
        entry.observed += 1;
        entry.containers.insert(container);
    }

    pub(crate) fn summary(&self) -> SharingSummary {
        let mut aggregates: BTreeMap<(Vec<usize>, bool), u64> = BTreeMap::new();
        for link in self.inodes.values() {
            let unresolved = link.changed || link.observed != link.links;
            if link.containers.len() < 2 && !unresolved {
                continue;
            }
            *aggregates
                .entry((link.containers.iter().copied().collect(), unresolved))
                .or_default() += link.bytes;
        }
        let mut summary = SharingSummary::default();
        let mut memberships = 0;
        let mut path_bytes = 0;
        let mut aggregates: Vec<_> = aggregates.into_iter().collect();
        aggregates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for ((ids, unresolved_links), bytes) in aggregates {
            let group_path_bytes: usize =
                ids.iter().map(|id| self.paths[*id].as_os_str().len()).sum();
            // Bound output, not correctness: omitted evidence is explicitly counted.
            if summary.groups.len() >= 4096
                || memberships + ids.len() > 65536
                || path_bytes + group_path_bytes > 1024 * 1024
            {
                summary.omitted_groups += 1;
                summary.omitted_bytes += bytes;
                continue;
            }
            memberships += ids.len();
            path_bytes += group_path_bytes;
            summary.groups.push(SharingGroup {
                containers: ids.into_iter().map(|id| self.paths[id].clone()).collect(),
                bytes,
                unresolved_links,
            });
        }
        summary
    }
}
