//! The detail pane under the table, in plain words: what the selected row
//! is, how big, what getting it back costs, and only the evidence that
//! changes a decision. The full evidence contract (sources, coverage,
//! freshness) stays in `swamp report` and `--json`; the pane names the
//! facts a person acts on and keeps the honest caveat short.

use crate::model::{Row, age_label, human_bytes};
use swamp_core::evidence::{Evidence, FactKind, FactStatus, FactValue};
use swamp_core::report::ArtifactKind;

/// A space estimate that could be off by less than this is not worth a
/// caveat line.
const CAVEAT_FROM_BYTES: u64 = 1 << 20;

/// What the row is and what rebuilding costs, in one sentence.
fn what_it_is(kind: &ArtifactKind) -> Option<&'static str> {
    Some(match kind {
        ArtifactKind::BuildOutput => {
            "Build output. Your build tool makes it again; the next build takes longer."
        }
        ArtifactKind::DependencyTree => {
            "Installed dependencies. Installing again brings them back, usually over the network."
        }
        ArtifactKind::Cache => "A cache. It fills up again as you work; the next run is slower.",
        ArtifactKind::Git => "Git history. Local commits that were never pushed cannot come back.",
        ArtifactKind::Source => "Tracked files. Recoverable from the remote only if all is pushed.",
        ArtifactKind::Ignored => {
            "Files git ignores. Generated ones rebuild; private ones cannot come back."
        }
        ArtifactKind::Untracked => "Files in no version control. Git cannot bring them back.",
        ArtifactKind::DockerImage => "A Docker image. Removed for good, no Trash; pull it again.",
        ArtifactKind::DockerBuildCache => {
            "Docker build cache. Removed for good; the next image build is slower."
        }
        ArtifactKind::DockerVolume => {
            "A Docker volume. Its data is removed for good and cannot be rebuilt."
        }
        ArtifactKind::Loose | ArtifactKind::Unknown => return None,
    })
}

fn priority(kind: FactKind) -> u8 {
    match kind {
        FactKind::CurrentUse => 0,
        FactKind::Recovery => 1,
        FactKind::Consumer => 2,
        FactKind::Activity => 3,
        FactKind::Reclaimability => 4,
    }
}

/// One plain sentence for a fact, or nothing when it would only be noise.
fn sentence(e: &Evidence) -> Option<String> {
    match (e.kind, &e.status) {
        (FactKind::CurrentUse, FactStatus::Known(FactValue::Bool(true))) => {
            Some("In use right now.".into())
        }
        (FactKind::CurrentUse, FactStatus::Unavailable { .. } | FactStatus::Unknown { .. }) => {
            Some("Could not check whether it is in use right now.".into())
        }
        (FactKind::Recovery, FactStatus::Known(FactValue::Text(t)))
            if t == "PotentiallyUniqueLocalState" || t == "BackupDependent" =>
        {
            Some("May be the only copy: nothing else has this.".into())
        }
        (FactKind::Consumer, FactStatus::Known(FactValue::Text(who))) => {
            Some(format!("Used by: {who}"))
        }
        (FactKind::Consumer, FactStatus::Known(FactValue::List(who))) => {
            Some(format!("Projects: {}", who.join(", ")))
        }
        (FactKind::Activity, FactStatus::Known(FactValue::Timestamp(t))) => {
            let age = age_label(Some(e.observed_at.saturating_sub(*t)));
            Some(if age == "<1m" {
                "Changed just now.".to_string()
            } else {
                format!("Last changed {age} ago.")
            })
        }
        // Freed space can differ from the size shown when files are shared
        // with other copies. Say so only where the gap is big enough to
        // change a choice.
        (FactKind::Reclaimability, FactStatus::Conflicting { candidates, .. }) => {
            let sizes: Vec<u64> = candidates
                .iter()
                .filter_map(|c| match c {
                    FactValue::Bytes(b) => Some(*b),
                    _ => None,
                })
                .collect();
            let (lo, hi) = (sizes.iter().min()?, sizes.iter().max()?);
            (*hi >= CAVEAT_FROM_BYTES && lo < hi).then(|| {
                format!(
                    "Freed space is between {} and {}: files shared with other copies count once.",
                    human_bytes(*lo),
                    human_bytes(*hi)
                )
            })
        }
        _ => None,
    }
}

/// Short name for a fact the evidence could not establish.
fn unknown_name(kind: FactKind) -> &'static str {
    match kind {
        FactKind::Activity => "last use",
        FactKind::Consumer => "who uses it",
        FactKind::CurrentUse => "whether it is in use",
        FactKind::Recovery => "how to get it back",
        FactKind::Reclaimability => "space freed",
    }
}

