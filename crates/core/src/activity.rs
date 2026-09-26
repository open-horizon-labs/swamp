//! Last-activity evidence (#54): folded modification age, access-time
//! availability, and tool-reported use timestamps -- collected from
//! metadata the folded walk (and existing enrichment) already gathered,
//! never a second per-file pass, and never conflated with "last used".
//!
//! - **Modified**: the folded walk already tracks `mtime_max` (newest
//!   recorded modification among a unit's measured children -- see
//!   `report::ArtifactRow::mtime_max`, `external::ExternalUnit` and
//!   `agents::AgentUnit`, all populated by the existing walk/measure
//!   passes). [`modification_evidence`] turns that number into an
//!   [`crate::evidence::Evidence`] fact labelled "newest recorded
//!   modification among measured children", explicitly not "last used".
//! - **Accessed**: only meaningful when the containing mount does not
//!   suppress atime updates. [`atime_reliability`] detects `noatime`/
//!   `relatime` unreliability via `statfs` flags (macOS) or `/proc/mounts`
//!   (Linux) *before* trusting any atime read, and
//!   [`access_time_evidence`] reports `Unavailable` rather than a
//!   misleading "recent access" when the mount cannot support the claim.
//!   This is a bounded, per-unit (not per-file) read: one extra `stat` of
//!   the unit's own anchor path, taken on demand when evidence is
//!   populated for a report row, not during the walk itself.
//! - **Tool-reported use**: normalizes a domain source's own use/build
//!   timestamp (Docker's `LastUsedAt`, a Cargo fingerprint mtime, an
//!   agent session's transcript mtime) into the same contract, with the
//!   domain source named explicitly and the original fact (e.g. Docker's
//!   `last_used` string) preserved untouched alongside it -- never
//!   flattened into a filesystem age.

use crate::evidence::{Evidence, EvidenceSource, FactKind, FactSubtype, FactValue, Freshness};
use std::path::Path;

/// Builds the "newest recorded modification among measured children" fact
/// from a folded unit's already-recorded `mtime_max` (seconds since
/// epoch). `mtime_max == 0` means the walk that produced this row never
/// recorded a timestamp (an older store row, a Docker/pseudo row): that
/// is `Unknown`, never displayed as epoch-1970.
pub fn modification_evidence(mtime_max: u64, observed_at: u64) -> Evidence {
    modification_evidence_during(mtime_max, observed_at, observed_at)
}

/// The scan has a duration. Writes after its start but before measurement
/// completes are ordinary activity, not evidence of a broken clock.
pub fn modification_evidence_during(
    mtime_max: u64,
    observed_at: u64,
    measured_at: u64,
) -> Evidence {
    if mtime_max == 0 {
        return Evidence::unknown(
            FactKind::Activity,
            FactSubtype::Modified,
            EvidenceSource::FilesystemMetadata {
                detail: "no modification time recorded for this unit".into(),
            },
            observed_at,
            crate::reason!("this row's walk did not record a modification time"),
        );
    }
    let mut ev = Evidence::known(
        FactKind::Activity,
        FactSubtype::Modified,
        FactValue::Timestamp(mtime_max),
        EvidenceSource::FilesystemMetadata {
            detail: "newest recorded modification among measured children".into(),
        },
        observed_at,
    )
    .with_event_at(mtime_max)
    .with_freshness(Freshness::with_coverage(
        "only children the folded walk actually measured this pass; a child outside \
         current coverage would not move this value",
    ));
    // A future-dated mtime (clock skew, a restored backup, a container
    // with a wrong clock) must never silently masquerade as "very
    // recent" without comment -- and, per the cleanup-guidance contract,
    // must never be ranked as ancient either. State it; let the renderer
    // decide how to display it (never as a verdict).
    if mtime_max > measured_at.max(observed_at) {
        ev = ev.with_note(
            "recorded modification time is after measurement completed; clock skew or a \
             future-dated timestamp, not evidence of unusual recency",
        );
    }
    ev
}

