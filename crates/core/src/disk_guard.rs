//! Refuse to observe when the volume holding the swamp store is nearly
//! full.
//!
//! On a full disk an observation walks tens of GB for minutes and then
//! risks a failed or partial Parquet write. One `statfs` up front
//! (through `fs_gate::fs_space`, never raw libc here) answers "is there
//! room to write the store at all" before any walk starts.
//!
//! Nothing here writes. An aborted run must not tombstone roots, mark
//! them missing, or take the writer lock: callers check *before*
//! persisting scope facts or acquiring the lock, so coverage facts are
//! exactly what the last completed observation left.

use std::path::Path;

const GIB: u64 = 1 << 30;

/// Default floor: the greater of 1 GiB and 1% of the volume.
pub fn default_threshold_bytes(total: u64) -> u64 {
    GIB.max(total / 100)
}

/// Outcome of the free-space decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskDecision {
    Proceed,
    Abort {
        free: u64,
        threshold: u64,
        total: Option<u64>,
    },
}

/// Pure decision. `configured` is the `min_free_bytes` config key:
/// `None` uses the default, `Some(0)` disables the check. An unreadable
/// free-space figure proceeds: a missing measurement is not evidence of
/// a full disk.
pub fn decide(free: Option<u64>, total: Option<u64>, configured: Option<u64>) -> DiskDecision {
    let Some(free) = free else {
        return DiskDecision::Proceed;
    };
    let threshold = match configured {
        Some(n) => n,
        None => default_threshold_bytes(total.unwrap_or(0)),
    };
    if threshold > 0 && free < threshold {
        DiskDecision::Abort {
            free,
            threshold,
            total,
        }
    } else {
        DiskDecision::Proceed
    }
}

/// One `statfs` on the store's volume. A store directory that does not
/// exist yet is measured at its nearest existing ancestor (same
/// volume); nothing is created.
pub fn check(store_dir: &Path, configured: Option<u64>) -> DiskDecision {
    let mut probe = Some(store_dir);
    while let Some(p) = probe {
        if let Some(free) = crate::fs_gate::fs_space::available_bytes(p) {
            let total = crate::fs_gate::fs_space::total_bytes(p);
            return decide(Some(free), total, configured);
        }
        probe = p.parent().filter(|q| !q.as_os_str().is_empty());
    }
    DiskDecision::Proceed
}

pub fn human(bytes: u64) -> String {
    crate::render::human_bytes_pub(bytes)
}

/// The stderr message for an aborted `observe`.
pub fn abort_message(store_dir: &Path, free: u64, threshold: u64) -> String {
    format!(
        "swamp observe: aborted, disk nearly full: {} free ({free} bytes) on the volume holding {}, below the {} ({threshold} bytes) minimum (`min_free_bytes` in config.toml). Nothing was walked and swamp did not write to the store.",
        human(free),
        store_dir.display(),
        human(threshold),
    )
}

/// Process exit code for an observation refused for lack of disk space.
pub const EXIT_DISK_FULL: i32 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    const TB: u64 = 1 << 40;

    #[test]
    fn default_is_one_gib_on_small_volumes_and_one_percent_on_large() {
        assert_eq!(default_threshold_bytes(50 * GIB), GIB);
        assert_eq!(default_threshold_bytes(461_000_000_000), 4_610_000_000);
        assert_eq!(default_threshold_bytes(TB), TB / 100);
    }

    #[test]
    fn the_reported_incident_aborts_and_ample_space_proceeds() {
        // 2 GB free of 461 GB: 1% is 4.6 GB, so it aborts.
        assert!(matches!(
            decide(Some(2_000_000_000), Some(461_000_000_000), None),
            DiskDecision::Abort {
                threshold: 4_610_000_000,
                ..
            }
        ));
        assert_eq!(
            decide(Some(50 * GIB), Some(461_000_000_000), None),
            DiskDecision::Proceed
        );
    }

    #[test]
    fn exactly_the_threshold_proceeds() {
        assert_eq!(
            decide(Some(GIB), Some(10 * GIB), None),
            DiskDecision::Proceed
        );
        assert!(matches!(
            decide(Some(GIB - 1), Some(10 * GIB), None),
            DiskDecision::Abort { .. }
        ));
    }

    #[test]
    fn configured_value_overrides_and_zero_disables() {
        assert!(matches!(
            decide(Some(5 * GIB), Some(TB), Some(10 * GIB)),
            DiskDecision::Abort { threshold, .. } if threshold == 10 * GIB
        ));
        assert_eq!(decide(Some(0), Some(TB), Some(0)), DiskDecision::Proceed);
        assert_eq!(
            decide(Some(5 * GIB), Some(TB), Some(GIB)),
            DiskDecision::Proceed
        );
    }

    #[test]
    fn unknown_free_space_proceeds_and_unknown_total_uses_the_floor() {
        assert_eq!(decide(None, None, None), DiskDecision::Proceed);
        assert!(matches!(
            decide(Some(GIB - 1), None, None),
            DiskDecision::Abort { threshold, .. } if threshold == GIB
        ));
    }

    #[test]
    fn a_missing_store_dir_is_measured_at_an_existing_ancestor() {
        let d = tempfile::tempdir().unwrap();
        let missing = d.path().join("no/such/store");
        assert_eq!(check(&missing, Some(0)), DiskDecision::Proceed);
        assert!(matches!(
            check(&missing, Some(u64::MAX)),
            DiskDecision::Abort { .. }
        ));
        assert!(!missing.exists(), "the check must not create the store");
    }
}