/// The pane's lines for `row`, most decision-relevant first. `sharing` is
/// the row's shared-files lines (already worded by the report). Facts the
/// evidence could not establish are named in one line, never left out: a
/// missing fact must not read as "nothing to worry about".
pub fn lines(row: &Row, sharing: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut facts: Vec<&Evidence> = row.evidence.iter().collect();
    facts.sort_by_key(|e| priority(e.kind));
    out.extend(
        facts
            .iter()
            .copied()
            .filter(|e| e.kind == FactKind::CurrentUse)
            .filter_map(sentence),
    );
    for kind in [FactKind::Recovery, FactKind::Consumer] {
        out.extend(
            facts
                .iter()
                .copied()
                .filter(|e| e.kind == kind)
                .filter_map(sentence),
        );
    }
    out.extend(
        facts
            .iter()
            .copied()
            .filter(|e| e.kind == FactKind::Reclaimability)
            .filter_map(sentence),
    );
    out.extend(sharing.iter().cloned());
    // Enrichment has its own line, rather than competing with the item name.
    out.extend(
        row.detail_lines
            .iter()
            .filter(|line| line.starts_with("Model:"))
            .cloned(),
    );
    // Recorded use belongs before bookkeeping and generic advice.
    out.extend(row.last_used.iter().cloned());
    out.extend(
        row.detail_lines
            .iter()
            .filter(|line| line.starts_with("Storage:"))
            .cloned(),
    );
    let mut unknown: Vec<&str> = Vec::new();
    for e in &facts {
        if matches!(
            e.status,
            FactStatus::Unknown { .. } | FactStatus::Unavailable { .. }
        ) && e.kind != FactKind::CurrentUse
        {
            let n = unknown_name(e.kind);
            if !unknown.contains(&n) {
                unknown.push(n);
            }
        }
    }
    if !unknown.is_empty() {
        out.push(format!("Unknown: {}.", unknown.join(" · ")));
    }
    out.extend(
        row.detail_lines
            .iter()
            .filter(|line| line.starts_with("Removal:"))
            .cloned(),
    );
    if !row.signals.is_empty() {
        out.extend(
            row.signals
                .iter()
                .filter(|line| {
                    !(row.last_used.is_some()
                        && (line.starts_with("last used ") || line.starts_with("last read ")))
                })
                .cloned(),
        );
    } else if let Some(text) = row.kind.as_ref().and_then(what_it_is) {
        out.push(text.to_string());
    }
    out.extend(
        row.detail_lines
            .iter()
            .filter(|line| line.starts_with("what it is:"))
            .cloned(),
    );
    if let Some(unit) = &row.unit {
        let label = match row.kind.as_ref() {
            Some(
                ArtifactKind::DockerImage
                | ArtifactKind::DockerBuildCache
                | ArtifactKind::DockerVolume,
            ) => "Object ID",
            _ => "Path",
        };
        out.push(format!("{label}: {}", unit.0));
    }
    out.extend(
        row.detail_lines
            .iter()
            .filter(|line| {
                !["Removal:", "what it is:", "Model:", "Storage:"]
                    .iter()
                    .any(|prefix| line.starts_with(prefix))
            })
            .cloned(),
    );
    out.extend(
        facts
            .iter()
            .copied()
            .filter(|e| e.kind == FactKind::Activity)
            .filter_map(sentence),
    );
    let mut seen = std::collections::HashSet::new();
    out.retain(|line| seen.insert(line.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use swamp_core::evidence::{EvidenceSource, FactSubtype, Reason};

    fn conflicting(lo: u64, hi: u64) -> Evidence {
        Evidence::conflicting(
            FactKind::Reclaimability,
            FactSubtype::EstimatedReclaimable,
            vec![FactValue::Bytes(lo), FactValue::Bytes(hi)],
            EvidenceSource::Inferred {
                basis: "test".into(),
            },
            10,
            Reason::fixed("clone/snapshot sharing is not queried"),
        )
    }

    #[test]
    fn the_caveat_appears_only_where_the_gap_matters() {
        assert!(sentence(&conflicting(0, 24_600)).is_none());
        let big = sentence(&conflicting(0, 5 << 20)).unwrap();
        assert!(big.contains("between 0B and 5.2MB"), "{big}");
        assert!(!big.contains("APFS"), "{big}");
    }

    #[test]
    fn facts_are_plain_sentences_in_decision_order() {
        let now = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::Process,
            FactValue::Bool(true),
            EvidenceSource::Inferred { basis: "t".into() },
            1_000_000,
        );
        let when = Evidence::known(
            FactKind::Activity,
            FactSubtype::Modified,
            FactValue::Timestamp(1_000_000 - 3 * 86_400),
            EvidenceSource::Inferred { basis: "t".into() },
            1_000_000,
        );
        let mut row = Row::leaf(0, "x".into(), 0, None);
        row.evidence = vec![when, now];
        let l = lines(&row, &[]);
        assert_eq!(l, vec!["In use right now.", "Last changed 3d ago."]);
    }

    #[test]
    fn selected_unit_keeps_its_exact_path_without_repeating_size() {
        let mut row = Row::leaf(0, "Cargo cache".into(), 2_500_000_000, None);
        row.unit = Some(crate::units::UnitId("/Users/me/.cargo/registry".into()));
        row.signals = vec!["No declared consumers".into()];
        let rendered = lines(&row, &[]);
        assert!(
            rendered
                .iter()
                .any(|line| line == "Path: /Users/me/.cargo/registry")
        );
        assert!(!rendered.iter().any(|line| line.contains("2.5GB")));
    }

    #[test]
    fn use_and_shared_bytes_precede_last_use_and_supporting_notes() {
        let current = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::Process,
            FactValue::Bool(true),
            EvidenceSource::Inferred {
                basis: "test".into(),
            },
            10,
        );
        let mut row = Row::leaf(0, "cache".into(), 0, None);
        row.last_used = Some("Last used: no record (source unavailable)".into());
        row.detail_lines = vec!["Regeneration source: registry".into()];
        row.evidence = vec![current];
        let rendered = lines(&row, &["Shared with another copy.".into()]);
        let position = |needle: &str| rendered.iter().position(|line| line == needle).unwrap();
        assert!(position("In use right now.") < position("Shared with another copy."));
        assert!(
            position("Shared with another copy.")
                < position("Last used: no record (source unavailable)")
        );
        assert!(
            position("Last used: no record (source unavailable)")
                < position("Regeneration source: registry")
        );
    }

    #[test]
    fn the_first_four_detail_lines_keep_the_decision_facts_visible() {
        let current = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::Process,
            FactValue::Bool(true),
            EvidenceSource::Inferred {
                basis: "test".into(),
            },
            10,
        );
        let recovery = Evidence::known(
            FactKind::Recovery,
            FactSubtype::PotentiallyUniqueLocalState,
            FactValue::Text("PotentiallyUniqueLocalState".into()),
            EvidenceSource::Inferred {
                basis: "test".into(),
            },
            10,
        );
        let unknown = Evidence::unknown(
            FactKind::Activity,
            FactSubtype::Modified,
            EvidenceSource::Inferred {
                basis: "test".into(),
            },
            10,
            Reason::fixed("no reliable timestamp"),
        );
        let mut row = Row::leaf(0, "Cargo cache".into(), 2_500_000_000, None);
        row.unit = Some(crate::units::UnitId("/Users/me/.cargo/registry".into()));
        row.signals = vec!["Rebuild takes longer".into()];
        row.evidence = vec![unknown, recovery, current];
        let rendered = lines(&row, &["Shared files count once.".into()]);
        assert_eq!(
            &rendered[..4],
            [
                "In use right now.",
                "May be the only copy: nothing else has this.",
                "Shared files count once.",
                "Unknown: last use."
            ]
        );
        assert_eq!(rendered[4], "Rebuild takes longer");
        assert!(rendered.contains(&"Path: /Users/me/.cargo/registry".to_string()));
    }

    #[test]
    fn a_short_detail_keeps_use_sharing_signal_and_path_in_the_visible_prefix() {
        let current = Evidence::known(
            FactKind::CurrentUse,
            FactSubtype::Process,
            FactValue::Bool(true),
            EvidenceSource::Inferred {
                basis: "test".into(),
            },
            10,
        );
        let mut row = Row::leaf(0, "Cargo home".into(), 0, None);
        row.unit = Some(crate::units::UnitId("/Users/me/.cargo".into()));
        row.signals = vec!["Can be fetched again".into()];
        row.evidence = vec![current];
        let rendered = lines(&row, &["Shared files count once.".into()]);
        assert_eq!(
            &rendered[..4],
            [
                "In use right now.",
                "Shared files count once.",
                "Can be fetched again",
                "Path: /Users/me/.cargo"
            ]
        );
    }

    #[test]
    fn enrichment_and_sourced_use_do_not_compete_with_duplicate_signals() {
        let mut row = Row::leaf(1, "org/model".into(), 1_900_000, None);
        row.unit = Some(crate::units::UnitId("/cache/models--org--model".into()));
        row.last_used = Some("Last used: Sep 29 (file access time)".into());
        row.signals = vec!["last used Sep 29 (file access time)".into()];
        row.detail_lines = vec![
            "Model: whisper · 241.7M params · float32".into(),
            "Storage: 968.9MB including shared blobs · Folder: 1.9MB".into(),
        ];
        let rendered = lines(&row, &[]);
        assert_eq!(rendered.len(), 4);
        assert!(rendered[0].starts_with("Model:"));
        assert!(rendered[1].starts_with("Last used:"));
        assert!(rendered[2].starts_with("Storage:"));
        assert!(rendered[3].starts_with("Path:"));
    }

    #[test]
    fn what_could_not_be_established_is_named_in_one_line() {
        let mut row = Row::leaf(0, "x".into(), 0, None);
        row.evidence = vec![Evidence::unknown(
            FactKind::Recovery,
            FactSubtype::UnknownPrerequisites,
            EvidenceSource::Inferred { basis: "t".into() },
            10,
            Reason::fixed("no lockfile"),
        )];
        assert_eq!(lines(&row, &[]), vec!["Unknown: how to get it back."]);
    }
}