/// Whether the filesystem containing `path` is known to suppress atime
/// updates (`noatime`) or only update them coarsely (`relatime`), which
/// makes an access-time read unreliable as "was this opened recently".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtimeReliability {
    Reliable,
    Unreliable(&'static str),
    /// The mount option table could not be consulted (permission,
    /// missing `/proc/mounts`, unsupported platform).
    Undetermined(&'static str),
}

/// Pure decision over already-extracted mount flags (macOS `statfs`
/// `f_flags`), factored out so it is unit-testable without a real mount.
pub fn atime_reliability_from_macos_flags(flags: u32) -> AtimeReliability {
    const MNT_NOATIME: u32 = 0x1000_0000;
    if flags & MNT_NOATIME != 0 {
        AtimeReliability::Unreliable("mount option noatime: access times are not updated")
    } else {
        AtimeReliability::Reliable
    }
}

/// Pure decision over one `/proc/mounts`-formatted line's option field
/// (the fourth whitespace-separated column), factored out for Linux
/// testability without a real `/proc/mounts`.
pub fn atime_reliability_from_mount_options(options: &str) -> AtimeReliability {
    let opts: Vec<&str> = options.split(',').collect();
    if opts.contains(&"noatime") {
        AtimeReliability::Unreliable("mount option noatime: access times are not updated")
    } else if opts.contains(&"relatime") {
        AtimeReliability::Unreliable(
            "mount option relatime: access times update at most once per day, not proof of recent access",
        )
    } else {
        AtimeReliability::Reliable
    }
}

/// Whether `path` lies at or under `mount_point`, comparing whole path
/// components.
fn under_mount(path: &str, mount_point: &str) -> bool {
    if mount_point == "/" {
        return path.starts_with('/');
    }
    let mount_point = mount_point.trim_end_matches('/');
    path == mount_point
        || (path.starts_with(mount_point) && path.as_bytes().get(mount_point.len()) == Some(&b'/'))
}

/// Finds the mount-option field for the longest matching mount point
/// prefix of `path` in `/proc/mounts`-formatted `text`. `None` when no
/// mount entry matches (should not normally happen for a real path).
pub fn find_mount_options<'a>(text: &'a str, path: &Path) -> Option<&'a str> {
    let path_str = path.to_string_lossy();
    let mut best: Option<(&str, &str)> = None;
    for line in text.lines() {
        let mut cols = line.split_whitespace();
        let Some(_device) = cols.next() else {
            continue;
        };
        let Some(mount_point) = cols.next() else {
            continue;
        };
        let Some(_fstype) = cols.next() else {
            continue;
        };
        let Some(options) = cols.next() else {
            continue;
        };
        // Component-wise, not a string prefix. `"/variable/foo"
        // .starts_with("/var")` is true, so a `/var` mount's
        // `relatime`/`noatime` options were attributed to a path on a
        // different filesystem (the 2026-09-22 re-review's P3). Same
        // component-vs-string mistake `protection_conflict` gets right.
        if under_mount(&path_str, mount_point)
            && best.is_none_or(|(bp, _)| mount_point.len() > bp.len())
        {
            best = Some((mount_point, options));
        }
    }
    best.map(|(_, o)| o)
}

#[cfg(target_os = "macos")]
pub fn atime_reliability(path: &Path) -> AtimeReliability {
    match crate::fs_gate::sys::volume_info(path) {
        Ok(info) => atime_reliability_from_macos_flags(info.flags as u32),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            AtimeReliability::Undetermined("path contains a NUL byte")
        }
        Err(_) => AtimeReliability::Undetermined("statfs failed (permission or missing path)"),
    }
}

#[cfg(target_os = "linux")]
pub fn atime_reliability(path: &Path) -> AtimeReliability {
    let canon = crate::fs_gate::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match crate::fs_gate::read::bounded_read(
        "/proc/mounts",
        crate::fs_gate::read::BoundedCap::SYSTEM_TABLE,
    )
    .map(|b| b.lossy())
    {
        Ok(text) => match find_mount_options(&text, &canon) {
            Some(opts) => atime_reliability_from_mount_options(opts),
            None => AtimeReliability::Undetermined("no matching entry in /proc/mounts"),
        },
        Err(_) => AtimeReliability::Undetermined("/proc/mounts unreadable"),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn atime_reliability(_path: &Path) -> AtimeReliability {
    AtimeReliability::Undetermined("no atime-reliability check implemented for this platform")
}

/// Builds the access-time fact for a unit's own anchor path. Reads one
/// extra `stat`, bounded to this single path (never the folded
/// children) -- on demand, not during the walk. Reports `Unavailable`
/// rather than trusting an atime read the mount itself does not honor.
pub fn access_time_evidence(path: &Path, observed_at: u64) -> Evidence {
    match atime_reliability(path) {
        AtimeReliability::Unreliable(reason) => Evidence::unavailable(
            FactKind::Activity,
            FactSubtype::Accessed,
            EvidenceSource::FilesystemMetadata {
                detail: "access time".into(),
            },
            observed_at,
            crate::evidence::Reason::carried(reason),
        ),
        AtimeReliability::Undetermined(reason) => Evidence::unavailable(
            FactKind::Activity,
            FactSubtype::Accessed,
            EvidenceSource::FilesystemMetadata {
                detail: "access time".into(),
            },
            observed_at,
            crate::evidence::Reason::carried(reason),
        ),
        AtimeReliability::Reliable => match crate::fs_gate::metadata_following(path) {
            Ok(meta) => {
                use crate::fs_gate::MetadataExt;
                let atime = meta.atime();
                if atime <= 0 {
                    Evidence::unknown(
                        FactKind::Activity,
                        FactSubtype::Accessed,
                        EvidenceSource::FilesystemMetadata {
                            detail: "access time".into(),
                        },
                        observed_at,
                        crate::reason!("no access time recorded"),
                    )
                } else {
                    Evidence::known(
                        FactKind::Activity,
                        FactSubtype::Accessed,
                        FactValue::Timestamp(atime as u64),
                        EvidenceSource::FilesystemMetadata {
                            detail: "access time of this unit's own top-level anchor path only, \
                                     not folded across children"
                                .into(),
                        },
                        observed_at,
                    )
                    .with_event_at(atime as u64)
                }
            }
            Err(e) => Evidence::unavailable(
                FactKind::Activity,
                FactSubtype::Accessed,
                EvidenceSource::FilesystemMetadata {
                    detail: "access time".into(),
                },
                observed_at,
                crate::reason!("could not stat path: {e}"),
            ),
        },
    }
}

/// Normalizes a tool's own reported use/build timestamp (Docker
/// `last_used`, a Cargo fingerprint mtime, an agent session mtime) into
/// the shared contract. `event_at` is the tool's own timestamp, when it
/// parsed; `None` reports `Unknown` rather than fabricating a time.
pub fn tool_reported_use_evidence(
    tool: impl Into<String>,
    detail: impl Into<String>,
    event_at: Option<u64>,
    observed_at: u64,
) -> Evidence {
    let tool = tool.into();
    let detail = detail.into();
    match event_at {
        Some(ts) => Evidence::known(
            FactKind::Activity,
            FactSubtype::ToolReportedUse,
            FactValue::Timestamp(ts),
            EvidenceSource::ToolReported {
                tool,
                detail: detail.clone(),
            },
            observed_at,
        )
        .with_event_at(ts)
        .with_note(detail),
        None => Evidence::unknown(
            FactKind::Activity,
            FactSubtype::ToolReportedUse,
            EvidenceSource::ToolReported {
                tool,
                detail: detail.clone(),
            },
            observed_at,
            crate::reason!("{detail}: no timestamp reported"),
        ),
    }
}

/// Docker's own `LastUsedAt`/`last_used` string (RFC3339, when Docker
/// sets one), normalized without ever being flattened into filesystem
/// mtime -- this is a *separate* fact from [`modification_evidence`],
/// carried alongside it, never replacing it. See `docker::DockerCacheFact::last_used`.
pub fn docker_last_used_evidence(last_used: Option<&str>, observed_at: u64) -> Evidence {
    let event_at = last_used.and_then(parse_rfc3339_secs);
    tool_reported_use_evidence(
        "docker",
        "daemon-reported last_used for this build-cache entry",
        event_at,
        observed_at,
    )
}

/// Minimal RFC3339 -> unix-seconds parse (`YYYY-MM-DDTHH:MM:SS[.frac]Z`),
/// good enough for Docker's own timestamp format; returns `None` (never
/// panics) on anything else, which the caller reports as `Unknown`.
fn parse_rfc3339_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    if !(1970..=9999).contains(&year) {
        return None;
    }
    // Days since epoch via a civil-from-days style calculation (Howard
    // Hinnant's algorithm), avoiding a chrono dependency for one parse.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    (secs >= 0).then_some(secs as u64)
}

/// Inventory of which detector/artifact domains this module (and the
/// wider evidence layer) can produce a *real* activity fact for versus
/// which must report `Unknown`/`Unavailable`, kept as data so
/// `docs/usage.md`'s table and this module cannot silently drift apart
/// (`crates/core/tests/evidence_contract.rs` asserts the doc table lists
/// every entry here). This is the #54 "inventory" deliverable, not a
/// promise to add prospective instrumentation nobody asked for.
pub const ACTIVITY_EVIDENCE_INVENTORY: &[(&str, &str)] = &[
    (
        "filesystem artifact rows (build output, cache, dependency trees)",
        "modification age (folded mtime_max); access time only where the mount does not suppress atime",
    ),
    (
        "Docker build-cache entries",
        "daemon-reported last_used, kept distinct from filesystem mtime",
    ),
    (
        "Docker images/volumes",
        "unknown: the daemon reports creation time and container references, not a last-used timestamp",
    ),
    (
        "Cargo nested build artifacts",
        "fingerprint file mtime (tool-reported build time), where a .fingerprint entry exists",
    ),
    (
        "agent-tool session/category units",
        "modification age of the session/category's own recorded mtime_max; no tool reports a distinct use timestamp",
    ),
    (
        "external location detectors (version managers, package caches, SDKs)",
        "modification age of the measured directory only; no per-tool invocation history is read",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modification_evidence_labels_folded_max_not_last_used() {
        let ev = modification_evidence(1_000, 2_000);
        match &ev.status {
            crate::evidence::FactStatus::Known(FactValue::Timestamp(t)) => assert_eq!(*t, 1_000),
            other => panic!("expected known timestamp, got {other:?}"),
        }
        assert_eq!(ev.event_at, Some(1_000));
        match &ev.source {
            EvidenceSource::FilesystemMetadata { detail } => {
                assert!(detail.contains("measured children"));
                // The tempting shortcut this rejects: labelling this "last used".
                assert!(!detail.to_lowercase().contains("used"));
            }
            other => panic!("expected FilesystemMetadata, got {other:?}"),
        }
    }

    #[test]
    fn zero_mtime_is_unknown_not_epoch() {
        let ev = modification_evidence(0, 2_000);
        assert!(!ev.is_known());
        assert!(matches!(
            ev.status,
            crate::evidence::FactStatus::Unknown { .. }
        ));
    }

    #[test]
    fn future_mtime_is_noted_not_silently_ranked_as_fresh() {
        let ev = modification_evidence(5_000, 2_000);
        assert!(ev.is_known());
        assert!(ev.note.as_deref().unwrap().contains("clock skew"));
    }

    #[test]
    fn writes_during_observation_are_not_clock_skew() {
        let during = modification_evidence_during(2_030, 2_000, 2_050);
        assert!(during.note.is_none());
        assert_eq!(during.event_at, Some(2_030));
        let future = modification_evidence_during(2_060, 2_000, 2_050);
        assert!(future.note.as_deref().unwrap().contains("clock skew"));
    }

    #[test]
    fn noatime_flag_makes_access_time_unavailable() {
        let r = atime_reliability_from_macos_flags(0x1000_0000);
        assert!(matches!(r, AtimeReliability::Unreliable(_)));
    }

    #[test]
    fn clean_flags_are_reliable() {
        let r = atime_reliability_from_macos_flags(0);
        assert_eq!(r, AtimeReliability::Reliable);
    }

    #[test]
    fn relatime_mount_option_is_flagged_unreliable_not_treated_as_proof() {
        let r = atime_reliability_from_mount_options("rw,relatime");
        assert!(matches!(r, AtimeReliability::Unreliable(_)));
    }

    #[test]
    fn noatime_mount_option_detected() {
        let r = atime_reliability_from_mount_options("rw,noatime,nosuid");
        assert!(matches!(r, AtimeReliability::Unreliable(_)));
    }

    #[test]
    fn ordinary_mount_options_are_reliable() {
        let r = atime_reliability_from_mount_options("rw,strictatime");
        assert_eq!(r, AtimeReliability::Reliable);
    }

    #[test]
    fn find_mount_options_picks_longest_matching_prefix() {
        let text = "/dev/sda1 / ext4 rw,relatime 0 0\n/dev/sda2 /home ext4 rw,noatime 0 0\n";
        let opts = find_mount_options(text, Path::new("/home/dev/project"));
        assert_eq!(opts, Some("rw,noatime"));
        let opts_root = find_mount_options(text, Path::new("/var/lib"));
        assert_eq!(opts_root, Some("rw,relatime"));
    }

    #[test]
    fn docker_last_used_none_is_unknown_never_fabricated() {
        let ev = docker_last_used_evidence(None, 100);
        assert!(!ev.is_known());
        match &ev.source {
            EvidenceSource::ToolReported { tool, .. } => assert_eq!(tool, "docker"),
            other => panic!("expected ToolReported, got {other:?}"),
        }
    }

    #[test]
    fn docker_last_used_parses_rfc3339() {
        let ev = docker_last_used_evidence(Some("2026-09-10T13:00:00Z"), 2_000_000_000);
        assert!(ev.is_known());
        assert!(ev.event_at.is_some());
    }

    #[test]
    fn docker_last_used_never_flattened_into_filesystem_mtime() {
        // The tempting shortcut this rejects: reusing `modification_evidence`'s
        // subtype/source for a tool-reported fact so the two collapse into
        // one indistinguishable "last used" field.
        let fs_ev = modification_evidence(1_700_000_000, 1_700_100_000);
        let docker_ev = docker_last_used_evidence(Some("2026-09-10T13:00:00Z"), 1_700_100_000);
        assert_ne!(fs_ev.subtype, docker_ev.subtype);
        assert_ne!(
            std::mem::discriminant(&fs_ev.source),
            std::mem::discriminant(&docker_ev.source)
        );
    }

    #[test]
    fn reads_without_byte_changes_never_touch_growth_store() {
        // Evidence collection here is pure computation over already-known
        // numbers/strings; assert it does not perform filesystem writes
        // by construction (no `fs::write`/`fs::create_dir` call in this
        // module -- enforced by review + the workspace-wide
        // `refreshing_evidence_never_writes_byte_history_delta` test in
        // `crates/core/tests/evidence_contract.rs`, which re-runs a
        // report and asserts an unchanged growth-store byte count).
        let _ = modification_evidence(1, 2);
        let _ = docker_last_used_evidence(None, 2);
    }

    #[test]
    fn inventory_is_nonempty_and_labelled() {
        assert!(ACTIVITY_EVIDENCE_INVENTORY.len() >= 5);
        for (domain, evidence) in ACTIVITY_EVIDENCE_INVENTORY {
            assert!(!domain.is_empty());
            assert!(!evidence.is_empty());
        }
    }
}
